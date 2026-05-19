//! Atomic intrinsic validation.
//!
//! Atomic orderings are compile-time enum/integer values. This file evaluates
//! and validates those order arguments, keeping target-specific atomic type
//! checks in the shared intrinsic dispatcher.

use super::*;
use crate::def::Def;
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

        if self.atomic_order_arg_depends_on_unbound_const_param(arg) {
            return None;
        }

        let mut evaluator = ConstEvaluator::new(self.ctx);
        let order = match evaluator.eval_inner(arg, 0) {
            Ok(crate::checker::ConstValue::Int(value)) => value,
            Ok(crate::checker::ConstValue::Enum { tag, payload: None })
                if self.expr_is_extern_enum(arg) =>
            {
                tag
            }
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

    fn expr_is_extern_enum(&mut self, expr: &Expr) -> bool {
        let ty = self.ctx.node_type_or_error(expr.id);
        let norm = self.resolve_tv(ty);
        let TypeKind::Enum(def_id, _) = self.ctx.type_registry.get(norm).clone() else {
            return false;
        };
        matches!(
            self.ctx.defs.get(def_id.0 as usize),
            Some(Def::Enum(enum_def)) if enum_def.is_extern
        )
    }

    fn atomic_order_arg_depends_on_unbound_const_param(&mut self, arg: &Expr) -> bool {
        match &arg.kind {
            ExprKind::Identifier(name) => self
                .ctx
                .scopes
                .resolve_value_symbol(*name)
                .is_some_and(|info| info.kind == SymbolKind::ConstParam),
            ExprKind::Binary { lhs, rhs, .. } | ExprKind::Assign { lhs, rhs, .. } => {
                self.atomic_order_arg_depends_on_unbound_const_param(lhs)
                    || self.atomic_order_arg_depends_on_unbound_const_param(rhs)
            }
            ExprKind::Range { start, end, .. } => {
                start
                    .as_deref()
                    .is_some_and(|expr| self.atomic_order_arg_depends_on_unbound_const_param(expr))
                    || end.as_deref().is_some_and(|expr| {
                        self.atomic_order_arg_depends_on_unbound_const_param(expr)
                    })
            }
            ExprKind::Unary { operand, .. }
            | ExprKind::Grouped { expr: operand }
            | ExprKind::FieldAccess { lhs: operand, .. }
            | ExprKind::As { lhs: operand, .. }
            | ExprKind::Propagate { operand } => {
                self.atomic_order_arg_depends_on_unbound_const_param(operand)
            }
            ExprKind::GenericInstantiation { target, args } => {
                self.atomic_order_arg_depends_on_unbound_const_param(target)
                    || args.iter().any(|arg| match arg {
                        kernc_ast::GenericArg::ConstExpr(expr) => {
                            self.atomic_order_arg_depends_on_unbound_const_param(expr)
                        }
                        kernc_ast::GenericArg::Type(_)
                        | kernc_ast::GenericArg::AssocBinding { .. } => false,
                    })
            }
            ExprKind::IndexAccess { lhs, index, .. } => {
                self.atomic_order_arg_depends_on_unbound_const_param(lhs)
                    || self.atomic_order_arg_depends_on_unbound_const_param(index)
            }
            ExprKind::Call { callee, args } => {
                self.atomic_order_arg_depends_on_unbound_const_param(callee)
                    || args
                        .iter()
                        .any(|arg| self.atomic_order_arg_depends_on_unbound_const_param(arg))
            }
            ExprKind::DataInit { literal, .. } => {
                self.atomic_order_data_literal_depends_on_unbound_const_param(literal)
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.atomic_order_arg_depends_on_unbound_const_param(cond)
                    || self.atomic_order_arg_depends_on_unbound_const_param(then_branch)
                    || else_branch.as_deref().is_some_and(|expr| {
                        self.atomic_order_arg_depends_on_unbound_const_param(expr)
                    })
            }
            ExprKind::Match { target, arms } => {
                self.atomic_order_arg_depends_on_unbound_const_param(target)
                    || arms.iter().any(|arm| {
                        self.atomic_order_arg_depends_on_unbound_const_param(&arm.body)
                            || arm.patterns.iter().any(|pattern| match &pattern.kind {
                                kernc_ast::MatchPatternKind::Value(expr) => {
                                    self.atomic_order_arg_depends_on_unbound_const_param(expr)
                                }
                                kernc_ast::MatchPatternKind::Pattern(_) => false,
                            })
                    })
            }
            ExprKind::Block { stmts, result } => {
                stmts.iter().any(|stmt| match &stmt.kind {
                    kernc_ast::StmtKind::ExprStmt(expr) | kernc_ast::StmtKind::ExprValue(expr) => {
                        self.atomic_order_arg_depends_on_unbound_const_param(expr)
                    }
                    kernc_ast::StmtKind::Use(_) => false,
                }) || result
                    .as_deref()
                    .is_some_and(|expr| self.atomic_order_arg_depends_on_unbound_const_param(expr))
            }
            ExprKind::While { cond, body } => {
                self.atomic_order_arg_depends_on_unbound_const_param(cond)
                    || self.atomic_order_arg_depends_on_unbound_const_param(body)
            }
            ExprKind::SliceOp {
                lhs, start, end, ..
            } => {
                self.atomic_order_arg_depends_on_unbound_const_param(lhs)
                    || start.as_deref().is_some_and(|expr| {
                        self.atomic_order_arg_depends_on_unbound_const_param(expr)
                    })
                    || end.as_deref().is_some_and(|expr| {
                        self.atomic_order_arg_depends_on_unbound_const_param(expr)
                    })
            }
            ExprKind::Defer { expr }
            | ExprKind::Return(Some(expr))
            | ExprKind::Let { init: expr, .. } => {
                self.atomic_order_arg_depends_on_unbound_const_param(expr)
            }
            ExprKind::Static { init, .. } => init
                .as_deref()
                .is_some_and(|expr| self.atomic_order_arg_depends_on_unbound_const_param(expr)),
            ExprKind::Closure { captures, body, .. } => {
                captures.iter().any(|capture| {
                    self.atomic_order_arg_depends_on_unbound_const_param(&capture.value)
                }) || self.atomic_order_arg_depends_on_unbound_const_param(body)
            }
            ExprKind::Error
            | ExprKind::Integer { .. }
            | ExprKind::Float { .. }
            | ExprKind::Bool(_)
            | ExprKind::Char(_)
            | ExprKind::ByteChar(_)
            | ExprKind::String(_)
            | ExprKind::AnchoredPath { .. }
            | ExprKind::TypeNode(_)
            | ExprKind::EnumLiteral { .. }
            | ExprKind::Break
            | ExprKind::Continue
            | ExprKind::Return(None)
            | ExprKind::Undef
            | ExprKind::Infer
            | ExprKind::SelfValue => false,
        }
    }

    fn atomic_order_data_literal_depends_on_unbound_const_param(
        &mut self,
        literal: &kernc_ast::DataLiteralKind,
    ) -> bool {
        match literal {
            kernc_ast::DataLiteralKind::Struct(fields) => fields
                .iter()
                .any(|field| self.atomic_order_arg_depends_on_unbound_const_param(&field.value)),
            kernc_ast::DataLiteralKind::Array(values) => values
                .iter()
                .any(|value| self.atomic_order_arg_depends_on_unbound_const_param(value)),
            kernc_ast::DataLiteralKind::Repeat { value, count } => {
                self.atomic_order_arg_depends_on_unbound_const_param(value)
                    || self.atomic_order_arg_depends_on_unbound_const_param(count)
            }
            kernc_ast::DataLiteralKind::Scalar(value) => {
                self.atomic_order_arg_depends_on_unbound_const_param(value)
            }
        }
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
                "expected an integer type or a normal raw pointer (`&T` / `&mut T`)"
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
