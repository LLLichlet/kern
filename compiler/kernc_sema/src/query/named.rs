use super::*;

impl<'a, 'ctx> MemberQuery<'a, 'ctx> {
    pub(super) fn collect_named_type_field_candidates(
        &mut self,
        current_module_id: Option<DefId>,
        def_id: DefId,
        generic_args: &[crate::ty::GenericArg],
        candidates: &mut Vec<MemberCandidate>,
    ) {
        let Some(def_ptr) = self.ctx.defs.get(def_id.0 as usize).map(std::ptr::from_ref) else {
            return;
        };

        // Safety: member queries do not mutate `ctx.defs`; using raw pointers here avoids
        // cloning whole AST-backed definitions on every field lookup.
        unsafe {
            match &*def_ptr {
                Def::Struct(struct_def) => {
                    for field in &struct_def.fields {
                        if !field_visibility_allows_access(
                            self.ctx,
                            field,
                            def_id,
                            current_module_id,
                        ) {
                            continue;
                        }

                        let ty = self.apply_generics_to_field(
                            &struct_def.generics,
                            generic_args,
                            field.type_node.id,
                        );
                        push_member_candidate(
                            candidates,
                            MemberCandidate {
                                name: field.name,
                                kind: SymbolKind::Var,
                                type_id: ty,
                                def_id: None,
                                definition_span: field.name_span,
                                is_mut: false,
                            },
                        );
                    }
                }
                Def::Union(union_def) => {
                    for field in &union_def.fields {
                        if !field_visibility_allows_access(
                            self.ctx,
                            field,
                            def_id,
                            current_module_id,
                        ) {
                            continue;
                        }

                        let ty = self.apply_generics_to_field(
                            &union_def.generics,
                            generic_args,
                            field.type_node.id,
                        );
                        push_member_candidate(
                            candidates,
                            MemberCandidate {
                                name: field.name,
                                kind: SymbolKind::Var,
                                type_id: ty,
                                def_id: None,
                                definition_span: field.name_span,
                                is_mut: false,
                            },
                        );
                    }
                }
                _ => {}
            }
        }
    }

    pub(super) fn resolve_named_type_field(
        &mut self,
        current_module_id: Option<DefId>,
        def_id: DefId,
        generic_args: &[crate::ty::GenericArg],
        member_name: SymbolId,
        access_span: Span,
    ) -> Option<MemberCandidate> {
        let cache_key = (
            current_module_id,
            def_id,
            generic_args.to_vec(),
            member_name,
        );
        if let Some(cached) = self
            .ctx
            .analysis
            .query_caches
            .named_field_query_cache
            .get(&cache_key)
            .cloned()
        {
            return cached;
        }

        let def_ptr = self
            .ctx
            .defs
            .get(def_id.0 as usize)
            .map(std::ptr::from_ref)?;

        // Safety: semantic definition storage is immutable while member queries run.
        unsafe {
            match &*def_ptr {
                Def::Struct(struct_def) => {
                    let Some(field) = struct_def
                        .fields
                        .iter()
                        .find(|field| field.name == member_name)
                    else {
                        self.ctx
                            .analysis
                            .query_caches
                            .named_field_query_cache
                            .insert(cache_key, None);
                        return None;
                    };
                    if !field_visibility_allows_access(self.ctx, field, def_id, current_module_id) {
                        self.ctx
                            .struct_error(
                                access_span,
                                format!(
                                    "field `{}` of type `{}` is private",
                                    self.ctx.resolve(member_name),
                                    self.ctx.resolve(struct_def.name)
                                ),
                            )
                            .with_hint(
                                "widen the field visibility, or access it from a module allowed by its visibility",
                            )
                            .emit();
                        return Some(MemberCandidate {
                            name: member_name,
                            kind: SymbolKind::Var,
                            type_id: TypeId::ERROR,
                            def_id: None,
                            definition_span: field.name_span,
                            is_mut: false,
                        });
                    }

                    let candidate = MemberCandidate {
                        name: member_name,
                        kind: SymbolKind::Var,
                        type_id: self.apply_generics_to_field(
                            &struct_def.generics,
                            generic_args,
                            field.type_node.id,
                        ),
                        def_id: None,
                        definition_span: field.name_span,
                        is_mut: false,
                    };
                    self.ctx
                        .analysis
                        .query_caches
                        .named_field_query_cache
                        .insert(cache_key, Some(candidate.clone()));
                    Some(candidate)
                }
                Def::Union(union_def) => {
                    let Some(field) = union_def
                        .fields
                        .iter()
                        .find(|field| field.name == member_name)
                    else {
                        self.ctx
                            .analysis
                            .query_caches
                            .named_field_query_cache
                            .insert(cache_key, None);
                        return None;
                    };
                    if !field_visibility_allows_access(self.ctx, field, def_id, current_module_id) {
                        self.ctx
                            .struct_error(
                                access_span,
                                format!(
                                    "field `{}` of type `{}` is private",
                                    self.ctx.resolve(member_name),
                                    self.ctx.resolve(union_def.name)
                                ),
                            )
                            .with_hint(
                                "widen the field visibility, or access it from a module allowed by its visibility",
                            )
                            .emit();
                        return Some(MemberCandidate {
                            name: member_name,
                            kind: SymbolKind::Var,
                            type_id: TypeId::ERROR,
                            def_id: None,
                            definition_span: field.name_span,
                            is_mut: false,
                        });
                    }

                    let candidate = MemberCandidate {
                        name: member_name,
                        kind: SymbolKind::Var,
                        type_id: self.apply_generics_to_field(
                            &union_def.generics,
                            generic_args,
                            field.type_node.id,
                        ),
                        def_id: None,
                        definition_span: field.name_span,
                        is_mut: false,
                    };
                    self.ctx
                        .analysis
                        .query_caches
                        .named_field_query_cache
                        .insert(cache_key, Some(candidate.clone()));
                    Some(candidate)
                }
                _ => None,
            }
        }
    }

    pub(super) fn resolve_named_field_in_type(
        &mut self,
        current_module_id: Option<DefId>,
        search_norm: TypeId,
        member_name: SymbolId,
        access_span: Span,
    ) -> Option<MemberResolution> {
        let started = self.ctx.collects_timings().then(Instant::now);
        let named_type = match self.ctx.type_registry.get(search_norm) {
            TypeKind::Def(def_id, generic_args) => Some((*def_id, generic_args.to_vec())),
            _ => None,
        };
        if let Some((def_id, generic_args)) = named_type
            && let Some(candidate) = self.resolve_named_type_field(
                current_module_id,
                def_id,
                &generic_args,
                member_name,
                access_span,
            )
        {
            if let Some(started) = started {
                self.ctx
                    .analysis
                    .expr_timing_stats
                    .access_field_query_named_type += started.elapsed();
            }
            return Some(MemberResolution {
                candidate,
                owner_trait_ty: None,
            });
        }
        if let Some(started) = started {
            self.ctx
                .analysis
                .expr_timing_stats
                .access_field_query_named_type += started.elapsed();
        }

        if let TypeKind::AnonymousStruct(_, fields) | TypeKind::AnonymousUnion(_, fields) =
            self.ctx.type_registry.get(search_norm)
            && let Some(field) = fields.iter().find(|field| field.name == member_name)
        {
            return Some(MemberResolution {
                candidate: MemberCandidate {
                    name: member_name,
                    kind: SymbolKind::Var,
                    type_id: field.ty,
                    def_id: None,
                    definition_span: Span::default(),
                    is_mut: false,
                },
                owner_trait_ty: None,
            });
        }

        None
    }

    pub(super) fn resolve_named_method_in_type(
        &mut self,
        search_norm: TypeId,
        receiver_ty: TypeId,
        member_name: SymbolId,
        env: &MemberQueryEnv,
        diagnostic_span: Option<Span>,
    ) -> Option<MemberResolution> {
        let started = self.ctx.collects_timings().then(Instant::now);
        if matches!(
            self.ctx.type_registry.get(search_norm),
            TypeKind::TraitObject(..)
        ) && let Some(resolution) = self.resolve_trait_object_method_named(
            search_norm,
            member_name,
            receiver_ty,
            diagnostic_span,
        ) {
            if let Some(started) = started {
                self.ctx
                    .analysis
                    .expr_timing_stats
                    .access_field_query_trait_object += started.elapsed();
            }
            return Some(resolution);
        }
        if let Some(started) = started {
            self.ctx
                .analysis
                .expr_timing_stats
                .access_field_query_trait_object += started.elapsed();
        }

        let started = self.ctx.collects_timings().then(Instant::now);
        if let Some(resolution) =
            self.resolve_bound_member(search_norm, receiver_ty, member_name, env, diagnostic_span)
        {
            if let Some(started) = started {
                self.ctx.analysis.expr_timing_stats.access_field_query_bound += started.elapsed();
            }
            return Some(resolution);
        }
        if let Some(started) = started {
            self.ctx.analysis.expr_timing_stats.access_field_query_bound += started.elapsed();
        }

        if let Some(resolution) = self.resolve_projection_assoc_bound_method(
            search_norm,
            receiver_ty,
            member_name,
            diagnostic_span,
        ) {
            return Some(resolution);
        }

        let started = self.ctx.collects_timings().then(Instant::now);
        let resolution = self
            .resolve_named_impl_method(search_norm, member_name, diagnostic_span)
            .map(|candidate| MemberResolution {
                candidate,
                owner_trait_ty: None,
            });
        if let Some(started) = started {
            self.ctx.analysis.expr_timing_stats.access_field_query_impl += started.elapsed();
        }
        if resolution.is_some() {
            return resolution;
        }

        self.resolve_named_invalid_impl_method(search_norm, member_name)
            .map(|candidate| MemberResolution {
                candidate,
                owner_trait_ty: None,
            })
    }
}
