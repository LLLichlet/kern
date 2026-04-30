use super::*;
use crate::scope::SymbolNamespace;

struct ImplAssocTypeContract<'a> {
    trait_def_id: DefId,
    trait_generics: &'a [ast::GenericParam],
    trait_args: &'a [GenericArg],
    trait_assoc: &'a AssociatedTypeDef,
    impl_assoc: &'a AssociatedTypeDef,
    resolved_target: TypeId,
    assoc_targets: &'a HashMap<DefId, TypeId>,
}

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    fn canonical_trait_assoc_generic_arg(&mut self, param: &ast::GenericParam) -> GenericArg {
        match &param.kind {
            ast::GenericParamKind::Type => {
                GenericArg::Type(self.ctx.type_registry.intern(TypeKind::Param(param.name)))
            }
            ast::GenericParamKind::Const { ty } => {
                GenericArg::Const(crate::ty::ConstGeneric::Param(
                    param.name,
                    self.ctx
                        .facts
                        .node_types
                        .get(&ty.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR),
                ))
            }
        }
    }

    fn canonicalize_impl_assoc_target_for_trait_binding(
        &mut self,
        trait_assoc: &AssociatedTypeDef,
        impl_assoc: &AssociatedTypeDef,
        target_ty: TypeId,
    ) -> TypeId {
        if target_ty == TypeId::ERROR
            || impl_assoc.generics.is_empty()
            || impl_assoc.generics.len() != trait_assoc.generics.len()
        {
            return target_ty;
        }

        // Trait-object assoc bindings are keyed by the trait assoc `DefId`, so any generic
        // parameters captured in the stored target must also use the trait declaration's generic
        // names. Otherwise a later projection like `T.Mapper.Apply[i32]` has no way to substitute
        // the impl-local `Apply` generic placeholders that were only visible inside the impl body.
        let subst_map = impl_assoc
            .generics
            .iter()
            .zip(trait_assoc.generics.iter())
            .map(|(impl_param, trait_param)| {
                (
                    impl_param.name,
                    self.canonical_trait_assoc_generic_arg(trait_param),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
        subst.substitute(target_ty)
    }

    fn emit_duplicate_assoc_type_definition(
        &mut self,
        assoc_name: SymbolId,
        assoc_span: Span,
        previous_span: Span,
    ) {
        let assoc_name = self.ctx.resolve(assoc_name).to_string();
        self.ctx
            .struct_error(
                assoc_span,
                format!(
                    "the associated type `{}` is defined multiple times",
                    assoc_name
                ),
            )
            .with_hint(format!(
                "`{}` must be defined only once in the same trait or impl",
                assoc_name
            ))
            .with_span_label(
                previous_span,
                format!(
                    "previous definition of associated type `{}` was here",
                    assoc_name
                ),
            )
            .emit();
    }

    fn check_impl_assoc_type_contracts(
        &mut self,
        impl_def: &ImplDef,
        contract: ImplAssocTypeContract<'_>,
    ) {
        if contract.resolved_target == TypeId::ERROR {
            return;
        }

        let trait_generic_args = contract
            .trait_generics
            .iter()
            .zip(contract.trait_args.iter())
            .map(|(param, arg)| (param.name, *arg))
            .collect::<HashMap<_, _>>();

        let prev_bounds_len = self.push_impl_context_where_bounds(impl_def);
        for clause in &contract.impl_assoc.where_clauses {
            let target_ty = self.ctx.type_registry.normalize(
                self.ctx
                    .facts
                    .node_types
                    .get(&clause.target_ty.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR),
            );
            let bounds = clause
                .bounds
                .iter()
                .map(|bound| {
                    self.ctx.type_registry.normalize(
                        self.ctx
                            .facts
                            .node_types
                            .get(&bound.id)
                            .copied()
                            .unwrap_or(TypeId::ERROR),
                    )
                })
                .collect::<Vec<_>>();
            self.ctx.analysis.active_bounds.push((target_ty, bounds));
        }
        self.push_instantiated_where_bounds(
            &contract.trait_assoc.where_clauses,
            &trait_generic_args,
            contract.assoc_targets,
            contract.trait_def_id,
            contract.trait_args,
            contract.resolved_target,
        );
        if self.ctx.analysis.active_bounds.len() != prev_bounds_len {
            self.ctx.clear_active_bound_caches();
        }

        let assoc_name = self.ctx.resolve(contract.impl_assoc.name).to_string();
        for &bound_ty in &contract.trait_assoc.resolved_bounds {
            let instantiated_bound = self.instantiate_trait_assoc_contract_ty(
                bound_ty,
                &trait_generic_args,
                contract.assoc_targets,
                contract.trait_def_id,
                contract.trait_args,
                contract.resolved_target,
            );
            let instantiated_bound = self.ctx.type_registry.normalize(instantiated_bound);
            if instantiated_bound == TypeId::ERROR {
                continue;
            }

            let bound_ok = {
                let mut checker = ExprChecker::new(self.ctx, None);
                checker.check_trait_impl(contract.resolved_target, instantiated_bound)
            };
            if bound_ok {
                continue;
            }

            let target_str = self.ctx.ty_to_string(contract.resolved_target);
            let bound_str = self.ctx.ty_to_string(instantiated_bound);
            self.ctx
                .struct_error(
                    contract.impl_assoc.span,
                    format!(
                        "associated type `{}` does not satisfy the bounds declared by the trait",
                        assoc_name
                    ),
                )
                .with_span_label(
                    contract.impl_assoc.span,
                    "this impl-associated type target does not implement the required bound",
                )
                .with_span_label(
                    contract.trait_assoc.span,
                    "the trait declares the associated-type contract here",
                )
                .with_hint(format!("required bound: `{}: {}`", target_str, bound_str))
                .emit();
        }

        self.ctx.analysis.active_bounds.truncate(prev_bounds_len);
        self.ctx.clear_active_bound_caches();
    }

    pub(super) fn push_impl_context_where_bounds(&mut self, impl_def: &ImplDef) -> usize {
        let prev_bounds_len = self.ctx.analysis.active_bounds.len();
        for clause in &impl_def.where_clauses {
            let target_ty = self.ctx.type_registry.normalize(
                self.ctx
                    .facts
                    .node_types
                    .get(&clause.target_ty.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR),
            );
            let bounds = clause
                .bounds
                .iter()
                .map(|bound| {
                    self.ctx.type_registry.normalize(
                        self.ctx
                            .facts
                            .node_types
                            .get(&bound.id)
                            .copied()
                            .unwrap_or(TypeId::ERROR),
                    )
                })
                .collect::<Vec<_>>();
            self.ctx.analysis.active_bounds.push((target_ty, bounds));
        }
        if self.ctx.analysis.active_bounds.len() != prev_bounds_len {
            self.ctx.clear_active_bound_caches();
        }
        prev_bounds_len
    }

    pub(super) fn bind_trait_assoc_types(&mut self, assoc_type_ids: &[DefId], scope: ScopeId) {
        for &assoc_id in assoc_type_ids {
            let Some(Def::AssociatedType(assoc_def)) =
                self.ctx.defs.get(assoc_id.0 as usize).cloned()
            else {
                continue;
            };
            let info = SymbolInfo {
                kind: SymbolKind::AssociatedType,
                node_id: self.ctx.next_node_id(),
                type_id: self
                    .ctx
                    .type_registry
                    .intern(TypeKind::Associated(assoc_id, vec![])),
                def_id: Some(assoc_id),
                span: assoc_def.span,
                vis: Visibility::Private,
                is_mut: false,
            };
            self.ctx.scopes.set_current_scope(scope);
            if let Err(old_info) = self.ctx.scopes.define(assoc_def.name, info) {
                self.emit_duplicate_assoc_type_definition(
                    assoc_def.name,
                    assoc_def.span,
                    old_info.span,
                );
            }
        }
    }

    pub(super) fn resolve_assoc_type_bounds(
        &mut self,
        assoc_type_ids: &[DefId],
        parent_scope: ScopeId,
    ) {
        for &assoc_id in assoc_type_ids {
            let Some(Def::AssociatedType(assoc_def)) =
                self.ctx.defs.get(assoc_id.0 as usize).cloned()
            else {
                continue;
            };
            self.ctx.scopes.set_current_scope(parent_scope);
            let assoc_scope = self.ctx.scopes.enter_scope();
            self.bind_generics(&assoc_def.generics, assoc_scope);
            self.resolve_where_clauses(&assoc_def.where_clauses, assoc_scope);
            let mut resolved_bounds = Vec::with_capacity(assoc_def.bounds.len());
            for bound in &assoc_def.bounds {
                resolved_bounds.push(self.resolve_type(bound, assoc_scope));
            }
            if let Some(target) = &assoc_def.target {
                self.resolve_type(target, assoc_scope);
            }
            self.ctx.scopes.exit_scope();
            if let Def::AssociatedType(updated) = &mut self.ctx.defs[assoc_id.0 as usize] {
                updated.resolved_bounds = resolved_bounds;
            }
        }
    }

    pub(super) fn bind_impl_assoc_types(
        &mut self,
        impl_def: &ImplDef,
        assoc_type_ids: &[DefId],
        resolved_trait_ty: Option<TypeId>,
        scope: ScopeId,
        span: Span,
    ) -> Option<TypeId> {
        let mut impl_assoc_by_name = HashMap::new();
        for &assoc_id in assoc_type_ids {
            let Some(Def::AssociatedType(assoc_def)) =
                self.ctx.defs.get(assoc_id.0 as usize).cloned()
            else {
                continue;
            };

            let info = SymbolInfo {
                kind: SymbolKind::AssociatedType,
                node_id: self.ctx.next_node_id(),
                type_id: self
                    .ctx
                    .type_registry
                    .intern(TypeKind::Associated(assoc_id, vec![])),
                def_id: Some(assoc_id),
                span: assoc_def.span,
                vis: Visibility::Private,
                is_mut: false,
            };
            self.ctx.scopes.set_current_scope(scope);
            if let Err(old_info) = self.ctx.scopes.define(assoc_def.name, info) {
                self.emit_duplicate_assoc_type_definition(
                    assoc_def.name,
                    assoc_def.span,
                    old_info.span,
                );
            }
            impl_assoc_by_name.entry(assoc_def.name).or_insert(assoc_id);
        }

        let Some(trait_ty) = resolved_trait_ty else {
            for &assoc_id in assoc_type_ids {
                if let Some(Def::AssociatedType(assoc_def)) = self.ctx.defs.get(assoc_id.0 as usize)
                {
                    self.ctx
                        .struct_error(
                            assoc_def.span,
                            "associated type definitions require a trait impl",
                        )
                        .with_hint(
                            "write `impl Type: Trait { ... }` when defining associated types",
                        )
                        .emit();
                }
            }
            return None;
        };

        let trait_norm = self.ctx.type_registry.normalize(trait_ty);
        let TypeKind::TraitObject(trait_def_id, trait_args, _) =
            self.ctx.type_registry.get(trait_norm).clone()
        else {
            self.ctx
                .emit_error(span, "impl trait target is not a trait");
            return Some(trait_ty);
        };

        let (trait_generics, trait_assoc_ids) = match self.ctx.defs.get(trait_def_id.0 as usize) {
            Some(Def::Trait(trait_def)) => {
                (trait_def.generics.clone(), trait_def.assoc_types.clone())
            }
            _ => (Vec::new(), Vec::new()),
        };
        let mut ordered_assoc_targets = vec![None; trait_assoc_ids.len()];

        let mut trait_assoc_names = HashMap::new();
        for (assoc_index, trait_assoc_id) in trait_assoc_ids.iter().copied().enumerate() {
            let Some(Def::AssociatedType(trait_assoc)) =
                self.ctx.defs.get(trait_assoc_id.0 as usize).cloned()
            else {
                continue;
            };
            trait_assoc_names
                .entry(trait_assoc.name)
                .or_insert(trait_assoc_id);

            let Some(&impl_assoc_id) = impl_assoc_by_name.get(&trait_assoc.name) else {
                let _ = assoc_index;
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "missing associated type definition `{}` in impl",
                            self.ctx.resolve(trait_assoc.name)
                        ),
                    )
                    .emit();
                continue;
            };

            let Some(Def::AssociatedType(impl_assoc)) =
                self.ctx.defs.get(impl_assoc_id.0 as usize).cloned()
            else {
                continue;
            };

            if trait_assoc.generics.len() != impl_assoc.generics.len() {
                self.ctx
                    .struct_error(
                        impl_assoc.span,
                        format!(
                            "associated type `{}` expects {} generic parameters, but impl provides {}",
                            self.ctx.resolve(trait_assoc.name),
                            trait_assoc.generics.len(),
                            impl_assoc.generics.len()
                        ),
                    )
                    .emit();
            }

            if let Def::AssociatedType(updated) = &mut self.ctx.defs[impl_assoc_id.0 as usize] {
                updated.parent_trait = Some(trait_def_id);
                updated.implemented_trait_assoc = Some(trait_assoc_id);
            }
        }

        for (&impl_assoc_name, &impl_assoc_id) in &impl_assoc_by_name {
            if !trait_assoc_names.contains_key(&impl_assoc_name)
                && let Some(Def::AssociatedType(impl_assoc)) =
                    self.ctx.defs.get(impl_assoc_id.0 as usize)
            {
                self.ctx
                    .struct_error(
                        impl_assoc.span,
                        format!(
                            "associated type `{}` is not declared by the target trait",
                            self.ctx.resolve(impl_assoc_name)
                        ),
                    )
                    .emit();
            }
        }

        let mut resolved_impl_assoc_targets = HashMap::new();
        for &assoc_id in assoc_type_ids {
            let Some(Def::AssociatedType(assoc_def)) =
                self.ctx.defs.get(assoc_id.0 as usize).cloned()
            else {
                continue;
            };
            self.ctx.scopes.set_current_scope(scope);
            let assoc_scope = self.ctx.scopes.enter_scope();
            let prev_suppress_unqualified_impl_assoc_types =
                self.suppress_unqualified_impl_assoc_types;
            self.suppress_unqualified_impl_assoc_types = true;
            self.bind_generics(&assoc_def.generics, assoc_scope);
            self.resolve_where_clauses(&assoc_def.where_clauses, assoc_scope);
            let mut resolved_bounds = Vec::with_capacity(assoc_def.bounds.len());
            for bound in &assoc_def.bounds {
                resolved_bounds.push(self.resolve_type(bound, assoc_scope));
            }
            let resolved_target = assoc_def
                .target
                .as_ref()
                .map(|target| self.resolve_type(target, assoc_scope));
            self.suppress_unqualified_impl_assoc_types = prev_suppress_unqualified_impl_assoc_types;
            self.ctx.scopes.exit_scope();
            if let Some(resolved_target) = resolved_target {
                self.ctx.scopes.set_current_scope(scope);
                self.ctx.scopes.update_type_in_namespace(
                    assoc_def.name,
                    SymbolNamespace::Type,
                    resolved_target,
                );
                resolved_impl_assoc_targets.insert(assoc_def.name, resolved_target);
            }
            if let Def::AssociatedType(updated) = &mut self.ctx.defs[assoc_id.0 as usize] {
                updated.resolved_bounds = resolved_bounds;
            }
        }

        let resolved_trait_assoc_targets = resolved_impl_assoc_targets
            .iter()
            .filter_map(|(&assoc_name, &resolved_target)| {
                trait_assoc_names
                    .get(&assoc_name)
                    .copied()
                    .map(|assoc_id| (assoc_id, resolved_target))
            })
            .collect::<HashMap<_, _>>();
        let canonical_trait_assoc_targets = trait_assoc_ids
            .iter()
            .copied()
            .filter_map(|trait_assoc_id| {
                let Def::AssociatedType(trait_assoc) =
                    self.ctx.defs.get(trait_assoc_id.0 as usize)?.clone()
                else {
                    return None;
                };
                let &resolved_target = resolved_impl_assoc_targets.get(&trait_assoc.name)?;
                let impl_assoc_id = impl_assoc_by_name.get(&trait_assoc.name).copied()?;
                let Def::AssociatedType(impl_assoc) =
                    self.ctx.defs.get(impl_assoc_id.0 as usize)?.clone()
                else {
                    return None;
                };
                Some((
                    trait_assoc_id,
                    self.canonicalize_impl_assoc_target_for_trait_binding(
                        &trait_assoc,
                        &impl_assoc,
                        resolved_target,
                    ),
                ))
            })
            .collect::<HashMap<_, _>>();

        let generic_args = trait_args
            .iter()
            .take(trait_generics.len())
            .copied()
            .collect::<Vec<_>>();
        let trait_assoc_ids = match self.ctx.defs.get(trait_def_id.0 as usize) {
            Some(Def::Trait(trait_def)) => trait_def.assoc_types.clone(),
            _ => Vec::new(),
        };
        for (assoc_index, trait_assoc_id) in trait_assoc_ids.iter().copied().enumerate() {
            let Some(Def::AssociatedType(trait_assoc)) =
                self.ctx.defs.get(trait_assoc_id.0 as usize).cloned()
            else {
                continue;
            };
            if let Some(&canonical_target) = canonical_trait_assoc_targets.get(&trait_assoc_id) {
                ordered_assoc_targets[assoc_index] = Some(canonical_target);
                if let Some(&impl_assoc_id) = impl_assoc_by_name.get(&trait_assoc.name)
                    && let Some(Def::AssociatedType(impl_assoc)) =
                        self.ctx.defs.get(impl_assoc_id.0 as usize).cloned()
                {
                    let resolved_target = resolved_impl_assoc_targets
                        .get(&trait_assoc.name)
                        .copied()
                        .unwrap_or(canonical_target);
                    self.check_impl_assoc_type_contracts(
                        impl_def,
                        ImplAssocTypeContract {
                            trait_def_id,
                            trait_generics: &trait_generics,
                            trait_args: &trait_args,
                            trait_assoc: &trait_assoc,
                            impl_assoc: &impl_assoc,
                            resolved_target,
                            assoc_targets: &resolved_trait_assoc_targets,
                        },
                    );
                }
            }
        }

        let assoc_bindings = trait_assoc_ids
            .iter()
            .copied()
            .zip(ordered_assoc_targets)
            .filter_map(|(assoc_id, target)| target.map(|ty| (assoc_id, ty)))
            .collect::<Vec<_>>();
        Some(self.ctx.type_registry.intern(TypeKind::TraitObject(
            trait_def_id,
            generic_args,
            assoc_bindings,
        )))
    }
}
