use super::*;
use kernc_utils::FastHashMap;

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    pub(super) fn validate_trait_impl_method_contracts(&mut self) {
        let trait_impl_entries = self.ctx.trait_impl_entries();
        for entry in trait_impl_entries {
            let impl_def = entry.def;
            let Some(resolved_trait_ty) = impl_def.resolved_trait_ty else {
                continue;
            };
            self.validate_trait_impl_method_contract(&impl_def, resolved_trait_ty);
        }
    }

    fn validate_trait_impl_method_contract(
        &mut self,
        impl_def: &ImplDef,
        resolved_trait_ty: TypeId,
    ) {
        if impl_def.is_imported || resolved_trait_ty == TypeId::ERROR {
            return;
        }

        let resolved_trait_ty = self.ctx.type_registry.normalize(resolved_trait_ty);
        let TypeKind::TraitObject(trait_def_id, _, _) =
            self.ctx.type_registry.get(resolved_trait_ty).clone()
        else {
            return;
        };
        let Some(Def::Trait(trait_def)) = self.ctx.defs.get(trait_def_id.0 as usize).cloned()
        else {
            return;
        };

        let implemented_methods = impl_def
            .methods
            .iter()
            .filter_map(|method_id| match self.ctx.defs.get(method_id.0 as usize) {
                Some(Def::Function(function)) => Some(function.name),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        let trait_name = self.ctx.resolve(trait_def.name).to_string();

        for method in &trait_def.methods {
            if method.default_impl.is_some() || implemented_methods.contains(&method.signature.name)
            {
                continue;
            }

            let method_name = self.ctx.resolve(method.signature.name).to_string();
            self.ctx
                .struct_error(
                    impl_def.span,
                    format!(
                        "impl of trait `{}` is missing required method `{}`",
                        trait_name, method_name
                    ),
                )
                .with_code(kernc_utils::DiagnosticCode::MissingTraitImplMethod)
                .with_span_label(impl_def.span, "this trait impl is incomplete")
                .with_span_label(method.signature.name_span, "required method declared here")
                .with_hint(format!("add method stub `{}` to this impl", method_name))
                .emit();
        }
    }

    pub(super) fn validate_trait_impl_coherence(&mut self) {
        let trait_impl_groups = self.ctx.trait_impl_groups().clone();
        for trait_impl_ids in trait_impl_groups.into_values() {
            for (index, left_impl_id) in trait_impl_ids.iter().copied().enumerate() {
                for right_impl_id in trait_impl_ids.iter().copied().skip(index + 1) {
                    let Some(overlap) =
                        self.overlapping_trait_impl_pair(left_impl_id, right_impl_id)
                    else {
                        continue;
                    };
                    if matches!(
                        crate::query::compare_impl_specificity(
                            self.ctx,
                            left_impl_id,
                            right_impl_id
                        ),
                        crate::query::ImplSpecificity::LeftMoreSpecific
                            | crate::query::ImplSpecificity::RightMoreSpecific
                    ) {
                        continue;
                    }

                    let left_target = self.ctx.ty_to_string(overlap.left_target_ty);
                    let left_trait = self.ctx.ty_to_string(overlap.left_trait_ty);
                    let right_target = self.ctx.ty_to_string(overlap.right_target_ty);
                    let right_trait = self.ctx.ty_to_string(overlap.right_trait_ty);

                    self.ctx
                    .struct_error(
                        overlap.right_span,
                        format!(
                            "overlapping trait impls are not allowed for `{}` and `{}`",
                            right_target, right_trait
                        ),
                    )
                    .with_hint(
                        "Kern requires trait impls to be globally coherent; overlapping heads would make proof search and associated type projection ambiguous",
                    )
                    .with_span_label(
                        overlap.left_span,
                        format!("first impl head: `{} : {}`", left_target, left_trait),
                    )
                    .with_span_label(
                        overlap.right_span,
                        format!("second impl head: `{} : {}`", right_target, right_trait),
                    )
                    .emit();
                }
            }
        }
    }

    pub(super) fn validate_trait_impl_supertrait_contracts_for_impl(
        &mut self,
        impl_def: &ImplDef,
        resolved_target: TypeId,
        resolved_trait: TypeId,
    ) {
        if resolved_target == TypeId::ERROR || resolved_trait == TypeId::ERROR {
            return;
        }

        let resolved_target = self.ctx.type_registry.normalize(resolved_target);
        let resolved_trait = self.ctx.type_registry.normalize(resolved_trait);
        let TypeKind::TraitObject(trait_def_id, trait_args, assoc_bindings) =
            self.ctx.type_registry.get(resolved_trait).clone()
        else {
            return;
        };
        let Some(Def::Trait(trait_def)) = self.ctx.defs.get(trait_def_id.0 as usize).cloned()
        else {
            return;
        };
        if trait_def.resolved_supertraits.is_empty() {
            return;
        }

        let trait_name = self.ctx.resolve(trait_def.name).to_string();
        let trait_arg_map = trait_def
            .generics
            .iter()
            .zip(trait_args.iter())
            .map(|(param, arg)| (param.name, *arg))
            .collect::<FastHashMap<_, _>>();
        let assoc_binding_map = assoc_bindings.into_iter().collect::<FastHashMap<_, _>>();

        let prev_bounds_len = self.push_impl_context_where_bounds(impl_def);
        for super_ty in trait_def.resolved_supertraits {
            let instantiated_super = if trait_arg_map.is_empty() {
                super_ty
            } else {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &trait_arg_map);
                subst.substitute(super_ty)
            };
            let instantiated_super = crate::ty::substitute_associated_types(
                &mut self.ctx.type_registry,
                &self.ctx.defs,
                instantiated_super,
                &assoc_binding_map,
            );
            let instantiated_super = crate::query::augment_trait_object_assoc_bindings_from_map(
                self.ctx,
                instantiated_super,
                &assoc_binding_map,
            );
            // Traversal needs inherited assoc bindings to flow through intermediate traits, but
            // the proof we check here must be phrased as the supertrait actually declares it.
            // Otherwise a richer child-trait head can leak extra assoc equalities onto the
            // parent and make an otherwise valid `impl T: Child` fail `impl T: Parent`.
            let instantiated_super = crate::query::retain_declared_trait_object_assoc_bindings(
                self.ctx,
                instantiated_super,
            );
            let instantiated_super = self.ctx.normalize_concrete_type(instantiated_super);
            let instantiated_super = self.ctx.type_registry.normalize(instantiated_super);
            if instantiated_super == TypeId::ERROR {
                continue;
            }

            let super_ok = {
                let mut checker = ExprChecker::new(self.ctx, None);
                checker.check_trait_impl(resolved_target, instantiated_super)
            };
            if super_ok {
                continue;
            }

            let target_str = self.ctx.ty_to_string(resolved_target);
            let super_str = self.ctx.ty_to_string(instantiated_super);
            self.ctx
                .struct_error(
                    impl_def.span,
                    format!(
                        "impl of trait `{}` is missing a required supertrait proof",
                        trait_name
                    ),
                )
                .with_span_label(
                    impl_def.span,
                    "this impl does not prove the declared supertrait contract",
                )
                .with_hint(format!("required bound: `{}: {}`", target_str, super_str))
                .with_hint(
                    "every trait impl must also establish each declared supertrait for the same target",
                )
                .emit();
        }
        self.ctx.analysis.active_bounds.truncate(prev_bounds_len);
        self.ctx.clear_active_bound_caches();
    }

    pub(super) fn validate_trait_impl_orphan(
        &mut self,
        impl_def: &ImplDef,
        target_ty: TypeId,
        trait_ty: TypeId,
    ) {
        if impl_def.is_imported || impl_def.parent_module.is_none() {
            return;
        }

        if target_ty == TypeId::ERROR || trait_ty == TypeId::ERROR {
            return;
        }

        if self.trait_impl_is_orphan_legal(impl_def.id, target_ty, trait_ty) {
            return;
        }

        self.ctx
            .struct_error(
                impl_def.span,
                format!(
                    "orphan trait impls are not allowed for `{}` and `{}`",
                    self.ctx.ty_to_string(target_ty),
                    self.ctx.ty_to_string(trait_ty)
                ),
            )
            .with_hint(
                "when the trait comes from another package or module root, the impl target must be anchored by a local type (directly or through builtin pointer/slice/array wrappers)",
            )
            .with_hint(
                "this prevents downstream packages from creating competing global proofs for the same foreign trait and foreign type family",
            )
            .emit();
    }

    pub(super) fn validate_impl_associated_type_targets(&mut self) {
        for entry in self.ctx.trait_impl_entries() {
            let impl_def = entry.def;

            for assoc_id in impl_def.assoc_types {
                let Some(assoc_def) = self.ctx.defs.get(assoc_id.0 as usize).and_then(|def| {
                    if let Def::AssociatedType(assoc_def) = def {
                        Some(assoc_def.clone())
                    } else {
                        None
                    }
                }) else {
                    continue;
                };
                let Some(target) = assoc_def.target.as_ref() else {
                    continue;
                };
                let resolved_target = self.ctx.node_type_or_error(target.id);
                if resolved_target == TypeId::ERROR {
                    continue;
                }

                let _ = self.ctx.normalize_concrete_type(resolved_target);
            }
        }
    }

    fn overlapping_trait_impl_pair(
        &mut self,
        left_impl_id: DefId,
        right_impl_id: DefId,
    ) -> Option<OverlappingTraitImplPair> {
        let (left_impl, right_impl) = {
            let left_impl = self.ctx.defs.get(left_impl_id.0 as usize).and_then(|def| {
                if let Def::Impl(impl_def) = def {
                    Some(impl_def.clone())
                } else {
                    None
                }
            })?;
            let right_impl = self
                .ctx
                .defs
                .get(right_impl_id.0 as usize)
                .and_then(|def| {
                    if let Def::Impl(impl_def) = def {
                        Some(impl_def.clone())
                    } else {
                        None
                    }
                })?;
            (left_impl, right_impl)
        };

        let _ = left_impl.trait_type.as_ref()?;
        let _ = right_impl.trait_type.as_ref()?;
        if left_impl.parent_module.is_none() || right_impl.parent_module.is_none() {
            return None;
        }

        let left_target_ty = self.ctx.node_type_or_error(left_impl.target_type.id);
        let left_trait_ty = left_impl
            .trait_type
            .as_ref()
            .and_then(|trait_ty| self.ctx.node_type(trait_ty.id))
            .unwrap_or(TypeId::ERROR);
        let right_target_ty = self.ctx.node_type_or_error(right_impl.target_type.id);
        let right_trait_ty = right_impl
            .trait_type
            .as_ref()
            .and_then(|trait_ty| self.ctx.node_type(trait_ty.id))
            .unwrap_or(TypeId::ERROR);
        let left_trait_head_ty = crate::query::erase_trait_assoc_bindings(self.ctx, left_trait_ty);
        let right_trait_head_ty =
            crate::query::erase_trait_assoc_bindings(self.ctx, right_trait_ty);

        if matches!(
            (
                left_target_ty,
                left_trait_head_ty,
                right_target_ty,
                right_trait_head_ty
            ),
            (TypeId::ERROR, _, _, _)
                | (_, TypeId::ERROR, _, _)
                | (_, _, TypeId::ERROR, _)
                | (_, _, _, TypeId::ERROR)
        ) {
            return None;
        }

        let overlaps = {
            let mut checker = ExprChecker::new(self.ctx, None);
            let (left_fresh_target, left_fresh_trait) = Self::freshen_impl_head_types_for_overlap(
                &mut checker,
                &left_impl,
                left_target_ty,
                left_trait_head_ty,
            );
            let (right_fresh_target, right_fresh_trait) = Self::freshen_impl_head_types_for_overlap(
                &mut checker,
                &right_impl,
                right_target_ty,
                right_trait_head_ty,
            );
            let mut type_map = FastHashMap::default();
            let mut const_map = FastHashMap::default();
            checker.unify_with_const_map(
                left_fresh_target,
                right_fresh_target,
                &mut type_map,
                &mut const_map,
            ) && checker.unify_with_const_map(
                left_fresh_trait,
                right_fresh_trait,
                &mut type_map,
                &mut const_map,
            )
        };

        if !overlaps {
            return None;
        }

        Some(OverlappingTraitImplPair {
            left_span: left_impl.span,
            right_span: right_impl.span,
            left_target_ty,
            left_trait_ty,
            right_target_ty,
            right_trait_ty,
        })
    }

    fn freshen_impl_head_types_for_overlap(
        checker: &mut ExprChecker<'_, '_>,
        impl_def: &ImplDef,
        target_ty: TypeId,
        trait_ty: TypeId,
    ) -> (TypeId, TypeId) {
        let mut subst_map = FastHashMap::default();

        for (index, param) in impl_def.generics.iter().enumerate() {
            let fresh_name = checker.ctx.intern(&format!(
                "__coherence_impl{}_{}_{}",
                impl_def.id.0,
                index,
                checker.ctx.resolve(param.name)
            ));
            let fresh_arg = match &param.kind {
                ast::GenericParamKind::Type => GenericArg::Type(checker.fresh_type_var()),
                ast::GenericParamKind::Const { ty } => {
                    let const_ty = checker.ctx.node_type_or_error(ty.id);
                    GenericArg::Const(ConstGeneric::Param(fresh_name, const_ty))
                }
            };
            subst_map.insert(param.name, fresh_arg);
        }

        let mut subst = Substituter::new(&mut checker.ctx.type_registry, &subst_map);
        (subst.substitute(target_ty), subst.substitute(trait_ty))
    }

    fn trait_impl_is_orphan_legal(
        &mut self,
        impl_id: DefId,
        target_ty: TypeId,
        trait_ty: TypeId,
    ) -> bool {
        let Some(impl_home) = self.definition_locality(impl_id) else {
            return true;
        };

        let trait_norm = self.ctx.type_registry.normalize(trait_ty);
        let TypeKind::TraitObject(trait_def_id, _, _) =
            self.ctx.type_registry.get(trait_norm).clone()
        else {
            return false;
        };

        if self
            .definition_locality(trait_def_id)
            .is_none_or(|trait_home| trait_home == impl_home)
        {
            return true;
        }

        self.type_has_local_impl_anchor(target_ty, impl_home)
    }

    fn definition_locality(&self, def_id: DefId) -> Option<ImplLocality> {
        let owner_module = self.ctx.def_parent_module(def_id)?;
        Some(self.module_locality(owner_module))
    }

    fn module_locality(&self, module_id: DefId) -> ImplLocality {
        self.ctx.root_module_package_name(module_id).map_or_else(
            || ImplLocality::Root(self.ctx.module_root(module_id)),
            ImplLocality::Package,
        )
    }

    fn type_has_local_impl_anchor(&mut self, ty: TypeId, impl_home: ImplLocality) -> bool {
        let ty = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(ty).clone() {
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::Array { elem, .. }
            | TypeKind::ArrayInfer { elem, .. } => self.type_has_local_impl_anchor(elem, impl_home),
            TypeKind::Range { start, end, .. } => {
                start.is_some_and(|ty| self.type_has_local_impl_anchor(ty, impl_home))
                    || end.is_some_and(|ty| self.type_has_local_impl_anchor(ty, impl_home))
            }
            TypeKind::Alias(_, target) => self.type_has_local_impl_anchor(target, impl_home),
            TypeKind::Def(def_id, _)
            | TypeKind::Enum(def_id, _)
            | TypeKind::Associated(def_id, _)
            | TypeKind::FnDef(def_id, _)
            | TypeKind::TraitObject(def_id, _, _) => {
                self.definition_is_local_anchor(def_id, impl_home)
            }
            TypeKind::AnonymousStruct(..)
            | TypeKind::AnonymousUnion(..)
            | TypeKind::AnonymousEnum(..)
            | TypeKind::ClosureInterface { .. }
            | TypeKind::AnonymousState { .. } => true,
            TypeKind::Primitive(_)
            | TypeKind::Simd { .. }
            | TypeKind::Function { .. }
            | TypeKind::Module(_)
            | TypeKind::Error
            | TypeKind::TypeVar(_)
            | TypeKind::Param(_)
            | TypeKind::Projection { .. }
            | TypeKind::EnumPayload(..)
            | TypeKind::AnonymousEnumPayload(..) => false,
        }
    }

    fn definition_is_local_anchor(&self, def_id: DefId, impl_home: ImplLocality) -> bool {
        self.definition_locality(def_id) == Some(impl_home)
    }
}

struct OverlappingTraitImplPair {
    left_span: Span,
    right_span: Span,
    left_target_ty: TypeId,
    left_trait_ty: TypeId,
    right_target_ty: TypeId,
    right_trait_ty: TypeId,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ImplLocality {
    Package(SymbolId),
    Root(DefId),
}
