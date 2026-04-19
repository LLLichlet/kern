use super::*;

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(super) fn resolve_named_match_adt(
        &mut self,
        def_id: kernc_sema::def::DefId,
        args: Vec<kernc_sema::ty::GenericArg>,
        span: Span,
    ) -> Option<MatchAdtInfo> {
        let Def::Enum(def) = self.ctx.defs[def_id.0 as usize].clone() else {
            self.ctx.emit_ice(
                span,
                format!("Kern ICE (Lowering): DefId {} is not an Enum.", def_id.0),
            );
            return None;
        };

        let pure = self.is_pure_enum(&def);
        let mono_id = if pure {
            MonoId(0)
        } else {
            self.instantiate_data(def_id, &args)
        };
        let tag_ty = def.backing_type.as_ref().map_or(TypeId::U32, |backing_ty| {
            self.ctx
                .node_types
                .get(&backing_ty.id)
                .copied()
                .unwrap_or(TypeId::U32)
        });

        Some(MatchAdtInfo::Named {
            mono_id,
            gen_args: args,
            def,
            is_pure: pure,
            tag_ty,
        })
    }

    pub(super) fn build_match_tag_expr(
        &mut self,
        target_var_expr: &MastExpr,
        adt_info: &Option<MatchAdtInfo>,
        span: Span,
    ) -> MastExpr {
        let Some(info) = adt_info else {
            return target_var_expr.clone();
        };

        let (mono_id, is_pure, tag_ty) = match info {
            MatchAdtInfo::Named {
                mono_id,
                is_pure,
                tag_ty,
                ..
            }
            | MatchAdtInfo::Anonymous {
                mono_id,
                is_pure,
                tag_ty,
                ..
            } => (*mono_id, *is_pure, *tag_ty),
        };

        if is_pure {
            MastExpr::new(tag_ty, target_var_expr.kind.clone(), span)
        } else {
            MastExpr::new(
                tag_ty,
                MastExprKind::FieldAccess {
                    lhs: Box::new(target_var_expr.clone()),
                    struct_id: mono_id,
                    field_idx: 0,
                },
                span,
            )
        }
    }

    pub(super) fn resolve_match_variant(
        &mut self,
        info: &MatchAdtInfo,
        variant_name: SymbolId,
        span: Span,
    ) -> Option<(usize, u128)> {
        match info {
            MatchAdtInfo::Named { def, .. } => {
                self.named_enum_variant_info(def, variant_name, span)
            }
            MatchAdtInfo::Anonymous { def, .. } => {
                let variant = self.anon_enum_variant_info(def, variant_name);
                if variant.is_none() {
                    self.ctx.emit_ice(
                        span,
                        format!(
                            "Kern ICE (Lowering): variant `{}` not found in anonymous enum match.",
                            self.ctx.resolve(variant_name)
                        ),
                    );
                }
                variant
            }
        }
    }

    pub(super) fn payload_union_id(&mut self, mono_id: MonoId, span: Span) -> Option<MonoId> {
        match self.adt_union_map.get(&mono_id).copied() {
            Some(id) => Some(id),
            None => {
                self.ctx.emit_ice(
                    span,
                    "Kern ICE (Lowering): missing enum payload union mapping in `adt_union_map`.",
                );
                None
            }
        }
    }

    pub(super) fn build_payload_extract_expr(
        &mut self,
        span: Span,
        target_expr: &MastExpr,
        mono_id: MonoId,
        field_idx: usize,
        payload_ty: TypeId,
    ) -> Option<MastExpr> {
        let target_union_id = self.payload_union_id(mono_id, span)?;
        let union_access = MastExpr::new(
            TypeId::VOID,
            MastExprKind::FieldAccess {
                lhs: Box::new(target_expr.clone()),
                struct_id: mono_id,
                field_idx: 1,
            },
            span,
        );

        Some(MastExpr::new(
            payload_ty,
            MastExprKind::FieldAccess {
                lhs: Box::new(union_access),
                struct_id: target_union_id,
                field_idx,
            },
            span,
        ))
    }
    pub(super) fn build_variant_value_expr(
        &mut self,
        enum_ty: TypeId,
        variant_name: SymbolId,
        payload: Option<MastExpr>,
        span: Span,
    ) -> Option<MastExpr> {
        let adt_info = self.resolve_match_adt(enum_ty, span)?;
        let (variant_idx, tag_value) = self.resolve_match_variant(&adt_info, variant_name, span)?;

        match adt_info {
            MatchAdtInfo::Named {
                mono_id,
                def,
                is_pure,
                ..
            } => {
                let expects_payload = def.variants[variant_idx].payload_type.is_some();
                if is_pure {
                    return Some(MastExpr::new(
                        enum_ty,
                        MastExprKind::Integer(tag_value),
                        span,
                    ));
                }

                let payload_expr = if expects_payload {
                    payload?
                } else {
                    MastExpr::new(TypeId::VOID, MastExprKind::Undef, span)
                };
                Some(MastExpr::new(
                    enum_ty,
                    MastExprKind::DataInit {
                        data_struct_id: mono_id,
                        tag_value,
                        payload: Box::new(payload_expr),
                    },
                    span,
                ))
            }
            MatchAdtInfo::Anonymous {
                mono_id,
                def,
                is_pure,
                ..
            } => {
                let expects_payload = def
                    .variants
                    .get(variant_idx)
                    .and_then(|variant| variant.payload_ty)
                    .is_some();
                if is_pure {
                    return Some(MastExpr::new(
                        enum_ty,
                        MastExprKind::Integer(tag_value),
                        span,
                    ));
                }

                let payload_expr = if expects_payload {
                    payload?
                } else {
                    MastExpr::new(TypeId::VOID, MastExprKind::Undef, span)
                };
                Some(MastExpr::new(
                    enum_ty,
                    MastExprKind::DataInit {
                        data_struct_id: mono_id,
                        tag_value,
                        payload: Box::new(payload_expr),
                    },
                    span,
                ))
            }
        }
    }

    pub(crate) fn lower_propagate(
        &mut self,
        operand: &Expr,
        kind: ast::PropagateKind,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        span: Span,
    ) -> MastExprKind {
        let lowered_operand = self.lower_expr(operand, subst_map, None);
        let operand_ty = lowered_operand.ty;
        let current_return_ty = match self.current_return_type(span) {
            Some(ty) => ty,
            None => return MastExprKind::Trap,
        };

        let (target_let, target_var_expr) =
            self.build_match_target_binding(operand_ty, lowered_operand, operand.span);
        let operand_info = match self.resolve_match_adt(operand_ty, span) {
            Some(info) => info,
            None => {
                self.ctx.emit_ice(
                    span,
                    "Kern ICE (Lowering): propagation operand did not lower to an enum-like ADT.",
                );
                return MastExprKind::Trap;
            }
        };

        let some_name = self.ctx.intern("Some");
        let none_name = self.ctx.intern("None");
        let ok_name = self.ctx.intern("Ok");
        let err_name = self.ctx.intern("Err");

        let (success_variant, failure_variant, success_payload_info, failure_payload_info) = match (
            kind,
            operand_info,
        ) {
            (ast::PropagateKind::Option, MatchAdtInfo::Anonymous { def, .. })
                if def.builtin == Some(BuiltinAnonymousEnumKind::Optional) =>
            {
                let success = self.build_enum_variant_condition(
                    span,
                    &target_var_expr,
                    operand_ty,
                    some_name,
                );
                let failure = self.build_enum_variant_condition(
                    span,
                    &target_var_expr,
                    operand_ty,
                    none_name,
                );
                let (success_cond, success_payload_info) = match success {
                    Some(value) => value,
                    None => return MastExprKind::Trap,
                };
                let (_, failure_payload_info) = match failure {
                    Some(value) => value,
                    None => return MastExprKind::Trap,
                };
                (
                    success_cond,
                    none_name,
                    success_payload_info,
                    failure_payload_info,
                )
            }
            (ast::PropagateKind::Result, MatchAdtInfo::Anonymous { def, .. })
                if def.builtin == Some(BuiltinAnonymousEnumKind::Result) =>
            {
                let success =
                    self.build_enum_variant_condition(span, &target_var_expr, operand_ty, ok_name);
                let failure =
                    self.build_enum_variant_condition(span, &target_var_expr, operand_ty, err_name);
                let (success_cond, success_payload_info) = match success {
                    Some(value) => value,
                    None => return MastExprKind::Trap,
                };
                let (_, failure_payload_info) = match failure {
                    Some(value) => value,
                    None => return MastExprKind::Trap,
                };
                (
                    success_cond,
                    err_name,
                    success_payload_info,
                    failure_payload_info,
                )
            }
            _ => {
                self.ctx.emit_ice(
                    span,
                    "Kern ICE (Lowering): propagation kind and operand builtin enum kind disagreed.",
                );
                return MastExprKind::Trap;
            }
        };

        let Some((success_field_idx, success_payload_ty, success_mono_id)) = success_payload_info
        else {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Lowering): propagation success branch is missing its payload.",
            );
            return MastExprKind::Trap;
        };
        let Some(success_value) = self.build_payload_extract_expr(
            span,
            &target_var_expr,
            success_mono_id,
            success_field_idx,
            success_payload_ty,
        ) else {
            return MastExprKind::Trap;
        };

        let failure_value = match kind {
            ast::PropagateKind::Option => {
                match self.build_variant_value_expr(current_return_ty, failure_variant, None, span)
                {
                    Some(expr) => expr,
                    None => return MastExprKind::Trap,
                }
            }
            ast::PropagateKind::Result => {
                let Some((failure_field_idx, failure_payload_ty, failure_mono_id)) =
                    failure_payload_info
                else {
                    self.ctx.emit_ice(
                        span,
                        "Kern ICE (Lowering): result propagation error branch is missing its payload.",
                    );
                    return MastExprKind::Trap;
                };
                let Some(failure_payload) = self.build_payload_extract_expr(
                    span,
                    &target_var_expr,
                    failure_mono_id,
                    failure_field_idx,
                    failure_payload_ty,
                ) else {
                    return MastExprKind::Trap;
                };
                match self.build_variant_value_expr(
                    current_return_ty,
                    failure_variant,
                    Some(failure_payload),
                    span,
                ) {
                    Some(expr) => expr,
                    None => return MastExprKind::Trap,
                }
            }
        };

        let early_return = MastExpr::new(
            TypeId::NEVER,
            self.lower_return_lowered_value(Some(failure_value), span),
            span,
        );

        MastExprKind::Block(MastBlock {
            stmts: vec![target_let],
            result: Some(Box::new(MastExpr::new(
                success_payload_ty,
                MastExprKind::If {
                    cond: Box::new(success_variant),
                    then_branch: MastBlock {
                        stmts: vec![],
                        result: Some(Box::new(success_value)),
                        defers: vec![],
                    },
                    else_branch: Some(MastBlock {
                        stmts: vec![],
                        result: Some(Box::new(early_return)),
                        defers: vec![],
                    }),
                },
                span,
            ))),
            defers: vec![],
        })
    }
    pub(super) fn build_enum_variant_condition(
        &mut self,
        span: Span,
        target_expr: &MastExpr,
        target_ty: TypeId,
        variant_name: SymbolId,
    ) -> Option<(MastExpr, Option<MatchVariantPayloadInfo>)> {
        let adt_info = self.resolve_match_adt(target_ty, span)?;
        let (variant_idx, tag_value) = self.resolve_match_variant(&adt_info, variant_name, span)?;
        let tag_expr = self.build_match_tag_expr(target_expr, &Some(adt_info.clone()), span);
        let tag_cond = MastExpr::new(
            TypeId::BOOL,
            MastExprKind::Binary {
                op: ast::BinaryOperator::Equal,
                lhs: Box::new(tag_expr.clone()),
                rhs: Box::new(MastExpr::new(
                    tag_expr.ty,
                    MastExprKind::Integer(tag_value),
                    span,
                )),
            },
            span,
        );

        let payload_info = match adt_info {
            MatchAdtInfo::Named {
                mono_id,
                gen_args,
                def,
                is_pure,
                ..
            } => {
                if is_pure {
                    None
                } else {
                    def.variants[variant_idx]
                        .payload_type
                        .as_ref()
                        .map(|payload_ast| {
                            let mut payload_ty = self
                                .ctx
                                .node_types
                                .get(&payload_ast.id)
                                .copied()
                                .unwrap_or(TypeId::ERROR);
                            if !def.generics.is_empty() && !gen_args.is_empty() {
                                let mut map = HashMap::new();
                                for (i, param) in def.generics.iter().enumerate() {
                                    map.insert(param.name, gen_args[i]);
                                }
                                payload_ty = self.substitute_type_with_map(payload_ty, &map);
                            }
                            (variant_idx, payload_ty, mono_id)
                        })
                }
            }
            MatchAdtInfo::Anonymous {
                mono_id,
                def,
                is_pure,
                ..
            } => {
                if is_pure {
                    None
                } else {
                    def.variants[variant_idx]
                        .payload_ty
                        .map(|payload_ty| (variant_idx, payload_ty, mono_id))
                }
            }
        };

        Some((tag_cond, payload_info))
    }
    /// Helper: synthesize a temporary `let` binding to isolate scope effects.
    pub(super) fn build_match_target_binding(
        &mut self,
        target_ty: TypeId,
        lowered_target: MastExpr,
        span: Span,
    ) -> (MastStmt, MastExpr) {
        let new_mono_id = self.new_mono_id();
        let tmp_sym = self
            .ctx
            .intern(&format!("__match_target_{}", new_mono_id.0));

        self.bind_local_type(span, tmp_sym, target_ty, false, "match target binding");

        let let_stmt = MastStmt::Let {
            name: tmp_sym,
            ty: target_ty,
            is_mut: false,
            init: lowered_target,
        };

        let target_var_expr = MastExpr::new(target_ty, MastExprKind::Var(tmp_sym), span);

        (let_stmt, target_var_expr)
    }

    /// Helper: recover ADT metadata for a target type.
    pub(super) fn resolve_match_adt(
        &mut self,
        target_ty: TypeId,
        span: Span,
    ) -> Option<MatchAdtInfo> {
        let norm_target_ty = self.ctx.type_registry.normalize(target_ty);

        match self.ctx.type_registry.get(norm_target_ty).clone() {
            TypeKind::Enum(def_id, args) => self.resolve_named_match_adt(def_id, args, span),
            TypeKind::AnonymousEnum(def) => {
                let pure = def
                    .variants
                    .iter()
                    .all(|variant| variant.payload_ty.is_none());
                let mono_id = if !pure {
                    self.instantiate_anon_enum(norm_target_ty)
                } else {
                    MonoId(0)
                };
                let tag_ty = def.backing_ty.unwrap_or(TypeId::U32);

                Some(MatchAdtInfo::Anonymous {
                    mono_id,
                    def,
                    is_pure: pure,
                    tag_ty,
                })
            }
            _ => None,
        }
    }
}
