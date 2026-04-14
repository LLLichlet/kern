use super::ExprChecker;
use crate::LayoutEngine;
use crate::checker::{ConstEvaluator, ConstValue, Substituter};
use crate::def::{Def, DefId};
use crate::passes::TypeResolver;
use crate::scope::{ScopeId, SymbolInfo, SymbolKind};
use crate::semantic::SemanticSymbolKind;
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_utils::{AtomicOrdering, FastHashMap, Span};
use std::time::Instant;

struct SimdRelationOperand<'a> {
    ty: TypeId,
    span: Span,
    label: &'a str,
}

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    fn is_signed_integer_type(&mut self, ty: TypeId) -> bool {
        matches!(
            self.resolve_tv(ty),
            TypeId::I8 | TypeId::I16 | TypeId::I32 | TypeId::I64 | TypeId::I128 | TypeId::ISIZE
        )
    }

    fn resolve_simd_intrinsic_result_type(
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

    fn eval_simd_align_arg(&mut self, arg: &Expr, intrinsic_name: &str) -> Option<u64> {
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

        Some(align)
    }

    fn eval_simd_rotate_amount(
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

    fn eval_simd_lane_indices(
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

            let Ok(idx) = u32::try_from(idx) else {
                self.ctx
                    .struct_error(
                        arg.span,
                        format!("`{}` indices must be non-negative", intrinsic_name),
                    )
                    .emit();
                return None;
            };

            if idx >= upper_bound {
                self.ctx
                    .struct_error(
                        arg.span,
                        format!(
                            "`{}` index {} is out of range for {}",
                            intrinsic_name, idx, range_desc
                        ),
                    )
                    .with_hint(format!("valid indices are 0 through {}", upper_bound - 1))
                    .emit();
                return None;
            }

            out.push(idx);
        }

        Some(out)
    }

    fn eval_simd_shuffle_indices(&mut self, arg: &Expr, lanes: u16) -> Option<Vec<u32>> {
        self.eval_simd_lane_indices(
            arg,
            "@simdShuffle",
            lanes,
            (lanes as u32) * 2,
            format!("two `{}`-lane input vectors", lanes),
            "example: `@simdShuffle(a, b, [4]u32.{ 0, 5, 2, 7 })`",
        )
    }

    fn eval_simd_swizzle_indices(&mut self, arg: &Expr, lanes: u16) -> Option<Vec<u32>> {
        self.eval_simd_lane_indices(
            arg,
            "@simdSwizzle",
            lanes,
            lanes as u32,
            format!("a `{}`-lane input vector", lanes),
            "example: `@simdSwizzle(a, [4]u32.{ 3, 0, 2, 1 })`",
        )
    }

    fn check_simd_index_ptr_arg(&mut self, arg: &Expr, intrinsic_name: &str) -> Option<TypeId> {
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

    fn check_simd_mask_arg(&mut self, arg: &Expr, intrinsic_name: &str) -> Option<(TypeId, u16)> {
        let mask_ty = self.check_expr(arg, None);
        let norm_mask = self.resolve_tv(mask_ty);
        let Some((elem_ty, lanes)) = self.ctx.type_registry.simd_info(norm_mask) else {
            if norm_mask != TypeId::ERROR {
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
        Some((norm_mask, lanes))
    }

    fn check_simd_mask_lane_match(
        &mut self,
        mask_arg: &Expr,
        intrinsic_name: &str,
        mask_lanes: u16,
        value_lanes: u16,
    ) {
        if mask_lanes != value_lanes {
            self.ctx
                .struct_error(
                    mask_arg.span,
                    format!(
                        "`{}` mask lane count must match the value lane count",
                        intrinsic_name
                    ),
                )
                .with_hint(format!(
                    "found {} mask lanes and {} value lanes",
                    mask_lanes, value_lanes
                ))
                .emit();
        }
    }

    fn check_simd_half_relation(
        &mut self,
        intrinsic_name: &str,
        full: SimdRelationOperand<'_>,
        half: SimdRelationOperand<'_>,
    ) -> Option<(TypeId, u16, u16)> {
        let norm_full = self.resolve_tv(full.ty);
        let Some((full_elem, full_lanes)) = self.ctx.type_registry.simd_info(norm_full) else {
            if norm_full != TypeId::ERROR {
                self.ctx
                    .struct_error(
                        full.span,
                        format!(
                            "`{}` expects `{}` to be a SIMD value",
                            intrinsic_name, full.label
                        ),
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
                        format!(
                            "`{}` expects `{}` to be a SIMD value",
                            intrinsic_name, half.label
                        ),
                    )
                    .emit();
            }
            return None;
        };

        if full_elem != half_elem {
            let full_elem_str = self.ctx.ty_to_string(full_elem);
            let half_elem_str = self.ctx.ty_to_string(half_elem);
            self.ctx
                .struct_error(
                    half.span,
                    format!(
                        "`{}` requires `{}` and `{}` to use the same SIMD lane type",
                        intrinsic_name, full.label, half.label
                    ),
                )
                .with_hint(format!(
                    "found `{}` lanes and `{}` lanes",
                    full_elem_str, half_elem_str
                ))
                .emit();
            return None;
        }

        if full_lanes != half_lanes.saturating_mul(2) {
            self.ctx
                .struct_error(
                    half.span,
                    format!(
                        "`{}` requires `{}` to have exactly twice as many lanes as `{}`",
                        intrinsic_name, full.label, half.label
                    ),
                )
                .with_hint(format!(
                    "found {} lanes and {} lanes",
                    full_lanes, half_lanes
                ))
                .emit();
            return None;
        }

        Some((full_elem, full_lanes, half_lanes))
    }

    fn resolve_current_scope_for_types(&mut self, span: Span, context: &str) -> Option<ScopeId> {
        match self.ctx.scopes.current_scope_id() {
            Some(scope) => Some(scope),
            None => {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Compiler ICE: missing active scope while resolving types for {}.",
                        context
                    ),
                );
                None
            }
        }
    }

    fn intrinsic_def_from_callee_ty(&self, callee_ty: TypeId) -> Option<DefId> {
        match self
            .ctx
            .type_registry
            .get(self.ctx.type_registry.normalize(callee_ty))
        {
            TypeKind::FnDef(def_id, _) => Some(*def_id),
            _ => None,
        }
    }

    fn intrinsic_generic_arg(&self, callee_ty: TypeId, index: usize) -> Option<TypeId> {
        match self
            .ctx
            .type_registry
            .get(self.ctx.type_registry.normalize(callee_ty))
        {
            TypeKind::FnDef(_, args) => args.get(index).copied(),
            _ => None,
        }
    }

    fn eval_atomic_order_arg(
        &mut self,
        arg: &Expr,
        arg_label: &str,
        validator: impl Fn(AtomicOrdering) -> bool,
        hint: &str,
    ) -> Option<AtomicOrdering> {
        let arg_ty = self.check_expr(arg, None);
        if arg_ty == TypeId::ERROR {
            return None;
        }

        let mut evaluator = ConstEvaluator::new(self.ctx);
        let order = match evaluator.eval_inner(arg, 0) {
            Ok(crate::checker::ConstValue::Int(value)) => value,
            Ok(_) => {
                self.ctx
                    .struct_error(
                        arg.span,
                        format!(
                            "atomic ordering `{}` must evaluate to an integer constant",
                            arg_label
                        ),
                    )
                    .with_hint(hint)
                    .emit();
                return None;
            }
            Err(_) => return None,
        };

        let Some(ordering) = AtomicOrdering::from_abi_const(order) else {
            self.ctx
                .struct_error(
                    arg.span,
                    format!(
                        "invalid atomic ordering constant `{}` for `{}`",
                        order, arg_label
                    ),
                )
                .with_hint("valid values are 0=Relaxed, 1=Acquire, 2=Release, 3=AcqRel, 4=SeqCst")
                .emit();
            return None;
        };

        if !validator(ordering) {
            self.ctx
                .struct_error(
                    arg.span,
                    format!(
                        "atomic ordering `{}` is not valid for `{}`",
                        ordering.as_str(),
                        arg_label
                    ),
                )
                .with_hint(hint)
                .emit();
            return None;
        }

        self.ctx.atomic_orderings.insert(arg.id, ordering);
        Some(ordering)
    }

    fn check_atomic_target_type(
        &mut self,
        ty: TypeId,
        span: Span,
        intrinsic_name: &str,
        allow_pointers: bool,
    ) {
        let norm = self.resolve_tv(ty);
        if norm == TypeId::ERROR {
            return;
        }

        let is_supported = self.ctx.type_registry.is_integer(norm)
            || (allow_pointers
                && matches!(self.ctx.type_registry.get(norm), TypeKind::Pointer { .. }));

        if !is_supported {
            let ty_str = self.ctx.ty_to_string(norm);
            let kind_hint = if allow_pointers {
                "expected an integer type or a normal raw pointer (`*T` / `*mut T`)"
            } else {
                "expected an integer type"
            };
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "`{}` does not support atomic type `{}`",
                        intrinsic_name, ty_str
                    ),
                )
                .with_hint(kind_hint)
                .emit();
            return;
        }

        let mut layout = LayoutEngine::new(self.ctx);
        let bits = layout.compute_type_size(norm) * 8;
        let max_bits = self.ctx.sess.target.max_lock_free_atomic_bits();
        if bits > max_bits {
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "target `{}` supports lock-free atomics only up to {} bits, but `{}` is {} bits",
                        self.ctx.sess.target.triple,
                        max_bits,
                        self.ctx.ty_to_string(norm),
                        bits
                    ),
                )
                .with_hint(
                    "Kern is freestanding and cannot fall back to compiler runtime helpers like `__atomic_*`",
                )
                .emit();
        }
    }

    fn check_atomic_intrinsic_call(
        &mut self,
        intrinsic_name: &str,
        callee_ty: TypeId,
        args: &[Expr],
        params: &[TypeId],
    ) -> bool {
        match intrinsic_name {
            "@atomicLoad" => {
                let ptr_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], ptr_ty);
                if let Some(target_ty) = self.intrinsic_generic_arg(callee_ty, 0) {
                    self.check_atomic_target_type(target_ty, args[0].span, intrinsic_name, true);
                }
                let _ = self.eval_atomic_order_arg(
                    &args[1],
                    "load order",
                    AtomicOrdering::valid_for_load,
                    "load order must be Relaxed, Acquire, or SeqCst",
                );
                true
            }
            "@atomicStore" => {
                let ptr_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], ptr_ty);
                let val_ty = self.check_expr(&args[1], Some(params[1]));
                self.check_coercion(&args[1], params[1], val_ty);
                if let Some(target_ty) = self.intrinsic_generic_arg(callee_ty, 0) {
                    self.check_atomic_target_type(target_ty, args[0].span, intrinsic_name, true);
                }
                let _ = self.eval_atomic_order_arg(
                    &args[2],
                    "store order",
                    AtomicOrdering::valid_for_store,
                    "store order must be Relaxed, Release, or SeqCst",
                );
                true
            }
            "@atomicCas" | "@atomicCasWeak" => {
                let ptr_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], ptr_ty);
                let expected_ty = self.check_expr(&args[1], Some(params[1]));
                self.check_coercion(&args[1], params[1], expected_ty);
                let desired_ty = self.check_expr(&args[2], Some(params[2]));
                self.check_coercion(&args[2], params[2], desired_ty);

                if let Some(target_ty) = self.intrinsic_generic_arg(callee_ty, 0) {
                    self.check_atomic_target_type(target_ty, args[0].span, intrinsic_name, true);
                }

                let success = self.eval_atomic_order_arg(
                    &args[3],
                    "cmpxchg success order",
                    AtomicOrdering::valid_for_rmw,
                    "success order must be Relaxed, Acquire, Release, AcqRel, or SeqCst",
                );
                let failure = self.eval_atomic_order_arg(
                    &args[4],
                    "cmpxchg failure order",
                    AtomicOrdering::valid_for_cmpxchg_failure,
                    "failure order must be Relaxed, Acquire, or SeqCst",
                );

                if let (Some(success), Some(failure)) = (success, failure)
                    && !failure.failure_not_stronger_than(success)
                {
                    self.ctx
                        .struct_error(
                            args[4].span,
                            format!(
                                "cmpxchg failure ordering `{}` cannot be stronger than success ordering `{}`",
                                failure.as_str(),
                                success.as_str()
                            ),
                        )
                        .with_hint("valid examples: (AcqRel, Acquire), (SeqCst, SeqCst), (Relaxed, Relaxed)")
                        .emit();
                }
                true
            }
            "@atomicXchg" => {
                let ptr_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], ptr_ty);
                let val_ty = self.check_expr(&args[1], Some(params[1]));
                self.check_coercion(&args[1], params[1], val_ty);
                if let Some(target_ty) = self.intrinsic_generic_arg(callee_ty, 0) {
                    self.check_atomic_target_type(target_ty, args[0].span, intrinsic_name, true);
                }
                let _ = self.eval_atomic_order_arg(
                    &args[2],
                    "atomicrmw order",
                    AtomicOrdering::valid_for_rmw,
                    "atomic RMW order must be Relaxed, Acquire, Release, AcqRel, or SeqCst",
                );
                true
            }
            "@atomicRmwAdd" | "@atomicRmwSub" | "@atomicRmwAnd" | "@atomicRmwNand"
            | "@atomicRmwOr" | "@atomicRmwXor" | "@atomicRmwMax" | "@atomicRmwMin"
            | "@atomicRmwUMax" | "@atomicRmwUMin" => {
                let ptr_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], ptr_ty);
                let val_ty = self.check_expr(&args[1], Some(params[1]));
                self.check_coercion(&args[1], params[1], val_ty);
                if let Some(target_ty) = self.intrinsic_generic_arg(callee_ty, 0) {
                    self.check_atomic_target_type(target_ty, args[0].span, intrinsic_name, false);
                }
                let _ = self.eval_atomic_order_arg(
                    &args[2],
                    "atomicrmw order",
                    AtomicOrdering::valid_for_rmw,
                    "atomic RMW order must be Relaxed, Acquire, Release, AcqRel, or SeqCst",
                );
                true
            }
            "@fence" => {
                let _ = self.eval_atomic_order_arg(
                    &args[0],
                    "fence order",
                    AtomicOrdering::valid_for_fence,
                    "fence order must be Acquire, Release, AcqRel, or SeqCst",
                );
                true
            }
            _ => false,
        }
    }

    fn check_simd_intrinsic_call(
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

    fn generic_target_identity(
        &mut self,
        target_norm: TypeId,
        span: Span,
    ) -> Option<(DefId, Vec<TypeId>)> {
        match self.ctx.type_registry.get(target_norm) {
            TypeKind::FnDef(id, args)
            | TypeKind::Def(id, args)
            | TypeKind::Enum(id, args)
            | TypeKind::TraitObject(id, args, _) => Some((*id, args.clone())),
            _ => {
                self.ctx
                    .struct_error(
                        span,
                        "this expression does not support generic instantiation",
                    )
                    .emit();
                None
            }
        }
    }

    fn resolve_generic_instantiation_types(
        &mut self,
        types: &[ast::TypeNode],
        span: Span,
    ) -> Option<Vec<TypeId>> {
        let scope = self.resolve_current_scope_for_types(span, "generic instantiation")?;
        let mut resolver = TypeResolver::new(self.ctx);

        let mut arg_tys = Vec::with_capacity(types.len());
        for ty_node in types {
            arg_tys.push(resolver.resolve_type(ty_node, scope));
        }
        Some(arg_tys)
    }

    fn instantiate_call_signature(
        &mut self,
        callee_ty: TypeId,
        raw_sig: TypeId,
        generics: &[ast::GenericParam],
        generic_args: &[TypeId],
    ) -> TypeId {
        if generics.is_empty() || generic_args.is_empty() {
            return raw_sig;
        }

        if let Some(&cached_sig) = self.ctx.call_signature_instantiation_cache.get(&callee_ty) {
            return cached_sig;
        }

        let mut map = FastHashMap::default();
        for (param, generic_arg) in generics.iter().zip(generic_args.iter()) {
            map.insert(param.name, *generic_arg);
        }

        let sig_ty = if map.is_empty() {
            raw_sig
        } else {
            let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
            subst.substitute(raw_sig)
        };
        self.ctx
            .call_signature_instantiation_cache
            .insert(callee_ty, sig_ty);
        sig_ty
    }

    pub(crate) fn check_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> TypeId {
        // 1. Intercept `@asm` macro invocations.
        if let ExprKind::Identifier(sym) = &callee.kind
            && self.ctx.resolve(*sym) == "@asm"
        {
            self.ctx.node_types.insert(callee.id, TypeId::VOID);
            return self.check_asm_call(args, span);
        }

        let callee_ty = self.check_expr(callee, None);
        let norm_callee = self.resolve_tv(callee_ty);

        if norm_callee == TypeId::ERROR {
            // Keep the AST type cache structurally complete.
            for arg in args {
                self.check_expr(arg, None);
            }
            return TypeId::ERROR;
        }

        // 2. Detect method calls and extract receiver information.
        let (is_method, receiver_ty) = self.resolve_method_context(callee);
        let has_user_explicit_generics =
            matches!(callee.kind, ExprKind::GenericInstantiation { .. });

        // 3. Infer generic arguments and recover the final callee signature.
        let signature_started = Instant::now();
        let (sig_ty, inferred_callee_ty, inferred_arg_tys) = self.deduce_and_resolve_signature(
            norm_callee,
            args,
            is_method,
            receiver_ty,
            callee.span,
            has_user_explicit_generics,
        );
        self.ctx.expr_timing_stats.call_signature += signature_started.elapsed();

        // 4. Write inferred generic arguments back into the AST-visible callee type.
        if let Some(fixed_ty) = inferred_callee_ty {
            self.ctx.node_types.insert(callee.id, fixed_ty);
        }

        // 5. Validate the final signature and dispatch strategy.
        // Extract call parameters, return type, and varargs metadata without cloning parameter
        // vectors on every call-site check.
        let (params_ptr, ret, is_variadic) = match self.ctx.type_registry.get(sig_ty) {
            // A. Plain functions.
            TypeKind::Function {
                params,
                ret,
                is_variadic,
            } => (std::ptr::from_ref(params.as_slice()), *ret, *is_variadic),

            // B. Closure fat pointers (`*Fn`).
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let inner_norm = self.ctx.type_registry.normalize(*elem);
                if let TypeKind::ClosureInterface { params, ret } =
                    self.ctx.type_registry.get(inner_norm)
                {
                    (std::ptr::from_ref(params.as_slice()), *ret, false)
                } else {
                    let callee_str = self.ctx.ty_to_string(callee_ty);
                    self.ctx
                        .struct_error(callee.span, "expression is not callable")
                        .with_hint(format!("type is `{}`", callee_str))
                        .emit();
                    return TypeId::ERROR;
                }
            }

            // C. Everything else is not callable.
            _ => {
                let callee_str = self.ctx.ty_to_string(callee_ty);
                self.ctx
                    .struct_error(callee.span, "expression is not callable")
                    .with_hint(format!("type is `{}`", callee_str))
                    .emit();
                return TypeId::ERROR;
            }
        };
        // Safety: signature parameter buffers are immutable after interning; later lookups may
        // grow the registry, but they do not mutate these interned parameter slices.
        let params = unsafe { &*params_ptr };

        self.check_call_arity(args.len(), params.len(), is_method, is_variadic, span);

        if is_method && !params.is_empty() {
            let expected_self = params[0];
            self.check_method_receiver(expected_self, receiver_ty, callee);
            if receiver_ty != expected_self
                && let ExprKind::FieldAccess { lhs, .. } = &callee.kind
            {
                self.ctx.node_types.insert(lhs.id, expected_self);
            }
        }

        let mut final_ret = ret;
        let intrinsic_started = Instant::now();
        let handled_intrinsic = self
            .intrinsic_def_from_callee_ty(inferred_callee_ty.unwrap_or(norm_callee))
            .and_then(|def_id| {
                let intrinsic_name = match &self.ctx.defs[def_id.0 as usize] {
                    Def::Function(func) if func.is_intrinsic => {
                        Some(self.ctx.resolve(func.name).to_string())
                    }
                    _ => None,
                }?;

                let atomic_handled = self.check_atomic_intrinsic_call(
                    intrinsic_name.as_str(),
                    inferred_callee_ty.unwrap_or(norm_callee),
                    args,
                    params,
                );
                let simd_ret = self.check_simd_intrinsic_call(
                    intrinsic_name.as_str(),
                    inferred_callee_ty.unwrap_or(norm_callee),
                    args,
                    params,
                    ret,
                );
                if let Some(simd_ret) = simd_ret {
                    final_ret = simd_ret;
                }

                Some(atomic_handled || simd_ret.is_some())
            })
            .unwrap_or(false);
        self.ctx.expr_timing_stats.call_intrinsic += intrinsic_started.elapsed();

        if !handled_intrinsic {
            let arguments_started = Instant::now();
            self.check_call_arguments(
                args,
                params,
                is_method,
                is_variadic,
                inferred_arg_tys.as_deref(),
            );
            self.ctx.expr_timing_stats.call_arguments += arguments_started.elapsed();
        }
        final_ret
    }

    /// Helper: infer generic arguments and resolve the instantiated signature.
    pub(crate) fn deduce_and_resolve_signature(
        &mut self,
        norm_callee: TypeId,
        args: &[Expr],
        is_method: bool,
        receiver_ty: TypeId,
        span: Span,
        has_user_explicit_generics: bool,
    ) -> (TypeId, Option<TypeId>, Option<Vec<Option<TypeId>>>) {
        if let TypeKind::FnDef(def_id, explicit_args) = self.ctx.type_registry.get(norm_callee) {
            let def_id = *def_id;
            let explicit_args_ptr = std::ptr::from_ref(explicit_args.as_slice());
            let explicit_args_len = explicit_args.len();
            // Safety: interned `FnDef` generic arguments are immutable in the type registry.
            let explicit_args = unsafe { &*explicit_args_ptr };
            let Some(function_ptr) =
                self.ctx
                    .defs
                    .get(def_id.0 as usize)
                    .and_then(|def| match def {
                        Def::Function(func) => Some(std::ptr::from_ref(func)),
                        _ => None,
                    })
            else {
                let other = &self.ctx.defs[def_id.0 as usize];
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Compiler ICE: expected function Def for callee, found `{:?}`.",
                        other
                    ),
                );
                return (TypeId::ERROR, None, None);
            };
            // Safety: semantic defs are immutable while checking expressions.
            let function = unsafe { &*function_ptr };
            let Some(raw_sig) = function.resolved_sig else {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Compiler ICE: function `{}` has no resolved signature during call checking.",
                        self.ctx.resolve(function.name)
                    ),
                );
                return (TypeId::ERROR, None, None);
            };
            let fn_name_id = function.name;
            let generics = function.generics.as_slice();
            let generics_count = generics.len();

            // Monomorphic callees can return their original signature directly.
            if generics_count == 0 {
                return (raw_sig, None, None);
            }

            if explicit_args_len > generics_count {
                let name_str = self.ctx.resolve(fn_name_id).to_string();
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Compiler ICE: function `{}` carried {} generic arguments, but only {} generic parameters exist.",
                        name_str,
                        explicit_args_len,
                        generics_count
                    ),
                );
                return (TypeId::ERROR, None, None);
            }

            // Rule A: the user supplied a complete explicit generic argument list.
            if explicit_args.len() == generics_count {
                return (
                    self.instantiate_call_signature(norm_callee, raw_sig, generics, explicit_args),
                    None,
                    None,
                );
            }

            // Rule B: partial user-written generic lists are rejected, except for receiver-bound prefixes.
            if has_user_explicit_generics && !explicit_args.is_empty() {
                let name_str = self.ctx.resolve(fn_name_id).to_string();
                self.ctx.struct_error(span, format!("function `{}` requires exactly {} generic arguments, but {} were provided", name_str, generics_count, explicit_args.len()))
                    .with_hint("either provide all generic arguments or omit them entirely to let the compiler infer them")
                    .emit();
                return (TypeId::ERROR, None, None);
            }

            // Rule C: if generics are omitted entirely, infer them from usage.
            let mut map = FastHashMap::default();
            for (param, explicit_arg) in generics.iter().zip(explicit_args.iter()) {
                map.insert(param.name, *explicit_arg);
            }
            let raw_params_ptr = match self.ctx.type_registry.get(raw_sig) {
                TypeKind::Function { params, .. } => std::ptr::from_ref(params.as_slice()),
                other => {
                    self.ctx.emit_ice(
                        span,
                        format!(
                            "Compiler ICE: expected function signature type during call checking, found `{:?}`.",
                            other
                        ),
                    );
                    return (TypeId::ERROR, None, None);
                }
            };
            // Safety: the resolved signature is immutable; local interning may grow the registry,
            // but it does not mutate the parameter buffer referenced by this slice.
            let raw_params = unsafe { &*raw_params_ptr };
            let raw_param_count = raw_params.len();
            if raw_param_count == 0 && is_method {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Compiler ICE: method call `{}` resolved to a signature without receiver parameter.",
                        self.ctx.resolve(fn_name_id)
                    ),
                );
                return (TypeId::ERROR, None, None);
            }
            let mut inferred_arg_tys = vec![None; args.len()];

            let param_offset = if is_method { 1 } else { 0 };

            // 1. Infer from the receiver first, for example in `list.push(...)`.
            if is_method {
                let mut stripped_recv = self.resolve_tv(receiver_ty);
                let expected_recv =
                    self.resolve_tv(raw_params.first().copied().unwrap_or(TypeId::ERROR));
                if let TypeKind::Pointer { is_mut: false, .. } =
                    self.ctx.type_registry.get(expected_recv)
                {
                    if let TypeKind::Pointer { is_mut: true, elem } =
                        self.ctx.type_registry.get(stripped_recv).clone()
                    {
                        stripped_recv = self.ctx.type_registry.intern(TypeKind::Pointer {
                            is_mut: false,
                            elem,
                        });
                    }
                } else if let TypeKind::VolatilePtr { is_mut: false, .. } =
                    self.ctx.type_registry.get(expected_recv)
                    && let TypeKind::VolatilePtr { is_mut: true, elem } =
                        self.ctx.type_registry.get(stripped_recv).clone()
                {
                    stripped_recv = self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                        is_mut: false,
                        elem,
                    });
                }

                self.unify(expected_recv, stripped_recv, &mut map);
            }

            // 2. Infer from positional arguments.
            for (i, arg) in args.iter().enumerate() {
                let sig_idx = i + param_offset;
                let expected_param = raw_params.get(sig_idx).copied();
                if let Some(expected_param) = expected_param {
                    let arg_ty = self.check_expr(arg, None);
                    inferred_arg_tys[i] = Some(arg_ty);
                    let arg_norm = self.resolve_tv(arg_ty);
                    if arg_norm != TypeId::ERROR {
                        self.unify(expected_param, arg_norm, &mut map);
                    }
                }
            }

            // 3. Ensure every generic parameter was inferred.
            let mut missing_generics = Vec::new();
            let mut resolved_args = Vec::new();
            for param in generics {
                if let Some(&inferred_ty) = map.get(&param.name) {
                    resolved_args.push(inferred_ty);
                } else {
                    missing_generics.push(self.ctx.resolve(param.name).to_string());
                }
            }

            // Rule D: report an error if any generic parameter remains unknown.
            if !missing_generics.is_empty() {
                let name_str = self.ctx.resolve(fn_name_id).to_string();
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "cannot infer generic type(s) `{}` for function `{}`",
                            missing_generics.join(", "),
                            name_str
                        ),
                    )
                    .with_hint("the compiler needs these generic types to be explicitly specified")
                    .emit();
                return (TypeId::ERROR, None, Some(inferred_arg_tys));
            }

            self.check_generic_bounds(span, def_id, generics, &resolved_args);

            // Build the instantiated `FnDef` type for later AST updates.
            let inferred_callee_ty = self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, resolved_args));
            let inferred_args_ptr = match self.ctx.type_registry.get(inferred_callee_ty) {
                TypeKind::FnDef(_, args) => std::ptr::from_ref(args.as_slice()),
                _ => unreachable!("just interned FnDef must remain a FnDef"),
            };
            // Safety: the inferred `FnDef` arguments are interned immutably in the type registry.
            let inferred_args = unsafe { &*inferred_args_ptr };
            return (
                self.instantiate_call_signature(
                    inferred_callee_ty,
                    raw_sig,
                    generics,
                    inferred_args,
                ),
                Some(inferred_callee_ty),
                Some(inferred_arg_tys),
            );
        }

        (norm_callee, None, None)
    }

    /// Helper 2: detect method-call syntax and extract the receiver type.
    pub(crate) fn resolve_method_context(&self, callee: &Expr) -> (bool, TypeId) {
        if let ExprKind::FieldAccess { lhs, .. } = &callee.kind {
            // Use the resolved type to distinguish modules from real receivers.
            let callee_node_ty = self
                .ctx
                .node_types
                .get(&callee.id)
                .copied()
                .unwrap_or(TypeId::ERROR);

            let lhs_node_ty = self
                .ctx
                .node_types
                .get(&lhs.id)
                .copied()
                .unwrap_or(TypeId::ERROR);

            let norm_lhs = self.ctx.type_registry.normalize(lhs_node_ty);

            // Module-qualified syntax is not a value receiver.
            if matches!(self.ctx.type_registry.get(norm_lhs), TypeKind::Module(..)) {
                return (false, TypeId::ERROR);
            }

            let norm_node_ty = self.ctx.type_registry.normalize(callee_node_ty);

            if matches!(
                self.ctx.type_registry.get(norm_node_ty),
                TypeKind::FnDef(..) | TypeKind::Function { .. }
            ) {
                return (true, lhs_node_ty);
            }
        }
        (false, TypeId::ERROR)
    }

    /// Helper 3: validate call arity.
    pub(crate) fn check_call_arity(
        &mut self,
        arg_count: usize,
        param_count: usize,
        is_method: bool,
        is_variadic: bool,
        span: Span,
    ) {
        let expected_arg_count = if is_method {
            param_count.saturating_sub(1)
        } else {
            param_count
        };

        if is_variadic {
            if arg_count < expected_arg_count {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "function expects at least {} arguments, but {} were provided",
                            expected_arg_count, arg_count
                        ),
                    )
                    .emit();
            }
        } else if arg_count != expected_arg_count {
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "function expects exactly {} arguments, but {} were provided",
                        expected_arg_count, arg_count
                    ),
                )
                .emit();
        }
    }

    /// Helper 4: enforce Kern-specific receiver compatibility rules.
    fn check_method_receiver(&mut self, expected_self: TypeId, receiver_ty: TypeId, expr: &Expr) {
        let norm_expected = self.resolve_tv(expected_self);

        if !self.check_coercion(expr, expected_self, receiver_ty) {
            let is_exp_ptr = matches!(
                self.ctx.type_registry.get(norm_expected),
                TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
            );

            if is_exp_ptr {
                self.ctx.struct_error(expr.span, "method receiver type mismatch")
                    .with_hint("the method expects a pointer receiver")
                    .with_hint("Kern does not implicitly take addresses for method calls. Try using `(&obj).method()` or `obj.&.method()`")
                    .emit();
            }
        }
    }

    /// Helper 5: check argument coercions, including C ABI varargs promotions.
    fn check_call_arguments(
        &mut self,
        args: &[Expr],
        params: &[TypeId],
        is_method: bool,
        _is_variadic: bool,
        inferred_arg_tys: Option<&[Option<TypeId>]>,
    ) {
        let param_offset = if is_method { 1 } else { 0 };

        for (i, arg) in args.iter().enumerate() {
            let sig_param_idx = i + param_offset;

            if sig_param_idx < params.len() {
                // 1. Check a regular fixed parameter.
                let expected = params[sig_param_idx];
                let arg_ty = inferred_arg_tys
                    .and_then(|tys| tys.get(i))
                    .and_then(|ty| *ty)
                    .unwrap_or_else(|| self.check_expr(arg, Some(expected)));
                self.check_coercion(arg, expected, arg_ty);
            } else {
                // 2. Check trailing variadic arguments under C ABI rules.
                let arg_ty = inferred_arg_tys
                    .and_then(|tys| tys.get(i))
                    .and_then(|ty| *ty)
                    .unwrap_or_else(|| self.check_expr(arg, None));
                let norm_arg = self.resolve_tv(arg_ty);

                if norm_arg == TypeId::ERROR {
                    continue;
                }

                // C ABI integral promotion requires at least 32 bits for variadic integers.
                let is_small_int = matches!(
                    norm_arg,
                    TypeId::I8 | TypeId::I16 | TypeId::U8 | TypeId::U16
                );

                if is_small_int {
                    self.ctx.struct_error(arg.span, "C ABI requires integer arguments passed to `...` to be at least 32-bit")
                        .with_hint("please cast it explicitly (e.g., `as i32` or `as u32`)")
                        .emit();
                } else if norm_arg == TypeId::F32 {
                    // C ABI floating-point promotion upgrades variadic floats to `f64`.
                    self.ctx
                        .struct_error(
                            arg.span,
                            "C ABI requires float arguments passed to `...` to be 64-bit",
                        )
                        .with_hint("please cast it explicitly (e.g., `as f64`)")
                        .emit();
                }
            }
        }
    }

    pub(crate) fn check_generic_instantiation(
        &mut self,
        target: &Expr,
        types: &[ast::TypeNode],
        span: Span,
    ) -> TypeId {
        let target_ty = self.check_expr(target, None);
        let target_norm = self.resolve_tv(target_ty);

        if target_norm == TypeId::ERROR {
            return TypeId::ERROR;
        }

        let Some(resolved_arg_tys) = self.resolve_generic_instantiation_types(types, span) else {
            return TypeId::ERROR;
        };
        let arg_tys = resolved_arg_tys;

        let Some((def_id, _)) = self.generic_target_identity(target_norm, span) else {
            return TypeId::ERROR;
        };

        let generics = {
            let def = &self.ctx.defs[def_id.0 as usize];
            match def {
                Def::Function(f) => f.generics.clone(),
                Def::Struct(s) => s.generics.clone(),
                Def::Union(u) => u.generics.clone(),
                Def::TypeAlias(t) => t.generics.clone(),
                Def::Enum(e) => e.generics.clone(),
                Def::Trait(t) => t.generics.clone(),
                other => {
                    self.ctx.emit_ice(
                        span,
                        format!(
                            "Compiler ICE: generic instantiation resolved to unsupported def `{:?}`.",
                            other
                        ),
                    );
                    return TypeId::ERROR;
                }
            }
        };

        if generics.len() != arg_tys.len() {
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "expected {} generic arguments, but {} were provided",
                        generics.len(),
                        arg_tys.len()
                    ),
                )
                .emit();
            return TypeId::ERROR;
        }

        self.check_generic_bounds(span, def_id, &generics, &arg_tys);

        match self.ctx.type_registry.get(target_norm) {
            TypeKind::FnDef(..) => self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, arg_tys)),
            TypeKind::Enum(..) => self
                .ctx
                .type_registry
                .intern(TypeKind::Enum(def_id, arg_tys)),
            TypeKind::TraitObject(..) => self
                .ctx
                .type_registry
                .intern(TypeKind::TraitObject(def_id, arg_tys, Vec::new())),
            _ => self
                .ctx
                .type_registry
                .intern(TypeKind::Def(def_id, arg_tys)),
        }
    }

    fn check_generic_bounds(
        &mut self,
        span: Span,
        def_id: DefId,
        generics: &[ast::GenericParam],
        arg_tys: &[TypeId],
    ) {
        // Fast path: most generic items do not carry additional trait obligations.
        let has_where_clauses = match &self.ctx.defs[def_id.0 as usize] {
            Def::Function(f) => !f.where_clauses.is_empty(),
            Def::Struct(s) => !s.where_clauses.is_empty(),
            Def::Union(u) => !u.where_clauses.is_empty(),
            Def::TypeAlias(t) => !t.where_clauses.is_empty(),
            Def::Impl(i) => !i.where_clauses.is_empty(),
            Def::Enum(e) => !e.where_clauses.is_empty(),
            Def::Trait(t) => !t.where_clauses.is_empty(),
            _ => false,
        };
        if !has_where_clauses {
            return;
        }

        // 1. Extract the callee's where-clauses only when they are actually present.
        let where_clauses_ptr = match &self.ctx.defs[def_id.0 as usize] {
            Def::Function(f) => std::ptr::from_ref(f.where_clauses.as_slice()),
            Def::Struct(s) => std::ptr::from_ref(s.where_clauses.as_slice()),
            Def::Union(u) => std::ptr::from_ref(u.where_clauses.as_slice()),
            Def::TypeAlias(t) => std::ptr::from_ref(t.where_clauses.as_slice()),
            Def::Impl(i) => std::ptr::from_ref(i.where_clauses.as_slice()),
            Def::Enum(e) => std::ptr::from_ref(e.where_clauses.as_slice()),
            Def::Trait(t) => std::ptr::from_ref(t.where_clauses.as_slice()),
            _ => return,
        };
        // Safety: semantic defs stay immutable while call checking walks their bounds.
        let where_clauses = unsafe { &*where_clauses_ptr };

        // 2. Build the generic argument substitution map.
        let mut map = FastHashMap::default();
        for (i, param) in generics.iter().enumerate() {
            if i < arg_tys.len() {
                map.insert(param.name, arg_tys[i]);
            }
        }

        // 3. Stream each instantiated obligation directly into trait checking.
        for clause in where_clauses {
            let original_target = self
                .ctx
                .node_types
                .get(&clause.target_ty.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let sub_target = {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                subst.substitute(original_target)
            };

            for bound_ast in &clause.bounds {
                let original_bound = self
                    .ctx
                    .node_types
                    .get(&bound_ast.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let sub_bound = {
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                    subst.substitute(original_bound)
                };

                if sub_target != TypeId::ERROR
                    && sub_bound != TypeId::ERROR
                    && !self.check_trait_impl(sub_target, sub_bound)
                {
                    let req_str = self.ctx.ty_to_string(sub_bound);
                    let act_str = self.ctx.ty_to_string(sub_target);
                    self.ctx
                        .struct_error(span, "type does not satisfy trait bounds")
                        .with_hint(format!("required bound: `{}: {}`", act_str, req_str))
                        .emit();
                }
            }
        }
    }

    pub(crate) fn check_closure(
        &mut self,
        node_id: kernc_utils::NodeId,
        captures: &[ast::CapturePattern],
        params: &[ast::FuncParam],
        ast_ret_ty: &ast::TypeNode,
        body: &ast::Expr,
        span: Span,
    ) -> TypeId {
        // Infer all captured expressions first.
        let mut state_fields = Vec::new();
        let mut capture_env = Vec::new();

        for cap in captures {
            let cap_ty = self.check_expr(&cap.value, None);
            state_fields.push(cap_ty);
            capture_env.push((cap.name, cap_ty, cap.name_span));
        }

        let current_scope = match self.ctx.scopes.current_scope_id() {
            Some(id) => id,
            None => {
                self.ctx.emit_ice(
                    span,
                    "Compiler Bug: Closure evaluated outside of any active scope",
                );
                crate::scope::ScopeId(0)
            }
        };

        // Resolve parameter and return types in the parent scope so aliases remain visible.
        let (param_tys, expected_ret) = {
            let mut param_tys = Vec::new();
            let mut type_resolver = TypeResolver::new(self.ctx);
            for param in params {
                let p_ty = type_resolver.resolve_type(&param.type_node, current_scope);
                param_tys.push(p_ty);
            }
            let expected_ret = type_resolver.resolve_type(ast_ret_ty, current_scope);
            (param_tys, expected_ret)
        };

        let closure_state_ty = self.ctx.type_registry.intern(TypeKind::AnonymousState {
            closure_node_id: node_id,
            captures: state_fields,
            params: param_tys.clone(),
            ret: expected_ret,
        });

        // Enter the closure's body scope.
        let _ = self.ctx.scopes.enter_scope();

        // Inject captures by value into the closure scope.
        for (name, ty, cap_span) in capture_env {
            let info = SymbolInfo {
                kind: SymbolKind::Var,
                node_id, // Reuse the closure expression ID for the synthetic capture binding.
                type_id: ty,
                def_id: None,
                span: cap_span,
                is_pub: false,
                is_mut: false,
            };
            if self.ctx.scopes.define(name, info.clone()).is_ok() {
                self.ctx.record_symbol_definition(
                    info.span,
                    SemanticSymbolKind::Variable,
                    info.is_mut,
                    info.is_pub,
                );
            }
        }

        // Inject explicit closure parameters.
        for (i, param) in params.iter().enumerate() {
            if self.ctx.resolve(param.pattern.name) == "_" {
                continue;
            }
            let param_node_id = self.ctx.next_node_id();
            let info = SymbolInfo {
                kind: SymbolKind::Var,
                node_id: param_node_id,
                type_id: param_tys[i],
                def_id: None,
                span: param.pattern.name_span,
                is_pub: false,
                is_mut: param.pattern.is_mut,
            };
            if self
                .ctx
                .scopes
                .define(param.pattern.name, info.clone())
                .is_ok()
            {
                self.ctx.record_symbol_definition(
                    info.span,
                    SemanticSymbolKind::Parameter,
                    info.is_mut,
                    info.is_pub,
                );
            }
        }

        // Check the closure body.
        let (actual_ret_ty, has_returned) = {
            let mut sub_checker = ExprChecker::new(self.ctx, Some(expected_ret));
            let ty = sub_checker.check_expr(body, Some(expected_ret));
            (ty, sub_checker.has_returned)
        };

        // Validate body-vs-signature compatibility.
        if actual_ret_ty != TypeId::ERROR
            && expected_ret != TypeId::ERROR
            && actual_ret_ty != TypeId::NEVER
        {
            let norm_actual = self.ctx.type_registry.normalize(actual_ret_ty);
            let norm_expected = self.ctx.type_registry.normalize(expected_ret);

            // A `void` body for a non-void signature usually means the tail expression is missing.
            let is_missing_tail = norm_actual == TypeId::VOID && norm_expected != TypeId::VOID;

            // Explicit `return` statements can still satisfy the contract.
            if is_missing_tail && has_returned {
                // Safe: at least one path returns explicitly.
            } else if norm_actual != norm_expected {
                let expected_str = self.ctx.ty_to_string(expected_ret);
                let actual_str = self.ctx.ty_to_string(actual_ret_ty);

                self.ctx.struct_error(
                    body.span,
                    format!("closure body evaluates to `{}`, but signature expects `{}`", actual_str, expected_str)
                )
                .with_hint("ensure the final expression or return statements match the explicit return type")
                .emit();
            }
        }

        // 9. Leave the closure scope and record the resulting type.
        self.ctx.scopes.exit_scope();
        self.ctx.node_types.insert(node_id, closure_state_ty);

        closure_state_ty
    }

    /// Validate the special `@asm(.{ ... })` input form.
    fn check_asm_call(&mut self, args: &[Expr], span: Span) -> TypeId {
        if args.len() != 1 {
            self.ctx
                .struct_error(span, "`@asm` expects exactly one anonymous struct argument")
                .with_hint("example: `@asm(.{ asm: \"nop\", volatile: true })`")
                .emit();
            return TypeId::ERROR;
        }

        let config_arg = &args[0];
        let fields = match &config_arg.kind {
            ExprKind::DataInit {
                literal: ast::DataLiteralKind::Struct(f),
                type_node: None,
            } => f,
            _ => {
                self.ctx
                    .struct_error(
                        config_arg.span,
                        "`@asm` argument must be an untyped anonymous struct `.{ ... }`",
                    )
                    .emit();
                // Continue checking nested expressions to reduce cascades, but mark the outer node as invalid.
                self.check_expr(config_arg, None);
                return TypeId::ERROR;
            }
        };

        let mut has_asm = false;

        for field in fields {
            let field_name = self.ctx.resolve(field.name).to_string();
            match field_name.as_str() {
                "asm" => {
                    has_asm = true;
                    match &field.value.kind {
                        ExprKind::String(_) => {
                            self.check_expr(&field.value, None);
                        }
                        ExprKind::DataInit {
                            literal: ast::DataLiteralKind::Array(elems),
                            ..
                        } => {
                            for e in elems {
                                if !matches!(e.kind, ExprKind::String(_)) {
                                    self.ctx
                                        .struct_error(
                                            e.span,
                                            "all elements in asm array must be string literals",
                                        )
                                        .emit();
                                }
                                self.check_expr(e, None);
                            }
                        }
                        _ => {
                            self.ctx.struct_error(field.value.span, "`asm` template must be a string literal or an array of strings").emit();
                        }
                    }
                }
                "outputs" | "inputs" => {
                    if let ExprKind::DataInit {
                        literal: ast::DataLiteralKind::Struct(regs),
                        ..
                    } = &field.value.kind
                    {
                        for reg_field in regs {
                            let val_ty = self.check_expr(&reg_field.value, None);
                            let val_ty_str = self.ctx.ty_to_string(val_ty);

                            if field_name == "outputs"
                                && val_ty != TypeId::ERROR
                                && !self.is_mut_pointer(val_ty)
                            {
                                self.ctx.struct_error(reg_field.value.span, "inline assembly outputs must be bound to mutable pointers (e.g., `status..&`)")
                                    .with_hint(format!("type found: {}", val_ty_str))
                                    .emit();
                            }
                        }
                    } else {
                        self.ctx.struct_error(field.value.span, format!("`{}` must be an anonymous struct mapping registers to variables", field_name)).emit();
                        self.check_expr(&field.value, None);
                    }
                }
                "clobbers" => {
                    if let ExprKind::DataInit {
                        literal: ast::DataLiteralKind::Array(clobbers),
                        ..
                    } = &field.value.kind
                    {
                        for c in clobbers {
                            if !matches!(c.kind, ExprKind::String(_)) {
                                self.ctx.struct_error(c.span, "clobbers must be a list of string literals (e.g., `.{ \"memory\", \"cc\" }`)").emit();
                            }
                            self.check_expr(c, None);
                        }
                    } else {
                        self.ctx
                            .struct_error(
                                field.value.span,
                                "`clobbers` must be a slice/array of strings",
                            )
                            .emit();
                        self.check_expr(&field.value, None);
                    }
                }
                "volatile" => {
                    let ty = self.check_expr(&field.value, Some(TypeId::BOOL));
                    self.check_coercion(&field.value, TypeId::BOOL, ty);
                }
                _ => {
                    self.ctx
                        .struct_error(
                            field.span,
                            format!("unknown field `{}` in `@asm` configuration", field_name),
                        )
                        .emit();
                    self.check_expr(&field.value, None);
                }
            }
        }

        if !has_asm {
            self.ctx
                .struct_error(
                    span,
                    "`@asm` configuration is missing the required `asm` template string",
                )
                .emit();
        }

        // Bind `config_arg` as `void` so the AST cache stays complete.
        self.ctx.node_types.insert(config_arg.id, TypeId::VOID);

        // Inline assembly returns through output pointers instead of a direct value.
        TypeId::VOID
    }

    /// Return whether an inline-assembly output binding is a mutable pointer.
    fn is_mut_pointer(&mut self, ty: TypeId) -> bool {
        let norm = self.resolve_tv(ty);
        match self.ctx.type_registry.get(norm).clone() {
            TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => is_mut,
            _ => false,
        }
    }
}
