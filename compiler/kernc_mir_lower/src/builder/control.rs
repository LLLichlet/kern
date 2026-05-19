//! Control-flow lowering into MIR blocks and terminators.
//!
//! Structured MAST control constructs are lowered into explicit basic blocks,
//! branches, switches, gotos, and joins. Value-producing control flow writes
//! into a caller-provided destination place before jumping to the join block.

use super::*;

impl MirFunctionBuilder {
    pub(super) fn lower_value_block(
        &mut self,
        start: MirBlockId,
        block: &MastBlock,
        place: MirPlace,
    ) -> LowerResult<Option<MirBlockId>> {
        self.push_scope();
        let mut current = Some(start);
        for stmt in &block.stmts {
            let Some(block_id) = current else {
                self.pop_scope();
                return Ok(None);
            };
            current = self.lower_stmt(block_id, stmt)?;
        }

        let Some(block_id) = current else {
            self.pop_scope();
            return Ok(None);
        };

        let value_end = match block.result.as_deref() {
            Some(result) if result.ty == TypeId::NEVER => {
                let end = self.lower_never_block_tail(block_id, result, &block.defers, None)?;
                self.pop_scope();
                return Ok(end);
            }
            Some(result) if result.ty == TypeId::VOID || result.ty == TypeId::ERROR => {
                let Some(end_block) = self.lower_control_or_eval_stmt(block_id, result)? else {
                    self.pop_scope();
                    return Ok(None);
                };
                Some(end_block)
            }
            Some(result) => self.lower_expr_into_place(block_id, result, place.clone())?,
            None => Some(block_id),
        };

        let Some(block_id) = value_end else {
            self.pop_scope();
            return Ok(None);
        };
        // Value blocks must materialize their trailing result before running defers so
        // `{ ...; defer cleanup(); value }` yields the pre-cleanup value.
        let Some(block_id) = self.lower_block_defers(block_id, &block.defers)? else {
            self.pop_scope();
            return Ok(None);
        };
        let end = if block.result.is_some() {
            Some(block_id)
        } else {
            self.lower_value_tail(block_id, None, place)?
        };
        self.pop_scope();
        Ok(end)
    }

    pub(super) fn lower_value_if(
        &mut self,
        block_id: MirBlockId,
        cond: &MastExpr,
        then_branch: &MastBlock,
        else_branch: Option<&MastBlock>,
        place: MirPlace,
    ) -> LowerResult<Option<MirBlockId>> {
        let then_block = self.new_block();
        let else_block = self.new_block();
        let join = self.new_block();
        let mut cond_block = block_id;
        let cond_span = cond.span;
        let Some(cond) = self.lower_rvalue(&mut cond_block, cond)? else {
            return Ok(None);
        };
        self.set_terminator(
            cond_block,
            cond_span,
            MirTerminator::Branch {
                cond,
                then_block,
                else_block,
            },
        );
        let then_end = self.lower_value_block(then_block, then_branch, place.clone())?;
        if let Some(then_end) = then_end {
            self.set_terminator(then_end, cond_span, MirTerminator::Goto(join));
        }
        if let Some(else_branch) = else_branch {
            let else_end = self.lower_value_block(else_block, else_branch, place)?;
            if let Some(else_end) = else_end {
                self.set_terminator(else_end, cond_span, MirTerminator::Goto(join));
            }
        } else {
            self.set_terminator(else_block, cond_span, MirTerminator::Goto(join));
        }
        Ok(Some(join))
    }

    pub(super) fn lower_value_switch(
        &mut self,
        block_id: MirBlockId,
        target: &MastExpr,
        cases: &[kernc_mast::MastSwitchCase],
        default_case: Option<&MastBlock>,
        place: MirPlace,
    ) -> LowerResult<Option<MirBlockId>> {
        let join = self.new_block();
        let mut mir_cases = Vec::with_capacity(cases.len());
        for case in cases {
            let case_block = self.new_block();
            mir_cases.push(MirSwitchTarget {
                values: case.values.clone(),
                block: case_block,
            });
        }
        let default_block = default_case.as_ref().map(|_| self.new_block());
        let mut target_block = block_id;
        let target_span = target.span;
        let Some(target) = self.lower_rvalue(&mut target_block, target)? else {
            return Ok(None);
        };
        self.set_terminator(
            target_block,
            target_span,
            MirTerminator::Switch {
                target,
                cases: mir_cases.clone(),
                default_block,
            },
        );
        for (case, mir_case) in cases.iter().zip(mir_cases.iter()) {
            let end = self.lower_value_block(mir_case.block, &case.body, place.clone())?;
            if let Some(end) = end {
                self.set_terminator(end, target_span, MirTerminator::Goto(join));
            }
        }
        if let Some(default_case) = default_case {
            // `default_block` is created from the same `default_case` option above.
            let Some(default_id) = default_block else {
                return self.missing_switch_default_block(target_span);
            };
            let end = self.lower_value_block(default_id, default_case, place)?;
            if let Some(end) = end {
                self.set_terminator(end, target_span, MirTerminator::Goto(join));
            }
        }
        Ok(Some(join))
    }

    fn lower_value_tail(
        &mut self,
        block_id: MirBlockId,
        result: Option<&MastExpr>,
        place: MirPlace,
    ) -> LowerResult<Option<MirBlockId>> {
        let Some(result) = result else {
            return Ok(Some(block_id));
        };
        match &result.kind {
            MastExprKind::Block(block) => self.lower_value_block(block_id, block, place),
            MastExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.lower_value_if(block_id, cond, then_branch, else_branch.as_ref(), place),
            MastExprKind::Switch {
                target,
                cases,
                default_case,
            } => self.lower_value_switch(block_id, target, cases, default_case.as_ref(), place),
            MastExprKind::Return(_) | MastExprKind::Break | MastExprKind::Continue => {
                self.lower_tail(block_id, Some(result), None)
            }
            MastExprKind::Loop { .. } => {
                let _ = self.lower_control_or_eval_stmt(block_id, result)?;
                Ok(None)
            }
            MastExprKind::Unreachable => {
                self.set_terminator(block_id, result.span, MirTerminator::Unreachable);
                Ok(None)
            }
            MastExprKind::Trap => self.lower_tail(block_id, Some(result), None),
            MastExprKind::Breakpoint => self.unsupported_expr(result, "value tail position"),
            _ => self.lower_expr_into_place(block_id, result, place),
        }
    }

    pub(super) fn lower_block(
        &mut self,
        start: MirBlockId,
        block: &MastBlock,
        fallthrough: Option<MirBlockId>,
    ) -> LowerResult<Option<MirBlockId>> {
        self.push_scope();
        let mut current = Some(start);
        for stmt in &block.stmts {
            let Some(block_id) = current else {
                self.pop_scope();
                return Ok(None);
            };
            current = self.lower_stmt(block_id, stmt)?;
        }

        let Some(block_id) = current else {
            self.pop_scope();
            return Ok(None);
        };
        if let Some(result) = block.result.as_deref() {
            if result.ty == TypeId::NEVER {
                let end =
                    self.lower_never_block_tail(block_id, result, &block.defers, fallthrough)?;
                self.pop_scope();
                return Ok(end);
            }

            let mut block_id = block_id;
            if result.ty == TypeId::VOID || result.ty == TypeId::ERROR {
                let Some(end_block) = self.lower_control_or_eval_stmt(block_id, result)? else {
                    self.pop_scope();
                    return Ok(None);
                };
                block_id = end_block;
                let after_defers = self.lower_block_defers(block_id, &block.defers)?;
                let Some(block_id) = after_defers else {
                    self.pop_scope();
                    return Ok(None);
                };
                if let Some(next) = fallthrough {
                    self.set_terminator(block_id, result.span, MirTerminator::Goto(next));
                    self.pop_scope();
                    return Ok(Some(next));
                }
                self.set_terminator(block_id, result.span, MirTerminator::Return(None));
                self.pop_scope();
                return Ok(None);
            }

            let result_temp = self.new_temp_local(result.ty, result.span);
            let Some(end_block) =
                self.lower_expr_into_place(block_id, result, MirPlace::Local(result_temp))?
            else {
                self.pop_scope();
                return Ok(None);
            };
            let after_defers = self.lower_block_defers(end_block, &block.defers)?;
            let Some(block_id) = after_defers else {
                self.pop_scope();
                return Ok(None);
            };
            if let Some(next) = fallthrough {
                self.set_terminator(block_id, result.span, MirTerminator::Goto(next));
                self.pop_scope();
                return Ok(Some(next));
            }
            self.set_terminator(
                block_id,
                result.span,
                MirTerminator::Return(Some(MirRvalue::Use(MirOperand::Local(result_temp)))),
            );
            self.pop_scope();
            return Ok(None);
        }

        let Some(block_id) = self.lower_block_defers(block_id, &block.defers)? else {
            self.pop_scope();
            return Ok(None);
        };
        let end = self.lower_tail(block_id, None, fallthrough)?;
        self.pop_scope();
        Ok(end)
    }

    fn lower_block_defers(
        &mut self,
        block_id: MirBlockId,
        defers: &[MastExpr],
    ) -> LowerResult<Option<MirBlockId>> {
        let mut current = Some(block_id);
        for defer in defers {
            let Some(defer_block) = current else {
                return Ok(None);
            };
            current = self.lower_defer_expr(defer_block, defer)?;
        }
        Ok(current)
    }

    fn lower_never_block_tail(
        &mut self,
        block_id: MirBlockId,
        result: &MastExpr,
        defers: &[MastExpr],
        fallthrough: Option<MirBlockId>,
    ) -> LowerResult<Option<MirBlockId>> {
        match &result.kind {
            MastExprKind::Return(value) => {
                let mut block_id = block_id;
                let ret_value = match value.as_deref() {
                    Some(value) if value.ty != TypeId::VOID && value.ty != TypeId::ERROR => {
                        let ret_temp = self.new_temp_local(value.ty, value.span);
                        let Some(end_block) =
                            self.lower_expr_into_place(block_id, value, MirPlace::Local(ret_temp))?
                        else {
                            return Ok(None);
                        };
                        block_id = end_block;
                        Some(MirRvalue::Use(MirOperand::Local(ret_temp)))
                    }
                    Some(value) => {
                        let Some(end_block) = self.lower_control_or_eval_stmt(block_id, value)?
                        else {
                            return Ok(None);
                        };
                        block_id = end_block;
                        None
                    }
                    None => None,
                };
                let Some(block_id) = self.lower_block_defers(block_id, defers)? else {
                    return Ok(None);
                };
                self.set_terminator(block_id, result.span, MirTerminator::Return(ret_value));
                Ok(None)
            }
            MastExprKind::Break => {
                let Some(block_id) = self.lower_block_defers(block_id, defers)? else {
                    return Ok(None);
                };
                let break_block = self
                    .loop_stack
                    .last()
                    .map(|targets| targets.break_block)
                    .unwrap_or_else(|| self.new_block());
                self.set_terminator(block_id, result.span, MirTerminator::Goto(break_block));
                Ok(None)
            }
            MastExprKind::Continue => {
                let Some(block_id) = self.lower_block_defers(block_id, defers)? else {
                    return Ok(None);
                };
                let continue_block = self
                    .loop_stack
                    .last()
                    .map(|targets| targets.continue_block)
                    .unwrap_or_else(|| self.new_block());
                self.set_terminator(block_id, result.span, MirTerminator::Goto(continue_block));
                Ok(None)
            }
            _ => {
                let Some(block_id) = self.lower_block_defers(block_id, defers)? else {
                    return Ok(None);
                };
                self.lower_tail(block_id, Some(result), fallthrough)
            }
        }
    }

    fn lower_defer_expr(
        &mut self,
        block_id: MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<MirBlockId>> {
        match &expr.kind {
            MastExprKind::Trap => {
                self.emit_instruction(block_id, expr.span, MirInstruction::Trap);
                self.set_terminator(block_id, expr.span, MirTerminator::Unreachable);
                Ok(None)
            }
            MastExprKind::Breakpoint => {
                self.emit_instruction(block_id, expr.span, MirInstruction::Breakpoint);
                Ok(Some(block_id))
            }
            MastExprKind::Unreachable => {
                self.set_terminator(block_id, expr.span, MirTerminator::Unreachable);
                Ok(None)
            }
            _ => {
                let mut defer_block = block_id;
                let Some(defer_rvalue) = self.lower_rvalue(&mut defer_block, expr)? else {
                    return Ok(None);
                };
                self.emit_instruction(defer_block, expr.span, MirInstruction::Defer(defer_rvalue));
                Ok(Some(defer_block))
            }
        }
    }

    fn lower_stmt(
        &mut self,
        block_id: MirBlockId,
        stmt: &MastStmt,
    ) -> LowerResult<Option<MirBlockId>> {
        match stmt {
            MastStmt::Let {
                name,
                ty,
                is_mut,
                init,
            } => {
                let mut block_id = block_id;
                let init_span = init.span;
                let local = self.new_local(*name, init_span, *ty, *is_mut, MirLocalKind::Let);
                let Some(init) = self.lower_rvalue(&mut block_id, init)? else {
                    return Ok(None);
                };
                self.bind_local(*name, local);
                self.emit_instruction(
                    block_id,
                    init_span,
                    MirInstruction::Let {
                        place: MirPlace::Local(local),
                        init,
                    },
                );
                Ok(Some(block_id))
            }
            MastStmt::Expr(expr) => self.lower_control_or_eval_stmt(block_id, expr),
        }
    }

    pub(super) fn lower_control_or_eval_stmt(
        &mut self,
        block_id: MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<MirBlockId>> {
        let mut block_id = block_id;
        match &expr.kind {
            MastExprKind::Block(block) => {
                let join = self.new_block();
                let end = self.lower_block(block_id, block, Some(join))?;
                Ok(if end.is_none() { None } else { Some(join) })
            }
            MastExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let then_block = self.new_block();
                let else_block = self.new_block();
                let join = self.new_block();
                let cond_span = cond.span;
                let Some(cond) = self.lower_rvalue(&mut block_id, cond)? else {
                    return Ok(None);
                };
                self.set_terminator(
                    block_id,
                    cond_span,
                    MirTerminator::Branch {
                        cond,
                        then_block,
                        else_block,
                    },
                );
                let then_end = self.lower_block(then_block, then_branch, Some(join))?;
                if then_end.is_none() && else_branch.is_none() {
                    self.set_terminator(else_block, cond_span, MirTerminator::Goto(join));
                } else if let Some(else_branch) = else_branch {
                    let _ = self.lower_block(else_block, else_branch, Some(join))?;
                } else {
                    self.set_terminator(else_block, cond_span, MirTerminator::Goto(join));
                }
                Ok(Some(join))
            }
            MastExprKind::Switch {
                target,
                cases,
                default_case,
            } => {
                let join = self.new_block();
                let mut mir_cases = Vec::with_capacity(cases.len());
                for case in cases {
                    let case_block = self.new_block();
                    mir_cases.push(MirSwitchTarget {
                        values: case.values.clone(),
                        block: case_block,
                    });
                }
                let default_block = default_case.as_ref().map(|_| self.new_block());
                let target_span = target.span;
                let Some(target) = self.lower_rvalue(&mut block_id, target)? else {
                    return Ok(None);
                };
                self.set_terminator(
                    block_id,
                    target_span,
                    MirTerminator::Switch {
                        target,
                        cases: mir_cases.clone(),
                        default_block,
                    },
                );
                for (case, mir_case) in cases.iter().zip(mir_cases.iter()) {
                    let _ = self.lower_block(mir_case.block, &case.body, Some(join))?;
                }
                if let Some(default_case) = default_case {
                    // `default_block` is created from the same `default_case` option above.
                    let Some(default_block) = default_block else {
                        return self.missing_switch_default_block(target_span);
                    };
                    let _ = self.lower_block(default_block, default_case, Some(join))?;
                }
                Ok(Some(join))
            }
            MastExprKind::Loop { body, latch } => {
                let body_block = self.new_block();
                let continue_block = latch
                    .as_ref()
                    .map(|_| self.new_block())
                    .unwrap_or(body_block);
                let exit_block = self.new_block();
                self.set_terminator(block_id, expr.span, MirTerminator::Goto(body_block));
                self.loop_stack.push(MirLoopTargets {
                    break_block: exit_block,
                    continue_block,
                });
                let _ = self.lower_block(body_block, body, Some(continue_block))?;
                if let Some(latch) = latch {
                    let _ = self.lower_block(continue_block, latch, Some(body_block))?;
                }
                self.loop_stack.pop();
                Ok(Some(exit_block))
            }
            MastExprKind::Return(value) => {
                let ret_value = match value.as_deref() {
                    Some(value) => match self.lower_rvalue(&mut block_id, value)? {
                        Some(value) => Some(value),
                        None => return Ok(None),
                    },
                    None => None,
                };
                self.set_terminator(block_id, expr.span, MirTerminator::Return(ret_value));
                Ok(None)
            }
            MastExprKind::Break => {
                let break_block = self
                    .loop_stack
                    .last()
                    .map(|targets| targets.break_block)
                    .unwrap_or_else(|| self.new_block());
                self.set_terminator(block_id, expr.span, MirTerminator::Goto(break_block));
                Ok(None)
            }
            MastExprKind::Continue => {
                let continue_block = self
                    .loop_stack
                    .last()
                    .map(|targets| targets.continue_block)
                    .unwrap_or_else(|| self.new_block());
                self.set_terminator(block_id, expr.span, MirTerminator::Goto(continue_block));
                Ok(None)
            }
            MastExprKind::Assign { .. } => {
                if self
                    .lower_assign_instruction(&mut block_id, expr)?
                    .is_none()
                {
                    return Ok(None);
                }
                Ok(Some(block_id))
            }
            MastExprKind::Discard(inner) => self.lower_control_or_eval_stmt(block_id, inner),
            MastExprKind::Unreachable => {
                self.set_terminator(block_id, expr.span, MirTerminator::Unreachable);
                Ok(None)
            }
            MastExprKind::Trap => {
                self.emit_instruction(block_id, expr.span, MirInstruction::Trap);
                self.set_terminator(block_id, expr.span, MirTerminator::Unreachable);
                Ok(None)
            }
            MastExprKind::Breakpoint => {
                self.emit_instruction(block_id, expr.span, MirInstruction::Breakpoint);
                Ok(Some(block_id))
            }
            MastExprKind::Memcpy { .. }
            | MastExprKind::Memmove { .. }
            | MastExprKind::Memset { .. } => {
                if self
                    .lower_memory_instruction(&mut block_id, expr)?
                    .is_none()
                {
                    return Ok(None);
                }
                Ok(Some(block_id))
            }
            MastExprKind::AtomicStore { .. } | MastExprKind::Fence { .. } => {
                if self
                    .lower_atomic_instruction(&mut block_id, expr)?
                    .is_none()
                {
                    return Ok(None);
                }
                Ok(Some(block_id))
            }
            MastExprKind::SimdStore { .. }
            | MastExprKind::SimdMaskedStore { .. }
            | MastExprKind::SimdScatter { .. }
            | MastExprKind::SimdMaskedScatter { .. } => {
                if self
                    .lower_simd_memory_instruction(&mut block_id, expr)?
                    .is_none()
                {
                    return Ok(None);
                }
                Ok(Some(block_id))
            }
            MastExprKind::Asm(_) => {
                if self
                    .lower_inline_asm_instruction(&mut block_id, expr)?
                    .is_none()
                {
                    return Ok(None);
                }
                Ok(Some(block_id))
            }
            _ => {
                let Some(value) = self.lower_rvalue(&mut block_id, expr)? else {
                    return Ok(None);
                };
                self.emit_instruction(block_id, expr.span, MirInstruction::Eval(value));
                Ok(Some(block_id))
            }
        }
    }

    fn lower_tail(
        &mut self,
        block_id: MirBlockId,
        result: Option<&MastExpr>,
        fallthrough: Option<MirBlockId>,
    ) -> LowerResult<Option<MirBlockId>> {
        let mut block_id = block_id;
        let Some(result) = result else {
            if let Some(next) = fallthrough {
                self.set_terminator(block_id, Span::default(), MirTerminator::Goto(next));
                return Ok(Some(next));
            }
            self.set_terminator(block_id, Span::default(), MirTerminator::Return(None));
            return Ok(None);
        };

        match &result.kind {
            MastExprKind::Block(block) => self.lower_block(block_id, block, fallthrough),
            MastExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let then_block = self.new_block();
                let else_block = self.new_block();
                let cond_span = cond.span;
                let Some(cond) = self.lower_rvalue(&mut block_id, cond)? else {
                    return Ok(None);
                };
                self.set_terminator(
                    block_id,
                    cond_span,
                    MirTerminator::Branch {
                        cond,
                        then_block,
                        else_block,
                    },
                );
                let _ = self.lower_block(then_block, then_branch, fallthrough)?;
                if let Some(else_branch) = else_branch {
                    let _ = self.lower_block(else_block, else_branch, fallthrough)?;
                } else if let Some(next) = fallthrough {
                    self.set_terminator(else_block, cond_span, MirTerminator::Goto(next));
                } else {
                    self.set_terminator(else_block, cond_span, MirTerminator::Return(None));
                }
                Ok(fallthrough)
            }
            MastExprKind::Switch {
                target,
                cases,
                default_case,
            } => {
                let mut mir_cases = Vec::with_capacity(cases.len());
                for _ in cases {
                    mir_cases.push(MirSwitchTarget {
                        values: vec![],
                        block: self.new_block(),
                    });
                }
                for (mir_case, case) in mir_cases.iter_mut().zip(cases.iter()) {
                    mir_case.values = case.values.clone();
                    let _ = self.lower_block(mir_case.block, &case.body, fallthrough)?;
                }
                let default_block = if let Some(default_case) = default_case {
                    let id = self.new_block();
                    let _ = self.lower_block(id, default_case, fallthrough)?;
                    Some(id)
                } else {
                    None
                };
                let target_span = target.span;
                let Some(target) = self.lower_rvalue(&mut block_id, target)? else {
                    return Ok(None);
                };
                self.set_terminator(
                    block_id,
                    target_span,
                    MirTerminator::Switch {
                        target,
                        cases: mir_cases,
                        default_block,
                    },
                );
                Ok(fallthrough)
            }
            MastExprKind::Loop { .. }
            | MastExprKind::Return(_)
            | MastExprKind::Break
            | MastExprKind::Continue => self.lower_control_or_eval_stmt(block_id, result),
            MastExprKind::Unreachable => {
                self.set_terminator(block_id, result.span, MirTerminator::Unreachable);
                Ok(None)
            }
            MastExprKind::Trap => {
                self.emit_instruction(block_id, result.span, MirInstruction::Trap);
                self.set_terminator(block_id, result.span, MirTerminator::Unreachable);
                Ok(None)
            }
            MastExprKind::Breakpoint => {
                self.emit_instruction(block_id, result.span, MirInstruction::Breakpoint);
                if let Some(next) = fallthrough {
                    self.set_terminator(block_id, result.span, MirTerminator::Goto(next));
                    Ok(Some(next))
                } else {
                    Ok(Some(block_id))
                }
            }
            MastExprKind::Assign { .. } => {
                let Some(place) = self.lower_assign_instruction(&mut block_id, result)? else {
                    return Ok(None);
                };
                if let Some(next) = fallthrough {
                    self.set_terminator(block_id, result.span, MirTerminator::Goto(next));
                    Ok(Some(next))
                } else {
                    self.set_terminator(
                        block_id,
                        result.span,
                        MirTerminator::Return(Some(MirRvalue::Load(place))),
                    );
                    Ok(None)
                }
            }
            MastExprKind::Discard(inner) => {
                let Some(end_block) = self.lower_control_or_eval_stmt(block_id, inner)? else {
                    return Ok(None);
                };
                if let Some(next) = fallthrough {
                    self.set_terminator(end_block, result.span, MirTerminator::Goto(next));
                    Ok(Some(next))
                } else {
                    self.set_terminator(end_block, result.span, MirTerminator::Return(None));
                    Ok(None)
                }
            }
            MastExprKind::Memcpy { .. }
            | MastExprKind::Memmove { .. }
            | MastExprKind::Memset { .. } => {
                if self
                    .lower_memory_instruction(&mut block_id, result)?
                    .is_none()
                {
                    return Ok(None);
                }
                if let Some(next) = fallthrough {
                    self.set_terminator(block_id, result.span, MirTerminator::Goto(next));
                    Ok(Some(next))
                } else {
                    self.set_terminator(block_id, result.span, MirTerminator::Return(None));
                    Ok(None)
                }
            }
            MastExprKind::AtomicStore { .. } | MastExprKind::Fence { .. } => {
                if self
                    .lower_atomic_instruction(&mut block_id, result)?
                    .is_none()
                {
                    return Ok(None);
                }
                if let Some(next) = fallthrough {
                    self.set_terminator(block_id, result.span, MirTerminator::Goto(next));
                    Ok(Some(next))
                } else {
                    self.set_terminator(block_id, result.span, MirTerminator::Return(None));
                    Ok(None)
                }
            }
            MastExprKind::SimdStore { .. }
            | MastExprKind::SimdMaskedStore { .. }
            | MastExprKind::SimdScatter { .. }
            | MastExprKind::SimdMaskedScatter { .. } => {
                if self
                    .lower_simd_memory_instruction(&mut block_id, result)?
                    .is_none()
                {
                    return Ok(None);
                }
                if let Some(next) = fallthrough {
                    self.set_terminator(block_id, result.span, MirTerminator::Goto(next));
                    Ok(Some(next))
                } else {
                    self.set_terminator(block_id, result.span, MirTerminator::Return(None));
                    Ok(None)
                }
            }
            MastExprKind::Asm(_) => {
                if self
                    .lower_inline_asm_instruction(&mut block_id, result)?
                    .is_none()
                {
                    return Ok(None);
                }
                if let Some(next) = fallthrough {
                    self.set_terminator(block_id, result.span, MirTerminator::Goto(next));
                    Ok(Some(next))
                } else {
                    self.set_terminator(block_id, result.span, MirTerminator::Return(None));
                    Ok(None)
                }
            }
            _ => {
                let Some(lowered) = self.lower_rvalue(&mut block_id, result)? else {
                    return Ok(None);
                };
                if let Some(next) = fallthrough {
                    self.emit_instruction(block_id, result.span, MirInstruction::Eval(lowered));
                    self.set_terminator(block_id, result.span, MirTerminator::Goto(next));
                    Ok(Some(next))
                } else {
                    self.set_terminator(
                        block_id,
                        result.span,
                        MirTerminator::Return(Some(lowered)),
                    );
                    Ok(None)
                }
            }
        }
    }
}
