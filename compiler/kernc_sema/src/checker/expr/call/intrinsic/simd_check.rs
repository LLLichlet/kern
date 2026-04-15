use super::*;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub(crate) fn check_simd_intrinsic_call(
        &mut self,
        intrinsic_name: &str,
        callee_ty: TypeId,
        args: &[Expr],
        params: &[TypeId],
        default_ret: TypeId,
    ) -> Option<TypeId> {
        match intrinsic_name {
            "@simdAny" | "@simdAll" => {
                let mask_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], mask_ty);
                let norm_mask = self.resolve_tv(mask_ty);
                if norm_mask != TypeId::ERROR && !self.ctx.type_registry.is_simd_mask(norm_mask) {
                    self.ctx
                        .struct_error(
                            args[0].span,
                            format!("`{}` expects a SIMD mask (`boolxN`)", intrinsic_name),
                        )
                        .emit();
                }
                Some(default_ret)
            }
            "@simdBitmask" => {
                let mask_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], mask_ty);
                let norm_mask = self.resolve_tv(mask_ty);
                let Some((elem_ty, lanes)) = self.ctx.type_registry.simd_info(norm_mask) else {
                    if norm_mask != TypeId::ERROR {
                        self.ctx
                            .struct_error(
                                args[0].span,
                                "`@simdBitmask` expects a SIMD mask (`boolxN`)",
                            )
                            .emit();
                    }
                    return Some(TypeId::ERROR);
                };
                if elem_ty != TypeId::BOOL {
                    self.ctx
                        .struct_error(
                            args[0].span,
                            "`@simdBitmask` expects a SIMD mask (`boolxN`)",
                        )
                        .emit();
                    return Some(TypeId::ERROR);
                }

                let usize_bits = (self.ctx.sess.target.pointer_size as u16) * 8;
                if lanes > usize_bits {
                    self.ctx
                        .struct_error(
                            args[0].span,
                            format!(
                                "`@simdBitmask` requires the mask lane count to fit in `usize` on target `{}`",
                                self.ctx.sess.target.triple
                            ),
                        )
                        .with_hint(format!(
                            "found {} lanes, but `usize` on this target only has {} bits",
                            lanes, usize_bits
                        ))
                        .emit();
                    return Some(TypeId::ERROR);
                }

                Some(default_ret)
            }
            "@simdSplat" => {
                let Some((norm_value, elem_ty, _)) = self.resolve_simd_intrinsic_result_type(
                    intrinsic_name,
                    callee_ty,
                    args[0].span,
                    "example: `@simdSplat[i32x4](7)`",
                ) else {
                    return Some(TypeId::ERROR);
                };

                let scalar_ty = self.check_expr(&args[0], Some(elem_ty));
                self.check_coercion(&args[0], elem_ty, scalar_ty);
                let norm_scalar = self.resolve_tv(scalar_ty);
                if norm_scalar != TypeId::ERROR && norm_scalar != elem_ty {
                    let expected_ty = self.ctx.ty_to_string(elem_ty);
                    self.ctx
                        .struct_error(
                            args[0].span,
                            "`@simdSplat` scalar value must match the SIMD lane type",
                        )
                        .with_hint(format!("expected `{}`", expected_ty))
                        .emit();
                }

                Some(norm_value)
            }
            "@simdCast" => {
                let value_ty = self.check_expr(&args[0], None);
                let norm_src = self.resolve_tv(value_ty);
                let Some((norm_dst, dst_elem, dst_lanes)) = self
                    .resolve_simd_intrinsic_result_type(
                        intrinsic_name,
                        callee_ty,
                        args[0].span,
                        "example: `@simdCast[f32x4](i32x4.{ 1, 2, 3, 4 })`",
                    )
                else {
                    return Some(TypeId::ERROR);
                };

                let Some((src_elem, src_lanes)) = self.ctx.type_registry.simd_info(norm_src) else {
                    if norm_src != TypeId::ERROR {
                        self.ctx
                            .struct_error(args[0].span, "`@simdCast` expects a SIMD value")
                            .emit();
                    }
                    return Some(TypeId::ERROR);
                };

                if src_lanes != dst_lanes {
                    self.ctx
                        .struct_error(
                            args[0].span,
                            "`@simdCast` requires matching SIMD lane counts",
                        )
                        .with_hint(format!("found {} lanes and {} lanes", src_lanes, dst_lanes))
                        .emit();
                    return Some(TypeId::ERROR);
                }

                let src_numeric = self.ctx.type_registry.is_integer(src_elem)
                    || self.ctx.type_registry.is_float(src_elem)
                    || src_elem == TypeId::BOOL;
                let dst_numeric = self.ctx.type_registry.is_integer(dst_elem)
                    || self.ctx.type_registry.is_float(dst_elem);
                if !src_numeric || !dst_numeric {
                    self.ctx
                        .struct_error(
                            args[0].span,
                            "`@simdCast` only supports lane-wise numeric conversions",
                        )
                        .with_hint("source lanes may be integer, floating-point, or bool; target lanes may be integer or floating-point")
                        .emit();
                    return Some(TypeId::ERROR);
                }

                Some(norm_dst)
            }
            "@simdBitcast" => {
                let value_ty = self.check_expr(&args[0], None);
                let norm_src = self.resolve_tv(value_ty);
                let Some((norm_dst, _, _)) = self.resolve_simd_intrinsic_result_type(
                    intrinsic_name,
                    callee_ty,
                    args[0].span,
                    "example: `@simdBitcast[u32x4](f32x4.{ 1.0, 2.0, 3.0, 4.0 })`",
                ) else {
                    return Some(TypeId::ERROR);
                };

                if !self.ctx.type_registry.is_simd(norm_src) {
                    if norm_src != TypeId::ERROR {
                        self.ctx
                            .struct_error(args[0].span, "`@simdBitcast` expects a SIMD value")
                            .emit();
                    }
                    return Some(TypeId::ERROR);
                }

                let mut layout = LayoutEngine::new(self.ctx);
                let src_size = layout.compute_type_size(norm_src);
                let dst_size = layout.compute_type_size(norm_dst);
                if src_size != dst_size {
                    self.ctx
                        .struct_error(
                            args[0].span,
                            "`@simdBitcast` requires source and result vectors to have the same size",
                        )
                        .with_hint(format!(
                            "found {} bytes and {} bytes",
                            src_size, dst_size
                        ))
                        .emit();
                    return Some(TypeId::ERROR);
                }

                Some(norm_dst)
            }
            "@simdSelect" => {
                let mask_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], mask_ty);
                let true_ty = self.check_expr(&args[1], Some(params[1]));
                self.check_coercion(&args[1], params[1], true_ty);
                let false_ty = self.check_expr(&args[2], Some(params[2]));
                self.check_coercion(&args[2], params[2], false_ty);

                let norm_mask = self.resolve_tv(mask_ty);
                let norm_true = self.resolve_tv(true_ty);
                let norm_false = self.resolve_tv(false_ty);

                if norm_mask != TypeId::ERROR && !self.ctx.type_registry.is_simd_mask(norm_mask) {
                    self.ctx
                        .struct_error(args[0].span, "`@simdSelect` expects `boolxN` as its mask")
                        .emit();
                }

                if norm_true != TypeId::ERROR && !self.ctx.type_registry.is_simd(norm_true) {
                    self.ctx
                        .struct_error(
                            args[1].span,
                            "`@simdSelect` expects SIMD values for `on_true` and `on_false`",
                        )
                        .emit();
                }

                if norm_false != TypeId::ERROR && !self.ctx.type_registry.is_simd(norm_false) {
                    self.ctx
                        .struct_error(
                            args[2].span,
                            "`@simdSelect` expects SIMD values for `on_true` and `on_false`",
                        )
                        .emit();
                }

                if norm_true != TypeId::ERROR
                    && norm_false != TypeId::ERROR
                    && norm_true != norm_false
                {
                    let true_ty_str = self.ctx.ty_to_string(norm_true);
                    let false_ty_str = self.ctx.ty_to_string(norm_false);
                    self.ctx
                        .struct_error(
                            args[2].span,
                            "`@simdSelect` requires matching SIMD value types",
                        )
                        .with_hint(format!("found `{}` and `{}`", true_ty_str, false_ty_str))
                        .emit();
                }

                if let (Some((_, mask_lanes)), Some((_, value_lanes))) = (
                    self.ctx.type_registry.simd_info(norm_mask),
                    self.ctx.type_registry.simd_info(norm_true),
                ) && mask_lanes != value_lanes
                {
                    self.ctx
                        .struct_error(
                            args[0].span,
                            "`@simdSelect` mask lane count must match the value lane count",
                        )
                        .emit();
                }

                Some(default_ret)
            }
            "@simdShuffle" => {
                let lhs_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], lhs_ty);
                let rhs_ty = self.check_expr(&args[1], Some(params[1]));
                self.check_coercion(&args[1], params[1], rhs_ty);
                let idx_ty = self.check_expr(&args[2], Some(params[2]));
                self.check_coercion(&args[2], params[2], idx_ty);

                let norm_lhs = self.resolve_tv(lhs_ty);
                let norm_rhs = self.resolve_tv(rhs_ty);
                if norm_lhs != TypeId::ERROR && !self.ctx.type_registry.is_simd(norm_lhs) {
                    self.ctx
                        .struct_error(args[0].span, "`@simdShuffle` expects SIMD values")
                        .emit();
                }
                if norm_rhs != TypeId::ERROR && !self.ctx.type_registry.is_simd(norm_rhs) {
                    self.ctx
                        .struct_error(args[1].span, "`@simdShuffle` expects SIMD values")
                        .emit();
                }
                if norm_lhs != TypeId::ERROR && norm_rhs != TypeId::ERROR && norm_lhs != norm_rhs {
                    self.ctx
                        .struct_error(
                            args[1].span,
                            "`@simdShuffle` requires both input vectors to have the same type",
                        )
                        .emit();
                }

                if let Some((_, lanes)) = self.ctx.type_registry.simd_info(norm_lhs) {
                    let _ = self.eval_simd_shuffle_indices(&args[2], lanes);
                }

                Some(default_ret)
            }
            "@simdSwizzle" => {
                let value_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], value_ty);
                let idx_ty = self.check_expr(&args[1], Some(params[1]));
                self.check_coercion(&args[1], params[1], idx_ty);

                let norm_value = self.resolve_tv(value_ty);
                let Some((_, lanes)) = self.ctx.type_registry.simd_info(norm_value) else {
                    if norm_value != TypeId::ERROR {
                        self.ctx
                            .struct_error(args[0].span, "`@simdSwizzle` expects a SIMD value")
                            .emit();
                    }
                    return Some(TypeId::ERROR);
                };

                let _ = self.eval_simd_swizzle_indices(&args[1], lanes);
                Some(default_ret)
            }
            "@simdReverse" => {
                let value_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], value_ty);
                let norm_value = self.resolve_tv(value_ty);
                if norm_value != TypeId::ERROR && !self.ctx.type_registry.is_simd(norm_value) {
                    self.ctx
                        .struct_error(args[0].span, "`@simdReverse` expects a SIMD value")
                        .emit();
                }
                Some(default_ret)
            }
            "@simdRotateLeft" | "@simdRotateRight" => {
                let value_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], value_ty);
                let norm_value = self.resolve_tv(value_ty);
                let Some((_, lanes)) = self.ctx.type_registry.simd_info(norm_value) else {
                    if norm_value != TypeId::ERROR {
                        self.ctx
                            .struct_error(
                                args[0].span,
                                format!("`{}` expects a SIMD value", intrinsic_name),
                            )
                            .emit();
                    }
                    return Some(TypeId::ERROR);
                };
                let _ = self.eval_simd_rotate_amount(&args[1], intrinsic_name, lanes);
                Some(default_ret)
            }
            "@simdInterleaveLo"
            | "@simdInterleaveHi"
            | "@simdZipLo"
            | "@simdZipHi"
            | "@simdConcatLo"
            | "@simdConcatHi"
            | "@simdDeinterleaveLo"
            | "@simdDeinterleaveHi"
            | "@simdUnzipLo"
            | "@simdUnzipHi" => {
                let lhs_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], lhs_ty);
                let rhs_ty = self.check_expr(&args[1], Some(params[1]));
                self.check_coercion(&args[1], params[1], rhs_ty);

                let norm_lhs = self.resolve_tv(lhs_ty);
                let norm_rhs = self.resolve_tv(rhs_ty);

                if norm_lhs != TypeId::ERROR && !self.ctx.type_registry.is_simd(norm_lhs) {
                    self.ctx
                        .struct_error(
                            args[0].span,
                            format!("`{}` expects SIMD values", intrinsic_name),
                        )
                        .emit();
                }
                if norm_rhs != TypeId::ERROR && !self.ctx.type_registry.is_simd(norm_rhs) {
                    self.ctx
                        .struct_error(
                            args[1].span,
                            format!("`{}` expects SIMD values", intrinsic_name),
                        )
                        .emit();
                }
                if norm_lhs != TypeId::ERROR && norm_rhs != TypeId::ERROR && norm_lhs != norm_rhs {
                    self.ctx
                        .struct_error(
                            args[1].span,
                            format!(
                                "`{}` requires both input vectors to have the same type",
                                intrinsic_name
                            ),
                        )
                        .emit();
                }

                if let Some((_, lanes)) = self.ctx.type_registry.simd_info(norm_lhs)
                    && lanes % 2 != 0
                {
                    self.ctx
                        .struct_error(
                            args[0].span,
                            format!("`{}` requires an even SIMD lane count", intrinsic_name),
                        )
                        .emit();
                }

                Some(default_ret)
            }
            "@simdLowHalf" | "@simdHighHalf" => {
                let value_ty = self.check_expr(&args[0], None);
                let Some((norm_half, _, _)) = self.resolve_simd_intrinsic_result_type(
                    intrinsic_name,
                    callee_ty,
                    args[0].span,
                    "example: `@simdLowHalf[i32x2](i32x4.{ 1, 2, 3, 4 })`",
                ) else {
                    return Some(TypeId::ERROR);
                };

                if self
                    .check_simd_half_relation(
                        intrinsic_name,
                        SimdRelationOperand {
                            ty: value_ty,
                            span: args[0].span,
                            label: "value",
                        },
                        SimdRelationOperand {
                            ty: norm_half,
                            span: args[0].span,
                            label: "result",
                        },
                    )
                    .is_none()
                {
                    return Some(TypeId::ERROR);
                }

                Some(norm_half)
            }
            "@simdWithLowHalf" | "@simdWithHighHalf" => {
                let base_ty = self.check_expr(&args[0], None);
                let half_ty = self.check_expr(&args[1], None);
                let Some((norm_full, _, _)) = self.resolve_simd_intrinsic_result_type(
                    intrinsic_name,
                    callee_ty,
                    args[0].span,
                    "example: `@simdWithLowHalf[i32x4](i32x4.{ 10, 20, 30, 40 }, i32x2.{ 1, 2 })`",
                ) else {
                    return Some(TypeId::ERROR);
                };

                let norm_base = self.resolve_tv(base_ty);
                if norm_base != TypeId::ERROR && norm_base != norm_full {
                    let expected_full = self.ctx.ty_to_string(norm_full);
                    self.ctx
                        .struct_error(
                            args[0].span,
                            format!(
                                "`{}` base value must match the result SIMD type",
                                intrinsic_name
                            ),
                        )
                        .with_hint(format!("expected `{}`", expected_full))
                        .emit();
                }

                if self
                    .check_simd_half_relation(
                        intrinsic_name,
                        SimdRelationOperand {
                            ty: norm_full,
                            span: args[0].span,
                            label: "base",
                        },
                        SimdRelationOperand {
                            ty: half_ty,
                            span: args[1].span,
                            label: "half",
                        },
                    )
                    .is_none()
                {
                    return Some(TypeId::ERROR);
                }

                Some(norm_full)
            }
            "@simdReduceAdd" | "@simdReduceMul" | "@simdReduceAnd" | "@simdReduceOr"
            | "@simdReduceXor" | "@simdReduceMin" | "@simdReduceMax" => {
                let value_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], value_ty);
                let norm_value = self.resolve_tv(value_ty);
                let Some((elem_ty, _)) = self.ctx.type_registry.simd_info(norm_value) else {
                    if norm_value != TypeId::ERROR {
                        self.ctx
                            .struct_error(
                                args[0].span,
                                format!("`{}` expects a SIMD value", intrinsic_name),
                            )
                            .emit();
                    }
                    return Some(TypeId::ERROR);
                };

                match intrinsic_name {
                    "@simdReduceAdd" | "@simdReduceMul" => {
                        if !self.ctx.type_registry.is_integer(elem_ty)
                            && !self.ctx.type_registry.is_float(elem_ty)
                        {
                            self.ctx
                                .struct_error(
                                    args[0].span,
                                    format!(
                                        "`{}` requires integer or floating-point SIMD lanes",
                                        intrinsic_name
                                    ),
                                )
                                .emit();
                        }
                    }
                    "@simdReduceAnd" | "@simdReduceOr" | "@simdReduceXor" => {
                        if !self.ctx.type_registry.is_integer(elem_ty) && elem_ty != TypeId::BOOL {
                            self.ctx
                                .struct_error(
                                    args[0].span,
                                    format!(
                                        "`{}` requires integer or `boolxN` lanes",
                                        intrinsic_name
                                    ),
                                )
                                .emit();
                        }
                    }
                    "@simdReduceMin" | "@simdReduceMax" => {
                        if !self.ctx.type_registry.is_integer(elem_ty)
                            && !self.ctx.type_registry.is_float(elem_ty)
                        {
                            self.ctx
                                .struct_error(
                                    args[0].span,
                                    format!(
                                        "`{}` requires integer or floating-point SIMD lanes",
                                        intrinsic_name
                                    ),
                                )
                                .emit();
                        }
                    }
                    _ => {}
                }

                Some(elem_ty)
            }
            "@simdAbs" => {
                let value_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], value_ty);
                let norm_value = self.resolve_tv(value_ty);
                let Some((elem_ty, _)) = self.ctx.type_registry.simd_info(norm_value) else {
                    if norm_value != TypeId::ERROR {
                        self.ctx
                            .struct_error(args[0].span, "`@simdAbs` expects a SIMD value")
                            .emit();
                    }
                    return Some(TypeId::ERROR);
                };

                if !self.is_signed_integer_type(elem_ty)
                    && !self.ctx.type_registry.is_float(elem_ty)
                {
                    self.ctx
                        .struct_error(
                            args[0].span,
                            "`@simdAbs` requires signed integer or floating-point SIMD lanes",
                        )
                        .emit();
                }

                Some(default_ret)
            }
            "@simdSqrt" | "@simdFloor" | "@simdCeil" | "@simdTrunc" | "@simdRound" => {
                let value_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], value_ty);
                let norm_value = self.resolve_tv(value_ty);
                let Some((elem_ty, _)) = self.ctx.type_registry.simd_info(norm_value) else {
                    if norm_value != TypeId::ERROR {
                        self.ctx
                            .struct_error(
                                args[0].span,
                                format!("`{}` expects a SIMD value", intrinsic_name),
                            )
                            .emit();
                    }
                    return Some(TypeId::ERROR);
                };

                if !self.ctx.type_registry.is_float(elem_ty) {
                    self.ctx
                        .struct_error(
                            args[0].span,
                            format!("`{}` requires floating-point SIMD lanes", intrinsic_name),
                        )
                        .emit();
                }

                Some(default_ret)
            }
            "@simdMin" | "@simdMax" => {
                let lhs_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], lhs_ty);
                let rhs_ty = self.check_expr(&args[1], Some(params[1]));
                self.check_coercion(&args[1], params[1], rhs_ty);

                let norm_lhs = self.resolve_tv(lhs_ty);
                let norm_rhs = self.resolve_tv(rhs_ty);

                if norm_lhs != TypeId::ERROR && !self.ctx.type_registry.is_simd(norm_lhs) {
                    self.ctx
                        .struct_error(
                            args[0].span,
                            format!("`{}` expects SIMD values", intrinsic_name),
                        )
                        .emit();
                }
                if norm_rhs != TypeId::ERROR && !self.ctx.type_registry.is_simd(norm_rhs) {
                    self.ctx
                        .struct_error(
                            args[1].span,
                            format!("`{}` expects SIMD values", intrinsic_name),
                        )
                        .emit();
                }
                if norm_lhs != TypeId::ERROR && norm_rhs != TypeId::ERROR && norm_lhs != norm_rhs {
                    self.ctx
                        .struct_error(
                            args[1].span,
                            format!(
                                "`{}` requires both input vectors to have the same type",
                                intrinsic_name
                            ),
                        )
                        .emit();
                }

                if let Some((elem_ty, _)) = self.ctx.type_registry.simd_info(norm_lhs)
                    && !self.ctx.type_registry.is_integer(elem_ty)
                    && !self.ctx.type_registry.is_float(elem_ty)
                {
                    self.ctx
                        .struct_error(
                            args[0].span,
                            format!(
                                "`{}` requires integer or floating-point SIMD lanes",
                                intrinsic_name
                            ),
                        )
                        .emit();
                }

                Some(default_ret)
            }
            "@simdClamp" => {
                let value_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], value_ty);
                let lo_ty = self.check_expr(&args[1], Some(params[1]));
                self.check_coercion(&args[1], params[1], lo_ty);
                let hi_ty = self.check_expr(&args[2], Some(params[2]));
                self.check_coercion(&args[2], params[2], hi_ty);

                let norm_value = self.resolve_tv(value_ty);
                let norm_lo = self.resolve_tv(lo_ty);
                let norm_hi = self.resolve_tv(hi_ty);

                if norm_value != TypeId::ERROR && !self.ctx.type_registry.is_simd(norm_value) {
                    self.ctx
                        .struct_error(args[0].span, "`@simdClamp` expects SIMD values")
                        .emit();
                }
                if norm_lo != TypeId::ERROR && !self.ctx.type_registry.is_simd(norm_lo) {
                    self.ctx
                        .struct_error(args[1].span, "`@simdClamp` expects SIMD values")
                        .emit();
                }
                if norm_hi != TypeId::ERROR && !self.ctx.type_registry.is_simd(norm_hi) {
                    self.ctx
                        .struct_error(args[2].span, "`@simdClamp` expects SIMD values")
                        .emit();
                }

                if norm_value != TypeId::ERROR && norm_lo != TypeId::ERROR && norm_value != norm_lo
                {
                    self.ctx
                        .struct_error(
                            args[1].span,
                            "`@simdClamp` requires `value`, `lo`, and `hi` to have the same SIMD type",
                        )
                        .emit();
                }
                if norm_value != TypeId::ERROR && norm_hi != TypeId::ERROR && norm_value != norm_hi
                {
                    self.ctx
                        .struct_error(
                            args[2].span,
                            "`@simdClamp` requires `value`, `lo`, and `hi` to have the same SIMD type",
                        )
                        .emit();
                }

                if let Some((elem_ty, _)) = self.ctx.type_registry.simd_info(norm_value)
                    && !self.ctx.type_registry.is_integer(elem_ty)
                    && !self.ctx.type_registry.is_float(elem_ty)
                {
                    self.ctx
                        .struct_error(
                            args[0].span,
                            "`@simdClamp` requires integer or floating-point SIMD lanes",
                        )
                        .emit();
                }

                Some(default_ret)
            }
            "@simdLoad" => {
                let ptr_ty = self.check_expr(&args[0], None);
                let _ = self.eval_simd_align_arg(&args[1], intrinsic_name);

                let Some((norm_value, elem_ty, _)) = self.resolve_simd_intrinsic_result_type(
                    intrinsic_name,
                    callee_ty,
                    args[0].span,
                    "example: `@simdLoad[f32x4](ptr, 4)`",
                ) else {
                    return Some(TypeId::ERROR);
                };

                let norm_ptr = self.resolve_tv(ptr_ty);
                let expected_ptr_ty = self.ctx.ty_to_string(elem_ty);
                match self.ctx.type_registry.get(norm_ptr).clone() {
                    TypeKind::Pointer { elem, .. } => {
                        if elem != elem_ty && elem != TypeId::ERROR {
                            self.ctx
                                .struct_error(
                                    args[0].span,
                                    "`@simdLoad` pointer element type must match the SIMD lane type",
                                )
                                .with_hint(format!("expected pointer to `{}`", expected_ptr_ty))
                                .emit();
                        }
                    }
                    TypeKind::Error => {}
                    _ => {
                        self.ctx
                            .struct_error(args[0].span, "`@simdLoad` expects a raw pointer")
                            .emit();
                    }
                }

                Some(norm_value)
            }
            "@simdStore" => {
                let ptr_ty = self.check_expr(&args[0], None);
                let value_ty = self.check_expr(&args[1], Some(params[1]));
                self.check_coercion(&args[1], params[1], value_ty);
                let _ = self.eval_simd_align_arg(&args[2], intrinsic_name);

                let norm_value = self.resolve_tv(value_ty);
                let Some((elem_ty, _)) = self.ctx.type_registry.simd_info(norm_value) else {
                    if norm_value != TypeId::ERROR {
                        self.ctx
                            .struct_error(args[1].span, "`@simdStore` expects a SIMD value")
                            .emit();
                    }
                    return Some(default_ret);
                };

                let norm_ptr = self.resolve_tv(ptr_ty);
                let expected_ptr_ty = self.ctx.ty_to_string(elem_ty);
                match self.ctx.type_registry.get(norm_ptr).clone() {
                    TypeKind::Pointer { is_mut, elem } => {
                        if !is_mut {
                            self.ctx
                                .struct_error(
                                    args[0].span,
                                    "`@simdStore` requires a mutable raw pointer",
                                )
                                .emit();
                        }
                        if elem != elem_ty && elem != TypeId::ERROR {
                            self.ctx
                                .struct_error(
                                    args[0].span,
                                    "`@simdStore` pointer element type must match the SIMD lane type",
                                )
                                .with_hint(format!("expected pointer to `{}`", expected_ptr_ty))
                                .emit();
                        }
                    }
                    TypeKind::Error => {}
                    _ => {
                        self.ctx
                            .struct_error(
                                args[0].span,
                                "`@simdStore` expects a mutable raw pointer",
                            )
                            .emit();
                    }
                }

                Some(default_ret)
            }
            "@simdMaskedLoad" => {
                let ptr_ty = self.check_expr(&args[0], None);
                let mask_info = self.check_simd_mask_arg(&args[1], intrinsic_name);
                let or_else_ty = self.check_expr(&args[2], Some(params[2]));
                self.check_coercion(&args[2], params[2], or_else_ty);
                let _ = self.eval_simd_align_arg(&args[3], intrinsic_name);

                let Some((norm_value, elem_ty, lanes)) = self.resolve_simd_intrinsic_result_type(
                    intrinsic_name,
                    callee_ty,
                    args[0].span,
                    "example: `@simdMaskedLoad[f32x4](ptr, mask, fallback, 4)`",
                ) else {
                    return Some(TypeId::ERROR);
                };

                let norm_or_else = self.resolve_tv(or_else_ty);
                if norm_or_else != TypeId::ERROR && norm_or_else != norm_value {
                    let expected_ty = self.ctx.ty_to_string(norm_value);
                    self.ctx
                        .struct_error(
                            args[2].span,
                            "`@simdMaskedLoad` fallback value must match the result SIMD type",
                        )
                        .with_hint(format!("expected `{}`", expected_ty))
                        .emit();
                }

                if let Some((_, mask_lanes)) = mask_info {
                    self.check_simd_mask_lane_match(&args[1], intrinsic_name, mask_lanes, lanes);
                }

                let norm_ptr = self.resolve_tv(ptr_ty);
                let expected_ptr_ty = self.ctx.ty_to_string(elem_ty);
                match self.ctx.type_registry.get(norm_ptr).clone() {
                    TypeKind::Pointer { elem, .. } => {
                        if elem != elem_ty && elem != TypeId::ERROR {
                            self.ctx
                                .struct_error(
                                    args[0].span,
                                    "`@simdMaskedLoad` pointer element type must match the SIMD lane type",
                                )
                                .with_hint(format!("expected pointer to `{}`", expected_ptr_ty))
                                .emit();
                        }
                    }
                    TypeKind::Error => {}
                    _ => {
                        self.ctx
                            .struct_error(args[0].span, "`@simdMaskedLoad` expects a raw pointer")
                            .emit();
                    }
                }

                Some(norm_value)
            }
            "@simdMaskedStore" => {
                let ptr_ty = self.check_expr(&args[0], None);
                let mask_info = self.check_simd_mask_arg(&args[1], intrinsic_name);
                let value_ty = self.check_expr(&args[2], Some(params[2]));
                self.check_coercion(&args[2], params[2], value_ty);
                let _ = self.eval_simd_align_arg(&args[3], intrinsic_name);

                let norm_value = self.resolve_tv(value_ty);
                let Some((elem_ty, lanes)) = self.ctx.type_registry.simd_info(norm_value) else {
                    if norm_value != TypeId::ERROR {
                        self.ctx
                            .struct_error(args[2].span, "`@simdMaskedStore` expects a SIMD value")
                            .emit();
                    }
                    return Some(default_ret);
                };

                if let Some((_, mask_lanes)) = mask_info {
                    self.check_simd_mask_lane_match(&args[1], intrinsic_name, mask_lanes, lanes);
                }

                let norm_ptr = self.resolve_tv(ptr_ty);
                let expected_ptr_ty = self.ctx.ty_to_string(elem_ty);
                match self.ctx.type_registry.get(norm_ptr).clone() {
                    TypeKind::Pointer { is_mut, elem } => {
                        if !is_mut {
                            self.ctx
                                .struct_error(
                                    args[0].span,
                                    "`@simdMaskedStore` requires a mutable raw pointer",
                                )
                                .emit();
                        }
                        if elem != elem_ty && elem != TypeId::ERROR {
                            self.ctx
                                .struct_error(
                                    args[0].span,
                                    "`@simdMaskedStore` pointer element type must match the SIMD lane type",
                                )
                                .with_hint(format!("expected pointer to `{}`", expected_ptr_ty))
                                .emit();
                        }
                    }
                    TypeKind::Error => {}
                    _ => {
                        self.ctx
                            .struct_error(
                                args[0].span,
                                "`@simdMaskedStore` expects a mutable raw pointer",
                            )
                            .emit();
                    }
                }

                Some(default_ret)
            }
            "@simdGather" => {
                let ptr_ty = self.check_expr(&args[0], None);
                let _ = self.check_simd_index_ptr_arg(&args[1], intrinsic_name);

                let Some((norm_value, elem_ty, _)) = self.resolve_simd_intrinsic_result_type(
                    intrinsic_name,
                    callee_ty,
                    args[0].span,
                    "example: `@simdGather[f32x4](ptr, idx.[0].&)`",
                ) else {
                    return Some(TypeId::ERROR);
                };

                let norm_ptr = self.resolve_tv(ptr_ty);
                let expected_ptr_ty = self.ctx.ty_to_string(elem_ty);
                match self.ctx.type_registry.get(norm_ptr).clone() {
                    TypeKind::Pointer { elem, .. } => {
                        if elem != elem_ty && elem != TypeId::ERROR {
                            self.ctx
                                .struct_error(
                                    args[0].span,
                                    "`@simdGather` base pointer element type must match the SIMD lane type",
                                )
                                .with_hint(format!("expected pointer to `{}`", expected_ptr_ty))
                                .emit();
                        }
                    }
                    TypeKind::Error => {}
                    _ => {
                        self.ctx
                            .struct_error(args[0].span, "`@simdGather` expects a raw pointer")
                            .emit();
                    }
                }

                Some(norm_value)
            }
            "@simdScatter" => {
                let ptr_ty = self.check_expr(&args[0], None);
                let _ = self.check_simd_index_ptr_arg(&args[1], intrinsic_name);
                let value_ty = self.check_expr(&args[2], Some(params[2]));
                self.check_coercion(&args[2], params[2], value_ty);

                let norm_value = self.resolve_tv(value_ty);
                let Some((elem_ty, _)) = self.ctx.type_registry.simd_info(norm_value) else {
                    if norm_value != TypeId::ERROR {
                        self.ctx
                            .struct_error(args[2].span, "`@simdScatter` expects a SIMD value")
                            .emit();
                    }
                    return Some(default_ret);
                };

                let norm_ptr = self.resolve_tv(ptr_ty);
                let expected_ptr_ty = self.ctx.ty_to_string(elem_ty);
                match self.ctx.type_registry.get(norm_ptr).clone() {
                    TypeKind::Pointer { is_mut, elem } => {
                        if !is_mut {
                            self.ctx
                                .struct_error(
                                    args[0].span,
                                    "`@simdScatter` requires a mutable raw pointer",
                                )
                                .emit();
                        }
                        if elem != elem_ty && elem != TypeId::ERROR {
                            self.ctx
                                .struct_error(
                                    args[0].span,
                                    "`@simdScatter` base pointer element type must match the SIMD lane type",
                                )
                                .with_hint(format!("expected pointer to `{}`", expected_ptr_ty))
                                .emit();
                        }
                    }
                    TypeKind::Error => {}
                    _ => {
                        self.ctx
                            .struct_error(
                                args[0].span,
                                "`@simdScatter` expects a mutable raw pointer",
                            )
                            .emit();
                    }
                }

                Some(default_ret)
            }
            "@simdMaskedGather" => {
                let ptr_ty = self.check_expr(&args[0], None);
                let _ = self.check_simd_index_ptr_arg(&args[1], intrinsic_name);
                let mask_info = self.check_simd_mask_arg(&args[2], intrinsic_name);
                let or_else_ty = self.check_expr(&args[3], Some(params[3]));
                self.check_coercion(&args[3], params[3], or_else_ty);

                let Some((norm_value, elem_ty, lanes)) = self.resolve_simd_intrinsic_result_type(
                    intrinsic_name,
                    callee_ty,
                    args[0].span,
                    "example: `@simdMaskedGather[f32x4](ptr, idx.[0].&, mask, fallback)`",
                ) else {
                    return Some(TypeId::ERROR);
                };

                let norm_or_else = self.resolve_tv(or_else_ty);
                if norm_or_else != TypeId::ERROR && norm_or_else != norm_value {
                    let expected_ty = self.ctx.ty_to_string(norm_value);
                    self.ctx
                        .struct_error(
                            args[3].span,
                            "`@simdMaskedGather` fallback value must match the result SIMD type",
                        )
                        .with_hint(format!("expected `{}`", expected_ty))
                        .emit();
                }

                if let Some((_, mask_lanes)) = mask_info {
                    self.check_simd_mask_lane_match(&args[2], intrinsic_name, mask_lanes, lanes);
                }

                let norm_ptr = self.resolve_tv(ptr_ty);
                let expected_ptr_ty = self.ctx.ty_to_string(elem_ty);
                match self.ctx.type_registry.get(norm_ptr).clone() {
                    TypeKind::Pointer { elem, .. } => {
                        if elem != elem_ty && elem != TypeId::ERROR {
                            self.ctx
                                .struct_error(
                                    args[0].span,
                                    "`@simdMaskedGather` base pointer element type must match the SIMD lane type",
                                )
                                .with_hint(format!("expected pointer to `{}`", expected_ptr_ty))
                                .emit();
                        }
                    }
                    TypeKind::Error => {}
                    _ => {
                        self.ctx
                            .struct_error(args[0].span, "`@simdMaskedGather` expects a raw pointer")
                            .emit();
                    }
                }

                Some(norm_value)
            }
            "@simdMaskedScatter" => {
                let ptr_ty = self.check_expr(&args[0], None);
                let _ = self.check_simd_index_ptr_arg(&args[1], intrinsic_name);
                let mask_info = self.check_simd_mask_arg(&args[2], intrinsic_name);
                let value_ty = self.check_expr(&args[3], Some(params[3]));
                self.check_coercion(&args[3], params[3], value_ty);

                let norm_value = self.resolve_tv(value_ty);
                let Some((elem_ty, lanes)) = self.ctx.type_registry.simd_info(norm_value) else {
                    if norm_value != TypeId::ERROR {
                        self.ctx
                            .struct_error(args[3].span, "`@simdMaskedScatter` expects a SIMD value")
                            .emit();
                    }
                    return Some(default_ret);
                };

                if let Some((_, mask_lanes)) = mask_info {
                    self.check_simd_mask_lane_match(&args[2], intrinsic_name, mask_lanes, lanes);
                }

                let norm_ptr = self.resolve_tv(ptr_ty);
                let expected_ptr_ty = self.ctx.ty_to_string(elem_ty);
                match self.ctx.type_registry.get(norm_ptr).clone() {
                    TypeKind::Pointer { is_mut, elem } => {
                        if !is_mut {
                            self.ctx
                                .struct_error(
                                    args[0].span,
                                    "`@simdMaskedScatter` requires a mutable raw pointer",
                                )
                                .emit();
                        }
                        if elem != elem_ty && elem != TypeId::ERROR {
                            self.ctx
                                .struct_error(
                                    args[0].span,
                                    "`@simdMaskedScatter` base pointer element type must match the SIMD lane type",
                                )
                                .with_hint(format!("expected pointer to `{}`", expected_ptr_ty))
                                .emit();
                        }
                    }
                    TypeKind::Error => {}
                    _ => {
                        self.ctx
                            .struct_error(
                                args[0].span,
                                "`@simdMaskedScatter` expects a mutable raw pointer",
                            )
                            .emit();
                    }
                }

                Some(default_ret)
            }
            _ => None,
        }
    }
}
