use super::*;

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(crate) fn lower_block_as_body(
        &mut self,
        block_expr: &Expr,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        expected_ty: TypeId,
    ) -> MastBlock {
        self.measure_phase("      lower_block_scope_push", |this| {
            this.defer_stack.push(Vec::new());
            this.local_types.push(HashMap::new());
            this.local_forwardings.push(HashMap::new());
            this.local_value_forwardings.push(HashMap::new());
            this.local_statics.push(HashMap::new());
        });

        let mut stmts = Vec::new();
        let mut result = None;

        if let ExprKind::Block {
            stmts: ast_stmts,
            result: ast_res,
        } = &block_expr.kind
        {
            self.measure_phase("      lower_block_stmts", |this| {
                for stmt in ast_stmts {
                    match &stmt.kind {
                        ast::StmtKind::Use(_) => {}
                        ast::StmtKind::ExprStmt(e) | ast::StmtKind::ExprValue(e) => {
                            this.lower_block_stmt(e, subst_map, &mut stmts);
                        }
                    }
                }
            });
            if let Some(res) = ast_res {
                result = Some(Box::new(
                    self.measure_phase("      lower_block_result", |this| {
                        this.lower_expr(res, subst_map, Some(expected_ty))
                    }),
                ));
            }
        } else {
            result = Some(Box::new(
                self.measure_phase("      lower_block_expr", |this| {
                    this.lower_expr(block_expr, subst_map, Some(expected_ty))
                }),
            ));
        }

        let popped_defers = self.measure_phase("      lower_block_defers", |this| {
            this.pop_defer_scope(block_expr.span)
        });
        let mut defers = Vec::new();
        for d in popped_defers.into_iter().rev() {
            defers.push(d); // Preserve LIFO order in a dedicated array.
        }

        self.measure_phase("      lower_block_scope_pop", |this| {
            this.local_types.pop();
            this.local_forwardings.pop();
            this.local_value_forwardings.pop();
            this.local_statics.pop();
        });
        MastBlock {
            stmts,
            result,
            defers,
        } // Pass defers to the backend separately.
    }

    pub(crate) fn lower_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: Option<&Expr>,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        exp_ty: TypeId,
    ) -> MastExprKind {
        let c = self.measure_phase("            lower_if_cond", |this| {
            this.lower_expr(cond, subst_map, Some(TypeId::BOOL))
        });
        let t = self.measure_phase("            lower_if_then", |this| {
            this.lower_block_as_body(then_branch, subst_map, exp_ty)
        });
        let e = else_branch.map(|eb| {
            self.measure_phase("            lower_if_else", |this| {
                this.lower_block_as_body(eb, subst_map, exp_ty)
            })
        });
        MastExprKind::If {
            cond: Box::new(c),
            then_branch: t,
            else_branch: e,
        }
    }

    pub(crate) fn lower_for(
        &mut self,
        init: Option<&Expr>,
        cond: Option<&Expr>,
        post: Option<&Expr>,
        body: &Expr,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        span: Span,
    ) -> MastExprKind {
        let has_init_scope = init.is_some();
        if has_init_scope {
            self.local_types.push(HashMap::new());
            self.local_forwardings.push(HashMap::new());
            self.local_value_forwardings.push(HashMap::new());
        }

        let mut outer_stmts = Vec::new();
        if let Some(i) = init {
            match &i.kind {
                ExprKind::Let {
                    pattern,
                    init,
                    else_clause,
                } => outer_stmts.extend(self.measure_phase("            lower_for_init", |this| {
                    this.lower_let_stmts(
                        i,
                        pattern,
                        init,
                        else_clause.as_ref(),
                        subst_map,
                    )
                })),
                _ => {
                    if let Some(stmt) = self.measure_phase("            lower_for_init", |this| {
                        this.lower_optional_stmt_expr(i, subst_map)
                    }) {
                        outer_stmts.push(stmt);
                    }
                }
            }
        }

        let mut loop_stmts = Vec::new();

        if let Some(c) = cond {
            let c_expr = self.measure_phase("            lower_for_cond", |this| {
                this.lower_expr(c, subst_map, Some(TypeId::BOOL))
            });
            let not_c = MastExpr::new(
                TypeId::BOOL,
                MastExprKind::Unary {
                    op: ast::UnaryOperator::LogicalNot,
                    operand: Box::new(c_expr),
                },
                c.span,
            );

            loop_stmts.push(MastStmt::Expr(MastExpr::new(
                TypeId::VOID,
                MastExprKind::If {
                    cond: Box::new(not_c),
                    then_branch: MastBlock {
                        stmts: vec![MastStmt::Expr(MastExpr::new(
                            TypeId::VOID,
                            MastExprKind::Break,
                            c.span,
                        ))],
                        result: None,
                        defers: vec![],
                    },
                    else_branch: None,
                },
                c.span,
            )));
        }

        // Record the defer-stack height before entering the loop body.
        self.loop_frames.push(self.defer_stack.len());
        // Lower the loop body without the post expression.
        loop_stmts.push(MastStmt::Expr(
            self.measure_phase("            lower_for_body", |this| {
                this.lower_expr(body, subst_map, None)
            }),
        ));

        let body_block = MastBlock {
            stmts: loop_stmts,
            result: None,
            defers: vec![],
        };

        // Lower the post statement separately as the latch block.
        let latch_block = post.map(|p| {
            self.measure_phase("            lower_for_post", |this| MastBlock {
                stmts: this
                    .lower_optional_stmt_expr(p, subst_map)
                    .into_iter()
                    .collect(),
                result: None,
                defers: vec![],
            })
        });

        // Leave the loop body and pop its control-flow boundary.
        self.loop_frames.pop();

        let loop_expr = MastExpr::new(
            TypeId::VOID,
            // Handle the newer AST representation.
            MastExprKind::Loop {
                body: body_block,
                latch: latch_block,
            },
            span,
        );

        if has_init_scope {
            outer_stmts.push(MastStmt::Expr(loop_expr));
            let block = MastExprKind::Block(MastBlock {
                stmts: outer_stmts,
                result: None,
                defers: vec![],
            });
            self.local_types.pop();
            self.local_forwardings.pop();
            self.local_value_forwardings.pop();
            block
        } else {
            loop_expr.kind
        }
    }

    pub(crate) fn lower_return(
        &mut self,
        val: Option<&Expr>,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        span: Span,
    ) -> MastExprKind {
        let expected_ret_ty = self.current_return_type(span);
        let v = self.measure_phase("            lower_return_value", |this| {
            val.map(|e| this.lower_expr(e, subst_map, expected_ret_ty))
        });
        self.lower_return_lowered_value(v, span)
    }

    /// Expand defers specifically for `break` and `continue`.
    pub(crate) fn lower_jump(&mut self, jump_kind: MastExprKind, span: Span) -> MastExprKind {
        let mut defer_stmts = Vec::new();

        // Find the defer-stack depth at the start of the current loop.
        let boundary = match self.loop_frames.last().copied() {
            Some(b) => b,
            None => {
                self.ctx.emit_ice(
                    span,
                    "Kern ICE (Lowering): `break` or `continue` found outside any loop frame.",
                );
                return MastExprKind::Trap;
            }
        };

        // Walk backward through the defer stack until the loop boundary is reached.
        if boundary > self.defer_stack.len() {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Lowering): loop frame boundary exceeds current defer stack depth.",
            );
            return MastExprKind::Trap;
        }

        for stack in self.defer_stack[boundary..].iter().rev() {
            for d in stack.iter().rev() {
                defer_stmts.push(MastStmt::Expr(d.clone()));
            }
        }

        if defer_stmts.is_empty() {
            jump_kind
        } else {
            // Emit the real jump only after all cleanup work has run.
            defer_stmts.push(MastStmt::Expr(MastExpr::new(
                TypeId::NEVER,
                jump_kind,
                span,
            )));
            MastExprKind::Block(MastBlock {
                stmts: defer_stmts,
                result: None,
                defers: vec![],
            })
        }
    }
}
