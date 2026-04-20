use super::*;

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(crate) fn lower_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        span: Span,
    ) -> MastExprKind {
        if let Some(asm_call) = self.maybe_lower_asm_call(callee, args, subst_map, span) {
            return asm_call;
        }

        let raw_callee_ty = self
            .ctx
            .node_types
            .get(&callee.id)
            .copied()
            .unwrap_or(TypeId::ERROR);

        let substituted_callee = self.substitute_type_with_map(raw_callee_ty, subst_map);
        let norm_callee = self.ctx.type_registry.normalize(substituted_callee);
        let expected_param_tys = self.measure_phase("            lower_call_signature", |this| {
            this.get_callee_expected_params(norm_callee)
        });
        let method_call = self.measure_phase("            lower_call_detect_method", |this| {
            this.detect_method_call(callee, subst_map)
        });
        let intrinsic_name = self.intrinsic_name_for_lowering(norm_callee);

        let arg_masts = self.measure_phase("            lower_call_args", |this| {
            let mut arg_masts = Vec::new();
            for (i, a) in args.iter().enumerate() {
                let param_idx = if method_call.is_some() { i + 1 } else { i };
                let exp_ty = if matches!(
                    intrinsic_name.as_deref(),
                    Some(
                        "@simdSplat"
                            | "@simdCast"
                            | "@simdBitcast"
                            | "@simdLowHalf"
                            | "@simdHighHalf"
                            | "@simdWithLowHalf"
                            | "@simdWithHighHalf"
                            | "@simdMaskedLoad"
                            | "@simdMaskedStore"
                            | "@simdMaskedGather"
                            | "@simdMaskedScatter"
                    )
                ) {
                    None
                } else {
                    expected_param_tys
                        .get(param_idx)
                        .copied()
                        .filter(|&ty| ty != TypeId::ERROR)
                };
                arg_masts.push(this.lower_expr(a, subst_map, exp_ty));
            }
            arg_masts
        });

        if let Some((callee_id, field, recv)) = method_call {
            self.measure_phase("            lower_call_method_dispatch", |this| {
                this.lower_method_call(
                    callee_id,
                    recv,
                    arg_masts,
                    subst_map,
                    MethodCallSite {
                        field,
                        norm_callee,
                        expected_self_ty: expected_param_tys.first().copied(),
                        span,
                    },
                )
            })
        } else {
            self.measure_phase("            lower_call_plain_dispatch", |this| {
                this.lower_normal_call(callee, args, arg_masts, subst_map)
            })
        }
    }

    pub(crate) fn lower_method_call(
        &mut self,
        callee_id: NodeId,
        recv: MastExpr,
        arg_masts: Vec<MastExpr>,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        call: MethodCallSite,
    ) -> MastExprKind {
        // Resolve methods against the type that actually owns the implementation.
        let norm_base = self.ctx.type_registry.normalize(recv.ty);

        // Trait objects are always fat pointers in Kern, so inspect the pointee rather than the outer pointer.
        let mut inner_ty = norm_base;
        if let TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } =
            self.ctx.type_registry.get(norm_base).clone()
        {
            inner_ty = elem;
        }

        let owner_trait_ty = self
            .ctx
            .trait_method_owners
            .get(&callee_id)
            .copied()
            .unwrap_or(inner_ty);
        let owner_trait_ty = self.substitute_type_with_map(owner_trait_ty, subst_map);

        self.lower_resolved_trait_method_call(recv, arg_masts, owner_trait_ty, call)
    }

    pub(crate) fn lower_resolved_trait_method_call(
        &mut self,
        recv: MastExpr,
        mut arg_masts: Vec<MastExpr>,
        owner_trait_ty: TypeId,
        call: MethodCallSite,
    ) -> MastExprKind {
        let norm_base = self.ctx.type_registry.normalize(recv.ty);
        let mut inner_ty = norm_base;
        if let TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } =
            self.ctx.type_registry.get(norm_base).clone()
        {
            inner_ty = elem;
        }

        let field_name = self.ctx.resolve(call.field).to_string();
        if field_name == "eq"
            && self.builtin_trait_name(owner_trait_ty).as_deref() == Some("Eq")
            && arg_masts.len() == 1
            && self.is_pure_enum_value_type(recv.ty)
            && arg_masts[0].ty == recv.ty
        {
            return MastExprKind::Binary {
                op: ast::BinaryOperator::Equal,
                lhs: Box::new(recv),
                rhs: Box::new(arg_masts.remove(0)),
            };
        }

        // 2. Choose dynamic (vtable) or static dispatch based on the recovered type.
        if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(inner_ty) {
            // Hand the full fat pointer to the dynamic dispatcher so it can extract the vtable.
            self.measure_phase("              lower_call_dynamic_dispatch", |this| {
                this.lower_dynamic_method_dispatch(
                    recv,
                    arg_masts,
                    DynamicDispatchCall {
                        field: call.field,
                        recv_trait_ty: inner_ty,
                        owner_trait_ty,
                        norm_callee: call.norm_callee,
                        span: call.span,
                    },
                )
            })
        } else if let TypeKind::FnDef(method_id, generics) =
            self.ctx.type_registry.get(call.norm_callee).clone()
        {
            if let Def::Function(func) = &self.ctx.defs[method_id.0 as usize]
                && func.is_intrinsic
            {
                arg_masts.insert(0, recv.clone());
                if let Some(kind) = self.lower_builtin_operator_intrinsic(method_id, &mut arg_masts)
                {
                    return kind;
                }
            }
            self.measure_phase("              lower_call_static_dispatch", |this| {
                this.lower_static_method_dispatch(recv, arg_masts, method_id, &generics, call)
            })
        } else {
            // A plain `TypeKind::Function` here means Sema only knew a generic bound.
            // After monomorphization, find the concrete impl globally.
            let mut target_func_id = None;
            let mut resolved_impl_args = Vec::new();
            let mut resolved_self_ty = None;
            let owner_trait_norm = self.ctx.type_registry.normalize(owner_trait_ty);
            let owner_trait_filter = !self.type_contains_generic_placeholders(owner_trait_ty)
                && matches!(
                    self.ctx.type_registry.get(owner_trait_norm),
                    TypeKind::TraitObject(..)
                );

            let method_ids_ptr = self
                .ctx
                .impl_methods_by_name
                .get(&call.field)
                .map(|method_ids| std::ptr::from_ref(method_ids.as_slice()));

            if let Some(method_ids_ptr) = method_ids_ptr {
                self.measure_phase("              lower_call_bound_impl_lookup", |this| {
                    // Safety: method-name indexes are immutable while lowering reads semantic defs.
                    let method_ids = unsafe { &*method_ids_ptr };
                    let receiver_search_tys = this.receiver_search_types(norm_base);
                    let mut best_match: Option<(
                        DefId,
                        DefId,
                        Option<TypeId>,
                        Vec<kernc_sema::ty::GenericArg>,
                    )> = None;
                    for &method_id in method_ids {
                        let Some(impl_id) =
                            this.ctx
                                .defs
                                .get(method_id.0 as usize)
                                .and_then(|def| match def {
                                    Def::Function(function) => function.parent,
                                    _ => None,
                                })
                        else {
                            continue;
                        };

                        let Some(impl_ptr) =
                            this.ctx
                                .defs
                                .get(impl_id.0 as usize)
                                .and_then(|def| match def {
                                    Def::Impl(impl_def) => Some(std::ptr::from_ref(impl_def)),
                                    _ => None,
                                })
                        else {
                            continue;
                        };

                        // Safety: lowering only reads semantic definition storage.
                        let _impl_def = unsafe { &*impl_ptr };
                        let mut matched_receiver_ty = None;
                        let mut candidate_impl_args = None;
                        for search_ty in receiver_search_tys.iter().copied() {
                            if let Some(args) = MemberQuery::new(this.ctx)
                                .resolve_impl_applicability_for_type(search_ty, impl_id)
                            {
                                matched_receiver_ty = Some(search_ty);
                                candidate_impl_args = Some(args);
                                break;
                            }
                        }
                        let Some(matched_receiver_ty) = matched_receiver_ty else {
                            continue;
                        };
                        let Some(mut candidate_impl_args) = candidate_impl_args else {
                            continue;
                        };

                        if owner_trait_filter {
                            let Some(resolved_args) =
                                kernc_sema::query::resolve_trait_impl_obligation(
                                    this.ctx,
                                    matched_receiver_ty,
                                    owner_trait_norm,
                                    impl_id,
                                )
                            else {
                                continue;
                            };
                            candidate_impl_args = resolved_args;
                        }

                        let replace = match best_match.as_ref() {
                            None => true,
                            Some((selected_impl_id, ..)) => matches!(
                                kernc_sema::query::compare_impl_specificity(
                                    this.ctx,
                                    impl_id,
                                    *selected_impl_id,
                                ),
                                kernc_sema::query::ImplSpecificity::LeftMoreSpecific
                            ),
                        };
                        if replace {
                            best_match = Some((
                                impl_id,
                                method_id,
                                Some(matched_receiver_ty),
                                candidate_impl_args,
                            ));
                        }
                    }

                    if let Some((_, method_id, matched_receiver_ty, candidate_impl_args)) =
                        best_match
                    {
                        resolved_impl_args = candidate_impl_args;
                        resolved_self_ty = matched_receiver_ty;
                        target_func_id = Some(method_id);
                    }
                });
            }

            if let Some(func_id) = target_func_id {
                let mut final_recv = recv;

                // Normalize pointer-type differences for LLVM by inserting a bitcast after safe downgrades.
                let expected_self_ty = resolved_self_ty.or(call.expected_self_ty);
                if let Some(exp_self) = expected_self_ty
                    && final_recv.ty != exp_self
                {
                    final_recv = MastExpr::new(
                        exp_self,
                        MastExprKind::Cast {
                            kind: MastCastKind::Bitcast,
                            operand: Box::new(final_recv),
                        },
                        call.span,
                    );
                }

                if let Def::Function(func) = &self.ctx.defs[func_id.0 as usize]
                    && func.is_intrinsic
                {
                    arg_masts.insert(0, final_recv.clone());
                    if let Some(kind) =
                        self.lower_builtin_operator_intrinsic(func_id, &mut arg_masts)
                    {
                        return kind;
                    }
                }

                arg_masts.insert(0, final_recv);
                let mono_id = self.instantiate_function_at(func_id, &resolved_impl_args, call.span);
                let func_ref =
                    MastExpr::new(call.norm_callee, MastExprKind::FuncRef(mono_id), call.span);
                MastExprKind::Call {
                    callee: Box::new(func_ref),
                    args: arg_masts,
                }
            } else {
                let type_name = self.ctx.ty_to_string(norm_base);
                let field_name = self.ctx.resolve(call.field);
                self.ctx.emit_ice(
                    call.span,
                    format!(
                        "Kern ICE (Lowering): failed to devirtualize static trait method `{}` for exact type `{}`.",
                        field_name, type_name
                    ),
                );
                MastExprKind::Trap
            }
        }
    }

    /// Helper: build a statically dispatched method call.
    pub(crate) fn lower_static_method_dispatch(
        &mut self,
        mut recv: MastExpr,
        mut arg_masts: Vec<MastExpr>,
        method_id: DefId,
        generics: &[kernc_sema::ty::GenericArg],
        call: MethodCallSite,
    ) -> MastExprKind {
        recv = self.measure_phase("                lower_call_static_recv", |_this| {
            if let Some(exp_self) = call.expected_self_ty
                && recv.ty != exp_self
            {
                MastExpr::new(
                    exp_self,
                    MastExprKind::Cast {
                        kind: MastCastKind::Bitcast,
                        operand: Box::new(recv),
                    },
                    call.span,
                )
            } else {
                recv
            }
        });

        self.measure_phase("                lower_call_static_args", |_this| {
            arg_masts.insert(0, recv);
        });
        let func_id = self.measure_phase("                lower_call_static_instantiate", |this| {
            this.instantiate_function_at(method_id, generics, call.span)
        });
        self.measure_phase("                lower_call_static_build", |_this| {
            let func_ref =
                MastExpr::new(call.norm_callee, MastExprKind::FuncRef(func_id), call.span);
            MastExprKind::Call {
                callee: Box::new(func_ref),
                args: arg_masts,
            }
        })
    }

    fn lower_plain_fn_call(
        &mut self,
        callee_mast: MastExpr,
        args: &[Expr],
        mut arg_masts: Vec<MastExpr>,
        fn_id: DefId,
        fn_args: Vec<kernc_sema::ty::GenericArg>,
        span: Span,
    ) -> MastExprKind {
        if let Some(intrinsic) = self
            .measure_phase("              lower_call_plain_intrinsic", |this| {
                this.lower_intrinsic_call(fn_id, callee_mast.ty, args, &mut arg_masts, span)
            })
        {
            return intrinsic;
        }

        let mono_id = self.measure_phase("              lower_call_plain_instantiate", |this| {
            this.instantiate_function_at(fn_id, &fn_args, span)
        });
        self.measure_phase("              lower_call_plain_build", |_this| {
            let func_ref = MastExpr::new(callee_mast.ty, MastExprKind::FuncRef(mono_id), span);
            MastExprKind::Call {
                callee: Box::new(func_ref),
                args: arg_masts,
            }
        })
    }

    fn lower_plain_direct_call(
        &mut self,
        callee_mast: MastExpr,
        arg_masts: Vec<MastExpr>,
    ) -> MastExprKind {
        self.measure_phase("              lower_call_plain_direct", |_this| {
            MastExprKind::Call {
                callee: Box::new(callee_mast),
                args: arg_masts,
            }
        })
    }

    fn lower_plain_closure_call(
        &mut self,
        callee_mast: MastExpr,
        arg_masts: Vec<MastExpr>,
        inner_norm: TypeId,
        span: Span,
    ) -> MastExprKind {
        self.measure_phase("              lower_call_plain_closure", |this| {
            this.lower_closure_call(callee_mast, arg_masts, inner_norm, span)
        })
    }

    fn lower_plain_callee(
        &mut self,
        callee: &Expr,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
    ) -> MastExpr {
        self.measure_phase("              lower_call_plain_callee", |this| {
            this.lower_expr(callee, subst_map, None)
        })
    }

    /// Helper: build a dynamically dispatched method call by loading from the vtable.
    pub(crate) fn lower_dynamic_method_dispatch(
        &mut self,
        recv: MastExpr,
        mut arg_masts: Vec<MastExpr>,
        call: DynamicDispatchCall,
    ) -> MastExprKind {
        let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::VOID,
        });

        // Data pointer passed as the method's `self`.
        let data_ptr = MastExpr::new(
            void_ptr_ty,
            MastExprKind::ExtractFatPtrData(Box::new(recv.clone())),
            call.span,
        );
        arg_masts.insert(0, data_ptr);

        // Extract and cast the vtable pointer.
        let vtable_meta = MastExpr::new(
            TypeId::USIZE,
            MastExprKind::ExtractFatPtrMeta(Box::new(recv)),
            call.span,
        );
        let vtable_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: void_ptr_ty,
        });

        let vtable_ptr = MastExpr::new(
            vtable_ptr_ty,
            MastExprKind::Cast {
                kind: MastCastKind::IntToPtr,
                operand: Box::new(vtable_meta),
            },
            call.span,
        );

        let recv_trait_norm = self.ctx.type_registry.normalize(call.recv_trait_ty);
        let owner_trait_norm = self.ctx.type_registry.normalize(call.owner_trait_ty);

        let owner_vtable_ptr = if owner_trait_norm == recv_trait_norm {
            vtable_ptr
        } else {
            let Some(super_slot) = self.vtable_supertrait_slot(recv_trait_norm, owner_trait_norm)
            else {
                self.ctx.emit_ice(
                    call.span,
                    format!(
                        "Kern ICE (Lowering): trait `{}` is not a supertrait of `{}` during dynamic dispatch.",
                        self.ctx.ty_to_string(owner_trait_norm),
                        self.ctx.ty_to_string(recv_trait_norm)
                    ),
                );
                return MastExprKind::Trap;
            };

            let super_vtable_raw = MastExpr::new(
                void_ptr_ty,
                MastExprKind::IndexAccess {
                    lhs: Box::new(vtable_ptr),
                    index: Box::new(MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::Integer(super_slot as u128),
                        call.span,
                    )),
                },
                call.span,
            );

            MastExpr::new(
                vtable_ptr_ty,
                MastExprKind::Cast {
                    kind: MastCastKind::Bitcast,
                    operand: Box::new(super_vtable_raw),
                },
                call.span,
            )
        };

        let Some(vtable_idx) = self.direct_trait_method_slot(owner_trait_norm, call.field) else {
            self.ctx.emit_ice(
                call.span,
                format!(
                    "Kern ICE (Lowering): method `{}` not found in owner trait `{}`.",
                    self.ctx.resolve(call.field),
                    self.ctx.ty_to_string(owner_trait_norm),
                ),
            );
            return MastExprKind::Trap;
        };

        // Load the function pointer from the vtable slot.
        let func_ptr = MastExpr::new(
            void_ptr_ty,
            MastExprKind::IndexAccess {
                lhs: Box::new(owner_vtable_ptr),
                index: Box::new(MastExpr::new(
                    TypeId::USIZE,
                    MastExprKind::Integer(vtable_idx as u128),
                    call.span,
                )),
            },
            call.span,
        );

        // Rebuild the exact callable signature.
        let (ret_ty, is_variadic, mut patched_params) = if let TypeKind::Function {
            ret,
            is_variadic,
            params,
            ..
        } =
            self.ctx.type_registry.get(call.norm_callee)
        {
            (*ret, *is_variadic, params.clone())
        } else {
            self.ctx.emit_ice(
                call.span,
                "Kern ICE (Lowering): Callee type of dynamic method dispatch is not a Function.",
            );
            return MastExprKind::Trap;
        };

        if !patched_params.is_empty() {
            patched_params[0] = void_ptr_ty;
        }

        let patched_fn_ty = self.ctx.type_registry.intern(TypeKind::Function {
            params: patched_params,
            ret: ret_ty,
            is_variadic,
        });

        let func_ptr_typed = MastExpr::new(
            patched_fn_ty,
            MastExprKind::Cast {
                kind: MastCastKind::Bitcast,
                operand: Box::new(func_ptr),
            },
            call.span,
        );

        MastExprKind::Call {
            callee: Box::new(func_ptr_typed),
            args: arg_masts,
        }
    }

    pub(crate) fn lower_normal_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        arg_masts: Vec<MastExpr>,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
    ) -> MastExprKind {
        let callee_mast = self.lower_plain_callee(callee, subst_map);
        let norm_callee = self.ctx.type_registry.normalize(callee_mast.ty);

        // Intercept dynamic calls through closure fat pointers.
        if let TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } =
            self.ctx.type_registry.get(norm_callee).clone()
        {
            let inner_norm = self.ctx.type_registry.normalize(elem);
            if matches!(
                self.ctx.type_registry.get(inner_norm),
                TypeKind::ClosureInterface { .. }
            ) {
                return self.lower_plain_closure_call(
                    callee_mast,
                    arg_masts,
                    inner_norm,
                    callee.span,
                );
            }
        }

        if let TypeKind::FnDef(fn_id, fn_args) = self.ctx.type_registry.get(callee_mast.ty).clone()
        {
            self.lower_plain_fn_call(callee_mast, args, arg_masts, fn_id, fn_args, callee.span)
        } else {
            self.lower_plain_direct_call(callee_mast, arg_masts)
        }
    }

    pub(crate) fn lower_generic_instantiation(
        &mut self,
        concrete_ty: TypeId,
        span: Span,
    ) -> MastExprKind {
        let fn_info =
            if let TypeKind::FnDef(fn_id, fn_args) = self.ctx.type_registry.get(concrete_ty) {
                Some((*fn_id, fn_args.clone()))
            } else {
                None
            };
        if let Some((fn_id, fn_args)) = fn_info {
            let mono_id = self.instantiate_function_at(fn_id, &fn_args, span);
            MastExprKind::FuncRef(mono_id)
        } else {
            MastExprKind::Integer(0)
        }
    }

    pub(crate) fn get_callee_expected_params(&mut self, norm_callee: TypeId) -> Vec<TypeId> {
        match self.ctx.type_registry.get(norm_callee).clone() {
            TypeKind::Function { params, .. } => params,
            TypeKind::FnDef(def_id, gen_args) => {
                if let Def::Function(f) = &self.ctx.defs[def_id.0 as usize] {
                    if let Some(sig) = f.resolved_sig {
                        let norm_sig = self.ctx.type_registry.normalize(sig);
                        let raw_params = if let TypeKind::Function { params, .. } =
                            self.ctx.type_registry.get(norm_sig).clone()
                        {
                            params
                        } else {
                            Vec::new()
                        };

                        let all_generic_params = f.generics.clone();

                        let mut sig_subst_map = HashMap::new();
                        for (idx, param) in all_generic_params.iter().enumerate() {
                            if idx < gen_args.len() {
                                sig_subst_map.insert(param.name, gen_args[idx]);
                            }
                        }

                        raw_params
                            .into_iter()
                            .map(|p| self.substitute_type_with_map(p, &sig_subst_map))
                            .collect()
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }

    pub(crate) fn lower_closure_call(
        &mut self,
        callee_mast: MastExpr,
        mut arg_masts: Vec<MastExpr>,
        closure_interface_ty: TypeId,
        span: Span,
    ) -> MastExprKind {
        let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::VOID,
        });

        // 1. Extract `data_ptr` and inject it as argument 0.
        let data_ptr = MastExpr::new(
            void_ptr_ty,
            MastExprKind::ExtractFatPtrData(Box::new(callee_mast.clone())),
            span,
        );
        arg_masts.insert(0, data_ptr);

        // 2. Extract `meta_ptr`, which stores the code pointer.
        let code_ptr = MastExpr::new(
            TypeId::USIZE,
            MastExprKind::ExtractFatPtrMeta(Box::new(callee_mast.clone())),
            span,
        );

        // 3. Build the exact lowered function signature and cast the `usize` code pointer to it.
        let (params, ret) = if let TypeKind::ClosureInterface { params, ret } =
            self.ctx.type_registry.get(closure_interface_ty).clone()
        {
            (params, ret)
        } else {
            let actual_ty_str = self.ctx.ty_to_string(closure_interface_ty);
            self.ctx.emit_ice(
                span,
                format!(
                    "Kern ICE (Lowering): Expected `ClosureInterface`, found `{}`.",
                    actual_ty_str
                ),
            );
            return MastExprKind::Trap;
        };

        let mut patched_params = params.clone();
        patched_params.insert(0, void_ptr_ty); // Prepend the hidden environment parameter.

        let patched_fn_ty = self.ctx.type_registry.intern(TypeKind::Function {
            params: patched_params,
            ret,
            is_variadic: false,
        });

        let typed_code_ptr = MastExpr::new(
            patched_fn_ty,
            MastExprKind::Cast {
                kind: MastCastKind::IntToPtr,
                operand: Box::new(code_ptr),
            },
            span,
        );

        // 4. Emit the indirect call through the function pointer.
        MastExprKind::Call {
            callee: Box::new(typed_code_ptr),
            args: arg_masts,
        }
    }
}
