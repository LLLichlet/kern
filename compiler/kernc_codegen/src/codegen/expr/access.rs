use crate::codegen::CodeGenerator;
use crate::types::{BasicTypeEnum, StructType};
use crate::values::{BasicValueEnum, IntValue, PointerValue};
use kernc_mast::{MastExpr, MastExprKind, MonoId};
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{Span, SymbolId};

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    fn null_ptr(&self) -> PointerValue<'ctx> {
        self.context.ptr_type(Default::default()).const_zero()
    }

    fn lookup_struct_type(
        &mut self,
        struct_id: MonoId,
        span: Span,
        context: &str,
    ) -> Option<StructType<'ctx>> {
        match self.structs.get(&struct_id).copied() {
            Some(ty) => Some(ty),
            None => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): missing struct MonoId {:?} while compiling {}.",
                        struct_id, context
                    ),
                );
                None
            }
        }
    }

    fn slice_base_parts(
        &mut self,
        lhs: &MastExpr,
        lhs_val: BasicValueEnum<'ctx>,
    ) -> Option<(PointerValue<'ctx>, Option<IntValue<'ctx>>, TypeId)> {
        let norm_lhs = self.type_registry.normalize(lhs.ty);
        match self.type_registry.get(norm_lhs) {
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                Some((lhs_val.into_pointer_value(), None, *elem))
            }
            TypeKind::Slice { elem, .. } => {
                let struct_val = lhs_val.into_struct_value();
                let ptr = self
                    .builder
                    .build_extract_value(struct_val, 0, "s_ptr")
                    .unwrap()
                    .into_pointer_value();
                let len = self
                    .builder
                    .build_extract_value(struct_val, 1, "s_len")
                    .unwrap()
                    .into_int_value();
                Some((ptr, Some(len), *elem))
            }
            TypeKind::Array { elem, len, .. } => {
                let ptr = self.compile_lvalue(lhs);
                let len_val = self.context.i64_type().const_int(*len, false);
                Some((ptr, Some(len_val), *elem))
            }
            _ => None,
        }
    }

    pub(crate) fn compile_lvalue(&mut self, expr: &MastExpr) -> PointerValue<'ctx> {
        match &expr.kind {
            MastExprKind::Var(name) => {
                if let Some(ptr) = self.locals.get(name) {
                    *ptr
                } else {
                    let var_name = self.resolve_symbol(*name);
                    self.sess.emit_ice(
                        expr.span,
                        format!(
                            "Local variable `{}` not found during l-value compilation",
                            var_name
                        ),
                    );
                    self.null_ptr()
                }
            }
            MastExprKind::GlobalRef(mono_id) => {
                if let Some(g) = self.globals.get(mono_id) {
                    g.as_pointer_value()
                } else {
                    self.sess.emit_ice(
                        expr.span,
                        "Global reference not found in codegen".to_string(),
                    );
                    self.null_ptr()
                }
            }
            MastExprKind::FieldAccess {
                lhs,
                struct_id,
                field_idx,
            } => {
                let struct_ptr = self.compile_lvalue(lhs);
                let Some(struct_llvm_ty) =
                    self.lookup_struct_type(*struct_id, expr.span, "field l-value")
                else {
                    return self.null_ptr();
                };
                self.builder
                    .build_struct_gep(struct_llvm_ty, struct_ptr, *field_idx as u32, "lvalue_gep")
                    .unwrap()
            }
            MastExprKind::IndexAccess { lhs, index } => {
                let idx_val = self.compile_expr(index).into_int_value();
                let norm_lhs = self.type_registry.normalize(lhs.ty);

                if let TypeKind::Slice { .. } = self.type_registry.get(norm_lhs) {
                    let slice_val = self.compile_expr(lhs).into_struct_value();
                    let ptr_val = self
                        .builder
                        .build_extract_value(slice_val, 0, "slice_ptr")
                        .unwrap()
                        .into_pointer_value();
                    let elem_ty = self.get_llvm_type(expr.ty);
                    unsafe {
                        self.builder
                            .build_gep(elem_ty, ptr_val, &[idx_val], "slice_lvalue")
                            .unwrap()
                    }
                } else if let TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. } =
                    self.type_registry.get(norm_lhs)
                {
                    let ptr_val = self.compile_expr(lhs).into_pointer_value();
                    let elem_ty = self.get_llvm_type(expr.ty);
                    unsafe {
                        self.builder
                            .build_gep(elem_ty, ptr_val, &[idx_val], "ptr_lvalue")
                            .unwrap()
                    }
                } else {
                    let array_ptr = self.compile_lvalue(lhs);
                    let zero = self.context.i64_type().const_zero();
                    let array_llvm_ty = self.get_llvm_type(lhs.ty);
                    unsafe {
                        self.builder
                            .build_gep(array_llvm_ty, array_ptr, &[zero, idx_val], "array_lvalue")
                            .unwrap()
                    }
                }
            }
            MastExprKind::Deref(operand) => self.compile_expr(operand).into_pointer_value(),

            // 当编译器需要一个左值（内存地址），但遇到的是一个纯右值
            // （比如函数调用 `Call` 返回的结构体，或者是字面量）时，
            // 我们在当前函数的栈帧上开辟一块临时内存，将右值存进去，并返回这个内存地址。
            // 这完美解决了“动态派发后的连缀访问”引发的崩溃问题。
            _ => {
                let rval = self.compile_expr(expr);
                let llvm_ty = self.get_llvm_type(expr.ty);
                let temp_ptr = self.create_entry_block_alloca(llvm_ty, "tmp_materialized_lvalue");
                self.builder.build_store(temp_ptr, rval).unwrap();
                temp_ptr
            }
        }
    }

    pub(crate) fn compile_var_ref(
        &mut self,
        name: SymbolId,
        expected_ty: BasicTypeEnum<'ctx>,
        span: Span,
    ) -> BasicValueEnum<'ctx> {
        let var_name = self.resolve_symbol(name);

        if let Some(ptr) = self.locals.get(&name) {
            return self
                .builder
                .build_load(expected_ty, *ptr, &format!("load_{}", var_name))
                .unwrap();
        }

        if let Some(global_val) = self.module.get_global(var_name) {
            return self
                .builder
                .build_load(
                    expected_ty,
                    global_val.as_pointer_value(),
                    &format!("load_global_{}", var_name),
                )
                .unwrap();
        }

        self.sess.emit_ice(
            span,
            format!(
                "Variable `{}` (SymbolId: {}) not found in locals or globals!\nDid the lowerer forget to allocate it, or is it an unhandled discard `_`?",
                var_name, name.0
            )
        );
        expected_ty.const_zero()
    }

    pub(crate) fn compile_global_ref(
        &mut self,
        mono_id: MonoId,
        expected_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let Some(global_val) = self.globals.get(&mono_id) else {
            self.sess.emit_ice(
                Span::default(),
                format!(
                    "Global MonoId {:?} not found during code generation",
                    mono_id
                ),
            );
            return expected_ty.const_zero();
        };
        let ptr = global_val.as_pointer_value();
        self.builder
            .build_load(expected_ty, ptr, "global_load")
            .unwrap()
    }

    pub(crate) fn compile_func_ref(&mut self, mono_id: MonoId) -> BasicValueEnum<'ctx> {
        let Some(func_val) = self.functions.get(&mono_id) else {
            self.sess.emit_ice(
                Span::default(),
                format!(
                    "Function MonoId {:?} not found during code generation",
                    mono_id
                ),
            );
            return self
                .context
                .ptr_type(Default::default())
                .const_zero()
                .into();
        };
        func_val.as_global_value().as_pointer_value().into()
    }

    pub(crate) fn compile_deref(
        &mut self,
        operand: &MastExpr,
        expected_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let ptr_val = self.compile_expr(operand).into_pointer_value();
        self.builder
            .build_load(expected_ty, ptr_val, "deref")
            .unwrap()
    }

    pub(crate) fn compile_field_access(
        &mut self,
        lhs: &MastExpr,
        struct_id: MonoId,
        field_idx: usize,
        expected_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let struct_ptr = self.compile_lvalue(lhs);
        let Some(struct_llvm_ty) = self.lookup_struct_type(struct_id, lhs.span, "field access")
        else {
            return expected_ty.const_zero();
        };
        let is_union = self.union_ids.contains(&struct_id);

        if is_union {
            self.builder
                .build_load(expected_ty, struct_ptr, "union_field_load")
                .unwrap()
        } else {
            let field_ptr = self
                .builder
                .build_struct_gep(struct_llvm_ty, struct_ptr, field_idx as u32, "field_gep")
                .unwrap();
            self.builder
                .build_load(expected_ty, field_ptr, "field_load")
                .unwrap()
        }
    }

    pub(crate) fn compile_index_access(
        &mut self,
        lhs: &MastExpr,
        index: &MastExpr,
        expected_ty: BasicTypeEnum<'ctx>,
        expr_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let idx_val = self.compile_expr(index).into_int_value();
        let norm_lhs = self.type_registry.normalize(lhs.ty);

        let elem_ptr = if let TypeKind::Slice { .. } = self.type_registry.get(norm_lhs) {
            let slice_val = self.compile_expr(lhs).into_struct_value();
            let ptr_val = self
                .builder
                .build_extract_value(slice_val, 0, "slice_ptr")
                .unwrap()
                .into_pointer_value();
            let elem_ty = self.get_llvm_type(expr_ty);
            unsafe {
                self.builder
                    .build_gep(elem_ty, ptr_val, &[idx_val], "slice_idx")
                    .unwrap()
            }
        } else if let TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. } =
            self.type_registry.get(norm_lhs)
        {
            let ptr_val = self.compile_expr(lhs).into_pointer_value();
            let elem_ty = self.get_llvm_type(expr_ty);
            unsafe {
                self.builder
                    .build_gep(elem_ty, ptr_val, &[idx_val], "ptr_idx")
                    .unwrap()
            }
        } else {
            let array_ptr = self.compile_lvalue(lhs);
            let zero = self.context.i64_type().const_zero();
            let array_llvm_ty = self.get_llvm_type(lhs.ty);
            unsafe {
                self.builder
                    .build_gep(array_llvm_ty, array_ptr, &[zero, idx_val], "array_idx")
                    .unwrap()
            }
        };

        self.builder
            .build_load(expected_ty, elem_ptr, "idx_load")
            .unwrap()
    }

    /// 专门处理切片构造 [start..end] 的底层 LLVM 生成
    pub(crate) fn compile_slice_op(
        &mut self,
        lhs: &MastExpr,
        start: Option<&MastExpr>,
        end: Option<&MastExpr>,
        is_inclusive: bool,
        expected_llvm_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let lhs_val = self.compile_expr(lhs);
        let Some((base_ptr, base_len, elem_ty)) = self.slice_base_parts(lhs, lhs_val) else {
            self.sess.emit_ice(
                lhs.span,
                format!(
                    "Kern ICE (Codegen): invalid base type `{:?}` for slice operation.",
                    self.type_registry.get(self.type_registry.normalize(lhs.ty))
                ),
            );
            return expected_llvm_ty.const_zero();
        };

        // 2. 计算 start (缺省为 0)
        let start_val = if let Some(s) = start {
            self.compile_expr(s).into_int_value()
        } else {
            self.context.i64_type().const_zero()
        };

        // 3. 计算 end (缺省为基底长度)
        let end_val = if let Some(e) = end {
            self.compile_expr(e).into_int_value()
        } else {
            let Some(len) = base_len else {
                self.sess.emit_ice(
                    lhs.span,
                    "Kern ICE (Codegen): slicing a raw pointer requires an explicit end index.",
                );
                return expected_llvm_ty.const_zero();
            };
            len
        };

        // 4. 计算新切片的长度: len = end - start + (1 if inclusive)
        let mut slice_len = self
            .builder
            .build_int_sub(end_val, start_val, "slice_len")
            .unwrap();
        if is_inclusive {
            let one = self.context.i64_type().const_int(1, false);
            slice_len = self
                .builder
                .build_int_add(slice_len, one, "slice_len_inc")
                .unwrap();
        }

        // 5. 偏移基底指针: ptr = base_ptr + start
        let llvm_elem_ty = self.get_llvm_type(elem_ty);

        let slice_ptr = unsafe {
            self.builder
                .build_gep(llvm_elem_ty, base_ptr, &[start_val], "slice_ptr")
                .unwrap()
        };

        // 6. 组装并返回新的胖指针结构体
        let struct_ty = expected_llvm_ty.into_struct_type();
        let mut slice_struct = struct_ty.get_undef();
        slice_struct = self
            .builder
            .build_insert_value(slice_struct, slice_ptr, 0, "insert_ptr")
            .unwrap()
            .into_struct_value();
        slice_struct = self
            .builder
            .build_insert_value(slice_struct, slice_len, 1, "insert_len")
            .unwrap()
            .into_struct_value();

        slice_struct.into()
    }
}
