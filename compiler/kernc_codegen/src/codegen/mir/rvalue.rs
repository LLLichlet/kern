use super::*;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    fn atomic_memory_alignment(&mut self, ty: TypeId) -> u32 {
        match self.type_registry.normalize(ty) {
            TypeId::BOOL | TypeId::I8 | TypeId::U8 => 1,
            TypeId::I16 | TypeId::U16 => 2,
            TypeId::I32 | TypeId::U32 | TypeId::F32 => 4,
            TypeId::I64 | TypeId::U64 | TypeId::ISIZE | TypeId::USIZE | TypeId::F64 => 8,
            TypeId::I128 | TypeId::U128 => 16,
            norm => match self.type_registry.get(norm) {
                TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. } => 8,
                _ => 1,
            },
        }
    }

    pub(super) fn is_atomic_bool_ty(&mut self, ty: TypeId) -> bool {
        self.type_registry.normalize(ty) == TypeId::BOOL
    }

    pub(super) fn atomic_memory_type(&mut self, ty: TypeId) -> BasicTypeEnum<'ctx> {
        if self.is_atomic_bool_ty(ty) {
            self.context.i8_type().into()
        } else {
            self.get_llvm_type(ty)
        }
    }

    pub(super) fn atomic_bool_to_i8(&self, value: BasicValueEnum<'ctx>) -> IntValue<'ctx> {
        self.builder
            .build_int_z_extend(
                value.into_int_value(),
                self.context.i8_type(),
                "mir_atomic_bool_i8",
            )
            .unwrap()
    }

    pub(super) fn atomic_i8_to_bool(&self, value: IntValue<'ctx>) -> IntValue<'ctx> {
        self.builder
            .build_int_compare(
                crate::IntPredicate::NE,
                value,
                self.context.i8_type().const_zero(),
                "mir_atomic_i8_bool",
            )
            .unwrap()
    }

    pub(super) fn compile_mir_call_target_value(
        &mut self,
        _body: &MirBody,
        callee: &MirCallTarget,
    ) -> Option<FunctionValue<'ctx>> {
        match callee {
            MirCallTarget::Direct(id) => self.lookup_function_value(*id, Span::default()),
            MirCallTarget::Operand(_) => None,
        }
    }

    pub(super) fn compile_mir_rvalue(
        &mut self,
        body: &MirBody,
        rvalue: &MirRvalue,
        expected_ty: Option<TypeId>,
    ) -> BasicValueEnum<'ctx> {
        match rvalue {
            MirRvalue::Use(operand) => self.compile_mir_operand(body, operand),
            MirRvalue::Call { callee, args } => {
                let return_ty = self
                    .mir_call_return_ty(body, callee, expected_ty)
                    .unwrap_or(TypeId::ERROR);
                let llvm_ty = self.get_llvm_type(return_ty);
                let llvm_args = args
                    .iter()
                    .map(|arg| self.compile_mir_operand(body, arg))
                    .collect::<Vec<_>>();

                let call_site =
                    if let Some(llvm_func) = self.compile_mir_call_target_value(body, callee) {
                        self.builder
                            .build_call(llvm_func, &llvm_args, "mir_call")
                            .unwrap()
                    } else {
                        let MirCallTarget::Operand(operand) = callee else {
                            return self.get_undef_val(llvm_ty);
                        };
                        let ptr_val = self.compile_mir_operand(body, operand).into_pointer_value();
                        let Some(fn_ty) = self.llvm_fn_type_from_callable(
                            self.mir_operand_ty(body, operand).unwrap_or(TypeId::ERROR),
                            Span::default(),
                        ) else {
                            return self.get_undef_val(llvm_ty);
                        };
                        self.builder
                            .build_indirect_call(fn_ty, ptr_val, &llvm_args, "mir_icall")
                            .unwrap()
                    };

                if return_ty == TypeId::NEVER {
                    self.builder.build_unreachable().unwrap();
                    self.get_undef_val(llvm_ty)
                } else if return_ty == TypeId::VOID || return_ty == TypeId::ERROR {
                    self.context.i8_type().const_zero().into()
                } else {
                    call_site.try_as_basic_value().unwrap_basic()
                }
            }
            MirRvalue::Aggregate { ty, kind, fields } => {
                let expected_ty = expected_ty.unwrap_or(*ty);
                let expected_llvm_ty = self.get_llvm_type(expected_ty);
                let field_values = fields
                    .iter()
                    .map(|field| self.compile_mir_operand(body, field))
                    .collect::<Vec<_>>();

                match kind {
                    MirAggregateKind::Struct { struct_id } => {
                        let Some(struct_ty) =
                            self.lookup_struct_type(*struct_id, Span::default(), "MIR aggregate")
                        else {
                            return expected_llvm_ty.const_zero();
                        };
                        let mut value = struct_ty.get_undef();
                        for (idx, field) in field_values.iter().enumerate() {
                            value = self
                                .builder
                                .build_insert_value(value, *field, idx as u32, "mir_struct_insert")
                                .unwrap()
                                .into_struct_value();
                        }
                        value.into()
                    }
                    MirAggregateKind::Union { union_id, .. } => {
                        let Some(union_ty) = self.lookup_struct_type(
                            *union_id,
                            Span::default(),
                            "MIR union aggregate",
                        ) else {
                            return expected_llvm_ty.const_zero();
                        };
                        let Some(value) = field_values.first().copied() else {
                            let empty = union_ty.const_zero();
                            return empty.into();
                        };
                        if let Some(packed) = self.pack_union_runtime_value(union_ty, value) {
                            packed
                        } else {
                            let alloca =
                                self.create_entry_block_alloca(union_ty.into(), "mir_union_init");
                            self.builder.build_store(alloca, value).unwrap();
                            self.builder
                                .build_load(union_ty, alloca, "mir_union_load")
                                .unwrap()
                        }
                    }
                    MirAggregateKind::Array => match expected_llvm_ty {
                        BasicTypeEnum::ArrayType(array_ty) => {
                            let mut value = array_ty.const_zero();
                            for (idx, field) in field_values.iter().enumerate() {
                                value = self
                                    .builder
                                    .build_insert_value(
                                        value,
                                        *field,
                                        idx as u32,
                                        "mir_array_insert",
                                    )
                                    .unwrap()
                                    .into_array_value();
                            }
                            value.into()
                        }
                        BasicTypeEnum::VectorType(vector_ty) => {
                            let mut value = vector_ty.const_zero().into_vector_value();
                            for (idx, field) in field_values.iter().enumerate() {
                                let lane = self.context.i32_type().const_int(idx as u64, false);
                                value = self
                                    .builder
                                    .build_insert_element(value, *field, lane, "mir_vec_insert")
                                    .unwrap()
                                    .into_vector_value();
                            }
                            value.into()
                        }
                        _ => {
                            self.sess.emit_ice(
                                Span::default(),
                                "Kern ICE (Codegen): MIR array aggregate expected LLVM array/vector type.",
                            );
                            expected_llvm_ty.const_zero()
                        }
                    },
                    MirAggregateKind::FatPointer => {
                        let struct_ty = expected_llvm_ty.into_struct_type();
                        let mut value = struct_ty.const_zero();
                        for (idx, field) in field_values.iter().enumerate() {
                            value = self
                                .builder
                                .build_insert_value(value, *field, idx as u32, "mir_fatptr_insert")
                                .unwrap()
                                .into_struct_value();
                        }
                        value.into()
                    }
                    MirAggregateKind::Data {
                        data_struct_id,
                        tag_value,
                    } => {
                        let Some(struct_ty) = self.lookup_struct_type(
                            *data_struct_id,
                            Span::default(),
                            "MIR data aggregate",
                        ) else {
                            return expected_llvm_ty.const_zero();
                        };
                        let tag_field_ty = struct_ty.get_field_type_at_index(0).unwrap();
                        let tag_ty = tag_field_ty.into_int_type();
                        let union_field_ty = struct_ty.get_field_type_at_index(1).unwrap();
                        let union_ty = union_field_ty.into_struct_type();
                        let tag_val = tag_ty.const_u128(*tag_value);

                        let union_val = if let Some(payload) = field_values.first().copied() {
                            if let Some(packed) = self.pack_union_runtime_value(union_ty, payload) {
                                packed.into_struct_value()
                            } else {
                                let alloca = self.create_entry_block_alloca(
                                    union_ty.into(),
                                    "mir_data_union_init",
                                );
                                self.builder.build_store(alloca, payload).unwrap();
                                self.builder
                                    .build_load(union_ty, alloca, "mir_data_union_load")
                                    .unwrap()
                                    .into_struct_value()
                            }
                        } else {
                            union_ty.const_zero()
                        };

                        let mut data = struct_ty.const_zero();
                        data = self
                            .builder
                            .build_insert_value(data, tag_val, 0, "mir_data_tag")
                            .unwrap()
                            .into_struct_value();
                        data = self
                            .builder
                            .build_insert_value(data, union_val, 1, "mir_data_payload")
                            .unwrap()
                            .into_struct_value();
                        data.into()
                    }
                }
            }
            MirRvalue::Projection { kind, operand } => {
                let fat_ptr = self.compile_mir_operand(body, operand).into_struct_value();
                let index = match kind {
                    MirProjectionKind::FatPtrData => 0,
                    MirProjectionKind::FatPtrMeta => 1,
                };
                self.builder
                    .build_extract_value(fat_ptr, index, "mir_projection")
                    .unwrap()
            }
            MirRvalue::Unary { op, operand } => {
                let operand_ty = self.mir_operand_ty(body, operand).unwrap_or(TypeId::ERROR);
                let result_ty = expected_ty.unwrap_or(operand_ty);
                let op_val = self.compile_mir_operand(body, operand);

                if self.type_registry.is_simd(operand_ty) {
                    let Some((elem_ty, _)) = self.simd_elem_and_lanes(operand_ty) else {
                        let llvm_ty = self.get_llvm_type(result_ty);
                        return self.get_undef_val(llvm_ty);
                    };
                    return match op {
                        UnaryOperator::Negate => {
                            if self.type_registry.is_float(elem_ty) {
                                self.builder
                                    .build_basic_float_neg(op_val, "mir_simd_fneg")
                                    .unwrap()
                            } else {
                                self.builder
                                    .build_basic_neg(op_val, "mir_simd_neg")
                                    .unwrap()
                            }
                        }
                        UnaryOperator::LogicalNot | UnaryOperator::BitwiseNot => self
                            .builder
                            .build_basic_not(op_val, "mir_simd_not")
                            .unwrap(),
                        UnaryOperator::MetaOf => {
                            self.compile_mir_unary_meta(body, operand, op_val, Span::default())
                        }
                        _ => {
                            self.sess.emit_ice(
                                Span::default(),
                                format!(
                                    "Kern ICE (Codegen): unsupported MIR SIMD unary operator `{:?}`.",
                                    op
                                ),
                            );
                            self.zero_i8_value()
                        }
                    };
                }

                match op {
                    UnaryOperator::Negate => {
                        if op_val.is_int_value() {
                            self.builder
                                .build_int_neg(op_val.into_int_value(), "mir_neg")
                                .unwrap()
                                .into()
                        } else if op_val.is_float_value() {
                            self.builder
                                .build_float_neg(op_val.into_float_value(), "mir_fneg")
                                .unwrap()
                                .into()
                        } else {
                            self.sess.emit_ice(
                                Span::default(),
                                "Kern ICE (Codegen): MIR negate applied to a non-numeric value.",
                            );
                            self.zero_i8_value()
                        }
                    }
                    UnaryOperator::LogicalNot | UnaryOperator::BitwiseNot => {
                        if op_val.is_int_value() {
                            self.builder
                                .build_not(op_val.into_int_value(), "mir_not")
                                .unwrap()
                                .into()
                        } else {
                            self.sess.emit_ice(
                                Span::default(),
                                "Kern ICE (Codegen): MIR not applied to a non-integer value.",
                            );
                            self.zero_i8_value()
                        }
                    }
                    UnaryOperator::MetaOf => {
                        self.compile_mir_unary_meta(body, operand, op_val, Span::default())
                    }
                    _ => {
                        self.sess.emit_ice(
                            Span::default(),
                            format!(
                                "Kern ICE (Codegen): unsupported MIR unary operator `{:?}`.",
                                op
                            ),
                        );
                        self.zero_i8_value()
                    }
                }
            }
            MirRvalue::Binary { op, lhs, rhs } => {
                let lhs_ty = self.mir_operand_ty(body, lhs).unwrap_or(TypeId::ERROR);
                let rhs_ty = self.mir_operand_ty(body, rhs).unwrap_or(TypeId::ERROR);
                let result_ty = expected_ty.unwrap_or(lhs_ty);
                if self.type_registry.is_simd(lhs_ty) {
                    let simd_result_ty = expected_ty.unwrap_or({
                        if matches!(
                            op,
                            BinaryOperator::Equal
                                | BinaryOperator::NotEqual
                                | BinaryOperator::LessThan
                                | BinaryOperator::LessOrEqual
                                | BinaryOperator::GreaterThan
                                | BinaryOperator::GreaterOrEqual
                        ) {
                            rhs_ty
                        } else {
                            lhs_ty
                        }
                    });
                    return self.compile_mir_simd_binary(body, *op, lhs, rhs, simd_result_ty);
                }

                let lhs_val = self.compile_mir_operand(body, lhs);
                let rhs_val = self.compile_mir_operand(body, rhs);

                if lhs_val.is_pointer_value() || rhs_val.is_pointer_value() {
                    self.compile_ptr_math(*op, lhs_val, rhs_val, lhs_ty, rhs_ty, Span::default())
                } else if lhs_val.is_int_value() && rhs_val.is_int_value() {
                    // Sema can contextualize arithmetic to a wider integer result without
                    // leaving behind an explicit cast node in MIR.
                    let arithmetic_result = !matches!(
                        op,
                        BinaryOperator::Equal
                            | BinaryOperator::NotEqual
                            | BinaryOperator::LessThan
                            | BinaryOperator::LessOrEqual
                            | BinaryOperator::GreaterThan
                            | BinaryOperator::GreaterOrEqual
                    );
                    let lhs_int = if arithmetic_result && self.type_registry.is_integer(result_ty) {
                        self.cast_mir_int_to_expected_type(lhs_val.into_int_value(), result_ty)
                    } else {
                        lhs_val.into_int_value()
                    };
                    let rhs_int = if arithmetic_result && self.type_registry.is_integer(result_ty) {
                        self.cast_mir_int_to_expected_type(rhs_val.into_int_value(), result_ty)
                    } else {
                        rhs_val.into_int_value()
                    };
                    self.compile_int_math(
                        *op,
                        lhs_int,
                        rhs_int,
                        if arithmetic_result {
                            self.is_signed_int(result_ty)
                        } else {
                            self.is_signed_int(lhs_ty)
                        },
                        Span::default(),
                    )
                } else if lhs_val.is_float_value() && rhs_val.is_float_value() {
                    self.compile_float_math(
                        *op,
                        lhs_val.into_float_value(),
                        rhs_val.into_float_value(),
                        Span::default(),
                    )
                } else {
                    let lhs_llvm_ty = lhs_val.get_type();
                    let rhs_llvm_ty = rhs_val.get_type();
                    self.sess.emit_ice(
                        Span::default(),
                        format!(
                            "Kern ICE (Codegen): unsupported MIR binary operand types for `{:?}`: lhs TypeId({:?}) LLVM {:?}, rhs TypeId({:?}) LLVM {:?}.",
                            op, lhs_ty, lhs_llvm_ty, rhs_ty, rhs_llvm_ty
                        ),
                    );
                    self.zero_i8_value()
                }
            }
            MirRvalue::Cast { kind, operand } => {
                let Some(target_ty) = expected_ty else {
                    self.sess.emit_ice(
                        Span::default(),
                        "Kern ICE (Codegen): MIR cast requires an expected target type.",
                    );
                    return self.zero_i8_value();
                };
                self.compile_mir_cast(body, *kind, operand, target_ty)
            }
            MirRvalue::BitIntrinsic { kind, operand } => {
                self.compile_mir_bit_intrinsic(body, *kind, operand, expected_ty)
            }
            MirRvalue::AtomicLoad { ptr, ordering } => {
                let Some(target_ty) =
                    expected_ty.or_else(|| self.mir_operand_pointee_ty(body, ptr))
                else {
                    self.sess.emit_ice(
                        Span::default(),
                        "Kern ICE (Codegen): MIR atomic load requires a recoverable pointee type.",
                    );
                    return self.zero_i8_value();
                };
                self.compile_mir_atomic_load(body, ptr, *ordering, target_ty)
            }
            MirRvalue::AtomicCas {
                weak,
                ptr,
                expected,
                desired,
                success,
                failure,
            } => {
                let Some(result_ty) = expected_ty else {
                    self.sess.emit_ice(
                        Span::default(),
                        "Kern ICE (Codegen): MIR cmpxchg requires an expected result type.",
                    );
                    return self.zero_i8_value();
                };
                self.compile_mir_atomic_cas(
                    body,
                    super::atomic::AtomicCasArgs {
                        result_ty,
                        weak: *weak,
                        ptr,
                        expected,
                        desired,
                        success: *success,
                        failure: *failure,
                    },
                )
            }
            MirRvalue::AtomicRmw {
                op,
                ptr,
                value,
                ordering,
            } => {
                let Some(result_ty) =
                    expected_ty.or_else(|| self.mir_operand_pointee_ty(body, ptr))
                else {
                    self.sess.emit_ice(
                        Span::default(),
                        "Kern ICE (Codegen): MIR atomic rmw requires a recoverable pointee type.",
                    );
                    return self.zero_i8_value();
                };
                self.compile_mir_atomic_rmw(body, result_ty, *op, ptr, value, *ordering)
            }
            MirRvalue::SimdUnaryIntrinsic { kind, operand } => {
                let result_ty = expected_ty
                    .or_else(|| self.mir_operand_ty(body, operand))
                    .unwrap_or(TypeId::ERROR);
                self.compile_mir_simd_unary_intrinsic(body, *kind, operand, result_ty)
            }
            MirRvalue::SimdBinaryIntrinsic { kind, lhs, rhs } => {
                let result_ty = expected_ty
                    .or_else(|| self.mir_operand_ty(body, lhs))
                    .unwrap_or(TypeId::ERROR);
                self.compile_mir_simd_binary_intrinsic(body, *kind, lhs, rhs, result_ty)
            }
            MirRvalue::SimdReduce { kind, operand } => {
                let result_ty = expected_ty.unwrap_or_else(|| {
                    self.mir_operand_ty(body, operand)
                        .and_then(|ty| self.type_registry.get_elem_type(ty))
                        .unwrap_or(TypeId::ERROR)
                });
                self.compile_mir_simd_reduce(body, *kind, operand, result_ty)
            }
            MirRvalue::SimdAny { operand } => {
                self.compile_mir_simd_reduce_mask(body, operand, false, false)
            }
            MirRvalue::SimdAll { operand } => {
                self.compile_mir_simd_reduce_mask(body, operand, true, true)
            }
            MirRvalue::SimdBitmask { operand } => self.compile_mir_simd_bitmask(body, operand),
            MirRvalue::SimdSplat { value } => {
                let Some(result_ty) = expected_ty else {
                    self.sess.emit_ice(
                        Span::default(),
                        "Kern ICE (Codegen): MIR SIMD splat requires an expected result type.",
                    );
                    return self.zero_i8_value();
                };
                self.compile_mir_simd_splat(body, value, result_ty)
            }
            MirRvalue::SimdCast { value } => {
                let Some(result_ty) = expected_ty else {
                    self.sess.emit_ice(
                        Span::default(),
                        "Kern ICE (Codegen): MIR SIMD cast requires an expected result type.",
                    );
                    return self.zero_i8_value();
                };
                self.compile_mir_simd_cast(body, value, result_ty)
            }
            MirRvalue::SimdBitcast { value } => {
                let Some(result_ty) = expected_ty else {
                    self.sess.emit_ice(
                        Span::default(),
                        "Kern ICE (Codegen): MIR SIMD bitcast requires an expected result type.",
                    );
                    return self.zero_i8_value();
                };
                self.compile_mir_simd_bitcast(body, value, result_ty)
            }
            MirRvalue::SimdSelect {
                mask,
                on_true,
                on_false,
            } => self.compile_mir_simd_select(body, mask, on_true, on_false),
            MirRvalue::SimdShuffle { lhs, rhs, indices } => {
                self.compile_mir_simd_shuffle(body, lhs, rhs, indices)
            }
            MirRvalue::SimdInsertHalf {
                base,
                half,
                high_half,
            } => {
                let Some(result_ty) = expected_ty else {
                    self.sess.emit_ice(
                        Span::default(),
                        "Kern ICE (Codegen): MIR SIMD half insert requires an expected result type.",
                    );
                    return self.zero_i8_value();
                };
                self.compile_mir_simd_insert_half(body, base, half, result_ty, *high_half)
            }
            MirRvalue::SimdLoad { ptr, align } => {
                let Some(result_ty) =
                    expected_ty.or_else(|| self.mir_operand_pointee_ty(body, ptr))
                else {
                    self.sess.emit_ice(
                        Span::default(),
                        "Kern ICE (Codegen): MIR SIMD load requires a recoverable result type.",
                    );
                    return self.zero_i8_value();
                };
                self.compile_mir_simd_load(body, ptr, result_ty, *align)
            }
            MirRvalue::SimdMaskedLoad {
                ptr,
                mask,
                or_else,
                align,
            } => {
                let Some(result_ty) = expected_ty.or_else(|| self.mir_operand_ty(body, or_else))
                else {
                    self.sess.emit_ice(
                        Span::default(),
                        "Kern ICE (Codegen): MIR SIMD masked load requires a recoverable result type.",
                    );
                    return self.zero_i8_value();
                };
                self.compile_mir_simd_masked_load(body, ptr, mask, or_else, result_ty, *align)
            }
            MirRvalue::SimdGather { ptr, indices } => {
                let Some(result_ty) = expected_ty else {
                    self.sess.emit_ice(
                        Span::default(),
                        "Kern ICE (Codegen): MIR SIMD gather requires an expected result type.",
                    );
                    return self.zero_i8_value();
                };
                self.compile_mir_simd_gather(body, ptr, indices, result_ty)
            }
            MirRvalue::SimdMaskedGather {
                ptr,
                indices,
                mask,
                or_else,
            } => {
                let Some(result_ty) = expected_ty.or_else(|| self.mir_operand_ty(body, or_else))
                else {
                    self.sess.emit_ice(
                        Span::default(),
                        "Kern ICE (Codegen): MIR SIMD masked gather requires a recoverable result type.",
                    );
                    return self.zero_i8_value();
                };
                self.compile_mir_simd_masked_gather(body, ptr, indices, mask, or_else, result_ty)
            }
            MirRvalue::SliceOp {
                lhs,
                start,
                end,
                is_inclusive,
            } => {
                let Some(result_ty) = expected_ty else {
                    self.sess.emit_ice(
                        Span::default(),
                        "Kern ICE (Codegen): MIR slice op requires an expected result type.",
                    );
                    return self.zero_i8_value();
                };
                self.compile_mir_slice_op(
                    body,
                    lhs,
                    start.as_ref(),
                    end.as_ref(),
                    *is_inclusive,
                    result_ty,
                )
            }
            MirRvalue::AddressOf(place) => self
                .compile_mir_place_ptr(body, place, Span::default())
                .into(),
            MirRvalue::Load(place) => {
                let target_ty = expected_ty
                    .or_else(|| self.mir_place_ty(body, place))
                    .unwrap_or(TypeId::ERROR);
                self.compile_mir_place_load(body, place, target_ty, Span::default())
            }
        }
    }

    pub(super) fn compile_mir_unary_meta(
        &mut self,
        body: &MirBody,
        operand: &MirOperand,
        op_val: BasicValueEnum<'ctx>,
        span: Span,
    ) -> BasicValueEnum<'ctx> {
        let Some(operand_ty) = self.mir_operand_ty(body, operand) else {
            return self.zero_i8_value();
        };
        let norm_ty = self.type_registry.normalize(operand_ty);
        match self.type_registry.get(norm_ty) {
            TypeKind::Array { len, .. } => {
                let Some(len) = self.const_generic_usize(*len, span) else {
                    return self.zero_i8_value();
                };
                self.context.i64_type().const_int(len, false).into()
            }
            TypeKind::Slice { .. } => self
                .builder
                .build_extract_value(op_val.into_struct_value(), 1, "mir_slice_len")
                .unwrap(),
            other => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): MIR `MetaOf` applied to invalid type {:?}.",
                        other
                    ),
                );
                self.zero_i8_value()
            }
        }
    }

    pub(super) fn compile_mir_cast(
        &mut self,
        body: &MirBody,
        kind: MirCastKind,
        operand: &MirOperand,
        target_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let target_llvm_ty = self.get_llvm_type(target_ty);
        let value = self.compile_mir_operand(body, operand);

        match kind {
            MirCastKind::Bitcast => {
                if value.is_struct_value() && target_llvm_ty.is_pointer_type() {
                    self.builder
                        .build_extract_value(value.into_struct_value(), 0, "mir_slice_ptr")
                        .unwrap()
                        .into_pointer_value()
                        .into()
                } else {
                    self.builder
                        .build_bit_cast(value, target_llvm_ty, "mir_bitcast")
                        .unwrap()
                }
            }
            MirCastKind::PtrToInt => self
                .builder
                .build_ptr_to_int(
                    value.into_pointer_value(),
                    target_llvm_ty.into_int_type(),
                    "mir_ptr2int",
                )
                .unwrap()
                .into(),
            MirCastKind::IntToPtr => self
                .builder
                .build_int_to_ptr(
                    value.into_int_value(),
                    target_llvm_ty.into_pointer_type(),
                    "mir_int2ptr",
                )
                .unwrap()
                .into(),
            MirCastKind::ZeroExt => self
                .builder
                .build_int_z_extend(
                    value.into_int_value(),
                    target_llvm_ty.into_int_type(),
                    "mir_zext",
                )
                .unwrap()
                .into(),
            MirCastKind::SignExt => self
                .builder
                .build_int_s_extend(
                    value.into_int_value(),
                    target_llvm_ty.into_int_type(),
                    "mir_sext",
                )
                .unwrap()
                .into(),
            MirCastKind::Trunc => self
                .builder
                .build_int_truncate(
                    value.into_int_value(),
                    target_llvm_ty.into_int_type(),
                    "mir_trunc",
                )
                .unwrap()
                .into(),
            MirCastKind::SIntToFloat => self
                .builder
                .build_signed_int_to_float(
                    value.into_int_value(),
                    target_llvm_ty.into_float_type(),
                    "mir_sitofp",
                )
                .unwrap()
                .into(),
            MirCastKind::UIntToFloat => self
                .builder
                .build_unsigned_int_to_float(
                    value.into_int_value(),
                    target_llvm_ty.into_float_type(),
                    "mir_uitofp",
                )
                .unwrap()
                .into(),
            MirCastKind::FloatToSInt => self
                .builder
                .build_float_to_signed_int(
                    value.into_float_value(),
                    target_llvm_ty.into_int_type(),
                    "mir_fptosi",
                )
                .unwrap()
                .into(),
            MirCastKind::FloatToUInt => self
                .builder
                .build_float_to_unsigned_int(
                    value.into_float_value(),
                    target_llvm_ty.into_int_type(),
                    "mir_fptoui",
                )
                .unwrap()
                .into(),
            MirCastKind::FloatCast => self
                .builder
                .build_float_cast(
                    value.into_float_value(),
                    target_llvm_ty.into_float_type(),
                    "mir_fcast",
                )
                .unwrap()
                .into(),
            MirCastKind::ArrayToSlice => {
                let Some(source_ty) = self.mir_operand_ty(body, operand) else {
                    return target_llvm_ty.const_zero();
                };
                let array_len = match self
                    .type_registry
                    .get(self.type_registry.normalize(source_ty))
                {
                    TypeKind::Array { len, .. } => {
                        let Some(len) = self.const_generic_usize(*len, Span::default()) else {
                            return target_llvm_ty.const_zero();
                        };
                        len
                    }
                    other => {
                        self.sess.emit_ice(
                            Span::default(),
                            format!(
                                "Kern ICE (Codegen): MIR ArrayToSlice expected array operand, found {:?}.",
                                other
                            ),
                        );
                        return target_llvm_ty.const_zero();
                    }
                };

                let array_ptr = match operand {
                    MirOperand::Local(local) => self
                        .lookup_mir_local_ptr(*local, body)
                        .unwrap_or_else(|| self.null_ptr()),
                    MirOperand::Const(const_value) => match const_value {
                        MirConst::GlobalRef { id, .. } => self
                            .globals
                            .get(id)
                            .map(|global| global.as_pointer_value())
                            .unwrap_or_else(|| self.null_ptr()),
                        _ => {
                            let source_llvm_ty = self.get_llvm_type(source_ty);
                            let tmp = self.create_entry_block_alloca(
                                source_llvm_ty,
                                "mir_tmp_array_for_slice",
                            );
                            let array_val = self.compile_mir_const_operand(const_value);
                            self.builder.build_store(tmp, array_val).unwrap();
                            tmp
                        }
                    },
                };

                let slice_ty = target_llvm_ty.into_struct_type();
                let mut slice = slice_ty.get_undef();
                slice = self
                    .builder
                    .build_insert_value(slice, array_ptr, 0, "mir_slice_ptr")
                    .unwrap()
                    .into_struct_value();
                let len_val = self.context.i64_type().const_int(array_len, false);
                slice = self
                    .builder
                    .build_insert_value(slice, len_val, 1, "mir_slice_len")
                    .unwrap()
                    .into_struct_value();
                slice.into()
            }
        }
    }

    pub(super) fn compile_mir_bit_intrinsic(
        &mut self,
        body: &MirBody,
        kind: MirBitIntrinsicKind,
        operand: &MirOperand,
        expected_ty: Option<TypeId>,
    ) -> BasicValueEnum<'ctx> {
        let result_ty = expected_ty
            .or_else(|| self.mir_operand_ty(body, operand))
            .unwrap_or(TypeId::ERROR);
        let llvm_ty = self.get_llvm_type(result_ty);
        let value = self.compile_mir_operand(body, operand);
        if self.current_block_is_terminated() {
            return self.get_undef_val(llvm_ty);
        }

        let intrinsic_name = match kind {
            MirBitIntrinsicKind::PopCount => "llvm.ctpop",
            MirBitIntrinsicKind::Clz => "llvm.ctlz",
            MirBitIntrinsicKind::Ctz => "llvm.cttz",
            MirBitIntrinsicKind::Bswap => "llvm.bswap",
        };
        let intrinsic = Intrinsic::find(intrinsic_name).unwrap();
        let decl = intrinsic.get_declaration(&self.module, &[llvm_ty]).unwrap();
        let call = if matches!(
            kind,
            MirBitIntrinsicKind::PopCount | MirBitIntrinsicKind::Bswap
        ) {
            self.builder
                .build_call(decl, &[value], "mir_bit_op")
                .unwrap()
        } else {
            let is_zero_poison = self.context.bool_type().const_zero();
            self.builder
                .build_call(decl, &[value, is_zero_poison.into()], "mir_lz_tz")
                .unwrap()
        };
        call.try_as_basic_value().unwrap_basic()
    }

    pub(super) fn compile_mir_atomic_load(
        &mut self,
        body: &MirBody,
        ptr: &MirOperand,
        ordering: AtomicOrdering,
        target_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let llvm_ty = self.get_llvm_type(target_ty);
        let atomic_ty = self.atomic_memory_type(target_ty);
        let ptr_val = self.compile_mir_operand(body, ptr).into_pointer_value();
        if self.current_block_is_terminated() {
            return self.get_undef_val(llvm_ty);
        }

        if matches!(
            self.type_registry
                .get(self.type_registry.normalize(target_ty)),
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
        ) {
            let ptr_int_ty = self.atomic_xchg_pointer_width_int();
            let load = self
                .builder
                .build_load(ptr_int_ty, ptr_val, "mir_atomic_load_ptr_int")
                .unwrap();
            if let Some(inst) = load.as_instruction_value() {
                inst.set_atomic_ordering(Self::llvm_atomic_ordering(ordering));
                inst.set_alignment(self.atomic_memory_alignment(target_ty));
            }
            return self
                .builder
                .build_int_to_ptr(
                    load.into_int_value(),
                    llvm_ty.into_pointer_type(),
                    "mir_atomic_load_ptr",
                )
                .unwrap()
                .into();
        }

        let load = self
            .builder
            .build_load(atomic_ty, ptr_val, "mir_atomic_load")
            .unwrap();
        if let Some(inst) = load.as_instruction_value() {
            inst.set_atomic_ordering(Self::llvm_atomic_ordering(ordering));
            inst.set_alignment(self.atomic_memory_alignment(target_ty));
        }
        if self.is_atomic_bool_ty(target_ty) {
            return self.atomic_i8_to_bool(load.into_int_value()).into();
        }
        load
    }

    pub(super) fn compile_mir_atomic_store(
        &mut self,
        body: &MirBody,
        ptr: &MirOperand,
        value: &MirOperand,
        ordering: AtomicOrdering,
    ) {
        let ptr_val = self.compile_mir_operand(body, ptr).into_pointer_value();
        if self.current_block_is_terminated() {
            return;
        }
        let value_val = self.compile_mir_operand(body, value);
        if self.current_block_is_terminated() {
            return;
        }
        let value_ty = self.mir_operand_ty(body, value);
        let store_val = if value_ty.is_some_and(|value_ty| {
            matches!(
                self.type_registry
                    .get(self.type_registry.normalize(value_ty)),
                TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
            )
        }) {
            self.builder
                .build_ptr_to_int(
                    value_val.into_pointer_value(),
                    self.atomic_xchg_pointer_width_int(),
                    "mir_atomic_store_ptr_int",
                )
                .unwrap()
                .into()
        } else if value_ty.is_some_and(|value_ty| self.is_atomic_bool_ty(value_ty)) {
            self.atomic_bool_to_i8(value_val).into()
        } else {
            value_val
        };
        let store = self.builder.build_store(ptr_val, store_val).unwrap();
        store.set_atomic_ordering(Self::llvm_atomic_ordering(ordering));
        if let Some(value_ty) = value_ty {
            store.set_alignment(self.atomic_memory_alignment(value_ty));
        }
    }

    pub(super) fn compile_mir_inline_asm(&mut self, body: &MirBody, asm: &MirInlineAsm) {
        let mut input_args = Vec::with_capacity(asm.input_args.len());
        for input in &asm.input_args {
            input_args.push(self.compile_mir_operand(body, input));
            if self.current_block_is_terminated() {
                return;
            }
        }

        let mut output_ptrs = Vec::with_capacity(asm.output_ptrs.len());
        for output in &asm.output_ptrs {
            output_ptrs.push(self.compile_mir_operand(body, output).into_pointer_value());
            if self.current_block_is_terminated() {
                return;
            }
        }

        self.compile_inline_asm_parts(
            &asm.asm_template,
            &asm.constraints,
            &input_args,
            &output_ptrs,
            &asm.output_tys,
            asm.is_volatile,
        );
    }
}
