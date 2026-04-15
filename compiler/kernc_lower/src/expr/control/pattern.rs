use super::*;

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(super) fn is_ignored_binding(&self, name: SymbolId) -> bool {
        self.ctx.resolve(name) == "_"
    }

    pub(super) fn resolve_struct_pattern_field(
        &mut self,
        target_ty: TypeId,
        field_name: SymbolId,
        span: Span,
    ) -> Option<(TypeId, MonoId, usize)> {
        let norm_target = self.ctx.type_registry.normalize(target_ty);
        match self.ctx.type_registry.get(norm_target).clone() {
            TypeKind::Def(def_id, gen_args) => {
                let Def::Struct(def) = &self.ctx.defs[def_id.0 as usize] else {
                    self.ctx.emit_ice(
                        span,
                        "Kern ICE (Lowering): expected a struct definition while lowering a destructuring pattern.",
                    );
                    return None;
                };

                let ast_idx = def
                    .fields
                    .iter()
                    .position(|field| field.name == field_name)?;
                let mut field_ty = self
                    .ctx
                    .node_types
                    .get(&def.fields[ast_idx].type_node.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);

                if !def.generics.is_empty() && !gen_args.is_empty() {
                    let mut map = HashMap::new();
                    for (i, param) in def.generics.iter().enumerate() {
                        map.insert(param.name, gen_args[i]);
                    }
                    field_ty = self.substitute_type_with_map(field_ty, &map);
                }

                let field_idx = self.get_physical_field_index(target_ty, field_name, span);
                let struct_id = self.instantiate_struct(def_id, &gen_args);
                Some((field_ty, struct_id, field_idx))
            }
            TypeKind::AnonymousStruct(_, fields) => {
                let field_idx = fields.iter().position(|field| field.name == field_name)?;
                let struct_id = self.instantiate_anon_struct(norm_target);
                Some((fields[field_idx].ty, struct_id, field_idx))
            }
            _ => None,
        }
    }

    pub(super) fn and_expr(&self, span: Span, lhs: MastExpr, rhs: MastExpr) -> MastExpr {
        if matches!(lhs.kind, MastExprKind::Bool(true)) {
            return rhs;
        }
        if matches!(rhs.kind, MastExprKind::Bool(true)) {
            return lhs;
        }

        MastExpr::new(
            TypeId::BOOL,
            MastExprKind::If {
                cond: Box::new(lhs),
                then_branch: MastBlock {
                    stmts: vec![],
                    result: Some(Box::new(rhs)),
                    defers: vec![],
                },
                else_branch: Some(MastBlock {
                    stmts: vec![],
                    result: Some(Box::new(self.bool_expr(span, false))),
                    defers: vec![],
                }),
            },
            span,
        )
    }
    pub(super) fn collect_pattern_plan(
        &mut self,
        span: Span,
        pattern: &ast::Pattern,
        target_expr: &MastExpr,
        target_ty: TypeId,
        bindings: &mut Vec<PatternBindingPlan>,
    ) -> MastExpr {
        match &pattern.kind {
            ast::PatternKind::Binding(binding) => {
                if !self.is_ignored_binding(binding.name) {
                    bindings.push(PatternBindingPlan {
                        name: binding.name,
                        ty: target_ty,
                        is_mut: binding.is_mut,
                        init: target_expr.clone(),
                    });
                }
                self.bool_expr(span, true)
            }
            ast::PatternKind::Ignore => self.bool_expr(span, true),
            ast::PatternKind::Variant(variant) => self
                .build_enum_variant_condition(span, target_expr, target_ty, variant.variant_name)
                .map(|(cond, _)| cond)
                .unwrap_or_else(|| self.bool_expr(span, false)),
            ast::PatternKind::Destructure(destructure) => {
                let norm_target = self.ctx.type_registry.normalize(target_ty);
                if matches!(
                    self.ctx.type_registry.get(norm_target),
                    TypeKind::Enum(_, _) | TypeKind::AnonymousEnum(_)
                ) {
                    let Some(field) = destructure.fields.first() else {
                        return self.bool_expr(span, true);
                    };
                    let Some((tag_cond, payload_info)) =
                        self.build_enum_variant_condition(span, target_expr, target_ty, field.name)
                    else {
                        return self.bool_expr(span, false);
                    };
                    let Some((variant_idx, payload_ty, mono_id)) = payload_info else {
                        return tag_cond;
                    };
                    let Some(payload_expr) = self.build_payload_extract_expr(
                        span,
                        target_expr,
                        mono_id,
                        variant_idx,
                        payload_ty,
                    ) else {
                        return self.bool_expr(span, false);
                    };
                    let inner = self.collect_pattern_plan(
                        span,
                        &field.pattern,
                        &payload_expr,
                        payload_ty,
                        bindings,
                    );
                    self.and_expr(span, tag_cond, inner)
                } else {
                    let mut cond = self.bool_expr(span, true);
                    for field in &destructure.fields {
                        let Some((field_ty, struct_id, field_idx)) =
                            self.resolve_struct_pattern_field(target_ty, field.name, field.span)
                        else {
                            return self.bool_expr(span, false);
                        };
                        let field_expr = MastExpr::new(
                            field_ty,
                            MastExprKind::FieldAccess {
                                lhs: Box::new(target_expr.clone()),
                                struct_id,
                                field_idx,
                            },
                            field.span,
                        );
                        let inner = self.collect_pattern_plan(
                            field.span,
                            &field.pattern,
                            &field_expr,
                            field_ty,
                            bindings,
                        );
                        cond = self.and_expr(field.span, cond, inner);
                    }
                    cond
                }
            }
        }
    }

    pub(super) fn lower_match_pattern_body(
        &mut self,
        arm_body: &Expr,
        bindings: Vec<PatternBindingPlan>,
        subst_map: &HashMap<SymbolId, TypeId>,
        exp_ty: TypeId,
    ) -> MastBlock {
        self.local_types.push(HashMap::new());
        self.local_forwardings.push(HashMap::new());
        self.local_value_forwardings.push(HashMap::new());
        let mut prefix = Vec::new();
        for binding in bindings {
            self.bind_local_type(
                arm_body.span,
                binding.name,
                binding.ty,
                binding.is_mut,
                "match pattern binding",
            );
            prefix.push(MastStmt::Let {
                name: binding.name,
                ty: binding.ty,
                is_mut: binding.is_mut,
                init: binding.init,
            });
        }

        let mut block = self.lower_block_as_body(arm_body, subst_map, exp_ty);
        prefix.append(&mut block.stmts);
        block.stmts = prefix;
        self.local_types.pop();
        self.local_forwardings.pop();
        self.local_value_forwardings.pop();
        block
    }

    pub(crate) fn lower_let_stmts(
        &mut self,
        expr: &Expr,
        pattern: &ast::LetPattern,
        init: &Expr,
        else_pattern: Option<&ast::Pattern>,
        else_branch: Option<&Expr>,
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> Vec<MastStmt> {
        if else_branch.is_none() {
            match &pattern.pattern.kind {
                ast::PatternKind::Binding(binding) => {
                    if self.measure_phase("        lower_let_binding_ignored", |this| {
                        this.is_ignored_binding(binding.name)
                    }) {
                        if self.is_pure_dead_initializer(expr.id) {
                            return Vec::new();
                        }

                        return self
                            .lower_optional_stmt_expr(init, subst_map)
                            .into_iter()
                            .collect();
                    }

                    let target_ty = self.measure_phase("        lower_let_binding_type", |this| {
                        this.substitute_type_with_map(this.resolve_expr_type(init), subst_map)
                    });

                    if self.measure_phase("        lower_let_binding_elide", |this| {
                        !binding.is_mut && this.is_elidable_binding(expr.id)
                    }) {
                        return Vec::new();
                    }

                    self.measure_phase("        lower_let_binding_bind_local", |this| {
                        this.bind_local_type(
                            expr.span,
                            binding.name,
                            target_ty,
                            binding.is_mut,
                            "let pattern binding",
                        );
                    });

                    if self.measure_phase("        lower_let_binding_forward_value", |this| {
                        !binding.is_mut && this.is_forwardable_value_binding(expr.id)
                    }) {
                        let init = self.measure_phase(
                            "        lower_let_binding_forward_value_init",
                            |this| this.lower_expr(init, subst_map, Some(target_ty)),
                        );
                        self.measure_phase(
                            "        lower_let_binding_forward_value_record",
                            |this| {
                                this.record_local_value_forwarding(
                                    expr.span,
                                    binding.name,
                                    init,
                                    "recording forwardable pure value binding",
                                );
                            },
                        );
                        return Vec::new();
                    }

                    if !binding.is_mut
                        && let Some(source_name) = self
                            .measure_phase("        lower_let_binding_forward_alias", |this| {
                                this.forwardable_binding_source(expr.id)
                            })
                    {
                        self.measure_phase(
                            "        lower_let_binding_forward_alias_record",
                            |this| {
                                this.record_local_forwarding(
                                    expr.span,
                                    binding.name,
                                    source_name,
                                    "recording forwardable immutable alias binding",
                                );
                            },
                        );
                        return Vec::new();
                    }

                    let init = if self
                        .measure_phase("        lower_let_binding_dead_init", |this| {
                            this.is_pure_dead_initializer(expr.id)
                        }) {
                        MastExpr::new(target_ty, MastExprKind::Undef, expr.span)
                    } else {
                        self.measure_phase("        lower_let_binding_init", |this| {
                            this.lower_expr(init, subst_map, Some(target_ty))
                        })
                    };

                    return self.measure_phase("        lower_let_binding_emit", |_| {
                        vec![MastStmt::Let {
                            name: binding.name,
                            ty: target_ty,
                            is_mut: binding.is_mut,
                            init,
                        }]
                    });
                }
                ast::PatternKind::Ignore => {
                    if self.is_pure_dead_initializer(expr.id) {
                        return Vec::new();
                    }

                    return self
                        .lower_optional_stmt_expr(init, subst_map)
                        .into_iter()
                        .collect();
                }
                _ => {}
            }
        }

        let lowered_init = self.measure_phase("        lower_let_pattern_init", |this| {
            this.lower_expr(init, subst_map, None)
        });
        let target_ty = lowered_init.ty;
        let (target_let, target_var_expr) = self
            .measure_phase("        lower_let_pattern_target", |this| {
                this.build_match_target_binding(target_ty, lowered_init, init.span)
            });

        let mut bindings = Vec::new();
        let condition = self.measure_phase("        lower_let_pattern_plan", |this| {
            this.collect_pattern_plan(
                expr.span,
                &pattern.pattern,
                &target_var_expr,
                target_ty,
                &mut bindings,
            )
        });

        if let Some(else_expr) = else_branch {
            let mut outer_stmts = Vec::new();
            let mut success_stmts = Vec::new();
            let mut finalized_bindings = Vec::new();

            self.measure_phase("        lower_let_else_bindings", |this| {
                for binding in bindings {
                    let temp_id = this.new_mono_id();
                    let temp_name = this
                        .ctx
                        .intern(&format!("__let_else_binding_{}", temp_id.0));

                    // Keep success values in hidden temps until the control-flow block
                    // completes so the initializer still resolves any shadowed outer name.
                    outer_stmts.push(MastStmt::Let {
                        name: temp_name,
                        ty: binding.ty,
                        is_mut: false,
                        init: MastExpr::new(binding.ty, MastExprKind::Undef, expr.span),
                    });
                    success_stmts.push(MastStmt::Expr(MastExpr::new(
                        TypeId::VOID,
                        MastExprKind::Assign {
                            op: ast::AssignmentOperator::Assign,
                            lhs: Box::new(MastExpr::new(
                                binding.ty,
                                MastExprKind::Var(temp_name),
                                expr.span,
                            )),
                            rhs: Box::new(binding.init),
                        },
                        expr.span,
                    )));

                    this.bind_local_type(
                        expr.span,
                        binding.name,
                        binding.ty,
                        binding.is_mut,
                        "let pattern binding",
                    );
                    finalized_bindings.push((binding.name, binding.ty, binding.is_mut, temp_name));
                }
            });

            let lowered_else = if let Some(else_pattern) = else_pattern {
                let mut else_bindings = Vec::new();
                let else_condition =
                    self.measure_phase("        lower_let_else_pattern_plan", |this| {
                        this.collect_pattern_plan(
                            expr.span,
                            else_pattern,
                            &target_var_expr,
                            target_ty,
                            &mut else_bindings,
                        )
                    });
                let else_body = self.measure_phase("        lower_let_else_pattern_body", |this| {
                    this.lower_match_pattern_body(else_expr, else_bindings, subst_map, TypeId::VOID)
                });

                MastBlock {
                    stmts: vec![],
                    result: Some(Box::new(MastExpr::new(
                        TypeId::VOID,
                        MastExprKind::If {
                            cond: Box::new(else_condition),
                            then_branch: else_body,
                            else_branch: Some(MastBlock {
                                stmts: vec![],
                                result: Some(Box::new(MastExpr::new(
                                    TypeId::NEVER,
                                    MastExprKind::Trap,
                                    else_expr.span,
                                ))),
                                defers: vec![],
                            }),
                        },
                        else_expr.span,
                    ))),
                    defers: vec![],
                }
            } else {
                self.measure_phase("        lower_let_else_block", |this| {
                    this.lower_block_as_body(else_expr, subst_map, TypeId::VOID)
                })
            };

            let if_expr = MastExpr::new(
                TypeId::VOID,
                MastExprKind::If {
                    cond: Box::new(condition),
                    then_branch: MastBlock {
                        stmts: success_stmts,
                        result: None,
                        defers: vec![],
                    },
                    else_branch: Some(lowered_else),
                },
                expr.span,
            );

            self.measure_phase("        lower_let_else_emit", |_| {
                outer_stmts.push(MastStmt::Expr(MastExpr::new(
                    TypeId::VOID,
                    MastExprKind::Block(MastBlock {
                        stmts: vec![target_let],
                        result: Some(Box::new(if_expr)),
                        defers: vec![],
                    }),
                    expr.span,
                )));

                for (name, ty, is_mut, temp_name) in finalized_bindings {
                    outer_stmts.push(MastStmt::Let {
                        name,
                        ty,
                        is_mut,
                        init: MastExpr::new(ty, MastExprKind::Var(temp_name), expr.span),
                    });
                }
            });

            outer_stmts
        } else {
            let mut stmts = vec![target_let];
            self.measure_phase("        lower_let_pattern_bindings", |this| {
                for binding in bindings {
                    this.bind_local_type(
                        expr.span,
                        binding.name,
                        binding.ty,
                        binding.is_mut,
                        "let pattern binding",
                    );
                    stmts.push(MastStmt::Let {
                        name: binding.name,
                        ty: binding.ty,
                        is_mut: binding.is_mut,
                        init: binding.init,
                    });
                }
            });
            stmts
        }
    }

    pub(crate) fn lower_match(
        &mut self,
        target: &Expr,
        arms: &[ast::MatchArm],
        subst_map: &HashMap<SymbolId, TypeId>,
        exp_ty: TypeId,
    ) -> MastExprKind {
        let lowered_target = self.measure_phase("            lower_match_target", |this| {
            this.lower_expr(target, subst_map, None)
        });
        let target_ty = lowered_target.ty;
        let (let_stmt, target_var_expr) =
            self.build_match_target_binding(target_ty, lowered_target, target.span);
        let match_context = MatchLowerContext {
            arms,
            target_var_expr: &target_var_expr,
            target_ty,
            subst_map,
            exp_ty,
        };
        let match_expr = self.measure_phase("            lower_match_arms", |this| {
            this.lower_match_arm_chain(&match_context, 0)
        });

        MastExprKind::Block(MastBlock {
            stmts: vec![let_stmt],
            result: Some(Box::new(match_expr)),
            defers: vec![],
        })
    }

    pub(super) fn lower_match_arm_chain(
        &mut self,
        match_context: &MatchLowerContext<'_>,
        arm_index: usize,
    ) -> MastExpr {
        if arm_index >= match_context.arms.len() {
            return MastExpr::new(
                match_context.exp_ty,
                MastExprKind::Trap,
                match_context.target_var_expr.span,
            );
        }

        let arm = &match_context.arms[arm_index];
        self.lower_match_pattern_chain(match_context, &arm.patterns, 0, arm, arm_index)
    }

    pub(super) fn lower_match_pattern_chain(
        &mut self,
        match_context: &MatchLowerContext<'_>,
        patterns: &[ast::MatchPattern],
        pattern_index: usize,
        arm: &ast::MatchArm,
        arm_index: usize,
    ) -> MastExpr {
        if pattern_index >= patterns.len() {
            return self.lower_match_arm_chain(match_context, arm_index + 1);
        }

        let pattern = &patterns[pattern_index];
        let (cond, bindings) = match &pattern.kind {
            ast::MatchPatternKind::Value(value) => {
                self.measure_phase("              lower_match_pattern_value", |this| {
                    let cond = if let ExprKind::EnumLiteral { variant, .. } = value.kind {
                        this.build_enum_variant_condition(
                            pattern.span,
                            match_context.target_var_expr,
                            match_context.target_ty,
                            variant,
                        )
                        .map(|(cond, _)| cond)
                        .unwrap_or_else(|| this.bool_expr(pattern.span, false))
                    } else {
                        let value_expr = this.lower_expr(
                            value,
                            match_context.subst_map,
                            Some(match_context.target_ty),
                        );
                        MastExpr::new(
                            TypeId::BOOL,
                            MastExprKind::Binary {
                                op: ast::BinaryOperator::Equal,
                                lhs: Box::new(match_context.target_var_expr.clone()),
                                rhs: Box::new(value_expr),
                            },
                            pattern.span,
                        )
                    };
                    (cond, Vec::new())
                })
            }
            ast::MatchPatternKind::Range {
                start,
                end,
                inclusive,
            } => self.measure_phase("              lower_match_pattern_range", |this| {
                let start_expr = this.lower_expr(
                    start,
                    match_context.subst_map,
                    Some(match_context.target_ty),
                );
                let end_expr =
                    this.lower_expr(end, match_context.subst_map, Some(match_context.target_ty));
                let lower = MastExpr::new(
                    TypeId::BOOL,
                    MastExprKind::Binary {
                        op: ast::BinaryOperator::LessOrEqual,
                        lhs: Box::new(start_expr),
                        rhs: Box::new(match_context.target_var_expr.clone()),
                    },
                    pattern.span,
                );
                let upper_op = if *inclusive {
                    ast::BinaryOperator::LessOrEqual
                } else {
                    ast::BinaryOperator::LessThan
                };
                let upper = MastExpr::new(
                    TypeId::BOOL,
                    MastExprKind::Binary {
                        op: upper_op,
                        lhs: Box::new(match_context.target_var_expr.clone()),
                        rhs: Box::new(end_expr),
                    },
                    pattern.span,
                );
                (this.and_expr(pattern.span, lower, upper), Vec::new())
            }),
            ast::MatchPatternKind::Pattern(inner) => {
                self.measure_phase("              lower_match_pattern_plan", |this| {
                    let mut bindings = Vec::new();
                    let cond = this.collect_pattern_plan(
                        pattern.span,
                        inner,
                        match_context.target_var_expr,
                        match_context.target_ty,
                        &mut bindings,
                    );
                    (cond, bindings)
                })
            }
        };

        let then_branch = self.measure_phase("              lower_match_pattern_body", |this| {
            this.lower_match_pattern_body(
                &arm.body,
                bindings,
                match_context.subst_map,
                match_context.exp_ty,
            )
        });
        let fallback = self.measure_phase("              lower_match_pattern_fallback", |this| {
            this.lower_match_pattern_chain(
                match_context,
                patterns,
                pattern_index + 1,
                arm,
                arm_index,
            )
        });

        MastExpr::new(
            match_context.exp_ty,
            MastExprKind::If {
                cond: Box::new(cond),
                then_branch,
                else_branch: Some(MastBlock {
                    stmts: vec![],
                    result: Some(Box::new(fallback)),
                    defers: vec![],
                }),
            },
            arm.span,
        )
    }
}
