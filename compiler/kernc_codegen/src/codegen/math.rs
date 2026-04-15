use super::CodeGenerator;
use crate::values::{BasicValueEnum, FloatValue, IntValue};
use crate::{FloatPredicate, IntPredicate};
use kernc_ast::BinaryOperator;
use kernc_sema::ty::{PrimitiveType, TypeId, TypeKind};
use kernc_utils::Span;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub(crate) fn zero_i8_value(&self) -> BasicValueEnum<'ctx> {
        self.context.i8_type().const_zero().into()
    }

    fn ptr_elem_llvm_type(
        &mut self,
        ptr_ty: TypeId,
        span: Span,
        context: &str,
    ) -> Option<crate::types::BasicTypeEnum<'ctx>> {
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

    fn is_address_pointer_type(&self, ty: TypeId) -> bool {
        matches!(
            self.type_registry.get(self.type_registry.normalize(ty)),
            TypeKind::VolatilePtr { .. }
        )
    }

    fn address_offset_int(&self, value: IntValue<'ctx>, ty: TypeId, name: &str) -> IntValue<'ctx> {
        let target_ty = self.context.i64_type();
        if value.get_type() == target_ty {
            return value;
        }

        if self.is_signed_int(ty) {
            self.builder
                .build_int_s_extend(value, target_ty, name)
                .unwrap()
        } else {
            self.builder
                .build_int_z_extend(value, target_ty, name)
                .unwrap()
        }
    }

    fn compile_address_ptr_offset(
        &self,
        ptr_val: crate::values::PointerValue<'ctx>,
        int_val: IntValue<'ctx>,
        int_ty: TypeId,
        subtract: bool,
    ) -> BasicValueEnum<'ctx> {
        let addr_ty = self.context.i64_type();
        let base = self
            .builder
            .build_ptr_to_int(ptr_val, addr_ty, "addr_ptr_base")
            .unwrap();
        let offset = self.address_offset_int(int_val, int_ty, "addr_ptr_offset");
        let addr = if subtract {
            self.builder
                .build_int_sub(base, offset, "addr_ptr_sub")
                .unwrap()
        } else {
            self.builder
                .build_int_add(base, offset, "addr_ptr_add")
                .unwrap()
        };

        self.builder
            .build_int_to_ptr(addr, ptr_val.get_type(), "addr_ptr_result")
            .unwrap()
            .into()
    }

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

    pub(crate) fn compile_ptr_math(
        &mut self,
        op: BinaryOperator,
        l_val: BasicValueEnum<'ctx>,
        r_val: BasicValueEnum<'ctx>,
        lhs_ty: TypeId,
        rhs_ty: TypeId,
        span: Span,
    ) -> BasicValueEnum<'ctx> {
        use BinaryOperator::*;
        match op {
            Add => {
                let (ptr_val, int_val, ptr_ty, int_ty) = if l_val.is_pointer_value() {
                    if !r_val.is_int_value() {
                        self.sess.emit_ice(
                            span,
                            "Kern ICE (Codegen): expected integer for RHS of pointer addition.",
                        );
                        return self.zero_i8_value();
                    }
                    (
                        l_val.into_pointer_value(),
                        r_val.into_int_value(),
                        lhs_ty,
                        rhs_ty,
                    )
                } else {
                    if !l_val.is_int_value() {
                        self.sess.emit_ice(
                            span,
                            "Kern ICE (Codegen): expected integer for LHS of pointer addition.",
                        );
                        return self.zero_i8_value();
                    }
                    (
                        r_val.into_pointer_value(),
                        l_val.into_int_value(),
                        rhs_ty,
                        lhs_ty,
                    )
                };

                if self.is_address_pointer_type(ptr_ty) {
                    return self.compile_address_ptr_offset(ptr_val, int_val, int_ty, false);
                }

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
                    if self.is_address_pointer_type(lhs_ty) {
                        let addr_ty = self.context.i64_type();
                        let l_addr = self
                            .builder
                            .build_ptr_to_int(l_ptr, addr_ty, "addr_ptr_lhs")
                            .unwrap();
                        let r_addr = self
                            .builder
                            .build_ptr_to_int(r_ptr, addr_ty, "addr_ptr_rhs")
                            .unwrap();
                        return self
                            .builder
                            .build_int_sub(l_addr, r_addr, "addr_ptr_diff")
                            .unwrap()
                            .into();
                    }
                    let Some(elem_sema_ty) = self.type_registry.get_elem_type(lhs_ty) else {
                        self.sess.emit_ice(
                            span,
                            "Kern ICE (Codegen): pointer subtraction missing pointee type.",
                        );
                        return self.zero_i8_value();
                    };

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
                    if self.is_address_pointer_type(lhs_ty) {
                        return self.compile_address_ptr_offset(ptr_val, int_val, rhs_ty, true);
                    }
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
            Equal | NotEqual | LessThan | LessOrEqual | GreaterThan | GreaterOrEqual => {
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

    pub(crate) fn compile_int_math(
        &mut self,
        op: BinaryOperator,
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

    pub(crate) fn compile_float_math(
        &mut self,
        op: BinaryOperator,
        l_float: FloatValue<'ctx>,
        r_float: FloatValue<'ctx>,
        span: Span,
    ) -> BasicValueEnum<'ctx> {
        use BinaryOperator::*;
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
}
