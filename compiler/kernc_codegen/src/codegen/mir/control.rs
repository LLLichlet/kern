use super::*;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub(super) fn compile_mir_instruction(&mut self, body: &MirBody, instruction: &MirInstruction) {
        match instruction {
            MirInstruction::Let { place, init } => {
                let expected_ty = self.mir_place_ty(body, place);
                let value = self.compile_mir_rvalue(body, init, expected_ty);
                if self.current_block_is_terminated() {
                    return;
                }
                let Some(place_ty) = expected_ty else {
                    self.sess.emit_ice(
                        Span::default(),
                        "Kern ICE (Codegen): MIR let target has no recoverable type.",
                    );
                    return;
                };
                self.compile_mir_store(body, place, value, place_ty, Span::default());
            }
            MirInstruction::Assign { place, op, value } => {
                let Some(place_ty) = self.mir_place_ty(body, place) else {
                    self.sess.emit_ice(
                        Span::default(),
                        format!(
                            "Kern ICE (Codegen): MIR assignment target has no recoverable type: {:?}.",
                            place
                        ),
                    );
                    return;
                };

                if *op == AssignmentOperator::Assign {
                    let rhs = self.compile_mir_rvalue(body, value, Some(place_ty));
                    if self.current_block_is_terminated() {
                        return;
                    }
                    self.compile_mir_store(body, place, rhs, place_ty, Span::default());
                    return;
                }

                let lhs = self.compile_mir_place_load(body, place, place_ty, Span::default());
                if self.current_block_is_terminated() {
                    return;
                }
                let rhs_hint = self.mir_rvalue_ty(body, value, Some(place_ty));
                let rhs = self.compile_mir_rvalue(body, value, rhs_hint);
                if self.current_block_is_terminated() {
                    return;
                }
                let updated = self.compile_mir_assign_op(*op, lhs, rhs, place_ty, Span::default());
                if self.current_block_is_terminated() {
                    return;
                }
                self.compile_mir_store(body, place, updated, place_ty, Span::default());
            }
            MirInstruction::Memory(intrinsic) => {
                self.compile_mir_memory_instruction(body, intrinsic)
            }
            MirInstruction::InlineAsm(asm) => self.compile_mir_inline_asm(body, asm),
            MirInstruction::SimdStore { ptr, value, align } => {
                self.compile_mir_simd_store(body, ptr, value, *align)
            }
            MirInstruction::SimdMaskedStore {
                ptr,
                mask,
                value,
                align,
            } => self.compile_mir_simd_masked_store(body, ptr, mask, value, *align),
            MirInstruction::SimdScatter {
                ptr,
                indices,
                value,
            } => self.compile_mir_simd_scatter(body, ptr, indices, value),
            MirInstruction::SimdMaskedScatter {
                ptr,
                indices,
                mask,
                value,
            } => self.compile_mir_simd_masked_scatter(body, ptr, indices, mask, value),
            MirInstruction::AtomicStore {
                ptr,
                value,
                ordering,
            } => self.compile_mir_atomic_store(body, ptr, value, *ordering),
            MirInstruction::Fence { ordering } => self.compile_mir_atomic_fence(*ordering),
            MirInstruction::Trap => self.compile_mir_trap(),
            MirInstruction::Breakpoint => self.compile_mir_breakpoint(),
            MirInstruction::Eval(rvalue) | MirInstruction::Defer(rvalue) => {
                let hint = self.mir_rvalue_ty(body, rvalue, None);
                let _ = self.compile_mir_rvalue(body, rvalue, hint);
            }
        }
    }

    pub(super) fn compile_mir_memory_instruction(
        &mut self,
        body: &MirBody,
        intrinsic: &MirMemoryIntrinsic,
    ) {
        match intrinsic {
            MirMemoryIntrinsic::Copy { dest, src, len } => {
                let dest = self.compile_mir_operand(body, dest).into_pointer_value();
                let src = self.compile_mir_operand(body, src).into_pointer_value();
                let len = self.compile_mir_operand(body, len).into_int_value();
                self.builder.build_memcpy(dest, 1, src, 1, len).unwrap();
            }
            MirMemoryIntrinsic::Move { dest, src, len } => {
                let dest = self.compile_mir_operand(body, dest).into_pointer_value();
                let src = self.compile_mir_operand(body, src).into_pointer_value();
                let len = self.compile_mir_operand(body, len).into_int_value();
                self.builder.build_memmove(dest, 1, src, 1, len).unwrap();
            }
            MirMemoryIntrinsic::Set { dest, val, len } => {
                let dest = self.compile_mir_operand(body, dest).into_pointer_value();
                let val = self.compile_mir_operand(body, val).into_int_value();
                let len = self.compile_mir_operand(body, len).into_int_value();
                self.builder.build_memset(dest, 1, val, len).unwrap();
            }
        }
    }

    pub(super) fn compile_mir_terminator(
        &mut self,
        body: &MirBody,
        function: &MirFunction,
        blocks: &HashMap<MirBlockId, BasicBlock<'ctx>>,
        terminator: &MirTerminator,
    ) {
        match terminator {
            MirTerminator::Goto(target) => {
                let Some(block) = blocks.get(target).copied() else {
                    self.sess.emit_ice(
                        Span::default(),
                        format!(
                            "Kern ICE (Codegen): MIR goto target {:?} missing from block map.",
                            target
                        ),
                    );
                    return;
                };
                self.builder.build_unconditional_branch(block).unwrap();
            }
            MirTerminator::Branch {
                cond,
                then_block,
                else_block,
            } => {
                let cond_val = self
                    .compile_mir_rvalue(body, cond, Some(TypeId::BOOL))
                    .into_int_value();
                let Some(then_bb) = blocks.get(then_block).copied() else {
                    self.sess.emit_ice(
                        Span::default(),
                        format!(
                            "Kern ICE (Codegen): MIR then-block {:?} missing from block map.",
                            then_block
                        ),
                    );
                    return;
                };
                let Some(else_bb) = blocks.get(else_block).copied() else {
                    self.sess.emit_ice(
                        Span::default(),
                        format!(
                            "Kern ICE (Codegen): MIR else-block {:?} missing from block map.",
                            else_block
                        ),
                    );
                    return;
                };
                self.builder
                    .build_conditional_branch(cond_val, then_bb, else_bb)
                    .unwrap();
            }
            MirTerminator::Switch {
                target,
                cases,
                default_block,
            } => {
                let target_ty = self
                    .mir_rvalue_ty(body, target, None)
                    .unwrap_or(TypeId::USIZE);
                let target_val = self
                    .compile_mir_rvalue(body, target, Some(target_ty))
                    .into_int_value();
                let default_bb = default_block
                    .and_then(|id| blocks.get(&id).copied())
                    .unwrap_or_else(|| {
                        let current = self
                            .builder
                            .get_insert_block()
                            .and_then(|block| block.get_parent())
                            .expect("current MIR block must have parent function");
                        self.context
                            .append_basic_block(current, "mir_switch_default_unreachable")
                    });

                let mut llvm_cases = Vec::new();
                for case in cases {
                    let Some(case_block) = blocks.get(&case.block).copied() else {
                        self.sess.emit_ice(
                            Span::default(),
                            format!(
                                "Kern ICE (Codegen): MIR switch block {:?} missing from block map.",
                                case.block
                            ),
                        );
                        continue;
                    };
                    for value in &case.values {
                        llvm_cases.push((target_val.get_type().const_u128(*value), case_block));
                    }
                }

                self.builder
                    .build_switch(target_val, default_bb, &llvm_cases)
                    .unwrap();

                if default_block.is_none() {
                    self.builder.position_at_end(default_bb);
                    self.builder.build_unreachable().unwrap();
                }
            }
            MirTerminator::Return(value) => {
                if function.ret_ty == TypeId::VOID {
                    if let Some(value) = value {
                        let _ = self.compile_mir_rvalue(body, value, Some(TypeId::VOID));
                        if self.current_block_is_terminated() {
                            return;
                        }
                    }
                    self.builder.build_return(None).unwrap();
                    return;
                }

                let Some(value) = value else {
                    self.sess.emit_ice(
                        Span::default(),
                        format!(
                            "Kern ICE (Codegen): non-void MIR function `{}` returned no value.",
                            function.name
                        ),
                    );
                    self.builder.build_unreachable().unwrap();
                    return;
                };

                let ret = self.compile_mir_rvalue(body, value, Some(function.ret_ty));
                if self.current_block_is_terminated() {
                    return;
                }
                self.builder.build_return(Some(&ret)).unwrap();
            }
            MirTerminator::Unreachable => {
                self.builder.build_unreachable().unwrap();
            }
        }
    }
}
