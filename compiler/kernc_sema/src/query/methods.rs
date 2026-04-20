use super::*;
use crate::ty::GenericArg;

impl<'a, 'ctx> MemberQuery<'a, 'ctx> {
    pub(super) fn collect_bound_method_candidates(
        &mut self,
        search_norm: TypeId,
        receiver_ty: TypeId,
        env: &MemberQueryEnv<'_>,
        candidates: &mut Vec<MemberCandidate>,
    ) {
        self.for_each_matching_bound_trait_object(search_norm, env, |this, bound_norm| {
            let trait_object = match this.ctx.type_registry.get(bound_norm) {
                TypeKind::TraitObject(trait_def_id, trait_args, assoc_bindings) => {
                    Some((*trait_def_id, trait_args.to_vec(), assoc_bindings.to_vec()))
                }
                _ => None,
            };
            if let Some((trait_def_id, trait_args, assoc_bindings)) = trait_object {
                this.collect_trait_object_method_candidates(
                    trait_def_id,
                    &trait_args,
                    &assoc_bindings,
                    receiver_ty,
                    candidates,
                );
            }
            false
        });
    }

    pub(super) fn resolve_bound_member(
        &mut self,
        search_norm: TypeId,
        receiver_ty: TypeId,
        member_name: SymbolId,
        env: &MemberQueryEnv<'_>,
        access_span: Span,
    ) -> Option<MemberResolution> {
        let mut resolution = None;
        self.for_each_matching_bound_trait_object(search_norm, env, |this, bound_norm| {
            if let Some(found) = this.resolve_trait_object_method_named(
                bound_norm,
                member_name,
                receiver_ty,
                Some(access_span),
            ) {
                resolution = Some(found);
                return true;
            }
            false
        });
        resolution
    }

    pub(super) fn for_each_matching_bound_trait_object(
        &mut self,
        search_norm: TypeId,
        env: &MemberQueryEnv<'_>,
        mut visit: impl FnMut(&mut Self, TypeId) -> bool,
    ) -> bool {
        if env.is_empty() {
            return false;
        }

        let current_bounds = self.ctx.active_bounds.as_slice();
        let can_use_cache = matches!(
            &env.active_bounds,
            Cow::Borrowed(bounds)
                if bounds.len() == current_bounds.len()
                    && std::ptr::eq(bounds.as_ptr(), current_bounds.as_ptr())
        );

        if can_use_cache
            && let Some(cached_matches) =
                self.ctx.bound_trait_match_cache.get(&search_norm).cloned()
        {
            for bound_norm in cached_matches {
                if visit(self, bound_norm) {
                    return true;
                }
            }
            return false;
        }

        let mut matched_bounds = if can_use_cache {
            Some(Vec::new())
        } else {
            None
        };
        for (env_target, bound_tys) in env.active_bounds() {
            if *env_target != search_norm {
                continue;
            }

            for bound_ty in bound_tys.iter().copied() {
                if matches!(
                    self.ctx.type_registry.get(bound_ty),
                    TypeKind::TraitObject(..)
                ) {
                    if let Some(bounds) = matched_bounds.as_mut() {
                        bounds.push(bound_ty);
                    }
                    if visit(self, bound_ty) {
                        if let Some(bounds) = matched_bounds {
                            self.ctx.bound_trait_match_cache.insert(search_norm, bounds);
                        }
                        return true;
                    }
                }
            }
        }

        if let Some(bounds) = matched_bounds {
            self.ctx.bound_trait_match_cache.insert(search_norm, bounds);
        }

        false
    }

    pub(super) fn collect_impl_method_candidates(
        &mut self,
        receiver_norm: TypeId,
        candidates: &mut Vec<MemberCandidate>,
    ) {
        let impl_count = self.ctx.global_impls.len();
        for impl_index in 0..impl_count {
            let impl_id = self.ctx.global_impls[impl_index];
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

            // Safety: queries do not mutate `defs`; avoid cloning every impl block just to inspect it.
            let impl_def = unsafe { &*impl_ptr };
            let Some(resolved_impl_args) = self.resolve_impl_applicability(receiver_norm, impl_id)
            else {
                continue;
            };

            for method_id in &impl_def.methods {
                let Def::Function(function) = &self.ctx.defs[method_id.0 as usize] else {
                    continue;
                };
                let type_id = self
                    .ctx
                    .type_registry
                    .intern(TypeKind::FnDef(*method_id, resolved_impl_args.clone()));
                push_member_candidate(
                    candidates,
                    MemberCandidate {
                        name: function.name,
                        kind: SymbolKind::Function,
                        type_id,
                        def_id: Some(*method_id),
                        definition_span: function.name_span,
                        is_mut: false,
                    },
                );
            }
        }
    }

    pub(super) fn resolve_named_impl_method(
        &mut self,
        receiver_norm: TypeId,
        member_name: SymbolId,
    ) -> Option<MemberCandidate> {
        let receiver_norm = self.ctx.type_registry.normalize(receiver_norm);
        let cache_key = (receiver_norm, member_name);
        if let Some(cached) = self.ctx.impl_method_query_cache.get(&cache_key).cloned() {
            return cached;
        }

        let method_ids_ptr = self
            .ctx
            .impl_methods_by_name
            .get(&member_name)
            .map(|method_ids| std::ptr::from_ref(method_ids.as_slice()))?;

        // Safety: method-name indexes are immutable during member lookup.
        let method_ids = unsafe { &*method_ids_ptr };
        let mut best_match: Option<(DefId, DefId, Span, Vec<GenericArg>)> = None;
        for &method_id in method_ids {
            let Some((impl_id, function_name_span)) = self
                .ctx
                .defs
                .get(method_id.0 as usize)
                .and_then(|def| match def {
                    Def::Function(function) => {
                        function.parent.map(|parent| (parent, function.name_span))
                    }
                    _ => None,
                })
            else {
                continue;
            };

            let Some(resolved_impl_args) = self.resolve_impl_applicability(receiver_norm, impl_id)
            else {
                continue;
            };

            let replace = match best_match.as_ref() {
                None => true,
                Some((best_impl_id, ..)) => matches!(
                    super::compare_impl_specificity(self.ctx, impl_id, *best_impl_id),
                    super::ImplSpecificity::LeftMoreSpecific
                ),
            };
            if replace {
                best_match = Some((impl_id, method_id, function_name_span, resolved_impl_args));
            }
        }

        if let Some((_, method_id, function_name_span, resolved_impl_args)) = best_match {
            let candidate = MemberCandidate {
                name: member_name,
                kind: SymbolKind::Function,
                type_id: self
                    .ctx
                    .type_registry
                    .intern(TypeKind::FnDef(method_id, resolved_impl_args)),
                def_id: Some(method_id),
                definition_span: function_name_span,
                is_mut: false,
            };
            self.ctx
                .impl_method_query_cache
                .insert(cache_key, Some(candidate.clone()));
            return Some(candidate);
        }

        self.ctx.impl_method_query_cache.insert(cache_key, None);
        None
    }

    pub(super) fn resolve_named_invalid_impl_method(
        &mut self,
        receiver_norm: TypeId,
        member_name: SymbolId,
    ) -> Option<MemberCandidate> {
        let method_ids_ptr = self
            .ctx
            .impl_methods_by_name
            .get(&member_name)
            .map(|method_ids| std::ptr::from_ref(method_ids.as_slice()))?;

        let method_ids = unsafe { &*method_ids_ptr };
        for &method_id in method_ids {
            let Some((impl_id, function_name_span)) = self
                .ctx
                .defs
                .get(method_id.0 as usize)
                .and_then(|def| match def {
                    Def::Function(function) => {
                        function.parent.map(|parent| (parent, function.name_span))
                    }
                    _ => None,
                })
            else {
                continue;
            };

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

            let impl_def = unsafe { &*impl_ptr };
            if self
                .ctx
                .direct_self_referential_impl_requirement(impl_def)
                .is_none()
                && self
                    .ctx
                    .indirect_self_referential_impl_requirement(impl_id)
                    .is_none()
            {
                continue;
            }

            let impl_target_ty = self
                .ctx
                .node_types
                .get(&impl_def.target_type.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let mut checker = ExprChecker::new(self.ctx, None);
            let mut type_map = FastHashMap::default();
            let mut const_map = FastHashMap::default();
            if !checker.match_available_type_against_requirement(
                impl_target_ty,
                receiver_norm,
                &mut type_map,
                &mut const_map,
            ) {
                continue;
            }

            return Some(MemberCandidate {
                name: member_name,
                kind: SymbolKind::Function,
                type_id: TypeId::ERROR,
                def_id: Some(method_id),
                definition_span: function_name_span,
                is_mut: false,
            });
        }

        None
    }

    pub(super) fn resolve_impl_applicability(
        &mut self,
        receiver_norm: TypeId,
        impl_id: DefId,
    ) -> Option<Vec<crate::ty::GenericArg>> {
        let cache_key = (receiver_norm, impl_id);
        if let Some(cached) = self.ctx.impl_applicability_cache.get(&cache_key).cloned() {
            return cached;
        }

        let resolved_args = {
            let mut checker = ExprChecker::new(self.ctx, None);
            let Some(impl_ptr) =
                checker
                    .ctx
                    .defs
                    .get(impl_id.0 as usize)
                    .and_then(|def| match def {
                        Def::Impl(impl_def) => Some(std::ptr::from_ref(impl_def)),
                        _ => None,
                    })
            else {
                checker.ctx.impl_applicability_cache.insert(cache_key, None);
                return None;
            };

            let impl_def = unsafe { &*impl_ptr };
            let impl_target_ty = checker
                .ctx
                .node_types
                .get(&impl_def.target_type.id)
                .copied()
                .unwrap_or(TypeId::ERROR);

            if checker
                .ctx
                .direct_self_referential_impl_requirement(impl_def)
                .is_some()
                || checker
                    .ctx
                    .indirect_self_referential_impl_requirement(impl_id)
                    .is_some()
                || checker
                    .ctx
                    .non_decreasing_impl_requirement(impl_id)
                    .is_some()
            {
                None
            } else if impl_def.generics.is_empty() && impl_def.where_clauses.is_empty() {
                if checker.ctx.type_registry.normalize(impl_target_ty) == receiver_norm {
                    Some(Vec::new())
                } else {
                    None
                }
            } else {
                let mut type_map = FastHashMap::default();
                let mut const_map = FastHashMap::default();
                if !checker.match_available_type_against_requirement(
                    impl_target_ty,
                    receiver_norm,
                    &mut type_map,
                    &mut const_map,
                ) || !impl_bounds_satisfied(
                    &mut checker,
                    &impl_def.where_clauses,
                    &type_map,
                    &const_map,
                ) {
                    None
                } else {
                    Some(
                        impl_def
                            .generics
                            .iter()
                            .map(|param| match &param.kind {
                                ast::GenericParamKind::Type => GenericArg::Type(
                                    type_map.get(&param.name).copied().unwrap_or(TypeId::ERROR),
                                ),
                                ast::GenericParamKind::Const { .. } => GenericArg::Const(
                                    const_map
                                        .get(&param.name)
                                        .copied()
                                        .unwrap_or(crate::ty::ConstGeneric::Error),
                                ),
                            })
                            .collect::<Vec<_>>(),
                    )
                }
            }
        };

        self.ctx
            .impl_applicability_cache
            .insert(cache_key, resolved_args.clone());
        resolved_args
    }

    pub(super) fn collect_trait_object_method_candidates(
        &mut self,
        trait_def_id: DefId,
        trait_args: &[crate::ty::GenericArg],
        assoc_bindings: &[(DefId, TypeId)],
        receiver_ty: TypeId,
        candidates: &mut Vec<MemberCandidate>,
    ) {
        let mut visited = FastHashSet::default();
        self.collect_trait_methods_in_hierarchy(
            trait_def_id,
            trait_args,
            assoc_bindings,
            receiver_ty,
            &mut visited,
            candidates,
        );
    }

    pub(super) fn resolve_trait_object_method_named(
        &mut self,
        trait_object_ty: TypeId,
        member_name: SymbolId,
        receiver_ty: TypeId,
        diagnostic_span: Option<Span>,
    ) -> Option<MemberResolution> {
        let trait_object_ty = self.ctx.type_registry.normalize(trait_object_ty);
        let trait_object = match self.ctx.type_registry.get(trait_object_ty) {
            TypeKind::TraitObject(trait_def_id, trait_args, assoc_bindings) => {
                Some((*trait_def_id, trait_args.to_vec(), assoc_bindings.to_vec()))
            }
            _ => None,
        };
        let Some((trait_def_id, trait_args, assoc_bindings)) = trait_object else {
            return None;
        };

        let cache_key = (trait_object_ty, member_name, receiver_ty);
        if let Some(cached) = self.ctx.trait_method_query_cache.get(&cache_key).cloned() {
            return Some(cached);
        }

        let mut visited = FastHashSet::default();
        let resolution = self.resolve_trait_method_in_hierarchy(
            trait_def_id,
            TraitMethodLookup {
                trait_args: &trait_args,
                assoc_bindings: &assoc_bindings,
                member_name,
                receiver_ty,
                diagnostic_span,
            },
            &mut visited,
        );
        if let Some(resolution) = resolution.clone() {
            self.ctx
                .trait_method_query_cache
                .insert(cache_key, resolution);
        }
        resolution
    }

    pub(super) fn collect_trait_methods_in_hierarchy(
        &mut self,
        trait_def_id: DefId,
        trait_args: &[crate::ty::GenericArg],
        assoc_bindings: &[(DefId, TypeId)],
        receiver_ty: TypeId,
        visited: &mut FastHashSet<TypeId>,
        candidates: &mut Vec<MemberCandidate>,
    ) {
        let trait_view = self.ctx.type_registry.intern(TypeKind::TraitObject(
            trait_def_id,
            trait_args.to_vec(),
            assoc_bindings.to_vec(),
        ));
        let trait_view = self.ctx.type_registry.normalize(trait_view);
        if !visited.insert(trait_view) {
            return;
        }

        let Some(trait_ptr) =
            self.ctx
                .defs
                .get(trait_def_id.0 as usize)
                .and_then(|def| match def {
                    Def::Trait(trait_def) => Some(std::ptr::from_ref(trait_def)),
                    _ => None,
                })
        else {
            return;
        };
        // Safety: trait definitions are immutable during semantic member queries.
        let trait_def = unsafe { &*trait_ptr };
        let trait_arg_map = if trait_def.generics.is_empty() || trait_args.is_empty() {
            None
        } else {
            Some(
                trait_def
                    .generics
                    .iter()
                    .zip(trait_args.iter())
                    .map(|(param, arg)| (param.name, *arg))
                    .collect::<FastHashMap<_, _>>(),
            )
        };

        for (method_name, method_ty) in &trait_def.resolved_methods {
            let mut method_ty = *method_ty;
            if let TypeKind::Function {
                params,
                ret,
                is_variadic,
            } = self.ctx.type_registry.get(method_ty)
            {
                let mut new_params = params.clone();
                if !new_params.is_empty() {
                    new_params[0] = receiver_ty;
                }
                method_ty = self.ctx.type_registry.intern(TypeKind::Function {
                    params: new_params,
                    ret: *ret,
                    is_variadic: *is_variadic,
                });
            }

            if let Some(trait_arg_map) = trait_arg_map.as_ref() {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, trait_arg_map);
                method_ty = subst.substitute(method_ty);
            }
            method_ty = self.materialize_trait_assoc_placeholders(
                method_ty,
                receiver_ty,
                trait_def_id,
                trait_args,
                assoc_bindings,
            );

            push_member_candidate(
                candidates,
                MemberCandidate {
                    name: *method_name,
                    kind: SymbolKind::Function,
                    type_id: method_ty,
                    def_id: None,
                    definition_span: trait_method_span(trait_def, *method_name),
                    is_mut: false,
                },
            );
        }

        let assoc_binding_map = assoc_bindings
            .iter()
            .copied()
            .collect::<FastHashMap<_, _>>();

        for &super_ty in &trait_def.resolved_supertraits {
            let inst_super_ty = if let Some(trait_arg_map) = trait_arg_map.as_ref() {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, trait_arg_map);
                let substituted = subst.substitute(super_ty);
                crate::checker::substitute_associated_types(
                    &mut self.ctx.type_registry,
                    substituted,
                    &assoc_binding_map,
                )
            } else if assoc_binding_map.is_empty() {
                super_ty
            } else {
                crate::checker::substitute_associated_types(
                    &mut self.ctx.type_registry,
                    super_ty,
                    &assoc_binding_map,
                )
            };
            let inst_super_norm = crate::query::augment_trait_object_assoc_bindings_from_map(
                self.ctx,
                inst_super_ty,
                &assoc_binding_map,
            );

            let super_trait = match self.ctx.type_registry.get(inst_super_norm) {
                TypeKind::TraitObject(super_def_id, super_args, super_assoc_bindings) => Some((
                    *super_def_id,
                    super_args.to_vec(),
                    super_assoc_bindings.to_vec(),
                )),
                _ => None,
            };
            if let Some((super_def_id, super_args, super_assoc_bindings)) = super_trait {
                self.collect_trait_methods_in_hierarchy(
                    super_def_id,
                    &super_args,
                    &super_assoc_bindings,
                    receiver_ty,
                    visited,
                    candidates,
                );
            }
        }
    }

    pub(super) fn resolve_trait_method_in_hierarchy(
        &mut self,
        trait_def_id: DefId,
        lookup: TraitMethodLookup<'_>,
        visited: &mut FastHashSet<TypeId>,
    ) -> Option<MemberResolution> {
        let TraitMethodLookup {
            trait_args,
            assoc_bindings,
            member_name,
            receiver_ty,
            diagnostic_span,
        } = lookup;
        let trait_view = self.ctx.type_registry.intern(TypeKind::TraitObject(
            trait_def_id,
            trait_args.to_vec(),
            assoc_bindings.to_vec(),
        ));
        let trait_view = self.ctx.type_registry.normalize(trait_view);
        if !visited.insert(trait_view) {
            return None;
        }

        let trait_ptr = self
            .ctx
            .defs
            .get(trait_def_id.0 as usize)
            .and_then(|def| match def {
                Def::Trait(trait_def) => Some(std::ptr::from_ref(trait_def)),
                _ => None,
            })?;
        // Safety: trait definitions are immutable during member resolution.
        let trait_def = unsafe { &*trait_ptr };
        let trait_arg_map = if trait_def.generics.is_empty() || trait_args.is_empty() {
            None
        } else {
            Some(
                trait_def
                    .generics
                    .iter()
                    .zip(trait_args.iter())
                    .map(|(param, arg)| (param.name, *arg))
                    .collect::<FastHashMap<_, _>>(),
            )
        };

        let assoc_binding_map = assoc_bindings
            .iter()
            .copied()
            .collect::<FastHashMap<_, _>>();

        if let Some((_, method_ty)) = trait_def
            .resolved_methods
            .iter()
            .find(|(name, _)| *name == member_name)
        {
            let mut method_ty = *method_ty;
            if let TypeKind::Function {
                params,
                ret,
                is_variadic,
            } = self.ctx.type_registry.get(method_ty)
            {
                let mut new_params = params.clone();
                if !new_params.is_empty() {
                    new_params[0] = receiver_ty;
                }
                method_ty = self.ctx.type_registry.intern(TypeKind::Function {
                    params: new_params,
                    ret: *ret,
                    is_variadic: *is_variadic,
                });
            }

            if let Some(trait_arg_map) = trait_arg_map.as_ref() {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, trait_arg_map);
                method_ty = subst.substitute(method_ty);
            }
            method_ty = self.materialize_trait_assoc_placeholders(
                method_ty,
                receiver_ty,
                trait_def_id,
                trait_args,
                assoc_bindings,
            );

            return Some(MemberResolution {
                candidate: MemberCandidate {
                    name: member_name,
                    kind: SymbolKind::Function,
                    type_id: method_ty,
                    def_id: None,
                    definition_span: trait_method_span(trait_def, member_name),
                    is_mut: false,
                },
                owner_trait_ty: Some(self.ctx.type_registry.intern(TypeKind::TraitObject(
                    trait_def_id,
                    trait_args.to_vec(),
                    assoc_bindings.to_vec(),
                ))),
            });
        }

        let mut matches = Vec::new();
        for &super_ty in &trait_def.resolved_supertraits {
            let inst_super_ty = if let Some(trait_arg_map) = trait_arg_map.as_ref() {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, trait_arg_map);
                let substituted = subst.substitute(super_ty);
                crate::checker::substitute_associated_types(
                    &mut self.ctx.type_registry,
                    substituted,
                    &assoc_binding_map,
                )
            } else if assoc_binding_map.is_empty() {
                super_ty
            } else {
                crate::checker::substitute_associated_types(
                    &mut self.ctx.type_registry,
                    super_ty,
                    &assoc_binding_map,
                )
            };
            let inst_super_norm = crate::query::augment_trait_object_assoc_bindings_from_map(
                self.ctx,
                inst_super_ty,
                &assoc_binding_map,
            );

            let super_trait = match self.ctx.type_registry.get(inst_super_norm) {
                TypeKind::TraitObject(super_def_id, super_args, super_assoc_bindings) => Some((
                    *super_def_id,
                    super_args.to_vec(),
                    super_assoc_bindings.to_vec(),
                )),
                _ => None,
            };
            if let Some((super_def_id, super_args, super_assoc_bindings)) = super_trait
                && let Some(resolution) = self.resolve_trait_method_in_hierarchy(
                    super_def_id,
                    TraitMethodLookup {
                        trait_args: &super_args,
                        assoc_bindings: &super_assoc_bindings,
                        member_name,
                        receiver_ty,
                        diagnostic_span,
                    },
                    visited,
                )
            {
                matches.push(resolution);
            }
        }

        if matches.len() > 1 {
            if let Some(span) = diagnostic_span {
                let owners = matches
                    .iter()
                    .filter_map(|resolution| resolution.owner_trait_ty)
                    .map(|owner| self.ctx.ty_to_string(owner))
                    .collect::<Vec<_>>();
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "ambiguous inherited trait method `{}`",
                            self.ctx.resolve(member_name)
                        ),
                    )
                    .with_hint(format!(
                        "the method is inherited from multiple parent traits: {}",
                        owners.join(", ")
                    ))
                    .emit();
            }
            return None;
        }

        matches.into_iter().next()
    }
}
