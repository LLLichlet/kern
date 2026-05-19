use super::*;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    fn insert_simd_lane_value(
        &mut self,
        vector: BasicValueEnum<'ctx>,
        lane_val: BasicValueEnum<'ctx>,
        lane_idx: IntValue<'ctx>,
        span: Span,
        context: &str,
    ) -> Option<BasicValueEnum<'ctx>> {
        let vector = self.expect_vector_value(vector, span, context)?;
        Some(
            self.builder
                .build_insert_element(vector, lane_val, lane_idx, context)
                .unwrap(),
        )
    }

    pub(super) fn compile_mir_simd_load(
        &mut self,
        body: &MirBody,
        ptr: &MirOperand,
        result_ty: TypeId,
        align: u32,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let ptr_val = self.compile_mir_operand(body, ptr);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let Some(ptr_val) =
            self.expect_pointer_value(ptr_val, Span::default(), "MIR SIMD load pointer")
        else {
            return self.get_undef_val(result_llvm_ty);
        };
        let loaded = self
            .builder
            .build_load(result_llvm_ty, ptr_val, "mir_simd_load")
            .unwrap();
        if let Some(inst) = loaded.as_instruction_value() {
            inst.set_alignment(align);
        }
        loaded
    }

    pub(super) fn compile_mir_simd_store(
        &mut self,
        body: &MirBody,
        ptr: &MirOperand,
        value: &MirOperand,
        align: u32,
    ) {
        let ptr_val = self.compile_mir_operand(body, ptr);
        if self.current_block_is_terminated() {
            return;
        }
        let Some(ptr_val) =
            self.expect_pointer_value(ptr_val, Span::default(), "MIR SIMD store pointer")
        else {
            return;
        };
        let value_val = self.compile_mir_operand(body, value);
        if self.current_block_is_terminated() {
            return;
        }
        let store = self.builder.build_store(ptr_val, value_val).unwrap();
        store.set_alignment(align);
    }

    pub(super) fn compile_mir_simd_masked_load(
        &mut self,
        body: &MirBody,
        ptr: &MirOperand,
        mask: &MirOperand,
        or_else: &MirOperand,
        result_ty: TypeId,
        _align: u32,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let ptr_val = self.compile_mir_operand(body, ptr);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let mask_val = self.compile_mir_operand(body, mask);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let fallback_val = self.compile_mir_operand(body, or_else);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }

        let Some((elem_ty, lanes)) = self.simd_elem_and_lanes(result_ty) else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR SIMD masked load expected a SIMD result type.",
            );
            return self.get_undef_val(result_llvm_ty);
        };
        let Some(func) = self.current_function_for_simd_memory("MIR SIMD masked load") else {
            return self.get_undef_val(result_llvm_ty);
        };

        let result_ptr = self.create_entry_block_alloca(result_llvm_ty, "mir_simd_masked_load_tmp");
        self.builder.build_store(result_ptr, fallback_val).unwrap();

        let Some(base_ptr) =
            self.expect_pointer_value(ptr_val, Span::default(), "MIR SIMD masked load pointer")
        else {
            return self.get_undef_val(result_llvm_ty);
        };
        let Some(mask_vec) =
            self.expect_vector_value(mask_val, Span::default(), "MIR SIMD masked load mask")
        else {
            return self.get_undef_val(result_llvm_ty);
        };
        let elem_llvm_ty = self.get_llvm_type(elem_ty);
        for lane in 0..lanes {
            let lane_idx = self.context.i32_type().const_int(lane as u64, false);
            let lane_mask_value = self
                .builder
                .build_extract_element(mask_vec, lane_idx, "mir_simd_masked_load_mask")
                .unwrap();
            let Some(lane_mask) = self.expect_int_value(
                lane_mask_value,
                Span::default(),
                "MIR SIMD masked load mask lane",
            ) else {
                return self.get_undef_val(result_llvm_ty);
            };
            let then_bb = self
                .context
                .append_basic_block(func, "mir_simd_masked_load.then");
            let cont_bb = self
                .context
                .append_basic_block(func, "mir_simd_masked_load.cont");
            self.builder
                .build_conditional_branch(lane_mask, then_bb, cont_bb)
                .unwrap();
            self.builder.position_at_end(then_bb);
            let lane_offset = self.context.i64_type().const_int(lane as u64, false);
            // SAFETY: `base_ptr` points to contiguous SIMD elements of
            // `elem_llvm_ty`; `lane_offset` is bounded by the vector lane count.
            let lane_ptr = unsafe {
                self.builder
                    .build_gep(
                        elem_llvm_ty,
                        base_ptr,
                        &[lane_offset],
                        "mir_simd_masked_load_ptr",
                    )
                    .unwrap()
            };
            let lane_val = self
                .builder
                .build_load(elem_llvm_ty, lane_ptr, "mir_simd_masked_load_lane")
                .unwrap();
            let current_vector = self
                .builder
                .build_load(result_llvm_ty, result_ptr, "mir_simd_masked_load_cur")
                .unwrap();
            let Some(updated_vector) = self.insert_simd_lane_value(
                current_vector,
                lane_val,
                lane_idx,
                Span::default(),
                "mir_simd_masked_load_insert",
            ) else {
                return self.get_undef_val(result_llvm_ty);
            };
            self.builder
                .build_store(result_ptr, updated_vector)
                .unwrap();
            self.builder.build_unconditional_branch(cont_bb).unwrap();
            self.builder.position_at_end(cont_bb);
        }

        self.builder
            .build_load(result_llvm_ty, result_ptr, "mir_simd_masked_load_result")
            .unwrap()
    }

    pub(super) fn compile_mir_simd_masked_store(
        &mut self,
        body: &MirBody,
        ptr: &MirOperand,
        mask: &MirOperand,
        value: &MirOperand,
        _align: u32,
    ) {
        let ptr_val = self.compile_mir_operand(body, ptr);
        if self.current_block_is_terminated() {
            return;
        }
        let mask_val = self.compile_mir_operand(body, mask);
        if self.current_block_is_terminated() {
            return;
        }
        let vector_val = self.compile_mir_operand(body, value);
        if self.current_block_is_terminated() {
            return;
        }

        let value_ty = self.mir_operand_ty(body, value).unwrap_or(TypeId::ERROR);
        let Some((elem_ty, lanes)) = self.simd_elem_and_lanes(value_ty) else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR SIMD masked store expected a SIMD value operand.",
            );
            return;
        };
        let Some(func) = self.current_function_for_simd_memory("MIR SIMD masked store") else {
            return;
        };

        let Some(base_ptr) =
            self.expect_pointer_value(ptr_val, Span::default(), "MIR SIMD masked store pointer")
        else {
            return;
        };
        let Some(mask_vec) =
            self.expect_vector_value(mask_val, Span::default(), "MIR SIMD masked store mask")
        else {
            return;
        };
        let Some(value_vec) =
            self.expect_vector_value(vector_val, Span::default(), "MIR SIMD masked store value")
        else {
            return;
        };
        let elem_llvm_ty = self.get_llvm_type(elem_ty);
        for lane in 0..lanes {
            let lane_idx = self.context.i32_type().const_int(lane as u64, false);
            let lane_mask_value = self
                .builder
                .build_extract_element(mask_vec, lane_idx, "mir_simd_masked_store_mask")
                .unwrap();
            let Some(lane_mask) = self.expect_int_value(
                lane_mask_value,
                Span::default(),
                "MIR SIMD masked store mask lane",
            ) else {
                return;
            };
            let then_bb = self
                .context
                .append_basic_block(func, "mir_simd_masked_store.then");
            let cont_bb = self
                .context
                .append_basic_block(func, "mir_simd_masked_store.cont");
            self.builder
                .build_conditional_branch(lane_mask, then_bb, cont_bb)
                .unwrap();
            self.builder.position_at_end(then_bb);
            let lane_offset = self.context.i64_type().const_int(lane as u64, false);
            // SAFETY: `base_ptr` points to contiguous SIMD elements of
            // `elem_llvm_ty`; `lane_offset` is bounded by the vector lane count.
            let lane_ptr = unsafe {
                self.builder
                    .build_gep(
                        elem_llvm_ty,
                        base_ptr,
                        &[lane_offset],
                        "mir_simd_masked_store_ptr",
                    )
                    .unwrap()
            };
            let lane_val = self
                .builder
                .build_extract_element(value_vec, lane_idx, "mir_simd_masked_store_lane")
                .unwrap();
            self.builder.build_store(lane_ptr, lane_val).unwrap();
            self.builder.build_unconditional_branch(cont_bb).unwrap();
            self.builder.position_at_end(cont_bb);
        }
    }

    pub(super) fn compile_mir_simd_gather(
        &mut self,
        body: &MirBody,
        ptr: &MirOperand,
        indices: &MirOperand,
        result_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let base_ptr = self.compile_mir_operand(body, ptr);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let indices_ptr = self.compile_mir_operand(body, indices);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let Some((elem_ty, lanes)) = self.simd_elem_and_lanes(result_ty) else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR SIMD gather expected a SIMD result type.",
            );
            return self.get_undef_val(result_llvm_ty);
        };
        let mut result = match result_llvm_ty {
            BasicTypeEnum::VectorType(vector_ty) => vector_ty.const_zero().into_vector_value(),
            other => {
                self.sess.emit_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Codegen): MIR SIMD gather expected a vector LLVM type, found `{:?}`.",
                        other
                    ),
                );
                return self.get_undef_val(result_llvm_ty);
            }
        };
        let elem_llvm_ty = self.get_llvm_type(elem_ty);
        let usize_llvm_ty = self.get_llvm_type(TypeId::USIZE);
        let Some(indices_ptr) =
            self.expect_pointer_value(indices_ptr, Span::default(), "MIR SIMD gather indices")
        else {
            return self.get_undef_val(result_llvm_ty);
        };
        let Some(base_ptr) =
            self.expect_pointer_value(base_ptr, Span::default(), "MIR SIMD gather pointer")
        else {
            return self.get_undef_val(result_llvm_ty);
        };
        for lane in 0..lanes {
            let lane_offset = self.context.i64_type().const_int(lane as u64, false);
            // SAFETY: `indices_ptr` points to the contiguous usize index array
            // consumed by this gather; `lane_offset` is bounded by `lanes`.
            let lane_index_ptr = unsafe {
                self.builder
                    .build_gep(
                        usize_llvm_ty,
                        indices_ptr,
                        &[lane_offset],
                        "mir_simd_gather_idx_ptr",
                    )
                    .unwrap()
            };
            let gathered_index_value = self
                .builder
                .build_load(usize_llvm_ty, lane_index_ptr, "mir_simd_gather_idx")
                .unwrap();
            let Some(gathered_index) = self.expect_int_value(
                gathered_index_value,
                Span::default(),
                "MIR SIMD gather index lane",
            ) else {
                return self.get_undef_val(result_llvm_ty);
            };
            // SAFETY: `base_ptr` points to elements of `elem_llvm_ty`; the
            // gathered index value is the MIR-provided element index.
            let lane_ptr = unsafe {
                self.builder
                    .build_gep(
                        elem_llvm_ty,
                        base_ptr,
                        &[gathered_index],
                        "mir_simd_gather_lane_ptr",
                    )
                    .unwrap()
            };
            let lane_val = self
                .builder
                .build_load(elem_llvm_ty, lane_ptr, "mir_simd_gather_lane")
                .unwrap();
            let lane_index = self.context.i32_type().const_int(lane as u64, false);
            let result_value = self
                .builder
                .build_insert_element(result, lane_val, lane_index, "mir_simd_gather_insert")
                .unwrap();
            let Some(next_result) = self.expect_vector_value(
                result_value,
                Span::default(),
                "MIR SIMD gather result vector",
            ) else {
                return self.get_undef_val(result_llvm_ty);
            };
            result = next_result;
        }
        result.into()
    }

    pub(super) fn compile_mir_simd_scatter(
        &mut self,
        body: &MirBody,
        ptr: &MirOperand,
        indices: &MirOperand,
        value: &MirOperand,
    ) {
        let base_ptr = self.compile_mir_operand(body, ptr);
        if self.current_block_is_terminated() {
            return;
        }
        let indices_ptr = self.compile_mir_operand(body, indices);
        if self.current_block_is_terminated() {
            return;
        }
        let vector_val = self.compile_mir_operand(body, value);
        if self.current_block_is_terminated() {
            return;
        }
        let value_ty = self.mir_operand_ty(body, value).unwrap_or(TypeId::ERROR);
        let Some((elem_ty, lanes)) = self.simd_elem_and_lanes(value_ty) else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR SIMD scatter expected a SIMD value operand.",
            );
            return;
        };
        let elem_llvm_ty = self.get_llvm_type(elem_ty);
        let usize_llvm_ty = self.get_llvm_type(TypeId::USIZE);
        let Some(indices_ptr) =
            self.expect_pointer_value(indices_ptr, Span::default(), "MIR SIMD scatter indices")
        else {
            return;
        };
        let Some(base_ptr) =
            self.expect_pointer_value(base_ptr, Span::default(), "MIR SIMD scatter pointer")
        else {
            return;
        };
        let Some(vector_val) =
            self.expect_vector_value(vector_val, Span::default(), "MIR SIMD scatter value")
        else {
            return;
        };
        for lane in 0..lanes {
            let lane_offset = self.context.i64_type().const_int(lane as u64, false);
            // SAFETY: `indices_ptr` points to the contiguous usize index array
            // consumed by this scatter; `lane_offset` is bounded by `lanes`.
            let lane_index_ptr = unsafe {
                self.builder
                    .build_gep(
                        usize_llvm_ty,
                        indices_ptr,
                        &[lane_offset],
                        "mir_simd_scatter_idx_ptr",
                    )
                    .unwrap()
            };
            let scattered_index_value = self
                .builder
                .build_load(usize_llvm_ty, lane_index_ptr, "mir_simd_scatter_idx")
                .unwrap();
            let Some(scattered_index) = self.expect_int_value(
                scattered_index_value,
                Span::default(),
                "MIR SIMD scatter index lane",
            ) else {
                return;
            };
            // SAFETY: `base_ptr` points to elements of `elem_llvm_ty`; the
            // scattered index value is the MIR-provided element index.
            let lane_ptr = unsafe {
                self.builder
                    .build_gep(
                        elem_llvm_ty,
                        base_ptr,
                        &[scattered_index],
                        "mir_simd_scatter_lane_ptr",
                    )
                    .unwrap()
            };
            let lane_index = self.context.i32_type().const_int(lane as u64, false);
            let lane_val = self
                .builder
                .build_extract_element(vector_val, lane_index, "mir_simd_scatter_lane")
                .unwrap();
            self.builder.build_store(lane_ptr, lane_val).unwrap();
        }
    }

    pub(super) fn compile_mir_simd_masked_gather(
        &mut self,
        body: &MirBody,
        ptr: &MirOperand,
        indices: &MirOperand,
        mask: &MirOperand,
        or_else: &MirOperand,
        result_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let result_llvm_ty = self.get_llvm_type(result_ty);
        let base_ptr = self.compile_mir_operand(body, ptr);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let indices_ptr = self.compile_mir_operand(body, indices);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let mask_val = self.compile_mir_operand(body, mask);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let fallback_val = self.compile_mir_operand(body, or_else);
        if self.current_block_is_terminated() {
            return self.get_undef_val(result_llvm_ty);
        }
        let Some((elem_ty, lanes)) = self.simd_elem_and_lanes(result_ty) else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR SIMD masked gather expected a SIMD result type.",
            );
            return self.get_undef_val(result_llvm_ty);
        };
        let Some(func) = self.current_function_for_simd_memory("MIR SIMD masked gather") else {
            return self.get_undef_val(result_llvm_ty);
        };
        let result_ptr =
            self.create_entry_block_alloca(result_llvm_ty, "mir_simd_masked_gather_tmp");
        self.builder.build_store(result_ptr, fallback_val).unwrap();
        let elem_llvm_ty = self.get_llvm_type(elem_ty);
        let usize_llvm_ty = self.get_llvm_type(TypeId::USIZE);
        let Some(base_ptr) =
            self.expect_pointer_value(base_ptr, Span::default(), "MIR SIMD masked gather pointer")
        else {
            return self.get_undef_val(result_llvm_ty);
        };
        let Some(indices_ptr) = self.expect_pointer_value(
            indices_ptr,
            Span::default(),
            "MIR SIMD masked gather indices",
        ) else {
            return self.get_undef_val(result_llvm_ty);
        };
        let Some(mask_vec) =
            self.expect_vector_value(mask_val, Span::default(), "MIR SIMD masked gather mask")
        else {
            return self.get_undef_val(result_llvm_ty);
        };
        for lane in 0..lanes {
            let lane_idx = self.context.i32_type().const_int(lane as u64, false);
            let lane_mask_value = self
                .builder
                .build_extract_element(mask_vec, lane_idx, "mir_simd_masked_gather_mask")
                .unwrap();
            let Some(lane_mask) = self.expect_int_value(
                lane_mask_value,
                Span::default(),
                "MIR SIMD masked gather mask lane",
            ) else {
                return self.get_undef_val(result_llvm_ty);
            };
            let then_bb = self
                .context
                .append_basic_block(func, "mir_simd_masked_gather.then");
            let cont_bb = self
                .context
                .append_basic_block(func, "mir_simd_masked_gather.cont");
            self.builder
                .build_conditional_branch(lane_mask, then_bb, cont_bb)
                .unwrap();
            self.builder.position_at_end(then_bb);
            let lane_offset = self.context.i64_type().const_int(lane as u64, false);
            // SAFETY: `indices_ptr` points to the contiguous usize index array
            // consumed by this masked gather; `lane_offset` is bounded by
            // `lanes`.
            let lane_index_ptr = unsafe {
                self.builder
                    .build_gep(
                        usize_llvm_ty,
                        indices_ptr,
                        &[lane_offset],
                        "mir_simd_masked_gather_idx_ptr",
                    )
                    .unwrap()
            };
            let gathered_index_value = self
                .builder
                .build_load(usize_llvm_ty, lane_index_ptr, "mir_simd_masked_gather_idx")
                .unwrap();
            let Some(gathered_index) = self.expect_int_value(
                gathered_index_value,
                Span::default(),
                "MIR SIMD masked gather index lane",
            ) else {
                return self.get_undef_val(result_llvm_ty);
            };
            // SAFETY: `base_ptr` points to elements of `elem_llvm_ty`; the
            // gathered index value is the MIR-provided element index.
            let lane_ptr = unsafe {
                self.builder
                    .build_gep(
                        elem_llvm_ty,
                        base_ptr,
                        &[gathered_index],
                        "mir_simd_masked_gather_lane_ptr",
                    )
                    .unwrap()
            };
            let lane_val = self
                .builder
                .build_load(elem_llvm_ty, lane_ptr, "mir_simd_masked_gather_lane")
                .unwrap();
            let current_vector = self
                .builder
                .build_load(result_llvm_ty, result_ptr, "mir_simd_masked_gather_cur")
                .unwrap();
            let Some(updated_vector) = self.insert_simd_lane_value(
                current_vector,
                lane_val,
                lane_idx,
                Span::default(),
                "mir_simd_masked_gather_insert",
            ) else {
                return self.get_undef_val(result_llvm_ty);
            };
            self.builder
                .build_store(result_ptr, updated_vector)
                .unwrap();
            self.builder.build_unconditional_branch(cont_bb).unwrap();
            self.builder.position_at_end(cont_bb);
        }
        self.builder
            .build_load(result_llvm_ty, result_ptr, "mir_simd_masked_gather_result")
            .unwrap()
    }

    pub(super) fn compile_mir_simd_masked_scatter(
        &mut self,
        body: &MirBody,
        ptr: &MirOperand,
        indices: &MirOperand,
        mask: &MirOperand,
        value: &MirOperand,
    ) {
        let base_ptr = self.compile_mir_operand(body, ptr);
        if self.current_block_is_terminated() {
            return;
        }
        let indices_ptr = self.compile_mir_operand(body, indices);
        if self.current_block_is_terminated() {
            return;
        }
        let mask_val = self.compile_mir_operand(body, mask);
        if self.current_block_is_terminated() {
            return;
        }
        let vector_val = self.compile_mir_operand(body, value);
        if self.current_block_is_terminated() {
            return;
        }
        let value_ty = self.mir_operand_ty(body, value).unwrap_or(TypeId::ERROR);
        let Some((elem_ty, lanes)) = self.simd_elem_and_lanes(value_ty) else {
            self.sess.emit_ice(
                Span::default(),
                "Kern ICE (Codegen): MIR SIMD masked scatter expected a SIMD value operand.",
            );
            return;
        };
        let Some(func) = self.current_function_for_simd_memory("MIR SIMD masked scatter") else {
            return;
        };
        let elem_llvm_ty = self.get_llvm_type(elem_ty);
        let usize_llvm_ty = self.get_llvm_type(TypeId::USIZE);
        let Some(indices_ptr) = self.expect_pointer_value(
            indices_ptr,
            Span::default(),
            "MIR SIMD masked scatter indices",
        ) else {
            return;
        };
        let Some(base_ptr) =
            self.expect_pointer_value(base_ptr, Span::default(), "MIR SIMD masked scatter pointer")
        else {
            return;
        };
        let Some(mask_vec) =
            self.expect_vector_value(mask_val, Span::default(), "MIR SIMD masked scatter mask")
        else {
            return;
        };
        let Some(vector_val) =
            self.expect_vector_value(vector_val, Span::default(), "MIR SIMD masked scatter value")
        else {
            return;
        };
        for lane in 0..lanes {
            let lane_idx = self.context.i32_type().const_int(lane as u64, false);
            let lane_mask_value = self
                .builder
                .build_extract_element(mask_vec, lane_idx, "mir_simd_masked_scatter_mask")
                .unwrap();
            let Some(lane_mask) = self.expect_int_value(
                lane_mask_value,
                Span::default(),
                "MIR SIMD masked scatter mask lane",
            ) else {
                return;
            };
            let then_bb = self
                .context
                .append_basic_block(func, "mir_simd_masked_scatter.then");
            let cont_bb = self
                .context
                .append_basic_block(func, "mir_simd_masked_scatter.cont");
            self.builder
                .build_conditional_branch(lane_mask, then_bb, cont_bb)
                .unwrap();
            self.builder.position_at_end(then_bb);
            let lane_offset = self.context.i64_type().const_int(lane as u64, false);
            // SAFETY: `indices_ptr` points to the contiguous usize index array
            // consumed by this masked scatter; `lane_offset` is bounded by
            // `lanes`.
            let lane_index_ptr = unsafe {
                self.builder
                    .build_gep(
                        usize_llvm_ty,
                        indices_ptr,
                        &[lane_offset],
                        "mir_simd_masked_scatter_idx_ptr",
                    )
                    .unwrap()
            };
            let scattered_index_value = self
                .builder
                .build_load(usize_llvm_ty, lane_index_ptr, "mir_simd_masked_scatter_idx")
                .unwrap();
            let Some(scattered_index) = self.expect_int_value(
                scattered_index_value,
                Span::default(),
                "MIR SIMD masked scatter index lane",
            ) else {
                return;
            };
            // SAFETY: `base_ptr` points to elements of `elem_llvm_ty`; the
            // scattered index value is the MIR-provided element index.
            let lane_ptr = unsafe {
                self.builder
                    .build_gep(
                        elem_llvm_ty,
                        base_ptr,
                        &[scattered_index],
                        "mir_simd_masked_scatter_lane_ptr",
                    )
                    .unwrap()
            };
            let lane_val = self
                .builder
                .build_extract_element(vector_val, lane_idx, "mir_simd_masked_scatter_lane")
                .unwrap();
            self.builder.build_store(lane_ptr, lane_val).unwrap();
            self.builder.build_unconditional_branch(cont_bb).unwrap();
            self.builder.position_at_end(cont_bb);
        }
    }
}
