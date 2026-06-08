//! Direct, method, and dynamic-dispatch call lowering.
//!
//! This module decides whether a call is a plain function call, a statically
//! resolved method call, a member intrinsic, or trait-object dispatch, then
//! materializes the receiver/arguments and concrete call target.

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
    fn maybe_lower_member_intrinsic_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        span: Span,
        result_ty: TypeId,
    ) -> Option<MastExpr> {
        if !args.is_empty() {
            return None;
        }
        let ExprKind::FieldAccess { lhs, field, .. } = &callee.kind else {
            return None;
        };

        let name = self.ctx.resolve(*field).to_string();
        if !name.starts_with('@') {
            return None;
        }
        let recv = self.lower_expr(lhs, subst_map, None);
        let recv_norm = self.ctx.type_registry.normalize(recv.ty);

        let kind = match self.ctx.type_registry.get(recv_norm).clone() {
            TypeKind::Array { len, .. } if name == "@len" => {
                let Some(len) = self.const_generic_usize(len, lhs.span) else {
                    return Some(MastExpr::new(result_ty, MastExprKind::Trap, span));
                };
                MastExprKind::Integer(len as u128)
            }
            TypeKind::ArrayInfer { .. } if name == "@len" => MastExprKind::Trap,
            TypeKind::Array { .. } | TypeKind::ArrayInfer { .. } if name == "@ptr" => {
                MastExprKind::ExtractElementPtr(Box::new(recv))
            }
            TypeKind::Slice { .. } if name == "@len" => {
                MastExprKind::ExtractFatPtrMeta(Box::new(recv))
            }
            TypeKind::Slice { .. } if name == "@ptr" => {
                MastExprKind::ExtractFatPtrData(Box::new(recv))
            }
            TypeKind::Range { start, .. } if name == "@start" => {
                let Some(_) = start else {
                    return Some(MastExpr::new(result_ty, MastExprKind::Trap, span));
                };
                let field = self.ctx.intern("start");
                self.lower_field_access(lhs, field, subst_map, span)
            }
            TypeKind::Range { end, .. } if name == "@end" => {
                let Some(_) = end else {
                    return Some(MastExpr::new(result_ty, MastExprKind::Trap, span));
                };
                let field = self.ctx.intern("end");
                self.lower_field_access(lhs, field, subst_map, span)
            }
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let elem_norm = self.ctx.type_registry.normalize(elem);
                match self.ctx.type_registry.get(elem_norm) {
                    TypeKind::TraitObject(..) if name == "@dataPtr" => {
                        MastExprKind::ExtractFatPtrData(Box::new(recv))
                    }
                    TypeKind::TraitObject(..) if name == "@vtablePtr" => MastExprKind::Cast {
                        kind: MastCastKind::IntToPtr,
                        operand: Box::new(MastExpr::new(
                            TypeId::USIZE,
                            MastExprKind::ExtractFatPtrMeta(Box::new(recv)),
                            span,
                        )),
                    },
                    TypeKind::ClosureInterface { .. } if name == "@statePtr" => {
                        MastExprKind::ExtractFatPtrData(Box::new(recv))
                    }
                    TypeKind::ClosureInterface { .. } if name == "@entryPtr" => {
                        MastExprKind::Cast {
                            kind: MastCastKind::IntToPtr,
                            operand: Box::new(MastExpr::new(
                                TypeId::USIZE,
                                MastExprKind::ExtractFatPtrMeta(Box::new(recv)),
                                span,
                            )),
                        }
                    }
                    _ => return None,
                }
            }
            _ => return None,
        };

        Some(MastExpr::new(result_ty, kind, span))
    }

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

    fn function_self_generic_ty(
        &mut self,
        method_id: DefId,
        generics: &[kernc_sema::ty::GenericArg],
    ) -> Option<TypeId> {
        let Def::Function(function) = self.ctx.defs.get(method_id.0 as usize)? else {
            return None;
        };
        let info = function.default_trait_method.as_ref()?;
        function
            .generics
            .iter()
            .position(|param| param.name == info.self_param)
            .and_then(|index| generics.get(index))
            .and_then(|arg| arg.as_type())
    }

    fn method_self_param_ty(
        &mut self,
        method_id: DefId,
        generics: &[kernc_sema::ty::GenericArg],
    ) -> Option<TypeId> {
        let Def::Function(function) = self.ctx.defs.get(method_id.0 as usize)?.clone() else {
            return None;
        };
        let raw_ty = self.ctx.node_type(function.params.first()?.type_node.id)?;
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
        let mut resolved = None;
        let methods = self.ctx.impl_methods_named(field);
        if !methods.is_empty() {
            let mut best_match: Option<(
                DefId,
                DefId,
                Option<TypeId>,
                Vec<kernc_sema::ty::GenericArg>,
            )> = None;
            for method in methods {
                let mut matched_receiver_ty = None;
                let mut candidate_impl_args = None;
                for search_ty in receiver_search_tys.iter().copied() {
                    let args = if owner_trait_filter {
                        kernc_sema::query::resolve_trait_impl_obligation(
                            self.ctx,
                            search_ty,
                            owner_trait_norm,
                            method.impl_id,
                        )
                    } else {
                        MemberQuery::new(self.ctx)
                            .resolve_impl_applicability_for_type(search_ty, method.impl_id)
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
                            method.impl_id,
                            *selected_impl_id,
                        ),
                        kernc_sema::query::ImplSpecificity::LeftMoreSpecific
                    ),
                };
                if replace {
                    best_match = Some((
                        method.impl_id,
                        method.method_id,
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

        if resolved.is_none()
            && let Some(default_target) =
                self.resolve_trait_default_method_target(receiver_norm, owner_trait_norm, field)
        {
            resolved = Some(default_target);
        }

        self.bound_impl_method_cache
            .insert(cache_key, resolved.clone());
        resolved
    }

    fn resolve_trait_default_method_target(
        &mut self,
        receiver_norm: TypeId,
        owner_trait_norm: TypeId,
        field: SymbolId,
    ) -> Option<(DefId, Option<TypeId>, Vec<kernc_sema::ty::GenericArg>)> {
        let TypeKind::TraitObject(trait_def_id, trait_args, _) =
            self.ctx.type_registry.get(owner_trait_norm).clone()
        else {
            return None;
        };
        let trait_def = match self.ctx.defs.get(trait_def_id.0 as usize) {
            Some(Def::Trait(trait_def)) => trait_def.clone(),
            _ => return None,
        };
        let direct_method = trait_def
            .methods
            .iter()
            .find(|method| method.signature.name == field && method.default_impl.is_some());
        let (default_id, method_trait_args) = if let Some(method) = direct_method {
            (method.default_impl?, trait_args)
        } else {
            let resolution = MemberQuery::new(self.ctx).resolve_trait_method_in_hierarchy(
                trait_def_id,
                kernc_sema::query::TraitMethodLookup {
                    trait_args: &trait_args,
                    assoc_bindings: &[],
                    member_name: field,
                    receiver_ty: receiver_norm,
                    diagnostic_span: None,
                },
                &mut kernc_utils::FastHashSet::default(),
            )?;
            let owner_trait_ty = resolution.owner_trait_ty?;
            let TypeKind::TraitObject(owner_trait_id, owner_args, _) =
                self.ctx.type_registry.get(owner_trait_ty).clone()
            else {
                return None;
            };
            let Some(Def::Trait(owner_trait_def)) = self.ctx.defs.get(owner_trait_id.0 as usize)
            else {
                return None;
            };
            let method = owner_trait_def
                .methods
                .iter()
                .find(|method| method.signature.name == field && method.default_impl.is_some())?;
            (method.default_impl?, owner_args)
        };
        let mut fn_args = method_trait_args;
        fn_args.push(kernc_sema::ty::GenericArg::Type(receiver_norm));
        Some((default_id, Some(receiver_norm), fn_args))
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

        if let Some(projection) =
            self.maybe_lower_member_intrinsic_call(callee, args, subst_map, span, result_ty)
        {
            return projection;
        }

        let raw_callee_ty = self.ctx.node_type(callee.id).unwrap_or(TypeId::ERROR);

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
        recv: MastExpr,
        arg_masts: Vec<MastExpr>,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        call: MethodCallSite,
    ) -> MastExpr {
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

                // Match the selected impl receiver. Default trait methods dispatch through a
                // synthetic `Self: Trait` bound, and `Self` may itself be pointer-shaped.
                let expected_self_ty = resolved_self_ty.or(call.expected_self_ty);
                if let Some(exp_self) = expected_self_ty
                    && final_recv.ty != exp_self
                {
                    final_recv = self.apply_implicit_cast(
                        final_recv.kind,
                        final_recv.ty,
                        exp_self,
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
        let expected_self_ty = self
            .method_self_param_ty(method_id, generics)
            .or_else(|| self.function_self_generic_ty(method_id, generics))
            .or(call.expected_self_ty)
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
                this.lower_intrinsic_call(super::intrinsic::IntrinsicCallLowering {
                    fn_id: call.fn_id,
                    callee_ty: call.callee_mast.ty,
                    result_ty: call.result_ty,
                    args: call.args,
                    arg_masts: &mut call.arg_masts,
                    subst_map: call.subst_map,
                    span: call.span,
                })
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
                            && let Some(self_ty) = self.ctx.node_type(impl_def.target_type.id)
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
