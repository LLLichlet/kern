use crate::codegen::CodeGenerator;
use crate::intrinsics::Intrinsic;
use crate::llvm_api::const_vector;
use crate::types::BasicTypeEnum;
use crate::values::{BasicValueEnum, FloatValue, FunctionValue, IntValue};
use crate::{FloatPredicate, IntPredicate};
use kernc_ast::{self as ast, BinaryOperator};
use kernc_mast::{MastExpr, SimdBinaryIntrinsicKind, SimdReduceKind, SimdUnaryIntrinsicKind};
use kernc_sema::ty::{PrimitiveType, TypeId, TypeKind};
use kernc_utils::Span;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    fn simd_elem_and_lanes(&self, ty: TypeId) -> Option<(TypeId, u16)> {
        self.type_registry.simd_info(ty)
    }

    fn current_function_for_simd_memory(&mut self, context: &str) -> Option<FunctionValue<'ctx>> {
        let Some(block) = self.builder.get_insert_block() else {
            self.sess.emit_ice(
                Span::default(),
                format!(
                    "Kern ICE (Codegen): missing insertion block while compiling {}.",
                    context
                ),
            );
            return None;
        };
        let Some(func) = block.get_parent() else {
            self.sess.emit_ice(
                Span::default(),
                format!(
                    "Kern ICE (Codegen): insertion block has no parent function while compiling {}.",
                    context
                ),
            );
            return None;
        };
        Some(func)
    }

    fn simd_int_pred(op: BinaryOperator, is_signed: bool) -> Option<IntPredicate> {
        match op {
            BinaryOperator::Equal => Some(IntPredicate::EQ),
            BinaryOperator::NotEqual => Some(IntPredicate::NE),
            BinaryOperator::LessThan => Some(if is_signed {
                IntPredicate::SLT
            } else {
                IntPredicate::ULT
            }),
            BinaryOperator::LessOrEqual => Some(if is_signed {
                IntPredicate::SLE
            } else {
                IntPredicate::ULE
            }),
            BinaryOperator::GreaterThan => Some(if is_signed {
                IntPredicate::SGT
            } else {
                IntPredicate::UGT
            }),
            BinaryOperator::GreaterOrEqual => Some(if is_signed {
                IntPredicate::SGE
            } else {
                IntPredicate::UGE
            }),
            _ => None,
        }
    }

    fn simd_float_pred(op: BinaryOperator) -> Option<FloatPredicate> {
        match op {
            BinaryOperator::Equal => Some(FloatPredicate::OEQ),
            BinaryOperator::NotEqual => Some(FloatPredicate::ONE),
            BinaryOperator::LessThan => Some(FloatPredicate::OLT),
            BinaryOperator::LessOrEqual => Some(FloatPredicate::OLE),
            BinaryOperator::GreaterThan => Some(FloatPredicate::OGT),
            BinaryOperator::GreaterOrEqual => Some(FloatPredicate::OGE),
            _ => None,
        }
    }

    fn simd_zero_vector(
        &mut self,
        ty: TypeId,
        span: Span,
        context: &str,
    ) -> Option<BasicValueEnum<'ctx>> {
        match self.get_llvm_type(ty) {
            BasicTypeEnum::VectorType(vector_ty) => Some(vector_ty.const_zero().into()),
            _ => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): expected SIMD vector type while compiling {}.",
                        context
                    ),
                );
                None
            }
        }
    }

    fn float_abs_mask_vector(
        &mut self,
        elem_ty: TypeId,
        lanes: u16,
        span: Span,
    ) -> Option<BasicValueEnum<'ctx>> {
        let (mask_ty, lane_mask) = match elem_ty {
            TypeId::F32 => (self.context.i32_type(), 0x7FFF_FFFF_u64),
            TypeId::F64 => (self.context.i64_type(), 0x7FFF_FFFF_FFFF_FFFF_u64),
            _ => {
                self.sess.emit_ice(
                    span,
                    "Kern ICE (Codegen): floating-point SIMD abs expected `f32` or `f64` lanes.",
                );
                return None;
            }
        };

        let lanes = (0..lanes)
            .map(|_| mask_ty.const_int(lane_mask, false).into())
            .collect::<Vec<_>>();
        Some(const_vector(&lanes).into())
    }

    fn float_constant_vector(
        &mut self,
        elem_ty: TypeId,
        lanes: u16,
        value: f64,
        span: Span,
        context: &str,
    ) -> Option<BasicValueEnum<'ctx>> {
        let lane_value = match elem_ty {
            TypeId::F32 => self.context.f32_type().const_float(value).into(),
            TypeId::F64 => self.context.f64_type().const_float(value).into(),
            _ => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): floating-point SIMD constant expected `f32` or `f64` lanes while compiling {}.",
                        context
                    ),
                );
                return None;
            }
        };

        let lanes = (0..lanes).map(|_| lane_value).collect::<Vec<_>>();
        Some(const_vector(&lanes).into())
    }

    fn reduce_simd_mask(
        &mut self,
        operand: &MastExpr,
        initial: bool,
        use_and: bool,
    ) -> BasicValueEnum<'ctx> {
        let vector_val = self.compile_expr(operand);
        let expected_bool = self.context.bool_type();
        if let Some(fallback) = self.expr_terminated_fallback(expected_bool.into()) {
            return fallback;
        }

        let Some((_, lanes)) = self.simd_elem_and_lanes(operand.ty) else {
            self.sess.emit_ice(
                operand.span,
                "Kern ICE (Codegen): SIMD reduction expected a SIMD operand.",
            );
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
                .build_extract_element(vector_val.into_vector_value(), lane_idx, "simd_reduce_lane")
                .unwrap()
                .into_int_value();
            acc = if use_and {
                self.builder.build_and(acc, lane_val, "simd_all").unwrap()
            } else {
                self.builder.build_or(acc, lane_val, "simd_any").unwrap()
            };
        }
        acc.into()
    }

    pub(crate) fn compile_simd_reduce_any(&mut self, operand: &MastExpr) -> BasicValueEnum<'ctx> {
        self.reduce_simd_mask(operand, false, false)
    }

    pub(crate) fn compile_simd_reduce_all(&mut self, operand: &MastExpr) -> BasicValueEnum<'ctx> {
        self.reduce_simd_mask(operand, true, true)
    }

    fn compile_simd_float_intrinsic_call(
        &mut self,
        intrinsic_name: &str,
        operand_val: BasicValueEnum<'ctx>,
        result_llvm_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let intrinsic = Intrinsic::find(intrinsic_name).unwrap();
        let decl = intrinsic
            .get_declaration(&self.module, &[result_llvm_ty])
            .unwrap();
        self.builder
            .build_call(decl, &[operand_val], "simd_float_intrinsic")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
    }

    pub(crate) fn compile_simd_unary_intrinsic(
        &mut self,
        kind: SimdUnaryIntrinsicKind,
        operand: &MastExpr,
        result_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let operand_val = self.compile_expr(operand);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }

        let Some((elem_ty, lanes)) = self.simd_elem_and_lanes(result_ty) else {
            self.sess.emit_ice(
                operand.span,
                "Kern ICE (Codegen): SIMD unary intrinsic expected a SIMD result type.",
            );
            return self.get_undef_val(result_llvm_ty);
        };

        match kind {
            SimdUnaryIntrinsicKind::Abs => {
                if self.type_registry.is_float(elem_ty) {
                    let Some(mask) = self.float_abs_mask_vector(elem_ty, lanes, operand.span)
                    else {
                        return self.get_undef_val(result_llvm_ty);
                    };
                    let int_mask_ty = mask.get_type();
                    let bits = self
                        .builder
                        .build_bit_cast(operand_val, int_mask_ty, "simd_abs_bits")
                        .unwrap();
                    let cleared = self
                        .builder
                        .build_basic_and(bits, mask, "simd_abs_clear_sign")
                        .unwrap();
                    self.builder
                        .build_bit_cast(cleared, result_llvm_ty, "simd_abs")
                        .unwrap()
                } else {
                    let Some(zero) = self.simd_zero_vector(result_ty, operand.span, "SIMD abs")
                    else {
                        return self.get_undef_val(result_llvm_ty);
                    };
                    let negated = self
                        .builder
                        .build_basic_neg(operand_val, "simd_abs_neg")
                        .unwrap();
                    let mask = self
                        .builder
                        .build_basic_int_compare(
                            IntPredicate::SLT,
                            operand_val,
                            zero,
                            "simd_abs_is_neg",
                        )
                        .unwrap();
                    self.builder
                        .build_select(mask, negated, operand_val, "simd_abs")
                        .unwrap()
                }
            }
            SimdUnaryIntrinsicKind::Sqrt => {
                self.compile_simd_float_intrinsic_call("llvm.sqrt", operand_val, result_llvm_ty)
            }
            SimdUnaryIntrinsicKind::Floor => {
                self.compile_simd_float_intrinsic_call("llvm.floor", operand_val, result_llvm_ty)
            }
            SimdUnaryIntrinsicKind::Ceil => {
                self.compile_simd_float_intrinsic_call("llvm.ceil", operand_val, result_llvm_ty)
            }
            SimdUnaryIntrinsicKind::Trunc => {
                self.compile_simd_float_intrinsic_call("llvm.trunc", operand_val, result_llvm_ty)
            }
            SimdUnaryIntrinsicKind::Round => {
                let truncated = self.compile_simd_float_intrinsic_call(
                    "llvm.trunc",
                    operand_val,
                    result_llvm_ty,
                );
                let Some(zero) = self.simd_zero_vector(result_ty, operand.span, "SIMD round")
                else {
                    return self.get_undef_val(result_llvm_ty);
                };
                let Some(half) =
                    self.float_constant_vector(elem_ty, lanes, 0.5, operand.span, "SIMD round")
                else {
                    return self.get_undef_val(result_llvm_ty);
                };
                let Some(one) =
                    self.float_constant_vector(elem_ty, lanes, 1.0, operand.span, "SIMD round")
                else {
                    return self.get_undef_val(result_llvm_ty);
                };
                let Some(abs_mask) = self.float_abs_mask_vector(elem_ty, lanes, operand.span)
                else {
                    return self.get_undef_val(result_llvm_ty);
                };

                let delta = self
                    .builder
                    .build_basic_float_sub(operand_val, truncated, "simd_round_delta")
                    .unwrap();
                let delta_bits = self
                    .builder
                    .build_bit_cast(delta, abs_mask.get_type(), "simd_round_delta_bits")
                    .unwrap();
                let abs_delta = self
                    .builder
                    .build_basic_and(delta_bits, abs_mask, "simd_round_abs_bits")
                    .unwrap();
                let abs_delta = self
                    .builder
                    .build_bit_cast(abs_delta, result_llvm_ty, "simd_round_abs")
                    .unwrap();
                let needs_increment = self
                    .builder
                    .build_basic_float_compare(
                        FloatPredicate::OGE,
                        abs_delta,
                        half,
                        "simd_round_needs_increment",
                    )
                    .unwrap();
                let non_negative = self
                    .builder
                    .build_basic_float_compare(
                        FloatPredicate::OGE,
                        operand_val,
                        zero,
                        "simd_round_non_negative",
                    )
                    .unwrap();
                let rounded_up = self
                    .builder
                    .build_basic_float_add(truncated, one, "simd_round_up")
                    .unwrap();
                let rounded_down = self
                    .builder
                    .build_basic_float_sub(truncated, one, "simd_round_down")
                    .unwrap();
                let adjusted = self
                    .builder
                    .build_select(
                        non_negative,
                        rounded_up,
                        rounded_down,
                        "simd_round_adjusted",
                    )
                    .unwrap();
                self.builder
                    .build_select(needs_increment, adjusted, truncated, "simd_round")
                    .unwrap()
            }
        }
    }

    pub(crate) fn compile_simd_binary_intrinsic(
        &mut self,
        kind: SimdBinaryIntrinsicKind,
        lhs: &MastExpr,
        rhs: &MastExpr,
        result_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let lhs_val = self.compile_expr(lhs);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }
        let rhs_val = self.compile_expr(rhs);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }

        let Some((elem_ty, _)) = self.simd_elem_and_lanes(result_ty) else {
            self.sess.emit_ice(
                lhs.span,
                "Kern ICE (Codegen): SIMD binary intrinsic expected a SIMD result type.",
            );
            return self.get_undef_val(result_llvm_ty);
        };

        let mask = if self.type_registry.is_float(elem_ty) {
            let pred = match kind {
                SimdBinaryIntrinsicKind::Min => FloatPredicate::OLT,
                SimdBinaryIntrinsicKind::Max => FloatPredicate::OGT,
            };
            self.builder
                .build_basic_float_compare(pred, lhs_val, rhs_val, "simd_pairwise_cmp")
                .unwrap()
        } else {
            let pred = match kind {
                SimdBinaryIntrinsicKind::Min => {
                    Self::simd_int_pred(BinaryOperator::LessThan, self.is_signed_int(elem_ty))
                        .unwrap()
                }
                SimdBinaryIntrinsicKind::Max => {
                    Self::simd_int_pred(BinaryOperator::GreaterThan, self.is_signed_int(elem_ty))
                        .unwrap()
                }
            };
            self.builder
                .build_basic_int_compare(pred, lhs_val, rhs_val, "simd_pairwise_cmp")
                .unwrap()
        };

        self.builder
            .build_select(mask, lhs_val, rhs_val, "simd_pairwise")
            .unwrap()
    }

    fn compile_simd_scalar_cast_value(
        &mut self,
        value: BasicValueEnum<'ctx>,
        src_elem: TypeId,
        dst_elem: TypeId,
    ) -> BasicValueEnum<'ctx> {
        if src_elem == dst_elem {
            return value;
        }

        let dst_llvm_ty = self.get_llvm_type(dst_elem);
        let src_is_int = self.type_registry.is_integer(src_elem) || src_elem == TypeId::BOOL;
        let dst_is_int = self.type_registry.is_integer(dst_elem);
        let src_is_float = self.type_registry.is_float(src_elem);
        let dst_is_float = self.type_registry.is_float(dst_elem);

        if src_is_int && dst_is_int {
            let src_int = value.into_int_value();
            let src_bits = src_int.get_type().bit_width();
            let dst_int_ty = dst_llvm_ty.into_int_type();
            let dst_bits = dst_int_ty.bit_width();
            return if src_bits == dst_bits {
                src_int.into()
            } else if src_bits > dst_bits {
                self.builder
                    .build_int_truncate(src_int, dst_int_ty, "simd_cast_trunc")
                    .unwrap()
                    .into()
            } else if src_elem == TypeId::BOOL || !self.is_signed_int(src_elem) {
                self.builder
                    .build_int_z_extend(src_int, dst_int_ty, "simd_cast_zext")
                    .unwrap()
                    .into()
            } else {
                self.builder
                    .build_int_s_extend(src_int, dst_int_ty, "simd_cast_sext")
                    .unwrap()
                    .into()
            };
        }

        if src_is_int && dst_is_float {
            let src_int = value.into_int_value();
            let dst_float_ty = dst_llvm_ty.into_float_type();
            return if src_elem == TypeId::BOOL || !self.is_signed_int(src_elem) {
                self.builder
                    .build_unsigned_int_to_float(src_int, dst_float_ty, "simd_cast_uitofp")
                    .unwrap()
                    .into()
            } else {
                self.builder
                    .build_signed_int_to_float(src_int, dst_float_ty, "simd_cast_sitofp")
                    .unwrap()
                    .into()
            };
        }

        if src_is_float && dst_is_int {
            let src_float = value.into_float_value();
            let dst_int_ty = dst_llvm_ty.into_int_type();
            return if self.is_signed_int(dst_elem) {
                self.builder
                    .build_float_to_signed_int(src_float, dst_int_ty, "simd_cast_fptosi")
                    .unwrap()
                    .into()
            } else {
                self.builder
                    .build_float_to_unsigned_int(src_float, dst_int_ty, "simd_cast_fptoui")
                    .unwrap()
                    .into()
            };
        }

        if src_is_float && dst_is_float {
            let src_float = value.into_float_value();
            let src_bits = if src_elem == TypeId::F32 { 32 } else { 64 };
            let dst_float_ty = dst_llvm_ty.into_float_type();
            let dst_bits = if dst_elem == TypeId::F32 { 32 } else { 64 };
            return if src_bits == dst_bits {
                src_float.into()
            } else {
                self.builder
                    .build_float_cast(src_float, dst_float_ty, "simd_cast_fcast")
                    .unwrap()
                    .into()
            };
        }

        self.sess.emit_ice(
            Span::default(),
            "Kern ICE (Codegen): unsupported SIMD scalar cast pair.",
        );
        self.get_undef_val(dst_llvm_ty)
    }

    pub(crate) fn compile_simd_splat(
        &mut self,
        value: &MastExpr,
        result_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let scalar_val = self.compile_expr(value);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }

        let Some((_, lanes)) = self.simd_elem_and_lanes(result_ty) else {
            self.sess.emit_ice(
                value.span,
                "Kern ICE (Codegen): SIMD splat expected a SIMD result type.",
            );
            return self.get_undef_val(result_llvm_ty);
        };

        let mut result = match result_llvm_ty {
            BasicTypeEnum::VectorType(vector_ty) => vector_ty.const_zero().into_vector_value(),
            other => {
                self.sess.emit_ice(
                    value.span,
                    format!(
                        "Kern ICE (Codegen): SIMD splat expected a vector LLVM type, found `{:?}`.",
                        other
                    ),
                );
                return self.get_undef_val(result_llvm_ty);
            }
        };

        for lane in 0..lanes {
            let idx = self.context.i32_type().const_int(lane as u64, false);
            result = self
                .builder
                .build_insert_element(result, scalar_val, idx, "simd_splat")
                .unwrap()
                .into_vector_value();
        }

        result.into()
    }

    pub(crate) fn compile_simd_cast(
        &mut self,
        value: &MastExpr,
        result_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let src_val = self.compile_expr(value);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }

        let Some((src_elem, src_lanes)) = self.simd_elem_and_lanes(value.ty) else {
            self.sess.emit_ice(
                value.span,
                "Kern ICE (Codegen): SIMD cast expected a SIMD source value.",
            );
            return self.get_undef_val(result_llvm_ty);
        };
        let Some((dst_elem, dst_lanes)) = self.simd_elem_and_lanes(result_ty) else {
            self.sess.emit_ice(
                value.span,
                "Kern ICE (Codegen): SIMD cast expected a SIMD result type.",
            );
            return self.get_undef_val(result_llvm_ty);
        };
        if src_lanes != dst_lanes {
            self.sess.emit_ice(
                value.span,
                "Kern ICE (Codegen): SIMD cast reached codegen with mismatched lane counts.",
            );
            return self.get_undef_val(result_llvm_ty);
        }

        let mut result = match result_llvm_ty {
            BasicTypeEnum::VectorType(vector_ty) => vector_ty.const_zero().into_vector_value(),
            other => {
                self.sess.emit_ice(
                    value.span,
                    format!(
                        "Kern ICE (Codegen): SIMD cast expected a vector LLVM type, found `{:?}`.",
                        other
                    ),
                );
                return self.get_undef_val(result_llvm_ty);
            }
        };

        let src_vec = src_val.into_vector_value();
        for lane in 0..src_lanes {
            let idx = self.context.i32_type().const_int(lane as u64, false);
            let lane_val = self
                .builder
                .build_extract_element(src_vec, idx, "simd_cast_lane")
                .unwrap();
            let cast_lane = self.compile_simd_scalar_cast_value(lane_val, src_elem, dst_elem);
            result = self
                .builder
                .build_insert_element(result, cast_lane, idx, "simd_cast_insert")
                .unwrap()
                .into_vector_value();
        }

        result.into()
    }

    pub(crate) fn compile_simd_bitcast(
        &mut self,
        value: &MastExpr,
        result_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let src_val = self.compile_expr(value);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }

        self.builder
            .build_bit_cast(src_val, result_llvm_ty, "simd_bitcast")
            .unwrap()
    }

    pub(crate) fn compile_simd_reduce(
        &mut self,
        kind: SimdReduceKind,
        operand: &MastExpr,
        result_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let vector_val = self.compile_expr(operand);
        let result_llvm_ty = self.get_llvm_type(result_ty);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }

        let Some((elem_ty, lanes)) = self.simd_elem_and_lanes(operand.ty) else {
            self.sess.emit_ice(
                operand.span,
                "Kern ICE (Codegen): SIMD reduction expected a SIMD operand.",
            );
            return self.get_undef_val(result_llvm_ty);
        };
        let elem_norm = self.type_registry.normalize(elem_ty);

        let mut acc = self
            .builder
            .build_extract_element(
                vector_val.into_vector_value(),
                self.context.i32_type().const_int(0, false),
                "simd_reduce_init",
            )
            .unwrap();

        for lane in 1..lanes {
            let lane_val = self
                .builder
                .build_extract_element(
                    vector_val.into_vector_value(),
                    self.context.i32_type().const_int(lane as u64, false),
                    "simd_reduce_lane",
                )
                .unwrap();

            acc = if self.type_registry.is_float(elem_norm) {
                let acc_f = acc.into_float_value();
                let lane_f = lane_val.into_float_value();
                match kind {
                    SimdReduceKind::Add => self
                        .builder
                        .build_float_add(acc_f, lane_f, "simd_reduce_add")
                        .unwrap()
                        .into(),
                    SimdReduceKind::Mul => self
                        .builder
                        .build_float_mul(acc_f, lane_f, "simd_reduce_mul")
                        .unwrap()
                        .into(),
                    SimdReduceKind::Min => {
                        let cond = self
                            .builder
                            .build_float_compare(
                                FloatPredicate::OLT,
                                acc_f,
                                lane_f,
                                "simd_reduce_min_cmp",
                            )
                            .unwrap();
                        self.builder
                            .build_select(
                                cond.into(),
                                acc_f.into(),
                                lane_f.into(),
                                "simd_reduce_min",
                            )
                            .unwrap()
                    }
                    SimdReduceKind::Max => {
                        let cond = self
                            .builder
                            .build_float_compare(
                                FloatPredicate::OGT,
                                acc_f,
                                lane_f,
                                "simd_reduce_max_cmp",
                            )
                            .unwrap();
                        self.builder
                            .build_select(
                                cond.into(),
                                acc_f.into(),
                                lane_f.into(),
                                "simd_reduce_max",
                            )
                            .unwrap()
                    }
                    SimdReduceKind::And | SimdReduceKind::Or | SimdReduceKind::Xor => {
                        self.sess.emit_ice(
                            operand.span,
                            "Kern ICE (Codegen): invalid floating-point SIMD reduction kind.",
                        );
                        return self.get_undef_val(result_llvm_ty);
                    }
                }
            } else {
                let acc_i = acc.into_int_value();
                let lane_i = lane_val.into_int_value();
                match kind {
                    SimdReduceKind::Add => self
                        .builder
                        .build_int_add(acc_i, lane_i, "simd_reduce_add")
                        .unwrap()
                        .into(),
                    SimdReduceKind::Mul => self
                        .builder
                        .build_int_mul(acc_i, lane_i, "simd_reduce_mul")
                        .unwrap()
                        .into(),
                    SimdReduceKind::And => self
                        .builder
                        .build_and(acc_i, lane_i, "simd_reduce_and")
                        .unwrap()
                        .into(),
                    SimdReduceKind::Or => self
                        .builder
                        .build_or(acc_i, lane_i, "simd_reduce_or")
                        .unwrap()
                        .into(),
                    SimdReduceKind::Xor => self
                        .builder
                        .build_xor(acc_i, lane_i, "simd_reduce_xor")
                        .unwrap()
                        .into(),
                    SimdReduceKind::Min => {
                        let pred = if self.is_signed_int(elem_ty) {
                            IntPredicate::SLT
                        } else {
                            IntPredicate::ULT
                        };
                        let cond = self
                            .builder
                            .build_int_compare(pred, acc_i, lane_i, "simd_reduce_min_cmp")
                            .unwrap();
                        self.builder
                            .build_select(
                                cond.into(),
                                acc_i.into(),
                                lane_i.into(),
                                "simd_reduce_min",
                            )
                            .unwrap()
                    }
                    SimdReduceKind::Max => {
                        let pred = if self.is_signed_int(elem_ty) {
                            IntPredicate::SGT
                        } else {
                            IntPredicate::UGT
                        };
                        let cond = self
                            .builder
                            .build_int_compare(pred, acc_i, lane_i, "simd_reduce_max_cmp")
                            .unwrap();
                        self.builder
                            .build_select(
                                cond.into(),
                                acc_i.into(),
                                lane_i.into(),
                                "simd_reduce_max",
                            )
                            .unwrap()
                    }
                }
            };
        }

        acc
    }

    pub(crate) fn compile_simd_select(
        &mut self,
        mask: &MastExpr,
        on_true: &MastExpr,
        on_false: &MastExpr,
    ) -> BasicValueEnum<'ctx> {
        let result_ty = self.get_llvm_type(on_true.ty);
        let mask_val = self.compile_expr(mask);
        if let Some(fallback) = self.expr_terminated_fallback(result_ty) {
            return fallback;
        }
        let true_val = self.compile_expr(on_true);
        if let Some(fallback) = self.expr_terminated_fallback(result_ty) {
            return fallback;
        }
        let false_val = self.compile_expr(on_false);
        if let Some(fallback) = self.expr_terminated_fallback(result_ty) {
            return fallback;
        }

        self.builder
            .build_select(mask_val, true_val, false_val, "simd_select")
            .unwrap()
    }

    pub(crate) fn compile_simd_shuffle(
        &mut self,
        lhs: &MastExpr,
        rhs: &MastExpr,
        indices: &[u32],
    ) -> BasicValueEnum<'ctx> {
        let lhs_ty = self.get_llvm_type(lhs.ty);
        let rhs_ty = self.get_llvm_type(rhs.ty);
        let lhs_val = self.compile_expr(lhs);
        if let Some(fallback) = self.expr_terminated_fallback(lhs_ty) {
            return fallback;
        }
        let rhs_val = self.compile_expr(rhs);
        if let Some(fallback) = self.expr_terminated_fallback(rhs_ty) {
            return fallback;
        }

        let mask_vals = indices
            .iter()
            .map(|&idx| self.context.i32_type().const_int(idx as u64, false).into())
            .collect::<Vec<BasicValueEnum<'ctx>>>();
        let mask = const_vector(&mask_vals);

        self.builder
            .build_shuffle_vector(
                lhs_val.into_vector_value(),
                rhs_val.into_vector_value(),
                mask,
                "simd_shuffle",
            )
            .unwrap()
    }

    pub(crate) fn compile_simd_insert_half(
        &mut self,
        base: &MastExpr,
        half: &MastExpr,
        result_ty: TypeId,
        high_half: bool,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let base_val = self.compile_expr(base);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }
        let half_val = self.compile_expr(half);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }

        let Some((_, full_lanes)) = self.simd_elem_and_lanes(result_ty) else {
            self.sess.emit_ice(
                base.span,
                "Kern ICE (Codegen): SIMD half insertion expected a SIMD result type.",
            );
            return self.get_undef_val(result_llvm_ty);
        };
        let Some((_, half_lanes)) = self.simd_elem_and_lanes(half.ty) else {
            self.sess.emit_ice(
                half.span,
                "Kern ICE (Codegen): SIMD half insertion expected a SIMD half operand.",
            );
            return self.get_undef_val(result_llvm_ty);
        };
        if full_lanes != half_lanes.saturating_mul(2) {
            self.sess.emit_ice(
                half.span,
                "Kern ICE (Codegen): SIMD half insertion reached codegen with invalid lane counts.",
            );
            return self.get_undef_val(result_llvm_ty);
        }

        let mut result = base_val.into_vector_value();
        let half_vec = half_val.into_vector_value();
        let base_lane = if high_half { half_lanes } else { 0 };
        for lane in 0..half_lanes {
            let src_idx = self.context.i32_type().const_int(lane as u64, false);
            let dst_idx = self
                .context
                .i32_type()
                .const_int((base_lane + lane) as u64, false);
            let lane_val = self
                .builder
                .build_extract_element(half_vec, src_idx, "simd_insert_half_lane")
                .unwrap();
            result = self
                .builder
                .build_insert_element(result, lane_val, dst_idx, "simd_insert_half")
                .unwrap()
                .into_vector_value();
        }

        result.into()
    }

    pub(crate) fn compile_simd_load(
        &mut self,
        ptr: &MastExpr,
        result_ty: TypeId,
        align: u32,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let ptr_val = self.compile_expr(ptr);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }

        let loaded = self
            .builder
            .build_load(result_llvm_ty, ptr_val.into_pointer_value(), "simd_load")
            .unwrap();
        if let Some(inst) = loaded.as_instruction_value() {
            inst.set_alignment(align);
        }
        loaded
    }

    pub(crate) fn compile_simd_store(
        &mut self,
        ptr: &MastExpr,
        value: &MastExpr,
        align: u32,
    ) -> BasicValueEnum<'ctx> {
        let ptr_ty = self.get_llvm_type(ptr.ty);
        let value_llvm_ty = self.get_llvm_type(value.ty);
        let ptr_val = self.compile_expr(ptr);
        if let Some(fallback) = self.expr_terminated_fallback(ptr_ty) {
            return fallback;
        }
        let value_val = self.compile_expr(value);
        if let Some(fallback) = self.expr_terminated_fallback(value_llvm_ty) {
            return fallback;
        }

        let store = self
            .builder
            .build_store(ptr_val.into_pointer_value(), value_val)
            .unwrap();
        store.set_alignment(align);
        self.context.i8_type().const_zero().into()
    }

    pub(crate) fn compile_simd_masked_load(
        &mut self,
        ptr: &MastExpr,
        mask: &MastExpr,
        or_else: &MastExpr,
        result_ty: TypeId,
        _align: u32,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let ptr_val = self.compile_expr(ptr);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }
        let mask_val = self.compile_expr(mask);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }
        let fallback_val = self.compile_expr(or_else);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }

        let Some((elem_ty, lanes)) = self.simd_elem_and_lanes(result_ty) else {
            self.sess.emit_ice(
                ptr.span,
                "Kern ICE (Codegen): SIMD masked load expected a SIMD result type.",
            );
            return self.get_undef_val(result_llvm_ty);
        };
        let Some(func) = self.current_function_for_simd_memory("SIMD masked load") else {
            return self.get_undef_val(result_llvm_ty);
        };

        let result_ptr = self.create_entry_block_alloca(result_llvm_ty, "simd_masked_load_tmp");
        self.builder.build_store(result_ptr, fallback_val).unwrap();

        let base_ptr = ptr_val.into_pointer_value();
        let mask_vec = mask_val.into_vector_value();
        let elem_llvm_ty = self.get_llvm_type(elem_ty);
        for lane in 0..lanes {
            let lane_idx = self.context.i32_type().const_int(lane as u64, false);
            let lane_mask = self
                .builder
                .build_extract_element(mask_vec, lane_idx, "simd_masked_load_mask")
                .unwrap()
                .into_int_value();
            let then_bb = self
                .context
                .append_basic_block(func, "simd_masked_load.then");
            let cont_bb = self
                .context
                .append_basic_block(func, "simd_masked_load.cont");
            self.builder
                .build_conditional_branch(lane_mask, then_bb, cont_bb)
                .unwrap();

            self.builder.position_at_end(then_bb);
            let lane_offset = self.context.i64_type().const_int(lane as u64, false);
            let lane_ptr = unsafe {
                self.builder
                    .build_gep(
                        elem_llvm_ty,
                        base_ptr,
                        &[lane_offset],
                        "simd_masked_load_ptr",
                    )
                    .unwrap()
            };
            let lane_val = self
                .builder
                .build_load(elem_llvm_ty, lane_ptr, "simd_masked_load_lane")
                .unwrap();
            let current_vector = self
                .builder
                .build_load(result_llvm_ty, result_ptr, "simd_masked_load_cur")
                .unwrap();
            let updated_vector = self
                .builder
                .build_insert_element(
                    current_vector.into_vector_value(),
                    lane_val,
                    lane_idx,
                    "simd_masked_load_insert",
                )
                .unwrap();
            self.builder
                .build_store(result_ptr, updated_vector)
                .unwrap();
            self.builder.build_unconditional_branch(cont_bb).unwrap();
            self.builder.position_at_end(cont_bb);
        }

        self.builder
            .build_load(result_llvm_ty, result_ptr, "simd_masked_load_result")
            .unwrap()
    }

    pub(crate) fn compile_simd_masked_store(
        &mut self,
        ptr: &MastExpr,
        mask: &MastExpr,
        value: &MastExpr,
        _align: u32,
    ) -> BasicValueEnum<'ctx> {
        let ptr_ty = self.get_llvm_type(ptr.ty);
        let mask_ty = self.get_llvm_type(mask.ty);
        let value_ty = self.get_llvm_type(value.ty);
        let ptr_val = self.compile_expr(ptr);
        if let Some(fallback) = self.expr_terminated_fallback(ptr_ty) {
            return fallback;
        }
        let mask_val = self.compile_expr(mask);
        if let Some(fallback) = self.expr_terminated_fallback(mask_ty) {
            return fallback;
        }
        let vector_val = self.compile_expr(value);
        if let Some(fallback) = self.expr_terminated_fallback(value_ty) {
            return fallback;
        }

        let Some((elem_ty, lanes)) = self.simd_elem_and_lanes(value.ty) else {
            self.sess.emit_ice(
                value.span,
                "Kern ICE (Codegen): SIMD masked store expected a SIMD value operand.",
            );
            return self.context.i8_type().const_zero().into();
        };
        let Some(func) = self.current_function_for_simd_memory("SIMD masked store") else {
            return self.context.i8_type().const_zero().into();
        };

        let base_ptr = ptr_val.into_pointer_value();
        let mask_vec = mask_val.into_vector_value();
        let value_vec = vector_val.into_vector_value();
        let elem_llvm_ty = self.get_llvm_type(elem_ty);
        for lane in 0..lanes {
            let lane_idx = self.context.i32_type().const_int(lane as u64, false);
            let lane_mask = self
                .builder
                .build_extract_element(mask_vec, lane_idx, "simd_masked_store_mask")
                .unwrap()
                .into_int_value();
            let then_bb = self
                .context
                .append_basic_block(func, "simd_masked_store.then");
            let cont_bb = self
                .context
                .append_basic_block(func, "simd_masked_store.cont");
            self.builder
                .build_conditional_branch(lane_mask, then_bb, cont_bb)
                .unwrap();

            self.builder.position_at_end(then_bb);
            let lane_offset = self.context.i64_type().const_int(lane as u64, false);
            let lane_ptr = unsafe {
                self.builder
                    .build_gep(
                        elem_llvm_ty,
                        base_ptr,
                        &[lane_offset],
                        "simd_masked_store_ptr",
                    )
                    .unwrap()
            };
            let lane_val = self
                .builder
                .build_extract_element(value_vec, lane_idx, "simd_masked_store_lane")
                .unwrap();
            self.builder.build_store(lane_ptr, lane_val).unwrap();
            self.builder.build_unconditional_branch(cont_bb).unwrap();
            self.builder.position_at_end(cont_bb);
        }

        self.context.i8_type().const_zero().into()
    }

    pub(crate) fn compile_simd_gather(
        &mut self,
        ptr: &MastExpr,
        indices: &MastExpr,
        result_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let base_ptr = self.compile_expr(ptr);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }
        let indices_ptr = self.compile_expr(indices);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }

        let Some((elem_ty, lanes)) = self.simd_elem_and_lanes(result_ty) else {
            self.sess.emit_ice(
                ptr.span,
                "Kern ICE (Codegen): SIMD gather expected a SIMD result type.",
            );
            return self.get_undef_val(result_llvm_ty);
        };

        let mut result = match result_llvm_ty {
            BasicTypeEnum::VectorType(vector_ty) => vector_ty.const_zero().into_vector_value(),
            other => {
                self.sess.emit_ice(
                    ptr.span,
                    format!(
                        "Kern ICE (Codegen): SIMD gather expected a vector LLVM type, found `{:?}`.",
                        other
                    ),
                );
                return self.get_undef_val(result_llvm_ty);
            }
        };

        let elem_llvm_ty = self.get_llvm_type(elem_ty);
        let usize_llvm_ty = self.get_llvm_type(TypeId::USIZE);
        let indices_ptr = indices_ptr.into_pointer_value();
        let base_ptr = base_ptr.into_pointer_value();

        for lane in 0..lanes {
            let lane_offset = self.context.i64_type().const_int(lane as u64, false);
            let lane_index_ptr = unsafe {
                self.builder
                    .build_gep(
                        usize_llvm_ty,
                        indices_ptr,
                        &[lane_offset],
                        "simd_gather_idx_ptr",
                    )
                    .unwrap()
            };
            let gathered_index = self
                .builder
                .build_load(usize_llvm_ty, lane_index_ptr, "simd_gather_idx")
                .unwrap()
                .into_int_value();
            let lane_ptr = unsafe {
                self.builder
                    .build_gep(
                        elem_llvm_ty,
                        base_ptr,
                        &[gathered_index],
                        "simd_gather_lane_ptr",
                    )
                    .unwrap()
            };
            let lane_val = self
                .builder
                .build_load(elem_llvm_ty, lane_ptr, "simd_gather_lane")
                .unwrap();
            let lane_index = self.context.i32_type().const_int(lane as u64, false);
            result = self
                .builder
                .build_insert_element(result, lane_val, lane_index, "simd_gather_insert")
                .unwrap()
                .into_vector_value();
        }

        result.into()
    }

    pub(crate) fn compile_simd_scatter(
        &mut self,
        ptr: &MastExpr,
        indices: &MastExpr,
        value: &MastExpr,
    ) -> BasicValueEnum<'ctx> {
        let ptr_llvm_ty = self.get_llvm_type(ptr.ty);
        let indices_llvm_ty = self.get_llvm_type(indices.ty);
        let value_llvm_ty = self.get_llvm_type(value.ty);
        let base_ptr = self.compile_expr(ptr);
        if let Some(fallback) = self.expr_terminated_fallback(ptr_llvm_ty) {
            return fallback;
        }
        let indices_ptr = self.compile_expr(indices);
        if let Some(fallback) = self.expr_terminated_fallback(indices_llvm_ty) {
            return fallback;
        }
        let vector_val = self.compile_expr(value);
        if let Some(fallback) = self.expr_terminated_fallback(value_llvm_ty) {
            return fallback;
        }

        let Some((elem_ty, lanes)) = self.simd_elem_and_lanes(value.ty) else {
            self.sess.emit_ice(
                value.span,
                "Kern ICE (Codegen): SIMD scatter expected a SIMD value operand.",
            );
            return self.context.i8_type().const_zero().into();
        };

        let elem_llvm_ty = self.get_llvm_type(elem_ty);
        let usize_llvm_ty = self.get_llvm_type(TypeId::USIZE);
        let indices_ptr = indices_ptr.into_pointer_value();
        let base_ptr = base_ptr.into_pointer_value();
        let vector_val = vector_val.into_vector_value();

        for lane in 0..lanes {
            let lane_offset = self.context.i64_type().const_int(lane as u64, false);
            let lane_index_ptr = unsafe {
                self.builder
                    .build_gep(
                        usize_llvm_ty,
                        indices_ptr,
                        &[lane_offset],
                        "simd_scatter_idx_ptr",
                    )
                    .unwrap()
            };
            let scattered_index = self
                .builder
                .build_load(usize_llvm_ty, lane_index_ptr, "simd_scatter_idx")
                .unwrap()
                .into_int_value();
            let lane_ptr = unsafe {
                self.builder
                    .build_gep(
                        elem_llvm_ty,
                        base_ptr,
                        &[scattered_index],
                        "simd_scatter_lane_ptr",
                    )
                    .unwrap()
            };
            let lane_index = self.context.i32_type().const_int(lane as u64, false);
            let lane_val = self
                .builder
                .build_extract_element(vector_val, lane_index, "simd_scatter_lane")
                .unwrap();
            self.builder.build_store(lane_ptr, lane_val).unwrap();
        }

        self.context.i8_type().const_zero().into()
    }

    pub(crate) fn compile_simd_masked_gather(
        &mut self,
        ptr: &MastExpr,
        indices: &MastExpr,
        mask: &MastExpr,
        or_else: &MastExpr,
        result_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let base_ptr = self.compile_expr(ptr);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }
        let indices_ptr = self.compile_expr(indices);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }
        let mask_val = self.compile_expr(mask);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }
        let fallback_val = self.compile_expr(or_else);
        if let Some(fallback) = self.expr_terminated_fallback(result_llvm_ty) {
            return fallback;
        }

        let Some((elem_ty, lanes)) = self.simd_elem_and_lanes(result_ty) else {
            self.sess.emit_ice(
                ptr.span,
                "Kern ICE (Codegen): SIMD masked gather expected a SIMD result type.",
            );
            return self.get_undef_val(result_llvm_ty);
        };
        let Some(func) = self.current_function_for_simd_memory("SIMD masked gather") else {
            return self.get_undef_val(result_llvm_ty);
        };

        let result_ptr = self.create_entry_block_alloca(result_llvm_ty, "simd_masked_gather_tmp");
        self.builder.build_store(result_ptr, fallback_val).unwrap();

        let elem_llvm_ty = self.get_llvm_type(elem_ty);
        let usize_llvm_ty = self.get_llvm_type(TypeId::USIZE);
        let base_ptr = base_ptr.into_pointer_value();
        let indices_ptr = indices_ptr.into_pointer_value();
        let mask_vec = mask_val.into_vector_value();
        for lane in 0..lanes {
            let lane_idx = self.context.i32_type().const_int(lane as u64, false);
            let lane_mask = self
                .builder
                .build_extract_element(mask_vec, lane_idx, "simd_masked_gather_mask")
                .unwrap()
                .into_int_value();
            let then_bb = self
                .context
                .append_basic_block(func, "simd_masked_gather.then");
            let cont_bb = self
                .context
                .append_basic_block(func, "simd_masked_gather.cont");
            self.builder
                .build_conditional_branch(lane_mask, then_bb, cont_bb)
                .unwrap();

            self.builder.position_at_end(then_bb);
            let lane_offset = self.context.i64_type().const_int(lane as u64, false);
            let lane_index_ptr = unsafe {
                self.builder
                    .build_gep(
                        usize_llvm_ty,
                        indices_ptr,
                        &[lane_offset],
                        "simd_masked_gather_idx_ptr",
                    )
                    .unwrap()
            };
            let gathered_index = self
                .builder
                .build_load(usize_llvm_ty, lane_index_ptr, "simd_masked_gather_idx")
                .unwrap()
                .into_int_value();
            let lane_ptr = unsafe {
                self.builder
                    .build_gep(
                        elem_llvm_ty,
                        base_ptr,
                        &[gathered_index],
                        "simd_masked_gather_lane_ptr",
                    )
                    .unwrap()
            };
            let lane_val = self
                .builder
                .build_load(elem_llvm_ty, lane_ptr, "simd_masked_gather_lane")
                .unwrap();
            let current_vector = self
                .builder
                .build_load(result_llvm_ty, result_ptr, "simd_masked_gather_cur")
                .unwrap();
            let updated_vector = self
                .builder
                .build_insert_element(
                    current_vector.into_vector_value(),
                    lane_val,
                    lane_idx,
                    "simd_masked_gather_insert",
                )
                .unwrap();
            self.builder
                .build_store(result_ptr, updated_vector)
                .unwrap();
            self.builder.build_unconditional_branch(cont_bb).unwrap();
            self.builder.position_at_end(cont_bb);
        }

        self.builder
            .build_load(result_llvm_ty, result_ptr, "simd_masked_gather_result")
            .unwrap()
    }

    pub(crate) fn compile_simd_masked_scatter(
        &mut self,
        ptr: &MastExpr,
        indices: &MastExpr,
        mask: &MastExpr,
        value: &MastExpr,
    ) -> BasicValueEnum<'ctx> {
        let ptr_ty = self.get_llvm_type(ptr.ty);
        let indices_ty = self.get_llvm_type(indices.ty);
        let mask_ty = self.get_llvm_type(mask.ty);
        let value_ty = self.get_llvm_type(value.ty);
        let base_ptr = self.compile_expr(ptr);
        if let Some(fallback) = self.expr_terminated_fallback(ptr_ty) {
            return fallback;
        }
        let indices_ptr = self.compile_expr(indices);
        if let Some(fallback) = self.expr_terminated_fallback(indices_ty) {
            return fallback;
        }
        let mask_val = self.compile_expr(mask);
        if let Some(fallback) = self.expr_terminated_fallback(mask_ty) {
            return fallback;
        }
        let vector_val = self.compile_expr(value);
        if let Some(fallback) = self.expr_terminated_fallback(value_ty) {
            return fallback;
        }

        let Some((elem_ty, lanes)) = self.simd_elem_and_lanes(value.ty) else {
            self.sess.emit_ice(
                value.span,
                "Kern ICE (Codegen): SIMD masked scatter expected a SIMD value operand.",
            );
            return self.context.i8_type().const_zero().into();
        };
        let Some(func) = self.current_function_for_simd_memory("SIMD masked scatter") else {
            return self.context.i8_type().const_zero().into();
        };

        let elem_llvm_ty = self.get_llvm_type(elem_ty);
        let usize_llvm_ty = self.get_llvm_type(TypeId::USIZE);
        let base_ptr = base_ptr.into_pointer_value();
        let indices_ptr = indices_ptr.into_pointer_value();
        let mask_vec = mask_val.into_vector_value();
        let vector_val = vector_val.into_vector_value();
        for lane in 0..lanes {
            let lane_idx = self.context.i32_type().const_int(lane as u64, false);
            let lane_mask = self
                .builder
                .build_extract_element(mask_vec, lane_idx, "simd_masked_scatter_mask")
                .unwrap()
                .into_int_value();
            let then_bb = self
                .context
                .append_basic_block(func, "simd_masked_scatter.then");
            let cont_bb = self
                .context
                .append_basic_block(func, "simd_masked_scatter.cont");
            self.builder
                .build_conditional_branch(lane_mask, then_bb, cont_bb)
                .unwrap();

            self.builder.position_at_end(then_bb);
            let lane_offset = self.context.i64_type().const_int(lane as u64, false);
            let lane_index_ptr = unsafe {
                self.builder
                    .build_gep(
                        usize_llvm_ty,
                        indices_ptr,
                        &[lane_offset],
                        "simd_masked_scatter_idx_ptr",
                    )
                    .unwrap()
            };
            let scattered_index = self
                .builder
                .build_load(usize_llvm_ty, lane_index_ptr, "simd_masked_scatter_idx")
                .unwrap()
                .into_int_value();
            let lane_ptr = unsafe {
                self.builder
                    .build_gep(
                        elem_llvm_ty,
                        base_ptr,
                        &[scattered_index],
                        "simd_masked_scatter_lane_ptr",
                    )
                    .unwrap()
            };
            let lane_val = self
                .builder
                .build_extract_element(vector_val, lane_idx, "simd_masked_scatter_lane")
                .unwrap();
            self.builder.build_store(lane_ptr, lane_val).unwrap();
            self.builder.build_unconditional_branch(cont_bb).unwrap();
            self.builder.position_at_end(cont_bb);
        }

        self.context.i8_type().const_zero().into()
    }

    fn compile_simd_binary(
        &mut self,
        op: ast::BinaryOperator,
        lhs: &MastExpr,
        rhs: &MastExpr,
    ) -> BasicValueEnum<'ctx> {
        let lhs_ty = self.get_llvm_type(lhs.ty);
        let rhs_ty = self.get_llvm_type(rhs.ty);
        let l_val = self.compile_expr(lhs);
        if let Some(fallback) = self.expr_terminated_fallback(lhs_ty) {
            return fallback;
        }
        let r_val = self.compile_expr(rhs);
        if let Some(fallback) = self.expr_terminated_fallback(rhs_ty) {
            return fallback;
        }

        let Some((elem_ty, _)) = self.simd_elem_and_lanes(lhs.ty) else {
            self.sess.emit_ice(
                lhs.span,
                "Kern ICE (Codegen): SIMD binary path reached with a non-SIMD type.",
            );
            return self.zero_i8_value();
        };

        let elem_norm = self.type_registry.normalize(elem_ty);
        let elem_is_float = self.type_registry.is_float(elem_norm);

        match op {
            BinaryOperator::Add => {
                if elem_is_float {
                    self.builder
                        .build_basic_float_add(l_val, r_val, "simd_fadd")
                        .unwrap()
                } else {
                    self.builder
                        .build_basic_int_add(l_val, r_val, "simd_add")
                        .unwrap()
                }
            }
            BinaryOperator::Subtract => {
                if elem_is_float {
                    self.builder
                        .build_basic_float_sub(l_val, r_val, "simd_fsub")
                        .unwrap()
                } else {
                    self.builder
                        .build_basic_int_sub(l_val, r_val, "simd_sub")
                        .unwrap()
                }
            }
            BinaryOperator::Multiply => {
                if elem_is_float {
                    self.builder
                        .build_basic_float_mul(l_val, r_val, "simd_fmul")
                        .unwrap()
                } else {
                    self.builder
                        .build_basic_int_mul(l_val, r_val, "simd_mul")
                        .unwrap()
                }
            }
            BinaryOperator::Divide => {
                if elem_is_float {
                    self.builder
                        .build_basic_float_div(l_val, r_val, "simd_fdiv")
                        .unwrap()
                } else if self.is_signed_int(elem_ty) {
                    self.builder
                        .build_basic_int_signed_div(l_val, r_val, "simd_sdiv")
                        .unwrap()
                } else {
                    self.builder
                        .build_basic_int_unsigned_div(l_val, r_val, "simd_udiv")
                        .unwrap()
                }
            }
            BinaryOperator::Modulo => {
                if elem_is_float {
                    self.builder
                        .build_basic_float_rem(l_val, r_val, "simd_frem")
                        .unwrap()
                } else if self.is_signed_int(elem_ty) {
                    self.builder
                        .build_basic_int_signed_rem(l_val, r_val, "simd_srem")
                        .unwrap()
                } else {
                    self.builder
                        .build_basic_int_unsigned_rem(l_val, r_val, "simd_urem")
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
                        .build_basic_float_compare(pred, l_val, r_val, "simd_fcmp")
                        .unwrap()
                } else {
                    let pred = Self::simd_int_pred(op, self.is_signed_int(elem_ty)).unwrap();
                    self.builder
                        .build_basic_int_compare(pred, l_val, r_val, "simd_icmp")
                        .unwrap()
                }
            }
            BinaryOperator::BitwiseAnd => self
                .builder
                .build_basic_and(l_val, r_val, "simd_and")
                .unwrap(),
            BinaryOperator::BitwiseOr => self
                .builder
                .build_basic_or(l_val, r_val, "simd_or")
                .unwrap(),
            BinaryOperator::BitwiseXor => self
                .builder
                .build_basic_xor(l_val, r_val, "simd_xor")
                .unwrap(),
            BinaryOperator::ShiftLeft => self
                .builder
                .build_basic_shl(l_val, r_val, "simd_shl")
                .unwrap(),
            BinaryOperator::ShiftRight => {
                if self.is_signed_int(elem_ty) {
                    self.builder
                        .build_basic_ashr(l_val, r_val, "simd_ashr")
                        .unwrap()
                } else {
                    self.builder
                        .build_basic_lshr(l_val, r_val, "simd_lshr")
                        .unwrap()
                }
            }
            BinaryOperator::LogicalAnd | BinaryOperator::LogicalOr => {
                self.sess.emit_ice(
                    lhs.span,
                    "Kern ICE (Codegen): logical short-circuit operators are not valid on SIMD values.",
                );
                self.zero_i8_value()
            }
        }
    }

    fn zero_i8_value(&self) -> BasicValueEnum<'ctx> {
        self.context.i8_type().const_zero().into()
    }

    fn ptr_elem_llvm_type(
        &mut self,
        ptr_ty: TypeId,
        span: Span,
        context: &str,
    ) -> Option<BasicTypeEnum<'ctx>> {
        let Some(elem_sema_ty) = self.type_registry.get_elem_type(ptr_ty) else {
            self.sess.emit_ice(
                span,
                format!(
                    "Kern ICE (Codegen): missing pointee type while compiling {}.",
                    context
                ),
            );
            return None;
        };
        Some(self.get_llvm_type(elem_sema_ty))
    }

    fn pointer_compare_pred(op: BinaryOperator) -> Option<IntPredicate> {
        match op {
            BinaryOperator::Equal => Some(IntPredicate::EQ),
            BinaryOperator::NotEqual => Some(IntPredicate::NE),
            BinaryOperator::LessThan => Some(IntPredicate::ULT),
            BinaryOperator::LessOrEqual => Some(IntPredicate::ULE),
            BinaryOperator::GreaterThan => Some(IntPredicate::UGT),
            BinaryOperator::GreaterOrEqual => Some(IntPredicate::UGE),
            _ => None,
        }
    }

    // Helper for determining whether an integer type is signed.
    pub(crate) fn is_signed_int(&self, ty: TypeId) -> bool {
        let norm = self.type_registry.normalize(ty);
        if let TypeKind::Primitive(p) = self.type_registry.get(norm) {
            matches!(
                p,
                PrimitiveType::I8
                    | PrimitiveType::I16
                    | PrimitiveType::I32
                    | PrimitiveType::I64
                    | PrimitiveType::I128
                    | PrimitiveType::ISize
            )
        } else {
            false
        }
    }

    /// Main dispatch for binary operators.
    pub(crate) fn compile_binary(
        &mut self,
        op: ast::BinaryOperator,
        lhs: &MastExpr,
        rhs: &MastExpr,
    ) -> BasicValueEnum<'ctx> {
        if self.type_registry.is_simd(lhs.ty) {
            return self.compile_simd_binary(op, lhs, rhs);
        }

        let result_ty = self.get_llvm_type(lhs.ty);
        let l_val = self.compile_expr(lhs);
        if let Some(fallback) = self.expr_terminated_fallback(result_ty) {
            return fallback;
        }
        let r_val = self.compile_expr(rhs);
        if let Some(fallback) = self.expr_terminated_fallback(result_ty) {
            return fallback;
        }
        let span = lhs.span;

        if l_val.is_pointer_value() || r_val.is_pointer_value() {
            self.compile_ptr_math(op, l_val, r_val, lhs.ty, rhs.ty, span)
        } else if l_val.is_int_value() && r_val.is_int_value() {
            let is_signed = self.is_signed_int(lhs.ty);
            self.compile_int_math(
                op,
                l_val.into_int_value(),
                r_val.into_int_value(),
                is_signed,
                span,
            )
        } else if l_val.is_float_value() && r_val.is_float_value() {
            self.compile_float_math(op, l_val.into_float_value(), r_val.into_float_value(), span)
        } else {
            self.sess.emit_ice(
                span,
                "Kern ICE (Codegen): Unsupported types for binary operation. Sema missed this type mismatch.",
            );
            self.zero_i8_value()
        }
    }

    fn compile_i128_divrem(
        &mut self,
        op: BinaryOperator,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        is_signed: bool,
    ) -> BasicValueEnum<'ctx> {
        let helper = match (is_signed, op) {
            (false, BinaryOperator::Divide) => self.ensure_i128_unsigned_divrem_helper(false),
            (false, BinaryOperator::Modulo) => self.ensure_i128_unsigned_divrem_helper(true),
            (true, BinaryOperator::Divide) => self.ensure_i128_signed_divrem_helper(false),
            (true, BinaryOperator::Modulo) => self.ensure_i128_signed_divrem_helper(true),
            _ => {
                self.sess.emit_ice(
                    Span::default(),
                    "Kern ICE (Codegen): invalid i128 helper request for a non div/rem operator.",
                );
                return self.zero_i8_value();
            }
        };

        self.builder
            .build_call(helper, &[lhs.into(), rhs.into()], "i128_divrem")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
    }

    fn ensure_i128_unsigned_divrem_helper(
        &mut self,
        return_remainder: bool,
    ) -> crate::values::FunctionValue<'ctx> {
        let name = if return_remainder {
            "__kern_umodti3"
        } else {
            "__kern_udivti3"
        };
        if let Some(func) = self.module.get_function(name) {
            return func;
        }

        let saved_insert_block = self.builder.get_insert_block();
        let i128_ty = self.context.i128_type();
        let fn_ty = i128_ty.fn_type(&[i128_ty.into(), i128_ty.into()], false);
        let func = self
            .module
            .add_function(name, fn_ty, Some(crate::llvm_api::Linkage::Internal));

        let entry_bb = self.context.append_basic_block(func, "entry");
        let divzero_bb = self.context.append_basic_block(func, "divzero");
        let loop_bb = self.context.append_basic_block(func, "loop");
        let ge_bb = self.context.append_basic_block(func, "ge");
        let lt_bb = self.context.append_basic_block(func, "lt");
        let merge_bb = self.context.append_basic_block(func, "merge");
        let exit_bb = self.context.append_basic_block(func, "exit");

        self.builder.position_at_end(entry_bb);
        let dividend = func.get_nth_param(0).unwrap().into_int_value();
        let divisor = func.get_nth_param(1).unwrap().into_int_value();
        let zero = i128_ty.const_zero();
        let one = i128_ty.const_int(1, false);
        let high_bit = i128_ty.const_int(127, false);
        let divisor_is_zero = self
            .builder
            .build_int_compare(IntPredicate::EQ, divisor, zero, "divisor_is_zero")
            .unwrap();
        self.builder
            .build_conditional_branch(divisor_is_zero, divzero_bb, loop_bb)
            .unwrap();

        self.builder.position_at_end(divzero_bb);
        self.builder.build_unreachable().unwrap();

        self.builder.position_at_end(loop_bb);
        let quotient_phi = self.builder.build_phi(i128_ty, "quotient").unwrap();
        let remainder_phi = self.builder.build_phi(i128_ty, "remainder").unwrap();
        let shift_phi = self.builder.build_phi(i128_ty, "shift").unwrap();
        quotient_phi.add_incoming(&[(&zero, entry_bb)]);
        remainder_phi.add_incoming(&[(&zero, entry_bb)]);
        shift_phi.add_incoming(&[(&high_bit, entry_bb)]);

        let quotient = quotient_phi.as_basic_value().into_int_value();
        let remainder = remainder_phi.as_basic_value().into_int_value();
        let shift = shift_phi.as_basic_value().into_int_value();
        let shifted = self
            .builder
            .build_right_shift(dividend, shift, false, "shifted")
            .unwrap();
        let bit = self.builder.build_and(shifted, one, "bit").unwrap();
        let remainder_shifted = self
            .builder
            .build_left_shift(remainder, one, "remainder_shifted")
            .unwrap();
        let candidate_remainder = self
            .builder
            .build_or(remainder_shifted, bit, "candidate_remainder")
            .unwrap();
        let can_subtract = self
            .builder
            .build_int_compare(
                IntPredicate::UGE,
                candidate_remainder,
                divisor,
                "can_subtract",
            )
            .unwrap();
        self.builder
            .build_conditional_branch(can_subtract, ge_bb, lt_bb)
            .unwrap();

        self.builder.position_at_end(ge_bb);
        let subtracted_remainder = self
            .builder
            .build_int_sub(candidate_remainder, divisor, "subtracted_remainder")
            .unwrap();
        let quotient_bit = self
            .builder
            .build_left_shift(one, shift, "quotient_bit")
            .unwrap();
        let updated_quotient = self
            .builder
            .build_or(quotient, quotient_bit, "updated_quotient")
            .unwrap();
        self.builder.build_unconditional_branch(merge_bb).unwrap();

        self.builder.position_at_end(lt_bb);
        self.builder.build_unconditional_branch(merge_bb).unwrap();

        self.builder.position_at_end(merge_bb);
        let next_quotient_phi = self.builder.build_phi(i128_ty, "next_quotient").unwrap();
        next_quotient_phi.add_incoming(&[(&updated_quotient, ge_bb), (&quotient, lt_bb)]);
        let next_remainder_phi = self.builder.build_phi(i128_ty, "next_remainder").unwrap();
        next_remainder_phi.add_incoming(&[
            (&subtracted_remainder, ge_bb),
            (&candidate_remainder, lt_bb),
        ]);

        let next_quotient = next_quotient_phi.as_basic_value().into_int_value();
        let next_remainder = next_remainder_phi.as_basic_value().into_int_value();
        let is_last_bit = self
            .builder
            .build_int_compare(IntPredicate::EQ, shift, zero, "is_last_bit")
            .unwrap();
        let next_shift = self
            .builder
            .build_int_sub(shift, one, "next_shift")
            .unwrap();
        quotient_phi.add_incoming(&[(&next_quotient, merge_bb)]);
        remainder_phi.add_incoming(&[(&next_remainder, merge_bb)]);
        shift_phi.add_incoming(&[(&next_shift, merge_bb)]);
        self.builder
            .build_conditional_branch(is_last_bit, exit_bb, loop_bb)
            .unwrap();

        self.builder.position_at_end(exit_bb);
        let result = if return_remainder {
            next_remainder
        } else {
            next_quotient
        };
        self.builder.build_return(Some(&result)).unwrap();

        if let Some(block) = saved_insert_block {
            self.builder.position_at_end(block);
        }

        func
    }

    fn ensure_i128_signed_divrem_helper(
        &mut self,
        return_remainder: bool,
    ) -> crate::values::FunctionValue<'ctx> {
        let name = if return_remainder {
            "__kern_modti3"
        } else {
            "__kern_divti3"
        };
        if let Some(func) = self.module.get_function(name) {
            return func;
        }

        let unsigned_helper = self.ensure_i128_unsigned_divrem_helper(return_remainder);
        let saved_insert_block = self.builder.get_insert_block();
        let i128_ty = self.context.i128_type();
        let fn_ty = i128_ty.fn_type(&[i128_ty.into(), i128_ty.into()], false);
        let func = self
            .module
            .add_function(name, fn_ty, Some(crate::llvm_api::Linkage::Internal));

        let entry_bb = self.context.append_basic_block(func, "entry");
        self.builder.position_at_end(entry_bb);

        let lhs = func.get_nth_param(0).unwrap().into_int_value();
        let rhs = func.get_nth_param(1).unwrap().into_int_value();
        let zero = i128_ty.const_zero();
        let sign_shift = i128_ty.const_int(127, false);

        let lhs_mask = self
            .builder
            .build_right_shift(lhs, sign_shift, true, "lhs_mask")
            .unwrap();
        let rhs_mask = self
            .builder
            .build_right_shift(rhs, sign_shift, true, "rhs_mask")
            .unwrap();

        let lhs_xor = self.builder.build_xor(lhs, lhs_mask, "lhs_xor").unwrap();
        let lhs_abs = self
            .builder
            .build_int_sub(lhs_xor, lhs_mask, "lhs_abs")
            .unwrap();
        let rhs_xor = self.builder.build_xor(rhs, rhs_mask, "rhs_xor").unwrap();
        let rhs_abs = self
            .builder
            .build_int_sub(rhs_xor, rhs_mask, "rhs_abs")
            .unwrap();

        let unsigned_result = self
            .builder
            .build_call(
                unsigned_helper,
                &[lhs_abs.into(), rhs_abs.into()],
                "unsigned_i128_divrem",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
            .into_int_value();

        let result_mask = if return_remainder {
            lhs_mask
        } else {
            self.builder
                .build_xor(lhs_mask, rhs_mask, "result_mask")
                .unwrap()
        };
        let signed_xor = self
            .builder
            .build_xor(unsigned_result, result_mask, "signed_xor")
            .unwrap();
        let signed_result = self
            .builder
            .build_int_sub(signed_xor, result_mask, "signed_result")
            .unwrap();

        self.builder.build_return(Some(&signed_result)).unwrap();

        if let Some(block) = saved_insert_block {
            self.builder.position_at_end(block);
        }

        let _ = zero;
        func
    }

    /// Helper: lower pointer arithmetic and pointer comparisons.
    fn compile_ptr_math(
        &mut self,
        op: ast::BinaryOperator,
        l_val: BasicValueEnum<'ctx>,
        r_val: BasicValueEnum<'ctx>,
        lhs_ty: TypeId,
        rhs_ty: TypeId,
        span: Span,
    ) -> BasicValueEnum<'ctx> {
        use BinaryOperator::*;
        match op {
            Add => {
                let (ptr_val, int_val) = if l_val.is_pointer_value() {
                    if !r_val.is_int_value() {
                        self.sess.emit_ice(
                            span,
                            "Kern ICE (Codegen): expected integer for RHS of pointer addition.",
                        );
                        return self.zero_i8_value();
                    }
                    (l_val.into_pointer_value(), r_val.into_int_value())
                } else {
                    if !l_val.is_int_value() {
                        self.sess.emit_ice(
                            span,
                            "Kern ICE (Codegen): expected integer for LHS of pointer addition.",
                        );
                        return self.zero_i8_value();
                    }
                    (r_val.into_pointer_value(), l_val.into_int_value())
                };

                let ptr_ty = if l_val.is_pointer_value() {
                    lhs_ty
                } else {
                    rhs_ty
                };
                let Some(elem_llvm_ty) = self.ptr_elem_llvm_type(ptr_ty, span, "pointer addition")
                else {
                    return self.zero_i8_value();
                };

                unsafe {
                    self.builder
                        .build_gep(elem_llvm_ty, ptr_val, &[int_val], "ptr_add")
                        .unwrap()
                        .into()
                }
            }
            Subtract => {
                if l_val.is_pointer_value() && r_val.is_pointer_value() {
                    let l_ptr = l_val.into_pointer_value();
                    let r_ptr = r_val.into_pointer_value();
                    let Some(elem_sema_ty) = self.type_registry.get_elem_type(lhs_ty) else {
                        self.sess.emit_ice(
                            span,
                            "Kern ICE (Codegen): pointer subtraction missing pointee type.",
                        );
                        return self.zero_i8_value();
                    };

                    // *void - *void === 0
                    if self.is_void_type(elem_sema_ty) {
                        return self.context.i64_type().const_zero().into();
                    }

                    let elem_llvm_ty = self.get_llvm_type(elem_sema_ty);

                    self.builder
                        .build_ptr_diff(elem_llvm_ty, l_ptr, r_ptr, "ptr_diff")
                        .unwrap()
                        .into()
                } else {
                    let ptr_val = l_val.into_pointer_value();
                    let int_val = r_val.into_int_value();
                    let neg_int = self.builder.build_int_neg(int_val, "neg_offset").unwrap();
                    let Some(elem_llvm_ty) =
                        self.ptr_elem_llvm_type(lhs_ty, span, "pointer subtraction")
                    else {
                        return self.zero_i8_value();
                    };

                    unsafe {
                        self.builder
                            .build_gep(elem_llvm_ty, ptr_val, &[neg_int], "ptr_sub")
                            .unwrap()
                            .into()
                    }
                }
            }

            // Handle cases such as `ptr == 0` or `ptr1 > ptr2`.
            Equal | NotEqual | LessThan | LessOrEqual | GreaterThan | GreaterOrEqual => {
                // Compare memory addresses numerically by converting both operands to `usize`.
                let l_int = if l_val.is_pointer_value() {
                    self.builder
                        .build_ptr_to_int(
                            l_val.into_pointer_value(),
                            self.context.i64_type(),
                            "p2i_l",
                        )
                        .unwrap()
                } else {
                    l_val.into_int_value()
                };

                let r_int = if r_val.is_pointer_value() {
                    self.builder
                        .build_ptr_to_int(
                            r_val.into_pointer_value(),
                            self.context.i64_type(),
                            "p2i_r",
                        )
                        .unwrap()
                } else {
                    r_val.into_int_value()
                };

                // Pointer comparisons are always unsigned.
                let Some(pred) = Self::pointer_compare_pred(op) else {
                    self.sess.emit_ice(
                        span,
                        format!(
                            "Kern ICE (Codegen): invalid pointer comparison operator `{:?}`.",
                            op
                        ),
                    );
                    return self.zero_i8_value();
                };

                self.builder
                    .build_int_compare(pred, l_int, r_int, "ptr_cmp")
                    .unwrap()
                    .into()
            }

            _ => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): invalid pointer arithmetic operation `{:?}`.",
                        op
                    ),
                );
                self.zero_i8_value()
            }
        }
    }

    /// Helper: lower integer arithmetic and comparisons.
    fn compile_int_math(
        &mut self,
        op: ast::BinaryOperator,
        l_int: IntValue<'ctx>,
        r_int: IntValue<'ctx>,
        is_signed: bool,
        span: Span,
    ) -> BasicValueEnum<'ctx> {
        use BinaryOperator::*;
        if l_int.get_type() == self.context.i128_type() && matches!(op, Divide | Modulo) {
            return self.compile_i128_divrem(op, l_int, r_int, is_signed);
        }
        match op {
            Add => self
                .builder
                .build_int_add(l_int, r_int, "add")
                .unwrap()
                .into(),
            Subtract => self
                .builder
                .build_int_sub(l_int, r_int, "sub")
                .unwrap()
                .into(),
            Multiply => self
                .builder
                .build_int_mul(l_int, r_int, "mul")
                .unwrap()
                .into(),
            Divide => {
                if is_signed {
                    self.builder
                        .build_int_signed_div(l_int, r_int, "sdiv")
                        .unwrap()
                        .into()
                } else {
                    self.builder
                        .build_int_unsigned_div(l_int, r_int, "udiv")
                        .unwrap()
                        .into()
                }
            }
            Modulo => {
                if is_signed {
                    self.builder
                        .build_int_signed_rem(l_int, r_int, "srem")
                        .unwrap()
                        .into()
                } else {
                    self.builder
                        .build_int_unsigned_rem(l_int, r_int, "urem")
                        .unwrap()
                        .into()
                }
            }
            BitwiseAnd => self.builder.build_and(l_int, r_int, "and").unwrap().into(),
            BitwiseOr => self.builder.build_or(l_int, r_int, "or").unwrap().into(),
            BitwiseXor => self.builder.build_xor(l_int, r_int, "xor").unwrap().into(),
            ShiftLeft => self
                .builder
                .build_left_shift(l_int, r_int, "shl")
                .unwrap()
                .into(),
            ShiftRight => self
                .builder
                .build_right_shift(l_int, r_int, is_signed, "shr")
                .unwrap()
                .into(),
            Equal => self
                .builder
                .build_int_compare(IntPredicate::EQ, l_int, r_int, "eq")
                .unwrap()
                .into(),
            NotEqual => self
                .builder
                .build_int_compare(IntPredicate::NE, l_int, r_int, "ne")
                .unwrap()
                .into(),
            LessThan => {
                let pred = if is_signed {
                    IntPredicate::SLT
                } else {
                    IntPredicate::ULT
                };
                self.builder
                    .build_int_compare(pred, l_int, r_int, "lt")
                    .unwrap()
                    .into()
            }
            LessOrEqual => {
                let pred = if is_signed {
                    IntPredicate::SLE
                } else {
                    IntPredicate::ULE
                };
                self.builder
                    .build_int_compare(pred, l_int, r_int, "le")
                    .unwrap()
                    .into()
            }
            GreaterThan => {
                let pred = if is_signed {
                    IntPredicate::SGT
                } else {
                    IntPredicate::UGT
                };
                self.builder
                    .build_int_compare(pred, l_int, r_int, "gt")
                    .unwrap()
                    .into()
            }
            GreaterOrEqual => {
                let pred = if is_signed {
                    IntPredicate::SGE
                } else {
                    IntPredicate::UGE
                };
                self.builder
                    .build_int_compare(pred, l_int, r_int, "ge")
                    .unwrap()
                    .into()
            }
            _ => {
                self.sess.emit_ice(
                    span,
                    format!("Kern ICE (Codegen): Unhandled integer operator `{:?}`.", op),
                );
                l_int.get_type().const_zero().into()
            }
        }
    }

    /// Helper: lower floating-point arithmetic and comparisons.
    fn compile_float_math(
        &mut self,
        op: ast::BinaryOperator,
        l_float: FloatValue<'ctx>,
        r_float: FloatValue<'ctx>,
        span: Span,
    ) -> BasicValueEnum<'ctx> {
        use ast::BinaryOperator::*;
        match op {
            Add => self
                .builder
                .build_float_add(l_float, r_float, "fadd")
                .unwrap()
                .into(),
            Subtract => self
                .builder
                .build_float_sub(l_float, r_float, "fsub")
                .unwrap()
                .into(),
            Multiply => self
                .builder
                .build_float_mul(l_float, r_float, "fmul")
                .unwrap()
                .into(),
            Divide => self
                .builder
                .build_float_div(l_float, r_float, "fdiv")
                .unwrap()
                .into(),
            Modulo => self
                .builder
                .build_float_rem(l_float, r_float, "frem")
                .unwrap()
                .into(),
            Equal => self
                .builder
                .build_float_compare(FloatPredicate::OEQ, l_float, r_float, "feq")
                .unwrap()
                .into(),
            NotEqual => self
                .builder
                .build_float_compare(FloatPredicate::ONE, l_float, r_float, "fne")
                .unwrap()
                .into(),
            LessThan => self
                .builder
                .build_float_compare(FloatPredicate::OLT, l_float, r_float, "flt")
                .unwrap()
                .into(),
            LessOrEqual => self
                .builder
                .build_float_compare(FloatPredicate::OLE, l_float, r_float, "fle")
                .unwrap()
                .into(),
            GreaterThan => self
                .builder
                .build_float_compare(FloatPredicate::OGT, l_float, r_float, "fgt")
                .unwrap()
                .into(),
            GreaterOrEqual => self
                .builder
                .build_float_compare(FloatPredicate::OGE, l_float, r_float, "fge")
                .unwrap()
                .into(),
            _ => {
                self.sess.emit_ice(
                    span,
                    format!("Kern ICE (Codegen): Unhandled float operator `{:?}`.", op),
                );
                l_float.get_type().const_zero().into()
            }
        }
    }

    pub(crate) fn compile_unary(
        &mut self,
        op: ast::UnaryOperator,
        operand: &MastExpr,
    ) -> BasicValueEnum<'ctx> {
        if self.type_registry.is_simd(operand.ty) {
            let result_ty = self.get_llvm_type(operand.ty);
            let op_val = self.compile_expr(operand);
            if let Some(fallback) = self.expr_terminated_fallback(result_ty) {
                return fallback;
            }
            let Some((elem_ty, _)) = self.simd_elem_and_lanes(operand.ty) else {
                self.sess.emit_ice(
                    operand.span,
                    "Kern ICE (Codegen): SIMD unary path reached with a non-SIMD type.",
                );
                return self.zero_i8_value();
            };
            return match op {
                ast::UnaryOperator::Negate => {
                    if self.type_registry.is_float(elem_ty) {
                        self.builder
                            .build_basic_float_neg(op_val, "simd_fneg")
                            .unwrap()
                    } else {
                        self.builder.build_basic_neg(op_val, "simd_neg").unwrap()
                    }
                }
                ast::UnaryOperator::LogicalNot | ast::UnaryOperator::BitwiseNot => {
                    self.builder.build_basic_not(op_val, "simd_not").unwrap()
                }
                _ => {
                    self.sess.emit_ice(
                        operand.span,
                        format!(
                            "Kern ICE (Codegen): unsupported SIMD unary operator `{:?}`.",
                            op
                        ),
                    );
                    self.zero_i8_value()
                }
            };
        }

        let result_ty = self.get_llvm_type(operand.ty);
        let op_val = self.compile_expr(operand);
        if let Some(fallback) = self.expr_terminated_fallback(result_ty) {
            return fallback;
        }
        let span = operand.span; // Preserve source location for diagnostics.

        match op {
            ast::UnaryOperator::Negate => {
                if op_val.is_int_value() {
                    self.builder
                        .build_int_neg(op_val.into_int_value(), "neg")
                        .unwrap()
                        .into()
                } else if op_val.is_float_value() {
                    self.builder
                        .build_float_neg(op_val.into_float_value(), "fneg")
                        .unwrap()
                        .into()
                } else {
                    self.sess.emit_ice(
                        span,
                        "Kern ICE (Codegen): negate operator applied to a non-numeric type.",
                    );
                    self.zero_i8_value()
                }
            }
            ast::UnaryOperator::LogicalNot | ast::UnaryOperator::BitwiseNot => {
                if op_val.is_int_value() {
                    self.builder
                        .build_not(op_val.into_int_value(), "not")
                        .unwrap()
                        .into()
                } else {
                    self.sess.emit_ice(
                        span,
                        format!(
                            "Kern ICE (Codegen): not operator `{:?}` applied to a non-integer/boolean type.",
                            op
                        ),
                    );
                    self.zero_i8_value()
                }
            }
            ast::UnaryOperator::MetaOf => {
                // By this stage MAST guarantees the operand type is already a physical type.
                let norm_ty = self.type_registry.normalize(operand.ty);
                match self.type_registry.get(norm_ty) {
                    TypeKind::Array { len, .. } => {
                        self.context.i64_type().const_int(*len, false).into()
                    }
                    TypeKind::Slice { .. } => self
                        .builder
                        .build_extract_value(op_val.into_struct_value(), 1, "slice_len")
                        .unwrap(),
                    other => {
                        self.sess.emit_ice(
                            span,
                            format!(
                                "Kern ICE (Codegen): `MetaOf` applied to invalid type {:?}.",
                                other
                            ),
                        );
                        self.zero_i8_value()
                    }
                }
            }
            _ => {
                self.sess.emit_ice(
                    span,
                    format!("Kern ICE (Codegen): Unhandled unary operator `{:?}`.", op),
                );
                self.zero_i8_value()
            }
        }
    }

    pub(crate) fn compile_assign(
        &mut self,
        op: ast::AssignmentOperator,
        lhs: &MastExpr,
        rhs: &MastExpr,
    ) -> BasicValueEnum<'ctx> {
        if let kernc_mast::MastExprKind::IndexAccess { lhs: base, index } = &lhs.kind
            && self.type_registry.is_simd(base.ty)
        {
            let ptr = self.compile_lvalue(base);
            if self.current_block_is_terminated() {
                return self.context.struct_type(&[], false).get_undef().into();
            }
            let vector_ty = self.get_llvm_type(base.ty);
            let vector_val = self
                .builder
                .build_load(vector_ty, ptr, "simd_lane_load")
                .unwrap();
            let idx_val = self.compile_expr(index).into_int_value();
            if self.current_block_is_terminated() {
                return self.context.struct_type(&[], false).get_undef().into();
            }
            let old_lane = self
                .builder
                .build_extract_element(vector_val.into_vector_value(), idx_val, "simd_lane_old")
                .unwrap();
            let rhs_val = self.compile_expr(rhs);
            if self.current_block_is_terminated() {
                return self.context.struct_type(&[], false).get_undef().into();
            }

            let new_lane = if op == ast::AssignmentOperator::Assign {
                rhs_val
            } else if old_lane.is_int_value() {
                let l_int = old_lane.into_int_value();
                let r_int = rhs_val.into_int_value();
                let is_signed = self.is_signed_int(lhs.ty);
                use ast::AssignmentOperator::*;
                match op {
                    AddAssign => self
                        .builder
                        .build_int_add(l_int, r_int, "simd_lane_add")
                        .unwrap()
                        .into(),
                    SubtractAssign => self
                        .builder
                        .build_int_sub(l_int, r_int, "simd_lane_sub")
                        .unwrap()
                        .into(),
                    MultiplyAssign => self
                        .builder
                        .build_int_mul(l_int, r_int, "simd_lane_mul")
                        .unwrap()
                        .into(),
                    DivideAssign => {
                        if is_signed {
                            self.builder
                                .build_int_signed_div(l_int, r_int, "simd_lane_sdiv")
                                .unwrap()
                                .into()
                        } else {
                            self.builder
                                .build_int_unsigned_div(l_int, r_int, "simd_lane_udiv")
                                .unwrap()
                                .into()
                        }
                    }
                    ModuloAssign => {
                        if is_signed {
                            self.builder
                                .build_int_signed_rem(l_int, r_int, "simd_lane_srem")
                                .unwrap()
                                .into()
                        } else {
                            self.builder
                                .build_int_unsigned_rem(l_int, r_int, "simd_lane_urem")
                                .unwrap()
                                .into()
                        }
                    }
                    BitwiseAndAssign => self
                        .builder
                        .build_and(l_int, r_int, "simd_lane_and")
                        .unwrap()
                        .into(),
                    BitwiseOrAssign => self
                        .builder
                        .build_or(l_int, r_int, "simd_lane_or")
                        .unwrap()
                        .into(),
                    BitwiseXorAssign => self
                        .builder
                        .build_xor(l_int, r_int, "simd_lane_xor")
                        .unwrap()
                        .into(),
                    ShiftLeftAssign => self
                        .builder
                        .build_left_shift(l_int, r_int, "simd_lane_shl")
                        .unwrap()
                        .into(),
                    ShiftRightAssign => self
                        .builder
                        .build_right_shift(l_int, r_int, is_signed, "simd_lane_shr")
                        .unwrap()
                        .into(),
                    _ => rhs_val,
                }
            } else if old_lane.is_float_value() {
                let l_float = old_lane.into_float_value();
                let r_float = rhs_val.into_float_value();
                use ast::AssignmentOperator::*;
                match op {
                    AddAssign => self
                        .builder
                        .build_float_add(l_float, r_float, "simd_lane_fadd")
                        .unwrap()
                        .into(),
                    SubtractAssign => self
                        .builder
                        .build_float_sub(l_float, r_float, "simd_lane_fsub")
                        .unwrap()
                        .into(),
                    MultiplyAssign => self
                        .builder
                        .build_float_mul(l_float, r_float, "simd_lane_fmul")
                        .unwrap()
                        .into(),
                    DivideAssign => self
                        .builder
                        .build_float_div(l_float, r_float, "simd_lane_fdiv")
                        .unwrap()
                        .into(),
                    ModuloAssign => self
                        .builder
                        .build_float_rem(l_float, r_float, "simd_lane_frem")
                        .unwrap()
                        .into(),
                    _ => rhs_val,
                }
            } else {
                rhs_val
            };

            let updated_vector = self
                .builder
                .build_insert_element(
                    vector_val.into_vector_value(),
                    new_lane,
                    idx_val,
                    "simd_lane_set",
                )
                .unwrap();
            self.builder.build_store(ptr, updated_vector).unwrap();
            return self.context.struct_type(&[], false).get_undef().into();
        }

        let ptr = self.compile_lvalue(lhs);
        if self.current_block_is_terminated() {
            return self.context.struct_type(&[], false).get_undef().into();
        }
        let rhs_val = self.compile_expr(rhs);
        if self.current_block_is_terminated() {
            return self.context.struct_type(&[], false).get_undef().into();
        }
        let span = lhs.span;

        if op == ast::AssignmentOperator::Assign {
            self.builder.build_store(ptr, rhs_val).unwrap();
        } else {
            let expected_lhs_ty = self.get_llvm_type(lhs.ty);
            let lhs_val = self
                .builder
                .build_load(expected_lhs_ty, ptr, "assign_load")
                .unwrap();

            let new_val: BasicValueEnum<'ctx> = if lhs_val.is_int_value() {
                let l_int = lhs_val.into_int_value();
                let r_int = rhs_val.into_int_value();
                let is_signed = self.is_signed_int(lhs.ty);

                use ast::AssignmentOperator::*;
                match op {
                    AddAssign => self
                        .builder
                        .build_int_add(l_int, r_int, "add_a")
                        .unwrap()
                        .into(),
                    SubtractAssign => self
                        .builder
                        .build_int_sub(l_int, r_int, "sub_a")
                        .unwrap()
                        .into(),
                    MultiplyAssign => self
                        .builder
                        .build_int_mul(l_int, r_int, "mul_a")
                        .unwrap()
                        .into(),
                    DivideAssign => {
                        if is_signed {
                            self.builder
                                .build_int_signed_div(l_int, r_int, "sdiv_a")
                                .unwrap()
                                .into()
                        } else {
                            self.builder
                                .build_int_unsigned_div(l_int, r_int, "udiv_a")
                                .unwrap()
                                .into()
                        }
                    }
                    ModuloAssign => {
                        if is_signed {
                            self.builder
                                .build_int_signed_rem(l_int, r_int, "srem_a")
                                .unwrap()
                                .into()
                        } else {
                            self.builder
                                .build_int_unsigned_rem(l_int, r_int, "urem_a")
                                .unwrap()
                                .into()
                        }
                    }
                    BitwiseAndAssign => self
                        .builder
                        .build_and(l_int, r_int, "and_a")
                        .unwrap()
                        .into(),
                    BitwiseOrAssign => self.builder.build_or(l_int, r_int, "or_a").unwrap().into(),
                    BitwiseXorAssign => self
                        .builder
                        .build_xor(l_int, r_int, "xor_a")
                        .unwrap()
                        .into(),
                    ShiftLeftAssign => self
                        .builder
                        .build_left_shift(l_int, r_int, "shl_a")
                        .unwrap()
                        .into(),
                    ShiftRightAssign => self
                        .builder
                        .build_right_shift(l_int, r_int, is_signed, "shr_a")
                        .unwrap()
                        .into(),
                    _ => {
                        self.sess.emit_ice(
                            span,
                            format!(
                                "Kern ICE (Codegen): Unhandled integer assignment operator `{:?}`.",
                                op
                            ),
                        );
                        l_int.get_type().const_zero().into()
                    }
                }
            } else if lhs_val.is_float_value() {
                let l_float = lhs_val.into_float_value();
                let r_float = rhs_val.into_float_value();
                use ast::AssignmentOperator::*;
                match op {
                    AddAssign => self
                        .builder
                        .build_float_add(l_float, r_float, "fadd_a")
                        .unwrap()
                        .into(),
                    SubtractAssign => self
                        .builder
                        .build_float_sub(l_float, r_float, "fsub_a")
                        .unwrap()
                        .into(),
                    MultiplyAssign => self
                        .builder
                        .build_float_mul(l_float, r_float, "fmul_a")
                        .unwrap()
                        .into(),
                    DivideAssign => self
                        .builder
                        .build_float_div(l_float, r_float, "fdiv_a")
                        .unwrap()
                        .into(),
                    ModuloAssign => self
                        .builder
                        .build_float_rem(l_float, r_float, "frem_a")
                        .unwrap()
                        .into(),
                    _ => {
                        self.sess.emit_ice(
                            span,
                            format!(
                                "Kern ICE (Codegen): Unsupported float assignment operator `{:?}`.",
                                op
                            ),
                        );
                        l_float.get_type().const_zero().into()
                    }
                }
            } else {
                self.sess.emit_ice(
                    span,
                    "Kern ICE (Codegen): unsupported type for compound assignment.",
                );
                self.zero_i8_value()
            };
            self.builder.build_store(ptr, new_val).unwrap();
        }
        self.context.struct_type(&[], false).get_undef().into()
    }
}
