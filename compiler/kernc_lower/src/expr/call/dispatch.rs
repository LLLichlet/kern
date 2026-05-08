use super::*;

struct PlainFnCallLowering<'a> {
    callee_mast: MastExpr,
    args: &'a [Expr],
    arg_masts: Vec<MastExpr>,
    fn_id: DefId,
    fn_args: Vec<kernc_sema::ty::GenericArg>,
    subst_map: &'a HashMap<SymbolId, kernc_sema::ty::GenericArg>,
    span: Span,
    result_ty: TypeId,
}

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    fn coerce_call_args_to_params(
        &mut self,
        arg_masts: Vec<MastExpr>,
        params: &[TypeId],
        param_offset: usize,
        span: Span,
    ) -> Vec<MastExpr> {
        arg_masts
            .into_iter()
            .enumerate()
            .map(|(idx, arg)| {
                let Some(expected) = params.get(idx + param_offset).copied() else {
                    return arg;
                };
                if expected == TypeId::ERROR || arg.ty == expected {
                    return arg;
                }
                self.apply_implicit_cast(arg.kind, arg.ty, expected, span)
            })
            .collect()
    }

    fn function_first_param_ty(
        &mut self,
        method_id: DefId,
        generics: &[kernc_sema::ty::GenericArg],
    ) -> Option<TypeId> {
        let Def::Function(function) = self.ctx.defs.get(method_id.0 as usize)?.clone() else {
            return None;
        };
        let raw_ty = if let Some(first_param) = function.params.first() {
            self.ctx.node_type(first_param.type_node.id)?
        } else if let Some(parent) = function.parent
            && let Some(Def::Impl(impl_def)) = self.ctx.defs.get(parent.0 as usize)
        {
            self.ctx.node_type(impl_def.target_type.id)?
        } else {
            return None;
        };
        let mut subst_map = HashMap::new();
        for (idx, param) in function.generics.iter().enumerate() {
            if idx < generics.len() {
                subst_map.insert(param.name, generics[idx]);
            }
        }
        Some(self.substitute_type_with_map(raw_ty, &subst_map))
    }

    fn resolve_bound_impl_method_target(
        &mut self,
        receiver_ty: TypeId,
        owner_trait_ty: TypeId,
        field: SymbolId,
    ) -> Option<(DefId, Option<TypeId>, Vec<kernc_sema::ty::GenericArg>)> {
        let receiver_norm = self.ctx.type_registry.normalize(receiver_ty);
        let owner_trait_norm = self.ctx.type_registry.normalize(owner_trait_ty);
        let cache_key = (receiver_norm, owner_trait_norm, field);
        if let Some(cached) = self.bound_impl_method_cache.get(&cache_key) {
            return cached.clone();
        }

        let owner_trait_filter = !self.type_contains_generic_placeholders(owner_trait_ty)
            && matches!(
                self.ctx.type_registry.get(owner_trait_norm),
                TypeKind::TraitObject(..)
            );
        let receiver_search_tys = self.receiver_search_types(receiver_norm);
        let method_ids_ptr = self
            .ctx
            .impl_index
            .impl_methods_by_name
            .get(&field)
            .map(|method_ids: &Vec<DefId>| std::ptr::from_ref(method_ids.as_slice()));

        let mut resolved = None;
        if let Some(method_ids_ptr) = method_ids_ptr {
            // Safety: lowering only reads the method-name index; semantic impl indexes stay
            // immutable during this pass.
            let method_ids = unsafe { &*method_ids_ptr };
            let mut best_match: Option<(
                DefId,
                DefId,
                Option<TypeId>,
                Vec<kernc_sema::ty::GenericArg>,
            )> = None;
            for &method_id in method_ids {
                let Some(impl_id) =
                    self.ctx
                        .defs
                        .get(method_id.0 as usize)
                        .and_then(|def| match def {
                            Def::Function(function) => function.parent,
                            _ => None,
                        })
                else {
                    continue;
                };

                let mut matched_receiver_ty = None;
                let mut candidate_impl_args = None;
                for search_ty in receiver_search_tys.iter().copied() {
                    let args = if owner_trait_filter {
                        kernc_sema::query::resolve_trait_impl_obligation(
                            self.ctx,
                            search_ty,
                            owner_trait_norm,
                            impl_id,
                        )
                    } else {
                        MemberQuery::new(self.ctx)
                            .resolve_impl_applicability_for_type(search_ty, impl_id)
                    };
                    if let Some(args) = args {
                        matched_receiver_ty = Some(search_ty);
                        candidate_impl_args = Some(args);
                        break;
                    }
                }
                let Some(matched_receiver_ty) = matched_receiver_ty else {
                    continue;
                };
                let Some(candidate_impl_args) = candidate_impl_args else {
                    continue;
                };

                let replace = match best_match.as_ref() {
                    None => true,
                    Some((selected_impl_id, ..)) => matches!(
                        kernc_sema::query::compare_impl_specificity(
                            self.ctx,
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

            resolved =
                best_match.map(|(_, method_id, matched_receiver_ty, candidate_impl_args)| {
                    (method_id, matched_receiver_ty, candidate_impl_args)
                });
        }

        self.bound_impl_method_cache
            .insert(cache_key, resolved.clone());
        resolved
    }

    pub(crate) fn lower_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        span: Span,
        result_ty: TypeId,
    ) -> MastExpr {
        if let Some(asm_call) = self.maybe_lower_asm_call(callee, args, subst_map, span) {
            return MastExpr::new(result_ty, asm_call, span);
        }

        let raw_callee_ty = self
            .ctx.node_type(callee.id)
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
                    this.ctx
                        .call_arg_expected_ty(a.id)
                        .map(|ty| this.substitute_type_with_map(ty, subst_map))
                        .or_else(|| expected_param_tys.get(param_idx).copied())
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
                        default_ret_ty: result_ty,
                        span,
                    },
                )
            })
        } else {
            let kind = self.measure_phase("            lower_call_plain_dispatch", |this| {
                this.lower_normal_call(callee, args, arg_masts, subst_map, result_ty)
            });
            MastExpr::new(result_ty, kind, span)
        }
    }

    pub(crate) fn lower_method_call(
        &mut self,
        callee_id: NodeId,
        mut recv: MastExpr,
        arg_masts: Vec<MastExpr>,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        call: MethodCallSite,
    ) -> MastExpr {
        let stored_owner_ty = self.ctx.method_owner_ty(callee_id);
        let expected_self_ty = call.expected_self_ty.or(stored_owner_ty).or_else(|| {
            self.get_callee_expected_params(call.norm_callee)
                .first()
                .copied()
        });
        if let Some(expected_self_ty) = expected_self_ty
            && recv.ty != expected_self_ty
        {
            recv = self.apply_implicit_cast(recv.kind, recv.ty, expected_self_ty, call.span);
        }

        // Resolve methods against the type that actually owns the implementation.
        let norm_base = self.ctx.type_registry.normalize(recv.ty);

        // Trait objects are always fat pointers in Kern, so inspect the pointee rather than the outer pointer.
        let mut inner_ty = norm_base;
        if let TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } =
            self.ctx.type_registry.get(norm_base).clone()
        {
            inner_ty = elem;
        }

        let owner_trait_ty = self.ctx.method_owner_ty(callee_id).unwrap_or(inner_ty);
        let owner_trait_ty = self.substitute_type_with_map(owner_trait_ty, subst_map);
        let owner_trait_ty = kernc_sema::query::retain_declared_trait_object_assoc_bindings(
            self.ctx,
            owner_trait_ty,
        );

        self.lower_resolved_trait_method_call(recv, arg_masts, owner_trait_ty, call)
    }

    pub(crate) fn lower_resolved_trait_method_call(
        &mut self,
        recv: MastExpr,
        mut arg_masts: Vec<MastExpr>,
        owner_trait_ty: TypeId,
        call: MethodCallSite,
    ) -> MastExpr {
        let norm_base = self.ctx.type_registry.normalize(recv.ty);
        let mut inner_ty = norm_base;
        if let TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } =
            self.ctx.type_registry.get(norm_base).clone()
        {
            inner_ty = elem;
        }
        if call.field == self.ctx.intern("eq")
            && self.is_builtin_trait_named(owner_trait_ty, "Eq")
            && arg_masts.len() == 1
            && self.is_pure_enum_value_type(recv.ty)
            && arg_masts[0].ty == recv.ty
        {
            return MastExpr::new(
                call.default_ret_ty,
                MastExprKind::Binary {
                    op: ast::BinaryOperator::Equal,
                    lhs: Box::new(recv),
                    rhs: Box::new(arg_masts.remove(0)),
                },
                call.span,
            );
        }

        // 2. Choose static dispatch first when Sema resolved an inherent impl method.
        if let TypeKind::FnDef(method_id, generics) =
            self.ctx.type_registry.get(call.norm_callee).clone()
        {
            if let Def::Function(func) = &self.ctx.defs[method_id.0 as usize]
                && func.is_intrinsic
            {
                arg_masts.insert(0, recv.clone());
                if let Some(kind) = self.lower_builtin_operator_intrinsic(method_id, &mut arg_masts)
                {
                    return MastExpr::new(call.default_ret_ty, kind, call.span);
                }
            }
            let kind = self.measure_phase("              lower_call_static_dispatch", |this| {
                this.lower_static_method_dispatch(recv, arg_masts, method_id, &generics, call)
            });
            MastExpr::new(call.default_ret_ty, kind, call.span)
        } else if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(inner_ty) {
            // Hand the full fat pointer to the dynamic dispatcher so it can extract the vtable.
            let kind = self.measure_phase("              lower_call_dynamic_dispatch", |this| {
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
            });
            MastExpr::new(call.default_ret_ty, kind, call.span)
        } else {
            // A plain `TypeKind::Function` here means Sema only knew a generic bound.
            // After monomorphization, find the concrete impl globally.
            let resolved_impl_target = self
                .measure_phase("              lower_call_bound_impl_lookup", |this| {
                    this.resolve_bound_impl_method_target(norm_base, owner_trait_ty, call.field)
                });

            if let Some((func_id, resolved_self_ty, resolved_impl_args)) = resolved_impl_target {
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
                        return MastExpr::new(call.default_ret_ty, kind, call.span);
                    }
                }

                arg_masts.insert(0, final_recv);
                let mono_id = self.instantiate_function_at(func_id, &resolved_impl_args, call.span);
                let callee_ty = self
                    .ctx
                    .type_registry
                    .intern(TypeKind::FnDef(func_id, resolved_impl_args.clone()));
                let ret_ty = self
                    .fn_like_signature(callee_ty, call.span)
                    .map(|(_, ret_ty)| ret_ty)
                    .unwrap_or(call.default_ret_ty);
                let func_ref = MastExpr::new(callee_ty, MastExprKind::FuncRef(mono_id), call.span);
                MastExpr::new(
                    ret_ty,
                    MastExprKind::Call {
                        callee: Box::new(func_ref),
                        args: arg_masts,
                    },
                    call.span,
                )
            } else {
                let type_name = self.ctx.ty_to_string(norm_base);
                let field_name = self.ctx.resolve(call.field);
                self.lower_error_expr(
                    call.default_ret_ty,
                    call.span,
                    format!(
                        "cannot resolve a concrete impl for trait method `{}` on exact type `{}` during lowering",
                        field_name, type_name
                    ),
                )
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
        let expected_self_ty = call
            .expected_self_ty
            .or_else(|| self.function_first_param_ty(method_id, generics));
        recv = self.measure_phase("                lower_call_static_recv", |this| {
            if let Some(exp_self) = expected_self_ty
                && recv.ty != exp_self
            {
                this.apply_implicit_cast(recv.kind, recv.ty, exp_self, call.span)
            } else {
                recv
            }
        });
        let expected_params = self.get_callee_expected_params(call.norm_callee);
        arg_masts = self.coerce_call_args_to_params(arg_masts, &expected_params, 1, call.span);

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

    fn lower_plain_fn_call(&mut self, mut call: PlainFnCallLowering<'_>) -> MastExprKind {
        if let Some(intrinsic) =
            self.measure_phase("              lower_call_plain_intrinsic", |this| {
                this.lower_intrinsic_call(
                    call.fn_id,
                    call.callee_mast.ty,
                    call.result_ty,
                    call.args,
                    &mut call.arg_masts,
                    call.subst_map,
                    call.span,
                )
            })
        {
            return intrinsic;
        }

        let expected_params = self.get_callee_expected_params(call.callee_mast.ty);
        call.arg_masts =
            self.coerce_call_args_to_params(call.arg_masts, &expected_params, 0, call.span);

        let mono_id = self.measure_phase("              lower_call_plain_instantiate", |this| {
            this.instantiate_function_at(call.fn_id, &call.fn_args, call.span)
        });
        self.measure_phase("              lower_call_plain_build", |_this| {
            let func_ref = MastExpr::new(
                call.callee_mast.ty,
                MastExprKind::FuncRef(mono_id),
                call.span,
            );
            MastExprKind::Call {
                callee: Box::new(func_ref),
                args: call.arg_masts,
            }
        })
    }

    fn lower_plain_direct_call(
        &mut self,
        callee_mast: MastExpr,
        arg_masts: Vec<MastExpr>,
    ) -> MastExprKind {
        let expected_params = self.get_callee_expected_params(callee_mast.ty);
        let arg_masts =
            self.coerce_call_args_to_params(arg_masts, &expected_params, 0, callee_mast.span);
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

        let owner_vtable_ptr = if self
            .trait_object_satisfies_required(recv_trait_norm, owner_trait_norm)
        {
            vtable_ptr
        } else {
            let Some(super_slot) = self.vtable_supertrait_slot(recv_trait_norm, owner_trait_norm)
            else {
                return self.lower_error_kind(
                    call.span,
                    format!(
                        "cannot dynamically dispatch through trait `{}` because it is not a supertrait of `{}`",
                        self.ctx.ty_to_string(owner_trait_norm),
                        self.ctx.ty_to_string(recv_trait_norm)
                    ),
                );
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
            return self.lower_error_kind(
                call.span,
                format!(
                    "trait method `{}` not found in owner trait `{}` during dynamic dispatch",
                    self.ctx.resolve(call.field),
                    self.ctx.ty_to_string(owner_trait_norm),
                ),
            );
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
            return self.lower_error_kind(
                call.span,
                "cannot lower dynamic method dispatch because the recovered callee type is not a function",
            );
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
        result_ty: TypeId,
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
            self.lower_plain_fn_call(PlainFnCallLowering {
                callee_mast,
                args,
                arg_masts,
                fn_id,
                fn_args,
                subst_map,
                span: callee.span,
                result_ty,
            })
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
        if let Some(params) = self.callee_expected_params_cache.get(&norm_callee) {
            return params.clone();
        }

        let params = match self.ctx.type_registry.get(norm_callee).clone() {
            TypeKind::Function { params, .. } => params,
            TypeKind::FnDef(def_id, gen_args) => {
                if let Def::Function(f) = self.ctx.defs[def_id.0 as usize].clone() {
                    if let Some(sig) = f.resolved_sig {
                        let norm_sig = self.ctx.type_registry.normalize(sig);
                        let mut raw_params = if let TypeKind::Function { params, .. } =
                            self.ctx.type_registry.get(norm_sig).clone()
                        {
                            params
                        } else {
                            Vec::new()
                        };
                        if let Some(parent) = f.parent
                            && let Some(Def::Impl(impl_def)) = self.ctx.defs.get(parent.0 as usize)
                            && let Some(self_ty) = self
                                .ctx.node_type(impl_def.target_type.id)
                            && raw_params.first().is_none_or(|first| {
                                self.ctx.type_registry.normalize(*first)
                                    != self.ctx.type_registry.normalize(self_ty)
                            })
                        {
                            raw_params.insert(0, self_ty);
                        }

                        let mut sig_subst_map = HashMap::new();
                        for (idx, param) in f.generics.iter().enumerate() {
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
        };

        self.callee_expected_params_cache
            .insert(norm_callee, params.clone());
        params
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
            return self.lower_error_kind(
                span,
                format!(
                    "cannot lower closure call because callee does not have a closure interface type; found `{}`",
                    actual_ty_str
                ),
            );
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
