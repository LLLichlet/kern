use super::*;
use crate::ty::GenericArg;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    fn trait_object_satisfies_required(
        &mut self,
        available_trait_ty: TypeId,
        required_trait_ty: TypeId,
    ) -> bool {
        let available_norm = self.resolve_tv(available_trait_ty);
        let required_norm = self.resolve_tv(required_trait_ty);

        let (
            TypeKind::TraitObject(available_def_id, available_args, available_assoc_bindings),
            TypeKind::TraitObject(required_def_id, required_args, required_assoc_bindings),
        ) = (
            self.ctx.type_registry.get(available_norm).clone(),
            self.ctx.type_registry.get(required_norm).clone(),
        )
        else {
            return false;
        };

        if available_def_id != required_def_id || available_args != required_args {
            return false;
        }

        if required_assoc_bindings.is_empty() {
            return true;
        }

        // Upcasts are allowed to forget extra inherited equalities carried by
        // the richer source object, but they may not fabricate or rewrite any
        // binding explicitly requested by the target type.
        let available_assoc_bindings = available_assoc_bindings
            .into_iter()
            .collect::<FastHashMap<_, _>>();
        required_assoc_bindings
            .into_iter()
            .all(|(assoc_def_id, required_assoc_ty)| {
                available_assoc_bindings
                    .get(&assoc_def_id)
                    .is_some_and(|available_assoc_ty| {
                        self.resolve_tv(*available_assoc_ty) == self.resolve_tv(required_assoc_ty)
                    })
            })
    }

    pub(super) fn check_pointer_coercions(
        &mut self,
        expr: &Expr,
        _exp: TypeId,
        act: TypeId,
        exp_kind: &TypeKind,
        act_kind: &TypeKind,
    ) -> bool {
        if let TypeKind::Pointer {
            is_mut: e_mut,
            elem: e_inner,
        } = exp_kind
        {
            let e_norm = self.resolve_tv(*e_inner);
            let actual_fat_pointer_value = match act_kind {
                TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                    let elem_norm = self.resolve_tv(*elem);
                    matches!(
                        self.ctx.type_registry.get(elem_norm),
                        TypeKind::TraitObject(..) | TypeKind::ClosureInterface { .. }
                    )
                }
                _ => false,
            };

            // Fat pointers already encode whether the erased receiver/storage was borrowed as
            // mutable or immutable. `let mut obj: *Trait` only makes the two-word handle
            // rebindable; it must not silently upgrade the underlying borrow to `*mut Trait`.
            if !actual_fat_pointer_value
                && self.check_pointer_to_pointer_coercion(*e_mut, e_norm, act, act_kind)
            {
                return true;
            }

            if actual_fat_pointer_value
                && let TypeKind::Pointer { is_mut, elem } | TypeKind::VolatilePtr { is_mut, elem } =
                    act_kind
            {
                let actual_elem_norm = self.resolve_tv(*elem);
                if Self::pointer_mutability_allows(*e_mut, *is_mut)
                    && matches!(
                        self.ctx.type_registry.get(actual_elem_norm),
                        TypeKind::TraitObject(..)
                    )
                    && self.is_trait_object_upcast(actual_elem_norm, e_norm)
                {
                    return true;
                }
            }

            if (actual_fat_pointer_value
                || !matches!(
                    act_kind,
                    TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
                ))
                && self.check_value_to_trait_object_pointer(expr, *e_mut, e_norm, act)
            {
                return true;
            }
        }
        false
    }

    fn pointer_mutability_allows(expected_mut: bool, actual_mut: bool) -> bool {
        !expected_mut || actual_mut
    }

    fn check_pointer_to_pointer_coercion(
        &mut self,
        expected_mut: bool,
        expected_elem: TypeId,
        actual_ty: TypeId,
        act_kind: &TypeKind,
    ) -> bool {
        let TypeKind::Pointer {
            is_mut: actual_mut,
            elem: actual_elem,
        } = act_kind
        else {
            return false;
        };

        if !Self::pointer_mutability_allows(expected_mut, *actual_mut) {
            return false;
        }

        let actual_elem_norm = self.resolve_tv(*actual_elem);
        if expected_elem == actual_elem_norm
            || self.ctx.type_registry.is_void(expected_elem)
            || self.is_anonymous_aggregate_equivalent(expected_elem, actual_elem_norm)
        {
            return true;
        }

        if matches!(
            (
                self.ctx.type_registry.get(expected_elem),
                self.ctx.type_registry.get(actual_elem_norm)
            ),
            (TypeKind::TraitObject(..), TypeKind::TraitObject(..))
        ) && self.is_trait_object_upcast(actual_elem_norm, expected_elem)
        {
            return true;
        }

        if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(expected_elem) {
            let trait_source_ty = if !expected_mut && *actual_mut {
                self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut: false,
                    elem: *actual_elem,
                })
            } else {
                actual_ty
            };

            if self.check_trait_impl(trait_source_ty, expected_elem) {
                return true;
            }
        }

        false
    }

    pub(crate) fn is_trait_object_upcast(
        &mut self,
        source_trait_ty: TypeId,
        target_trait_ty: TypeId,
    ) -> bool {
        let source_norm = self.resolve_tv(source_trait_ty);
        let target_norm = self.resolve_tv(target_trait_ty);

        let TypeKind::TraitObject(target_def_id, target_args, _) =
            self.ctx.type_registry.get(target_norm).clone()
        else {
            return false;
        };

        crate::query::trait_object_view_from_hierarchy(
            self.ctx,
            source_norm,
            target_def_id,
            &target_args,
        )
        .is_some_and(|candidate_view| {
            self.trait_object_satisfies_required(candidate_view, target_norm)
        })
    }

    fn check_value_to_trait_object_pointer(
        &mut self,
        expr: &Expr,
        expected_mut: bool,
        expected_elem: TypeId,
        actual_ty: TypeId,
    ) -> bool {
        if !matches!(
            self.ctx.type_registry.get(expected_elem),
            TypeKind::TraitObject(..)
        ) {
            return false;
        }

        if expected_mut && !self.can_take_mut_address_of(expr) {
            self.ctx
                .struct_error(
                    expr.span,
                    "cannot implicitly borrow an immutable value as a mutable trait object `*mut Trait`",
                )
                .with_code(DiagnosticCode::RequiresLetMut)
                .with_hint(
                    "consider declaring the variable as `let mut`, or pass a value expression that can be materialized into a mutable stack temporary",
                )
                .emit();
            return false;
        }

        if self.check_trait_impl(actual_ty, expected_elem) {
            return true;
        }

        let actual_norm = self.resolve_tv(actual_ty);
        if let TypeKind::Array { elem, .. } = self.ctx.type_registry.get(actual_norm).clone() {
            let slice_ty = self.ctx.type_registry.intern(TypeKind::Slice {
                elem,
                is_mut: false,
            });
            if self.check_trait_impl(slice_ty, expected_elem) {
                return true;
            }
        }

        let virtual_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: expected_mut,
            elem: actual_ty,
        });

        self.check_trait_impl(virtual_ptr_ty, expected_elem)
    }

    /// Core helper for checking named-to-anonymous aggregate decay.
    pub(super) fn is_anonymous_aggregate_equivalent(
        &mut self,
        exp_anon: TypeId,
        act_def: TypeId,
    ) -> bool {
        let exp_kind = self.ctx.type_registry.get(exp_anon).clone();
        let act_kind = self.ctx.type_registry.get(act_def).clone();

        if let TypeKind::Def(def_id, ref act_args) = act_kind {
            let act_def_clone = self.ctx.defs[def_id.0 as usize].clone();

            match (exp_kind.clone(), act_def_clone) {
                (TypeKind::AnonymousStruct(exp_is_extern, exp_fields), Def::Struct(act_s)) => {
                    if exp_is_extern != act_s.is_extern {
                        return false;
                    }
                    return self.compare_named_fields_to_anonymous(
                        &act_s.generics,
                        &act_s.fields,
                        act_args,
                        &exp_fields,
                        true,
                    );
                }
                (TypeKind::AnonymousUnion(exp_is_extern, exp_fields), Def::Union(act_u)) => {
                    if exp_is_extern != act_u.is_extern {
                        return false;
                    }
                    return self.compare_named_fields_to_anonymous(
                        &act_u.generics,
                        &act_u.fields,
                        act_args,
                        &exp_fields,
                        false,
                    );
                }
                _ => {}
            }
        }

        if let TypeKind::Enum(def_id, ref act_args) = act_kind {
            let act_def_clone = self.ctx.defs[def_id.0 as usize].clone();
            if let (TypeKind::AnonymousEnum(exp_enum), Def::Enum(act_enum)) =
                (exp_kind, act_def_clone)
            {
                let exp_backing = exp_enum.backing_ty.unwrap_or(TypeId::U32);
                let act_backing = act_enum.backing_type.as_ref().map_or(TypeId::U32, |bt| {
                    self.ctx
                        .facts
                        .node_types
                        .get(&bt.id)
                        .copied()
                        .unwrap_or(TypeId::U32)
                });

                if self.resolve_tv(exp_backing) != self.resolve_tv(act_backing) {
                    return false;
                }

                if exp_enum.variants.len() != act_enum.variants.len() {
                    return false;
                }

                let mut subst_map = FastHashMap::default();
                for (i, param) in act_enum.generics.iter().enumerate() {
                    subst_map.insert(param.name, act_args[i]);
                }

                let mut current_val: i128 = 0;
                for (exp_variant, act_variant) in
                    exp_enum.variants.iter().zip(act_enum.variants.iter())
                {
                    if let Some(v_expr) = &act_variant.value {
                        let mut ce = crate::checker::ConstEvaluator::new(self.ctx);
                        if let Ok(val) = ce.eval_math(v_expr) {
                            current_val = val;
                        }
                    }

                    if exp_variant.name != act_variant.name {
                        return false;
                    }

                    let act_payload = act_variant.payload_type.as_ref().map(|payload_ast| {
                        let raw_ty = self
                            .ctx
                            .facts
                            .node_types
                            .get(&payload_ast.id)
                            .copied()
                            .unwrap_or(TypeId::ERROR);
                        let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
                        let substituted = subst.substitute(raw_ty);
                        self.resolve_tv(substituted)
                    });

                    if exp_variant.payload_ty.map(|ty| self.resolve_tv(ty)) != act_payload {
                        return false;
                    }

                    let exp_value = exp_variant.explicit_value.unwrap_or(current_val);
                    if exp_value != current_val {
                        return false;
                    }

                    current_val += 1;
                }

                return true;
            }
        }
        false
    }

    pub(super) fn is_anonymous_aggregate_coercible(
        &mut self,
        expected: TypeId,
        actual: TypeId,
    ) -> bool {
        let exp_kind = self.ctx.type_registry.get(expected).clone();
        let act_kind = self.ctx.type_registry.get(actual).clone();
        let (
            TypeKind::AnonymousStruct(exp_is_extern, mut exp_fields),
            TypeKind::AnonymousStruct(act_is_extern, mut act_fields),
        ) = (exp_kind, act_kind)
        else {
            return false;
        };

        if exp_is_extern != act_is_extern || exp_fields.len() != act_fields.len() {
            return false;
        }

        exp_fields.sort_by_key(|field| field.name);
        act_fields.sort_by_key(|field| field.name);
        exp_fields
            .iter()
            .zip(act_fields.iter())
            .all(|(exp, act)| exp.name == act.name && self.type_is_field_coercible(exp.ty, act.ty))
    }

    fn type_is_field_coercible(&mut self, expected: TypeId, actual: TypeId) -> bool {
        let exp = self.resolve_tv(expected);
        let act = self.resolve_tv(actual);
        if exp == act || exp == TypeId::ERROR || act == TypeId::ERROR {
            return true;
        }

        match (
            self.ctx.type_registry.get(exp).clone(),
            self.ctx.type_registry.get(act).clone(),
        ) {
            (
                TypeKind::Slice {
                    is_mut: exp_mut,
                    elem: exp_elem,
                },
                TypeKind::Slice {
                    is_mut: act_mut,
                    elem: act_elem,
                },
            ) => (!exp_mut || act_mut) && self.resolve_tv(exp_elem) == self.resolve_tv(act_elem),
            (
                TypeKind::Slice {
                    is_mut: false,
                    elem: exp_elem,
                },
                TypeKind::Array { elem: act_elem, .. },
            ) => self.resolve_tv(exp_elem) == self.resolve_tv(act_elem),
            (
                TypeKind::AnonymousStruct(exp_is_extern, mut exp_fields),
                TypeKind::AnonymousStruct(act_is_extern, mut act_fields),
            ) => {
                if exp_is_extern != act_is_extern || exp_fields.len() != act_fields.len() {
                    return false;
                }
                exp_fields.sort_by_key(|field| field.name);
                act_fields.sort_by_key(|field| field.name);
                exp_fields.iter().zip(act_fields.iter()).all(|(exp, act)| {
                    exp.name == act.name && self.type_is_field_coercible(exp.ty, act.ty)
                })
            }
            _ => false,
        }
    }

    fn compare_named_fields_to_anonymous(
        &mut self,
        generics: &[kernc_ast::GenericParam],
        named_fields: &[kernc_ast::StructFieldDef],
        args: &[GenericArg],
        anon_fields: &[crate::ty::AnonymousField],
        _sort_named: bool,
    ) -> bool {
        if anon_fields.len() != named_fields.len() {
            return false;
        }

        let mut act_fields = Vec::new();
        for f in named_fields {
            let raw_ty = self
                .ctx
                .facts
                .node_types
                .get(&f.type_node.id)
                .copied()
                .unwrap_or(TypeId::ERROR);

            let inst_ty = if !generics.is_empty() && !args.is_empty() {
                let mut map = FastHashMap::default();
                for (i, param) in generics.iter().enumerate() {
                    if let Some(arg) = args.get(i).copied() {
                        map.insert(param.name, arg);
                    }
                }
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                subst.substitute(raw_ty)
            } else {
                raw_ty
            };

            act_fields.push((f.name, self.resolve_tv(inst_ty)));
        }

        act_fields.sort_by_key(|f| f.0);

        for (exp_f, act_f) in anon_fields.iter().zip(act_fields.iter()) {
            if exp_f.name != act_f.0 || self.resolve_tv(exp_f.ty) != act_f.1 {
                return false;
            }
        }

        true
    }

    pub(super) fn check_volatile_coercions(
        &mut self,
        _expr: &Expr,
        act: TypeId,
        exp_kind: &TypeKind,
        act_kind: &TypeKind,
    ) -> bool {
        if let TypeKind::VolatilePtr {
            is_mut: e_mut,
            elem: e_inner,
        } = exp_kind
            && let TypeKind::VolatilePtr {
                is_mut: a_mut,
                elem: a_inner,
            } = act_kind
            && (!*e_mut || *a_mut)
        {
            let e_norm = self.resolve_tv(*e_inner);
            let a_norm = self.resolve_tv(*a_inner);
            if e_norm == a_norm {
                return true;
            }
            if self.ctx.type_registry.is_void(e_norm) {
                return true;
            }
            if self.is_anonymous_aggregate_equivalent(e_norm, a_norm) {
                return true;
            }
            if matches!(
                (
                    self.ctx.type_registry.get(e_norm),
                    self.ctx.type_registry.get(a_norm)
                ),
                (TypeKind::TraitObject(..), TypeKind::TraitObject(..))
            ) && self.is_trait_object_upcast(a_norm, e_norm)
            {
                return true;
            }
            if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(e_norm)
                && self.check_trait_impl(act, e_norm)
            {
                return true;
            }
        }
        false
    }

    pub(super) fn check_slice_and_array_decay(
        &mut self,
        expr: &Expr,
        exp_kind: &TypeKind,
        act_kind: &TypeKind,
    ) -> bool {
        if let TypeKind::Slice {
            is_mut: e_mut,
            elem: exp_elem,
        } = exp_kind
        {
            if let TypeKind::Slice {
                is_mut: act_mut,
                elem: act_elem,
            } = act_kind
                && (!*e_mut || *act_mut)
                && self.resolve_tv(*exp_elem) == self.resolve_tv(*act_elem)
            {
                return true;
            }
            match self.check_array_decay(expr, *e_mut, *exp_elem, act_kind, expr.span) {
                Ok(true) => return true,
                Err(()) => return false,
                Ok(false) => {}
            }
        }
        false
    }

    /// Decay an array into a slice when the element types are compatible.
    fn check_array_decay(
        &mut self,
        expr: &Expr,
        exp_is_mut: bool,
        exp_elem: TypeId,
        act_kind: &TypeKind,
        span: Span,
    ) -> Result<bool, ()> {
        if let TypeKind::Array { elem: act_elem, .. } = act_kind {
            let exp_base = self.resolve_tv(exp_elem);
            let act_base = self.resolve_tv(*act_elem);

            if exp_base == act_base {
                if exp_is_mut && !self.can_take_mut_address_of(expr) {
                    self.ctx
                        .struct_error(
                            span,
                            "cannot implicitly convert an immutable array location to a mutable slice `[]mut T`",
                        )
                        .with_hint(
                            "mutable slice decay requires a mutable array binding, mutable field path, or mutable pointer dereference",
                        )
                        .emit();
                    return Err(());
                }
                return Ok(true);
            }
        }
        Ok(false)
    }
}
