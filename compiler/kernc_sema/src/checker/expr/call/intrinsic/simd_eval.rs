//! SIMD intrinsic constant helpers.
//!
//! These routines evaluate lane counts, shift amounts, shuffle masks, and
//! immediate arguments used by SIMD intrinsics after the high-level call checker
//! has established the expected vector shape.

use super::*;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub(super) fn is_signed_integer_type(&mut self, ty: TypeId) -> bool {
        matches!(
            self.resolve_tv(ty),
            TypeId::I8 | TypeId::I16 | TypeId::I32 | TypeId::I64 | TypeId::I128 | TypeId::ISIZE
        )
    }

    pub(super) fn resolve_simd_intrinsic_result_type(
        &mut self,
        intrinsic_name: &str,
        callee_ty: TypeId,
        span: Span,
        hint: &str,
    ) -> Option<(TypeId, TypeId, u16)> {
        let value_ty = self
            .intrinsic_generic_arg(callee_ty, 0)
            .unwrap_or(TypeId::ERROR);
        let norm_value = self.resolve_tv(value_ty);
        let Some((elem_ty, lanes)) = self.ctx.type_registry.simd_info(norm_value) else {
            if norm_value == TypeId::ERROR {
                self.ctx
                    .struct_error(
                        span,
                        format!("`{}` requires an explicit SIMD result type", intrinsic_name),
                    )
                    .with_hint(hint)
                    .emit();
            } else {
                self.ctx
                    .struct_error(
                        span,
                        format!("`{}` generic argument must be a SIMD type", intrinsic_name),
                    )
                    .emit();
            }
            return None;
        };
        Some((norm_value, elem_ty, lanes))
    }

    pub(super) fn eval_simd_align_arg(&mut self, arg: &Expr, intrinsic_name: &str) -> Option<u64> {
        let align_ty = self.check_expr(arg, Some(TypeId::USIZE));
        if align_ty == TypeId::ERROR {
            return None;
        }

        let mut evaluator = ConstEvaluator::new(self.ctx);
        let Ok(align) = evaluator.eval_usize(arg) else {
            self.ctx
                .struct_error(
                    arg.span,
                    format!(
                        "`{}` alignment must be a compile-time constant",
                        intrinsic_name
                    ),
                )
                .with_hint("example: `@simdLoad[f32x4](ptr, 4)`")
                .emit();
            return None;
        };

        if align == 0 || !align.is_power_of_two() {
            self.ctx
                .struct_error(
                    arg.span,
                    format!(
                        "`{}` alignment must be a non-zero power of two",
                        intrinsic_name
                    ),
                )
                .emit();
            return None;
        }

        if align > u32::MAX as u64 {
            self.ctx
                .struct_error(
                    arg.span,
                    format!(
                        "`{}` alignment `{}` exceeds the maximum backend-supported alignment of {}",
                        intrinsic_name,
                        align,
                        u32::MAX
                    ),
                )
                .with_hint("choose a smaller power-of-two alignment that fits within 32 bits")
                .emit();
            return None;
        }

        Some(align)
    }

    pub(super) fn eval_simd_rotate_amount(
        &mut self,
        arg: &Expr,
        intrinsic_name: &str,
        lanes: u16,
    ) -> Option<u32> {
        let amount_ty = self.check_expr(arg, Some(TypeId::USIZE));
        if amount_ty == TypeId::ERROR {
            return None;
        }

        let mut evaluator = ConstEvaluator::new(self.ctx);
        let Ok(amount) = evaluator.eval_usize(arg) else {
            self.ctx
                .struct_error(
                    arg.span,
                    format!(
                        "`{}` amount must be a compile-time constant",
                        intrinsic_name
                    ),
                )
                .emit();
            return None;
        };

        Some((amount % lanes as u64) as u32)
    }

    pub(super) fn eval_simd_lane_indices(
        &mut self,
        arg: &Expr,
        intrinsic_name: &str,
        lanes: u16,
        upper_bound: u32,
        range_desc: String,
        hint: &str,
    ) -> Option<Vec<u32>> {
        let mut evaluator = ConstEvaluator::new(self.ctx);
        let indices = match evaluator.eval_inner(arg, 0) {
            Ok(ConstValue::Array(values)) => values,
            Ok(_) => {
                self.ctx
                    .struct_error(
                        arg.span,
                        format!(
                            "`{}` indices must be a compile-time integer array",
                            intrinsic_name
                        ),
                    )
                    .with_hint(hint)
                    .emit();
                return None;
            }
            Err(_) => return None,
        };

        if indices.len() != lanes as usize {
            self.ctx
                .struct_error(
                    arg.span,
                    format!(
                        "`{}` index count must match the result lane count",
                        intrinsic_name
                    ),
                )
                .with_hint(format!(
                    "expected {} indices, found {}",
                    lanes,
                    indices.len()
                ))
                .emit();
            return None;
        }

        let mut out = Vec::with_capacity(indices.len());
        for value in indices {
            let ConstValue::Int(idx) = value else {
                self.ctx
                    .struct_error(
                        arg.span,
                        format!("`{}` indices must be integer constants", intrinsic_name),
                    )
                    .emit();
                return None;
            };
            if idx < 0 || idx >= upper_bound as i128 {
                self.ctx
                    .struct_error(
                        arg.span,
                        format!(
                            "`{}` indices must be in the range {}",
                            intrinsic_name, range_desc
                        ),
                    )
                    .with_hint(hint)
                    .emit();
                return None;
            }
            out.push(idx as u32);
        }

        Some(out)
    }

    pub(super) fn eval_simd_shuffle_indices(&mut self, arg: &Expr, lanes: u16) -> Option<Vec<u32>> {
        self.eval_simd_lane_indices(
            arg,
            "@simdShuffle",
            lanes,
            (lanes as u32) * 2,
            format!("[0, {})", (lanes as u32) * 2),
            "shuffle indices select from lhs lanes [0..N) and rhs lanes [N..2N)",
        )
    }

    pub(super) fn eval_simd_swizzle_indices(&mut self, arg: &Expr, lanes: u16) -> Option<Vec<u32>> {
        self.eval_simd_lane_indices(
            arg,
            "@simdSwizzle",
            lanes,
            lanes as u32,
            format!("[0, {})", lanes),
            "swizzle indices must stay within the source vector lane count",
        )
    }

    pub(super) fn check_simd_index_ptr_arg(
        &mut self,
        arg: &Expr,
        intrinsic_name: &str,
    ) -> Option<TypeId> {
        let index_ptr_ty = self.check_expr(arg, None);
        let norm_index_ptr = self.resolve_tv(index_ptr_ty);

        match self.ctx.type_registry.get(norm_index_ptr).clone() {
            TypeKind::Pointer { elem, .. } => {
                if elem != TypeId::USIZE && elem != TypeId::ERROR {
                    self.ctx
                        .struct_error(
                            arg.span,
                            format!("`{}` indices pointer must point to `usize`", intrinsic_name),
                        )
                        .with_hint("example: `idx.[0].&` where `idx` is `[N]usize`")
                        .emit();
                }
            }
            TypeKind::Error => {}
            _ => {
                self.ctx
                    .struct_error(
                        arg.span,
                        format!(
                            "`{}` expects a raw pointer to `usize` indices",
                            intrinsic_name
                        ),
                    )
                    .with_hint("example: `idx.[0].&` where `idx` is `[N]usize`")
                    .emit();
            }
        }

        Some(norm_index_ptr)
    }

    pub(super) fn check_simd_mask_arg(
        &mut self,
        arg: &Expr,
        intrinsic_name: &str,
    ) -> Option<(TypeId, u16)> {
        let ty = self.check_expr(arg, None);
        let norm = self.resolve_tv(ty);
        let Some((elem_ty, lanes)) = self.ctx.type_registry.simd_info(norm) else {
            if norm != TypeId::ERROR {
                self.ctx
                    .struct_error(
                        arg.span,
                        format!("`{}` expects a SIMD mask (`boolxN`)", intrinsic_name),
                    )
                    .emit();
            }
            return None;
        };

        if elem_ty != TypeId::BOOL {
            self.ctx
                .struct_error(
                    arg.span,
                    format!("`{}` expects a SIMD mask (`boolxN`)", intrinsic_name),
                )
                .emit();
            return None;
        }

        Some((norm, lanes))
    }

    pub(super) fn check_simd_mask_lane_match(
        &mut self,
        arg: &Expr,
        intrinsic_name: &str,
        mask_lanes: u16,
        value_lanes: u16,
    ) {
        if mask_lanes != value_lanes {
            self.ctx
                .struct_error(
                    arg.span,
                    format!(
                        "`{}` mask lane count must match the SIMD value lane count",
                        intrinsic_name
                    ),
                )
                .emit();
        }
    }

    pub(super) fn check_simd_half_relation(
        &mut self,
        intrinsic_name: &str,
        full: SimdRelationOperand<'_>,
        half: SimdRelationOperand<'_>,
    ) -> Option<(TypeId, u16, TypeId, u16)> {
        let norm_full = self.resolve_tv(full.ty);
        let Some((full_elem, full_lanes)) = self.ctx.type_registry.simd_info(norm_full) else {
            if norm_full != TypeId::ERROR {
                self.ctx
                    .struct_error(
                        full.span,
                        format!("`{}` {} must be a SIMD value", intrinsic_name, full.label),
                    )
                    .emit();
            }
            return None;
        };

        let norm_half = self.resolve_tv(half.ty);
        let Some((half_elem, half_lanes)) = self.ctx.type_registry.simd_info(norm_half) else {
            if norm_half != TypeId::ERROR {
                self.ctx
                    .struct_error(
                        half.span,
                        format!("`{}` {} must be a SIMD value", intrinsic_name, half.label),
                    )
                    .emit();
            }
            return None;
        };

        if full_elem != half_elem {
            self.ctx
                .struct_error(
                    half.span,
                    format!(
                        "`{}` {} lane type must match the {} lane type",
                        intrinsic_name, half.label, full.label
                    ),
                )
                .emit();
            return None;
        }

        if full_lanes != half_lanes * 2 {
            self.ctx
                .struct_error(
                    half.span,
                    format!(
                        "`{}` {} lane count must be exactly half of the {} lane count",
                        intrinsic_name, half.label, full.label
                    ),
                )
                .emit();
            return None;
        }

        Some((full_elem, full_lanes, half_elem, half_lanes))
    }
}
