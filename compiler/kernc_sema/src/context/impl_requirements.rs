use super::*;

#[derive(Clone)]
struct ApplicableImplRequirementCandidate {
    impl_id: DefId,
    impl_def: ImplDef,
    head_map: FastHashMap<SymbolId, crate::ty::GenericArg>,
}

impl<'a> SemaContext<'a> {
    pub(crate) fn direct_self_referential_impl_requirement(
        &self,
        impl_def: &ImplDef,
    ) -> Option<SelfReferentialImplRequirement> {
        let Some(trait_ty_node) = &impl_def.trait_type else {
            return None;
        };

        let impl_target_ty = self.normalized_node_type_or_error(impl_def.target_type.id);
        let impl_trait_ty = self.normalized_node_type_or_error(trait_ty_node.id);
        if impl_target_ty == TypeId::ERROR || impl_trait_ty == TypeId::ERROR {
            return None;
        }

        for clause in &impl_def.where_clauses {
            let clause_target_ty = self.normalized_node_type_or_error(clause.target_ty.id);
            if clause_target_ty != impl_target_ty {
                continue;
            }

            for bound in &clause.bounds {
                let bound_ty = self.normalized_node_type_or_error(bound.id);
                if self.trait_obligation_matches_impl_head(bound_ty, impl_trait_ty) {
                    return Some(SelfReferentialImplRequirement {
                        bound_span: bound.span,
                        target_ty: impl_target_ty,
                        trait_ty: impl_trait_ty,
                    });
                }
            }
        }

        None
    }

    fn trait_obligation_matches_impl_head(&self, bound_ty: TypeId, impl_trait_ty: TypeId) -> bool {
        let bound_norm = self.type_registry.normalize(bound_ty);
        let impl_norm = self.type_registry.normalize(impl_trait_ty);
        match (
            self.type_registry.get(bound_norm),
            self.type_registry.get(impl_norm),
        ) {
            (
                crate::ty::TypeKind::TraitObject(bound_def_id, bound_args, bound_assoc_bindings),
                crate::ty::TypeKind::TraitObject(impl_def_id, impl_args, impl_assoc_bindings),
            ) => {
                if bound_def_id != impl_def_id || bound_args != impl_args {
                    return false;
                }

                // Impl-head recursion checks should treat omitted associated-type bindings as
                // compatible, because callers may compare a bare trait head against a richer
                // obligation assembled from supertrait traversal. Conflicting bindings, however,
                // must keep the obligations distinct or we misreport impossible proofs as
                // self-recursive.
                let bound_assoc_bindings = bound_assoc_bindings
                    .iter()
                    .copied()
                    .collect::<FastHashMap<_, _>>();
                let impl_assoc_bindings = impl_assoc_bindings
                    .iter()
                    .copied()
                    .collect::<FastHashMap<_, _>>();
                bound_assoc_bindings
                    .iter()
                    .all(|(assoc_def_id, bound_assoc_ty)| {
                        impl_assoc_bindings
                            .get(assoc_def_id)
                            .is_none_or(|impl_assoc_ty| {
                                self.type_registry.normalize(*bound_assoc_ty)
                                    == self.type_registry.normalize(*impl_assoc_ty)
                            })
                    })
                    && impl_assoc_bindings
                        .iter()
                        .all(|(assoc_def_id, impl_assoc_ty)| {
                            bound_assoc_bindings
                                .get(assoc_def_id)
                                .is_none_or(|bound_assoc_ty| {
                                    self.type_registry.normalize(*impl_assoc_ty)
                                        == self.type_registry.normalize(*bound_assoc_ty)
                                })
                        })
            }
            _ => bound_norm == impl_norm,
        }
    }

    pub(crate) fn indirect_self_referential_impl_requirement(
        &mut self,
        impl_id: DefId,
    ) -> Option<ImplRequirementCycle> {
        // Cache the final cycle verdict per impl so later method/proof queries can cheaply reject
        // the same bad proof source without re-walking the obligation graph.
        if let Some(cached) = self
            .analysis
            .query_caches
            .impl_requirement_cycle_cache
            .get(&impl_id)
            .cloned()
        {
            return cached;
        }

        let cycle = self.compute_indirect_impl_requirement_cycle(impl_id);
        self.analysis
            .query_caches
            .impl_requirement_cycle_cache
            .insert(impl_id, cycle.clone());
        cycle
    }

    fn compute_indirect_impl_requirement_cycle(
        &mut self,
        impl_id: DefId,
    ) -> Option<ImplRequirementCycle> {
        let Some(Def::Impl(impl_def)) = self.defs.get(impl_id.0 as usize).cloned() else {
            return None;
        };
        if !self.impl_can_participate_in_cycle_search(impl_id, &impl_def) {
            return None;
        }

        let Some(trait_ty_node) = &impl_def.trait_type else {
            return None;
        };

        let start_target_ty = self.normalized_node_type_or_error(impl_def.target_type.id);
        let start_trait_ty = self.normalized_node_type_or_error(trait_ty_node.id);
        if start_target_ty == TypeId::ERROR || start_trait_ty == TypeId::ERROR {
            return None;
        }

        let initial_requirements = self.instantiated_impl_requirements_with_supertraits(
            &impl_def,
            &FastHashMap::<SymbolId, crate::ty::GenericArg>::default(),
        );
        // The stack tracks the current obligation path so we only report cycles that can
        // re-derive the original impl head, instead of every repeated obligation seen while
        // exploring unrelated branches.
        //
        // head: A: Trait
        //   requires B: Trait2
        //     requires C: Trait3
        //       requires A: Trait   <- report this path
        let mut obligation_stack = vec![(start_target_ty, start_trait_ty)];
        for requirement in initial_requirements {
            if self.obligation_matches_impl_head(
                requirement.target_ty,
                requirement.trait_ty,
                start_target_ty,
                start_trait_ty,
            ) {
                continue;
            }

            let mut path = vec![requirement];
            if self.find_impl_requirement_cycle_path(
                requirement.target_ty,
                requirement.trait_ty,
                start_target_ty,
                start_trait_ty,
                &mut obligation_stack,
                &mut path,
            ) {
                return Some(ImplRequirementCycle {
                    start_bound_span: requirement.requirement_span,
                    target_ty: start_target_ty,
                    trait_ty: start_trait_ty,
                    requirements: path,
                });
            }
        }

        None
    }

    fn find_impl_requirement_cycle_path(
        &mut self,
        source_ty: TypeId,
        target_trait_ty: TypeId,
        start_target_ty: TypeId,
        start_trait_ty: TypeId,
        obligation_stack: &mut Vec<(TypeId, TypeId)>,
        path: &mut Vec<ImplRequirementEdge>,
    ) -> bool {
        let source_ty = self.type_registry.normalize(source_ty);
        let target_trait_ty = self.type_registry.normalize(target_trait_ty);
        if source_ty == TypeId::ERROR || target_trait_ty == TypeId::ERROR {
            return false;
        }

        let obligation = (source_ty, target_trait_ty);
        if obligation_stack.contains(&obligation) {
            return false;
        }
        obligation_stack.push(obligation);

        let applicable_impls =
            self.collect_specificity_maximal_cycle_candidates(source_ty, target_trait_ty);
        for candidate in applicable_impls {
            // Follow the same specialization frontier as real proof search. If a more specific
            // impl already satisfies this obligation, a shadowed generic impl must not be allowed
            // to manufacture a fake cycle for the current start impl.
            let requirements = self.instantiated_impl_requirements_with_supertraits(
                &candidate.impl_def,
                &candidate.head_map,
            );
            for requirement in requirements {
                path.push(requirement);
                if self.obligation_matches_impl_head(
                    requirement.target_ty,
                    requirement.trait_ty,
                    start_target_ty,
                    start_trait_ty,
                ) {
                    obligation_stack.pop();
                    return true;
                }

                if self.find_impl_requirement_cycle_path(
                    requirement.target_ty,
                    requirement.trait_ty,
                    start_target_ty,
                    start_trait_ty,
                    obligation_stack,
                    path,
                ) {
                    obligation_stack.pop();
                    return true;
                }
                path.pop();
            }
        }

        let popped = obligation_stack.pop();
        debug_assert_eq!(popped, Some(obligation));
        false
    }

    fn collect_specificity_maximal_cycle_candidates(
        &mut self,
        source_ty: TypeId,
        target_trait_ty: TypeId,
    ) -> Vec<ApplicableImplRequirementCandidate> {
        // Snapshot the impl list before the recursive walk so later queries may borrow `self`
        // without fighting an outstanding borrow of the index.
        let target_trait_head_ty = crate::query::erase_trait_assoc_bindings(self, target_trait_ty);
        let TypeKind::TraitObject(target_trait_def_id, _, _) = self
            .type_registry
            .get(self.type_registry.normalize(target_trait_head_ty))
            .clone()
        else {
            return Vec::new();
        };
        let trait_impl_ids = self.trait_impl_ids_for_trait(target_trait_def_id);
        let mut applicable = Vec::new();

        for candidate_impl_id in trait_impl_ids {
            {
                let mut resolver = TypeResolver::new(self);
                resolver.ensure_impl_signature_types_resolved(candidate_impl_id);
            }

            let Some(Def::Impl(candidate_impl)) =
                self.defs.get(candidate_impl_id.0 as usize).cloned()
            else {
                continue;
            };
            let Some(candidate_trait_ast) = &candidate_impl.trait_type else {
                continue;
            };
            if !self.impl_can_participate_in_cycle_search(candidate_impl_id, &candidate_impl) {
                continue;
            }

            let impl_target_ty = self.node_type_or_error(candidate_impl.target_type.id);
            let impl_trait_ty = self.node_type_or_error(candidate_trait_ast.id);
            if impl_target_ty == TypeId::ERROR || impl_trait_ty == TypeId::ERROR {
                continue;
            }

            let mut head_type_map = FastHashMap::default();
            let mut head_const_map = FastHashMap::default();
            let matches_obligation = {
                let mut checker = ExprChecker::new(self, None);
                let impl_trait_head_ty =
                    crate::query::erase_trait_assoc_bindings(checker.ctx, impl_trait_ty);
                let target_trait_head_ty =
                    crate::query::erase_trait_assoc_bindings(checker.ctx, target_trait_ty);
                // Match the receiver and trait through one shared substitution map. Splitting this
                // into staged maps re-introduces the old "same-named outer generic polluted the
                // impl-local generic" bug because the second phase can accidentally treat an
                // obligation-side `T` as if it belonged to the candidate impl head.
                checker.match_available_type_against_requirement(
                    impl_target_ty,
                    source_ty,
                    &mut head_type_map,
                    &mut head_const_map,
                ) && checker.match_available_type_against_requirement(
                    impl_trait_head_ty,
                    target_trait_head_ty,
                    &mut head_type_map,
                    &mut head_const_map,
                ) && checker.match_available_type_against_requirement(
                    impl_trait_ty,
                    target_trait_ty,
                    &mut head_type_map,
                    &mut head_const_map,
                )
            };
            if !matches_obligation {
                continue;
            }
            let head_map = candidate_impl
                .generics
                .iter()
                .filter_map(|param| match &param.kind {
                    kernc_ast::GenericParamKind::Type => head_type_map
                        .get(&param.name)
                        .copied()
                        .map(crate::ty::GenericArg::Type)
                        .map(|arg| (param.name, arg)),
                    kernc_ast::GenericParamKind::Const { .. } => head_const_map
                        .get(&param.name)
                        .copied()
                        .map(crate::ty::GenericArg::Const)
                        .map(|arg| (param.name, arg)),
                })
                .collect::<FastHashMap<_, _>>();
            // Cycle search should only follow proof edges whose impl head has been fully
            // determined by the current obligation. Otherwise an impl-local generic can leak into
            // later requirement expansion and fabricate a path that no concrete proof could ever
            // traverse.
            if head_map.len() != candidate_impl.generics.len() {
                continue;
            }
            applicable.push(ApplicableImplRequirementCandidate {
                impl_id: candidate_impl_id,
                impl_def: candidate_impl,
                head_map,
            });
        }

        // Keep every undominated candidate. In coherent code this is usually a single impl; if
        // overlap validation has not run yet and multiple incomparable heads survive, we still
        // explore them all rather than accidentally pretending the ambiguity does not exist.
        applicable
            .iter()
            .enumerate()
            .filter(|(index, candidate)| {
                !applicable.iter().enumerate().any(|(other_index, other)| {
                    other_index != *index
                        && matches!(
                            crate::query::compare_impl_specificity(
                                self,
                                other.impl_id,
                                candidate.impl_id,
                            ),
                            crate::query::ImplSpecificity::LeftMoreSpecific
                        )
                })
            })
            .map(|(_, candidate)| candidate.clone())
            .collect()
    }

    fn impl_can_participate_in_cycle_search(&mut self, impl_id: DefId, impl_def: &ImplDef) -> bool {
        // Indirect cycle search is only meaningful over proof edges that are themselves
        // admissible. If we let already-rejected impls participate here, a bad impl can
        // manufacture bogus cycle diagnostics on otherwise unrelated impls.
        //
        // Deliberately only filter impls rejected for local reasons here (direct self-recursion
        // or Paterson boundedness). We do not try to pre-eliminate impls that are themselves
        // indirectly cyclic, because members of the same recursive SCC must still be traversable
        // while we are diagnosing the current start impl's own cycle.
        self.direct_self_referential_impl_requirement(impl_def)
            .is_none()
            && self.non_decreasing_impl_requirement(impl_id).is_none()
    }

    fn instantiated_impl_requirements(
        &mut self,
        impl_def: &ImplDef,
        map: &FastHashMap<SymbolId, crate::ty::GenericArg>,
    ) -> Vec<ImplRequirementEdge> {
        self.instantiated_impl_requirements_inner(impl_def, map, false)
    }

    fn instantiated_impl_requirements_with_supertraits(
        &mut self,
        impl_def: &ImplDef,
        map: &FastHashMap<SymbolId, crate::ty::GenericArg>,
    ) -> Vec<ImplRequirementEdge> {
        self.instantiated_impl_requirements_inner(impl_def, map, true)
    }

    fn instantiated_impl_requirements_inner(
        &mut self,
        impl_def: &ImplDef,
        map: &FastHashMap<SymbolId, crate::ty::GenericArg>,
        include_supertraits: bool,
    ) -> Vec<ImplRequirementEdge> {
        let mut requirements = Vec::new();
        let instantiated_impl_target = {
            // Cycle search and Paterson checks operate on the concrete obligation graph induced by
            // the currently matched impl head, not the generic skeleton written on the impl.
            let original_target = self.node_type_or_error(impl_def.target_type.id);
            let substituted_target = {
                let mut subst = Substituter::new(&mut self.type_registry, map);
                subst.substitute(original_target)
            };
            self.type_registry.normalize(substituted_target)
        };

        for clause in &impl_def.where_clauses {
            let target_ty = {
                let original_target = self.node_type_or_error(clause.target_ty.id);
                let substituted_target = {
                    let mut subst = Substituter::new(&mut self.type_registry, map);
                    subst.substitute(original_target)
                };
                self.type_registry.normalize(substituted_target)
            };

            for bound in &clause.bounds {
                let trait_ty = {
                    let original_bound = self.node_type_or_error(bound.id);
                    let substituted_bound = {
                        let mut subst = Substituter::new(&mut self.type_registry, map);
                        subst.substitute(original_bound)
                    };
                    self.type_registry.normalize(substituted_bound)
                };
                if !matches!(self.type_registry.get(trait_ty), TypeKind::TraitObject(..)) {
                    continue;
                }
                // Each where-clause contributes one proof edge:
                // `impl Head where Target: Bound` means proving `Head` may recurse into
                // proving `Target: Bound`.
                requirements.push(ImplRequirementEdge {
                    impl_id: impl_def.id,
                    requirement_span: bound.span,
                    target_ty,
                    trait_ty,
                });
            }
        }

        if !include_supertraits {
            return requirements;
        }

        let Some(trait_ty_node) = &impl_def.trait_type else {
            return requirements;
        };
        let instantiated_trait_ty = {
            let original_trait_ty = self.node_type_or_error(trait_ty_node.id);
            let substituted_trait_ty = {
                let mut subst = Substituter::new(&mut self.type_registry, map);
                subst.substitute(original_trait_ty)
            };
            self.type_registry.normalize(substituted_trait_ty)
        };
        let TypeKind::TraitObject(trait_def_id, trait_args, assoc_bindings) =
            self.type_registry.get(instantiated_trait_ty).clone()
        else {
            return requirements;
        };
        let Some(Def::Trait(trait_def)) = self.defs.get(trait_def_id.0 as usize).cloned() else {
            return requirements;
        };

        let trait_arg_map = trait_def
            .generics
            .iter()
            .zip(trait_args.iter())
            .map(|(param, arg)| (param.name, *arg))
            .collect::<FastHashMap<_, _>>();
        let assoc_binding_map = assoc_bindings.into_iter().collect::<FastHashMap<_, _>>();

        for (supertrait_index, &supertrait_ty) in trait_def.resolved_supertraits.iter().enumerate()
        {
            // Supertrait obligations inherit both the impl's concrete trait arguments and any
            // associated-type equalities already fixed on the head. The augment/retain pair keeps
            // inherited bindings available long enough for further traversal while still emitting
            // an obligation in the surface form users wrote for that supertrait.
            let substituted_supertrait = if trait_arg_map.is_empty() {
                supertrait_ty
            } else {
                let mut subst = Substituter::new(&mut self.type_registry, &trait_arg_map);
                subst.substitute(supertrait_ty)
            };
            let substituted_supertrait = crate::checker::substitute_associated_types(
                &mut self.type_registry,
                &self.defs,
                substituted_supertrait,
                &assoc_binding_map,
            );
            let substituted_supertrait = crate::query::augment_trait_object_assoc_bindings_from_map(
                self,
                substituted_supertrait,
                &assoc_binding_map,
            );
            let substituted_supertrait = crate::query::retain_declared_trait_object_assoc_bindings(
                self,
                substituted_supertrait,
            );
            let substituted_supertrait = self.type_registry.normalize(substituted_supertrait);
            if !matches!(
                self.type_registry.get(substituted_supertrait),
                TypeKind::TraitObject(..)
            ) {
                continue;
            }

            requirements.push(ImplRequirementEdge {
                impl_id: impl_def.id,
                requirement_span: trait_def
                    .supertraits
                    .get(supertrait_index)
                    .map(|supertrait| supertrait.span)
                    .unwrap_or(impl_def.span),
                target_ty: instantiated_impl_target,
                trait_ty: substituted_supertrait,
            });
        }

        requirements
    }

    fn obligation_matches_impl_head(
        &self,
        target_ty: TypeId,
        trait_ty: TypeId,
        impl_target_ty: TypeId,
        impl_trait_ty: TypeId,
    ) -> bool {
        self.type_registry.normalize(target_ty) == self.type_registry.normalize(impl_target_ty)
            && self.trait_obligation_matches_impl_head(trait_ty, impl_trait_ty)
    }

    pub(crate) fn non_decreasing_impl_requirement(
        &mut self,
        impl_id: DefId,
    ) -> Option<NonDecreasingImplRequirement> {
        if let Some(cached) = self
            .analysis
            .query_caches
            .impl_paterson_boundedness_cache
            .get(&impl_id)
            .cloned()
        {
            return cached;
        }

        let violation = self.compute_non_decreasing_impl_requirement(impl_id);
        self.analysis
            .query_caches
            .impl_paterson_boundedness_cache
            .insert(impl_id, violation.clone());
        violation
    }

    fn compute_non_decreasing_impl_requirement(
        &mut self,
        impl_id: DefId,
    ) -> Option<NonDecreasingImplRequirement> {
        let Some(Def::Impl(impl_def)) = self.defs.get(impl_id.0 as usize).cloned() else {
            return None;
        };
        let Some(trait_ty_node) = &impl_def.trait_type else {
            return None;
        };

        let head_target_ty = self.normalized_node_type_or_error(impl_def.target_type.id);
        let head_trait_ty = self.normalized_node_type_or_error(trait_ty_node.id);
        if head_target_ty == TypeId::ERROR || head_trait_ty == TypeId::ERROR {
            return None;
        }

        for requirement in self.instantiated_impl_requirements(
            &impl_def,
            &FastHashMap::<SymbolId, crate::ty::GenericArg>::default(),
        ) {
            let Some(issue) = self.compare_paterson_obligations(
                Some(head_target_ty),
                head_trait_ty,
                Some(requirement.target_ty),
                requirement.trait_ty,
            ) else {
                continue;
            };

            return Some(NonDecreasingImplRequirement {
                bound_span: requirement.requirement_span,
                head_target_ty,
                head_trait_ty,
                requirement_target_ty: requirement.target_ty,
                requirement_trait_ty: requirement.trait_ty,
                issue,
            });
        }

        None
    }

    pub(crate) fn compare_paterson_obligations(
        &self,
        head_target_ty: Option<TypeId>,
        head_trait_ty: TypeId,
        requirement_target_ty: Option<TypeId>,
        requirement_trait_ty: TypeId,
    ) -> Option<PatersonBoundednessIssue> {
        // If the requirement asks for a proof on a strictly smaller receiver, repeated references
        // to head parameters in the trait payload do not make the proof grow: the solver must first
        // move from the outer receiver to one of its pieces. This admits bounds such as
        // `impl[T] Box[T]: Iter where T: Add[T, Out = T]` without giving special treatment to
        // builtin operator traits.
        if let (Some(head_target_ty), Some(requirement_target_ty)) =
            (head_target_ty, requirement_target_ty)
        {
            let head_target = self.paterson_type_measure(head_target_ty);
            let requirement_target = self.paterson_type_measure(requirement_target_ty);
            if self.paterson_measure_strictly_decreases(&requirement_target, &head_target) {
                let payload = self.paterson_trait_payload_measure(requirement_trait_ty);
                if let Some(issue) = self.compare_decreased_receiver_payload(&head_target, &payload)
                {
                    return Some(issue);
                }
                return None;
            }
        }

        // Without receiver descent, Paterson-style boundedness rejects requirements that introduce
        // either more type constructors or more occurrences of any parameter than the impl head
        // already has.
        let head = self.paterson_obligation_measure(head_target_ty, head_trait_ty);
        let requirement =
            self.paterson_obligation_measure(requirement_target_ty, requirement_trait_ty);

        for (&param, &requirement_count) in &requirement.params {
            let head_count = head.params.get(&param).copied().unwrap_or(0);
            if requirement_count > head_count {
                return Some(PatersonBoundednessIssue::VariableCount {
                    param,
                    head: head_count,
                    requirement: requirement_count,
                });
            }
        }

        if requirement.constructors > head.constructors {
            return Some(PatersonBoundednessIssue::ConstructorCount {
                head: head.constructors,
                requirement: requirement.constructors,
            });
        }

        None
    }

    fn paterson_type_measure(&self, ty: TypeId) -> PatersonMeasure {
        let mut measure = PatersonMeasure::default();
        self.measure_paterson_type(ty, &mut measure);
        measure
    }

    fn paterson_trait_payload_measure(&self, trait_ty: TypeId) -> PatersonMeasure {
        let mut measure = PatersonMeasure::default();
        self.measure_paterson_trait_payload(trait_ty, &mut measure);
        measure
    }

    fn paterson_measure_strictly_decreases(
        &self,
        requirement: &PatersonMeasure,
        head: &PatersonMeasure,
    ) -> bool {
        if requirement.constructors > head.constructors {
            return false;
        }

        let mut strictly_smaller = requirement.constructors < head.constructors;
        for (&param, &requirement_count) in &requirement.params {
            let head_count = head.params.get(&param).copied().unwrap_or(0);
            if requirement_count > head_count {
                return false;
            }
            strictly_smaller |= requirement_count < head_count;
        }

        for (&param, &head_count) in &head.params {
            if head_count > 0 && !requirement.params.contains_key(&param) {
                strictly_smaller = true;
            }
        }

        strictly_smaller
    }

    fn compare_decreased_receiver_payload(
        &self,
        head_target: &PatersonMeasure,
        payload: &PatersonMeasure,
    ) -> Option<PatersonBoundednessIssue> {
        for (&param, &requirement_count) in &payload.params {
            let head_count = head_target.params.get(&param).copied().unwrap_or(0);
            if head_count == 0 {
                return Some(PatersonBoundednessIssue::VariableCount {
                    param,
                    head: head_count,
                    requirement: requirement_count,
                });
            }
        }

        if payload.constructors > head_target.constructors {
            return Some(PatersonBoundednessIssue::ConstructorCount {
                head: head_target.constructors,
                requirement: payload.constructors,
            });
        }

        None
    }

    pub(crate) fn compare_paterson_supertrait_against_generics(
        &self,
        head_generics: &[kernc_ast::GenericParam],
        requirement_trait_ty: TypeId,
    ) -> Option<PatersonBoundednessIssue> {
        // Trait declarations do not have a concrete receiver head yet, so the admissibility check
        // for `trait Child: Parent[...]` compares the supertrait solely against the parameter
        // budget exposed by the trait's own generic list.
        let head = self.paterson_generics_measure(head_generics);
        let requirement = self.paterson_obligation_measure(None, requirement_trait_ty);

        for (&param, &requirement_count) in &requirement.params {
            let head_count = head.params.get(&param).copied().unwrap_or(0);
            if requirement_count > head_count {
                return Some(PatersonBoundednessIssue::VariableCount {
                    param,
                    head: head_count,
                    requirement: requirement_count,
                });
            }
        }

        if requirement.constructors > head.constructors {
            return Some(PatersonBoundednessIssue::ConstructorCount {
                head: head.constructors,
                requirement: requirement.constructors,
            });
        }

        None
    }

    fn paterson_generics_measure(&self, generics: &[kernc_ast::GenericParam]) -> PatersonMeasure {
        let mut measure = PatersonMeasure::default();
        for generic in generics {
            match generic.kind {
                kernc_ast::GenericParamKind::Type => {
                    *measure
                        .params
                        .entry(PatersonParam::Type(generic.name))
                        .or_insert(0) += 1;
                }
                kernc_ast::GenericParamKind::Const { .. } => {
                    *measure
                        .params
                        .entry(PatersonParam::Const(generic.name))
                        .or_insert(0) += 1;
                }
            }
        }
        measure
    }

    fn paterson_obligation_measure(
        &self,
        target_ty: Option<TypeId>,
        trait_ty: TypeId,
    ) -> PatersonMeasure {
        // Trait obligations count both the receiver side and the trait payload, because recursive
        // growth in either position can make proof search diverge.
        let mut measure = PatersonMeasure::default();
        if let Some(target_ty) = target_ty {
            self.measure_paterson_type(target_ty, &mut measure);
        }
        self.measure_paterson_trait_payload(trait_ty, &mut measure);
        measure
    }

    fn measure_paterson_trait_payload(&self, trait_ty: TypeId, measure: &mut PatersonMeasure) {
        let trait_norm = self.type_registry.normalize(trait_ty);
        match self.type_registry.get(trait_norm) {
            TypeKind::TraitObject(_, args, assoc_bindings) => {
                for &arg in args {
                    self.measure_paterson_generic_arg(arg, measure);
                }
                for &(_, assoc_ty) in assoc_bindings {
                    self.measure_paterson_type(assoc_ty, measure);
                }
            }
            _ => self.measure_paterson_type(trait_norm, measure),
        }
    }

    fn measure_paterson_generic_arg(&self, arg: GenericArg, measure: &mut PatersonMeasure) {
        match arg {
            GenericArg::Type(ty) => self.measure_paterson_type(ty, measure),
            GenericArg::Const(value) => self.measure_paterson_const_generic(value, measure),
        }
    }

    fn measure_paterson_const_generic(
        &self,
        value: crate::ty::ConstGeneric,
        measure: &mut PatersonMeasure,
    ) {
        match value {
            crate::ty::ConstGeneric::Value(_) => {
                measure.constructors += 1;
            }
            crate::ty::ConstGeneric::Param(name, _) => {
                *measure
                    .params
                    .entry(PatersonParam::Const(name))
                    .or_insert(0) += 1;
            }
            crate::ty::ConstGeneric::Expr(expr_id) => {
                // Count the expression node itself as structure, then recurse into operands so
                // duplicated params such as `N + N` still grow the measure relative to bare `N`.
                measure.constructors += 1;
                match *self.type_registry.const_expr(expr_id) {
                    crate::ty::ConstExprKind::Unary { expr, .. }
                    | crate::ty::ConstExprKind::Cast { expr, .. } => {
                        self.measure_paterson_const_generic(expr, measure);
                    }
                    crate::ty::ConstExprKind::Binary { lhs, rhs, .. } => {
                        self.measure_paterson_const_generic(lhs, measure);
                        self.measure_paterson_const_generic(rhs, measure);
                    }
                }
            }
            crate::ty::ConstGeneric::Error => {}
        }
    }

    fn measure_paterson_type(&self, ty: TypeId, measure: &mut PatersonMeasure) {
        let norm = self.type_registry.normalize(ty);
        match self.type_registry.get(norm) {
            TypeKind::Error => return,
            TypeKind::Param(name) => {
                *measure
                    .params
                    .entry(PatersonParam::Type(*name))
                    .or_insert(0) += 1;
                return;
            }
            TypeKind::Alias(..) => unreachable!("aliases are removed by normalize"),
            _ => {
                // Every concrete constructor contributes one unit of structural size; recursive
                // arguments are counted separately below.
                measure.constructors += 1;
            }
        }

        match self.type_registry.get(norm) {
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::ArrayInfer { elem, .. }
            | TypeKind::AnonymousEnumPayload(elem)
            | TypeKind::Simd { elem, .. } => {
                self.measure_paterson_type(*elem, measure);
            }
            TypeKind::Array { elem, len, .. } => {
                self.measure_paterson_type(*elem, measure);
                self.measure_paterson_const_generic(*len, measure);
            }
            TypeKind::Def(_, args)
            | TypeKind::Enum(_, args)
            | TypeKind::EnumPayload(_, args)
            | TypeKind::FnDef(_, args)
            | TypeKind::Associated(_, args) => {
                for &arg in args {
                    self.measure_paterson_generic_arg(arg, measure);
                }
            }
            TypeKind::TraitObject(_, args, assoc_bindings) => {
                // Associated-type equalities are part of the proof obligation's size budget too;
                // otherwise `T: Trait[Assoc = Wrap[T]]` could sneak in growth through bindings
                // while keeping the nominal trait head unchanged.
                for &arg in args {
                    self.measure_paterson_generic_arg(arg, measure);
                }
                for &(_, assoc_ty) in assoc_bindings {
                    self.measure_paterson_type(assoc_ty, measure);
                }
            }
            TypeKind::Projection {
                target,
                trait_args,
                assoc_args,
                ..
            } => {
                self.measure_paterson_type(*target, measure);
                for &arg in trait_args {
                    self.measure_paterson_generic_arg(arg, measure);
                }
                for &arg in assoc_args {
                    self.measure_paterson_generic_arg(arg, measure);
                }
            }
            TypeKind::Function { params, ret, .. } | TypeKind::ClosureInterface { params, ret } => {
                for &param in params {
                    self.measure_paterson_type(param, measure);
                }
                self.measure_paterson_type(*ret, measure);
            }
            TypeKind::AnonymousState {
                captures,
                params,
                ret,
                ..
            } => {
                for &capture in captures {
                    self.measure_paterson_type(capture, measure);
                }
                for &param in params {
                    self.measure_paterson_type(param, measure);
                }
                self.measure_paterson_type(*ret, measure);
            }
            TypeKind::AnonymousStruct(_, fields) | TypeKind::AnonymousUnion(_, fields) => {
                for field in fields {
                    self.measure_paterson_type(field.ty, measure);
                }
            }
            TypeKind::AnonymousEnum(enum_def) => {
                if let Some(backing_ty) = enum_def.backing_ty {
                    self.measure_paterson_type(backing_ty, measure);
                }
                for variant in &enum_def.variants {
                    if let Some(payload_ty) = variant.payload_ty {
                        self.measure_paterson_type(payload_ty, measure);
                    }
                }
            }
            TypeKind::Primitive(_)
            | TypeKind::Module(_)
            | TypeKind::TypeVar(_)
            | TypeKind::Param(_)
            | TypeKind::Error
            | TypeKind::Alias(..) => {}
        }
    }

    pub(crate) fn describe_paterson_issue(&self, issue: &PatersonBoundednessIssue) -> String {
        match issue {
            PatersonBoundednessIssue::ConstructorCount { head, requirement } => format!(
                "structural constructor count grows from {} in the head to {} in the prerequisite",
                head, requirement
            ),
            PatersonBoundednessIssue::VariableCount {
                param,
                head,
                requirement,
            } => format!(
                "`{}` occurs {} time(s) in the head but {} time(s) in the prerequisite",
                self.paterson_param_name(*param),
                head,
                requirement
            ),
        }
    }

    pub(crate) fn describe_paterson_issue_brief(&self, issue: &PatersonBoundednessIssue) -> String {
        match issue {
            PatersonBoundednessIssue::ConstructorCount { .. } => {
                "this prerequisite is structurally larger than the impl head".to_string()
            }
            PatersonBoundednessIssue::VariableCount {
                param,
                head,
                requirement,
            } => format!(
                "`{}` is used {} time(s) here, but only {} time(s) in the impl head",
                self.paterson_param_name(*param),
                requirement,
                head
            ),
        }
    }

    fn paterson_param_name(&self, param: PatersonParam) -> String {
        match param {
            PatersonParam::Type(name) | PatersonParam::Const(name) => {
                self.resolve(name).to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def::{AssociatedTypeDef, TraitDef};
    use kernc_ast::Visibility;
    use kernc_utils::Session;

    #[test]
    fn impl_head_match_rejects_conflicting_assoc_bindings() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_id, assoc_id) = add_trait_with_assoc(&mut ctx, "TypeIs", "Is");

        let impl_trait_ty = ctx.type_registry.intern(TypeKind::TraitObject(
            trait_id,
            vec![],
            vec![(assoc_id, TypeId::I32)],
        ));
        let conflicting_requirement_ty = ctx.type_registry.intern(TypeKind::TraitObject(
            trait_id,
            vec![],
            vec![(assoc_id, TypeId::BOOL)],
        ));

        assert!(!ctx.trait_obligation_matches_impl_head(conflicting_requirement_ty, impl_trait_ty));
    }

    #[test]
    fn impl_head_match_accepts_missing_assoc_bindings() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_id, assoc_id) = add_trait_with_assoc(&mut ctx, "TypeIs", "Is");

        let bare_trait_ty =
            ctx.type_registry
                .intern(TypeKind::TraitObject(trait_id, vec![], Vec::new()));
        let enriched_trait_ty = ctx.type_registry.intern(TypeKind::TraitObject(
            trait_id,
            vec![],
            vec![(assoc_id, TypeId::I32)],
        ));

        assert!(ctx.trait_obligation_matches_impl_head(bare_trait_ty, enriched_trait_ty));
        assert!(ctx.trait_obligation_matches_impl_head(enriched_trait_ty, bare_trait_ty));
    }

    #[test]
    fn paterson_measure_counts_assoc_binding_growth() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_id, assoc_id) = add_trait_with_assoc(&mut ctx, "TypeIs", "Is");
        let param_t = ctx.intern("T");
        let param_ty = ctx.type_registry.intern(TypeKind::Param(param_t));
        let wrapped_param_ty = ctx.type_registry.intern(TypeKind::Def(
            DefId(700),
            vec![crate::ty::GenericArg::Type(param_ty)],
        ));

        let head_trait_ty = ctx.type_registry.intern(TypeKind::TraitObject(
            trait_id,
            vec![],
            vec![(assoc_id, param_ty)],
        ));
        let requirement_trait_ty = ctx.type_registry.intern(TypeKind::TraitObject(
            trait_id,
            vec![],
            vec![(assoc_id, wrapped_param_ty)],
        ));

        assert!(matches!(
            ctx.compare_paterson_obligations(None, head_trait_ty, None, requirement_trait_ty),
            Some(PatersonBoundednessIssue::ConstructorCount { .. })
        ));
    }

    #[test]
    fn supertrait_paterson_measure_counts_assoc_binding_growth() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_id, assoc_id) = add_trait_with_assoc(&mut ctx, "TypeIs", "Is");
        let param_t = ctx.intern("T");
        let param_ty = ctx.type_registry.intern(TypeKind::Param(param_t));
        let wrapped_param_ty = ctx.type_registry.intern(TypeKind::Def(
            DefId(701),
            vec![crate::ty::GenericArg::Type(param_ty)],
        ));
        let requirement_trait_ty = ctx.type_registry.intern(TypeKind::TraitObject(
            trait_id,
            vec![],
            vec![(assoc_id, wrapped_param_ty)],
        ));
        let head_generics = vec![kernc_ast::GenericParam {
            name: param_t,
            span: Span::default(),
            kind: kernc_ast::GenericParamKind::Type,
        }];

        assert!(matches!(
            ctx.compare_paterson_supertrait_against_generics(&head_generics, requirement_trait_ty),
            Some(PatersonBoundednessIssue::ConstructorCount { .. })
        ));
    }

    #[test]
    fn cycle_candidate_search_skips_specificity_shadowed_impls() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_id, _) = add_trait_with_assoc(&mut ctx, "Loop", "Out");
        let generic_param = ctx.intern("T");
        let generic_target_ty = ctx.type_registry.intern(TypeKind::Param(generic_param));
        let loop_trait_ty =
            ctx.type_registry
                .intern(TypeKind::TraitObject(trait_id, Vec::new(), Vec::new()));
        let generic_impl_id = add_trait_impl(
            &mut ctx,
            &[kernc_ast::GenericParam {
                name: generic_param,
                span: Span::default(),
                kind: kernc_ast::GenericParamKind::Type,
            }],
            generic_target_ty,
            loop_trait_ty,
        );
        let loop_trait_ty =
            ctx.type_registry
                .intern(TypeKind::TraitObject(trait_id, Vec::new(), Vec::new()));
        let specific_impl_id = add_trait_impl(&mut ctx, &[], TypeId::I32, loop_trait_ty);
        let loop_trait_ty =
            ctx.type_registry
                .intern(TypeKind::TraitObject(trait_id, Vec::new(), Vec::new()));

        let candidates =
            ctx.collect_specificity_maximal_cycle_candidates(TypeId::I32, loop_trait_ty);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].impl_id, specific_impl_id);
        assert_ne!(candidates[0].impl_id, generic_impl_id);
    }

    #[test]
    fn cycle_candidate_search_rejects_same_named_outer_generic_when_trait_arg_needs_refinement() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let generic_param = kernc_ast::GenericParam {
            name: ctx.intern("T"),
            span: Span::default(),
            kind: kernc_ast::GenericParamKind::Type,
        };
        let trait_id_value = DefId(ctx.defs.len() as u32);
        let marker_name = ctx.intern("Marker");
        let trait_id = ctx.add_def(Def::Trait(TraitDef {
            id: trait_id_value,
            name: marker_name,
            vis: Visibility::Private,
            is_imported: false,
            generics: vec![generic_param.clone()],
            where_clauses: Vec::new(),
            supertraits: Vec::new(),
            resolved_supertraits: Vec::new(),
            assoc_types: Vec::new(),
            methods: Vec::new(),
            resolved_methods: Vec::new(),
            span: Span::default(),
            is_builtin: false,
            docs: None,
        }));

        let outer_t = ctx
            .type_registry
            .intern(TypeKind::Param(generic_param.name));
        let impl_trait_ty = ctx.type_registry.intern(TypeKind::TraitObject(
            trait_id,
            vec![crate::ty::GenericArg::Type(outer_t)],
            Vec::new(),
        ));
        let impl_id = add_trait_impl(
            &mut ctx,
            std::slice::from_ref(&generic_param),
            outer_t,
            impl_trait_ty,
        );
        let obligation_trait_ty = ctx.type_registry.intern(TypeKind::TraitObject(
            trait_id,
            vec![crate::ty::GenericArg::Type(TypeId::I32)],
            Vec::new(),
        ));

        let candidates =
            ctx.collect_specificity_maximal_cycle_candidates(outer_t, obligation_trait_ty);

        assert!(candidates.is_empty());
        assert_eq!(impl_id, DefId(1));
    }

    #[test]
    fn cycle_candidate_search_skips_impl_with_unresolved_local_generic() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_id, _) = add_trait_with_assoc(&mut ctx, "Loop", "Out");
        let param_t = ctx.intern("T");
        let param_u = ctx.intern("U");
        let generic_target_ty = ctx.type_registry.intern(TypeKind::Param(param_t));
        let loop_trait_ty =
            ctx.type_registry
                .intern(TypeKind::TraitObject(trait_id, Vec::new(), Vec::new()));
        add_trait_impl(
            &mut ctx,
            &[
                kernc_ast::GenericParam {
                    name: param_t,
                    span: Span::default(),
                    kind: kernc_ast::GenericParamKind::Type,
                },
                kernc_ast::GenericParam {
                    name: param_u,
                    span: Span::default(),
                    kind: kernc_ast::GenericParamKind::Type,
                },
            ],
            generic_target_ty,
            loop_trait_ty,
        );

        let obligation_trait_ty =
            ctx.type_registry
                .intern(TypeKind::TraitObject(trait_id, Vec::new(), Vec::new()));
        let candidates =
            ctx.collect_specificity_maximal_cycle_candidates(TypeId::I32, obligation_trait_ty);

        assert!(candidates.is_empty());
    }

    fn add_trait_with_assoc(
        ctx: &mut SemaContext<'_>,
        trait_name: &str,
        assoc_name: &str,
    ) -> (DefId, DefId) {
        let trait_id_value = DefId(ctx.defs.len() as u32);
        let trait_name = ctx.intern(trait_name);
        let trait_id = ctx.add_def(Def::Trait(TraitDef {
            id: trait_id_value,
            name: trait_name,
            vis: Visibility::Private,
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            supertraits: Vec::new(),
            resolved_supertraits: Vec::new(),
            assoc_types: Vec::new(),
            methods: Vec::new(),
            resolved_methods: Vec::new(),
            span: Span::default(),
            is_builtin: false,
            docs: None,
        }));
        let assoc_id_value = DefId(ctx.defs.len() as u32);
        let assoc_name = ctx.intern(assoc_name);
        let assoc_id = ctx.add_def(Def::AssociatedType(AssociatedTypeDef {
            id: assoc_id_value,
            name: assoc_name,
            parent_trait: Some(trait_id),
            parent_impl: None,
            implemented_trait_assoc: None,
            is_imported: false,
            generics: Vec::new(),
            bounds: Vec::new(),
            where_clauses: Vec::new(),
            target: None,
            resolved_bounds: Vec::new(),
            span: Span::default(),
            docs: None,
        }));

        if let Def::Trait(trait_def) = &mut ctx.defs[trait_id.0 as usize] {
            trait_def.assoc_types.push(assoc_id);
        }

        (trait_id, assoc_id)
    }

    fn add_trait_impl(
        ctx: &mut SemaContext<'_>,
        generics: &[kernc_ast::GenericParam],
        target_ty: TypeId,
        trait_ty: TypeId,
    ) -> DefId {
        let base = ctx.defs.len() as u32;
        let target_node_id = NodeId(base + 1);
        let trait_node_id = NodeId(base + 2);
        let impl_id_value = DefId(base);
        let impl_id = ctx.add_def(Def::Impl(ImplDef {
            id: impl_id_value,
            parent_module: None,
            is_imported: false,
            generics: generics.to_vec(),
            where_clauses: Vec::new(),
            target_type: kernc_ast::TypeNode {
                id: target_node_id,
                kind: kernc_ast::TypeKind::Infer,
                span: Span::default(),
            },
            trait_type: Some(kernc_ast::TypeNode {
                id: trait_node_id,
                kind: kernc_ast::TypeKind::Infer,
                span: Span::default(),
            }),
            assoc_types: Vec::new(),
            methods: Vec::new(),
            span: Span::default(),
        }));
        ctx.facts.node_types.insert(target_node_id, target_ty);
        ctx.facts.node_types.insert(trait_node_id, trait_ty);
        ctx.impl_index.trait_impls.push(impl_id);
        impl_id
    }
}
