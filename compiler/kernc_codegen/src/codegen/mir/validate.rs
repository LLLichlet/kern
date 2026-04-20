use super::*;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub(super) fn validate_mir_body_codegen_ready(
        &mut self,
        function: &MirFunction,
        body: &MirBody,
    ) -> bool {
        for block in &body.blocks {
            for instruction in &block.instructions {
                match &instruction.kind {
                    MirInstruction::Let { init, .. } => {
                        if !self.mir_rvalue_is_codegen_ready(body, init) {
                            self.emit_mir_codegen_error(
                                function,
                                "MIR let initializer is not codegen-ready",
                            );
                            return false;
                        }
                    }
                    MirInstruction::Assign { place, op, value } => {
                        if !self.mir_place_is_codegen_ready(place) {
                            self.emit_mir_codegen_error(
                                function,
                                "MIR assignment target place is not codegen-ready",
                            );
                            return false;
                        }
                        if !self.mir_rvalue_is_codegen_ready(body, value) {
                            self.emit_mir_codegen_error(
                                function,
                                "MIR assignment value is not codegen-ready",
                            );
                            return false;
                        }
                        if *op != AssignmentOperator::Assign
                            && self.mir_place_ty(body, place).is_none()
                        {
                            self.emit_mir_codegen_error(
                                function,
                                "MIR assignment target type recovery failed",
                            );
                            return false;
                        }
                    }
                    MirInstruction::Memory(intrinsic) => {
                        let ready = match intrinsic {
                            MirMemoryIntrinsic::Copy { dest, src, len }
                            | MirMemoryIntrinsic::Move { dest, src, len } => {
                                self.mir_operand_is_codegen_ready(dest)
                                    && self.mir_operand_is_codegen_ready(src)
                                    && self.mir_operand_is_codegen_ready(len)
                            }
                            MirMemoryIntrinsic::Set { dest, val, len } => {
                                self.mir_operand_is_codegen_ready(dest)
                                    && self.mir_operand_is_codegen_ready(val)
                                    && self.mir_operand_is_codegen_ready(len)
                            }
                        };
                        if !ready {
                            self.emit_mir_codegen_error(
                                function,
                                "MIR memory intrinsic operand is not codegen-ready",
                            );
                            return false;
                        }
                    }
                    MirInstruction::InlineAsm(asm) => {
                        if !asm
                            .input_args
                            .iter()
                            .chain(asm.output_ptrs.iter())
                            .all(|operand| self.mir_operand_is_codegen_ready(operand))
                        {
                            self.emit_mir_codegen_error(
                                function,
                                "MIR inline asm operand is not codegen-ready",
                            );
                            return false;
                        }
                    }
                    MirInstruction::SimdStore { ptr, value, .. } => {
                        if !(self.mir_operand_is_codegen_ready(ptr)
                            && self.mir_operand_is_codegen_ready(value))
                        {
                            self.emit_mir_codegen_error(
                                function,
                                "MIR SIMD store operand is not codegen-ready",
                            );
                            return false;
                        }
                    }
                    MirInstruction::SimdMaskedStore {
                        ptr, mask, value, ..
                    } => {
                        if !(self.mir_operand_is_codegen_ready(ptr)
                            && self.mir_operand_is_codegen_ready(mask)
                            && self.mir_operand_is_codegen_ready(value))
                        {
                            self.emit_mir_codegen_error(
                                function,
                                "MIR masked SIMD store operand is not codegen-ready",
                            );
                            return false;
                        }
                    }
                    MirInstruction::SimdScatter {
                        ptr,
                        indices,
                        value,
                    } => {
                        if !(self.mir_operand_is_codegen_ready(ptr)
                            && self.mir_operand_is_codegen_ready(indices)
                            && self.mir_operand_is_codegen_ready(value))
                        {
                            self.emit_mir_codegen_error(
                                function,
                                "MIR SIMD scatter operand is not codegen-ready",
                            );
                            return false;
                        }
                    }
                    MirInstruction::SimdMaskedScatter {
                        ptr,
                        indices,
                        mask,
                        value,
                    } => {
                        if !(self.mir_operand_is_codegen_ready(ptr)
                            && self.mir_operand_is_codegen_ready(indices)
                            && self.mir_operand_is_codegen_ready(mask)
                            && self.mir_operand_is_codegen_ready(value))
                        {
                            self.emit_mir_codegen_error(
                                function,
                                "MIR masked SIMD scatter operand is not codegen-ready",
                            );
                            return false;
                        }
                    }
                    MirInstruction::AtomicStore { ptr, value, .. } => {
                        if !(self.mir_operand_is_codegen_ready(ptr)
                            && self.mir_operand_is_codegen_ready(value))
                        {
                            self.emit_mir_codegen_error(
                                function,
                                "MIR atomic store operand is not codegen-ready",
                            );
                            return false;
                        }
                    }
                    MirInstruction::Fence { .. }
                    | MirInstruction::Trap
                    | MirInstruction::Breakpoint => {}
                    MirInstruction::Eval(rvalue) | MirInstruction::Defer(rvalue) => {
                        if !self.mir_rvalue_is_codegen_ready(body, rvalue) {
                            self.emit_mir_codegen_error(
                                function,
                                "MIR eval/defer rvalue is not codegen-ready",
                            );
                            return false;
                        }
                    }
                }
            }

            match &block.terminator.kind {
                MirTerminator::Goto(_) | MirTerminator::Unreachable => {}
                MirTerminator::Branch { cond, .. } => {
                    if !self.mir_rvalue_is_codegen_ready(body, cond) {
                        self.emit_mir_codegen_error(
                            function,
                            "MIR branch condition is not codegen-ready",
                        );
                        return false;
                    }
                }
                MirTerminator::Switch { target, .. } => {
                    if !self.mir_rvalue_is_codegen_ready(body, target) {
                        self.emit_mir_codegen_error(
                            function,
                            "MIR switch target is not codegen-ready",
                        );
                        return false;
                    }
                }
                MirTerminator::Return(value) => {
                    if let Some(value) = value
                        && !self.mir_rvalue_is_codegen_ready(body, value)
                    {
                        self.emit_mir_codegen_error(
                            function,
                            "MIR return value is not codegen-ready",
                        );
                        return false;
                    }
                }
            }
        }

        true
    }

    pub(super) fn emit_mir_codegen_error(&mut self, function: &MirFunction, reason: &str) {
        self.sess.emit_ice(
            Span::default(),
            format!(
                "Kern ICE (Codegen): MIR function `{}` reached codegen with invalid MIR/codegen boundary: {}.",
                function.name, reason
            ),
        );
    }

    pub(super) fn mir_const_is_codegen_ready(&self, value: &MirConst) -> bool {
        let _ = value;
        true
    }

    pub(super) fn mir_operand_is_codegen_ready(&self, operand: &MirOperand) -> bool {
        match operand {
            MirOperand::Local(_) => true,
            MirOperand::Const(value) => self.mir_const_is_codegen_ready(value),
        }
    }

    pub(super) fn mir_place_is_codegen_ready(&self, place: &MirPlace) -> bool {
        match place {
            MirPlace::Local(_) | MirPlace::Global(_) => true,
            MirPlace::Deref(operand) => self.mir_operand_is_codegen_ready(operand),
            MirPlace::Field { base, .. } => self.mir_place_is_codegen_ready(base),
            MirPlace::Index { base, index } => {
                self.mir_place_is_codegen_ready(base) && self.mir_operand_is_codegen_ready(index)
            }
        }
    }

    pub(super) fn mir_rvalue_is_codegen_ready(&self, body: &MirBody, rvalue: &MirRvalue) -> bool {
        let _ = body;
        match rvalue {
            MirRvalue::Use(operand)
            | MirRvalue::Projection { operand, .. }
            | MirRvalue::Unary { operand, .. }
            | MirRvalue::Cast { operand, .. }
            | MirRvalue::BitIntrinsic { operand, .. }
            | MirRvalue::SimdUnaryIntrinsic { operand, .. }
            | MirRvalue::SimdReduce { operand, .. }
            | MirRvalue::SimdAny { operand }
            | MirRvalue::SimdAll { operand }
            | MirRvalue::SimdBitmask { operand }
            | MirRvalue::SimdSplat { value: operand }
            | MirRvalue::SimdCast { value: operand }
            | MirRvalue::SimdBitcast { value: operand } => {
                self.mir_operand_is_codegen_ready(operand)
            }
            MirRvalue::Call { callee, args } => {
                (match callee {
                    MirCallTarget::Direct(_) => true,
                    MirCallTarget::Operand(operand) => self.mir_operand_is_codegen_ready(operand),
                }) && args
                    .iter()
                    .all(|operand| self.mir_operand_is_codegen_ready(operand))
            }
            MirRvalue::Aggregate { fields, .. } => fields
                .iter()
                .all(|operand| self.mir_operand_is_codegen_ready(operand)),
            MirRvalue::Binary { lhs, rhs, .. }
            | MirRvalue::SimdBinaryIntrinsic { lhs, rhs, .. } => {
                self.mir_operand_is_codegen_ready(lhs) && self.mir_operand_is_codegen_ready(rhs)
            }
            MirRvalue::AtomicLoad { ptr, .. } => self.mir_operand_is_codegen_ready(ptr),
            MirRvalue::AtomicCas {
                ptr,
                expected,
                desired,
                ..
            } => {
                self.mir_operand_is_codegen_ready(ptr)
                    && self.mir_operand_is_codegen_ready(expected)
                    && self.mir_operand_is_codegen_ready(desired)
            }
            MirRvalue::AtomicRmw { ptr, value, .. } => {
                self.mir_operand_is_codegen_ready(ptr) && self.mir_operand_is_codegen_ready(value)
            }
            MirRvalue::SimdLoad { ptr, .. } => self.mir_operand_is_codegen_ready(ptr),
            MirRvalue::SimdMaskedLoad {
                ptr, mask, or_else, ..
            } => {
                self.mir_operand_is_codegen_ready(ptr)
                    && self.mir_operand_is_codegen_ready(mask)
                    && self.mir_operand_is_codegen_ready(or_else)
            }
            MirRvalue::SimdGather { ptr, indices } => {
                self.mir_operand_is_codegen_ready(ptr) && self.mir_operand_is_codegen_ready(indices)
            }
            MirRvalue::SimdMaskedGather {
                ptr,
                indices,
                mask,
                or_else,
            } => {
                self.mir_operand_is_codegen_ready(ptr)
                    && self.mir_operand_is_codegen_ready(indices)
                    && self.mir_operand_is_codegen_ready(mask)
                    && self.mir_operand_is_codegen_ready(or_else)
            }
            MirRvalue::SliceOp {
                lhs,
                start,
                end,
                is_inclusive: _,
            } => {
                (match lhs {
                    MirSliceBase::Operand(operand) => self.mir_operand_is_codegen_ready(operand),
                    MirSliceBase::Place(place) => self.mir_place_is_codegen_ready(place),
                }) && start
                    .iter()
                    .all(|operand| self.mir_operand_is_codegen_ready(operand))
                    && end
                        .iter()
                        .all(|operand| self.mir_operand_is_codegen_ready(operand))
            }
            MirRvalue::SimdSelect {
                mask,
                on_true,
                on_false,
            } => {
                self.mir_operand_is_codegen_ready(mask)
                    && self.mir_operand_is_codegen_ready(on_true)
                    && self.mir_operand_is_codegen_ready(on_false)
            }
            MirRvalue::SimdShuffle { lhs, rhs, .. } => {
                self.mir_operand_is_codegen_ready(lhs) && self.mir_operand_is_codegen_ready(rhs)
            }
            MirRvalue::SimdInsertHalf { base, half, .. } => {
                self.mir_operand_is_codegen_ready(base) && self.mir_operand_is_codegen_ready(half)
            }
            MirRvalue::AddressOf(place) | MirRvalue::Load(place) => {
                self.mir_place_is_codegen_ready(place)
            }
        }
    }
}
