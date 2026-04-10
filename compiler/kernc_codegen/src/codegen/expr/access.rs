use crate::codegen::CodeGenerator;
use crate::types::{BasicTypeEnum, StructType};
use crate::values::{BasicValueEnum, IntValue, PointerValue};
use kernc_mast::{MastExpr, MastExprKind, MonoId};
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{Span, SymbolId};

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    fn expr_is_addressable(expr: &MastExpr) -> bool {
        matches!(
            expr.kind,
            MastExprKind::Var(_)
                | MastExprKind::GlobalRef(_)
                | MastExprKind::FieldAccess { .. }
                | MastExprKind::IndexAccess { .. }
                | MastExprKind::Deref(_)
        )
    }

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
                if self.current_block_is_terminated() {
                    return self.null_ptr();
                }
                if self.union_ids.contains(struct_id) {
                    // Union fields all begin at offset 0 and share the same storage.
                    // Under opaque pointers we can treat the union allocation itself as
                    // the lvalue base for the selected field type.
                    return struct_ptr;
                }
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
                let idx_raw = self.compile_expr(index);
                if self.current_block_is_terminated() {
                    return self.null_ptr();
                }
                let idx_val = idx_raw.into_int_value();
                let norm_lhs = self.type_registry.normalize(lhs.ty);

                if self.type_registry.is_simd(norm_lhs) {
                    self.sess.emit_ice(
                        expr.span,
                        "Kern ICE (Codegen): attempted to form an lvalue for a SIMD lane.",
                    );
                    return self.null_ptr();
                }

                if let TypeKind::Slice { .. } = self.type_registry.get(norm_lhs) {
                    let slice_raw = self.compile_expr(lhs);
                    if self.current_block_is_terminated() {
                        return self.null_ptr();
                    }
                    let slice_val = slice_raw.into_struct_value();
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
                    let ptr_raw = self.compile_expr(lhs);
                    if self.current_block_is_terminated() {
                        return self.null_ptr();
                    }
                    let ptr_val = ptr_raw.into_pointer_value();
                    let elem_ty = self.get_llvm_type(expr.ty);
                    unsafe {
                        self.builder
                            .build_gep(elem_ty, ptr_val, &[idx_val], "ptr_lvalue")
                            .unwrap()
                    }
                } else {
                    let array_ptr = self.compile_lvalue(lhs);
                    if self.current_block_is_terminated() {
                        return self.null_ptr();
                    }
                    let zero = self.context.i64_type().const_zero();
                    let array_llvm_ty = self.get_llvm_type(lhs.ty);
                    unsafe {
                        self.builder
                            .build_gep(array_llvm_ty, array_ptr, &[zero, idx_val], "array_lvalue")
                            .unwrap()
                    }
                }
            }
            MastExprKind::Deref(operand) => {
                let ptr_raw = self.compile_expr(operand);
                if self.current_block_is_terminated() {
                    return self.null_ptr();
                }
                ptr_raw.into_pointer_value()
            }

            // Materialize pure rvalues into temporary stack storage whenever an lvalue address is required.
            _ => {
                let rval = self.compile_expr(expr);
                if self.current_block_is_terminated() {
                    return self.null_ptr();
                }
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
        let ptr_raw = self.compile_expr(operand);
        if let Some(fallback) = self.expr_terminated_fallback(expected_ty) {
            return fallback;
        }
        let ptr_val = ptr_raw.into_pointer_value();
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
        let is_union = self.union_ids.contains(&struct_id);
        if !is_union && !Self::expr_is_addressable(lhs) {
            let lhs_val = self.compile_expr(lhs);
            if let Some(fallback) = self.expr_terminated_fallback(expected_ty) {
                return fallback;
            }
            return self
                .builder
                .build_extract_value(
                    lhs_val.into_struct_value(),
                    field_idx as u32,
                    "field_extract",
                )
                .unwrap();
        }

        let struct_ptr = self.compile_lvalue(lhs);
        if let Some(fallback) = self.expr_terminated_fallback(expected_ty) {
            return fallback;
        }
        let Some(struct_llvm_ty) = self.lookup_struct_type(struct_id, lhs.span, "field access")
        else {
            return expected_ty.const_zero();
        };

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
        let idx_raw = self.compile_expr(index);
        if let Some(fallback) = self.expr_terminated_fallback(expected_ty) {
            return fallback;
        }
        let idx_val = idx_raw.into_int_value();
        let norm_lhs = self.type_registry.normalize(lhs.ty);

        if self.type_registry.is_simd(norm_lhs) {
            let vector_val = self.compile_expr(lhs);
            if let Some(fallback) = self.expr_terminated_fallback(expected_ty) {
                return fallback;
            }
            return self
                .builder
                .build_extract_element(vector_val.into_vector_value(), idx_val, "simd_lane")
                .unwrap();
        }

        let elem_ptr = if let TypeKind::Slice { .. } = self.type_registry.get(norm_lhs) {
            let slice_raw = self.compile_expr(lhs);
            if let Some(fallback) = self.expr_terminated_fallback(expected_ty) {
                return fallback;
            }
            let slice_val = slice_raw.into_struct_value();
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
            let ptr_raw = self.compile_expr(lhs);
            if let Some(fallback) = self.expr_terminated_fallback(expected_ty) {
                return fallback;
            }
            let ptr_val = ptr_raw.into_pointer_value();
            let elem_ty = self.get_llvm_type(expr_ty);
            unsafe {
                self.builder
                    .build_gep(elem_ty, ptr_val, &[idx_val], "ptr_idx")
                    .unwrap()
            }
        } else {
            if !Self::expr_is_addressable(lhs)
                && let MastExprKind::Integer(idx) = index.kind
            {
                let lhs_val = self.compile_expr(lhs);
                if let Some(fallback) = self.expr_terminated_fallback(expected_ty) {
                    return fallback;
                }
                let array_val = lhs_val.into_array_value();
                let array_ty = array_val.get_type();
                if (idx as u32) < array_ty.len() {
                    return self
                        .builder
                        .build_extract_value(array_val, idx as u32, "array_extract")
                        .unwrap();
                }
            }

            let array_ptr = self.compile_lvalue(lhs);
            if let Some(fallback) = self.expr_terminated_fallback(expected_ty) {
                return fallback;
            }
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

    /// Lower slice construction `[start..end]` to the underlying LLVM fat-pointer form.
    pub(crate) fn compile_slice_op(
        &mut self,
        lhs: &MastExpr,
        start: Option<&MastExpr>,
        end: Option<&MastExpr>,
        is_inclusive: bool,
        expected_llvm_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let lhs_val = self.compile_expr(lhs);
        if let Some(fallback) = self.expr_terminated_fallback(expected_llvm_ty) {
            return fallback;
        }
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

        // 2. Compute `start`, defaulting to zero.
        let start_val = if let Some(s) = start {
            let start_raw = self.compile_expr(s);
            if let Some(fallback) = self.expr_terminated_fallback(expected_llvm_ty) {
                return fallback;
            }
            start_raw.into_int_value()
        } else {
            self.context.i64_type().const_zero()
        };

        // 3. Compute `end`, defaulting to the base length.
        let end_val = if let Some(e) = end {
            let end_raw = self.compile_expr(e);
            if let Some(fallback) = self.expr_terminated_fallback(expected_llvm_ty) {
                return fallback;
            }
            end_raw.into_int_value()
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

        // 4. Compute the new length: `end - start + 1` when inclusive.
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

        // 5. Offset the base pointer by `start`.
        let llvm_elem_ty = self.get_llvm_type(elem_ty);

        let slice_ptr = unsafe {
            self.builder
                .build_gep(llvm_elem_ty, base_ptr, &[start_val], "slice_ptr")
                .unwrap()
        };

        // 6. Assemble and return the new fat-pointer struct.
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
