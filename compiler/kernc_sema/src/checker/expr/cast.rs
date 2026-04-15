use super::ExprChecker;
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{DataLiteralKind, Expr, ExprKind, UnaryOperator};
use kernc_utils::Span;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub(crate) fn check_as_expr(&mut self, lhs: &Expr, target_ty: TypeId) -> TypeId {
        let lhs_ty = self.check_expr(lhs, None);
        if self.check_non_null_pointer_zero_cast(lhs, target_ty) {
            return target_ty;
        }
        self.check_cast(lhs.span, lhs_ty, target_ty);
        target_ty
    }

    fn check_non_null_pointer_zero_cast(&mut self, lhs: &Expr, target_ty: TypeId) -> bool {
        let target_norm = self.resolve_tv(target_ty);
        if !self.is_object_pointer_type(target_norm) {
            return false;
        }

        if self.is_explicit_zero_value(lhs) {
            self.ctx
                .struct_error(
                    lhs.span,
                    "non-null raw pointers cannot be created from the constant address `0`",
                )
                .with_hint("use `?*T.None` for a nullable pointer, or cast a real non-zero address")
                .emit();
            return true;
        }

        false
    }

    fn is_explicit_zero_value(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Integer(value) => *value == 0,
            ExprKind::As { lhs, .. } => self.is_explicit_zero_value(lhs),
            ExprKind::Unary {
                op: UnaryOperator::Negate,
                operand,
            } => self.is_explicit_zero_value(operand),
            ExprKind::DataInit {
                literal: DataLiteralKind::Scalar(inner),
                ..
            } => self.is_explicit_zero_value(inner),
            _ => false,
        }
    }

    fn check_cast(&mut self, span: Span, from: TypeId, to: TypeId) {
        if from == to || from == TypeId::ERROR || to == TypeId::ERROR {
            return;
        }

        let f_norm = self.resolve_tv(from);
        let t_norm = self.resolve_tv(to);

        let is_f_int = self.ctx.type_registry.is_integer(f_norm);
        let is_t_int = self.ctx.type_registry.is_integer(t_norm);
        let is_f_float = self.ctx.type_registry.is_float(f_norm);
        let is_t_float = self.ctx.type_registry.is_float(t_norm);

        let is_f_ptr = matches!(
            self.ctx.type_registry.get(f_norm),
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
        );
        let is_t_ptr = matches!(
            self.ctx.type_registry.get(t_norm),
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
        );
        let target_optional_object_ptr = self.optional_object_pointer_payload(t_norm);

        // 0. `?*T` is the nullable object-pointer airlock; `*T` must stay explicit.
        if self.is_address_pointer_type(f_norm) && self.is_object_pointer_type(t_norm) {
            self.ctx
                .struct_error(
                    span,
                    "cannot cast an address pointer directly to a non-null object pointer",
                )
                .with_hint("cast to `?*T` first, then handle `.None` / `.{ Some: ptr }` explicitly")
                .emit();
            return;
        }

        if self.is_address_pointer_type(f_norm) && target_optional_object_ptr.is_some() {
            return;
        }

        // 1. Allow pointer reinterpretation such as `*i32 as *u8`.
        if is_f_ptr && is_t_ptr {
            let t_inner = self.ctx.type_registry.get_elem_type(t_norm).unwrap();
            let t_inner_id = self.resolve_tv(t_inner);

            let t_is_fat = matches!(
                self.ctx.type_registry.get(t_inner_id),
                TypeKind::TraitObject(..) | TypeKind::Slice { .. }
            );
            if t_is_fat {
                self.ctx
                    .struct_error(
                        span,
                        "cannot cast a thin pointer to a fat pointer using `as`",
                    )
                    .with_hint("use explicit constructor syntax: `TargetType.{ pointer }`")
                    .emit();
            }
            return;
        }

        // 2. Allow pointer -> integer casts and integer -> address-pointer casts.
        let is_f_ptr_int = f_norm == TypeId::USIZE || f_norm == TypeId::ISIZE;
        let is_t_ptr_int = t_norm == TypeId::USIZE || t_norm == TypeId::ISIZE;
        if is_f_ptr && is_t_ptr_int {
            return;
        }

        if is_f_ptr_int && self.is_address_pointer_type(t_norm) {
            return;
        }

        if is_f_ptr_int && self.is_object_pointer_type(t_norm) {
            let to_str = self.ctx.ty_to_string(to);
            self.ctx
                .struct_error(
                    span,
                    "integer addresses cannot be cast directly to non-null object pointers",
                )
                .with_hint("cast to `?*T` first, then handle `.None` / `.{ Some: ptr }` explicitly")
                .with_hint(format!("attempted to cast into `{}`", to_str))
                .emit();
            return;
        }

        if is_f_ptr_int && target_optional_object_ptr.is_some() {
            return;
        }

        // 3. Allow all numeric casts, including int/float cross-casts and `bool -> int`.
        let is_f_numeric = is_f_int || is_f_float || f_norm == TypeId::BOOL;
        let is_t_numeric = is_t_int || is_t_float;
        if is_f_numeric && is_t_numeric {
            return;
        }

        // 4. Reject everything else with a proper diagnostic.
        let from_str = self.ctx.ty_to_string(from);
        let to_str = self.ctx.ty_to_string(to);
        self.ctx
            .struct_error(span, "invalid `as` cast")
            .with_hint("`as` supports numeric conversions, pointer casting, and pointer-integer conversions")
            .with_hint("for trait objects or slices, use explicit constructor syntax")
            .with_hint(format!("attempted to cast from `{}` to `{}`", from_str, to_str))
            .emit();
    }

    fn is_object_pointer_type(&self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        matches!(self.ctx.type_registry.get(norm), TypeKind::Pointer { .. })
    }

    fn is_address_pointer_type(&self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        matches!(
            self.ctx.type_registry.get(norm),
            TypeKind::VolatilePtr { .. }
        )
    }

    fn optional_object_pointer_payload(&self, ty: TypeId) -> Option<TypeId> {
        let norm = self.ctx.type_registry.normalize(ty);
        let TypeKind::AnonymousEnum(enum_def) = self.ctx.type_registry.get(norm) else {
            return None;
        };
        let payload = enum_def.builtin_optional_payload()?;
        if self.is_object_pointer_type(payload) {
            Some(payload)
        } else {
            None
        }
    }
}
