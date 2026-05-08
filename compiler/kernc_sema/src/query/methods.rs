use super::*;
use crate::ty::GenericArg;

#[derive(Debug, Clone)]
struct ApplicableImplMethodCandidate {
    impl_id: DefId,
    method_id: DefId,
    method_span: Span,
    impl_args: Vec<GenericArg>,
}

#[derive(Clone, Copy)]
enum BuiltinOperatorMethodShape {
    Binary(&'static str),
    UnaryPrefix(&'static str),
}

impl<'a, 'ctx> MemberQuery<'a, 'ctx> {
    fn same_inherited_method_resolution(
        &self,
        left: &MemberResolution,
        right: &MemberResolution,
    ) -> bool {
        left.candidate.name == right.candidate.name
            && left.candidate.kind == right.candidate.kind
            && left.candidate.def_id == right.candidate.def_id
            && left.candidate.definition_span == right.candidate.definition_span
            && left.candidate.is_mut == right.candidate.is_mut
            && self.ctx.type_registry.normalize(left.candidate.type_id)
                == self.ctx.type_registry.normalize(right.candidate.type_id)
            && left
                .owner_trait_ty
                .map(|ty| self.ctx.type_registry.normalize(ty))
                == right
                    .owner_trait_ty
                    .map(|ty| self.ctx.type_registry.normalize(ty))
    }

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

    pub(super) fn resolve_projection_assoc_bound_method(
        &mut self,
        search_norm: TypeId,
        receiver_ty: TypeId,
        member_name: SymbolId,
        access_span: Span,
    ) -> Option<MemberResolution> {
        let TypeKind::Projection {
            target,
            trait_def_id,
            trait_args,
            assoc_def_id,
            assoc_args,
        } = self.ctx.type_registry.get(search_norm).clone()
        else {
            return None;
        };

        let owner_trait_ty = self.ctx.type_registry.intern(TypeKind::TraitObject(
            trait_def_id,
            trait_args.clone(),
            Vec::new(),
        ));
        if !ExprChecker::new(self.ctx, None).check_trait_impl(target, owner_trait_ty) {
            return None;
        }

        let Some(Def::Trait(trait_def)) = self.ctx.defs.get(trait_def_id.0 as usize).cloned()
        else {
            return None;
        };
        let Some(Def::AssociatedType(assoc_def)) =
            self.ctx.defs.get(assoc_def_id.0 as usize).cloned()
        else {
            return None;
        };
        if assoc_def.parent_trait != Some(trait_def_id) {
            return None;
        }

        let mut subst_map = FastHashMap::default();
        for (param, arg) in trait_def.generics.iter().zip(trait_args.iter().copied()) {
            subst_map.insert(param.name, arg);
        }
        for (param, arg) in assoc_def.generics.iter().zip(assoc_args.iter().copied()) {
            subst_map.insert(param.name, arg);
        }

        for bound_ty in assoc_def.resolved_bounds {
            let bound_ty = if subst_map.is_empty() {
                bound_ty
            } else {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
                subst.substitute(bound_ty)
            };
            let bound_ty = self.ctx.type_registry.normalize(bound_ty);
            if !matches!(
                self.ctx.type_registry.get(bound_ty),
                TypeKind::TraitObject(..)
            ) {
                continue;
            }
            if let Some(resolution) = self.resolve_trait_object_method_named(
                bound_ty,
                member_name,
                receiver_ty,
                Some(access_span),
            ) {
                return Some(resolution);
            }
        }

        None
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

        let can_use_cache = env.is_current_active_bounds(self.ctx);

        if can_use_cache
            && let Some(cached_matches) = self
                .ctx
                .analysis
                .query_caches
                .bound_trait_match_cache
                .get(&search_norm)
                .cloned()
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
        let matched_trait_bounds =
            crate::query::instantiated_env_trait_bounds(self.ctx, search_norm, env.active_bounds());

        for bound_norm in matched_trait_bounds {
            if let Some(bounds) = matched_bounds.as_mut() {
                bounds.push(bound_norm);
            }
            if visit(self, bound_norm) {
                if let Some(bounds) = matched_bounds {
                    self.ctx
                        .analysis
                        .query_caches
                        .bound_trait_match_cache
                        .insert(search_norm, bounds);
                }
                return true;
            }
        }

        if let Some(bounds) = matched_bounds {
            self.ctx
                .analysis
                .query_caches
                .bound_trait_match_cache
                .insert(search_norm, bounds);
        }

        false
    }

    pub(super) fn collect_impl_method_candidates(
        &mut self,
        receiver_norm: TypeId,
        candidates: &mut Vec<MemberCandidate>,
    ) {
        let impl_count = self.ctx.impl_index.global_impls.len();
        for impl_index in 0..impl_count {
            let impl_id = self.ctx.impl_index.global_impls[impl_index];
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
        diagnostic_span: Option<Span>,
    ) -> Option<MemberCandidate> {
        let receiver_norm = self.ctx.type_registry.normalize(receiver_norm);
        let cache_key = (receiver_norm, member_name);
        if let Some(cached) = self
            .ctx
            .analysis
            .query_caches
            .impl_method_query_cache
            .get(&cache_key)
            .cloned()
        {
            return cached;
        }

        let candidates =
            self.collect_specificity_maximal_impl_method_candidates(receiver_norm, member_name)?;
        if candidates.len() > 1 {
            if let Some(span) = diagnostic_span {
                self.emit_ambiguous_impl_method_diagnostic(
                    span,
                    receiver_norm,
                    member_name,
                    &candidates,
                );
            }

            let first = &candidates[0];
            return Some(MemberCandidate {
                name: member_name,
                kind: SymbolKind::Function,
                type_id: TypeId::ERROR,
                def_id: Some(first.method_id),
                definition_span: first.method_span,
                is_mut: false,
            });
        }

        if let Some(ApplicableImplMethodCandidate {
            method_id,
            method_span,
            impl_args,
            ..
        }) = candidates.into_iter().next()
        {
            let candidate = MemberCandidate {
                name: member_name,
                kind: SymbolKind::Function,
                type_id: self
                    .ctx
                    .type_registry
                    .intern(TypeKind::FnDef(method_id, impl_args)),
                def_id: Some(method_id),
                definition_span: method_span,
                is_mut: false,
            };
            if candidate.type_id != TypeId::ERROR {
                self.ctx
                    .analysis
                    .query_caches
                    .impl_method_query_cache
                    .insert(cache_key, Some(candidate.clone()));
            }
            return Some(candidate);
        }

        self.ctx
            .analysis
            .query_caches
            .impl_method_query_cache
            .insert(cache_key, None);
        None
    }

    fn collect_specificity_maximal_impl_method_candidates(
        &mut self,
        receiver_norm: TypeId,
        member_name: SymbolId,
    ) -> Option<Vec<ApplicableImplMethodCandidate>> {
        let method_ids_ptr = self
            .ctx
            .impl_index
            .impl_methods_by_name
            .get(&member_name)
            .map(|method_ids| std::ptr::from_ref(method_ids.as_slice()))?;

        // Safety: method-name indexes are immutable during member lookup.
        let method_ids = unsafe { &*method_ids_ptr };
        let mut applicable = Vec::new();
        for &method_id in method_ids {
            let Some((impl_id, method_span)) =
                self.ctx
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

            let Some(impl_args) = self.resolve_impl_applicability(receiver_norm, impl_id) else {
                continue;
            };

            applicable.push(ApplicableImplMethodCandidate {
                impl_id,
                method_id,
                method_span,
                impl_args,
            });
        }

        if applicable.is_empty() {
            return None;
        }

        Some(
            applicable
                .iter()
                .enumerate()
                .filter(|(index, candidate)| {
                    !applicable.iter().enumerate().any(|(other_index, other)| {
                        other_index != *index
                            && matches!(
                                super::compare_impl_specificity(
                                    self.ctx,
                                    other.impl_id,
                                    candidate.impl_id,
                                ),
                                super::ImplSpecificity::LeftMoreSpecific
                            )
                    })
                })
                .map(|(_, candidate)| candidate.clone())
                .collect(),
        )
    }

    fn emit_ambiguous_impl_method_diagnostic(
        &mut self,
        span: Span,
        receiver_ty: TypeId,
        member_name: SymbolId,
        candidates: &[ApplicableImplMethodCandidate],
    ) {
        let member_name_str = self.ctx.resolve(member_name).to_string();
        let receiver_ty_str = self.ctx.ty_to_string(receiver_ty);
        let impl_heads = candidates
            .iter()
            .filter_map(|candidate| self.describe_impl_head(candidate.impl_id))
            .collect::<Vec<_>>();
        let candidate_spans = candidates
            .iter()
            .take(2)
            .filter_map(
                |candidate| match self.ctx.defs.get(candidate.impl_id.0 as usize) {
                    Some(Def::Impl(impl_def)) => Some(impl_def.span),
                    _ => None,
                },
            )
            .collect::<Vec<_>>();
        let builtin_operator_hints = self.builtin_operator_method_hints(member_name, candidates);
        let mut diagnostic = self
            .ctx
            .struct_error(span, format!("ambiguous impl method `{}`", member_name_str));
        diagnostic = diagnostic.with_hint(format!(
            "multiple equally specific impl methods named `{}` apply to receiver type `{}`",
            member_name_str, receiver_ty_str
        ));
        diagnostic = diagnostic.with_hint(
            "remove the overlap or make one impl head strictly more specific so method lookup has a unique result",
        );
        diagnostic = diagnostic.with_hint(
            "Kern method lookup resolves by receiver type and method name before argument typing; same-name methods on the same receiver do not overload by parameter type",
        );
        diagnostic = diagnostic.with_hint(
            "if these methods were meant to express different operations, prefer distinct names or an operator/helper with explicit dispatch",
        );
        if let Some(hints) = builtin_operator_hints {
            for hint in hints {
                diagnostic = diagnostic.with_hint(hint);
            }
        }
        if !impl_heads.is_empty() {
            diagnostic =
                diagnostic.with_hint(format!("conflicting impl heads: {}", impl_heads.join(", ")));
        }
        for candidate_span in candidate_spans {
            diagnostic = diagnostic
                .with_span_label(candidate_span, "equally specific impl method applies here");
        }
        diagnostic.emit();
    }

    fn builtin_operator_method_hints(
        &mut self,
        member_name: SymbolId,
        candidates: &[ApplicableImplMethodCandidate],
    ) -> Option<[String; 2]> {
        let trait_name = self.common_builtin_trait_name(candidates)?;
        let member_name_str = self.ctx.resolve(member_name);
        let shape = builtin_operator_method_shape(&trait_name, member_name_str)?;

        let syntax_hint = match shape {
            BuiltinOperatorMethodShape::Binary(op) => format!(
                "if you meant builtin `{}`, write `lhs {} rhs` instead of `lhs.{}(rhs)`",
                trait_name, op, member_name_str
            ),
            BuiltinOperatorMethodShape::UnaryPrefix(op) => format!(
                "if you meant builtin `{}`, write `{}value` instead of `value.{}()`",
                trait_name, op, member_name_str
            ),
        };

        Some([
            "builtin operator trait methods still use ordinary member lookup; they do not gain parameter-based overload resolution".to_string(),
            syntax_hint,
        ])
    }

    fn common_builtin_trait_name(
        &mut self,
        candidates: &[ApplicableImplMethodCandidate],
    ) -> Option<String> {
        let mut common = None;

        for candidate in candidates {
            let Def::Impl(impl_def) = self.ctx.defs.get(candidate.impl_id.0 as usize)? else {
                return None;
            };
            let trait_ty_node = impl_def.trait_type.as_ref()?;
            let trait_ty = self.ctx.normalized_node_type_or_error(trait_ty_node.id);
            let TypeKind::TraitObject(trait_def_id, ..) =
                self.ctx.type_registry.get(trait_ty).clone()
            else {
                return None;
            };
            let Def::Trait(trait_def) = self.ctx.defs.get(trait_def_id.0 as usize)? else {
                return None;
            };
            if !trait_def.is_builtin {
                return None;
            }
            let trait_name = self.ctx.resolve(trait_def.name).to_string();
            match common {
                Some(ref existing) if existing != &trait_name => return None,
                Some(_) => {}
                None => common = Some(trait_name),
            }
        }

        common
    }

    fn describe_impl_head(&mut self, impl_id: DefId) -> Option<String> {
        let Def::Impl(impl_def) = self.ctx.defs.get(impl_id.0 as usize)?.clone() else {
            return None;
        };
        let target_ty = self
            .ctx
            .normalized_node_type_or_error(impl_def.target_type.id);
        if target_ty == TypeId::ERROR {
            return None;
        }

        match impl_def.trait_type {
            Some(trait_ty_node) => {
                let trait_ty = self.ctx.normalized_node_type_or_error(trait_ty_node.id);
                if trait_ty == TypeId::ERROR {
                    None
                } else {
                    Some(format!(
                        "`{}: {}`",
                        self.ctx.ty_to_string(target_ty),
                        self.ctx.ty_to_string(trait_ty)
                    ))
                }
            }
            None => Some(format!("`impl {}`", self.ctx.ty_to_string(target_ty))),
        }
    }

    pub(super) fn resolve_named_invalid_impl_method(
        &mut self,
        receiver_norm: TypeId,
        member_name: SymbolId,
    ) -> Option<MemberCandidate> {
        let method_ids_ptr = self
            .ctx
            .impl_index
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

            let impl_target_ty = self.ctx.node_type_or_error(impl_def.target_type.id);
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
        if let Some(cached) = self
            .ctx
            .analysis
            .query_caches
            .impl_applicability_cache
            .get(&cache_key)
            .cloned()
        {
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
                checker
                    .ctx
                    .analysis
                    .query_caches
                    .impl_applicability_cache
                    .insert(cache_key, None);
                return None;
            };

            let impl_def = unsafe { &*impl_ptr };
            let impl_target_ty = checker.ctx.node_type_or_error(impl_def.target_type.id);

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
                    let resolved_args = impl_def
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
                        .collect::<Vec<_>>();

                    // Method lookup must reject impls whose local generics were never solved from
                    // the receiver match. Otherwise `impl[T] X: Trait` would incorrectly expose
                    // `Trait` methods on `X` even though no concrete proof exists.
                    if crate::query::impl_generic_args_fully_resolved(&resolved_args) {
                        Some(resolved_args)
                    } else {
                        None
                    }
                }
            }
        };

        self.ctx
            .analysis
            .query_caches
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
        let (trait_def_id, trait_args, assoc_bindings) = trait_object?;

        let cache_key = (trait_object_ty, member_name, receiver_ty);
        if let Some(cached) = self
            .ctx
            .analysis
            .query_caches
            .trait_method_query_cache
            .get(&cache_key)
            .cloned()
        {
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
                .analysis
                .query_caches
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
                    &self.ctx.defs,
                    substituted,
                    &assoc_binding_map,
                )
            } else if assoc_binding_map.is_empty() {
                super_ty
            } else {
                crate::checker::substitute_associated_types(
                    &mut self.ctx.type_registry,
                    &self.ctx.defs,
                    super_ty,
                    &assoc_binding_map,
                )
            };
            let inst_super_norm = crate::query::augment_trait_object_assoc_bindings_from_map(
                self.ctx,
                inst_super_ty,
                &assoc_binding_map,
            );
            let inst_super_norm = self.ctx.normalize_concrete_type(inst_super_norm);
            let inst_super_norm = self.ctx.type_registry.normalize(inst_super_norm);

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

            let owner_trait_ty = self.ctx.type_registry.intern(TypeKind::TraitObject(
                trait_def_id,
                trait_args.to_vec(),
                assoc_bindings.to_vec(),
            ));
            let owner_trait_ty =
                crate::query::retain_declared_trait_object_assoc_bindings(self.ctx, owner_trait_ty);

            return Some(MemberResolution {
                candidate: MemberCandidate {
                    name: member_name,
                    kind: SymbolKind::Function,
                    type_id: method_ty,
                    def_id: None,
                    definition_span: trait_method_span(trait_def, member_name),
                    is_mut: false,
                },
                owner_trait_ty: Some(owner_trait_ty),
            });
        }

        let mut matches = Vec::new();
        for &super_ty in &trait_def.resolved_supertraits {
            let inst_super_ty = if let Some(trait_arg_map) = trait_arg_map.as_ref() {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, trait_arg_map);
                let substituted = subst.substitute(super_ty);
                crate::checker::substitute_associated_types(
                    &mut self.ctx.type_registry,
                    &self.ctx.defs,
                    substituted,
                    &assoc_binding_map,
                )
            } else if assoc_binding_map.is_empty() {
                super_ty
            } else {
                crate::checker::substitute_associated_types(
                    &mut self.ctx.type_registry,
                    &self.ctx.defs,
                    super_ty,
                    &assoc_binding_map,
                )
            };
            let inst_super_norm = crate::query::augment_trait_object_assoc_bindings_from_map(
                self.ctx,
                inst_super_ty,
                &assoc_binding_map,
            );
            let inst_super_norm = self.ctx.normalize_concrete_type(inst_super_norm);
            let inst_super_norm = self.ctx.type_registry.normalize(inst_super_norm);

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

        let mut unique_matches = Vec::new();
        for resolution in matches {
            if unique_matches
                .iter()
                .any(|existing| self.same_inherited_method_resolution(existing, &resolution))
            {
                continue;
            }
            unique_matches.push(resolution);
        }
        let matches = unique_matches;

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

fn builtin_operator_method_shape(
    trait_name: &str,
    method_name: &str,
) -> Option<BuiltinOperatorMethodShape> {
    match (trait_name, method_name) {
        ("Eq", "eq") => Some(BuiltinOperatorMethodShape::Binary("==")),
        ("Lt", "lt") => Some(BuiltinOperatorMethodShape::Binary("<")),
        ("Le", "le") => Some(BuiltinOperatorMethodShape::Binary("<=")),
        ("Gt", "gt") => Some(BuiltinOperatorMethodShape::Binary(">")),
        ("Ge", "ge") => Some(BuiltinOperatorMethodShape::Binary(">=")),
        ("Add", "add") => Some(BuiltinOperatorMethodShape::Binary("+")),
        ("Sub", "sub") => Some(BuiltinOperatorMethodShape::Binary("-")),
        ("Mul", "mul") => Some(BuiltinOperatorMethodShape::Binary("*")),
        ("Div", "div") => Some(BuiltinOperatorMethodShape::Binary("/")),
        ("Rem", "rem") => Some(BuiltinOperatorMethodShape::Binary("%")),
        ("BitAnd", "bit_and") => Some(BuiltinOperatorMethodShape::Binary("&")),
        ("BitOr", "bit_or") => Some(BuiltinOperatorMethodShape::Binary("|")),
        ("BitXor", "bit_xor") => Some(BuiltinOperatorMethodShape::Binary("^")),
        ("Shl", "shl") => Some(BuiltinOperatorMethodShape::Binary("<<")),
        ("Shr", "shr") => Some(BuiltinOperatorMethodShape::Binary(">>")),
        ("Neg", "neg") => Some(BuiltinOperatorMethodShape::UnaryPrefix("-")),
        ("Not", "not") => Some(BuiltinOperatorMethodShape::UnaryPrefix("!")),
        ("BitNot", "bit_not") => Some(BuiltinOperatorMethodShape::UnaryPrefix("~")),
        _ => None,
    }
}
