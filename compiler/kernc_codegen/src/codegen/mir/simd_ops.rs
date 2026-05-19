use super::*;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub(super) fn compile_mir_simd_unary_intrinsic(
        &mut self,
        body: &MirBody,
        kind: MirSimdUnaryIntrinsicKind,
        operand: &MirOperand,
        result_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let operand_val = self.compile_mir_operand(body, operand);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let Some((elem_ty, lanes)) = self.simd_elem_and_lanes(result_ty) else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR SIMD unary intrinsic expected a SIMD result type.",
            );
            return self.get_undef_val(result_llvm_ty);
        };

        match kind {
            MirSimdUnaryIntrinsicKind::Abs => {
                if self.type_registry.is_float(elem_ty) {
                    let Some(mask) = self.float_abs_mask_vector(elem_ty, lanes, Span::default())
                    else {
                        return self.get_undef_val(result_llvm_ty);
                    };
                    let int_mask_ty = mask.get_type();
                    let bits = self
                        .builder
                        .build_bit_cast(operand_val, int_mask_ty, "mir_simd_abs_bits")
                        .unwrap();
                    let cleared = self
                        .builder
                        .build_basic_and(bits, mask, "mir_simd_abs_clear_sign")
                        .unwrap();
                    self.builder
                        .build_bit_cast(cleared, result_llvm_ty, "mir_simd_abs")
                        .unwrap()
                } else {
                    let Some(zero) =
                        self.simd_zero_vector(result_ty, Span::default(), "MIR SIMD abs")
                    else {
                        return self.get_undef_val(result_llvm_ty);
                    };
                    let negated = self
                        .builder
                        .build_basic_neg(operand_val, "mir_simd_abs_neg")
                        .unwrap();
                    let mask = self
                        .builder
                        .build_basic_int_compare(
                            crate::IntPredicate::SLT,
                            operand_val,
                            zero,
                            "mir_simd_abs_is_neg",
                        )
                        .unwrap();
                    self.builder
                        .build_select(mask, negated, operand_val, "mir_simd_abs")
                        .unwrap()
                }
            }
            MirSimdUnaryIntrinsicKind::Sqrt => {
                self.compile_simd_float_intrinsic_call("llvm.sqrt", operand_val, result_llvm_ty)
            }
            MirSimdUnaryIntrinsicKind::Floor => {
                self.compile_simd_float_intrinsic_call("llvm.floor", operand_val, result_llvm_ty)
            }
            MirSimdUnaryIntrinsicKind::Ceil => {
                self.compile_simd_float_intrinsic_call("llvm.ceil", operand_val, result_llvm_ty)
            }
            MirSimdUnaryIntrinsicKind::Trunc => {
                self.compile_simd_float_intrinsic_call("llvm.trunc", operand_val, result_llvm_ty)
            }
            MirSimdUnaryIntrinsicKind::Round => {
                let truncated = self.compile_simd_float_intrinsic_call(
                    "llvm.trunc",
                    operand_val,
                    result_llvm_ty,
                );
                let Some(zero) =
                    self.simd_zero_vector(result_ty, Span::default(), "MIR SIMD round")
                else {
                    return self.get_undef_val(result_llvm_ty);
                };
                let Some(half) = self.float_constant_vector(
                    elem_ty,
                    lanes,
                    0.5,
                    Span::default(),
                    "MIR SIMD round",
                ) else {
                    return self.get_undef_val(result_llvm_ty);
                };
                let Some(one) = self.float_constant_vector(
                    elem_ty,
                    lanes,
                    1.0,
                    Span::default(),
                    "MIR SIMD round",
                ) else {
                    return self.get_undef_val(result_llvm_ty);
                };
                let Some(abs_mask) = self.float_abs_mask_vector(elem_ty, lanes, Span::default())
                else {
                    return self.get_undef_val(result_llvm_ty);
                };

                let delta = self
                    .builder
                    .build_basic_float_sub(operand_val, truncated, "mir_simd_round_delta")
                    .unwrap();
                let delta_bits = self
                    .builder
                    .build_bit_cast(delta, abs_mask.get_type(), "mir_simd_round_delta_bits")
                    .unwrap();
                let abs_delta = self
                    .builder
                    .build_basic_and(delta_bits, abs_mask, "mir_simd_round_abs_bits")
                    .unwrap();
                let abs_delta = self
                    .builder
                    .build_bit_cast(abs_delta, result_llvm_ty, "mir_simd_round_abs")
                    .unwrap();
                let needs_increment = self
                    .builder
                    .build_basic_float_compare(
                        crate::FloatPredicate::OGE,
                        abs_delta,
                        half,
                        "mir_simd_round_needs_increment",
                    )
                    .unwrap();
                let non_negative = self
                    .builder
                    .build_basic_float_compare(
                        crate::FloatPredicate::OGE,
                        operand_val,
                        zero,
                        "mir_simd_round_non_negative",
                    )
                    .unwrap();
                let rounded_up = self
                    .builder
                    .build_basic_float_add(truncated, one, "mir_simd_round_up")
                    .unwrap();
                let rounded_down = self
                    .builder
                    .build_basic_float_sub(truncated, one, "mir_simd_round_down")
                    .unwrap();
                let adjusted = self
                    .builder
                    .build_select(
                        non_negative,
                        rounded_up,
                        rounded_down,
                        "mir_simd_round_adjusted",
                    )
                    .unwrap();
                self.builder
                    .build_select(needs_increment, adjusted, truncated, "mir_simd_round")
                    .unwrap()
            }
        }
    }

    pub(super) fn compile_mir_simd_binary_intrinsic(
        &mut self,
        body: &MirBody,
        kind: MirSimdBinaryIntrinsicKind,
        lhs: &MirOperand,
        rhs: &MirOperand,
        result_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let lhs_val = self.compile_mir_operand(body, lhs);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let rhs_val = self.compile_mir_operand(body, rhs);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let Some((elem_ty, _)) = self.simd_elem_and_lanes(result_ty) else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR SIMD binary intrinsic expected a SIMD result type.",
            );
            return self.get_undef_val(result_llvm_ty);
        };

        let mask = if self.type_registry.is_float(elem_ty) {
            let pred = match kind {
                MirSimdBinaryIntrinsicKind::Min => crate::FloatPredicate::OLT,
                MirSimdBinaryIntrinsicKind::Max => crate::FloatPredicate::OGT,
            };
            self.builder
                .build_basic_float_compare(pred, lhs_val, rhs_val, "mir_simd_pairwise_cmp")
                .unwrap()
        } else {
            let pred = match kind {
                MirSimdBinaryIntrinsicKind::Min => {
                    Self::simd_int_pred(BinaryOperator::LessThan, self.is_signed_int(elem_ty))
                        .unwrap()
                }
                MirSimdBinaryIntrinsicKind::Max => {
                    Self::simd_int_pred(BinaryOperator::GreaterThan, self.is_signed_int(elem_ty))
                        .unwrap()
                }
            };
            self.builder
                .build_basic_int_compare(pred, lhs_val, rhs_val, "mir_simd_pairwise_cmp")
                .unwrap()
        };

        self.builder
            .build_select(mask, lhs_val, rhs_val, "mir_simd_pairwise")
            .unwrap()
    }

    pub(super) fn compile_mir_simd_reduce(
        &mut self,
        body: &MirBody,
        kind: MirSimdReduceKind,
        operand: &MirOperand,
        result_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let vector_val = self.compile_mir_operand(body, operand);
        let result_llvm_ty = self.get_llvm_type(result_ty);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let operand_ty = self.mir_operand_ty(body, operand).unwrap_or(TypeId::ERROR);
        let Some((elem_ty, lanes)) = self.simd_elem_and_lanes(operand_ty) else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR SIMD reduction expected a SIMD operand.",
            );
            return self.get_undef_val(result_llvm_ty);
        };
        let elem_norm = self.type_registry.normalize(elem_ty);
        let Some(vector_val) =
            self.expect_vector_value(vector_val, Span::default(), "MIR SIMD reduction operand")
        else {
            return self.get_undef_val(result_llvm_ty);
        };

        let mut acc = self
            .builder
            .build_extract_element(
                vector_val,
                self.context.i32_type().const_int(0, false),
                "mir_simd_reduce_init",
            )
            .unwrap();

        for lane in 1..lanes {
            let lane_val = self
                .builder
                .build_extract_element(
                    vector_val,
                    self.context.i32_type().const_int(lane as u64, false),
                    "mir_simd_reduce_lane",
                )
                .unwrap();

            acc = if self.type_registry.is_float(elem_norm) {
                let Some(acc_f) = self.expect_float_value(
                    acc,
                    Span::default(),
                    "MIR SIMD float reduction accumulator",
                ) else {
                    return self.get_undef_val(result_llvm_ty);
                };
                let Some(lane_f) = self.expect_float_value(
                    lane_val,
                    Span::default(),
                    "MIR SIMD float reduction lane",
                ) else {
                    return self.get_undef_val(result_llvm_ty);
                };
                match kind {
                    MirSimdReduceKind::Add => self
                        .builder
                        .build_float_add(acc_f, lane_f, "mir_simd_reduce_add")
                        .unwrap()
                        .into(),
                    MirSimdReduceKind::Mul => self
                        .builder
                        .build_float_mul(acc_f, lane_f, "mir_simd_reduce_mul")
                        .unwrap()
                        .into(),
                    MirSimdReduceKind::Min => {
                        let cond = self
                            .builder
                            .build_float_compare(
                                crate::FloatPredicate::OLT,
                                acc_f,
                                lane_f,
                                "mir_simd_reduce_min_cmp",
                            )
                            .unwrap();
                        self.builder
                            .build_select(
                                cond.into(),
                                acc_f.into(),
                                lane_f.into(),
                                "mir_simd_reduce_min",
                            )
                            .unwrap()
                    }
                    MirSimdReduceKind::Max => {
                        let cond = self
                            .builder
                            .build_float_compare(
                                crate::FloatPredicate::OGT,
                                acc_f,
                                lane_f,
                                "mir_simd_reduce_max_cmp",
                            )
                            .unwrap();
                        self.builder
                            .build_select(
                                cond.into(),
                                acc_f.into(),
                                lane_f.into(),
                                "mir_simd_reduce_max",
                            )
                            .unwrap()
                    }
                    MirSimdReduceKind::And | MirSimdReduceKind::Or | MirSimdReduceKind::Xor => {
                        self.sess.emit_ice(
                            Span::default(),
                            "Kern ICE (Codegen): invalid floating-point MIR SIMD reduction kind.",
                        );
                        return self.get_undef_val(result_llvm_ty);
                    }
                }
            } else {
                let Some(acc_i) = self.expect_int_value(
                    acc,
                    Span::default(),
                    "MIR SIMD integer reduction accumulator",
                ) else {
                    return self.get_undef_val(result_llvm_ty);
                };
                let Some(lane_i) = self.expect_int_value(
                    lane_val,
                    Span::default(),
                    "MIR SIMD integer reduction lane",
                ) else {
                    return self.get_undef_val(result_llvm_ty);
                };
                match kind {
                    MirSimdReduceKind::Add => self
                        .builder
                        .build_int_add(acc_i, lane_i, "mir_simd_reduce_add")
                        .unwrap()
                        .into(),
                    MirSimdReduceKind::Mul => self
                        .builder
                        .build_int_mul(acc_i, lane_i, "mir_simd_reduce_mul")
                        .unwrap()
                        .into(),
                    MirSimdReduceKind::And => self
                        .builder
                        .build_and(acc_i, lane_i, "mir_simd_reduce_and")
                        .unwrap()
                        .into(),
                    MirSimdReduceKind::Or => self
                        .builder
                        .build_or(acc_i, lane_i, "mir_simd_reduce_or")
                        .unwrap()
                        .into(),
                    MirSimdReduceKind::Xor => self
                        .builder
                        .build_xor(acc_i, lane_i, "mir_simd_reduce_xor")
                        .unwrap()
                        .into(),
                    MirSimdReduceKind::Min => {
                        let pred = if self.is_signed_int(elem_ty) {
                            crate::IntPredicate::SLT
                        } else {
                            crate::IntPredicate::ULT
                        };
                        let cond = self
                            .builder
                            .build_int_compare(pred, acc_i, lane_i, "mir_simd_reduce_min_cmp")
                            .unwrap();
                        self.builder
                            .build_select(
                                cond.into(),
                                acc_i.into(),
                                lane_i.into(),
                                "mir_simd_reduce_min",
                            )
                            .unwrap()
                    }
                    MirSimdReduceKind::Max => {
                        let pred = if self.is_signed_int(elem_ty) {
                            crate::IntPredicate::SGT
                        } else {
                            crate::IntPredicate::UGT
                        };
                        let cond = self
                            .builder
                            .build_int_compare(pred, acc_i, lane_i, "mir_simd_reduce_max_cmp")
                            .unwrap();
                        self.builder
                            .build_select(
                                cond.into(),
                                acc_i.into(),
                                lane_i.into(),
                                "mir_simd_reduce_max",
                            )
                            .unwrap()
                    }
                }
            };
        }

        acc
    }

    pub(super) fn compile_mir_simd_reduce_mask(
        &mut self,
        body: &MirBody,
        operand: &MirOperand,
        initial: bool,
        use_and: bool,
    ) -> BasicValueEnum<'ctx> {
        let vector_val = self.compile_mir_operand(body, operand);
        if self.current_block_is_terminated() {
            return self.context.bool_type().const_zero().into();
        }
        let operand_ty = self.mir_operand_ty(body, operand).unwrap_or(TypeId::ERROR);
        let Some((_, lanes)) = self.simd_elem_and_lanes(operand_ty) else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR SIMD mask reduction expected a SIMD operand.",
            );
            return self.zero_i8_value();
        };
        let Some(vector_val) = self.expect_vector_value(
            vector_val,
            Span::default(),
            "MIR SIMD mask reduction operand",
        ) else {
            return self.zero_i8_value();
        };

        let mut acc = self
            .context
            .bool_type()
            .const_int(if initial { 1 } else { 0 }, false);
        for lane in 0..lanes {
            let lane_idx = self.context.i32_type().const_int(lane as u64, false);
            let lane_val = self
                .builder
                .build_extract_element(vector_val, lane_idx, "mir_simd_reduce_lane")
                .unwrap();
            let Some(lane_val) =
                self.expect_int_value(lane_val, Span::default(), "MIR SIMD mask reduction lane")
            else {
                return self.zero_i8_value();
            };
            acc = if use_and {
                self.builder
                    .build_and(acc, lane_val, "mir_simd_all")
                    .unwrap()
            } else {
                self.builder
                    .build_or(acc, lane_val, "mir_simd_any")
                    .unwrap()
            };
        }
        acc.into()
    }

    pub(super) fn compile_mir_simd_bitmask(
        &mut self,
        body: &MirBody,
        operand: &MirOperand,
    ) -> BasicValueEnum<'ctx> {
        let result_ty = self
            .context
            .custom_width_int_type((self.sess.target.pointer_size as u32) * 8);
        let vector_val = self.compile_mir_operand(body, operand);
        if self.current_block_is_terminated() {
            return result_ty.const_zero().into();
        }
        let operand_ty = self.mir_operand_ty(body, operand).unwrap_or(TypeId::ERROR);
        let Some((elem_ty, lanes)) = self.simd_elem_and_lanes(operand_ty) else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR `simdBitmask` expected a SIMD mask operand.",
            );
            return result_ty.const_zero().into();
        };
        if elem_ty != TypeId::BOOL {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR `simdBitmask` expected `boolxN`.",
            );
            return result_ty.const_zero().into();
        }

        if self.sess.target.triple.endianness() == Ok(target_lexicon::Endianness::Little) {
            let packed_ty = self.context.custom_width_int_type(lanes as u32);
            let packed = self
                .builder
                .build_bit_cast(vector_val, packed_ty, "mir_simd_bitmask_pack")
                .unwrap();
            let Some(packed) =
                self.expect_int_value(packed, Span::default(), "MIR SIMD bitmask packed value")
            else {
                return result_ty.const_zero().into();
            };
            if packed_ty.bit_width() == result_ty.bit_width() {
                return packed.into();
            }
            return self
                .builder
                .build_int_z_extend(packed, result_ty, "mir_simd_bitmask_zext")
                .unwrap()
                .into();
        }

        let mut bits = result_ty.const_zero();
        let Some(vector_val) =
            self.expect_vector_value(vector_val, Span::default(), "MIR SIMD bitmask operand")
        else {
            return result_ty.const_zero().into();
        };
        for lane in 0..lanes {
            let lane_idx = self.context.i32_type().const_int(lane as u64, false);
            let lane_val = self
                .builder
                .build_extract_element(vector_val, lane_idx, "mir_simd_bitmask_lane")
                .unwrap();
            let Some(lane_val) =
                self.expect_int_value(lane_val, Span::default(), "MIR SIMD bitmask lane")
            else {
                return result_ty.const_zero().into();
            };
            let lane_ext = self
                .builder
                .build_int_z_extend(lane_val, result_ty, "mir_simd_bitmask_lane_zext")
                .unwrap();
            let lane_shift = result_ty.const_int(lane as u64, false);
            let lane_bit = self
                .builder
                .build_left_shift(lane_ext, lane_shift, "mir_simd_bitmask_shift")
                .unwrap();
            bits = self
                .builder
                .build_or(bits, lane_bit, "mir_simd_bitmask_or")
                .unwrap();
        }
        bits.into()
    }

    pub(super) fn compile_mir_simd_splat(
        &mut self,
        body: &MirBody,
        value: &MirOperand,
        result_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let scalar_val = self.compile_mir_operand(body, value);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let Some((_, lanes)) = self.simd_elem_and_lanes(result_ty) else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR SIMD splat expected a SIMD result type.",
            );
            return self.get_undef_val(result_llvm_ty);
        };
        let mut result = match result_llvm_ty {
            BasicTypeEnum::VectorType(vector_ty) => vector_ty.const_zero().into_vector_value(),
            other => {
                self.sess.emit_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Codegen): MIR SIMD splat expected a vector LLVM type, found `{:?}`.",
                        other
                    ),
                );
                return self.get_undef_val(result_llvm_ty);
            }
        };
        for lane in 0..lanes {
            let idx = self.context.i32_type().const_int(lane as u64, false);
            let result_value = self
                .builder
                .build_insert_element(result, scalar_val, idx, "mir_simd_splat")
                .unwrap();
            let Some(next_result) =
                self.expect_vector_value(result_value, Span::default(), "MIR SIMD splat result")
            else {
                return self.get_undef_val(result_llvm_ty);
            };
            result = next_result;
        }
        result.into()
    }

    pub(super) fn compile_mir_simd_cast(
        &mut self,
        body: &MirBody,
        value: &MirOperand,
        result_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let src_val = self.compile_mir_operand(body, value);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let src_ty = self.mir_operand_ty(body, value).unwrap_or(TypeId::ERROR);
        let Some((src_elem, src_lanes)) = self.simd_elem_and_lanes(src_ty) else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR SIMD cast expected a SIMD source value.",
            );
            return self.get_undef_val(result_llvm_ty);
        };
        let Some((dst_elem, dst_lanes)) = self.simd_elem_and_lanes(result_ty) else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR SIMD cast expected a SIMD result type.",
            );
            return self.get_undef_val(result_llvm_ty);
        };
        if src_lanes != dst_lanes {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR SIMD cast reached codegen with mismatched lane counts.",
            );
            return self.get_undef_val(result_llvm_ty);
        }

        let mut result = match result_llvm_ty {
            BasicTypeEnum::VectorType(vector_ty) => vector_ty.const_zero().into_vector_value(),
            other => {
                self.sess.emit_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Codegen): MIR SIMD cast expected a vector LLVM type, found `{:?}`.",
                        other
                    ),
                );
                return self.get_undef_val(result_llvm_ty);
            }
        };
        let Some(src_vec) =
            self.expect_vector_value(src_val, Span::default(), "MIR SIMD cast source value")
        else {
            return self.get_undef_val(result_llvm_ty);
        };
        for lane in 0..src_lanes {
            let idx = self.context.i32_type().const_int(lane as u64, false);
            let lane_val = self
                .builder
                .build_extract_element(src_vec, idx, "mir_simd_cast_lane")
                .unwrap();
            let cast_lane = self.compile_simd_scalar_cast_value(lane_val, src_elem, dst_elem);
            let result_value = self
                .builder
                .build_insert_element(result, cast_lane, idx, "mir_simd_cast_insert")
                .unwrap();
            let Some(next_result) = self.expect_vector_value(
                result_value,
                Span::default(),
                "MIR SIMD cast result vector",
            ) else {
                return self.get_undef_val(result_llvm_ty);
            };
            result = next_result;
        }
        result.into()
    }

    pub(super) fn compile_mir_simd_bitcast(
        &mut self,
        body: &MirBody,
        value: &MirOperand,
        result_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let src_val = self.compile_mir_operand(body, value);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        self.builder
            .build_bit_cast(src_val, result_llvm_ty, "mir_simd_bitcast")
            .unwrap()
    }

    pub(super) fn compile_mir_simd_select(
        &mut self,
        body: &MirBody,
        mask: &MirOperand,
        on_true: &MirOperand,
        on_false: &MirOperand,
    ) -> BasicValueEnum<'ctx> {
        let result_ty = self
            .mir_operand_ty(body, on_true)
            .map(|ty| self.get_llvm_type(ty))
            .unwrap_or_else(|| self.context.i8_type().into());
        let mask_val = self.compile_mir_operand(body, mask);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_ty);
        }
        let true_val = self.compile_mir_operand(body, on_true);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_ty);
        }
        let false_val = self.compile_mir_operand(body, on_false);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_ty);
        }
        self.builder
            .build_select(mask_val, true_val, false_val, "mir_simd_select")
            .unwrap()
    }

    pub(super) fn compile_mir_simd_shuffle(
        &mut self,
        body: &MirBody,
        lhs: &MirOperand,
        rhs: &MirOperand,
        indices: &[u32],
    ) -> BasicValueEnum<'ctx> {
        let lhs_ty = self
            .mir_operand_ty(body, lhs)
            .map(|ty| self.get_llvm_type(ty))
            .unwrap_or_else(|| self.context.i8_type().into());
        let lhs_val = self.compile_mir_operand(body, lhs);
        if self.current_block_is_terminated() {
            return self.get_undef_val(lhs_ty);
        }
        let rhs_val = self.compile_mir_operand(body, rhs);
        if self.current_block_is_terminated() {
            return self.get_undef_val(lhs_ty);
        }
        let mask_vals = indices
            .iter()
            .map(|&idx| self.context.i32_type().const_int(idx as u64, false).into())
            .collect::<Vec<BasicValueEnum<'ctx>>>();
        let mask = crate::llvm_api::const_vector(&mask_vals);
        let Some(lhs_vec) =
            self.expect_vector_value(lhs_val, Span::default(), "MIR SIMD shuffle lhs")
        else {
            return self.get_undef_val(lhs_ty);
        };
        let Some(rhs_vec) =
            self.expect_vector_value(rhs_val, Span::default(), "MIR SIMD shuffle rhs")
        else {
            return self.get_undef_val(lhs_ty);
        };
        self.builder
            .build_shuffle_vector(lhs_vec, rhs_vec, mask, "mir_simd_shuffle")
            .unwrap()
    }

    pub(super) fn compile_mir_simd_insert_half(
        &mut self,
        body: &MirBody,
        base: &MirOperand,
        half: &MirOperand,
        result_ty: TypeId,
        high_half: bool,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let base_val = self.compile_mir_operand(body, base);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let half_val = self.compile_mir_operand(body, half);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let Some((_, full_lanes)) = self.simd_elem_and_lanes(result_ty) else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR SIMD half insertion expected a SIMD result type.",
            );
            return self.get_undef_val(result_llvm_ty);
        };
        let half_ty = self.mir_operand_ty(body, half).unwrap_or(TypeId::ERROR);
        let Some((_, half_lanes)) = self.simd_elem_and_lanes(half_ty) else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR SIMD half insertion expected a SIMD half operand.",
            );
            return self.get_undef_val(result_llvm_ty);
        };
        if full_lanes != half_lanes.saturating_mul(2) {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR SIMD half insertion reached codegen with invalid lane counts.",
            );
            return self.get_undef_val(result_llvm_ty);
        }
        let Some(mut result) =
            self.expect_vector_value(base_val, Span::default(), "MIR SIMD half insertion base")
        else {
            return self.get_undef_val(result_llvm_ty);
        };
        let Some(half_vec) =
            self.expect_vector_value(half_val, Span::default(), "MIR SIMD half insertion half")
        else {
            return self.get_undef_val(result_llvm_ty);
        };
        let base_lane = if high_half { half_lanes } else { 0 };
        for lane in 0..half_lanes {
            let src_idx = self.context.i32_type().const_int(lane as u64, false);
            let dst_idx = self
                .context
                .i32_type()
                .const_int((base_lane + lane) as u64, false);
            let lane_val = self
                .builder
                .build_extract_element(half_vec, src_idx, "mir_simd_insert_half_lane")
                .unwrap();
            let result_value = self
                .builder
                .build_insert_element(result, lane_val, dst_idx, "mir_simd_insert_half")
                .unwrap();
            let Some(next_result) = self.expect_vector_value(
                result_value,
                Span::default(),
                "MIR SIMD half insertion result",
            ) else {
                return self.get_undef_val(result_llvm_ty);
            };
            result = next_result;
        }
        result.into()
    }

    pub(super) fn compile_mir_simd_binary(
        &mut self,
        body: &MirBody,
        op: BinaryOperator,
        lhs: &MirOperand,
        rhs: &MirOperand,
        result_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let lhs_val = self.compile_mir_operand(body, lhs);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let rhs_val = self.compile_mir_operand(body, rhs);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let lhs_ty = self.mir_operand_ty(body, lhs).unwrap_or(TypeId::ERROR);
        let Some((elem_ty, _)) = self.simd_elem_and_lanes(lhs_ty) else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR SIMD binary path reached with a non-SIMD type.",
            );
            return self.get_undef_val(result_llvm_ty);
        };
        let elem_norm = self.type_registry.normalize(elem_ty);
        let elem_is_float = self.type_registry.is_float(elem_norm);

        match op {
            BinaryOperator::Add => {
                if elem_is_float {
                    self.builder
                        .build_basic_float_add(lhs_val, rhs_val, "mir_simd_fadd")
                        .unwrap()
                } else {
                    self.builder
                        .build_basic_int_add(lhs_val, rhs_val, "mir_simd_add")
                        .unwrap()
                }
            }
            BinaryOperator::Subtract => {
                if elem_is_float {
                    self.builder
                        .build_basic_float_sub(lhs_val, rhs_val, "mir_simd_fsub")
                        .unwrap()
                } else {
                    self.builder
                        .build_basic_int_sub(lhs_val, rhs_val, "mir_simd_sub")
                        .unwrap()
                }
            }
            BinaryOperator::Multiply => {
                if elem_is_float {
                    self.builder
                        .build_basic_float_mul(lhs_val, rhs_val, "mir_simd_fmul")
                        .unwrap()
                } else {
                    self.builder
                        .build_basic_int_mul(lhs_val, rhs_val, "mir_simd_mul")
                        .unwrap()
                }
            }
            BinaryOperator::Divide => {
                if elem_is_float {
                    self.builder
                        .build_basic_float_div(lhs_val, rhs_val, "mir_simd_fdiv")
                        .unwrap()
                } else if self.is_signed_int(elem_ty) {
                    self.builder
                        .build_basic_int_signed_div(lhs_val, rhs_val, "mir_simd_sdiv")
                        .unwrap()
                } else {
                    self.builder
                        .build_basic_int_unsigned_div(lhs_val, rhs_val, "mir_simd_udiv")
                        .unwrap()
                }
            }
            BinaryOperator::Modulo => {
                if elem_is_float {
                    self.builder
                        .build_basic_float_rem(lhs_val, rhs_val, "mir_simd_frem")
                        .unwrap()
                } else if self.is_signed_int(elem_ty) {
                    self.builder
                        .build_basic_int_signed_rem(lhs_val, rhs_val, "mir_simd_srem")
                        .unwrap()
                } else {
                    self.builder
                        .build_basic_int_unsigned_rem(lhs_val, rhs_val, "mir_simd_urem")
                        .unwrap()
                }
            }
            BinaryOperator::Equal
            | BinaryOperator::NotEqual
            | BinaryOperator::LessThan
            | BinaryOperator::LessOrEqual
            | BinaryOperator::GreaterThan
            | BinaryOperator::GreaterOrEqual => {
                if elem_is_float {
                    let pred = Self::simd_float_pred(op).unwrap();
                    self.builder
                        .build_basic_float_compare(pred, lhs_val, rhs_val, "mir_simd_fcmp")
                        .unwrap()
                } else {
                    let pred = Self::simd_int_pred(op, self.is_signed_int(elem_ty)).unwrap();
                    self.builder
                        .build_basic_int_compare(pred, lhs_val, rhs_val, "mir_simd_icmp")
                        .unwrap()
                }
            }
            BinaryOperator::BitwiseAnd => self
                .builder
                .build_basic_and(lhs_val, rhs_val, "mir_simd_and")
                .unwrap(),
            BinaryOperator::BitwiseOr => self
                .builder
                .build_basic_or(lhs_val, rhs_val, "mir_simd_or")
                .unwrap(),
            BinaryOperator::BitwiseXor => self
                .builder
                .build_basic_xor(lhs_val, rhs_val, "mir_simd_xor")
                .unwrap(),
            BinaryOperator::ShiftLeft => self
                .builder
                .build_basic_shl(lhs_val, rhs_val, "mir_simd_shl")
                .unwrap(),
            BinaryOperator::ShiftRight => {
                if self.is_signed_int(elem_ty) {
                    self.builder
                        .build_basic_ashr(lhs_val, rhs_val, "mir_simd_ashr")
                        .unwrap()
                } else {
                    self.builder
                        .build_basic_lshr(lhs_val, rhs_val, "mir_simd_lshr")
                        .unwrap()
                }
            }
            BinaryOperator::LogicalAnd | BinaryOperator::LogicalOr => {
                self.sess.emit_ice(
                    Span::default(),
                    "Kern ICE (Codegen): logical short-circuit operators are not valid on MIR SIMD values.",
                );
                self.zero_i8_value()
            }
        }
    }
}
