use super::Lowerer;
use kernc_ast::{Expr, ExprKind};
use kernc_mast::*;
use kernc_sema::ty::{GenericArg, TypeId, TypeKind};
use kernc_utils::SymbolId;
use std::collections::HashMap;

mod access;
mod call;
mod cast;
mod control;
mod literal;
mod ops;

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(crate) fn lower_expr(
        &mut self,
        expr: &Expr,
        subst_map: &HashMap<SymbolId, GenericArg>,
        expected_ty: Option<TypeId>,
    ) -> MastExpr {
        let raw_ty = self.resolve_expr_type(expr);
        let concrete_ty = self.substitute_type_with_map(raw_ty, subst_map);
        let exp_ty = expected_ty.unwrap_or(concrete_ty);

        if exp_ty == TypeId::ERROR {
            self.ctx
                .emit_ice(expr.span, "Lowering encountered an unresolved ERROR type.");
            // Abort lowering immediately so corrupted data never reaches LLVM.
            self.ctx.sess.print_diagnostics();
            std::process::exit(1);
        }

        let mast_kind = match &expr.kind {
            ExprKind::Integer(val) => MastExprKind::Integer(*val),
            ExprKind::Float(val) => MastExprKind::Float(*val),
            ExprKind::Bool(val) => MastExprKind::Bool(*val),
            ExprKind::Char(c) => MastExprKind::Integer(*c as u32 as u128),
            ExprKind::ByteChar(b) => MastExprKind::Integer(*b as u128),
            ExprKind::String(s) => self.measure_phase("        lower_expr_literal", |this| {
                this.lower_string_literal(s, expr.span)
            }),
            ExprKind::Identifier(name) => {
                self.measure_phase("        lower_expr_identifier", |this| {
                    let norm_ty = this.ctx.type_registry.normalize(concrete_ty);

                    match this.ctx.type_registry.get(norm_ty).clone() {
                        TypeKind::FnDef(fn_id, fn_args) => {
                            this.measure_phase("          lower_ident_fn_ref", |this| {
                                let mono_id =
                                    this.instantiate_function_at(fn_id, &fn_args, expr.span);
                                MastExprKind::FuncRef(mono_id)
                            })
                        }
                        TypeKind::Module(_) => {
                            this.measure_phase("          lower_ident_module", |this| {
                                // Modules live in the global namespace and never participate in closure capture.
                                this.lower_identifier(expr.id, *name)
                            })
                        }
                        _ => {
                            let kind = this.measure_phase("          lower_ident_value", |this| {
                                this.lower_identifier(expr.id, *name)
                            });

                            // Ordinary variables still need closure-capture safety checks.
                            if let MastExprKind::Var(v) = kind
                                && !this.measure_phase("          lower_ident_capture_check", |this| {
                                    this.has_local_binding(v)
                                })
                            {
                                let var_str = this.ctx.resolve(v).to_string();
                                this.ctx.struct_error(expr.span, "closures cannot capture environmental variables in Kern")
                                    .with_hint(format!("variable `{}` belongs to an outer scope", var_str))
                                    .with_hint("Kern anonymous functions compile directly to static C function pointers")
                                    .emit();
                            }
                            kind
                        }
                    }
                })
            }

            ExprKind::Let {
                pattern,
                init,
                else_pattern,
                else_branch,
            } => {
                return self.measure_phase("        lower_expr_binding", |this| {
                    MastExpr::new(
                        concrete_ty,
                        MastExprKind::Block(MastBlock {
                            stmts: this.lower_let_stmts(
                                expr,
                                pattern,
                                init,
                                else_pattern.as_ref(),
                                else_branch.as_deref(),
                                subst_map,
                            ),
                            result: None,
                            defers: vec![],
                        }),
                        expr.span,
                    )
                });
            }
            ExprKind::Static { pattern, init } => {
                self.measure_phase("        lower_expr_binding", |this| {
                    this.lower_static_decl(
                        pattern.name,
                        init,
                        subst_map,
                        concrete_ty,
                        pattern.is_mut,
                    )
                })
            }

            ExprKind::Binary { lhs, op, rhs } => self
                .measure_phase("        lower_expr_ops", |this| {
                    this.lower_binary(lhs, *op, rhs, subst_map, concrete_ty, expr.span)
                }),
            ExprKind::Unary { op, operand } => self
                .measure_phase("        lower_expr_ops", |this| {
                    this.lower_unary(*op, operand, subst_map, concrete_ty, expr.span)
                }),

            ExprKind::Call { callee, args } => self
                .measure_phase("        lower_expr_call", |this| {
                    this.lower_call(callee, args, subst_map, expr.span)
                }),
            ExprKind::FieldAccess { lhs, field, .. } => {
                self.measure_phase("        lower_expr_access", |this| {
                    // Use the substituted concrete type to avoid instantiating `FnDef` with raw generic params.
                    let norm_ty = this.ctx.type_registry.normalize(concrete_ty);

                    if let TypeKind::FnDef(fn_id, fn_args) =
                        this.ctx.type_registry.get(norm_ty).clone()
                    {
                        let mono_id = this.instantiate_function_at(fn_id, &fn_args, expr.span);
                        MastExprKind::FuncRef(mono_id)
                    } else {
                        // Fall back to normal struct or union field access lowering.
                        this.lower_field_access(lhs, *field, subst_map, expr.span)
                    }
                })
            }
            ExprKind::IndexAccess { lhs, index, .. } => self
                .measure_phase("        lower_expr_access", |this| {
                    this.lower_index_access(lhs, index, subst_map)
                }),

            ExprKind::DataInit { literal, .. } => self
                .measure_phase("        lower_expr_aggregate", |this| {
                    this.lower_data_init(literal, subst_map, concrete_ty, expr.span)
                }),
            ExprKind::EnumLiteral { variant, .. } => self
                .measure_phase("        lower_expr_aggregate", |this| {
                    this.lower_enum_literal(*variant, concrete_ty)
                }),

            ExprKind::As { lhs, target } => {
                return self.measure_phase("        lower_expr_ops", |this| {
                    this.lower_as_expr(lhs, target, concrete_ty, subst_map, expr.span)
                });
            }
            ExprKind::Propagate { operand, kind } => {
                self.measure_phase("        lower_expr_control", |this| {
                    this.measure_phase("          lower_expr_control_propagate", |this| {
                        this.lower_propagate(operand, *kind, subst_map, expr.span)
                    })
                })
            }

            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.measure_phase("        lower_expr_control", |this| {
                this.measure_phase("          lower_expr_control_if", |this| {
                    this.lower_if(cond, then_branch, else_branch.as_deref(), subst_map, exp_ty)
                })
            }),
            ExprKind::For {
                init,
                cond,
                post,
                body,
            } => self.measure_phase("        lower_expr_control", |this| {
                this.measure_phase("          lower_expr_control_for", |this| {
                    this.lower_for(
                        init.as_deref(),
                        cond.as_deref(),
                        post.as_deref(),
                        body,
                        subst_map,
                        expr.span,
                    )
                })
            }),
            ExprKind::Match { target, arms } => {
                self.measure_phase("        lower_expr_control", |this| {
                    this.measure_phase("          lower_expr_control_match", |this| {
                        this.lower_match(target, arms, subst_map, exp_ty)
                    })
                })
            }
            ExprKind::Closure {
                captures,
                params,
                ret_type: _,
                body,
            } => self.measure_phase("        lower_expr_control", |this| {
                this.measure_phase("          lower_expr_control_closure", |this| {
                    this.lower_closure_expr(control::ClosureLowerSpec {
                        node_id: expr.id,
                        captures,
                        params,
                        body,
                        concrete_ty,
                        subst_map,
                        exp_ty,
                    })
                })
            }),
            ExprKind::Block { .. } => self.measure_phase("        lower_expr_control", |this| {
                this.measure_phase("          lower_expr_control_block", |this| {
                    MastExprKind::Block(this.lower_block_as_body(expr, subst_map, exp_ty))
                })
            }),

            ExprKind::Return(val) => self.measure_phase("        lower_expr_control", |this| {
                this.measure_phase("          lower_expr_control_return", |this| {
                    this.lower_return(val.as_deref(), subst_map, expr.span)
                })
            }),
            ExprKind::Assign { lhs, op, rhs } => {
                self.measure_phase("        lower_expr_control", |this| {
                    this.measure_phase("          lower_expr_control_assign", |this| {
                        this.lower_assign(lhs, *op, rhs, subst_map)
                    })
                })
            }
            ExprKind::GenericInstantiation { .. } => self
                .measure_phase("        lower_expr_generic", |this| {
                    this.lower_generic_instantiation(concrete_ty, expr.span)
                }),

            ExprKind::SelfValue => MastExprKind::Var(self.ctx.intern("self")),
            ExprKind::Break => self.measure_phase("        lower_expr_control", |this| {
                this.lower_jump(MastExprKind::Break, expr.span)
            }),
            ExprKind::Continue => self.measure_phase("        lower_expr_control", |this| {
                this.lower_jump(MastExprKind::Continue, expr.span)
            }),
            ExprKind::Undef => MastExprKind::Undef,

            ExprKind::SliceOp {
                lhs,
                start,
                end,
                is_inclusive,
                ..
            } => self.measure_phase("        lower_expr_access", |this| {
                let mast_lhs = this.lower_expr(lhs, subst_map, None);
                let mast_start = start
                    .as_ref()
                    .map(|e| Box::new(this.lower_expr(e, subst_map, Some(TypeId::USIZE))));
                let mast_end = end
                    .as_ref()
                    .map(|e| Box::new(this.lower_expr(e, subst_map, Some(TypeId::USIZE))));

                MastExprKind::SliceOp {
                    lhs: Box::new(mast_lhs),
                    start: mast_start,
                    end: mast_end,
                    is_inclusive: *is_inclusive,
                }
            }),
            _ => {
                self.ctx.emit_ice(
                    expr.span,
                    format!("Unhandled ExprKind in lowering: {:?}", expr.kind),
                );
                MastExprKind::Trap
            }
        };

        self.apply_implicit_cast(mast_kind, concrete_ty, exp_ty, expr.span)
    }

    pub(crate) fn resolve_expr_type(&self, expr: &Expr) -> TypeId {
        let raw_ty = self
            .ctx
            .node_types
            .get(&expr.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        if raw_ty == TypeId::ERROR
            && let ExprKind::Identifier(name) = &expr.kind
            && let Some((local_ty, _)) = self.local_binding(*name)
        {
            return local_ty;
        }
        raw_ty
    }
}
