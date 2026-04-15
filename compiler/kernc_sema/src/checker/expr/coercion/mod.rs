use super::ExprChecker;
use crate::checker::Substituter;
use crate::def::{Def, DefId};
use crate::passes::TypeResolver;
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{Expr, ExprKind, UnaryOperator};
use kernc_utils::{DiagnosticCode, FastHashMap, FastHashSet, Span, SymbolId};
use std::collections::HashMap;
use std::hash::BuildHasher;

mod closure;
mod infer;
mod pointer;
mod trait_impl;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    /// Check whether an expression can be implicitly coerced to the target type.
    pub(crate) fn check_coercion(&mut self, expr: &Expr, expected: TypeId, actual: TypeId) -> bool {
        let exp = self.resolve_tv(expected);
        let act = self.resolve_tv(actual);

        if exp == act || exp == TypeId::ERROR || act == TypeId::ERROR {
            return true;
        }
        if act == TypeId::NEVER {
            return true;
        }

        let exp_kind = self.ctx.type_registry.get(exp).clone();
        let act_kind = self.ctx.type_registry.get(act).clone();

        // 1. Try plain unification first.
        if self.check_type_var(exp, act, &exp_kind, &act_kind) {
            return true;
        }

        // 2. Allow named-to-anonymous aggregate decay under value semantics.
        if self.is_anonymous_aggregate_equivalent(exp, act) {
            return true;
        }

        // 3. Handle pointer decay and trait-object packing.
        if self.check_pointer_coercions(expr, exp, act, &exp_kind, &act_kind) {
            return true;
        }

        // 4. Handle volatile-pointer decay and trait-object packing.
        if self.check_volatile_coercions(expr, exp, &exp_kind, &act_kind) {
            return true;
        }

        // 5. Handle slice coercions and array decay.
        if self.check_slice_and_array_decay(expr, &exp_kind, &act_kind) {
            return true;
        }

        // 6. Handle closure-related decay and boundary natural conversions.
        if self.check_closure_coercions(expr, &exp_kind, &act_kind) {
            return true;
        }

        // If no rule matched, emit the final mismatch diagnostic.
        self.emit_mismatch_error(expr.span, expected, actual);
        false
    }

    fn check_type_var(
        &mut self,
        exp: TypeId,
        act: TypeId,
        exp_kind: &TypeKind,
        act_kind: &TypeKind,
    ) -> bool {
        if let TypeKind::TypeVar(vid) = act_kind {
            self.bind_type_var(*vid, exp);
            return true;
        }
        if let TypeKind::TypeVar(vid) = exp_kind {
            self.bind_type_var(*vid, act);
            return true;
        }
        false
    }

    /// Format and emit a user-facing type mismatch diagnostic.
    pub fn emit_mismatch_error(&mut self, span: Span, expected: TypeId, actual: TypeId) {
        let exp_str = self.ctx.ty_to_string(expected);
        let act_str = self.ctx.ty_to_string(actual);

        self.ctx
            .struct_error(span, "mismatched types")
            .with_hint(format!("expected `{}`", exp_str))
            .with_hint(format!("   found `{}`", act_str))
            .emit();
    }
}
