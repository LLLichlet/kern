use super::*;

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(super) fn lower_builtin_operator_intrinsic(
        &mut self,
        fn_id: DefId,
        arg_masts: &mut Vec<MastExpr>,
    ) -> Option<MastExprKind> {
        let Def::Function(func) = &self.ctx.defs[fn_id.0 as usize] else {
            return None;
        };
        let parent_impl_id = func.parent?;
        let Def::Impl(impl_def) = &self.ctx.defs[parent_impl_id.0 as usize] else {
            return None;
        };
        let trait_node = impl_def.trait_type.as_ref()?;
        let trait_ty = self
            .ctx
            .node_types
            .get(&trait_node.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let norm_trait_ty = self.ctx.type_registry.normalize(trait_ty);
        let TypeKind::TraitObject(trait_def_id, _, _) =
            self.ctx.type_registry.get(norm_trait_ty).clone()
        else {
            return None;
        };
        let Def::Trait(trait_def) = &self.ctx.defs[trait_def_id.0 as usize] else {
            return None;
        };
        if !trait_def.is_builtin {
            return None;
        }

        let trait_name = self.ctx.resolve(trait_def.name);
        match trait_name {
            "Eq" => Some(MastExprKind::Binary {
                op: ast::BinaryOperator::Equal,
                lhs: Box::new(arg_masts.remove(0)),
                rhs: Box::new(arg_masts.remove(0)),
            }),
            "Lt" => Some(MastExprKind::Binary {
                op: ast::BinaryOperator::LessThan,
                lhs: Box::new(arg_masts.remove(0)),
                rhs: Box::new(arg_masts.remove(0)),
            }),
            "Le" => Some(MastExprKind::Binary {
                op: ast::BinaryOperator::LessOrEqual,
                lhs: Box::new(arg_masts.remove(0)),
                rhs: Box::new(arg_masts.remove(0)),
            }),
            "Gt" => Some(MastExprKind::Binary {
                op: ast::BinaryOperator::GreaterThan,
                lhs: Box::new(arg_masts.remove(0)),
                rhs: Box::new(arg_masts.remove(0)),
            }),
            "Ge" => Some(MastExprKind::Binary {
                op: ast::BinaryOperator::GreaterOrEqual,
                lhs: Box::new(arg_masts.remove(0)),
                rhs: Box::new(arg_masts.remove(0)),
            }),
            "Add" => Some(MastExprKind::Binary {
                op: ast::BinaryOperator::Add,
                lhs: Box::new(arg_masts.remove(0)),
                rhs: Box::new(arg_masts.remove(0)),
            }),
            "Sub" => Some(MastExprKind::Binary {
                op: ast::BinaryOperator::Subtract,
                lhs: Box::new(arg_masts.remove(0)),
                rhs: Box::new(arg_masts.remove(0)),
            }),
            "Mul" => Some(MastExprKind::Binary {
                op: ast::BinaryOperator::Multiply,
                lhs: Box::new(arg_masts.remove(0)),
                rhs: Box::new(arg_masts.remove(0)),
            }),
            "Div" => Some(MastExprKind::Binary {
                op: ast::BinaryOperator::Divide,
                lhs: Box::new(arg_masts.remove(0)),
                rhs: Box::new(arg_masts.remove(0)),
            }),
            "Rem" => Some(MastExprKind::Binary {
                op: ast::BinaryOperator::Modulo,
                lhs: Box::new(arg_masts.remove(0)),
                rhs: Box::new(arg_masts.remove(0)),
            }),
            "BitAnd" => Some(MastExprKind::Binary {
                op: ast::BinaryOperator::BitwiseAnd,
                lhs: Box::new(arg_masts.remove(0)),
                rhs: Box::new(arg_masts.remove(0)),
            }),
            "BitOr" => Some(MastExprKind::Binary {
                op: ast::BinaryOperator::BitwiseOr,
                lhs: Box::new(arg_masts.remove(0)),
                rhs: Box::new(arg_masts.remove(0)),
            }),
            "BitXor" => Some(MastExprKind::Binary {
                op: ast::BinaryOperator::BitwiseXor,
                lhs: Box::new(arg_masts.remove(0)),
                rhs: Box::new(arg_masts.remove(0)),
            }),
            "Shl" => Some(MastExprKind::Binary {
                op: ast::BinaryOperator::ShiftLeft,
                lhs: Box::new(arg_masts.remove(0)),
                rhs: Box::new(arg_masts.remove(0)),
            }),
            "Shr" => Some(MastExprKind::Binary {
                op: ast::BinaryOperator::ShiftRight,
                lhs: Box::new(arg_masts.remove(0)),
                rhs: Box::new(arg_masts.remove(0)),
            }),
            "Neg" => Some(MastExprKind::Unary {
                op: ast::UnaryOperator::Negate,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "BitNot" => Some(MastExprKind::Unary {
                op: ast::UnaryOperator::BitwiseNot,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "Not" => Some(MastExprKind::Unary {
                op: ast::UnaryOperator::LogicalNot,
                operand: Box::new(arg_masts.remove(0)),
            }),
            _ => None,
        }
    }

    pub(super) fn lower_intrinsic_call(
        &mut self,
        fn_id: DefId,
        callee_ty: TypeId,
        args: &[Expr],
        arg_masts: &mut Vec<MastExpr>,
        span: Span,
    ) -> Option<MastExprKind> {
        let (is_intrinsic, name_id) = match &self.ctx.defs[fn_id.0 as usize] {
            Def::Function(f) => (f.is_intrinsic, f.name),
            _ => return None,
        };
        if !is_intrinsic {
            return None;
        }

        if let Some(operator_kind) = self.lower_builtin_operator_intrinsic(fn_id, arg_masts) {
            return Some(operator_kind);
        }

        let name_str = self.ctx.resolve(name_id);
        match name_str {
            "@loc" => {
                let result_ty = self.intrinsic_return_type(fn_id, callee_ty);
                Some(self.lower_loc_intrinsic(result_ty, span))
            }
            "@sizeOf" => {
                let target_ty = self.intrinsic_generic_arg(callee_ty, 0);
                let mut le = LayoutEngine::new(self.ctx);
                Some(MastExprKind::Integer(
                    le.compute_type_size(target_ty) as u128
                ))
            }
            "@alignOf" => {
                let target_ty = self.intrinsic_generic_arg(callee_ty, 0);
                let mut le = LayoutEngine::new(self.ctx);
                Some(MastExprKind::Integer(
                    le.compute_type_align(target_ty) as u128
                ))
            }
            "@unreachable" => Some(MastExprKind::Unreachable),
            "@popCount" => Some(MastExprKind::BitIntrinsic {
                kind: BitIntrinsicKind::PopCount,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@clz" => Some(MastExprKind::BitIntrinsic {
                kind: BitIntrinsicKind::Clz,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@ctz" => Some(MastExprKind::BitIntrinsic {
                kind: BitIntrinsicKind::Ctz,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@bswap" => Some(MastExprKind::BitIntrinsic {
                kind: BitIntrinsicKind::Bswap,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@simdAbs" => Some(MastExprKind::SimdUnaryIntrinsic {
                kind: SimdUnaryIntrinsicKind::Abs,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@simdSqrt" => Some(MastExprKind::SimdUnaryIntrinsic {
                kind: SimdUnaryIntrinsicKind::Sqrt,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@simdFloor" => Some(MastExprKind::SimdUnaryIntrinsic {
                kind: SimdUnaryIntrinsicKind::Floor,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@simdCeil" => Some(MastExprKind::SimdUnaryIntrinsic {
                kind: SimdUnaryIntrinsicKind::Ceil,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@simdTrunc" => Some(MastExprKind::SimdUnaryIntrinsic {
                kind: SimdUnaryIntrinsicKind::Trunc,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@simdRound" => Some(MastExprKind::SimdUnaryIntrinsic {
                kind: SimdUnaryIntrinsicKind::Round,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@simdAny" => Some(MastExprKind::SimdAny {
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@simdAll" => Some(MastExprKind::SimdAll {
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@simdBitmask" => Some(MastExprKind::SimdBitmask {
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@simdSplat" => Some(MastExprKind::SimdSplat {
                value: Box::new(arg_masts.remove(0)),
            }),
            "@simdCast" => Some(MastExprKind::SimdCast {
                value: Box::new(arg_masts.remove(0)),
            }),
            "@simdBitcast" => Some(MastExprKind::SimdBitcast {
                value: Box::new(arg_masts.remove(0)),
            }),
            "@simdReduceAdd" => Some(MastExprKind::SimdReduce {
                kind: SimdReduceKind::Add,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@simdReduceMul" => Some(MastExprKind::SimdReduce {
                kind: SimdReduceKind::Mul,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@simdReduceAnd" => Some(MastExprKind::SimdReduce {
                kind: SimdReduceKind::And,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@simdReduceOr" => Some(MastExprKind::SimdReduce {
                kind: SimdReduceKind::Or,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@simdReduceXor" => Some(MastExprKind::SimdReduce {
                kind: SimdReduceKind::Xor,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@simdReduceMin" => Some(MastExprKind::SimdReduce {
                kind: SimdReduceKind::Min,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@simdReduceMax" => Some(MastExprKind::SimdReduce {
                kind: SimdReduceKind::Max,
                operand: Box::new(arg_masts.remove(0)),
            }),
            "@simdMin" => Some(MastExprKind::SimdBinaryIntrinsic {
                kind: SimdBinaryIntrinsicKind::Min,
                lhs: Box::new(arg_masts.remove(0)),
                rhs: Box::new(arg_masts.remove(0)),
            }),
            "@simdMax" => Some(MastExprKind::SimdBinaryIntrinsic {
                kind: SimdBinaryIntrinsicKind::Max,
                lhs: Box::new(arg_masts.remove(0)),
                rhs: Box::new(arg_masts.remove(0)),
            }),
            "@simdClamp" => {
                let value = arg_masts.remove(0);
                let lo = arg_masts.remove(0);
                let hi = arg_masts.remove(0);
                let inner_ty = value.ty;
                let inner_span = value.span;
                let clamped_low = MastExpr::new(
                    inner_ty,
                    MastExprKind::SimdBinaryIntrinsic {
                        kind: SimdBinaryIntrinsicKind::Max,
                        lhs: Box::new(value),
                        rhs: Box::new(lo),
                    },
                    inner_span,
                );
                Some(MastExprKind::SimdBinaryIntrinsic {
                    kind: SimdBinaryIntrinsicKind::Min,
                    lhs: Box::new(clamped_low),
                    rhs: Box::new(hi),
                })
            }
            "@simdSelect" => Some(MastExprKind::SimdSelect {
                mask: Box::new(arg_masts.remove(0)),
                on_true: Box::new(arg_masts.remove(0)),
                on_false: Box::new(arg_masts.remove(0)),
            }),
            "@simdShuffle" => Some(MastExprKind::SimdShuffle {
                lhs: Box::new(arg_masts.remove(0)),
                rhs: Box::new(arg_masts.remove(0)),
                indices: self.simd_shuffle_indices_arg(&args[2]),
            }),
            "@simdSwizzle" => {
                let value = arg_masts.remove(0);
                Some(MastExprKind::SimdShuffle {
                    lhs: Box::new(value.clone()),
                    rhs: Box::new(value),
                    indices: self.simd_shuffle_indices_arg(&args[1]),
                })
            }
            "@simdReverse" => {
                let value = arg_masts.remove(0);
                let lanes = self
                    .ctx
                    .type_registry
                    .simd_info(value.ty)
                    .map(|(_, lanes)| lanes)
                    .unwrap_or(0);
                Some(MastExprKind::SimdShuffle {
                    lhs: Box::new(value.clone()),
                    rhs: Box::new(value),
                    indices: self.simd_reverse_indices(lanes),
                })
            }
            "@simdRotateLeft" => {
                let value = arg_masts.remove(0);
                let lanes = self
                    .ctx
                    .type_registry
                    .simd_info(value.ty)
                    .map(|(_, lanes)| lanes)
                    .unwrap_or(1);
                let amount = self.simd_rotate_amount_arg(&args[1], lanes);
                Some(MastExprKind::SimdShuffle {
                    lhs: Box::new(value.clone()),
                    rhs: Box::new(value),
                    indices: self.simd_rotate_left_indices(lanes, amount),
                })
            }
            "@simdRotateRight" => {
                let value = arg_masts.remove(0);
                let lanes = self
                    .ctx
                    .type_registry
                    .simd_info(value.ty)
                    .map(|(_, lanes)| lanes)
                    .unwrap_or(1);
                let amount = self.simd_rotate_amount_arg(&args[1], lanes);
                Some(MastExprKind::SimdShuffle {
                    lhs: Box::new(value.clone()),
                    rhs: Box::new(value),
                    indices: self.simd_rotate_right_indices(lanes, amount),
                })
            }
            "@simdInterleaveLo" | "@simdZipLo" => {
                let lhs = arg_masts.remove(0);
                let rhs = arg_masts.remove(0);
                let lanes = self
                    .ctx
                    .type_registry
                    .simd_info(lhs.ty)
                    .map(|(_, lanes)| lanes)
                    .unwrap_or(0);
                Some(MastExprKind::SimdShuffle {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    indices: self.simd_interleave_indices(lanes, false),
                })
            }
            "@simdInterleaveHi" | "@simdZipHi" => {
                let lhs = arg_masts.remove(0);
                let rhs = arg_masts.remove(0);
                let lanes = self
                    .ctx
                    .type_registry
                    .simd_info(lhs.ty)
                    .map(|(_, lanes)| lanes)
                    .unwrap_or(0);
                Some(MastExprKind::SimdShuffle {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    indices: self.simd_interleave_indices(lanes, true),
                })
            }
            "@simdConcatLo" => {
                let lhs = arg_masts.remove(0);
                let rhs = arg_masts.remove(0);
                let lanes = self
                    .ctx
                    .type_registry
                    .simd_info(lhs.ty)
                    .map(|(_, lanes)| lanes)
                    .unwrap_or(0);
                Some(MastExprKind::SimdShuffle {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    indices: self.simd_concat_indices(lanes, false),
                })
            }
            "@simdConcatHi" => {
                let lhs = arg_masts.remove(0);
                let rhs = arg_masts.remove(0);
                let lanes = self
                    .ctx
                    .type_registry
                    .simd_info(lhs.ty)
                    .map(|(_, lanes)| lanes)
                    .unwrap_or(0);
                Some(MastExprKind::SimdShuffle {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    indices: self.simd_concat_indices(lanes, true),
                })
            }
            "@simdDeinterleaveLo" | "@simdUnzipLo" => {
                let lhs = arg_masts.remove(0);
                let rhs = arg_masts.remove(0);
                let lanes = self
                    .ctx
                    .type_registry
                    .simd_info(lhs.ty)
                    .map(|(_, lanes)| lanes)
                    .unwrap_or(0);
                Some(MastExprKind::SimdShuffle {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    indices: self.simd_deinterleave_indices(lanes, false),
                })
            }
            "@simdDeinterleaveHi" | "@simdUnzipHi" => {
                let lhs = arg_masts.remove(0);
                let rhs = arg_masts.remove(0);
                let lanes = self
                    .ctx
                    .type_registry
                    .simd_info(lhs.ty)
                    .map(|(_, lanes)| lanes)
                    .unwrap_or(0);
                Some(MastExprKind::SimdShuffle {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    indices: self.simd_deinterleave_indices(lanes, true),
                })
            }
            "@simdLowHalf" | "@simdHighHalf" => {
                let value = arg_masts.remove(0);
                let full_lanes = self
                    .ctx
                    .type_registry
                    .simd_info(value.ty)
                    .map(|(_, lanes)| lanes)
                    .unwrap_or(0);
                Some(MastExprKind::SimdShuffle {
                    lhs: Box::new(value.clone()),
                    rhs: Box::new(value),
                    indices: self
                        .simd_extract_half_indices(full_lanes, name_str == "@simdHighHalf"),
                })
            }
            "@simdWithLowHalf" | "@simdWithHighHalf" => Some(MastExprKind::SimdInsertHalf {
                base: Box::new(arg_masts.remove(0)),
                half: Box::new(arg_masts.remove(0)),
                high_half: name_str == "@simdWithHighHalf",
            }),
            "@simdLoad" => Some(MastExprKind::SimdLoad {
                ptr: Box::new(arg_masts.remove(0)),
                align: self.simd_align_arg(&args[1]),
            }),
            "@simdStore" => Some(MastExprKind::SimdStore {
                ptr: Box::new(arg_masts.remove(0)),
                value: Box::new(arg_masts.remove(0)),
                align: self.simd_align_arg(&args[2]),
            }),
            "@simdMaskedLoad" => Some(MastExprKind::SimdMaskedLoad {
                ptr: Box::new(arg_masts.remove(0)),
                mask: Box::new(arg_masts.remove(0)),
                or_else: Box::new(arg_masts.remove(0)),
                align: self.simd_align_arg(&args[3]),
            }),
            "@simdMaskedStore" => Some(MastExprKind::SimdMaskedStore {
                ptr: Box::new(arg_masts.remove(0)),
                mask: Box::new(arg_masts.remove(0)),
                value: Box::new(arg_masts.remove(0)),
                align: self.simd_align_arg(&args[3]),
            }),
            "@simdGather" => Some(MastExprKind::SimdGather {
                ptr: Box::new(arg_masts.remove(0)),
                indices: Box::new(arg_masts.remove(0)),
            }),
            "@simdScatter" => Some(MastExprKind::SimdScatter {
                ptr: Box::new(arg_masts.remove(0)),
                indices: Box::new(arg_masts.remove(0)),
                value: Box::new(arg_masts.remove(0)),
            }),
            "@simdMaskedGather" => Some(MastExprKind::SimdMaskedGather {
                ptr: Box::new(arg_masts.remove(0)),
                indices: Box::new(arg_masts.remove(0)),
                mask: Box::new(arg_masts.remove(0)),
                or_else: Box::new(arg_masts.remove(0)),
            }),
            "@simdMaskedScatter" => Some(MastExprKind::SimdMaskedScatter {
                ptr: Box::new(arg_masts.remove(0)),
                indices: Box::new(arg_masts.remove(0)),
                mask: Box::new(arg_masts.remove(0)),
                value: Box::new(arg_masts.remove(0)),
            }),
            "@trap" => Some(MastExprKind::Trap),
            "@atomicLoad" => Some(MastExprKind::AtomicLoad {
                ptr: Box::new(arg_masts.remove(0)),
                ordering: self.atomic_ordering_arg(&args[1]),
            }),
            "@atomicStore" => Some(MastExprKind::AtomicStore {
                ptr: Box::new(arg_masts.remove(0)),
                value: Box::new(arg_masts.remove(0)),
                ordering: self.atomic_ordering_arg(&args[2]),
            }),
            "@atomicCas" | "@atomicCasWeak" => {
                let is_weak = name_str == "@atomicCasWeak";
                let result_ty = self.intrinsic_return_type(fn_id, callee_ty);
                let norm_result_ty = self.ctx.type_registry.normalize(result_ty);
                if matches!(
                    self.ctx.type_registry.get(norm_result_ty),
                    TypeKind::AnonymousStruct(..)
                ) {
                    self.instantiate_anon_struct(norm_result_ty);
                }
                Some(MastExprKind::AtomicCas {
                    weak: is_weak,
                    ptr: Box::new(arg_masts.remove(0)),
                    expected: Box::new(arg_masts.remove(0)),
                    desired: Box::new(arg_masts.remove(0)),
                    success: self.atomic_ordering_arg(&args[3]),
                    failure: self.atomic_ordering_arg(&args[4]),
                })
            }
            "@atomicXchg" => Some(MastExprKind::AtomicRmw {
                op: AtomicRmwOp::Xchg,
                ptr: Box::new(arg_masts.remove(0)),
                value: Box::new(arg_masts.remove(0)),
                ordering: self.atomic_ordering_arg(&args[2]),
            }),
            "@atomicRmwAdd" => Some(MastExprKind::AtomicRmw {
                op: AtomicRmwOp::Add,
                ptr: Box::new(arg_masts.remove(0)),
                value: Box::new(arg_masts.remove(0)),
                ordering: self.atomic_ordering_arg(&args[2]),
            }),
            "@atomicRmwSub" => Some(MastExprKind::AtomicRmw {
                op: AtomicRmwOp::Sub,
                ptr: Box::new(arg_masts.remove(0)),
                value: Box::new(arg_masts.remove(0)),
                ordering: self.atomic_ordering_arg(&args[2]),
            }),
            "@atomicRmwAnd" => Some(MastExprKind::AtomicRmw {
                op: AtomicRmwOp::And,
                ptr: Box::new(arg_masts.remove(0)),
                value: Box::new(arg_masts.remove(0)),
                ordering: self.atomic_ordering_arg(&args[2]),
            }),
            "@atomicRmwNand" => Some(MastExprKind::AtomicRmw {
                op: AtomicRmwOp::Nand,
                ptr: Box::new(arg_masts.remove(0)),
                value: Box::new(arg_masts.remove(0)),
                ordering: self.atomic_ordering_arg(&args[2]),
            }),
            "@atomicRmwOr" => Some(MastExprKind::AtomicRmw {
                op: AtomicRmwOp::Or,
                ptr: Box::new(arg_masts.remove(0)),
                value: Box::new(arg_masts.remove(0)),
                ordering: self.atomic_ordering_arg(&args[2]),
            }),
            "@atomicRmwXor" => Some(MastExprKind::AtomicRmw {
                op: AtomicRmwOp::Xor,
                ptr: Box::new(arg_masts.remove(0)),
                value: Box::new(arg_masts.remove(0)),
                ordering: self.atomic_ordering_arg(&args[2]),
            }),
            "@atomicRmwMax" => Some(MastExprKind::AtomicRmw {
                op: AtomicRmwOp::Max,
                ptr: Box::new(arg_masts.remove(0)),
                value: Box::new(arg_masts.remove(0)),
                ordering: self.atomic_ordering_arg(&args[2]),
            }),
            "@atomicRmwMin" => Some(MastExprKind::AtomicRmw {
                op: AtomicRmwOp::Min,
                ptr: Box::new(arg_masts.remove(0)),
                value: Box::new(arg_masts.remove(0)),
                ordering: self.atomic_ordering_arg(&args[2]),
            }),
            "@atomicRmwUMax" => Some(MastExprKind::AtomicRmw {
                op: AtomicRmwOp::UMax,
                ptr: Box::new(arg_masts.remove(0)),
                value: Box::new(arg_masts.remove(0)),
                ordering: self.atomic_ordering_arg(&args[2]),
            }),
            "@atomicRmwUMin" => Some(MastExprKind::AtomicRmw {
                op: AtomicRmwOp::UMin,
                ptr: Box::new(arg_masts.remove(0)),
                value: Box::new(arg_masts.remove(0)),
                ordering: self.atomic_ordering_arg(&args[2]),
            }),
            "@fence" => Some(MastExprKind::Fence {
                ordering: self.atomic_ordering_arg(&args[0]),
            }),
            "@breakpoint" => Some(MastExprKind::Breakpoint),
            "@memcpy" => Some(MastExprKind::Memcpy {
                dest: Box::new(arg_masts.remove(0)),
                src: Box::new(arg_masts.remove(0)),
                len: Box::new(arg_masts.remove(0)),
            }),
            "@memmove" => Some(MastExprKind::Memmove {
                dest: Box::new(arg_masts.remove(0)),
                src: Box::new(arg_masts.remove(0)),
                len: Box::new(arg_masts.remove(0)),
            }),
            "@memset" => Some(MastExprKind::Memset {
                dest: Box::new(arg_masts.remove(0)),
                val: Box::new(arg_masts.remove(0)),
                len: Box::new(arg_masts.remove(0)),
            }),
            _ => None,
        }
    }

    pub(super) fn intrinsic_generic_arg(&mut self, callee_ty: TypeId, index: usize) -> TypeId {
        match self.ctx.type_registry.get(callee_ty) {
            TypeKind::FnDef(_, args) => args
                .get(index)
                .copied()
                .and_then(kernc_sema::ty::GenericArg::as_type)
                .unwrap_or(TypeId::ERROR),
            _ => TypeId::ERROR,
        }
    }

    pub(super) fn intrinsic_return_type(&mut self, fn_id: DefId, callee_ty: TypeId) -> TypeId {
        let Some(func) = (match &self.ctx.defs[fn_id.0 as usize] {
            Def::Function(func) => Some(func.clone()),
            _ => None,
        }) else {
            return TypeId::ERROR;
        };

        let Some(sig_ty) = func.resolved_sig else {
            return TypeId::ERROR;
        };
        let TypeKind::Function { ret, .. } = self.ctx.type_registry.get(sig_ty).clone() else {
            return TypeId::ERROR;
        };

        let fn_args = match self.ctx.type_registry.get(callee_ty).clone() {
            TypeKind::FnDef(_, args) => args,
            _ => Vec::new(),
        };

        if func.generics.is_empty() || fn_args.len() != func.generics.len() {
            return ret;
        }

        let mut subst_map = HashMap::new();
        for (param, arg) in func.generics.iter().zip(fn_args.iter().copied()) {
            subst_map.insert(param.name, arg);
        }

        self.substitute_type_with_map(ret, &subst_map)
    }

    pub(super) fn atomic_ordering_arg(&mut self, arg: &Expr) -> AtomicOrdering {
        if let Some(&ordering) = self.ctx.atomic_orderings.get(&arg.id) {
            return ordering;
        }

        let mut evaluator = ConstEvaluator::new(self.ctx);
        match evaluator.eval_inner(arg, 0) {
            Ok(ConstValue::Int(value)) => AtomicOrdering::from_abi_const(value).unwrap_or_else(|| {
                self.ctx.emit_ice(
                    arg.span,
                    format!(
                        "Kern ICE (Lowering): invalid atomic ordering constant `{}` passed semantic validation.",
                        value
                    ),
                );
                AtomicOrdering::SeqCst
            }),
            _ => {
                self.ctx.emit_ice(
                    arg.span,
                    "Kern ICE (Lowering): atomic ordering argument was not reduced to a compile-time integer.",
                );
                AtomicOrdering::SeqCst
            }
        }
    }

    pub(super) fn simd_align_arg(&mut self, arg: &Expr) -> u32 {
        let mut evaluator = ConstEvaluator::new(self.ctx);
        match evaluator.eval_usize(arg) {
            Ok(value) => u32::try_from(value).unwrap_or_else(|_| {
                self.ctx.emit_ice(
                    arg.span,
                    format!(
                        "Kern ICE (Lowering): SIMD alignment `{}` does not fit into u32.",
                        value
                    ),
                );
                1
            }),
            Err(_) => {
                self.ctx.emit_ice(
                    arg.span,
                    "Kern ICE (Lowering): SIMD alignment argument was not reduced to a compile-time integer.",
                );
                1
            }
        }
    }

    pub(super) fn simd_shuffle_indices_arg(&mut self, arg: &Expr) -> Vec<u32> {
        let mut evaluator = ConstEvaluator::new(self.ctx);
        match evaluator.eval_inner(arg, 0) {
            Ok(ConstValue::Array(values)) => values
                .into_iter()
                .map(|value| match value {
                    ConstValue::Int(idx) => u32::try_from(idx).unwrap_or_else(|_| {
                        self.ctx.emit_ice(
                            arg.span,
                            format!(
                                "Kern ICE (Lowering): SIMD shuffle index `{}` did not survive semantic validation.",
                                idx
                            ),
                        );
                        0
                    }),
                    other => {
                        self.ctx.emit_ice(
                            arg.span,
                            format!(
                                "Kern ICE (Lowering): SIMD shuffle indices must be integers, found `{:?}`.",
                                other
                            ),
                        );
                        0
                    }
                })
                .collect(),
            Ok(other) => {
                self.ctx.emit_ice(
                    arg.span,
                    format!(
                        "Kern ICE (Lowering): SIMD shuffle indices expected a constant array, found `{:?}`.",
                        other
                    ),
                );
                Vec::new()
            }
            Err(_) => {
                self.ctx.emit_ice(
                    arg.span,
                    "Kern ICE (Lowering): SIMD shuffle indices were not reduced to compile-time constants.",
                );
                Vec::new()
            }
        }
    }

    pub(super) fn simd_rotate_amount_arg(&mut self, arg: &Expr, lanes: u16) -> u32 {
        let mut evaluator = ConstEvaluator::new(self.ctx);
        match evaluator.eval_usize(arg) {
            Ok(value) => (value % lanes as u64) as u32,
            Err(_) => {
                self.ctx.emit_ice(
                    arg.span,
                    "Kern ICE (Lowering): SIMD rotate amount argument was not reduced to a compile-time integer.",
                );
                0
            }
        }
    }

    pub(super) fn simd_duplicate_shuffle_indices(
        &mut self,
        lanes: u16,
        indices: impl Fn(u32) -> u32,
    ) -> Vec<u32> {
        (0..lanes as u32).map(indices).collect()
    }

    pub(super) fn simd_reverse_indices(&mut self, lanes: u16) -> Vec<u32> {
        self.simd_duplicate_shuffle_indices(lanes, |i| lanes as u32 - 1 - i)
    }

    pub(super) fn simd_rotate_left_indices(&mut self, lanes: u16, amount: u32) -> Vec<u32> {
        self.simd_duplicate_shuffle_indices(lanes, |i| i + amount)
    }

    pub(super) fn simd_rotate_right_indices(&mut self, lanes: u16, amount: u32) -> Vec<u32> {
        let lanes_u32 = lanes as u32;
        self.simd_duplicate_shuffle_indices(lanes, |i| i + ((lanes_u32 - amount) % lanes_u32))
    }

    pub(super) fn simd_interleave_indices(&mut self, lanes: u16, high_half: bool) -> Vec<u32> {
        let half = lanes as u32 / 2;
        let base = if high_half { half } else { 0 };
        (0..half)
            .flat_map(|i| [base + i, lanes as u32 + base + i])
            .collect()
    }

    pub(super) fn simd_concat_indices(&mut self, lanes: u16, high_half: bool) -> Vec<u32> {
        let half = lanes as u32 / 2;
        let base = if high_half { half } else { 0 };
        (0..half)
            .map(|i| base + i)
            .chain((0..half).map(|i| lanes as u32 + base + i))
            .collect()
    }

    pub(super) fn simd_deinterleave_indices(&mut self, lanes: u16, odd_lanes: bool) -> Vec<u32> {
        let step = 2;
        let start = if odd_lanes { 1 } else { 0 };
        let count = lanes as u32 / 2;
        (0..count)
            .map(|i| start + i * step)
            .chain((0..count).map(|i| lanes as u32 + start + i * step))
            .collect()
    }

    pub(super) fn simd_extract_half_indices(&mut self, lanes: u16, high_half: bool) -> Vec<u32> {
        let half = lanes as u32 / 2;
        let start = if high_half { half } else { 0 };
        (0..half).map(|i| start + i).collect()
    }
}
