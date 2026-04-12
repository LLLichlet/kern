use super::Lowerer;
use std::collections::HashMap;

use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_mast::*;
use kernc_sema::LayoutEngine;
use kernc_sema::checker::{ConstEvaluator, ConstValue, Substituter};
use kernc_sema::def::{Def, DefId};
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{AtomicOrdering, AtomicRmwOp, NodeId, Span, SymbolId};

pub(crate) struct DynamicDispatchCall {
    pub(crate) field: SymbolId,
    pub(crate) recv_trait_ty: TypeId,
    pub(crate) owner_trait_ty: TypeId,
    pub(crate) norm_callee: TypeId,
    pub(crate) span: Span,
}

pub(crate) struct MethodCallSite {
    pub(crate) field: SymbolId,
    pub(crate) norm_callee: TypeId,
    pub(crate) expected_self_ty: Option<TypeId>,
    pub(crate) span: Span,
}

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    fn intrinsic_name_for_lowering(&mut self, callee_ty: TypeId) -> Option<String> {
        let norm = self.ctx.type_registry.normalize(callee_ty);
        let TypeKind::FnDef(def_id, _) = self.ctx.type_registry.get(norm).clone() else {
            return None;
        };
        let Def::Function(func) = &self.ctx.defs[def_id.0 as usize] else {
            return None;
        };
        if !func.is_intrinsic {
            return None;
        }
        Some(self.ctx.resolve(func.name).to_string())
    }

    fn builtin_trait_name(&mut self, trait_ty: TypeId) -> Option<String> {
        let norm = self.ctx.type_registry.normalize(trait_ty);
        let TypeKind::TraitObject(def_id, _) = self.ctx.type_registry.get(norm).clone() else {
            return None;
        };
        let Def::Trait(trait_def) = &self.ctx.defs[def_id.0 as usize] else {
            return None;
        };
        if !trait_def.is_builtin {
            return None;
        }
        Some(self.ctx.resolve(trait_def.name).to_string())
    }

    fn is_pure_enum_value_type(&mut self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm).clone() {
            TypeKind::Enum(def_id, _) => {
                let Def::Enum(def) = &self.ctx.defs[def_id.0 as usize] else {
                    return false;
                };
                self.is_pure_enum(def)
            }
            TypeKind::AnonymousEnum(anon) => anon
                .variants
                .iter()
                .all(|variant| variant.payload_ty.is_none()),
            _ => false,
        }
    }

    fn type_contains_generic_placeholders(&mut self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm).clone() {
            TypeKind::Param(_) | TypeKind::TypeVar(_) => true,
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::Alias(_, elem)
            | TypeKind::AnonymousEnumPayload(elem) => self.type_contains_generic_placeholders(elem),
            TypeKind::Array { elem, .. } | TypeKind::ArrayInfer { elem, .. } => {
                self.type_contains_generic_placeholders(elem)
            }
            TypeKind::Def(_, args)
            | TypeKind::Enum(_, args)
            | TypeKind::EnumPayload(_, args)
            | TypeKind::TraitObject(_, args)
            | TypeKind::FnDef(_, args) => args
                .into_iter()
                .any(|arg| self.type_contains_generic_placeholders(arg)),
            TypeKind::Function { params, ret, .. } => {
                params
                    .into_iter()
                    .any(|param| self.type_contains_generic_placeholders(param))
                    || self.type_contains_generic_placeholders(ret)
            }
            TypeKind::ClosureInterface { params, ret } => {
                params
                    .into_iter()
                    .any(|param| self.type_contains_generic_placeholders(param))
                    || self.type_contains_generic_placeholders(ret)
            }
            TypeKind::AnonymousState {
                captures,
                params,
                ret,
                ..
            } => {
                captures
                    .into_iter()
                    .any(|capture| self.type_contains_generic_placeholders(capture))
                    || params
                        .into_iter()
                        .any(|param| self.type_contains_generic_placeholders(param))
                    || self.type_contains_generic_placeholders(ret)
            }
            TypeKind::AnonymousStruct(_, fields) | TypeKind::AnonymousUnion(_, fields) => fields
                .into_iter()
                .any(|field| self.type_contains_generic_placeholders(field.ty)),
            TypeKind::AnonymousEnum(anon) => anon
                .variants
                .into_iter()
                .filter_map(|variant| variant.payload_ty)
                .any(|payload_ty| self.type_contains_generic_placeholders(payload_ty)),
            TypeKind::Primitive(_)
            | TypeKind::Simd { .. }
            | TypeKind::Error
            | TypeKind::Module(_) => false,
        }
    }

    fn maybe_lower_asm_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        subst_map: &HashMap<SymbolId, TypeId>,
        span: Span,
    ) -> Option<MastExprKind> {
        let ExprKind::Identifier(sym) = &callee.kind else {
            return None;
        };

        if self.ctx.resolve(*sym) == "@asm" {
            Some(self.lower_asm_call(args, subst_map, span))
        } else {
            None
        }
    }

    fn detect_method_call(
        &mut self,
        callee: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> Option<(NodeId, SymbolId, MastExpr)> {
        let ExprKind::FieldAccess { lhs, field, .. } = &callee.kind else {
            return None;
        };

        let lhs_ty = self
            .ctx
            .node_types
            .get(&lhs.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let norm_lhs = self.ctx.type_registry.normalize(lhs_ty);
        if matches!(self.ctx.type_registry.get(norm_lhs), TypeKind::Module(_)) {
            return None;
        }

        let callee_ty = self
            .ctx
            .node_types
            .get(&callee.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let norm_callee = self.ctx.type_registry.normalize(callee_ty);
        if !matches!(
            self.ctx.type_registry.get(norm_callee),
            TypeKind::FnDef(..) | TypeKind::Function { .. }
        ) {
            return None;
        }

        Some((callee.id, *field, self.lower_expr(lhs, subst_map, None)))
    }

    fn asm_config_fields<'b>(
        &mut self,
        args: &'b [Expr],
        span: Span,
    ) -> Option<&'b [ast::StructFieldInit]> {
        let Some(config_arg) = args.first() else {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Lowering): `@asm` lowering expected one configuration argument.",
            );
            return None;
        };

        if let ExprKind::DataInit {
            literal: ast::DataLiteralKind::Struct(fields),
            ..
        } = &config_arg.kind
        {
            Some(fields)
        } else {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Lowering): `@asm` macro argument must be a structural data literal (e.g. `.{ ... }`). Sema failed to validate this.",
            );
            None
        }
    }

    fn lower_asm_template(&mut self, value: &Expr) -> Option<String> {
        match &value.kind {
            ExprKind::String(s) => Some(s.clone()),
            ExprKind::DataInit {
                literal: ast::DataLiteralKind::Array(elems),
                ..
            } => {
                let mut lines = Vec::new();
                for e in elems {
                    if let ExprKind::String(s) = &e.kind {
                        lines.push(s.as_str());
                    } else {
                        self.ctx.emit_ice(
                            e.span,
                            "Kern ICE (Lowering): `@asm` template array must contain only strings.",
                        );
                        return None;
                    }
                }
                Some(lines.join("\n"))
            }
            _ => {
                self.ctx.emit_ice(
                    value.span,
                    "Kern ICE (Lowering): invalid format for `asm` field in `@asm` macro.",
                );
                None
            }
        }
    }

    fn asm_output_value_type(&mut self, ptr_expr: &MastExpr, span: Span) -> Option<TypeId> {
        match self.ctx.type_registry.get_elem_type(ptr_expr.ty) {
            Some(ty) => Some(ty),
            None => {
                self.ctx.emit_ice(
                    span,
                    "Kern ICE (Lowering): `@asm` output operand must lower to a pointer value.",
                );
                None
            }
        }
    }

    fn lower_builtin_operator_intrinsic(
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
        let TypeKind::TraitObject(trait_def_id, _) =
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

    fn lower_intrinsic_call(
        &mut self,
        fn_id: DefId,
        callee_ty: TypeId,
        args: &[Expr],
        arg_masts: &mut Vec<MastExpr>,
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

    fn intrinsic_generic_arg(&mut self, callee_ty: TypeId, index: usize) -> TypeId {
        match self.ctx.type_registry.get(callee_ty) {
            TypeKind::FnDef(_, args) => args.get(index).copied().unwrap_or(TypeId::ERROR),
            _ => TypeId::ERROR,
        }
    }

    fn intrinsic_return_type(&mut self, fn_id: DefId, callee_ty: TypeId) -> TypeId {
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

    fn atomic_ordering_arg(&mut self, arg: &Expr) -> AtomicOrdering {
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

    fn simd_align_arg(&mut self, arg: &Expr) -> u32 {
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

    fn simd_shuffle_indices_arg(&mut self, arg: &Expr) -> Vec<u32> {
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

    fn simd_rotate_amount_arg(&mut self, arg: &Expr, lanes: u16) -> u32 {
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

    fn simd_duplicate_shuffle_indices(
        &mut self,
        lanes: u16,
        indices: impl Fn(u32) -> u32,
    ) -> Vec<u32> {
        (0..lanes as u32).map(indices).collect()
    }

    fn simd_reverse_indices(&mut self, lanes: u16) -> Vec<u32> {
        self.simd_duplicate_shuffle_indices(lanes, |i| lanes as u32 - 1 - i)
    }

    fn simd_rotate_left_indices(&mut self, lanes: u16, amount: u32) -> Vec<u32> {
        self.simd_duplicate_shuffle_indices(lanes, |i| i + amount)
    }

    fn simd_rotate_right_indices(&mut self, lanes: u16, amount: u32) -> Vec<u32> {
        let lanes_u32 = lanes as u32;
        self.simd_duplicate_shuffle_indices(lanes, |i| i + ((lanes_u32 - amount) % lanes_u32))
    }

    fn simd_interleave_indices(&mut self, lanes: u16, high_half: bool) -> Vec<u32> {
        let half = lanes as u32 / 2;
        let base = if high_half { half } else { 0 };
        (0..half)
            .flat_map(|i| [base + i, lanes as u32 + base + i])
            .collect()
    }

    fn simd_concat_indices(&mut self, lanes: u16, high_half: bool) -> Vec<u32> {
        let half = lanes as u32 / 2;
        let base = if high_half { half } else { 0 };
        (0..half)
            .map(|i| base + i)
            .chain((0..half).map(|i| lanes as u32 + base + i))
            .collect()
    }

    fn simd_deinterleave_indices(&mut self, lanes: u16, odd_lanes: bool) -> Vec<u32> {
        let step = 2;
        let start = if odd_lanes { 1 } else { 0 };
        let count = lanes as u32 / 2;
        (0..count)
            .map(|i| start + i * step)
            .chain((0..count).map(|i| lanes as u32 + start + i * step))
            .collect()
    }

    fn simd_extract_half_indices(&mut self, lanes: u16, high_half: bool) -> Vec<u32> {
        let half = lanes as u32 / 2;
        let start = if high_half { half } else { 0 };
        (0..half).map(|i| start + i).collect()
    }

    pub(crate) fn lower_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        subst_map: &HashMap<SymbolId, TypeId>,
        span: Span,
    ) -> MastExprKind {
        if let Some(asm_call) = self.maybe_lower_asm_call(callee, args, subst_map, span) {
            return asm_call;
        }

        let raw_callee_ty = self
            .ctx
            .node_types
            .get(&callee.id)
            .copied()
            .unwrap_or(TypeId::ERROR);

        let substituted_callee = self.substitute_type_with_map(raw_callee_ty, subst_map);
        let norm_callee = self.ctx.type_registry.normalize(substituted_callee);
        let expected_param_tys = self.measure_phase("            lower_call_signature", |this| {
            this.get_callee_expected_params(norm_callee)
        });
        let method_call = self.measure_phase("            lower_call_detect_method", |this| {
            this.detect_method_call(callee, subst_map)
        });
        let intrinsic_name = self.intrinsic_name_for_lowering(norm_callee);

        let arg_masts = self.measure_phase("            lower_call_args", |this| {
            let mut arg_masts = Vec::new();
            for (i, a) in args.iter().enumerate() {
                let param_idx = if method_call.is_some() { i + 1 } else { i };
                let exp_ty = if matches!(
                    intrinsic_name.as_deref(),
                    Some(
                        "@simdSplat"
                            | "@simdCast"
                            | "@simdBitcast"
                            | "@simdLowHalf"
                            | "@simdHighHalf"
                            | "@simdWithLowHalf"
                            | "@simdWithHighHalf"
                            | "@simdMaskedLoad"
                            | "@simdMaskedStore"
                            | "@simdMaskedGather"
                            | "@simdMaskedScatter"
                    )
                ) {
                    None
                } else {
                    expected_param_tys
                        .get(param_idx)
                        .copied()
                        .filter(|&ty| ty != TypeId::ERROR)
                };
                arg_masts.push(this.lower_expr(a, subst_map, exp_ty));
            }
            arg_masts
        });

        if let Some((callee_id, field, recv)) = method_call {
            self.measure_phase("            lower_call_method_dispatch", |this| {
                this.lower_method_call(
                    callee_id,
                    recv,
                    arg_masts,
                    MethodCallSite {
                        field,
                        norm_callee,
                        expected_self_ty: expected_param_tys.first().copied(),
                        span,
                    },
                )
            })
        } else {
            self.measure_phase("            lower_call_plain_dispatch", |this| {
                this.lower_normal_call(callee, args, arg_masts, subst_map)
            })
        }
    }

    pub(crate) fn lower_asm_call(
        &mut self,
        args: &[Expr],
        subst_map: &HashMap<SymbolId, TypeId>,
        span: Span,
    ) -> MastExprKind {
        let Some(fields) = self.asm_config_fields(args, span) else {
            return MastExprKind::Trap;
        };

        let mut asm_template = String::new();
        let mut is_volatile = false;

        let mut outputs = Vec::new();
        let mut inputs = Vec::new();
        let mut clobbers = Vec::new();

        for field in fields {
            let field_name = self.ctx.resolve(field.name);
            match field_name {
                "asm" => {
                    let Some(template) = self.lower_asm_template(&field.value) else {
                        return MastExprKind::Trap;
                    };
                    asm_template = template;
                }
                "volatile" => {
                    if let ExprKind::Bool(b) = &field.value.kind {
                        is_volatile = *b;
                    }
                }
                "outputs" => {
                    if let ExprKind::DataInit {
                        literal: ast::DataLiteralKind::Struct(regs),
                        ..
                    } = &field.value.kind
                    {
                        for reg in regs {
                            let reg_name = self.ctx.resolve(reg.name);
                            // LLVM constraint mapping: `reg -> "=r"`, `freg -> "=f"`, `eax -> "={eax}"`.
                            let constraint = if reg_name == "reg" {
                                "=r".to_string()
                            } else if reg_name == "freg" {
                                "=f".to_string()
                            } else {
                                format!("={{{}}}", reg_name)
                            };

                            let ptr_expr = self.lower_expr(&reg.value, subst_map, None);
                            let Some(val_ty) =
                                self.asm_output_value_type(&ptr_expr, reg.value.span)
                            else {
                                return MastExprKind::Trap;
                            };
                            outputs.push((constraint, ptr_expr, val_ty));
                        }
                    }
                }
                "inputs" => {
                    if let ExprKind::DataInit {
                        literal: ast::DataLiteralKind::Struct(regs),
                        ..
                    } = &field.value.kind
                    {
                        for reg in regs {
                            let reg_name = self.ctx.resolve(reg.name);
                            // LLVM constraint mapping: `reg -> "r"`, `freg -> "f"`, `eax -> "{eax}"`.
                            let constraint = if reg_name == "reg" {
                                "r".to_string()
                            } else if reg_name == "freg" {
                                "f".to_string()
                            } else {
                                format!("{{{}}}", reg_name)
                            };

                            let val_expr = self.lower_expr(&reg.value, subst_map, None);
                            inputs.push((constraint, val_expr));
                        }
                    }
                }
                "clobbers" => {
                    if let ExprKind::DataInit {
                        literal: ast::DataLiteralKind::Array(elems),
                        ..
                    } = &field.value.kind
                    {
                        for e in elems {
                            if let ExprKind::String(s) = &e.kind {
                                clobbers.push(format!("~{{{}}}", s));
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // Build the final LLVM constraint string in output/input/clobber order.
        let mut all_constraints = Vec::new();
        let mut output_ptrs = Vec::new();
        let mut output_tys = Vec::new();
        for (c, ptr, ty) in outputs {
            all_constraints.push(c);
            output_ptrs.push(ptr);
            output_tys.push(ty);
        }

        let mut input_args = Vec::new();
        for (c, expr) in inputs {
            all_constraints.push(c);
            input_args.push(expr);
        }

        for c in clobbers {
            all_constraints.push(c);
        }

        MastExprKind::Asm(MastAsmBlock {
            asm_template,
            constraints: all_constraints.join(","),
            input_args,
            output_ptrs,
            output_tys,
            is_volatile,
        })
    }

    pub(crate) fn lower_method_call(
        &mut self,
        callee_id: NodeId,
        recv: MastExpr,
        arg_masts: Vec<MastExpr>,
        call: MethodCallSite,
    ) -> MastExprKind {
        // Resolve methods against the type that actually owns the implementation.
        let norm_base = self.ctx.type_registry.normalize(recv.ty);

        // Trait objects are always fat pointers in Kern, so inspect the pointee rather than the outer pointer.
        let mut inner_ty = norm_base;
        if let TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } =
            self.ctx.type_registry.get(norm_base).clone()
        {
            inner_ty = elem;
        }

        let owner_trait_ty = self
            .ctx
            .trait_method_owners
            .get(&callee_id)
            .copied()
            .unwrap_or(inner_ty);

        self.lower_resolved_trait_method_call(
            recv,
            arg_masts,
            owner_trait_ty,
            call,
        )
    }

    pub(crate) fn lower_resolved_trait_method_call(
        &mut self,
        recv: MastExpr,
        mut arg_masts: Vec<MastExpr>,
        owner_trait_ty: TypeId,
        call: MethodCallSite,
    ) -> MastExprKind {
        let norm_base = self.ctx.type_registry.normalize(recv.ty);
        let mut inner_ty = norm_base;
        if let TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } =
            self.ctx.type_registry.get(norm_base).clone()
        {
            inner_ty = elem;
        }

        let field_name = self.ctx.resolve(call.field).to_string();
        if field_name == "eq"
            && self.builtin_trait_name(owner_trait_ty).as_deref() == Some("Eq")
            && arg_masts.len() == 1
            && self.is_pure_enum_value_type(recv.ty)
            && arg_masts[0].ty == recv.ty
        {
            return MastExprKind::Binary {
                op: ast::BinaryOperator::Equal,
                lhs: Box::new(recv),
                rhs: Box::new(arg_masts.remove(0)),
            };
        }

        // 2. Choose dynamic (vtable) or static dispatch based on the recovered type.
        if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(inner_ty) {
            // Hand the full fat pointer to the dynamic dispatcher so it can extract the vtable.
            self.measure_phase("              lower_call_dynamic_dispatch", |this| {
                this.lower_dynamic_method_dispatch(
                    recv,
                    arg_masts,
                    DynamicDispatchCall {
                        field: call.field,
                        recv_trait_ty: inner_ty,
                        owner_trait_ty,
                        norm_callee: call.norm_callee,
                        span: call.span,
                    },
                )
            })
        } else if let TypeKind::FnDef(method_id, generics) =
            self.ctx.type_registry.get(call.norm_callee).clone()
        {
            if let Def::Function(func) = &self.ctx.defs[method_id.0 as usize]
                && func.is_intrinsic
            {
                arg_masts.insert(0, recv.clone());
                if let Some(kind) = self.lower_builtin_operator_intrinsic(method_id, &mut arg_masts)
                {
                    return kind;
                }
            }
            self.measure_phase("              lower_call_static_dispatch", |this| {
                this.lower_static_method_dispatch(
                    recv,
                    arg_masts,
                    method_id,
                    &generics,
                    call,
                )
            })
        } else {
            // A plain `TypeKind::Function` here means Sema only knew a generic bound.
            // After monomorphization, find the concrete impl globally.
            let mut target_func_id = None;
            let mut resolved_impl_args = Vec::new();
            let owner_trait_norm = self.ctx.type_registry.normalize(owner_trait_ty);
            let owner_trait_filter = !self.type_contains_generic_placeholders(owner_trait_ty)
                && matches!(
                    self.ctx.type_registry.get(owner_trait_norm),
                    TypeKind::TraitObject(..)
                );

            let method_ids_ptr = self
                .ctx
                .impl_methods_by_name
                .get(&call.field)
                .map(|method_ids| std::ptr::from_ref(method_ids.as_slice()));

            if let Some(method_ids_ptr) = method_ids_ptr {
                self.measure_phase("              lower_call_bound_impl_lookup", |this| {
                    // Safety: method-name indexes are immutable while lowering reads semantic defs.
                    let method_ids = unsafe { &*method_ids_ptr };
                    for &method_id in method_ids {
                        let Some(impl_id) =
                            this.ctx
                                .defs
                                .get(method_id.0 as usize)
                                .and_then(|def| match def {
                                    Def::Function(function) => function.parent,
                                    _ => None,
                                })
                        else {
                            continue;
                        };

                        let Some(impl_ptr) =
                            this.ctx
                                .defs
                                .get(impl_id.0 as usize)
                                .and_then(|def| match def {
                                    Def::Impl(impl_def) => Some(std::ptr::from_ref(impl_def)),
                                    _ => None,
                                })
                        else {
                            continue;
                        };

                        // Safety: lowering only reads semantic definition storage.
                        let impl_def = unsafe { &*impl_ptr };
                        let impl_target_raw = this
                            .ctx
                            .node_types
                            .get(&impl_def.target_type.id)
                            .copied()
                            .unwrap_or(TypeId::ERROR);
                        let norm_impl_target = this.ctx.type_registry.normalize(impl_target_raw);

                        // Non-generic impl.
                        if impl_def.generics.is_empty() {
                            let mut matched = false;

                            // Exact match: `*mut i32 == *mut i32` or `*i32 == *i32`.
                            if norm_base == norm_impl_target {
                                matched = true;
                            }
                            // Safe downgrade: allow `*mut i32` to use methods defined on `impl *i32`.
                            else if let TypeKind::Pointer { is_mut: true, elem } =
                                this.ctx.type_registry.get(norm_base).clone()
                            {
                                let const_ptr = this.ctx.type_registry.intern(TypeKind::Pointer {
                                    is_mut: false,
                                    elem,
                                });
                                if const_ptr == norm_impl_target {
                                    matched = true;
                                }
                            }

                            if matched {
                                if owner_trait_filter {
                                    let Some(trait_ast) = &impl_def.trait_type else {
                                        continue;
                                    };
                                    let impl_trait_ty = this
                                        .ctx
                                        .node_types
                                        .get(&trait_ast.id)
                                        .copied()
                                        .unwrap_or(TypeId::ERROR);
                                    if this.ctx.type_registry.normalize(impl_trait_ty)
                                        != owner_trait_norm
                                    {
                                        continue;
                                    }
                                }
                                target_func_id = Some(method_id);
                            }
                        }
                        // Generic impl matching.
                        else {
                            // Strip matching pointer layers so generic arguments can be recovered from the underlying `Def`.
                            let mut check_base = norm_base;
                            let mut check_impl = norm_impl_target;
                            let mut matched_ptr = false;

                            // Handle pointer downgrade and pointer peeling together.
                            if let TypeKind::Pointer {
                                is_mut: base_mut,
                                elem: base_elem,
                            } = this.ctx.type_registry.get(check_base).clone()
                            {
                                if let TypeKind::Pointer {
                                    is_mut: impl_mut,
                                    elem: impl_elem,
                                } = this.ctx.type_registry.get(check_impl).clone()
                                {
                                    // Allow exact matches and safe `*mut T -> *T` downgrades.
                                    if base_mut == impl_mut || (base_mut && !impl_mut) {
                                        check_base = base_elem;
                                        check_impl = impl_elem;
                                        matched_ptr = true;
                                    }
                                }
                            } else {
                                matched_ptr = true; // If neither side is a pointer, keep checking normally.
                            }

                            if matched_ptr
                                && let TypeKind::Def(base_def_id, base_args) =
                                    this.ctx.type_registry.get(check_base).clone()
                                && let TypeKind::Def(impl_def_id, impl_raw_args) =
                                    this.ctx.type_registry.get(check_impl).clone()
                                && base_def_id == impl_def_id
                                && base_args.len() == impl_raw_args.len()
                            {
                                if owner_trait_filter {
                                    let Some(trait_ast) = &impl_def.trait_type else {
                                        continue;
                                    };
                                    let impl_trait_ty = this
                                        .ctx
                                        .node_types
                                        .get(&trait_ast.id)
                                        .copied()
                                        .unwrap_or(TypeId::ERROR);
                                    let mut subst_map = HashMap::new();
                                    if let TypeKind::Def(_, impl_args) =
                                        this.ctx.type_registry.get(norm_impl_target).clone()
                                        && impl_def.generics.len() == impl_args.len()
                                    {
                                        for (param, arg) in
                                            impl_def.generics.iter().zip(base_args.iter().copied())
                                        {
                                            subst_map.insert(param.name, arg);
                                        }
                                    }
                                    let inst_trait_ty = if subst_map.is_empty() {
                                        impl_trait_ty
                                    } else {
                                        let mut subst = Substituter::new(
                                            &mut this.ctx.type_registry,
                                            &subst_map,
                                        );
                                        subst.substitute(impl_trait_ty)
                                    };
                                    if this.ctx.type_registry.normalize(inst_trait_ty)
                                        != owner_trait_norm
                                    {
                                        continue;
                                    }
                                }
                                resolved_impl_args = base_args.clone();
                                target_func_id = Some(method_id);
                            }
                        }

                        if target_func_id.is_some() {
                            break;
                        }
                    }
                });
            }

            if let Some(func_id) = target_func_id {
                let mut final_recv = recv;

                // Normalize pointer-type differences for LLVM by inserting a bitcast after safe downgrades.
                if let Some(exp_self) = call.expected_self_ty
                    && final_recv.ty != exp_self
                {
                    final_recv = MastExpr::new(
                        exp_self,
                        MastExprKind::Cast {
                            kind: MastCastKind::Bitcast,
                            operand: Box::new(final_recv),
                        },
                        call.span,
                    );
                }

                if let Def::Function(func) = &self.ctx.defs[func_id.0 as usize]
                    && func.is_intrinsic
                {
                    arg_masts.insert(0, final_recv.clone());
                    if let Some(kind) =
                        self.lower_builtin_operator_intrinsic(func_id, &mut arg_masts)
                    {
                        return kind;
                    }
                }

                arg_masts.insert(0, final_recv);
                let mono_id = self.instantiate_function(func_id, &resolved_impl_args);
                let func_ref = MastExpr::new(
                    call.norm_callee,
                    MastExprKind::FuncRef(mono_id),
                    call.span,
                );
                MastExprKind::Call {
                    callee: Box::new(func_ref),
                    args: arg_masts,
                }
            } else {
                let type_name = self.ctx.ty_to_string(norm_base);
                let field_name = self.ctx.resolve(call.field);
                self.ctx.emit_ice(
                    call.span,
                    format!(
                        "Kern ICE (Lowering): failed to devirtualize static trait method `{}` for exact type `{}`.",
                        field_name, type_name
                    ),
                );
                MastExprKind::Trap
            }
        }
    }

    /// Helper: build a statically dispatched method call.
    pub(crate) fn lower_static_method_dispatch(
        &mut self,
        mut recv: MastExpr,
        mut arg_masts: Vec<MastExpr>,
        method_id: DefId,
        generics: &[TypeId],
        call: MethodCallSite,
    ) -> MastExprKind {
        recv = self.measure_phase("                lower_call_static_recv", |_this| {
            if let Some(exp_self) = call.expected_self_ty
                && recv.ty != exp_self
            {
                MastExpr::new(
                    exp_self,
                    MastExprKind::Cast {
                        kind: MastCastKind::Bitcast,
                        operand: Box::new(recv),
                    },
                    call.span,
                )
            } else {
                recv
            }
        });

        self.measure_phase("                lower_call_static_args", |_this| {
            arg_masts.insert(0, recv);
        });
        let func_id = self.measure_phase("                lower_call_static_instantiate", |this| {
            this.instantiate_function(method_id, generics)
        });
        self.measure_phase("                lower_call_static_build", |_this| {
            let func_ref =
                MastExpr::new(call.norm_callee, MastExprKind::FuncRef(func_id), call.span);
            MastExprKind::Call {
                callee: Box::new(func_ref),
                args: arg_masts,
            }
        })
    }

    fn lower_plain_fn_call(
        &mut self,
        callee_mast: MastExpr,
        args: &[Expr],
        mut arg_masts: Vec<MastExpr>,
        fn_id: DefId,
        fn_args: Vec<TypeId>,
        span: Span,
    ) -> MastExprKind {
        if let Some(intrinsic) = self
            .measure_phase("              lower_call_plain_intrinsic", |this| {
                this.lower_intrinsic_call(fn_id, callee_mast.ty, args, &mut arg_masts)
            })
        {
            return intrinsic;
        }

        let mono_id = self.measure_phase("              lower_call_plain_instantiate", |this| {
            this.instantiate_function(fn_id, &fn_args)
        });
        self.measure_phase("              lower_call_plain_build", |_this| {
            let func_ref = MastExpr::new(callee_mast.ty, MastExprKind::FuncRef(mono_id), span);
            MastExprKind::Call {
                callee: Box::new(func_ref),
                args: arg_masts,
            }
        })
    }

    fn lower_plain_direct_call(
        &mut self,
        callee_mast: MastExpr,
        arg_masts: Vec<MastExpr>,
    ) -> MastExprKind {
        self.measure_phase("              lower_call_plain_direct", |_this| {
            MastExprKind::Call {
                callee: Box::new(callee_mast),
                args: arg_masts,
            }
        })
    }

    fn lower_plain_closure_call(
        &mut self,
        callee_mast: MastExpr,
        arg_masts: Vec<MastExpr>,
        inner_norm: TypeId,
        span: Span,
    ) -> MastExprKind {
        self.measure_phase("              lower_call_plain_closure", |this| {
            this.lower_closure_call(callee_mast, arg_masts, inner_norm, span)
        })
    }

    fn lower_plain_callee(
        &mut self,
        callee: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> MastExpr {
        self.measure_phase("              lower_call_plain_callee", |this| {
            this.lower_expr(callee, subst_map, None)
        })
    }

    /// Helper: build a dynamically dispatched method call by loading from the vtable.
    pub(crate) fn lower_dynamic_method_dispatch(
        &mut self,
        recv: MastExpr,
        mut arg_masts: Vec<MastExpr>,
        call: DynamicDispatchCall,
    ) -> MastExprKind {
        let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::VOID,
        });

        // Data pointer passed as the method's `self`.
        let data_ptr = MastExpr::new(
            void_ptr_ty,
            MastExprKind::ExtractFatPtrData(Box::new(recv.clone())),
            call.span,
        );
        arg_masts.insert(0, data_ptr);

        // Extract and cast the vtable pointer.
        let vtable_meta = MastExpr::new(
            TypeId::USIZE,
            MastExprKind::ExtractFatPtrMeta(Box::new(recv)),
            call.span,
        );
        let vtable_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: void_ptr_ty,
        });

        let vtable_ptr = MastExpr::new(
            vtable_ptr_ty,
            MastExprKind::Cast {
                kind: MastCastKind::IntToPtr,
                operand: Box::new(vtable_meta),
            },
            call.span,
        );

        let recv_trait_norm = self.ctx.type_registry.normalize(call.recv_trait_ty);
        let owner_trait_norm = self.ctx.type_registry.normalize(call.owner_trait_ty);

        let owner_vtable_ptr = if owner_trait_norm == recv_trait_norm {
            vtable_ptr
        } else {
            let Some(super_slot) = self.vtable_supertrait_slot(recv_trait_norm, owner_trait_norm)
            else {
                self.ctx.emit_ice(
                    call.span,
                    format!(
                        "Kern ICE (Lowering): trait `{}` is not a supertrait of `{}` during dynamic dispatch.",
                        self.ctx.ty_to_string(owner_trait_norm),
                        self.ctx.ty_to_string(recv_trait_norm)
                    ),
                );
                return MastExprKind::Trap;
            };

            let super_vtable_raw = MastExpr::new(
                void_ptr_ty,
                MastExprKind::IndexAccess {
                    lhs: Box::new(vtable_ptr),
                    index: Box::new(MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::Integer(super_slot as u128),
                        call.span,
                    )),
                },
                call.span,
            );

            MastExpr::new(
                vtable_ptr_ty,
                MastExprKind::Cast {
                    kind: MastCastKind::Bitcast,
                    operand: Box::new(super_vtable_raw),
                },
                call.span,
            )
        };

        let Some(vtable_idx) = self.direct_trait_method_slot(owner_trait_norm, call.field) else {
            self.ctx.emit_ice(
                call.span,
                format!(
                    "Kern ICE (Lowering): method `{}` not found in owner trait `{}`.",
                    self.ctx.resolve(call.field),
                    self.ctx.ty_to_string(owner_trait_norm),
                ),
            );
            return MastExprKind::Trap;
        };

        // Load the function pointer from the vtable slot.
        let func_ptr = MastExpr::new(
            void_ptr_ty,
            MastExprKind::IndexAccess {
                lhs: Box::new(owner_vtable_ptr),
                index: Box::new(MastExpr::new(
                    TypeId::USIZE,
                    MastExprKind::Integer(vtable_idx as u128),
                    call.span,
                )),
            },
            call.span,
        );

        // Rebuild the exact callable signature.
        let (ret_ty, is_variadic, mut patched_params) = if let TypeKind::Function {
            ret,
            is_variadic,
            params,
            ..
        } =
            self.ctx.type_registry.get(call.norm_callee)
        {
            (*ret, *is_variadic, params.clone())
        } else {
            self.ctx.emit_ice(
                call.span,
                "Kern ICE (Lowering): Callee type of dynamic method dispatch is not a Function.",
            );
            return MastExprKind::Trap;
        };

        if !patched_params.is_empty() {
            patched_params[0] = void_ptr_ty;
        }

        let patched_fn_ty = self.ctx.type_registry.intern(TypeKind::Function {
            params: patched_params,
            ret: ret_ty,
            is_variadic,
        });

        let func_ptr_typed = MastExpr::new(
            patched_fn_ty,
            MastExprKind::Cast {
                kind: MastCastKind::Bitcast,
                operand: Box::new(func_ptr),
            },
            call.span,
        );

        MastExprKind::Call {
            callee: Box::new(func_ptr_typed),
            args: arg_masts,
        }
    }

    pub(crate) fn lower_normal_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        arg_masts: Vec<MastExpr>,
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> MastExprKind {
        let callee_mast = self.lower_plain_callee(callee, subst_map);
        let norm_callee = self.ctx.type_registry.normalize(callee_mast.ty);

        // Intercept dynamic calls through closure fat pointers.
        if let TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } =
            self.ctx.type_registry.get(norm_callee).clone()
        {
            let inner_norm = self.ctx.type_registry.normalize(elem);
            if matches!(
                self.ctx.type_registry.get(inner_norm),
                TypeKind::ClosureInterface { .. }
            ) {
                return self.lower_plain_closure_call(
                    callee_mast,
                    arg_masts,
                    inner_norm,
                    callee.span,
                );
            }
        }

        if let TypeKind::FnDef(fn_id, fn_args) = self.ctx.type_registry.get(callee_mast.ty).clone()
        {
            self.lower_plain_fn_call(callee_mast, args, arg_masts, fn_id, fn_args, callee.span)
        } else {
            self.lower_plain_direct_call(callee_mast, arg_masts)
        }
    }

    pub(crate) fn lower_generic_instantiation(&mut self, concrete_ty: TypeId) -> MastExprKind {
        let fn_info =
            if let TypeKind::FnDef(fn_id, fn_args) = self.ctx.type_registry.get(concrete_ty) {
                Some((*fn_id, fn_args.clone()))
            } else {
                None
            };
        if let Some((fn_id, fn_args)) = fn_info {
            let mono_id = self.instantiate_function(fn_id, &fn_args);
            MastExprKind::FuncRef(mono_id)
        } else {
            MastExprKind::Integer(0)
        }
    }

    pub(crate) fn get_callee_expected_params(&mut self, norm_callee: TypeId) -> Vec<TypeId> {
        match self.ctx.type_registry.get(norm_callee).clone() {
            TypeKind::Function { params, .. } => params,
            TypeKind::FnDef(def_id, gen_args) => {
                if let Def::Function(f) = &self.ctx.defs[def_id.0 as usize] {
                    if let Some(sig) = f.resolved_sig {
                        let norm_sig = self.ctx.type_registry.normalize(sig);
                        let raw_params = if let TypeKind::Function { params, .. } =
                            self.ctx.type_registry.get(norm_sig).clone()
                        {
                            params
                        } else {
                            Vec::new()
                        };

                        let all_generic_params = f.generics.clone();

                        let mut sig_subst_map = HashMap::new();
                        for (idx, param) in all_generic_params.iter().enumerate() {
                            if idx < gen_args.len() {
                                sig_subst_map.insert(param.name, gen_args[idx]);
                            }
                        }

                        raw_params
                            .into_iter()
                            .map(|p| self.substitute_type_with_map(p, &sig_subst_map))
                            .collect()
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }

    pub(crate) fn lower_closure_call(
        &mut self,
        callee_mast: MastExpr,
        mut arg_masts: Vec<MastExpr>,
        closure_interface_ty: TypeId,
        span: Span,
    ) -> MastExprKind {
        let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::VOID,
        });

        // 1. Extract `data_ptr` and inject it as argument 0.
        let data_ptr = MastExpr::new(
            void_ptr_ty,
            MastExprKind::ExtractFatPtrData(Box::new(callee_mast.clone())),
            span,
        );
        arg_masts.insert(0, data_ptr);

        // 2. Extract `meta_ptr`, which stores the code pointer.
        let code_ptr = MastExpr::new(
            TypeId::USIZE,
            MastExprKind::ExtractFatPtrMeta(Box::new(callee_mast.clone())),
            span,
        );

        // 3. Build the exact lowered function signature and cast the `usize` code pointer to it.
        let (params, ret) = if let TypeKind::ClosureInterface { params, ret } =
            self.ctx.type_registry.get(closure_interface_ty).clone()
        {
            (params, ret)
        } else {
            let actual_ty_str = self.ctx.ty_to_string(closure_interface_ty);
            self.ctx.emit_ice(
                span,
                format!(
                    "Kern ICE (Lowering): Expected `ClosureInterface`, found `{}`.",
                    actual_ty_str
                ),
            );
            return MastExprKind::Trap;
        };

        let mut patched_params = params.clone();
        patched_params.insert(0, void_ptr_ty); // Prepend the hidden environment parameter.

        let patched_fn_ty = self.ctx.type_registry.intern(TypeKind::Function {
            params: patched_params,
            ret,
            is_variadic: false,
        });

        let typed_code_ptr = MastExpr::new(
            patched_fn_ty,
            MastExprKind::Cast {
                kind: MastCastKind::IntToPtr,
                operand: Box::new(code_ptr),
            },
            span,
        );

        // 4. Emit the indirect call through the function pointer.
        MastExprKind::Call {
            callee: Box::new(typed_code_ptr),
            args: arg_masts,
        }
    }
}
