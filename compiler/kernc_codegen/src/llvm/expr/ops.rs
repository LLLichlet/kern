use crate::llvm::CodeGenerator;
use inkwell::values::BasicValueEnum;
use kernc_ast::{self as ast, BinaryOperator};
use kernc_mast::MastExpr;
use kernc_sema::ty::TypeKind;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub(crate) fn compile_binary(
        &mut self,
        op: ast::BinaryOperator,
        lhs: &MastExpr,
        rhs: &MastExpr,
    ) -> BasicValueEnum<'ctx> {
        let l_val = self.compile_expr(lhs);
        let r_val = self.compile_expr(rhs);

        // 处理指针算术 ---
        if l_val.is_pointer_value() || r_val.is_pointer_value() {
            use BinaryOperator::*;
            match op {
                Add => {
                    // ptr + int 或 int + ptr
                    let (ptr_val, int_val) = if l_val.is_pointer_value() {
                        (l_val.into_pointer_value(), r_val.into_int_value())
                    } else {
                        (r_val.into_pointer_value(), l_val.into_int_value())
                    };
                    
                    // 获取指针指向的底层元素类型，以便 LLVM 计算步长
                    let ptr_ty = if l_val.is_pointer_value() { lhs.ty } else { rhs.ty };
                    let elem_sema_ty = self.type_registry.get_elem_type(ptr_ty).unwrap();
                    let elem_llvm_ty = self.get_llvm_type(elem_sema_ty);

                    // LLVM 的 GEP (GetElementPtr) 专门用于处理指针偏移，自动计算 sizeOf
                    unsafe {
                        self.builder
                            .build_gep(elem_llvm_ty, ptr_val, &[int_val], "ptr_add")
                            .unwrap()
                            .into()
                    }
                }
                Subtract => {
                    if l_val.is_pointer_value() && r_val.is_pointer_value() {
                        // ptr - ptr: 计算两个指针之间的元素个数差异
                        let l_ptr = l_val.into_pointer_value();
                        let r_ptr = r_val.into_pointer_value();
                        
                        let elem_sema_ty = self.type_registry.get_elem_type(lhs.ty).unwrap();
                        let elem_llvm_ty = self.get_llvm_type(elem_sema_ty);
                        
                        self.builder
                            .build_ptr_diff(elem_llvm_ty, l_ptr, r_ptr, "ptr_diff")
                            .unwrap()
                            .into()
                    } else {
                        // ptr - int: 负向偏移
                        let ptr_val = l_val.into_pointer_value();
                        let int_val = r_val.into_int_value();
                        
                        // 生成一个负的偏移量
                        let neg_int = self.builder.build_int_neg(int_val, "neg_offset").unwrap();

                        let elem_sema_ty = self.type_registry.get_elem_type(lhs.ty).unwrap();
                        let elem_llvm_ty = self.get_llvm_type(elem_sema_ty);

                        unsafe {
                            self.builder
                                .build_gep(elem_llvm_ty, ptr_val, &[neg_int], "ptr_sub")
                                .unwrap()
                                .into()
                        }
                    }
                }
                _ => unreachable!("Invalid pointer arithmetic operation lowered to MAST"),
            }
        } else if l_val.is_int_value() && r_val.is_int_value() {
            let l_int = l_val.into_int_value();
            let r_int = r_val.into_int_value();
            use BinaryOperator::*;
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
                Divide => self
                    .builder
                    .build_int_signed_div(l_int, r_int, "sdiv")
                    .unwrap()
                    .into(),
                Modulo => self
                    .builder
                    .build_int_signed_rem(l_int, r_int, "srem")
                    .unwrap()
                    .into(),
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
                    .build_right_shift(l_int, r_int, false, "shr")
                    .unwrap()
                    .into(),
                Equal => self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::EQ, l_int, r_int, "eq")
                    .unwrap()
                    .into(),
                NotEqual => self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::NE, l_int, r_int, "ne")
                    .unwrap()
                    .into(),
                LessThan => self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::SLT, l_int, r_int, "slt")
                    .unwrap()
                    .into(),
                LessOrEqual => self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::SLE, l_int, r_int, "sle")
                    .unwrap()
                    .into(),
                GreaterThan => self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::SGT, l_int, r_int, "sgt")
                    .unwrap()
                    .into(),
                GreaterOrEqual => self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::SGE, l_int, r_int, "sge")
                    .unwrap()
                    .into(),
                _ => unreachable!("Operator handled elsewhere"),
            }
        } else if l_val.is_float_value() && r_val.is_float_value() {
            let l_float = l_val.into_float_value();
            let r_float = r_val.into_float_value();
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
                    .build_float_compare(inkwell::FloatPredicate::OEQ, l_float, r_float, "feq")
                    .unwrap()
                    .into(),
                NotEqual => self
                    .builder
                    .build_float_compare(inkwell::FloatPredicate::ONE, l_float, r_float, "fne")
                    .unwrap()
                    .into(),
                LessThan => self
                    .builder
                    .build_float_compare(inkwell::FloatPredicate::OLT, l_float, r_float, "flt")
                    .unwrap()
                    .into(),
                LessOrEqual => self
                    .builder
                    .build_float_compare(inkwell::FloatPredicate::OLE, l_float, r_float, "fle")
                    .unwrap()
                    .into(),
                GreaterThan => self
                    .builder
                    .build_float_compare(inkwell::FloatPredicate::OGT, l_float, r_float, "fgt")
                    .unwrap()
                    .into(),
                GreaterOrEqual => self
                    .builder
                    .build_float_compare(inkwell::FloatPredicate::OGE, l_float, r_float, "fge")
                    .unwrap()
                    .into(),
                _ => unreachable!(),
            }
        } else {
            unreachable!()
        }
    }

    pub(crate) fn compile_unary(
        &mut self,
        op: ast::UnaryOperator,
        operand: &MastExpr,
    ) -> BasicValueEnum<'ctx> {
        let op_val = self.compile_expr(operand);
        match op {
            ast::UnaryOperator::Negate => {
                if op_val.is_int_value() {
                    self.builder
                        .build_int_neg(op_val.into_int_value(), "neg")
                        .unwrap()
                        .into()
                } else {
                    self.builder
                        .build_float_neg(op_val.into_float_value(), "fneg")
                        .unwrap()
                        .into()
                }
            }
            ast::UnaryOperator::LogicalNot | ast::UnaryOperator::BitwiseNot => self
                .builder
                .build_not(op_val.into_int_value(), "not")
                .unwrap()
                .into(),
            ast::UnaryOperator::LengthOf => {
                // MAST 保证了此时的类型已经是纯物理类型
                let norm_ty = self.type_registry.normalize(operand.ty);
                match self.type_registry.get(norm_ty) {
                    TypeKind::Array { len, .. } => {
                        self.context.i64_type().const_int(*len, false).into()
                    }
                    TypeKind::Slice { .. } => self
                        .builder
                        .build_extract_value(op_val.into_struct_value(), 1, "slice_len")
                        .unwrap(),
                    _ => unreachable!(),
                }
            }
            _ => unreachable!(),
        }
    }

    pub(crate) fn compile_assign(
        &mut self,
        op: ast::AssignmentOperator,
        lhs: &MastExpr,
        rhs: &MastExpr,
    ) -> BasicValueEnum<'ctx> {
        let ptr = self.compile_lvalue(lhs);
        let rhs_val = self.compile_expr(rhs);

        if op == ast::AssignmentOperator::Assign {
            self.builder.build_store(ptr, rhs_val).unwrap();
        } else {
            let expected_lhs_ty = self.get_llvm_type(lhs.ty);
            let lhs_val = self
                .builder
                .build_load(expected_lhs_ty, ptr, "assign_load")
                .unwrap();

            let new_val: inkwell::values::BasicValueEnum<'ctx> = if lhs_val.is_int_value() {
                let l_int = lhs_val.into_int_value();
                let r_int = rhs_val.into_int_value();
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
                    DivideAssign => self
                        .builder
                        .build_int_signed_div(l_int, r_int, "div_a")
                        .unwrap()
                        .into(),
                    ModuloAssign => self
                        .builder
                        .build_int_signed_rem(l_int, r_int, "rem_a")
                        .unwrap()
                        .into(),
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
                        .build_right_shift(l_int, r_int, false, "shr_a")
                        .unwrap()
                        .into(),
                    _ => unreachable!(),
                }
            } else if lhs_val.is_float_value() {
                // 新增：处理浮点数复合赋值
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
                    _ => unreachable!("Unsupported float assignment operator"),
                }
            } else {
                unreachable!("Unsupported type for assignment");
            };
            self.builder.build_store(ptr, new_val).unwrap();
        }
        self.context.i8_type().const_zero().into()
    }
}
