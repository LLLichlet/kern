use super::CodeGenerator;
use crate::llvm_api::const_vector;
use crate::types::BasicTypeEnum;
use crate::values::BasicValueEnum;
use crate::{FloatPredicate, IntPredicate};
use kernc_ast::BinaryOperator;
use kernc_sema::ty::TypeId;
use kernc_utils::Span;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub(crate) fn simd_elem_and_lanes(&self, ty: TypeId) -> Option<(TypeId, u16)> {
        self.type_registry.simd_info(ty)
    }

    pub(crate) fn simd_int_pred(op: BinaryOperator, is_signed: bool) -> Option<IntPredicate> {
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

    pub(crate) fn simd_float_pred(op: BinaryOperator) -> Option<FloatPredicate> {
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

    pub(crate) fn simd_zero_vector(
        &mut self,
        ty: TypeId,
        span: Span,
        context: &str,
    ) -> Option<BasicValueEnum<'ctx>> {
        match self.get_llvm_type(ty) {
            BasicTypeEnum::VectorType(vector_ty) => Some(vector_ty.const_zero()),
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

    pub(crate) fn float_abs_mask_vector(
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

    pub(crate) fn float_constant_vector(
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

    pub(crate) fn compile_simd_float_intrinsic_call(
        &mut self,
        intrinsic_name: &'static str,
        operand_val: BasicValueEnum<'ctx>,
        result_llvm_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let Some(decl) = self.lookup_intrinsic_declaration(
            intrinsic_name,
            &[result_llvm_ty],
            Span::default(),
            "SIMD floating-point intrinsic",
        ) else {
            return self.get_undef_val(result_llvm_ty);
        };
        self.builder
            .build_call(decl, &[operand_val], "simd_float_intrinsic")
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic()
    }

    pub(crate) fn compile_simd_scalar_cast_value(
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
            let Some(src_int) =
                self.expect_int_value(value, Span::default(), "SIMD scalar int-to-int cast")
            else {
                return self.get_undef_val(dst_llvm_ty);
            };
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
            let Some(src_int) =
                self.expect_int_value(value, Span::default(), "SIMD scalar int-to-float cast")
            else {
                return self.get_undef_val(dst_llvm_ty);
            };
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
            let Some(src_float) =
                self.expect_float_value(value, Span::default(), "SIMD scalar float-to-int cast")
            else {
                return self.get_undef_val(dst_llvm_ty);
            };
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
            let Some(src_float) =
                self.expect_float_value(value, Span::default(), "SIMD scalar float-to-float cast")
            else {
                return self.get_undef_val(dst_llvm_ty);
            };
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
}
