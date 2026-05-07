use super::ExprChecker;
use crate::ty::{TypeId, TypeKind};
use kernc_ast::Expr;
use kernc_utils::Span;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub(crate) fn check_as_expr(&mut self, lhs: &Expr, target_ty: TypeId) -> TypeId {
        let lhs_ty = self.check_expr(lhs, None);
        self.check_cast(lhs.span, lhs_ty, target_ty);
        target_ty
    }

    fn check_cast(&mut self, span: Span, from: TypeId, to: TypeId) {
        if from == to || from == TypeId::ERROR || to == TypeId::ERROR {
            return;
        }

        let f_norm = self.resolve_tv(from);
        let t_norm = self.resolve_tv(to);

        let is_f_int = self.ctx.type_registry.is_integer(f_norm)
            || self
                .type_numeric_candidates(f_norm)
                .is_some_and(Self::numeric_candidates_have_integers);
        let is_t_int = self.ctx.type_registry.is_integer(t_norm);
        let is_f_float = self.ctx.type_registry.is_float(f_norm)
            || self
                .type_numeric_candidates(f_norm)
                .is_some_and(Self::numeric_candidates_have_floats);
        let is_t_float = self.ctx.type_registry.is_float(t_norm);

        let is_f_fn = matches!(
            self.ctx.type_registry.get(f_norm),
            TypeKind::Function { .. } | TypeKind::FnDef(..)
        );
        let is_t_fn = matches!(
            self.ctx.type_registry.get(t_norm),
            TypeKind::Function { .. } | TypeKind::FnDef(..)
        );
        let is_f_ptr = is_f_fn
            || matches!(
            self.ctx.type_registry.get(f_norm),
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
        );
        let is_t_ptr = is_t_fn
            || matches!(
            self.ctx.type_registry.get(t_norm),
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
        );

        // 1. Allow thin pointer reinterpretation such as `*i32 as *u8`
        // and `fn(...) T as *void`.
        if is_f_ptr && is_t_ptr {
            if !is_t_fn {
                let Some(t_inner) = self.ctx.type_registry.get_elem_type(t_norm) else {
                    self.ctx.emit_ice(
                        span,
                        "Kern ICE (Typeck): pointer cast target is missing its element type.",
                    );
                    return;
                };
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
            }
            return;
        }

        // 2. Allow pointer -> integer casts and integer -> address-pointer casts.
        let f_norm = self.constrain_pointer_cast_integer(from);
        let is_f_ptr_int = f_norm == TypeId::USIZE
            || f_norm == TypeId::ISIZE
            || self
                .type_numeric_candidates(f_norm)
                .is_some_and(|candidates| {
                    candidates != 0 && (candidates & !Self::NUMERIC_CAND_POINTER_OFFSETS) == 0
                });
        let is_t_ptr_int = t_norm == TypeId::USIZE || t_norm == TypeId::ISIZE;
        if is_f_ptr && is_t_ptr_int {
            return;
        }

        if is_f_ptr_int && self.is_fat_pointer_value_type(t_norm) {
            self.ctx
                .struct_error(span, "cannot cast an integer to a fat pointer using `as`")
                .with_hint("trait objects, slices, and closure objects carry metadata")
                .with_hint("construct a concrete fat pointer with explicit constructor syntax")
                .emit();
            return;
        }

        if is_f_ptr_int && self.is_address_pointer_type(t_norm) {
            return;
        }

        if is_f_ptr_int && self.is_object_pointer_type(t_norm) {
            return;
        }

        if is_f_ptr_int && self.is_function_pointer_type(t_norm) {
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

    fn is_function_pointer_type(&self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        matches!(
            self.ctx.type_registry.get(norm),
            TypeKind::Function { .. } | TypeKind::FnDef(..)
        )
    }

    fn is_fat_pointer_value_type(&self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm) {
            TypeKind::Slice { .. } | TypeKind::TraitObject(..) => true,
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let elem_norm = self.ctx.type_registry.normalize(*elem);
                matches!(
                    self.ctx.type_registry.get(elem_norm),
                    TypeKind::TraitObject(..) | TypeKind::ClosureInterface { .. }
                )
            }
            _ => false,
        }
    }

    fn constrain_pointer_cast_integer(&mut self, ty: TypeId) -> TypeId {
        let resolved = self.resolve_tv(ty);
        let TypeKind::TypeVar(vid) = self.ctx.type_registry.get(resolved).clone() else {
            return resolved;
        };

        if self.numeric_inference_kind(vid).is_none() {
            return resolved;
        }

        if self.constrain_numeric_type_var(vid, Self::NUMERIC_CAND_POINTER_OFFSETS) {
            self.resolve_tv(ty)
        } else {
            resolved
        }
    }
}
