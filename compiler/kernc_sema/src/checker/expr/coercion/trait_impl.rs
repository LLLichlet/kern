use super::*;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    // Candidate impls may solve their own local generics while matching an obligation,
    // but they must never write back into the obligation's rigid generic parameters.
    pub(crate) fn match_available_type_against_requirement(
        &mut self,
        available_ty: TypeId,
        required_ty: TypeId,
        type_map: &mut FastHashMap<SymbolId, TypeId>,
        const_map: &mut FastHashMap<SymbolId, crate::ty::ConstGeneric>,
    ) -> bool {
        let available_norm = self.resolve_tv(available_ty);
        let required_norm = self.resolve_tv(required_ty);

        let available_kind = self.ctx.type_registry.get(available_norm).clone();
        let required_kind = self.ctx.type_registry.get(required_norm).clone();

        match (available_kind, required_kind) {
            (TypeKind::TypeVar(vid), _) => {
                self.bind_type_var(vid, required_ty);
                true
            }
            (_, TypeKind::TypeVar(vid)) => {
                self.bind_type_var(vid, available_ty);
                true
            }
            (TypeKind::Param(name), _) => {
                if let Some(&existing_ty) = type_map.get(&name) {
                    existing_ty == required_ty
                } else if matches!(
                    self.ctx.type_registry.get(required_norm),
                    TypeKind::Param(other) if *other == name
                ) {
                    type_map.insert(name, required_ty);
                    true
                } else if self.generic_param_occurs_in_type_with_map(name, required_ty, type_map) {
                    false
                } else {
                    type_map.insert(name, required_ty);
                    true
                }
            }
            (_, TypeKind::Param(_)) => false,
            (
                TypeKind::Pointer {
                    is_mut: available_mut,
                    elem: available_elem,
                },
                TypeKind::Pointer {
                    is_mut: required_mut,
                    elem: required_elem,
                },
            ) => {
                available_mut == required_mut
                    && self.match_available_type_against_requirement(
                        available_elem,
                        required_elem,
                        type_map,
                        const_map,
                    )
            }
            (
                TypeKind::VolatilePtr {
                    is_mut: available_mut,
                    elem: available_elem,
                },
                TypeKind::VolatilePtr {
                    is_mut: required_mut,
                    elem: required_elem,
                },
            ) => {
                available_mut == required_mut
                    && self.match_available_type_against_requirement(
                        available_elem,
                        required_elem,
                        type_map,
                        const_map,
                    )
            }
            (
                TypeKind::Slice {
                    is_mut: available_mut,
                    elem: available_elem,
                },
                TypeKind::Slice {
                    is_mut: required_mut,
                    elem: required_elem,
                },
            ) => {
                available_mut == required_mut
                    && self.match_available_type_against_requirement(
                        available_elem,
                        required_elem,
                        type_map,
                        const_map,
                    )
            }
            (
                TypeKind::Array {
                    elem: available_elem,
                    len: available_len,
                },
                TypeKind::Array {
                    elem: required_elem,
                    len: required_len,
                },
            ) => {
                self.match_available_const_generic_against_requirement(
                    available_len,
                    required_len,
                    const_map,
                ) && self.match_available_type_against_requirement(
                    available_elem,
                    required_elem,
                    type_map,
                    const_map,
                )
            }
            (
                TypeKind::ArrayInfer {
                    elem: available_elem,
                },
                TypeKind::ArrayInfer {
                    elem: required_elem,
                },
            ) => self.match_available_type_against_requirement(
                available_elem,
                required_elem,
                type_map,
                const_map,
            ),
            (
                TypeKind::Simd {
                    elem: available_elem,
                    lanes: available_lanes,
                },
                TypeKind::Simd {
                    elem: required_elem,
                    lanes: required_lanes,
                },
            ) => {
                available_lanes == required_lanes
                    && self.match_available_type_against_requirement(
                        available_elem,
                        required_elem,
                        type_map,
                        const_map,
                    )
            }
            (
                TypeKind::Def(available_id, available_args),
                TypeKind::Def(required_id, required_args),
            ) if available_id == required_id => self
                .match_available_generic_args_against_requirement(
                    &available_args,
                    &required_args,
                    type_map,
                    const_map,
                ),
            (
                TypeKind::Enum(available_id, available_args),
                TypeKind::Enum(required_id, required_args),
            ) if available_id == required_id => self
                .match_available_generic_args_against_requirement(
                    &available_args,
                    &required_args,
                    type_map,
                    const_map,
                ),
            (
                TypeKind::EnumPayload(available_id, available_args),
                TypeKind::EnumPayload(required_id, required_args),
            ) if available_id == required_id => self
                .match_available_generic_args_against_requirement(
                    &available_args,
                    &required_args,
                    type_map,
                    const_map,
                ),
            (
                TypeKind::TraitObject(available_id, available_args, available_assoc),
                TypeKind::TraitObject(required_id, required_args, required_assoc),
            ) if available_id == required_id => {
                self.match_available_generic_args_against_requirement(
                    &available_args,
                    &required_args,
                    type_map,
                    const_map,
                ) && {
                    // Obligations only constrain the associated items they mention.
                    // A candidate proof may carry extra normalized assoc equalities.
                    let available_assoc =
                        available_assoc.into_iter().collect::<FastHashMap<_, _>>();
                    required_assoc
                        .into_iter()
                        .all(|(assoc_def_id, required_assoc_ty)| {
                            let Some(&available_assoc_ty) = available_assoc.get(&assoc_def_id)
                            else {
                                return false;
                            };
                            self.match_available_type_against_requirement(
                                available_assoc_ty,
                                required_assoc_ty,
                                type_map,
                                const_map,
                            )
                        })
                }
            }
            (
                TypeKind::Projection {
                    target: available_target,
                    trait_def_id: available_trait_def_id,
                    trait_args: available_trait_args,
                    assoc_def_id: available_assoc_def_id,
                    assoc_args: available_assoc_args,
                },
                TypeKind::Projection {
                    target: required_target,
                    trait_def_id: required_trait_def_id,
                    trait_args: required_trait_args,
                    assoc_def_id: required_assoc_def_id,
                    assoc_args: required_assoc_args,
                },
            ) => {
                available_trait_def_id == required_trait_def_id
                    && available_assoc_def_id == required_assoc_def_id
                    && self.match_available_type_against_requirement(
                        available_target,
                        required_target,
                        type_map,
                        const_map,
                    )
                    && self.match_available_generic_args_against_requirement(
                        &available_trait_args,
                        &required_trait_args,
                        type_map,
                        const_map,
                    )
                    && self.match_available_generic_args_against_requirement(
                        &available_assoc_args,
                        &required_assoc_args,
                        type_map,
                        const_map,
                    )
            }
            (
                TypeKind::Associated(available_def_id, available_args),
                TypeKind::Associated(required_def_id, required_args),
            ) if available_def_id == required_def_id => self
                .match_available_generic_args_against_requirement(
                    &available_args,
                    &required_args,
                    type_map,
                    const_map,
                ),
            (
                TypeKind::ClosureInterface {
                    params: available_params,
                    ret: available_ret,
                },
                TypeKind::ClosureInterface {
                    params: required_params,
                    ret: required_ret,
                },
            ) => {
                available_params.len() == required_params.len()
                    && available_params.iter().zip(required_params.iter()).all(
                        |(available_param, required_param)| {
                            self.match_available_type_against_requirement(
                                *available_param,
                                *required_param,
                                type_map,
                                const_map,
                            )
                        },
                    )
                    && self.match_available_type_against_requirement(
                        available_ret,
                        required_ret,
                        type_map,
                        const_map,
                    )
            }
            (
                TypeKind::AnonymousState {
                    captures: available_captures,
                    params: available_params,
                    ret: available_ret,
                    ..
                },
                TypeKind::AnonymousState {
                    captures: required_captures,
                    params: required_params,
                    ret: required_ret,
                    ..
                },
            ) => {
                available_captures.len() == required_captures.len()
                    && available_params.len() == required_params.len()
                    && available_captures.iter().zip(required_captures.iter()).all(
                        |(available_capture, required_capture)| {
                            self.match_available_type_against_requirement(
                                *available_capture,
                                *required_capture,
                                type_map,
                                const_map,
                            )
                        },
                    )
                    && available_params.iter().zip(required_params.iter()).all(
                        |(available_param, required_param)| {
                            self.match_available_type_against_requirement(
                                *available_param,
                                *required_param,
                                type_map,
                                const_map,
                            )
                        },
                    )
                    && self.match_available_type_against_requirement(
                        available_ret,
                        required_ret,
                        type_map,
                        const_map,
                    )
            }
            (
                TypeKind::Function {
                    params: available_params,
                    ret: available_ret,
                    is_variadic: available_variadic,
                },
                TypeKind::Function {
                    params: required_params,
                    ret: required_ret,
                    is_variadic: required_variadic,
                },
            ) => {
                available_variadic == required_variadic
                    && available_params.len() == required_params.len()
                    && available_params.iter().zip(required_params.iter()).all(
                        |(available_param, required_param)| {
                            self.match_available_type_against_requirement(
                                *available_param,
                                *required_param,
                                type_map,
                                const_map,
                            )
                        },
                    )
                    && self.match_available_type_against_requirement(
                        available_ret,
                        required_ret,
                        type_map,
                        const_map,
                    )
            }
            (
                TypeKind::FnDef(available_id, available_args),
                TypeKind::FnDef(required_id, required_args),
            ) if available_id == required_id => self
                .match_available_generic_args_against_requirement(
                    &available_args,
                    &required_args,
                    type_map,
                    const_map,
                ),
            (
                TypeKind::AnonymousStruct(available_packed, available_fields),
                TypeKind::AnonymousStruct(required_packed, required_fields),
            ) => {
                available_packed == required_packed
                    && available_fields.len() == required_fields.len()
                    && available_fields.iter().zip(required_fields.iter()).all(
                        |(available_field, required_field)| {
                            available_field.name == required_field.name
                                && self.match_available_type_against_requirement(
                                    available_field.ty,
                                    required_field.ty,
                                    type_map,
                                    const_map,
                                )
                        },
                    )
            }
            (
                TypeKind::AnonymousUnion(available_packed, available_fields),
                TypeKind::AnonymousUnion(required_packed, required_fields),
            ) => {
                available_packed == required_packed
                    && available_fields.len() == required_fields.len()
                    && available_fields.iter().zip(required_fields.iter()).all(
                        |(available_field, required_field)| {
                            available_field.name == required_field.name
                                && self.match_available_type_against_requirement(
                                    available_field.ty,
                                    required_field.ty,
                                    type_map,
                                    const_map,
                                )
                        },
                    )
            }
            (TypeKind::AnonymousEnum(available_enum), TypeKind::AnonymousEnum(required_enum)) => {
                if available_enum.builtin != required_enum.builtin
                    || available_enum.backing_ty.is_some() != required_enum.backing_ty.is_some()
                    || available_enum.variants.len() != required_enum.variants.len()
                {
                    return false;
                }

                if let (Some(available_backing), Some(required_backing)) =
                    (available_enum.backing_ty, required_enum.backing_ty)
                    && !self.match_available_type_against_requirement(
                        available_backing,
                        required_backing,
                        type_map,
                        const_map,
                    )
                {
                    return false;
                }

                available_enum
                    .variants
                    .iter()
                    .zip(required_enum.variants.iter())
                    .all(|(available_variant, required_variant)| {
                        if available_variant.name != required_variant.name
                            || available_variant.explicit_value != required_variant.explicit_value
                            || available_variant.payload_ty.is_some()
                                != required_variant.payload_ty.is_some()
                        {
                            return false;
                        }

                        match (available_variant.payload_ty, required_variant.payload_ty) {
                            (Some(available_payload), Some(required_payload)) => self
                                .match_available_type_against_requirement(
                                    available_payload,
                                    required_payload,
                                    type_map,
                                    const_map,
                                ),
                            (None, None) => true,
                            _ => false,
                        }
                    })
            }
            (
                TypeKind::AnonymousEnumPayload(available_payload),
                TypeKind::AnonymousEnumPayload(required_payload),
            ) => self.match_available_type_against_requirement(
                available_payload,
                required_payload,
                type_map,
                const_map,
            ),
            _ => available_norm == required_norm,
        }
    }

    fn match_available_generic_args_against_requirement(
        &mut self,
        available_args: &[crate::ty::GenericArg],
        required_args: &[crate::ty::GenericArg],
        type_map: &mut FastHashMap<SymbolId, TypeId>,
        const_map: &mut FastHashMap<SymbolId, crate::ty::ConstGeneric>,
    ) -> bool {
        available_args.len() == required_args.len()
            && available_args.iter().zip(required_args.iter()).all(
                |(available_arg, required_arg)| {
                    self.match_available_generic_arg_against_requirement(
                        *available_arg,
                        *required_arg,
                        type_map,
                        const_map,
                    )
                },
            )
    }

    fn match_available_generic_arg_against_requirement(
        &mut self,
        available_arg: crate::ty::GenericArg,
        required_arg: crate::ty::GenericArg,
        type_map: &mut FastHashMap<SymbolId, TypeId>,
        const_map: &mut FastHashMap<SymbolId, crate::ty::ConstGeneric>,
    ) -> bool {
        match (available_arg, required_arg) {
            (
                crate::ty::GenericArg::Type(available_ty),
                crate::ty::GenericArg::Type(required_ty),
            ) => self.match_available_type_against_requirement(
                available_ty,
                required_ty,
                type_map,
                const_map,
            ),
            (
                crate::ty::GenericArg::Const(available_const),
                crate::ty::GenericArg::Const(required_const),
            ) => self.match_available_const_generic_against_requirement(
                available_const,
                required_const,
                const_map,
            ),
            _ => false,
        }
    }

    fn match_available_const_generic_against_requirement(
        &mut self,
        available: crate::ty::ConstGeneric,
        required: crate::ty::ConstGeneric,
        const_map: &mut FastHashMap<SymbolId, crate::ty::ConstGeneric>,
    ) -> bool {
        let available = {
            let subst_map = const_map
                .iter()
                .map(|(&name, &value)| (name, crate::ty::GenericArg::Const(value)))
                .collect::<FastHashMap<_, _>>();
            if subst_map.is_empty() {
                self.ctx.type_registry.fold_const_generic(available)
            } else {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
                subst.substitute_const_generic(available)
            }
        };
        let required = {
            let subst_map = const_map
                .iter()
                .map(|(&name, &value)| (name, crate::ty::GenericArg::Const(value)))
                .collect::<FastHashMap<_, _>>();
            if subst_map.is_empty() {
                self.ctx.type_registry.fold_const_generic(required)
            } else {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
                subst.substitute_const_generic(required)
            }
        };

        let available_ty = self.ctx.type_registry.const_generic_ty(available);
        let required_ty = self.ctx.type_registry.const_generic_ty(required);
        if available_ty != required_ty {
            return false;
        }

        match (available, required) {
            (crate::ty::ConstGeneric::Param(name, _), other) => {
                if let Some(&existing) = const_map.get(&name) {
                    existing == other
                } else if self.const_param_occurs_in_const_generic_with_map(name, other, const_map)
                {
                    false
                } else {
                    const_map.insert(name, other);
                    true
                }
            }
            (_, crate::ty::ConstGeneric::Param(_, _)) => available == required,
            _ => available == required,
        }
    }

    fn trait_obligation_matches_available_trait(
        &mut self,
        available_trait_ty: TypeId,
        required_trait_ty: TypeId,
    ) -> bool {
        let available_norm = self.resolve_tv(available_trait_ty);
        let required_norm = self.resolve_tv(required_trait_ty);

        if available_norm == required_norm || available_trait_ty == required_trait_ty {
            return true;
        }

        matches!(
            (
                self.ctx.type_registry.get(available_norm),
                self.ctx.type_registry.get(required_norm)
            ),
            (TypeKind::TraitObject(..), TypeKind::TraitObject(..))
        ) && self.is_trait_object_upcast(available_trait_ty, required_trait_ty)
    }

    pub(crate) fn check_trait_impl(&mut self, source_ty: TypeId, target_trait_ty: TypeId) -> bool {
        let source_norm = self.resolve_tv(source_ty);
        let target_norm = self.resolve_tv(target_trait_ty);
        let obligation = (source_norm, target_norm);
        if self.trait_obligation_stack.contains(&obligation) {
            return false;
        }
        self.trait_obligation_stack.push(obligation);

        let result = (|| {
            let mut visited = FastHashSet::default();
            if self.check_trait_impl_inner(source_ty, target_trait_ty, &mut visited) {
                return true;
            }

            if self.check_builtin_auto_trait_impl(source_ty, target_trait_ty) {
                return true;
            }

            // If a mutable pointer or slice lacks a direct impl, try its immutable form.
            let source_norm = self.resolve_tv(source_ty);
            let downgraded = match self.ctx.type_registry.get(source_norm).clone() {
                TypeKind::Pointer { is_mut: true, elem } => {
                    Some(self.ctx.type_registry.intern(TypeKind::Pointer {
                        is_mut: false,
                        elem,
                    }))
                }
                TypeKind::Pointer { is_mut, elem } => {
                    match self.ctx.type_registry.get(elem).clone() {
                        TypeKind::Slice {
                            is_mut: true,
                            elem: slice_elem,
                        } => {
                            let downgraded_slice = self.ctx.type_registry.intern(TypeKind::Slice {
                                is_mut: false,
                                elem: slice_elem,
                            });
                            Some(self.ctx.type_registry.intern(TypeKind::Pointer {
                                is_mut,
                                elem: downgraded_slice,
                            }))
                        }
                        _ => None,
                    }
                }
                TypeKind::VolatilePtr { is_mut: true, elem } => {
                    Some(self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                        is_mut: false,
                        elem,
                    }))
                }
                TypeKind::VolatilePtr { is_mut, elem } => {
                    match self.ctx.type_registry.get(elem).clone() {
                        TypeKind::Slice {
                            is_mut: true,
                            elem: slice_elem,
                        } => {
                            let downgraded_slice = self.ctx.type_registry.intern(TypeKind::Slice {
                                is_mut: false,
                                elem: slice_elem,
                            });
                            Some(self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                                is_mut,
                                elem: downgraded_slice,
                            }))
                        }
                        _ => None,
                    }
                }
                TypeKind::Slice { is_mut: true, elem } => {
                    Some(self.ctx.type_registry.intern(TypeKind::Slice {
                        is_mut: false,
                        elem,
                    }))
                }
                _ => None,
            };

            if let Some(down_ty) = downgraded {
                let mut visited = FastHashSet::default(); // Restart the search with a fresh visited set.
                return self.check_trait_impl_inner(down_ty, target_trait_ty, &mut visited);
            }

            false
        })();

        let popped = self.trait_obligation_stack.pop();
        debug_assert_eq!(popped, Some(obligation));
        result
    }

    fn check_builtin_auto_trait_impl(
        &mut self,
        source_ty: TypeId,
        target_trait_ty: TypeId,
    ) -> bool {
        let source_norm = self.resolve_tv(source_ty);
        let target_norm = self.resolve_tv(target_trait_ty);

        let TypeKind::TraitObject(trait_def_id, trait_args, _) =
            self.ctx.type_registry.get(target_norm).clone()
        else {
            return false;
        };

        let Some(eq_def_id) = self.ctx.builtin_def("Eq") else {
            return false;
        };
        if trait_def_id == eq_def_id
            && trait_args.len() == 1
            && trait_args[0] == crate::ty::GenericArg::Type(source_norm)
        {
            return match self.ctx.type_registry.get(source_norm).clone() {
                TypeKind::Enum(def_id, _) => {
                    let Def::Enum(def) = &self.ctx.defs[def_id.0 as usize] else {
                        return false;
                    };
                    def.variants
                        .iter()
                        .all(|variant| variant.payload_type.is_none())
                }
                TypeKind::AnonymousEnum(anon) => anon
                    .variants
                    .iter()
                    .all(|variant| variant.payload_ty.is_none()),
                _ => false,
            };
        }

        let simd_elem = self
            .ctx
            .type_registry
            .simd_info(source_norm)
            .map(|(elem, _)| self.resolve_tv(elem));

        let Some(integer_def_id) = self.ctx.builtin_def("Integer") else {
            return false;
        };
        if trait_def_id == integer_def_id && trait_args.is_empty() {
            return matches!(
                simd_elem,
                Some(elem) if self.ctx.type_registry.is_integer(elem)
            );
        }

        let Some(signed_integer_def_id) = self.ctx.builtin_def("SignedInteger") else {
            return false;
        };
        if trait_def_id == signed_integer_def_id && trait_args.is_empty() {
            return matches!(
                simd_elem,
                Some(
                    TypeId::I8
                        | TypeId::I16
                        | TypeId::I32
                        | TypeId::I64
                        | TypeId::I128
                        | TypeId::ISIZE
                )
            );
        }

        let Some(unsigned_integer_def_id) = self.ctx.builtin_def("UnsignedInteger") else {
            return false;
        };
        if trait_def_id == unsigned_integer_def_id && trait_args.is_empty() {
            return matches!(
                simd_elem,
                Some(
                    TypeId::U8
                        | TypeId::U16
                        | TypeId::U32
                        | TypeId::U64
                        | TypeId::U128
                        | TypeId::USIZE
                )
            );
        }

        false
    }

    fn check_trait_impl_inner(
        &mut self,
        source_ty: TypeId,
        target_trait_ty: TypeId,
        visited: &mut FastHashSet<DefId>,
    ) -> bool {
        // === 1. Check active where-bounds from the current environment first ===
        if self.check_trait_impl_in_env_bounds(source_ty, target_trait_ty, visited) {
            return true;
        }

        // === 2. Fall back to globally collected impl blocks ===
        if self.check_trait_impl_in_global_impls(source_ty, target_trait_ty, visited) {
            return true;
        }

        false
    }

    /// Helper 1: check constraints supplied by the current `active_bounds` context.
    fn check_trait_impl_in_env_bounds(
        &mut self,
        source_ty: TypeId,
        target_trait_ty: TypeId,
        _visited: &mut FastHashSet<DefId>,
    ) -> bool {
        if self.ctx.active_bounds.is_empty() {
            return false;
        }

        let active_bounds_ptr = std::ptr::from_ref(self.ctx.active_bounds.as_slice());
        let source_norm = self.resolve_tv(source_ty);
        let mut type_map = FastHashMap::default();
        let mut const_map = FastHashMap::default();
        // Safety: this helper only reads `active_bounds`; it never resizes or replaces the vec.
        for (env_target, env_bounds) in unsafe { &*active_bounds_ptr } {
            type_map.clear();
            const_map.clear();

            // If the queried source type matches the contextual target type, inspect its bounds.
            let matched = if *env_target == source_norm {
                true
            } else {
                self.match_available_type_against_requirement(
                    *env_target,
                    source_ty,
                    &mut type_map,
                    &mut const_map,
                )
            };
            if matched {
                if type_map.is_empty() && const_map.is_empty() {
                    for inst_env_bound in env_bounds.iter().copied() {
                        if self.trait_obligation_matches_available_trait(
                            inst_env_bound,
                            target_trait_ty,
                        ) {
                            return true;
                        }
                    }
                    continue;
                }

                for bound in env_bounds.iter().copied() {
                    let inst_env_bound =
                        self.substitute_type_with_unification_maps(bound, &type_map, &const_map);
                    if self
                        .trait_obligation_matches_available_trait(inst_env_bound, target_trait_ty)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Helper 2: scan globally registered impl blocks.
    fn check_trait_impl_in_global_impls(
        &mut self,
        source_ty: TypeId,
        target_trait_ty: TypeId,
        _visited: &mut FastHashSet<DefId>,
    ) -> bool {
        let trait_impl_ids_ptr = std::ptr::from_ref(self.ctx.trait_impls.as_slice());
        // Safety: this helper only reads the collected impl id list; it never mutates the vec.
        for impl_id in unsafe { &*trait_impl_ids_ptr }.iter().copied() {
            let Some(impl_ptr) = self
                .ctx
                .defs
                .get(impl_id.0 as usize)
                .and_then(|def| match def {
                    Def::Impl(impl_def) => Some(std::ptr::from_ref(impl_def)),
                    _ => None,
                })
            else {
                continue;
            };

            {
                let mut resolver = TypeResolver::new(self.ctx);
                resolver.ensure_impl_signature_types_resolved(impl_id);
            }

            if self.ctx.non_decreasing_impl_requirement(impl_id).is_some() {
                continue;
            }

            // Safety: semantic definitions are stable during type queries; use a raw pointer
            // to avoid cloning each impl block on every trait-impl check.
            let impl_def = unsafe { &*impl_ptr };
            let Some(trait_ast) = &impl_def.trait_type else {
                continue;
            };

            let impl_target_ty = self
                .ctx
                .node_types
                .get(&impl_def.target_type.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let impl_trait_ty = self
                .ctx
                .node_types
                .get(&trait_ast.id)
                .copied()
                .unwrap_or(TypeId::ERROR);

            if impl_target_ty == TypeId::ERROR || impl_trait_ty == TypeId::ERROR {
                continue;
            }

            let mut type_map = FastHashMap::default();
            let mut const_map = FastHashMap::default();

            if self.match_available_type_against_requirement(
                impl_target_ty,
                source_ty,
                &mut type_map,
                &mut const_map,
            ) {
                let instantiated_trait_ty = self.substitute_type_with_unification_maps(
                    impl_trait_ty,
                    &type_map,
                    &const_map,
                );
                let mut trait_type_map = type_map.clone();
                let mut trait_const_map = const_map.clone();
                if self.match_available_type_against_requirement(
                    instantiated_trait_ty,
                    target_trait_ty,
                    &mut trait_type_map,
                    &mut trait_const_map,
                ) && crate::query::impl_bounds_satisfied(
                    self,
                    &impl_def.where_clauses,
                    &trait_type_map,
                    &trait_const_map,
                ) {
                    return true;
                }

                if self.trait_obligation_matches_available_trait(
                    instantiated_trait_ty,
                    target_trait_ty,
                ) && crate::query::impl_bounds_satisfied(
                    self,
                    &impl_def.where_clauses,
                    &type_map,
                    &const_map,
                ) {
                    return true;
                }
            }
        }
        false
    }
}
