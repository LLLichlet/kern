use super::Lowerer;
use kernc_ast::{Expr, ExprKind};
use kernc_mast::*;
use kernc_sema::ty::{GenericArg, TypeId, TypeKind};
use kernc_utils::{Span, SymbolId};
use std::collections::HashMap;

mod access;
mod call;
mod cast;
mod control;
mod literal;
mod ops;

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    fn range_bound_tys(&self, ty: TypeId) -> (Option<TypeId>, Option<TypeId>) {
        match self
            .ctx
            .type_registry
            .get(self.ctx.type_registry.normalize(ty))
        {
            TypeKind::Range { start, end, .. } => (*start, *end),
            _ => (None, None),
        }
    }

    fn lower_range_expr(
        &mut self,
        start: Option<&Expr>,
        end: Option<&Expr>,
        subst_map: &HashMap<SymbolId, GenericArg>,
        concrete_ty: TypeId,
    ) -> MastExprKind {
        let norm_ty = self.ctx.type_registry.normalize(concrete_ty);
        let struct_id = self.instantiate_range_struct(norm_ty);
        let (start_ty, end_ty) = self.range_bound_tys(norm_ty);
        let mut fields = Vec::new();
        if let Some(start) = start {
            fields.push(self.lower_expr(start, subst_map, start_ty));
        }
        if let Some(end) = end {
            fields.push(self.lower_expr(end, subst_map, end_ty));
        }
        MastExprKind::StructInit { struct_id, fields }
    }

    pub(crate) fn lower_error_kind(
        &mut self,
        span: Span,
        message: impl Into<String>,
    ) -> MastExprKind {
        self.ctx.struct_error(span, message).emit();
        MastExprKind::Trap
    }

    pub(crate) fn lower_error_expr(
        &mut self,
        ty: TypeId,
        span: Span,
        message: impl Into<String>,
    ) -> MastExpr {
        MastExpr::new(ty, self.lower_error_kind(span, message), span)
    }

    pub(crate) fn lower_expr(
        &mut self,
        expr: &Expr,
        subst_map: &HashMap<SymbolId, GenericArg>,
        expected_ty: Option<TypeId>,
    ) -> MastExpr {
        if self.check_canceled().is_err() {
            return MastExpr::new(TypeId::ERROR, MastExprKind::Trap, expr.span);
        }
        let raw_ty = self.resolve_expr_type(expr);
        let concrete_ty = self.substitute_type_with_map(raw_ty, subst_map);
        let exp_ty = expected_ty.unwrap_or(concrete_ty);

        if exp_ty == TypeId::ERROR {
            let fallback_ty = if concrete_ty != TypeId::ERROR {
                concrete_ty
            } else {
                TypeId::VOID
            };
            return self.lower_error_expr(
                fallback_ty,
                expr.span,
                "cannot lower an expression whose type was left unresolved",
            );
        }

        let mast_kind = match &expr.kind {
            ExprKind::Error => MastExprKind::Trap,
            ExprKind::Integer { value, .. } => MastExprKind::Integer(*value),
            ExprKind::Float { value, .. } => MastExprKind::Float(*value),
            ExprKind::Bool(val) => MastExprKind::Bool(*val),
            ExprKind::Char(c) => MastExprKind::Integer(*c as u32 as u128),
            ExprKind::ByteChar(b) => MastExprKind::Integer(*b as u128),
            ExprKind::String(s) => self.measure_phase("        lower_expr_literal", |this| {
                this.lower_string_literal_array(s, expr.span)
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
                                this.lower_identifier(expr.id, *name, subst_map)
                            })
                        }
                        _ => {
                            let lowered_ident =
                                this.measure_phase("          lower_ident_value", |this| {
                                    this.lower_identifier_with_locality(
                                        expr.id,
                                        *name,
                                        subst_map,
                                        concrete_ty,
                                    )
                                });
                            let kind = lowered_ident.kind;

                            // Ordinary variables still need closure-capture safety checks.
                            if let MastExprKind::Var(v) = kind
                                && !lowered_ident.is_local_binding
                            {
                                let var_str = this.ctx.resolve(v).to_string();
                                this.ctx.struct_error(expr.span, "closure body uses an outer variable that is not in its capture list")
                                    .with_hint(format!("variable `{}` belongs to an outer scope", var_str))
                                    .with_hint(format!("add `{}` to the closure capture list, or pass the value as a parameter", var_str))
                                    .emit();
                            }
                            kind
                        }
                    }
                })
            }
            ExprKind::AnchoredPath { anchor, name, .. } => {
                self.measure_phase("        lower_expr_anchored_identifier", |this| {
                    let norm_ty = this.ctx.type_registry.normalize(concrete_ty);

                    match this.ctx.type_registry.get(norm_ty).clone() {
                        TypeKind::FnDef(fn_id, fn_args) => {
                            this.measure_phase("          lower_anchored_fn_ref", |this| {
                                let mono_id =
                                    this.instantiate_function_at(fn_id, &fn_args, expr.span);
                                MastExprKind::FuncRef(mono_id)
                            })
                        }
                        _ => this.lower_anchored_identifier(*anchor, *name, expr.span),
                    }
                })
            }

            ExprKind::Let {
                pattern,
                init,
                else_clause,
                ..
            } => {
                return self.measure_phase("        lower_expr_binding", |this| {
                    MastExpr::new(
                        concrete_ty,
                        MastExprKind::Block(MastBlock {
                            stmts: this.lower_let_stmts(
                                expr,
                                pattern,
                                init,
                                else_clause.as_ref(),
                                subst_map,
                            ),
                            result: None,
                            defers: vec![],
                        }),
                        expr.span,
                    )
                });
            }
            ExprKind::Static { pattern, init, .. } => {
                self.measure_phase("        lower_expr_binding", |this| {
                    let Some(init) = init.as_ref() else {
                        this.ctx.emit_ice(
                            expr.span,
                            "Kern ICE (Lowering): local static declaration missing initializer.",
                        );
                        return MastExprKind::Block(MastBlock {
                            stmts: vec![],
                            result: None,
                            defers: vec![],
                        });
                    };
                    let raw_static_ty = this.resolve_expr_type(init);
                    let static_ty = this.substitute_type_with_map(raw_static_ty, subst_map);
                    this.lower_static_decl(pattern.name, init, subst_map, static_ty, pattern.is_mut)
                })
            }

            ExprKind::Binary { lhs, op, rhs } => {
                self.measure_phase("        lower_expr_ops", |this| {
                    this.lower_binary(ops::BinaryLowerInput {
                        binary_expr_id: expr.id,
                        lhs,
                        op: *op,
                        rhs,
                        subst_map,
                        result_ty: concrete_ty,
                        span: expr.span,
                    })
                })
            }
            ExprKind::Range {
                start,
                end,
                is_inclusive: _,
            } => self.measure_phase("        lower_expr_data", |this| {
                this.lower_range_expr(start.as_deref(), end.as_deref(), subst_map, concrete_ty)
            }),
            ExprKind::Unary { op, operand } => self
                .measure_phase("        lower_expr_ops", |this| {
                    this.lower_unary(*op, operand, subst_map, concrete_ty, expr.span)
                }),
            ExprKind::Grouped { expr: inner } => {
                return self.measure_phase("        lower_expr_grouped", |this| {
                    let lowered = this.lower_expr(inner, subst_map, expected_ty);
                    this.apply_implicit_cast(lowered.kind, lowered.ty, exp_ty, expr.span)
                });
            }

            ExprKind::Call { callee, args } => {
                let call_expr = self.measure_phase("        lower_expr_call", |this| {
                    this.lower_call(callee, args, subst_map, expr.span, concrete_ty)
                });
                let target_ty = expected_ty.unwrap_or(call_expr.ty);
                return self.apply_implicit_cast(
                    call_expr.kind,
                    call_expr.ty,
                    target_ty,
                    expr.span,
                );
            }
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
                    let lowered =
                        this.lower_as_expr(lhs, target, concrete_ty, subst_map, expr.span);
                    this.apply_implicit_cast(lowered.kind, lowered.ty, exp_ty, expr.span)
                });
            }
            ExprKind::Propagate { operand } => {
                self.measure_phase("        lower_expr_control", |this| {
                    this.measure_phase("          lower_expr_control_propagate", |this| {
                        this.lower_propagate(operand, subst_map, expr.span)
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
            ExprKind::While { cond, body } => {
                self.measure_phase("        lower_expr_control", |this| {
                    this.measure_phase("          lower_expr_control_while", |this| {
                        this.lower_while(cond, body, subst_map, expr.span)
                    })
                })
            }
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
        let raw_ty = self.ctx.node_type(expr.id).unwrap_or(TypeId::ERROR);
        if raw_ty == TypeId::ERROR
            && let ExprKind::Identifier(name) = &expr.kind
            && let Some((local_ty, _)) = self.local_binding(*name)
        {
            return local_ty;
        }
        raw_ty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernc_mono::MonoId;
    use kernc_sema::SemaContext;
    use kernc_sema::def::{Def, UnionDef};
    use kernc_sema::scope::{SymbolInfo, SymbolKind};
    use kernc_sema::ty::{AnonymousEnum, AnonymousVariant, BuiltinAnonymousEnumKind};
    use kernc_utils::{AtomicOrdering, DiagnosticLevel, NodeId, Session, Span};

    #[test]
    fn lowering_unresolved_error_type_emits_error_and_returns_trap() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let expr = Expr {
            id: NodeId(0),
            span: Span::default(),
            kind: ExprKind::Integer {
                value: 7,
                suffix: None,
            },
        };
        ctx.set_node_type(expr.id, TypeId::ERROR);

        let lowered = Lowerer::new(&mut ctx).lower_expr(&expr, &HashMap::new(), None);

        assert!(matches!(lowered.kind, MastExprKind::Trap));
        assert_eq!(lowered.ty, TypeId::VOID);
        assert_eq!(ctx.sess.diagnostics.len(), 1);
        assert_eq!(
            ctx.sess.diagnostics[0].message,
            "cannot lower an expression whose type was left unresolved"
        );
    }

    #[test]
    fn lowering_anon_enum_missing_variant_emits_error_not_ice() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let present = ctx.intern("Present");
        let missing = ctx.intern("Missing");
        let anon_enum_ty = ctx
            .type_registry
            .intern(TypeKind::AnonymousEnum(AnonymousEnum {
                backing_ty: Some(TypeId::U8),
                builtin: None,
                variants: vec![AnonymousVariant {
                    name: present,
                    name_span: Span::default(),
                    payload_ty: None,
                    explicit_value: None,
                }],
            }));
        let expr = Expr {
            id: NodeId(0),
            span: Span::default(),
            kind: ExprKind::Identifier(missing),
        };

        let lowered = Lowerer::new(&mut ctx).lower_anon_enum_scalar_init(&expr, anon_enum_ty);

        assert!(matches!(lowered, MastExprKind::Trap));
        assert_eq!(ctx.sess.diagnostics.len(), 1);
        assert_eq!(ctx.sess.diagnostics[0].level, DiagnosticLevel::Error);
        assert_eq!(
            ctx.sess.diagnostics[0].message,
            "anonymous enum variant `Missing` not found during scalar initialization"
        );
    }

    #[test]
    fn lowering_loc_intrinsic_with_wrong_result_type_emits_error_not_ice() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);

        let lowered = Lowerer::new(&mut ctx).lower_loc_intrinsic(TypeId::U8, Span::default());

        assert!(matches!(lowered, MastExprKind::Trap));
        assert_eq!(ctx.sess.diagnostics.len(), 1);
        assert_eq!(ctx.sess.diagnostics[0].level, DiagnosticLevel::Error);
        assert_eq!(
            ctx.sess.diagnostics[0].message,
            "`@loc` must return an anonymous struct containing `file`, `line`, and `col`"
        );
    }

    #[test]
    fn lowering_unresolved_static_trait_dispatch_emits_error_not_ice() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let method = ctx.intern("missing");
        let call_sig = ctx.type_registry.intern(TypeKind::Function {
            params: vec![TypeId::U8],
            ret: TypeId::BOOL,
            is_variadic: false,
        });
        let recv = MastExpr::new(TypeId::U8, MastExprKind::Integer(0), Span::default());

        let lowered = Lowerer::new(&mut ctx).lower_resolved_trait_method_call(
            recv,
            Vec::new(),
            TypeId::U8,
            call::MethodCallSite {
                field: method,
                norm_callee: call_sig,
                expected_self_ty: Some(TypeId::U8),
                default_ret_ty: TypeId::BOOL,
                span: Span::default(),
            },
        );

        assert!(matches!(lowered.kind, MastExprKind::Trap));
        assert_eq!(lowered.ty, TypeId::BOOL);
        assert_eq!(ctx.sess.diagnostics.len(), 1);
        assert_eq!(ctx.sess.diagnostics[0].level, DiagnosticLevel::Error);
        assert_eq!(
            ctx.sess.diagnostics[0].message,
            "cannot resolve a concrete impl for trait method `missing` on exact type `u8` during lowering"
        );
    }

    #[test]
    fn lowering_missing_struct_field_returns_none_instead_of_fabricating_field_zero() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let present = ctx.intern("present");
        let missing = ctx.intern("missing");
        let anon_struct_ty = ctx.type_registry.intern(TypeKind::AnonymousStruct(
            false,
            vec![kernc_sema::ty::AnonymousField {
                name: present,
                ty: TypeId::U8,
            }],
        ));

        let field_idx = Lowerer::new(&mut ctx).get_physical_field_index(
            anon_struct_ty,
            missing,
            Span::default(),
        );

        assert_eq!(field_idx, None);
        assert_eq!(ctx.sess.diagnostics.len(), 1);
        assert_eq!(ctx.sess.diagnostics[0].level, DiagnosticLevel::Error);
        assert_eq!(
            ctx.sess.diagnostics[0].message,
            "field `missing` not found in anonymous struct"
        );
    }

    #[test]
    fn lowering_invalid_atomic_ordering_emits_error_not_ice() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let arg = Expr {
            id: NodeId(0),
            span: Span::default(),
            kind: ExprKind::Integer {
                value: 99,
                suffix: None,
            },
        };

        let ordering = Lowerer::new(&mut ctx).atomic_ordering_arg(
            &arg,
            &HashMap::new(),
            "load order",
            AtomicOrdering::valid_for_load,
            "load order must be Relaxed, Acquire, or SeqCst",
        );

        assert_eq!(ordering, AtomicOrdering::SeqCst);
        assert_eq!(ctx.sess.diagnostics.len(), 1);
        assert_eq!(ctx.sess.diagnostics[0].level, DiagnosticLevel::Error);
        assert_eq!(
            ctx.sess.diagnostics[0].message,
            "invalid atomic ordering constant `99` for `load order`"
        );
    }

    #[test]
    fn lowering_non_array_simd_shuffle_indices_emit_error_not_ice() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let arg = Expr {
            id: NodeId(0),
            span: Span::default(),
            kind: ExprKind::Bool(true),
        };

        let indices = Lowerer::new(&mut ctx).simd_shuffle_indices_arg(&arg);

        assert!(indices.is_empty());
        assert_eq!(ctx.sess.diagnostics.len(), 1);
        assert_eq!(ctx.sess.diagnostics[0].level, DiagnosticLevel::Error);
        assert_eq!(
            ctx.sess.diagnostics[0].message,
            "SIMD shuffle indices must be provided as a constant array, found `Bool(true)`"
        );
    }

    #[test]
    fn lowering_break_outside_loop_emits_error_not_ice() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);

        let lowered = Lowerer::new(&mut ctx).lower_jump(MastExprKind::Break, Span::default());

        assert!(matches!(lowered, MastExprKind::Trap));
        assert_eq!(ctx.sess.diagnostics.len(), 1);
        assert_eq!(ctx.sess.diagnostics[0].level, DiagnosticLevel::Error);
        assert_eq!(
            ctx.sess.diagnostics[0].message,
            "`break` or `continue` cannot appear outside a loop"
        );
    }

    #[test]
    fn lowering_return_without_active_function_emits_error_not_ice() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);

        let lowered = Lowerer::new(&mut ctx).lower_return(None, &HashMap::new(), Span::default());

        assert!(matches!(lowered, MastExprKind::Return(None)));
        assert_eq!(ctx.sess.diagnostics.len(), 1);
        assert_eq!(ctx.sess.diagnostics[0].level, DiagnosticLevel::Error);
        assert_eq!(
            ctx.sess.diagnostics[0].message,
            "cannot lower `return` or propagation outside an active function body"
        );
    }

    #[test]
    fn lowering_propagate_non_enum_operand_emits_error_not_ice() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let value = ctx.intern("value");
        let operand = Expr {
            id: NodeId(0),
            span: Span::default(),
            kind: ExprKind::Identifier(value),
        };
        ctx.set_node_type(operand.id, TypeId::U8);

        let mut lowerer = Lowerer::new(&mut ctx);
        lowerer
            .local_types
            .push(HashMap::from([(value, (TypeId::U8, false))]));
        lowerer.current_return_types.push(TypeId::U8);

        let lowered = lowerer.lower_propagate(&operand, &HashMap::new(), Span::default());

        assert!(matches!(lowered, MastExprKind::Trap));
        assert_eq!(lowerer.ctx.sess.diagnostics.len(), 1);
        assert_eq!(
            lowerer.ctx.sess.diagnostics[0].level,
            DiagnosticLevel::Error
        );
        assert_eq!(
            lowerer.ctx.sess.diagnostics[0].message,
            "propagation operand must be an enum-like `Option` or `Result` value"
        );
    }

    #[test]
    fn lowering_propagate_non_builtin_enum_operand_emits_error_not_ice() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let some = ctx.intern("Some");
        let none = ctx.intern("None");
        let value = ctx.intern("value");
        let optional_ty = ctx
            .type_registry
            .intern(TypeKind::AnonymousEnum(AnonymousEnum {
                backing_ty: Some(TypeId::U8),
                builtin: None,
                variants: vec![
                    AnonymousVariant {
                        name: some,
                        name_span: Span::default(),
                        payload_ty: Some(TypeId::I32),
                        explicit_value: None,
                    },
                    AnonymousVariant {
                        name: none,
                        name_span: Span::default(),
                        payload_ty: None,
                        explicit_value: None,
                    },
                ],
            }));
        let operand = Expr {
            id: NodeId(0),
            span: Span::default(),
            kind: ExprKind::Identifier(value),
        };
        ctx.set_node_type(operand.id, optional_ty);

        let mut lowerer = Lowerer::new(&mut ctx);
        lowerer
            .local_types
            .push(HashMap::from([(value, (optional_ty, false))]));
        lowerer.current_return_types.push(optional_ty);

        let lowered = lowerer.lower_propagate(&operand, &HashMap::new(), Span::default());

        assert!(matches!(lowered, MastExprKind::Trap));
        assert_eq!(lowerer.ctx.sess.diagnostics.len(), 1);
        assert_eq!(
            lowerer.ctx.sess.diagnostics[0].level,
            DiagnosticLevel::Error
        );
        assert_eq!(
            lowerer.ctx.sess.diagnostics[0].message,
            "propagation operand must be a builtin optional or result value"
        );
    }

    #[test]
    fn lowering_propagate_missing_success_payload_emits_error_not_ice() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let some = ctx.intern("Some");
        let none = ctx.intern("None");
        let value = ctx.intern("value");
        let broken_optional_ty = ctx
            .type_registry
            .intern(TypeKind::AnonymousEnum(AnonymousEnum {
                backing_ty: Some(TypeId::U8),
                builtin: Some(BuiltinAnonymousEnumKind::Optional),
                variants: vec![
                    AnonymousVariant {
                        name: some,
                        name_span: Span::default(),
                        payload_ty: None,
                        explicit_value: None,
                    },
                    AnonymousVariant {
                        name: none,
                        name_span: Span::default(),
                        payload_ty: None,
                        explicit_value: None,
                    },
                ],
            }));
        let operand = Expr {
            id: NodeId(0),
            span: Span::default(),
            kind: ExprKind::Identifier(value),
        };
        ctx.set_node_type(operand.id, broken_optional_ty);

        let mut lowerer = Lowerer::new(&mut ctx);
        lowerer
            .local_types
            .push(HashMap::from([(value, (broken_optional_ty, false))]));
        lowerer.current_return_types.push(broken_optional_ty);

        let lowered = lowerer.lower_propagate(&operand, &HashMap::new(), Span::default());

        assert!(matches!(lowered, MastExprKind::Trap));
        assert_eq!(lowerer.ctx.sess.diagnostics.len(), 1);
        assert_eq!(
            lowerer.ctx.sess.diagnostics[0].level,
            DiagnosticLevel::Error
        );
        assert_eq!(
            lowerer.ctx.sess.diagnostics[0].message,
            "propagation success branch must carry a payload value"
        );
    }

    #[test]
    fn lowering_missing_payload_union_mapping_emits_error_not_ice() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);

        let payload = Lowerer::new(&mut ctx).payload_union_id(MonoId(77), Span::default());

        assert_eq!(payload, None);
        assert_eq!(ctx.sess.diagnostics.len(), 1);
        assert_eq!(ctx.sess.diagnostics[0].level, DiagnosticLevel::Error);
        assert_eq!(
            ctx.sess.diagnostics[0].message,
            "missing enum payload union mapping during lowering"
        );
    }

    #[test]
    fn lowering_struct_pattern_on_non_struct_def_emits_error_not_ice() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let union_name = ctx.intern("U");
        let field_name = ctx.intern("field");
        let def_id = ctx.add_def_with(|def_id| {
            Def::Union(UnionDef {
                id: def_id,
                name: union_name,
                vis: kernc_ast::Visibility::Private,
                parent_module: None,
                is_imported: false,
                generics: vec![],
                where_clauses: vec![],
                fields: vec![],
                is_extern: false,
                span: Span::default(),
                name_span: Span::default(),
                docs: None,
            })
        });
        let union_ty = ctx.type_registry.intern(TypeKind::Def(def_id, vec![]));

        let resolved = Lowerer::new(&mut ctx).resolve_struct_pattern_field(
            union_ty,
            field_name,
            Span::default(),
        );

        assert_eq!(resolved, None);
        assert_eq!(ctx.sess.diagnostics.len(), 1);
        assert_eq!(ctx.sess.diagnostics[0].level, DiagnosticLevel::Error);
        assert_eq!(
            ctx.sess.diagnostics[0].message,
            "destructuring pattern expected a struct type"
        );
    }

    #[test]
    fn lowering_module_member_without_def_id_emits_error_not_ice() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let module_name = ctx.intern("m");
        let field_name = ctx.intern("f");
        let root_scope = ctx.scopes.current_scope_id().unwrap();
        let mod_scope = ctx.scopes.enter_scope();
        ctx.scopes
            .define(
                field_name,
                SymbolInfo {
                    kind: SymbolKind::Function,
                    node_id: NodeId(99),
                    type_id: TypeId::ERROR,
                    def_id: None,
                    span: Span::default(),
                    vis: kernc_ast::Visibility::Private,
                    is_mut: false,
                },
            )
            .unwrap();
        ctx.scopes.set_current_scope(root_scope);
        let module_def_id = ctx.add_def_with(|module_def_id| {
            Def::Module(kernc_sema::def::ModuleDef {
                id: module_def_id,
                name: module_name,
                parent: None,
                is_imported: false,
                scope_id: mod_scope,
                dir_path: std::path::PathBuf::new(),
                file_id: kernc_utils::FileId(0),
                submodules: HashMap::default(),
                items: vec![],
                imports: vec![],
                is_init: false,
                docs: None,
            })
        });
        let module_ty = ctx.type_registry.intern(TypeKind::Module(module_def_id));
        let lhs = Expr {
            id: NodeId(0),
            span: Span::default(),
            kind: ExprKind::Identifier(module_name),
        };
        ctx.set_node_type(lhs.id, module_ty);

        let lowered = Lowerer::new(&mut ctx).lower_field_access(
            &lhs,
            field_name,
            &HashMap::new(),
            Span::default(),
        );

        assert!(matches!(lowered, MastExprKind::Trap));
        assert_eq!(ctx.sess.diagnostics.len(), 1);
        assert_eq!(ctx.sess.diagnostics[0].level, DiagnosticLevel::Error);
        assert_eq!(
            ctx.sess.diagnostics[0].message,
            "module member `f` cannot be used as a value because it has no definition backing it"
        );
    }
}
