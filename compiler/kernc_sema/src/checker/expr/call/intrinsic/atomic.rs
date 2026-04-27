use super::*;
use crate::scope::SymbolKind;
use kernc_ast::ExprKind;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub(super) fn eval_atomic_order_arg(
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

        if self.atomic_order_arg_is_unbound_const_param(arg) {
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

        self.ctx.set_atomic_ordering(arg.id, ordering);
        Some(ordering)
    }

    fn atomic_order_arg_is_unbound_const_param(&mut self, arg: &Expr) -> bool {
        let ExprKind::Identifier(name) = arg.kind else {
            return false;
        };
        let Some(info) = self.ctx.scopes.resolve(name) else {
            return false;
        };
        info.kind == SymbolKind::ConstParam
    }

    pub(super) fn check_atomic_target_type(
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
        if matches!(self.ctx.type_registry.get(norm), TypeKind::Param(_)) {
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

    pub(crate) fn check_atomic_intrinsic_call(
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
}
