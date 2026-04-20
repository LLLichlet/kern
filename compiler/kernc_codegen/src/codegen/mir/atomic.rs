use super::*;

pub(super) struct AtomicCasArgs<'a> {
    pub(super) result_ty: TypeId,
    pub(super) weak: bool,
    pub(super) ptr: &'a MirOperand,
    pub(super) expected: &'a MirOperand,
    pub(super) desired: &'a MirOperand,
    pub(super) success: AtomicOrdering,
    pub(super) failure: AtomicOrdering,
}

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub(super) fn compile_mir_slice_op(
        &mut self,
        body: &MirBody,
        lhs: &MirSliceBase,
        start: Option<&MirOperand>,
        end: Option<&MirOperand>,
        is_inclusive: bool,
        result_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let Some((base_ptr, base_len, elem_ty)) =
            self.compile_mir_slice_base_parts(body, lhs, Span::default())
        else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): invalid MIR slice base type.",
            );
            return result_llvm_ty.const_zero();
        };
        let start_val = if let Some(start) = start {
            self.compile_mir_index_operand(body, start)
        } else {
            self.context.i64_type().const_zero()
        };
        if self.current_block_is_terminated() {
            return result_llvm_ty.const_zero();
        }
        let end_val = if let Some(end) = end {
            self.compile_mir_index_operand(body, end)
        } else if let Some(base_len) = base_len {
            base_len
        } else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): slicing a raw pointer requires an explicit end index.",
            );
            return result_llvm_ty.const_zero();
        };
        if self.current_block_is_terminated() {
            return result_llvm_ty.const_zero();
        }
        let mut slice_len = self
            .builder
            .build_int_sub(end_val, start_val, "mir_slice_len")
            .unwrap();
        if is_inclusive {
            let one = self.context.i64_type().const_int(1, false);
            slice_len = self
                .builder
                .build_int_add(slice_len, one, "mir_slice_len_inc")
                .unwrap();
        }
        let llvm_elem_ty = self.get_llvm_type(elem_ty);
        let slice_ptr = unsafe {
            self.builder
                .build_gep(llvm_elem_ty, base_ptr, &[start_val], "mir_slice_ptr")
                .unwrap()
        };
        let struct_ty = result_llvm_ty.into_struct_type();
        let mut slice_struct = struct_ty.get_undef();
        slice_struct = self
            .builder
            .build_insert_value(slice_struct, slice_ptr, 0, "mir_slice_insert_ptr")
            .unwrap()
            .into_struct_value();
        slice_struct = self
            .builder
            .build_insert_value(slice_struct, slice_len, 1, "mir_slice_insert_len")
            .unwrap()
            .into_struct_value();
        slice_struct.into()
    }

    pub(super) fn compile_mir_atomic_fence(&mut self, ordering: AtomicOrdering) {
        self.builder
            .build_fence(Self::llvm_atomic_ordering(ordering), 0, "")
            .unwrap();
    }

    pub(super) fn compile_mir_atomic_rmw(
        &mut self,
        body: &MirBody,
        result_ty: TypeId,
        op: AtomicRmwOp,
        ptr: &MirOperand,
        value: &MirOperand,
        ordering: AtomicOrdering,
    ) -> BasicValueEnum<'ctx> {
        let llvm_ty = self.get_llvm_type(result_ty);
        let llvm_order = Self::llvm_atomic_ordering(ordering);
        let ptr_val = self.compile_mir_operand(body, ptr).into_pointer_value();
        if self.current_block_is_terminated() {
            return self.get_undef_val(llvm_ty);
        }
        let value_val = self.compile_mir_operand(body, value);
        if self.current_block_is_terminated() {
            return self.get_undef_val(llvm_ty);
        }

        if matches!(
            self.type_registry
                .get(self.type_registry.normalize(result_ty)),
            TypeKind::Pointer { .. }
        ) && op == AtomicRmwOp::Xchg
        {
            let ptr_int_ty = self.atomic_xchg_pointer_width_int();
            let int_ptr_ty = self.context.ptr_type(crate::AddressSpace::default());
            let cast_ptr = self
                .builder
                .build_pointer_cast(ptr_val, int_ptr_ty, "mir_atomic_xchg_ptr_cast")
                .unwrap();
            let result_ptr_ty = self.get_llvm_type(result_ty).into_pointer_type();
            let cast_val = self
                .builder
                .build_ptr_to_int(
                    value_val.into_pointer_value(),
                    ptr_int_ty,
                    "mir_atomic_xchg_val",
                )
                .unwrap();
            let old_val = self
                .builder
                .build_atomicrmw(crate::AtomicRMWBinOp::Xchg, cast_ptr, cast_val, llvm_order)
                .unwrap();
            return self
                .builder
                .build_int_to_ptr(old_val, result_ptr_ty, "mir_atomic_xchg_old_ptr")
                .unwrap()
                .into();
        }

        let llvm_op = match op {
            AtomicRmwOp::Xchg => crate::AtomicRMWBinOp::Xchg,
            AtomicRmwOp::Add => crate::AtomicRMWBinOp::Add,
            AtomicRmwOp::Sub => crate::AtomicRMWBinOp::Sub,
            AtomicRmwOp::And => crate::AtomicRMWBinOp::And,
            AtomicRmwOp::Nand => crate::AtomicRMWBinOp::Nand,
            AtomicRmwOp::Or => crate::AtomicRMWBinOp::Or,
            AtomicRmwOp::Xor => crate::AtomicRMWBinOp::Xor,
            AtomicRmwOp::Max => crate::AtomicRMWBinOp::Max,
            AtomicRmwOp::Min => crate::AtomicRMWBinOp::Min,
            AtomicRmwOp::UMax => crate::AtomicRMWBinOp::UMax,
            AtomicRmwOp::UMin => crate::AtomicRMWBinOp::UMin,
        };

        self.builder
            .build_atomicrmw(llvm_op, ptr_val, value_val.into_int_value(), llvm_order)
            .unwrap()
            .into()
    }

    pub(super) fn compile_mir_atomic_cas(
        &mut self,
        body: &MirBody,
        cas: AtomicCasArgs<'_>,
    ) -> BasicValueEnum<'ctx> {
        let llvm_ty = self.get_llvm_type(cas.result_ty);
        let ptr_val = self.compile_mir_operand(body, cas.ptr).into_pointer_value();
        if self.current_block_is_terminated() {
            return self.get_undef_val(llvm_ty);
        }
        let expected_val = self.compile_mir_operand(body, cas.expected);
        if self.current_block_is_terminated() {
            return self.get_undef_val(llvm_ty);
        }
        let desired_val = self.compile_mir_operand(body, cas.desired);
        if self.current_block_is_terminated() {
            return self.get_undef_val(llvm_ty);
        }
        let cas_pair = self
            .builder
            .build_cmpxchg(
                ptr_val,
                expected_val,
                desired_val,
                Self::llvm_atomic_ordering(cas.success),
                Self::llvm_atomic_ordering(cas.failure),
            )
            .unwrap();
        if cas.weak {
            let Some(cas_inst) = cas_pair.as_instruction() else {
                self.sess.emit_ice(
                    Span::default(),
                    "Kern ICE (Codegen): MIR cmpxchg did not lower to an instruction value.",
                );
                return self.get_undef_val(llvm_ty);
            };
            unsafe { LLVMSetWeak(cas_inst.as_value_ref(), 1) };
        }

        let old_val = self
            .builder
            .build_extract_value(cas_pair, 0, "mir_cas_old")
            .unwrap();
        let success_val = self
            .builder
            .build_extract_value(cas_pair, 1, "mir_cas_success")
            .unwrap();

        let norm_ty = self.type_registry.normalize(cas.result_ty);
        let Some(&struct_id) = self.anon_struct_map.get(&norm_ty) else {
            self.sess.emit_ice(
                Span::default(),
                format!(
                    "Kern ICE (Codegen): MIR cmpxchg result type `{:?}` was not instantiated as an anonymous struct.",
                    norm_ty
                ),
            );
            return self.get_undef_val(llvm_ty);
        };

        let struct_ty = self.get_llvm_type(cas.result_ty).into_struct_type();
        let mut result = struct_ty.const_zero();
        if let Some(idx) = self.struct_field_index_by_name(struct_id, "success") {
            result = self
                .builder
                .build_insert_value(result, success_val, idx, "mir_cas_insert_success")
                .unwrap()
                .into_struct_value();
        }
        if let Some(idx) = self.struct_field_index_by_name(struct_id, "value") {
            result = self
                .builder
                .build_insert_value(result, old_val, idx, "mir_cas_insert_value")
                .unwrap()
                .into_struct_value();
        }
        result.into()
    }
}
