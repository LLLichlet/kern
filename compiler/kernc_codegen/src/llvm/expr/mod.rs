use super::CodeGenerator;
use inkwell::values::BasicValueEnum;
use kernc_mast::{MastExpr, MastExprKind};

mod access;
mod call;
mod cast;
mod control;
mod literal;
mod ops;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub(crate) fn compile_expr(&mut self, expr: &MastExpr) -> BasicValueEnum<'ctx> {
        let expected_llvm_ty = self.get_llvm_type(expr.ty);

        match &expr.kind {
            // === 1. 字面量与常量 ===
            MastExprKind::Undef => self.get_undef_val(expected_llvm_ty),
            MastExprKind::Unreachable => {
                self.builder.build_unreachable().unwrap();
                self.get_undef_val(self.context.i8_type().into())
            }
            MastExprKind::Integer(val) => {
                // 如果吸收上下文后，它变成了一个指针类型
                if expected_llvm_ty.is_pointer_type() {
                    let ptr_ty = expected_llvm_ty.into_pointer_type();
                    if *val == 0 {
                        // 语义为 NULL 空指针
                        ptr_ty.const_null().into()
                    } else {
                        // 语义为硬编码物理地址 (例如 MMIO 0xb8000)，生成 IntToIntPtr 转换
                        let int_val = self.context.i64_type().const_int(*val as u64, false);
                        self.builder
                            .build_int_to_ptr(int_val, ptr_ty, "ptr_lit")
                            .unwrap()
                            .into()
                    }
                } else {
                    // 常规的整数生成
                    expected_llvm_ty
                        .into_int_type()
                        .const_int(*val as u64, false)
                        .into()
                }
            }
            MastExprKind::Float(val) => expected_llvm_ty.into_float_type().const_float(*val).into(),
            MastExprKind::Bool(val) => self
                .context
                .bool_type()
                .const_int(if *val { 1 } else { 0 }, false)
                .into(),
            MastExprKind::StringLiteral(_) => unreachable!("Handled dynamically in Globals"),

            // === 2. 引用与解引用 ===
            MastExprKind::Var(name) => self.compile_var_ref(*name, expected_llvm_ty, expr.span),
            MastExprKind::GlobalRef(mono_id) => self.compile_global_ref(*mono_id, expected_llvm_ty),
            MastExprKind::FuncRef(mono_id) => self.compile_func_ref(*mono_id),
            MastExprKind::AddressOf(operand) => {
                match &operand.kind {
                    // 如果本身就是合法的左值（变量、全局变量、字段访问、索引、解引用），直接安全取地址
                    MastExprKind::Var(_)
                    | MastExprKind::GlobalRef(_)
                    | MastExprKind::FieldAccess { .. }
                    | MastExprKind::IndexAccess { .. }
                    | MastExprKind::Deref(_) => self.compile_lvalue(operand).into(),
                    // 如果是右值取地址（如 i32.{ 404 }.&），立即将其实体化到栈上
                    _ => {
                        let rval = self.compile_expr(operand);
                        let llvm_ty = self.get_llvm_type(operand.ty);

                        // 在当前函数的 entry block 开辟一个隐式的临时变量
                        let temp_ptr = self.create_entry_block_alloca(llvm_ty, "tmp_addrof");

                        // 将右值存入内存
                        self.builder.build_store(temp_ptr, rval).unwrap();

                        // 返回这个临时变量的地址
                        temp_ptr.into()
                    }
                }
            }
            MastExprKind::Deref(operand) => self.compile_deref(operand, expected_llvm_ty),

            // === 3. 聚合数据 (Struct/Union/Array) 构造与访问 ===
            MastExprKind::StructInit { struct_id, fields } => {
                self.compile_struct_init(*struct_id, fields)
            }
            MastExprKind::UnionInit {
                union_id, value, ..
            } => self.compile_union_init(*union_id, value),
            MastExprKind::DataInit {
                data_struct_id,
                tag_value,
                payload,
            } => self.compile_data_init(*data_struct_id, *tag_value, payload),
            MastExprKind::ArrayInit(elems) => self.compile_array_init(elems, expected_llvm_ty),
            MastExprKind::FieldAccess {
                lhs,
                struct_id,
                field_idx,
            } => self.compile_field_access(lhs, *struct_id, *field_idx, expected_llvm_ty),
            MastExprKind::IndexAccess { lhs, index } => {
                self.compile_index_access(lhs, index, expected_llvm_ty, expr.ty)
            }

            // === 4. 运算与赋值 ===
            MastExprKind::Call { callee, args } => self.compile_call(callee, args, expr.ty),
            MastExprKind::Binary { op, lhs, rhs } => self.compile_binary(*op, lhs, rhs),
            MastExprKind::Unary { op, operand } => self.compile_unary(*op, operand),
            MastExprKind::Assign { op, lhs, rhs } => self.compile_assign(*op, lhs, rhs),

            // === 5. 控制流与块级作用域 ===
            MastExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.compile_if(
                cond,
                then_branch,
                else_branch.as_ref(),
                expr.ty,
                expected_llvm_ty,
            ),
            MastExprKind::Loop { body, latch } => self.compile_loop(body, latch.as_ref()),
            MastExprKind::Break => self.compile_break(),
            MastExprKind::Continue => self.compile_continue(),
            MastExprKind::Switch {
                target,
                cases,
                default_case,
            } => self.compile_switch(
                target,
                cases,
                default_case.as_ref(),
                expr.ty,
                expected_llvm_ty,
            ),
            MastExprKind::Block(block) => self.compile_block_expr(block),
            MastExprKind::Return(ret_val) => self.compile_return(ret_val.as_deref()),

            // === 6. 类型转换与胖指针底层操作 ===
            MastExprKind::Cast { kind, operand } => {
                self.compile_cast(*kind, operand, expected_llvm_ty)
            }
            MastExprKind::ConstructFatPointer { data_ptr, meta } => {
                self.compile_construct_fat_ptr(data_ptr, meta)
            }
            MastExprKind::ExtractFatPtrData(fat_ptr_expr) => {
                self.compile_extract_fat_ptr(fat_ptr_expr, 0, "extract_data")
            }
            MastExprKind::ExtractFatPtrMeta(fat_ptr_expr) => {
                self.compile_extract_fat_ptr(fat_ptr_expr, 1, "extract_meta")
            }
            // === 7. LLVM Inline Assembly ===
            MastExprKind::Asm(asm_block) => self.compile_inline_asm(asm_block),
            MastExprKind::BitIntrinsic { kind, operand } => {
                self.compile_bit_intrinsic(*kind, operand, expected_llvm_ty)
            }
            MastExprKind::Trap => {
                let intrinsic = inkwell::intrinsics::Intrinsic::find("llvm.trap").unwrap();
                let decl = intrinsic.get_declaration(&self.module, &[]).unwrap();
                self.builder.build_call(decl, &[], "trap").unwrap();
                self.builder.build_unreachable().unwrap(); // LLVM trap 之后也是不可达的
                self.get_undef_val(expected_llvm_ty)
            }
            MastExprKind::Breakpoint => {
                let intrinsic = inkwell::intrinsics::Intrinsic::find("llvm.debugtrap").unwrap();
                let decl = intrinsic.get_declaration(&self.module, &[]).unwrap();
                self.builder.build_call(decl, &[], "bkpt").unwrap();
                self.context.i8_type().const_zero().into() // Void return
            }
            MastExprKind::Fence => {
                // 生成严格的 Sequential Consistent 内存屏障
                self.builder
                    .build_fence(inkwell::AtomicOrdering::SequentiallyConsistent, 0, "mfence")
                    .unwrap();
                self.context.i8_type().const_zero().into() // Void return
            }
            MastExprKind::Memcpy { dest, src, len } => {
                let d = self.compile_expr(dest).into_pointer_value();
                let s = self.compile_expr(src).into_pointer_value();
                let l = self.compile_expr(len).into_int_value();
                // 1 表示按字节(u8)对齐，这是最安全的假设。高级优化会由LLVM后端处理。
                self.builder.build_memcpy(d, 1, s, 1, l).unwrap();
                self.context.i8_type().const_zero().into() // Void 返回
            }
            MastExprKind::Memset { dest, val, len } => {
                let d = self.compile_expr(dest).into_pointer_value();
                let v = self.compile_expr(val).into_int_value();
                let l = self.compile_expr(len).into_int_value();
                self.builder.build_memset(d, 1, v, l).unwrap();
                self.context.i8_type().const_zero().into() // Void 返回
            }
            MastExprKind::SliceOp {
                lhs,
                start,
                end,
                is_inclusive,
            } => self.compile_slice_op(
                lhs,
                start.as_deref(),
                end.as_deref(),
                *is_inclusive,
                expected_llvm_ty,
            ),
        }
    }
}
