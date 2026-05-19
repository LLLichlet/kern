//! MIR place address/value code generation.
//!
//! Place lowering computes LLVM addresses for locals, globals, dereferences,
//! fields, indices, slices, and enum payloads, then emits stores/loads with the
//! correct volatility and aggregate update behavior.

use super::*;
use crate::module::Linkage;
use crate::types::IntType;
use crate::values::IntValue;
use kernc_ast::BinaryOperator;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    fn assignment_binary_operator(op: AssignmentOperator) -> Option<BinaryOperator> {
        match op {
            AssignmentOperator::AddAssign => Some(BinaryOperator::Add),
            AssignmentOperator::SubtractAssign => Some(BinaryOperator::Subtract),
            AssignmentOperator::MultiplyAssign => Some(BinaryOperator::Multiply),
            AssignmentOperator::DivideAssign => Some(BinaryOperator::Divide),
            AssignmentOperator::ModuloAssign => Some(BinaryOperator::Modulo),
            AssignmentOperator::BitwiseAndAssign => Some(BinaryOperator::BitwiseAnd),
            AssignmentOperator::BitwiseOrAssign => Some(BinaryOperator::BitwiseOr),
            AssignmentOperator::BitwiseXorAssign => Some(BinaryOperator::BitwiseXor),
            AssignmentOperator::ShiftLeftAssign => Some(BinaryOperator::ShiftLeft),
            AssignmentOperator::ShiftRightAssign => Some(BinaryOperator::ShiftRight),
            AssignmentOperator::Assign => None,
        }
    }

    pub(super) fn lookup_mir_local_ptr(
        &mut self,
        id: MirLocalId,
        body: &MirBody,
    ) -> Option<PointerValue<'ctx>> {
        match self.mir_locals.get(&id).copied() {
            Some(ptr) => Some(ptr),
            None => {
                let local_name = body
                    .locals
                    .get(id.0 as usize)
                    .map(|local| self.resolve_symbol(local.name).to_string())
                    .unwrap_or_else(|| format!("local#{}", id.0));
                self.sess.emit_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Codegen): MIR local `{}` ({:?}) missing from local storage map.",
                        local_name, id
                    ),
                );
                None
            }
        }
    }

    pub(super) fn mir_operand_ty(&self, body: &MirBody, operand: &MirOperand) -> Option<TypeId> {
        match operand {
            MirOperand::Local(local) => body.locals.get(local.0 as usize).map(|local| local.ty),
            MirOperand::Const(value) => Some(value.ty()),
        }
    }

    fn cast_int_value_to_target_width(
        &mut self,
        value: IntValue<'ctx>,
        target: IntType<'ctx>,
        extend_signed: bool,
    ) -> IntValue<'ctx> {
        let source_width = value.get_type().bit_width();
        let target_width = target.bit_width();

        if source_width == target_width {
            return value;
        }

        if source_width < target_width {
            if extend_signed {
                self.builder
                    .build_int_s_extend(value, target, "mir_idx_sext")
                    .unwrap()
            } else {
                self.builder
                    .build_int_z_extend(value, target, "mir_idx_zext")
                    .unwrap()
            }
        } else {
            self.builder
                .build_int_truncate(value, target, "mir_idx_trunc")
                .unwrap()
        }
    }

    pub(super) fn compile_mir_index_operand(
        &mut self,
        body: &MirBody,
        operand: &MirOperand,
    ) -> IntValue<'ctx> {
        let source_ty = self.mir_operand_ty(body, operand).unwrap_or(TypeId::USIZE);
        let value = self.compile_mir_operand(body, operand);
        let Some(value) = self.expect_int_value(value, Span::default(), "MIR index operand") else {
            return self.context.i64_type().const_zero();
        };
        // GEP indices must use a stable widened integer so signed `i32` indices do not leak
        // uninitialized high bits when a contextual `usize` temp is involved.
        self.cast_int_value_to_target_width(
            value,
            self.context.i64_type(),
            self.is_signed_int(source_ty),
        )
    }

    pub(super) fn cast_mir_int_to_expected_type(
        &mut self,
        value: IntValue<'ctx>,
        target_ty: TypeId,
    ) -> IntValue<'ctx> {
        let target_llvm_ty = self.get_llvm_type(target_ty);
        let Some(target) = self.expect_int_type(
            target_llvm_ty,
            Span::default(),
            "MIR integer contextual cast",
        ) else {
            return value;
        };
        self.cast_int_value_to_target_width(value, target, self.is_signed_int(target_ty))
    }

    pub(super) fn mir_operand_pointee_ty(
        &self,
        body: &MirBody,
        operand: &MirOperand,
    ) -> Option<TypeId> {
        self.mir_operand_ty(body, operand)
            .and_then(|ty| self.type_registry.get_elem_type(ty))
    }

    pub(super) fn mir_place_ty(&self, body: &MirBody, place: &MirPlace) -> Option<TypeId> {
        match place {
            MirPlace::Local(local) => body.locals.get(local.0 as usize).map(|local| local.ty),
            MirPlace::Global(global) => self.global_tys.get(global).copied(),
            MirPlace::Deref(operand) => self
                .mir_operand_ty(body, operand)
                .and_then(|ty| self.type_registry.get_elem_type(ty)),
            MirPlace::Field { field_ty, .. } => Some(*field_ty),
            MirPlace::Index { base, .. } => self
                .mir_place_ty(body, base)
                .and_then(|ty| self.type_registry.get_elem_type(ty)),
        }
    }

    pub(super) fn mir_place_access_is_volatile(&self, body: &MirBody, place: &MirPlace) -> bool {
        match place {
            MirPlace::Local(_) | MirPlace::Global(_) => false,
            MirPlace::Deref(operand) => self.mir_operand_ty(body, operand).is_some_and(|ty| {
                matches!(
                    self.type_registry.get(self.type_registry.normalize(ty)),
                    TypeKind::VolatilePtr { .. }
                )
            }),
            MirPlace::Field { base, .. } | MirPlace::Index { base, .. } => {
                self.mir_place_access_is_volatile(body, base)
            }
        }
    }

    pub(super) fn mir_call_return_ty(
        &self,
        body: &MirBody,
        callee: &MirCallTarget,
        hint: Option<TypeId>,
    ) -> Option<TypeId> {
        match callee {
            MirCallTarget::Direct(id) => self.function_ret_tys.get(id).copied().or(hint),
            MirCallTarget::Operand(operand) => {
                let callee_ty = self
                    .type_registry
                    .normalize(self.mir_operand_ty(body, operand)?);
                match self.type_registry.get(callee_ty) {
                    TypeKind::Function { ret, .. } | TypeKind::ClosureInterface { ret, .. } => {
                        Some(*ret)
                    }
                    _ => hint,
                }
            }
        }
    }

    pub(super) fn mir_rvalue_ty(
        &self,
        body: &MirBody,
        rvalue: &MirRvalue,
        hint: Option<TypeId>,
    ) -> Option<TypeId> {
        match rvalue {
            MirRvalue::Use(operand) => self.mir_operand_ty(body, operand),
            MirRvalue::Call { callee, .. } => self.mir_call_return_ty(body, callee, hint),
            MirRvalue::Aggregate { ty, .. } => Some(*ty),
            MirRvalue::Projection { .. }
            | MirRvalue::AtomicCas { .. }
            | MirRvalue::AddressOf(_)
            | MirRvalue::Load(_) => hint,
            MirRvalue::Cast { target_ty, .. } => Some(*target_ty),
            MirRvalue::Unary { operand, .. } => self.mir_operand_ty(body, operand),
            MirRvalue::BitIntrinsic { operand, .. } => self.mir_operand_ty(body, operand),
            MirRvalue::Binary { op, lhs, .. } => match op {
                BinaryOperator::Equal
                | BinaryOperator::NotEqual
                | BinaryOperator::LessThan
                | BinaryOperator::LessOrEqual
                | BinaryOperator::GreaterThan
                | BinaryOperator::GreaterOrEqual
                | BinaryOperator::LogicalAnd
                | BinaryOperator::LogicalOr => Some(TypeId::BOOL),
                _ => self.mir_operand_ty(body, lhs),
            },
            MirRvalue::AtomicLoad { ptr, .. } => {
                hint.or_else(|| self.mir_operand_pointee_ty(body, ptr))
            }
            MirRvalue::AtomicRmw { ptr, .. } => {
                hint.or_else(|| self.mir_operand_pointee_ty(body, ptr))
            }
            MirRvalue::SimdLoad { ptr, .. } => {
                hint.or_else(|| self.mir_operand_pointee_ty(body, ptr))
            }
            MirRvalue::SimdMaskedLoad { or_else, .. } => {
                hint.or_else(|| self.mir_operand_ty(body, or_else))
            }
            MirRvalue::SimdGather { .. } => hint,
            MirRvalue::SimdMaskedGather { or_else, .. } => {
                hint.or_else(|| self.mir_operand_ty(body, or_else))
            }
            MirRvalue::SliceOp { .. } => hint,
            MirRvalue::SimdUnaryIntrinsic { operand, .. }
            | MirRvalue::SimdReduce { operand, .. }
            | MirRvalue::SimdAny { operand }
            | MirRvalue::SimdAll { operand }
            | MirRvalue::SimdBitmask { operand }
            | MirRvalue::SimdSplat { value: operand }
            | MirRvalue::SimdCast { value: operand }
            | MirRvalue::SimdBitcast { value: operand } => {
                hint.or_else(|| self.mir_operand_ty(body, operand))
            }
            MirRvalue::SimdBinaryIntrinsic { lhs, .. } => {
                hint.or_else(|| self.mir_operand_ty(body, lhs))
            }
            MirRvalue::SimdSelect { on_true, .. } => {
                hint.or_else(|| self.mir_operand_ty(body, on_true))
            }
            MirRvalue::SimdShuffle { lhs, .. } => hint.or_else(|| self.mir_operand_ty(body, lhs)),
            MirRvalue::SimdInsertHalf { base, .. } => {
                hint.or_else(|| self.mir_operand_ty(body, base))
            }
        }
    }

    pub(super) fn compile_mir_const_operand(&mut self, value: &MirConst) -> BasicValueEnum<'ctx> {
        match value {
            MirConst::Undef { ty } => {
                let llvm_ty = self.get_llvm_type(*ty);
                self.get_undef_val(llvm_ty)
            }
            MirConst::Integer { ty, value } => {
                let llvm_ty = self
                    .llvm_integer_storage_type(*ty)
                    .unwrap_or_else(|| self.get_llvm_type(*ty));
                if llvm_ty.is_pointer_type() {
                    let Some(ptr_ty) =
                        self.expect_pointer_type(llvm_ty, Span::default(), "MIR pointer literal")
                    else {
                        return self.zero_i8_value();
                    };
                    if *value == 0 {
                        ptr_ty.const_null().into()
                    } else {
                        let int_val = self.context.i64_type().const_int(*value as u64, false);
                        self.builder
                            .build_int_to_ptr(int_val, ptr_ty, "mir_ptr_lit")
                            .unwrap()
                            .into()
                    }
                } else {
                    let Some(int_ty) =
                        self.expect_int_type(llvm_ty, Span::default(), "MIR integer literal")
                    else {
                        return self.zero_i8_value();
                    };
                    int_ty.const_u128(*value).into()
                }
            }
            MirConst::Float { ty, value } => {
                let llvm_ty = self.get_llvm_type(*ty);
                let Some(float_ty) =
                    self.expect_float_type(llvm_ty, Span::default(), "MIR floating-point literal")
                else {
                    return self.zero_i8_value();
                };
                float_ty.const_float(*value).into()
            }
            MirConst::Bool { value } => self
                .context
                .bool_type()
                .const_int(u64::from(*value), false)
                .into(),
            MirConst::StringLiteral { ty, value } => {
                if matches!(
                    self.type_registry.get(self.type_registry.normalize(*ty)),
                    TypeKind::Slice { .. }
                ) {
                    let array_val = self.context.const_string(value.as_bytes(), true);
                    let global_name = format!(".str.mir.{}", self.string_literal_counter);
                    self.string_literal_counter += 1;
                    let global =
                        self.module
                            .add_global(array_val.get_type().into(), None, &global_name);
                    global.set_linkage(Linkage::Internal);
                    global.set_constant(true);
                    global.set_initializer(&array_val);

                    let llvm_ty = self.get_llvm_type(*ty);
                    let Some(slice_ty) = self.expect_struct_type(
                        llvm_ty,
                        Span::default(),
                        "MIR string slice literal",
                    ) else {
                        return self.zero_i8_value();
                    };
                    return slice_ty
                        .const_named_struct(&[
                            global.as_pointer_value().into(),
                            self.context
                                .i64_type()
                                .const_int(value.len() as u64, false)
                                .into(),
                        ])
                        .into();
                }
                self.context.const_string(value.as_bytes(), true).into()
            }
            MirConst::GlobalRef { ty, id } => {
                let llvm_ty = self.get_llvm_type(*ty);
                self.compile_global_ref(*id, llvm_ty)
            }
            MirConst::FuncRef { id, .. } => self.compile_func_ref(*id),
        }
    }

    pub(super) fn compile_mir_trap(&mut self) {
        let Some(decl) =
            self.lookup_intrinsic_declaration("llvm.trap", &[], Span::default(), "MIR trap")
        else {
            return;
        };
        self.builder.build_call(decl, &[], "mir_trap").unwrap();
        self.builder.build_unreachable().unwrap();
    }

    pub(super) fn compile_mir_breakpoint(&mut self) {
        let Some(decl) = self.lookup_intrinsic_declaration(
            "llvm.debugtrap",
            &[],
            Span::default(),
            "MIR breakpoint",
        ) else {
            return;
        };
        self.builder.build_call(decl, &[], "mir_bkpt").unwrap();
    }

    pub(super) fn compile_mir_operand(
        &mut self,
        body: &MirBody,
        operand: &MirOperand,
    ) -> BasicValueEnum<'ctx> {
        match operand {
            MirOperand::Local(local) => {
                let Some(local_def) = body.locals.get(local.0 as usize) else {
                    self.sess.emit_ice(
                        Span::default(),
                        format!(
                            "Kern ICE (Codegen): MIR local {:?} is out of range while compiling operand.",
                            local
                        ),
                    );
                    return self.zero_i8_value();
                };
                let Some(ptr) = self.lookup_mir_local_ptr(*local, body) else {
                    let llvm_ty = self.get_llvm_type(local_def.ty);
                    return self.get_undef_val(llvm_ty);
                };
                let llvm_ty = self.get_llvm_type(local_def.ty);
                self.builder
                    .build_load(llvm_ty, ptr, &format!("mir_load_{}", local.0))
                    .unwrap()
            }
            MirOperand::Const(expr) => self.compile_mir_const_operand(expr),
        }
    }

    pub(super) fn compile_mir_operand_as_int(
        &mut self,
        body: &MirBody,
        operand: &MirOperand,
        context: &str,
    ) -> IntValue<'ctx> {
        match self.compile_mir_operand(body, operand) {
            BasicValueEnum::IntValue(value) => value,
            other => {
                self.sess.emit_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Codegen): expected integer MIR operand for {}, found {:?}.",
                        context,
                        other.get_type()
                    ),
                );
                self.context.i8_type().const_zero()
            }
        }
    }

    pub(super) fn compile_mir_operand_as_pointer(
        &mut self,
        body: &MirBody,
        operand: &MirOperand,
        context: &str,
    ) -> PointerValue<'ctx> {
        match self.compile_mir_operand(body, operand) {
            BasicValueEnum::PointerValue(value) => value,
            other => {
                self.sess.emit_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Codegen): expected pointer MIR operand for {}, found {:?}.",
                        context,
                        other.get_type()
                    ),
                );
                self.null_ptr()
            }
        }
    }

    pub(super) fn compile_mir_rvalue_as_int(
        &mut self,
        body: &MirBody,
        rvalue: &MirRvalue,
        expected_ty: Option<TypeId>,
        context: &str,
    ) -> IntValue<'ctx> {
        match self.compile_mir_rvalue(body, rvalue, expected_ty) {
            BasicValueEnum::IntValue(value) => value,
            other => {
                self.sess.emit_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Codegen): expected integer MIR rvalue for {}, found {:?}.",
                        context,
                        other.get_type()
                    ),
                );
                self.context.i8_type().const_zero()
            }
        }
    }

    pub(super) fn compile_mir_place_ptr(
        &mut self,
        body: &MirBody,
        place: &MirPlace,
        span: Span,
    ) -> PointerValue<'ctx> {
        match place {
            MirPlace::Local(local) => self
                .lookup_mir_local_ptr(*local, body)
                .unwrap_or_else(|| self.null_ptr()),
            MirPlace::Global(global) => self
                .globals
                .get(global)
                .map(|global| global.as_pointer_value())
                .unwrap_or_else(|| {
                    self.sess.emit_ice(
                        span,
                        format!(
                            "Kern ICE (Codegen): MIR global {:?} missing from global storage map.",
                            global
                        ),
                    );
                    self.null_ptr()
                }),
            MirPlace::Deref(operand) => {
                let value = self.compile_mir_operand(body, operand);
                self.expect_pointer_value(value, span, "MIR dereference place")
                    .unwrap_or_else(|| self.null_ptr())
            }
            MirPlace::Field {
                base,
                struct_id,
                field_idx,
                ..
            } => {
                let struct_ptr = self.compile_mir_place_ptr(body, base, span);
                if self.current_block_is_terminated() {
                    return self.null_ptr();
                }
                if self.union_ids.contains(struct_id) {
                    return struct_ptr;
                }
                let Some(struct_llvm_ty) =
                    self.lookup_struct_type(*struct_id, span, "MIR field l-value")
                else {
                    return self.null_ptr();
                };
                self.builder
                    .build_struct_gep(
                        struct_llvm_ty,
                        struct_ptr,
                        *field_idx as u32,
                        "mir_field_gep",
                    )
                    .unwrap()
            }
            MirPlace::Index { base, index } => {
                let idx_val = self.compile_mir_index_operand(body, index);
                if self.current_block_is_terminated() {
                    return self.null_ptr();
                }
                let Some(base_ty) = self.mir_place_ty(body, base) else {
                    self.sess.emit_ice(
                        span,
                        "Kern ICE (Codegen): failed to recover MIR base type for indexed place.",
                    );
                    return self.null_ptr();
                };
                let norm_base_ty = self.type_registry.normalize(base_ty);

                if self.type_registry.is_simd(norm_base_ty) {
                    self.sess.emit_ice(
                        span,
                        "Kern ICE (Codegen): SIMD lanes do not have addressable MIR pointers.",
                    );
                    return self.null_ptr();
                }

                match self.type_registry.get(norm_base_ty) {
                    TypeKind::Slice { elem, .. } => {
                        let value = self.compile_mir_place_load(body, base, norm_base_ty, span);
                        let Some(slice_val) =
                            self.expect_struct_value(value, span, "MIR slice indexed place")
                        else {
                            return self.null_ptr();
                        };
                        let ptr_value = self
                            .builder
                            .build_extract_value(slice_val, 0, "mir_slice_ptr")
                            .unwrap();
                        let Some(ptr_val) =
                            self.expect_pointer_value(ptr_value, span, "MIR slice pointer field")
                        else {
                            return self.null_ptr();
                        };
                        let elem_ty = self.get_llvm_type(*elem);
                        // SAFETY: `ptr_val` is the data pointer extracted from
                        // a MIR slice whose element type is `elem`; the single
                        // index is the lowered MIR slice index.
                        unsafe {
                            self.builder
                                .build_gep(elem_ty, ptr_val, &[idx_val], "mir_slice_idx")
                                .unwrap()
                        }
                    }
                    TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                        let value = self.compile_mir_place_load(body, base, norm_base_ty, span);
                        let Some(ptr_val) =
                            self.expect_pointer_value(value, span, "MIR pointer indexed place")
                        else {
                            return self.null_ptr();
                        };
                        let elem_ty = self.get_llvm_type(*elem);
                        // SAFETY: `ptr_val` comes from a MIR pointer place with
                        // pointee type `elem`; the single index is the lowered
                        // MIR pointer index.
                        unsafe {
                            self.builder
                                .build_gep(elem_ty, ptr_val, &[idx_val], "mir_ptr_idx")
                                .unwrap()
                        }
                    }
                    TypeKind::Array { .. } => {
                        let array_ptr = self.compile_mir_place_ptr(body, base, span);
                        let zero = self.context.i64_type().const_zero();
                        let array_llvm_ty = self.get_llvm_type(norm_base_ty);
                        // SAFETY: `array_ptr` points at a MIR array place with
                        // LLVM type `array_llvm_ty`; the leading zero enters the
                        // array aggregate and `idx_val` selects the element.
                        unsafe {
                            self.builder
                                .build_gep(
                                    array_llvm_ty,
                                    array_ptr,
                                    &[zero, idx_val],
                                    "mir_array_idx",
                                )
                                .unwrap()
                        }
                    }
                    _ => {
                        self.sess.emit_ice(
                            span,
                            format!(
                                "Kern ICE (Codegen): MIR indexed place has invalid base type `{:?}`.",
                                self.type_registry.get(norm_base_ty)
                            ),
                        );
                        self.null_ptr()
                    }
                }
            }
        }
    }

    pub(super) fn compile_mir_place_load(
        &mut self,
        body: &MirBody,
        place: &MirPlace,
        expected_ty: TypeId,
        span: Span,
    ) -> BasicValueEnum<'ctx> {
        let is_volatile = self.mir_place_access_is_volatile(body, place);
        if let MirPlace::Index { base, index } = place
            && let Some(base_ty) = self.mir_place_ty(body, base)
            && self.type_registry.is_simd(base_ty)
        {
            let Some(base_ptr) = Some(self.compile_mir_place_ptr(body, base, span)) else {
                return self.zero_i8_value();
            };
            let vector_ty = self.get_llvm_type(base_ty);
            let vector_val = if is_volatile {
                self.builder
                    .build_volatile_load(vector_ty, base_ptr, "mir_simd_load")
                    .unwrap()
            } else {
                self.builder
                    .build_load(vector_ty, base_ptr, "mir_simd_load")
                    .unwrap()
            };
            let idx_value = self.compile_mir_operand(body, index);
            let Some(idx_val) = self.expect_int_value(idx_value, span, "MIR SIMD lane load index")
            else {
                return self.zero_i8_value();
            };
            let Some(vector_val) =
                self.expect_vector_value(vector_val, span, "MIR SIMD lane load base")
            else {
                return self.zero_i8_value();
            };
            return self
                .builder
                .build_extract_element(vector_val, idx_val, "mir_simd_lane")
                .unwrap();
        }

        let ptr = self.compile_mir_place_ptr(body, place, span);
        if self.current_block_is_terminated() {
            let llvm_ty = self.get_llvm_type(expected_ty);
            return self.get_undef_val(llvm_ty);
        }
        let llvm_ty = self.get_llvm_type(expected_ty);
        if is_volatile {
            self.builder
                .build_volatile_load(llvm_ty, ptr, "mir_load")
                .unwrap()
        } else {
            self.builder.build_load(llvm_ty, ptr, "mir_load").unwrap()
        }
    }

    pub(super) fn compile_mir_slice_base_parts(
        &mut self,
        body: &MirBody,
        base: &MirSliceBase,
        span: Span,
    ) -> Option<(
        PointerValue<'ctx>,
        Option<crate::values::IntValue<'ctx>>,
        TypeId,
    )> {
        let base_ty = match base {
            MirSliceBase::Operand(operand) => self.mir_operand_ty(body, operand)?,
            MirSliceBase::Place(place) => self.mir_place_ty(body, place)?,
        };
        let norm_base_ty = self.type_registry.normalize(base_ty);

        match self.type_registry.get(norm_base_ty) {
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let ptr = match base {
                    MirSliceBase::Operand(operand) => {
                        let value = self.compile_mir_operand(body, operand);
                        self.expect_pointer_value(value, span, "MIR slice pointer base")?
                    }
                    MirSliceBase::Place(place) => {
                        let value = self.compile_mir_place_load(body, place, norm_base_ty, span);
                        self.expect_pointer_value(value, span, "MIR slice pointer place base")?
                    }
                };
                Some((ptr, None, *elem))
            }
            TypeKind::Slice { elem, .. } => {
                let slice_value = match base {
                    MirSliceBase::Operand(operand) => self.compile_mir_operand(body, operand),
                    MirSliceBase::Place(place) => {
                        self.compile_mir_place_load(body, place, norm_base_ty, span)
                    }
                };
                let slice_val =
                    self.expect_struct_value(slice_value, span, "MIR slice base value")?;
                let ptr_value = self
                    .builder
                    .build_extract_value(slice_val, 0, "mir_slice_base_ptr")
                    .unwrap();
                let ptr =
                    self.expect_pointer_value(ptr_value, span, "MIR slice base pointer field")?;
                let len_value = self
                    .builder
                    .build_extract_value(slice_val, 1, "mir_slice_base_len")
                    .unwrap();
                let len = self.expect_int_value(len_value, span, "MIR slice base length field")?;
                Some((ptr, Some(len), *elem))
            }
            TypeKind::Array { elem, len, .. } => {
                let array_ptr = match base {
                    MirSliceBase::Place(place) => self.compile_mir_place_ptr(body, place, span),
                    MirSliceBase::Operand(MirOperand::Local(local)) => self
                        .lookup_mir_local_ptr(*local, body)
                        .unwrap_or_else(|| self.null_ptr()),
                    MirSliceBase::Operand(operand) => {
                        let array_val = self.compile_mir_operand(body, operand);
                        let array_llvm_ty = self.get_llvm_type(norm_base_ty);
                        let tmp =
                            self.create_entry_block_alloca(array_llvm_ty, "mir_tmp_slice_array");
                        self.builder.build_store(tmp, array_val).unwrap();
                        tmp
                    }
                };
                let len = self.const_generic_usize(*len, span)?;
                let len_val = self.context.i64_type().const_int(len, false);
                Some((array_ptr, Some(len_val), *elem))
            }
            _ => None,
        }
    }

    pub(super) fn compile_mir_store(
        &mut self,
        body: &MirBody,
        place: &MirPlace,
        value: BasicValueEnum<'ctx>,
        place_ty: TypeId,
        span: Span,
    ) {
        let is_volatile = self.mir_place_access_is_volatile(body, place);
        if let MirPlace::Index { base, index } = place
            && let Some(base_ty) = self.mir_place_ty(body, base)
            && self.type_registry.is_simd(base_ty)
        {
            let base_ptr = self.compile_mir_place_ptr(body, base, span);
            let vector_ty = self.get_llvm_type(base_ty);
            let vector_val = if is_volatile {
                self.builder
                    .build_volatile_load(vector_ty, base_ptr, "mir_simd_store_load")
                    .unwrap()
            } else {
                self.builder
                    .build_load(vector_ty, base_ptr, "mir_simd_store_load")
                    .unwrap()
            };
            let idx_value = self.compile_mir_operand(body, index);
            let Some(idx_val) = self.expect_int_value(idx_value, span, "MIR SIMD lane store index")
            else {
                return;
            };
            let Some(vector_val) =
                self.expect_vector_value(vector_val, span, "MIR SIMD lane store base")
            else {
                return;
            };
            let updated_vector = self
                .builder
                .build_insert_element(vector_val, value, idx_val, "mir_simd_lane_set")
                .unwrap();
            if is_volatile {
                self.builder
                    .build_volatile_store(base_ptr, updated_vector)
                    .unwrap();
            } else {
                self.builder.build_store(base_ptr, updated_vector).unwrap();
            }
            return;
        }

        let ptr = self.compile_mir_place_ptr(body, place, span);
        if self.current_block_is_terminated() {
            return;
        }
        let _ = place_ty;
        if is_volatile {
            self.builder.build_volatile_store(ptr, value).unwrap();
        } else {
            self.builder.build_store(ptr, value).unwrap();
        }
    }

    pub(super) fn compile_mir_assign_op(
        &mut self,
        op: AssignmentOperator,
        lhs_val: BasicValueEnum<'ctx>,
        rhs_val: BasicValueEnum<'ctx>,
        lhs_ty: TypeId,
        span: Span,
    ) -> BasicValueEnum<'ctx> {
        use AssignmentOperator::*;

        if lhs_val.is_int_value() && rhs_val.is_int_value() {
            let Some(lhs) = self.expect_int_value(lhs_val, span, "MIR assignment lhs integer")
            else {
                return self.zero_i8_value();
            };
            let Some(rhs) = self.expect_int_value(rhs_val, span, "MIR assignment rhs integer")
            else {
                return self.zero_i8_value();
            };
            let is_signed = self.is_signed_int(lhs_ty);
            return if let Some(binary_op) = Self::assignment_binary_operator(op) {
                self.compile_int_math(binary_op, lhs, rhs, is_signed, span)
            } else {
                rhs_val
            };
        }

        if lhs_val.is_float_value() && rhs_val.is_float_value() {
            let Some(lhs) = self.expect_float_value(lhs_val, span, "MIR assignment lhs float")
            else {
                return self.zero_i8_value();
            };
            let Some(rhs) = self.expect_float_value(rhs_val, span, "MIR assignment rhs float")
            else {
                return self.zero_i8_value();
            };
            return match op {
                AddAssign => self
                    .builder
                    .build_float_add(lhs, rhs, "mir_fadd_assign")
                    .unwrap()
                    .into(),
                SubtractAssign => self
                    .builder
                    .build_float_sub(lhs, rhs, "mir_fsub_assign")
                    .unwrap()
                    .into(),
                MultiplyAssign => self
                    .builder
                    .build_float_mul(lhs, rhs, "mir_fmul_assign")
                    .unwrap()
                    .into(),
                DivideAssign => self
                    .builder
                    .build_float_div(lhs, rhs, "mir_fdiv_assign")
                    .unwrap()
                    .into(),
                ModuloAssign => self
                    .builder
                    .build_float_rem(lhs, rhs, "mir_frem_assign")
                    .unwrap()
                    .into(),
                Assign => rhs_val,
                _ => {
                    self.sess.emit_ice(
                        span,
                        format!(
                            "Kern ICE (Codegen): unsupported floating-point MIR assignment operator `{:?}`.",
                            op
                        ),
                    );
                    self.zero_i8_value()
                }
            };
        }

        self.sess.emit_ice(
            span,
            format!(
                "Kern ICE (Codegen): unsupported MIR assignment operand types for `{:?}`.",
                op
            ),
        );
        self.zero_i8_value()
    }
}
