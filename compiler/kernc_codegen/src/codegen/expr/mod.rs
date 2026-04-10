use super::CodeGenerator;
use crate::codegen::expr::call::AtomicCasRequest;
use crate::intrinsics::Intrinsic;
use crate::values::BasicValueEnum;
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
            // === 1. Literals and constants ===
            MastExprKind::Undef => self.get_undef_val(expected_llvm_ty),
            MastExprKind::Unreachable => {
                self.builder.build_unreachable().unwrap();
                self.get_undef_val(self.context.i8_type().into())
            }
            MastExprKind::Integer(val) => {
                // Integer literals can become pointers once contextual typing is applied.
                if expected_llvm_ty.is_pointer_type() {
                    let ptr_ty = expected_llvm_ty.into_pointer_type();
                    if *val == 0 {
                        // Semantic null pointer.
                        ptr_ty.const_null().into()
                    } else {
                        // Semantic fixed physical address, for example MMIO `0xb8000`.
                        let int_val = self.context.i64_type().const_int(*val as u64, false);
                        self.builder
                            .build_int_to_ptr(int_val, ptr_ty, "ptr_lit")
                            .unwrap()
                            .into()
                    }
                } else {
                    // Normal integer literal emission.
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
            MastExprKind::StringLiteral(_) => {
                self.sess.emit_ice(
                    expr.span,
                    "Kern ICE (Codegen): Unexpected StringLiteral reached expression codegen.",
                );
                self.get_undef_val(expected_llvm_ty)
            }

            // === 2. Addressing and dereference ===
            MastExprKind::Var(name) => self.compile_var_ref(*name, expected_llvm_ty, expr.span),
            MastExprKind::GlobalRef(mono_id) => self.compile_global_ref(*mono_id, expected_llvm_ty),
            MastExprKind::FuncRef(mono_id) => self.compile_func_ref(*mono_id),
            MastExprKind::AddressOf(operand) => {
                match &operand.kind {
                    // Legal lvalues can be addressed directly.
                    MastExprKind::Var(_)
                    | MastExprKind::GlobalRef(_)
                    | MastExprKind::FieldAccess { .. }
                    | MastExprKind::IndexAccess { .. }
                    | MastExprKind::Deref(_) => self.compile_lvalue(operand).into(),
                    // Addressing an rvalue materializes it on the stack first.
                    _ => {
                        let rval = self.compile_expr(operand);
                        if let Some(fallback) = self.expr_terminated_fallback(expected_llvm_ty) {
                            return fallback;
                        }
                        let llvm_ty = self.get_llvm_type(operand.ty);

                        // Allocate an implicit temporary in the current function's entry block.
                        let temp_ptr = self.create_entry_block_alloca(llvm_ty, "tmp_addrof");

                        // Store the rvalue into memory.
                        self.builder.build_store(temp_ptr, rval).unwrap();

                        // Return the address of that temporary.
                        temp_ptr.into()
                    }
                }
            }
            MastExprKind::Deref(operand) => self.compile_deref(operand, expected_llvm_ty),

            // === 3. Aggregate construction and access ===
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

            // === 4. Operators and assignment ===
            MastExprKind::Call { callee, args } => self.compile_call(callee, args, expr.ty),
            MastExprKind::Binary { op, lhs, rhs } => self.compile_binary(*op, lhs, rhs),
            MastExprKind::Unary { op, operand } => self.compile_unary(*op, operand),
            MastExprKind::Assign { op, lhs, rhs } => self.compile_assign(*op, lhs, rhs),

            // === 5. Control flow and block scopes ===
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
            MastExprKind::Loop { body, latch } => {
                self.compile_loop(body, latch.as_ref(), expr.ty, expected_llvm_ty)
            }
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

            // === 6. Casts and fat-pointer primitives ===
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
            MastExprKind::SimdUnaryIntrinsic { kind, operand } => {
                self.compile_simd_unary_intrinsic(*kind, operand, expr.ty)
            }
            MastExprKind::SimdBinaryIntrinsic { kind, lhs, rhs } => {
                self.compile_simd_binary_intrinsic(*kind, lhs, rhs, expr.ty)
            }
            MastExprKind::SimdReduce { kind, operand } => {
                self.compile_simd_reduce(*kind, operand, expr.ty)
            }
            MastExprKind::SimdAny { operand } => self.compile_simd_reduce_any(operand),
            MastExprKind::SimdAll { operand } => self.compile_simd_reduce_all(operand),
            MastExprKind::SimdSplat { value } => self.compile_simd_splat(value, expr.ty),
            MastExprKind::SimdCast { value } => self.compile_simd_cast(value, expr.ty),
            MastExprKind::SimdBitcast { value } => self.compile_simd_bitcast(value, expr.ty),
            MastExprKind::SimdSelect {
                mask,
                on_true,
                on_false,
            } => self.compile_simd_select(mask, on_true, on_false),
            MastExprKind::SimdShuffle { lhs, rhs, indices } => {
                self.compile_simd_shuffle(lhs, rhs, indices)
            }
            MastExprKind::SimdInsertHalf {
                base,
                half,
                high_half,
            } => self.compile_simd_insert_half(base, half, expr.ty, *high_half),
            MastExprKind::SimdLoad { ptr, align } => self.compile_simd_load(ptr, expr.ty, *align),
            MastExprKind::SimdStore { ptr, value, align } => {
                self.compile_simd_store(ptr, value, *align)
            }
            MastExprKind::SimdMaskedLoad {
                ptr,
                mask,
                or_else,
                align,
            } => self.compile_simd_masked_load(ptr, mask, or_else, expr.ty, *align),
            MastExprKind::SimdMaskedStore {
                ptr,
                mask,
                value,
                align,
            } => self.compile_simd_masked_store(ptr, mask, value, *align),
            MastExprKind::SimdGather { ptr, indices } => {
                self.compile_simd_gather(ptr, indices, expr.ty)
            }
            MastExprKind::SimdScatter {
                ptr,
                indices,
                value,
            } => self.compile_simd_scatter(ptr, indices, value),
            MastExprKind::SimdMaskedGather {
                ptr,
                indices,
                mask,
                or_else,
            } => self.compile_simd_masked_gather(ptr, indices, mask, or_else, expr.ty),
            MastExprKind::SimdMaskedScatter {
                ptr,
                indices,
                mask,
                value,
            } => self.compile_simd_masked_scatter(ptr, indices, mask, value),
            MastExprKind::AtomicLoad { ptr, ordering } => {
                self.compile_atomic_load(ptr, *ordering, expected_llvm_ty)
            }
            MastExprKind::AtomicStore {
                ptr,
                value,
                ordering,
            } => self.compile_atomic_store(ptr, value, *ordering),
            MastExprKind::AtomicCas {
                weak,
                ptr,
                expected,
                desired,
                success,
                failure,
            } => self.compile_atomic_cas(AtomicCasRequest {
                expr_ty: expr.ty,
                weak: *weak,
                ptr,
                expected,
                desired,
                success: *success,
                failure: *failure,
            }),
            MastExprKind::AtomicRmw {
                op,
                ptr,
                value,
                ordering,
            } => self.compile_atomic_rmw(expr.ty, *op, ptr, value, *ordering),
            MastExprKind::Trap => {
                let intrinsic = Intrinsic::find("llvm.trap").unwrap();
                let decl = intrinsic.get_declaration(&self.module, &[]).unwrap();
                self.builder.build_call(decl, &[], "trap").unwrap();
                self.builder.build_unreachable().unwrap(); // `llvm.trap` never returns.
                self.get_undef_val(expected_llvm_ty)
            }
            MastExprKind::Breakpoint => {
                let intrinsic = Intrinsic::find("llvm.debugtrap").unwrap();
                let decl = intrinsic.get_declaration(&self.module, &[]).unwrap();
                self.builder.build_call(decl, &[], "bkpt").unwrap();
                self.context.i8_type().const_zero().into() // Void return
            }
            MastExprKind::Fence { ordering } => self.compile_atomic_fence(*ordering),
            MastExprKind::Memcpy { dest, src, len } => {
                let d = self.compile_expr(dest);
                if let Some(fallback) = self.expr_terminated_fallback(expected_llvm_ty) {
                    return fallback;
                }
                let s = self.compile_expr(src);
                if let Some(fallback) = self.expr_terminated_fallback(expected_llvm_ty) {
                    return fallback;
                }
                let l = self.compile_expr(len);
                if let Some(fallback) = self.expr_terminated_fallback(expected_llvm_ty) {
                    return fallback;
                }
                // Alignment 1 is the safest conservative choice; LLVM may optimize further.
                self.builder
                    .build_memcpy(
                        d.into_pointer_value(),
                        1,
                        s.into_pointer_value(),
                        1,
                        l.into_int_value(),
                    )
                    .unwrap();
                self.context.i8_type().const_zero().into() // Void return.
            }
            MastExprKind::Memmove { dest, src, len } => {
                let d = self.compile_expr(dest);
                if let Some(fallback) = self.expr_terminated_fallback(expected_llvm_ty) {
                    return fallback;
                }
                let s = self.compile_expr(src);
                if let Some(fallback) = self.expr_terminated_fallback(expected_llvm_ty) {
                    return fallback;
                }
                let l = self.compile_expr(len);
                if let Some(fallback) = self.expr_terminated_fallback(expected_llvm_ty) {
                    return fallback;
                }
                self.builder
                    .build_memmove(
                        d.into_pointer_value(),
                        1,
                        s.into_pointer_value(),
                        1,
                        l.into_int_value(),
                    )
                    .unwrap();
                self.context.i8_type().const_zero().into() // Void return.
            }
            MastExprKind::Memset { dest, val, len } => {
                let d = self.compile_expr(dest);
                if let Some(fallback) = self.expr_terminated_fallback(expected_llvm_ty) {
                    return fallback;
                }
                let v = self.compile_expr(val);
                if let Some(fallback) = self.expr_terminated_fallback(expected_llvm_ty) {
                    return fallback;
                }
                let l = self.compile_expr(len);
                if let Some(fallback) = self.expr_terminated_fallback(expected_llvm_ty) {
                    return fallback;
                }
                self.builder
                    .build_memset(
                        d.into_pointer_value(),
                        1,
                        v.into_int_value(),
                        l.into_int_value(),
                    )
                    .unwrap();
                self.context.i8_type().const_zero().into() // Void return.
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
