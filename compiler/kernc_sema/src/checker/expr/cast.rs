//! Explicit `as` cast checking.
//!
//! Casts are intentionally narrower than coercions: this file checks the
//! operand expectation for numeric casts, validates permitted pointer/integer
//! and enum conversions, and enriches trait-object pointer casts when a proof is
//! available.

use super::ExprChecker;
use crate::ty::{TypeId, TypeKind};
use kernc_ast::Expr;
use kernc_utils::Span;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub(crate) fn check_as_expr(&mut self, lhs: &Expr, target_ty: TypeId) -> TypeId {
        let lhs_expected_ty = self.cast_operand_expected_type(target_ty);
        let lhs_ty = self.check_expr(lhs, lhs_expected_ty);
        self.check_cast(lhs.span, lhs_ty, target_ty);
        self.enrich_trait_object_pointer_cast_target(lhs_ty, target_ty)
    }

    fn cast_operand_expected_type(&mut self, target_ty: TypeId) -> Option<TypeId> {
        let norm = self.resolve_tv(target_ty);
        if self.ctx.type_registry.is_integer(norm) || self.ctx.type_registry.is_float(norm) {
            Some(norm)
        } else {
            None
        }
    }

    fn enrich_trait_object_pointer_cast_target(&mut self, from: TypeId, to: TypeId) -> TypeId {
        let to_norm = self.resolve_tv(to);
        let (is_mut, elem, is_volatile) = match self.ctx.type_registry.get(to_norm).clone() {
            TypeKind::Pointer { is_mut, elem } => (is_mut, elem, false),
            TypeKind::VolatilePtr { is_mut, elem } => (is_mut, elem, true),
            _ => return to,
        };
        let elem_norm = self.resolve_tv(elem);
        if !matches!(
            self.ctx.type_registry.get(elem_norm),
            TypeKind::TraitObject(..)
        ) {
            return to;
        }

        let from_norm = self.resolve_tv(from);
        let proof_ty = match self.ctx.type_registry.get(from_norm).clone() {
            TypeKind::Pointer {
                is_mut: from_mut,
                elem: from_elem,
            } => {
                if !is_mut && from_mut {
                    self.ctx.type_registry.intern(TypeKind::Pointer {
                        is_mut: false,
                        elem: from_elem,
                    })
                } else {
                    from
                }
            }
            TypeKind::VolatilePtr {
                is_mut: from_mut,
                elem: from_elem,
            } => {
                if !is_mut && from_mut {
                    self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                        is_mut: false,
                        elem: from_elem,
                    })
                } else {
                    from
                }
            }
            _ => return to,
        };
        let enriched_elem =
            crate::query::enrich_trait_object_assoc_bindings(self.ctx, proof_ty, elem_norm);
        if enriched_elem == elem_norm {
            return to;
        }
        if is_volatile {
            self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                is_mut,
                elem: enriched_elem,
            })
        } else {
            self.ctx.type_registry.intern(TypeKind::Pointer {
                is_mut,
                elem: enriched_elem,
            })
        }
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

        // 1. Allow pointer reinterpretation and explicit fat-pointer packaging
        // such as `value..& as &mut Trait` or `state..& as &Fn(...) T`.
        if is_f_ptr && is_t_ptr {
            if self.is_slice_pointer_value_type(t_norm) {
                self.ctx
                    .struct_error(span, "cannot cast a pointer to a slice using `as`")
                    .with_hint("slice length metadata must come from slice syntax such as `array.&[start...end]`")
                    .emit();
            } else if self.is_closure_fat_pointer_value_type(t_norm) {
                if self.fat_pointer_cast_mutability_mismatch(from, to) {
                    self.ctx
                        .struct_error(
                            span,
                            "cannot cast an immutable pointer to a mutable closure object",
                        )
                        .emit();
                    return;
                }
                if !self.can_cast_to_closure_fat_pointer(from, to) {
                    self.ctx
                        .struct_error(span, "cannot cast this pointer to a closure object")
                        .with_hint(
                            "the source must point to a compatible closure state or function",
                        )
                        .emit();
                }
            } else if self.is_trait_object_pointer_value_type(t_norm) {
                if self.fat_pointer_cast_mutability_mismatch(from, to) {
                    self.ctx
                        .struct_error(
                            span,
                            "cannot cast an immutable pointer to a mutable trait object",
                        )
                        .emit();
                    return;
                }
                if !self.can_cast_to_trait_object_pointer(from, to) {
                    self.ctx
                        .struct_error(span, "cannot cast this pointer to a trait object")
                        .with_hint("the source pointer type must implement the target trait")
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
                .with_hint("use a pointer source with `as`, or rely on an expected type context when metadata is known")
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
            .with_hint("for trait objects, cast a compatible pointer with `as`; for slices, use slice syntax")
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

    fn is_slice_pointer_value_type(&self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm) {
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let elem_norm = self.ctx.type_registry.normalize(*elem);
                matches!(
                    self.ctx.type_registry.get(elem_norm),
                    TypeKind::Slice { .. }
                )
            }
            _ => false,
        }
    }

    fn is_trait_object_pointer_value_type(&self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm) {
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let elem_norm = self.ctx.type_registry.normalize(*elem);
                matches!(
                    self.ctx.type_registry.get(elem_norm),
                    TypeKind::TraitObject(..)
                )
            }
            _ => false,
        }
    }

    fn is_closure_fat_pointer_value_type(&self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm) {
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let elem_norm = self.ctx.type_registry.normalize(*elem);
                matches!(
                    self.ctx.type_registry.get(elem_norm),
                    TypeKind::ClosureInterface { .. }
                )
            }
            _ => false,
        }
    }

    fn is_fat_pointer_value_type(&self, ty: TypeId) -> bool {
        self.is_slice_pointer_value_type(ty)
            || self.is_trait_object_pointer_value_type(ty)
            || self.is_closure_fat_pointer_value_type(ty)
    }

    fn can_cast_to_trait_object_pointer(&mut self, from: TypeId, to: TypeId) -> bool {
        let to_norm = self.resolve_tv(to);
        let (TypeKind::Pointer {
            is_mut: to_mut,
            elem: to_elem,
        }
        | TypeKind::VolatilePtr {
            is_mut: to_mut,
            elem: to_elem,
        }) = self.ctx.type_registry.get(to_norm).clone()
        else {
            return false;
        };

        let trait_norm = self.resolve_tv(to_elem);
        if !matches!(
            self.ctx.type_registry.get(trait_norm),
            TypeKind::TraitObject(..)
        ) {
            return false;
        }

        let from_norm = self.resolve_tv(from);
        let (TypeKind::Pointer {
            is_mut: from_mut,
            elem: from_elem,
        }
        | TypeKind::VolatilePtr {
            is_mut: from_mut,
            elem: from_elem,
        }) = self.ctx.type_registry.get(from_norm).clone()
        else {
            return false;
        };

        if to_mut && !from_mut {
            return false;
        }

        let from_elem_norm = self.resolve_tv(from_elem);
        if matches!(
            self.ctx.type_registry.get(from_elem_norm),
            TypeKind::TraitObject(..)
        ) {
            return self.is_trait_object_upcast(from_elem_norm, trait_norm);
        }

        let proof_ty = if !to_mut && from_mut {
            match self.ctx.type_registry.get(from_norm).clone() {
                TypeKind::Pointer { elem, .. } => {
                    self.ctx.type_registry.intern(TypeKind::Pointer {
                        is_mut: false,
                        elem,
                    })
                }
                TypeKind::VolatilePtr { elem, .. } => {
                    self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                        is_mut: false,
                        elem,
                    })
                }
                _ => from,
            }
        } else {
            from
        };

        self.check_trait_impl(proof_ty, trait_norm)
    }

    fn fat_pointer_cast_mutability_mismatch(&self, from: TypeId, to: TypeId) -> bool {
        let from_norm = self.ctx.type_registry.normalize(from);
        let to_norm = self.ctx.type_registry.normalize(to);
        let source_mut = match self.ctx.type_registry.get(from_norm) {
            TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => *is_mut,
            _ => return false,
        };
        let target_mut = match self.ctx.type_registry.get(to_norm) {
            TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => *is_mut,
            _ => return false,
        };
        target_mut && !source_mut
    }

    fn can_cast_to_closure_fat_pointer(&mut self, from: TypeId, to: TypeId) -> bool {
        let to_norm = self.resolve_tv(to);
        let (TypeKind::Pointer {
            is_mut: to_mut,
            elem: to_elem,
        }
        | TypeKind::VolatilePtr {
            is_mut: to_mut,
            elem: to_elem,
        }) = self.ctx.type_registry.get(to_norm).clone()
        else {
            return false;
        };

        let closure_norm = self.resolve_tv(to_elem);
        let TypeKind::ClosureInterface { params, ret } =
            self.ctx.type_registry.get(closure_norm).clone()
        else {
            return false;
        };

        let from_norm = self.resolve_tv(from);
        let (TypeKind::Pointer {
            is_mut: from_mut,
            elem: from_elem,
        }
        | TypeKind::VolatilePtr {
            is_mut: from_mut,
            elem: from_elem,
        }) = self.ctx.type_registry.get(from_norm).clone()
        else {
            return false;
        };

        if to_mut && !from_mut {
            return false;
        }

        let from_elem_norm = self.resolve_tv(from_elem);
        match self.ctx.type_registry.get(from_elem_norm).clone() {
            TypeKind::AnonymousState {
                params: state_params,
                ret: state_ret,
                ..
            } => self.signatures_compatible(&params, ret, &state_params, state_ret),
            TypeKind::Function {
                params: fn_params,
                ret: fn_ret,
                is_variadic: false,
            } => self.signatures_compatible(&params, ret, &fn_params, fn_ret),
            TypeKind::FnDef(def_id, args) => self
                .instantiate_fn_def_signature(def_id, &args, Span::default())
                .is_some_and(|(fn_params, fn_ret)| {
                    self.signatures_compatible(&params, ret, &fn_params, fn_ret)
                }),
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
