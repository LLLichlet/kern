//! Block and statement lowering.
//!
//! Blocks maintain lowering-local scopes, defer stacks, local forwarding maps,
//! and local statics. This module lowers statements and block results while
//! restoring those stacks at block boundaries.

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
                    if this.check_canceled().is_err() {
                        break;
                    }
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
            if self.check_canceled().is_err() {
                break;
            }
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

    pub(crate) fn lower_while(
        &mut self,
        cond: &Expr,
        body: &Expr,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        _span: Span,
    ) -> MastExprKind {
        let mut loop_stmts = Vec::new();

        let c_expr = self.measure_phase("            lower_while_cond", |this| {
            this.lower_expr(cond, subst_map, Some(TypeId::BOOL))
        });
        let not_c = MastExpr::new(
            TypeId::BOOL,
            MastExprKind::Unary {
                op: ast::UnaryOperator::LogicalNot,
                operand: Box::new(c_expr),
            },
            cond.span,
        );

        loop_stmts.push(MastStmt::Expr(MastExpr::new(
            TypeId::VOID,
            MastExprKind::If {
                cond: Box::new(not_c),
                then_branch: MastBlock {
                    stmts: vec![MastStmt::Expr(MastExpr::new(
                        TypeId::VOID,
                        MastExprKind::Break,
                        cond.span,
                    ))],
                    result: None,
                    defers: vec![],
                },
                else_branch: None,
            },
            cond.span,
        )));

        // Record the defer-stack height before entering the loop body.
        self.loop_frames.push(self.defer_stack.len());
        loop_stmts.push(MastStmt::Expr(
            self.measure_phase("            lower_while_body", |this| {
                this.lower_expr(body, subst_map, None)
            }),
        ));

        let body_block = MastBlock {
            stmts: loop_stmts,
            result: None,
            defers: vec![],
        };

        // Leave the loop body and pop its control-flow boundary.
        self.loop_frames.pop();

        MastExprKind::Loop {
            body: body_block,
            latch: None,
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
                return self
                    .lower_error_kind(span, "`break` or `continue` cannot appear outside a loop");
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
