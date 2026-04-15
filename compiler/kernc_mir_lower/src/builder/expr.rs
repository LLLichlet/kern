use super::*;

impl MirFunctionBuilder {
    pub(super) fn lower_rvalue(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<MirRvalue>> {
        match &expr.kind {
            MastExprKind::Block(_) | MastExprKind::If { .. } | MastExprKind::Switch { .. } => {
                let temp = self.new_temp_local(expr.ty);
                let Some(end_block) =
                    self.lower_expr_into_place(*block_id, expr, MirPlace::Local(temp))?
                else {
                    return Ok(None);
                };
                *block_id = end_block;
                Ok(Some(MirRvalue::Use(MirOperand::Local(temp))))
            }
            MastExprKind::Call { callee, args } => {
                let Some(callee) = self.lower_call_target(block_id, callee)? else {
                    return Ok(None);
                };
                let Some(args) = self.lower_operands(block_id, args)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Call { callee, args }))
            }
            MastExprKind::StructInit { struct_id, fields } => {
                let Some(fields) = self.lower_operands(block_id, fields)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Aggregate {
                    ty: expr.ty,
                    kind: MirAggregateKind::Struct {
                        struct_id: *struct_id,
                    },
                    fields,
                }))
            }
            MastExprKind::UnionInit {
                union_id,
                field_idx,
                value,
            } => {
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Aggregate {
                    ty: expr.ty,
                    kind: MirAggregateKind::Union {
                        union_id: *union_id,
                        field_idx: *field_idx,
                    },
                    fields: vec![value],
                }))
            }
            MastExprKind::ArrayInit(fields) => {
                let Some(fields) = self.lower_operands(block_id, fields)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Aggregate {
                    ty: expr.ty,
                    kind: MirAggregateKind::Array,
                    fields,
                }))
            }
            MastExprKind::ConstructFatPointer { data_ptr, meta } => {
                let Some(data_ptr) = self.lower_expr_to_operand(block_id, data_ptr)? else {
                    return Ok(None);
                };
                let Some(meta) = self.lower_expr_to_operand(block_id, meta)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Aggregate {
                    ty: expr.ty,
                    kind: MirAggregateKind::FatPointer,
                    fields: vec![data_ptr, meta],
                }))
            }
            MastExprKind::DataInit {
                data_struct_id,
                tag_value,
                payload,
            } => {
                let Some(payload) = self.lower_expr_to_operand(block_id, payload)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Aggregate {
                    ty: expr.ty,
                    kind: MirAggregateKind::Data {
                        data_struct_id: *data_struct_id,
                        tag_value: *tag_value,
                    },
                    fields: vec![payload],
                }))
            }
            MastExprKind::ExtractFatPtrData(inner) => {
                let Some(inner) = self.lower_expr_to_operand(block_id, inner)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Projection {
                    kind: MirProjectionKind::FatPtrData,
                    operand: inner,
                }))
            }
            MastExprKind::ExtractFatPtrMeta(inner) => {
                let Some(inner) = self.lower_expr_to_operand(block_id, inner)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Projection {
                    kind: MirProjectionKind::FatPtrMeta,
                    operand: inner,
                }))
            }
            MastExprKind::Unary { op, operand } => {
                let Some(operand) = self.lower_expr_to_operand(block_id, operand)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Unary { op: *op, operand }))
            }
            MastExprKind::Binary { op, lhs, rhs } => {
                let Some(lhs) = self.lower_expr_to_operand(block_id, lhs)? else {
                    return Ok(None);
                };
                let Some(rhs) = self.lower_expr_to_operand(block_id, rhs)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Binary { op: *op, lhs, rhs }))
            }
            MastExprKind::Cast { kind, operand } => {
                let Some(operand) = self.lower_expr_to_operand(block_id, operand)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Cast {
                    kind: lower_cast_kind(*kind),
                    operand,
                }))
            }
            MastExprKind::BitIntrinsic { kind, operand } => {
                let Some(operand) = self.lower_expr_to_operand(block_id, operand)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::BitIntrinsic {
                    kind: lower_bit_intrinsic_kind(*kind),
                    operand,
                }))
            }
            MastExprKind::AtomicLoad { ptr, ordering } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::AtomicLoad {
                    ptr,
                    ordering: *ordering,
                }))
            }
            MastExprKind::AtomicCas {
                weak,
                ptr,
                expected,
                desired,
                success,
                failure,
            } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(expected) = self.lower_expr_to_operand(block_id, expected)? else {
                    return Ok(None);
                };
                let Some(desired) = self.lower_expr_to_operand(block_id, desired)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::AtomicCas {
                    weak: *weak,
                    ptr,
                    expected,
                    desired,
                    success: *success,
                    failure: *failure,
                }))
            }
            MastExprKind::AtomicRmw {
                op,
                ptr,
                value,
                ordering,
            } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::AtomicRmw {
                    op: *op,
                    ptr,
                    value,
                    ordering: *ordering,
                }))
            }
            MastExprKind::SimdUnaryIntrinsic { kind, operand } => {
                let Some(operand) = self.lower_expr_to_operand(block_id, operand)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdUnaryIntrinsic {
                    kind: lower_simd_unary_intrinsic_kind(*kind),
                    operand,
                }))
            }
            MastExprKind::SimdBinaryIntrinsic { kind, lhs, rhs } => {
                let Some(lhs) = self.lower_expr_to_operand(block_id, lhs)? else {
                    return Ok(None);
                };
                let Some(rhs) = self.lower_expr_to_operand(block_id, rhs)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdBinaryIntrinsic {
                    kind: lower_simd_binary_intrinsic_kind(*kind),
                    lhs,
                    rhs,
                }))
            }
            MastExprKind::SimdReduce { kind, operand } => {
                let Some(operand) = self.lower_expr_to_operand(block_id, operand)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdReduce {
                    kind: lower_simd_reduce_kind(*kind),
                    operand,
                }))
            }
            MastExprKind::SimdAny { operand } => {
                let Some(operand) = self.lower_expr_to_operand(block_id, operand)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdAny { operand }))
            }
            MastExprKind::SimdAll { operand } => {
                let Some(operand) = self.lower_expr_to_operand(block_id, operand)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdAll { operand }))
            }
            MastExprKind::SimdBitmask { operand } => {
                let Some(operand) = self.lower_expr_to_operand(block_id, operand)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdBitmask { operand }))
            }
            MastExprKind::SimdSplat { value } => {
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdSplat { value }))
            }
            MastExprKind::SimdCast { value } => {
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdCast { value }))
            }
            MastExprKind::SimdBitcast { value } => {
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdBitcast { value }))
            }
            MastExprKind::SimdSelect {
                mask,
                on_true,
                on_false,
            } => {
                let Some(mask) = self.lower_expr_to_operand(block_id, mask)? else {
                    return Ok(None);
                };
                let Some(on_true) = self.lower_expr_to_operand(block_id, on_true)? else {
                    return Ok(None);
                };
                let Some(on_false) = self.lower_expr_to_operand(block_id, on_false)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdSelect {
                    mask,
                    on_true,
                    on_false,
                }))
            }
            MastExprKind::SimdShuffle { lhs, rhs, indices } => {
                let Some(lhs) = self.lower_expr_to_operand(block_id, lhs)? else {
                    return Ok(None);
                };
                let Some(rhs) = self.lower_expr_to_operand(block_id, rhs)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdShuffle {
                    lhs,
                    rhs,
                    indices: indices.clone(),
                }))
            }
            MastExprKind::SimdInsertHalf {
                base,
                half,
                high_half,
            } => {
                let Some(base) = self.lower_expr_to_operand(block_id, base)? else {
                    return Ok(None);
                };
                let Some(half) = self.lower_expr_to_operand(block_id, half)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdInsertHalf {
                    base,
                    half,
                    high_half: *high_half,
                }))
            }
            MastExprKind::SimdLoad { ptr, align } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdLoad { ptr, align: *align }))
            }
            MastExprKind::SimdMaskedLoad {
                ptr,
                mask,
                or_else,
                align,
            } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(mask) = self.lower_expr_to_operand(block_id, mask)? else {
                    return Ok(None);
                };
                let Some(or_else) = self.lower_expr_to_operand(block_id, or_else)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdMaskedLoad {
                    ptr,
                    mask,
                    or_else,
                    align: *align,
                }))
            }
            MastExprKind::SimdGather { ptr, indices } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(indices) = self.lower_expr_to_operand(block_id, indices)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdGather { ptr, indices }))
            }
            MastExprKind::SimdMaskedGather {
                ptr,
                indices,
                mask,
                or_else,
            } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(indices) = self.lower_expr_to_operand(block_id, indices)? else {
                    return Ok(None);
                };
                let Some(mask) = self.lower_expr_to_operand(block_id, mask)? else {
                    return Ok(None);
                };
                let Some(or_else) = self.lower_expr_to_operand(block_id, or_else)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdMaskedGather {
                    ptr,
                    indices,
                    mask,
                    or_else,
                }))
            }
            MastExprKind::SliceOp {
                lhs,
                start,
                end,
                is_inclusive,
            } => {
                let Some(lhs) = self.lower_slice_base(block_id, lhs)? else {
                    return Ok(None);
                };
                let start = match start.as_deref() {
                    Some(start) => {
                        let Some(start) = self.lower_expr_to_operand(block_id, start)? else {
                            return Ok(None);
                        };
                        Some(start)
                    }
                    None => None,
                };
                let end = match end.as_deref() {
                    Some(end) => {
                        let Some(end) = self.lower_expr_to_operand(block_id, end)? else {
                            return Ok(None);
                        };
                        Some(end)
                    }
                    None => None,
                };
                Ok(Some(MirRvalue::SliceOp {
                    lhs,
                    start,
                    end,
                    is_inclusive: *is_inclusive,
                }))
            }
            MastExprKind::AddressOf(inner) => {
                let Some(place) = self.lower_place(block_id, inner)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::AddressOf(place)))
            }
            MastExprKind::Deref(_)
            | MastExprKind::FieldAccess { .. }
            | MastExprKind::IndexAccess { .. } => {
                let Some(place) = self.lower_place(block_id, expr)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Load(place)))
            }
            MastExprKind::Return(_)
            | MastExprKind::Loop { .. }
            | MastExprKind::Break
            | MastExprKind::Continue
            | MastExprKind::Unreachable
            | MastExprKind::Trap => {
                let _ = self.lower_control_or_eval_stmt(*block_id, expr)?;
                Ok(None)
            }
            _ => {
                if let Some(operand) = self.lower_direct_operand(expr) {
                    Ok(Some(MirRvalue::Use(operand)))
                } else {
                    self.unsupported_expr(expr, "rvalue position")
                }
            }
        }
    }

    fn lower_call_target(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<MirCallTarget>> {
        match &expr.kind {
            MastExprKind::FuncRef(id) => Ok(Some(MirCallTarget::Direct(*id))),
            _ => {
                let Some(operand) = self.lower_expr_to_operand(block_id, expr)? else {
                    return Ok(None);
                };
                Ok(Some(MirCallTarget::Operand(operand)))
            }
        }
    }

    fn lower_direct_operand(&self, expr: &MastExpr) -> Option<MirOperand> {
        match &expr.kind {
            MastExprKind::Var(name) => self.lookup_local(*name).map(MirOperand::Local),
            MastExprKind::Undef => Some(MirOperand::Const(MirConst::Undef { ty: expr.ty })),
            MastExprKind::Integer(value) => Some(MirOperand::Const(MirConst::Integer {
                ty: expr.ty,
                value: *value,
            })),
            MastExprKind::Float(value) => Some(MirOperand::Const(MirConst::Float {
                ty: expr.ty,
                value: *value,
            })),
            MastExprKind::Bool(value) => Some(MirOperand::Const(MirConst::Bool { value: *value })),
            MastExprKind::StringLiteral(value) => {
                Some(MirOperand::Const(MirConst::StringLiteral {
                    ty: expr.ty,
                    value: value.clone(),
                }))
            }
            MastExprKind::GlobalRef(id) => Some(MirOperand::Const(MirConst::GlobalRef {
                ty: expr.ty,
                id: *id,
            })),
            MastExprKind::FuncRef(id) => Some(MirOperand::Const(MirConst::FuncRef {
                ty: expr.ty,
                id: *id,
            })),
            _ => None,
        }
    }

    pub(super) fn lower_expr_to_operand(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<MirOperand>> {
        if let Some(operand) = self.lower_direct_operand(expr) {
            return Ok(Some(operand));
        }

        if matches!(
            expr.kind,
            MastExprKind::Block(_) | MastExprKind::If { .. } | MastExprKind::Switch { .. }
        ) {
            let temp = self.new_temp_local(expr.ty);
            let Some(end_block) =
                self.lower_expr_into_place(*block_id, expr, MirPlace::Local(temp))?
            else {
                return Ok(None);
            };
            *block_id = end_block;
            return Ok(Some(MirOperand::Local(temp)));
        }

        match &expr.kind {
            MastExprKind::Loop { .. }
            | MastExprKind::Break
            | MastExprKind::Continue
            | MastExprKind::Return(_)
            | MastExprKind::Unreachable
            | MastExprKind::Trap => {
                let _ = self.lower_control_or_eval_stmt(*block_id, expr)?;
                Ok(None)
            }
            MastExprKind::Assign { .. }
            | MastExprKind::Asm(_)
            | MastExprKind::SimdStore { .. }
            | MastExprKind::SimdMaskedStore { .. }
            | MastExprKind::SimdScatter { .. }
            | MastExprKind::SimdMaskedScatter { .. }
            | MastExprKind::AtomicStore { .. }
            | MastExprKind::Fence { .. }
            | MastExprKind::Memcpy { .. }
            | MastExprKind::Memmove { .. }
            | MastExprKind::Memset { .. } => self.unsupported_expr(expr, "operand position"),
            _ => {
                let temp = self.new_temp_local(expr.ty);
                let Some(init) = self.lower_rvalue(block_id, expr)? else {
                    return Ok(None);
                };
                self.emit_instruction(
                    *block_id,
                    MirInstruction::Let {
                        place: MirPlace::Local(temp),
                        init,
                    },
                );
                Ok(Some(MirOperand::Local(temp)))
            }
        }
    }

    fn lower_operands(
        &mut self,
        block_id: &mut MirBlockId,
        exprs: &[MastExpr],
    ) -> LowerResult<Option<Vec<MirOperand>>> {
        let mut operands = Vec::with_capacity(exprs.len());
        for expr in exprs {
            let Some(operand) = self.lower_expr_to_operand(block_id, expr)? else {
                return Ok(None);
            };
            operands.push(operand);
        }
        Ok(Some(operands))
    }

    fn expr_is_addressable(&self, expr: &MastExpr) -> bool {
        matches!(
            expr.kind,
            MastExprKind::Var(_)
                | MastExprKind::GlobalRef(_)
                | MastExprKind::FieldAccess { .. }
                | MastExprKind::IndexAccess { .. }
                | MastExprKind::Deref(_)
        )
    }

    fn lower_slice_base(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<MirSliceBase>> {
        if self.expr_is_addressable(expr) {
            let Some(place) = self.lower_place(block_id, expr)? else {
                return Ok(None);
            };
            return Ok(Some(MirSliceBase::Place(place)));
        }
        let Some(operand) = self.lower_expr_to_operand(block_id, expr)? else {
            return Ok(None);
        };
        Ok(Some(MirSliceBase::Operand(operand)))
    }

    pub(super) fn lower_expr_into_place(
        &mut self,
        block_id: MirBlockId,
        expr: &MastExpr,
        place: MirPlace,
    ) -> LowerResult<Option<MirBlockId>> {
        match &expr.kind {
            MastExprKind::Block(block) => self.lower_value_block(block_id, block, place),
            MastExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.lower_value_if(block_id, cond, then_branch, else_branch.as_ref(), place),
            MastExprKind::Switch {
                target,
                cases,
                default_case,
            } => self.lower_value_switch(block_id, target, cases, default_case.as_ref(), place),
            _ => {
                let mut current = block_id;
                let Some(value) = self.lower_rvalue(&mut current, expr)? else {
                    return Ok(None);
                };
                self.emit_instruction(
                    current,
                    MirInstruction::Assign {
                        place,
                        op: kernc_ast::AssignmentOperator::Assign,
                        value,
                    },
                );
                Ok(Some(current))
            }
        }
    }

    pub(super) fn lower_place(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<MirPlace>> {
        match &expr.kind {
            MastExprKind::Var(name) => self
                .lookup_local(*name)
                .map(MirPlace::Local)
                .map(Some)
                .ok_or_else(|| {
                    MirLowerError::new(
                        expr.span,
                        format!("unresolved local binding {:?} in MIR place lowering", name),
                    )
                }),
            MastExprKind::GlobalRef(id) => Ok(Some(MirPlace::Global(*id))),
            MastExprKind::Deref(inner) => {
                let Some(inner) = self.lower_expr_to_operand(block_id, inner)? else {
                    return Ok(None);
                };
                Ok(Some(MirPlace::Deref(inner)))
            }
            MastExprKind::FieldAccess {
                lhs,
                struct_id,
                field_idx,
            } => {
                let Some(lhs) = self.lower_place(block_id, lhs)? else {
                    return Ok(None);
                };
                Ok(Some(MirPlace::Field {
                    base: Box::new(lhs),
                    struct_id: *struct_id,
                    field_idx: *field_idx,
                    field_ty: expr.ty,
                }))
            }
            MastExprKind::IndexAccess { lhs, index } => {
                let Some(lhs) = self.lower_place(block_id, lhs)? else {
                    return Ok(None);
                };
                let Some(index) = self.lower_expr_to_operand(block_id, index)? else {
                    return Ok(None);
                };
                Ok(Some(MirPlace::Index {
                    base: Box::new(lhs),
                    index,
                }))
            }
            _ => {
                let temp = self.new_temp_local(expr.ty);
                let Some(end_block) =
                    self.lower_expr_into_place(*block_id, expr, MirPlace::Local(temp))?
                else {
                    return if expr.ty == TypeId::NEVER {
                        Ok(None)
                    } else {
                        self.unsupported_expr(expr, "place position")
                    };
                };
                *block_id = end_block;
                Ok(Some(MirPlace::Local(temp)))
            }
        }
    }

    pub(super) fn lower_assign_instruction(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<MirPlace>> {
        let MastExprKind::Assign { op, lhs, rhs } = &expr.kind else {
            return Ok(None);
        };
        let Some(place) = self.lower_place(block_id, lhs)? else {
            return Ok(None);
        };
        let Some(value) = self.lower_rvalue(block_id, rhs)? else {
            return Ok(None);
        };
        self.emit_instruction(
            *block_id,
            MirInstruction::Assign {
                place: place.clone(),
                op: *op,
                value,
            },
        );
        Ok(Some(place))
    }

    pub(super) fn lower_memory_instruction(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<bool>> {
        let intrinsic = match &expr.kind {
            MastExprKind::Memcpy { dest, src, len } => {
                let Some(dest) = self.lower_expr_to_operand(block_id, dest)? else {
                    return Ok(None);
                };
                let Some(src) = self.lower_expr_to_operand(block_id, src)? else {
                    return Ok(None);
                };
                let Some(len) = self.lower_expr_to_operand(block_id, len)? else {
                    return Ok(None);
                };
                MirMemoryIntrinsic::Copy { dest, src, len }
            }
            MastExprKind::Memmove { dest, src, len } => {
                let Some(dest) = self.lower_expr_to_operand(block_id, dest)? else {
                    return Ok(None);
                };
                let Some(src) = self.lower_expr_to_operand(block_id, src)? else {
                    return Ok(None);
                };
                let Some(len) = self.lower_expr_to_operand(block_id, len)? else {
                    return Ok(None);
                };
                MirMemoryIntrinsic::Move { dest, src, len }
            }
            MastExprKind::Memset { dest, val, len } => {
                let Some(dest) = self.lower_expr_to_operand(block_id, dest)? else {
                    return Ok(None);
                };
                let Some(val) = self.lower_expr_to_operand(block_id, val)? else {
                    return Ok(None);
                };
                let Some(len) = self.lower_expr_to_operand(block_id, len)? else {
                    return Ok(None);
                };
                MirMemoryIntrinsic::Set { dest, val, len }
            }
            _ => return Ok(Some(false)),
        };
        self.emit_instruction(*block_id, MirInstruction::Memory(intrinsic));
        Ok(Some(true))
    }

    pub(super) fn lower_atomic_instruction(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<bool>> {
        let instruction = match &expr.kind {
            MastExprKind::AtomicStore {
                ptr,
                value,
                ordering,
            } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                MirInstruction::AtomicStore {
                    ptr,
                    value,
                    ordering: *ordering,
                }
            }
            MastExprKind::Fence { ordering } => MirInstruction::Fence {
                ordering: *ordering,
            },
            _ => return Ok(Some(false)),
        };
        self.emit_instruction(*block_id, instruction);
        Ok(Some(true))
    }

    pub(super) fn lower_inline_asm_instruction(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<bool>> {
        let MastExprKind::Asm(asm) = &expr.kind else {
            return Ok(Some(false));
        };
        let mut input_args = Vec::with_capacity(asm.input_args.len());
        for input in &asm.input_args {
            let Some(input) = self.lower_expr_to_operand(block_id, input)? else {
                return Ok(None);
            };
            input_args.push(input);
        }
        let mut output_ptrs = Vec::with_capacity(asm.output_ptrs.len());
        for output in &asm.output_ptrs {
            let Some(output) = self.lower_expr_to_operand(block_id, output)? else {
                return Ok(None);
            };
            output_ptrs.push(output);
        }
        self.emit_instruction(
            *block_id,
            MirInstruction::InlineAsm(MirInlineAsm {
                asm_template: asm.asm_template.clone(),
                constraints: asm.constraints.clone(),
                input_args,
                output_ptrs,
                output_tys: asm.output_tys.clone(),
                is_volatile: asm.is_volatile,
            }),
        );
        Ok(Some(true))
    }

    pub(super) fn lower_simd_memory_instruction(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<bool>> {
        let instruction = match &expr.kind {
            MastExprKind::SimdStore { ptr, value, align } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                MirInstruction::SimdStore {
                    ptr,
                    value,
                    align: *align,
                }
            }
            MastExprKind::SimdMaskedStore {
                ptr,
                mask,
                value,
                align,
            } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(mask) = self.lower_expr_to_operand(block_id, mask)? else {
                    return Ok(None);
                };
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                MirInstruction::SimdMaskedStore {
                    ptr,
                    mask,
                    value,
                    align: *align,
                }
            }
            MastExprKind::SimdScatter {
                ptr,
                indices,
                value,
            } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(indices) = self.lower_expr_to_operand(block_id, indices)? else {
                    return Ok(None);
                };
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                MirInstruction::SimdScatter {
                    ptr,
                    indices,
                    value,
                }
            }
            MastExprKind::SimdMaskedScatter {
                ptr,
                indices,
                mask,
                value,
            } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(indices) = self.lower_expr_to_operand(block_id, indices)? else {
                    return Ok(None);
                };
                let Some(mask) = self.lower_expr_to_operand(block_id, mask)? else {
                    return Ok(None);
                };
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                MirInstruction::SimdMaskedScatter {
                    ptr,
                    indices,
                    mask,
                    value,
                }
            }
            _ => return Ok(Some(false)),
        };
        self.emit_instruction(*block_id, instruction);
        Ok(Some(true))
    }
}
