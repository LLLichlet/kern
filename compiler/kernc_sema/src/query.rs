//! Query helpers for member lookup and impl applicability in semantic analysis.
//!
//! This module is the current "query" layer inside `kernc_sema`. It is not a
//! general incremental database. Its job is narrower and concrete:
//!
//! - gather visible member candidates for a receiver type
//! - resolve a named member access to one concrete field/method candidate
//! - evaluate whether a concrete `impl` applies to a receiver type
//! - thread active `where`-clause bounds into method lookup
//!
//! The current lookup model intentionally merges several sources:
//!
//! - module members
//! - named-type fields
//! - anonymous aggregate fields
//! - trait-object methods, including inherited parent-trait entries
//! - methods made available through active generic bounds
//! - inherent and trait impl methods
//!
//! Search is receiver-oriented rather than syntax-oriented. For mutable
//! receivers, the query also considers the corresponding immutable shape where
//! the language permits shared-method reuse, which keeps method lookup aligned
//! with the ordinary receiver coercion rules used elsewhere in sema.
//!
//! Ambiguity reporting still belongs to the calling checker. This module
//! collects and resolves candidates, but the surrounding expression checker
//! decides how to surface conflicts and access-site diagnostics.

use crate::SemaContext;
use crate::checker::{ExprChecker, Substituter};
use crate::def::{Def, DefId};
use crate::scope::SymbolKind;
use crate::ty::{TypeId, TypeKind};
use kernc_ast as ast;
use kernc_utils::{FastHashMap, FastHashSet, Span, SymbolId};
use std::borrow::Cow;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct MemberCandidate {
    pub name: SymbolId,
    pub kind: SymbolKind,
    pub type_id: TypeId,
    pub def_id: Option<DefId>,
    pub definition_span: Span,
    pub is_mut: bool,
}

#[derive(Debug, Clone, Default)]
pub struct MemberQueryEnv<'a> {
    active_bounds: Cow<'a, [(TypeId, Vec<TypeId>)]>,
}

impl<'a> MemberQueryEnv<'a> {
    pub fn from_active_bounds(bounds: &'a [(TypeId, Vec<TypeId>)]) -> Self {
        Self {
            active_bounds: Cow::Borrowed(bounds),
        }
    }

    pub fn from_active_bounds_owned(bounds: &[(TypeId, Vec<TypeId>)]) -> Self {
        Self {
            active_bounds: Cow::Owned(bounds.to_vec()),
        }
    }

    pub fn extend_with_where_clauses(
        &mut self,
        ctx: &SemaContext<'_>,
        where_clauses: &[ast::WhereClause],
    ) {
        for clause in where_clauses {
            let target_ty = ctx.type_registry.normalize(
                ctx.node_types
                    .get(&clause.target_ty.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR),
            );
            let bounds = clause
                .bounds
                .iter()
                .filter_map(|bound| {
                    ctx.node_types
                        .get(&bound.id)
                        .copied()
                        .map(|bound_ty| ctx.type_registry.normalize(bound_ty))
                })
                .collect();
            self.active_bounds.to_mut().push((target_ty, bounds));
        }
    }

    pub fn len(&self) -> usize {
        self.active_bounds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.active_bounds.is_empty()
    }

    pub fn truncate(&mut self, len: usize) {
        self.active_bounds.to_mut().truncate(len);
    }

    fn active_bounds(&self) -> &[(TypeId, Vec<TypeId>)] {
        self.active_bounds.as_ref()
    }
}

#[derive(Debug, Clone)]
pub struct MemberResolution {
    pub candidate: MemberCandidate,
    pub owner_trait_ty: Option<TypeId>,
}

pub struct MemberQuery<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
}

#[derive(Debug, Clone, Copy)]
struct TraitMethodLookup<'a> {
    trait_args: &'a [TypeId],
    assoc_bindings: &'a [(DefId, TypeId)],
    member_name: SymbolId,
    receiver_ty: TypeId,
    diagnostic_span: Option<Span>,
}

#[derive(Debug, Clone, Copy)]
struct SearchTypes {
    values: [TypeId; 3],
    len: usize,
}

impl SearchTypes {
    fn new(first: TypeId) -> Self {
        Self {
            values: [first; 3],
            len: 1,
        }
    }

    fn push_if_absent(&mut self, ty: TypeId) {
        if self.values[..self.len].contains(&ty) {
            return;
        }

        if let Some(slot) = self.values.get_mut(self.len) {
            *slot = ty;
            self.len += 1;
        }
    }

    fn iter(&self) -> impl Iterator<Item = TypeId> + '_ {
        self.values[..self.len].iter().copied()
    }
}

impl<'a, 'ctx> MemberQuery<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self { ctx }
    }

    pub fn context(&self) -> &SemaContext<'ctx> {
        self.ctx
    }

    pub fn member_candidates(
        &mut self,
        current_module_id: DefId,
        receiver_ty: TypeId,
    ) -> Vec<MemberCandidate> {
        let env = MemberQueryEnv::default();
        self.member_candidates_in_env(Some(current_module_id), receiver_ty, &env)
    }

    pub fn member_candidates_in_env(
        &mut self,
        current_module_id: Option<DefId>,
        receiver_ty: TypeId,
        env: &MemberQueryEnv<'_>,
    ) -> Vec<MemberCandidate> {
        let mut candidates = Vec::new();
        let base_norm = base_type(self.ctx, receiver_ty);

        if let TypeKind::Module(module_def_id) = self.ctx.type_registry.get(base_norm).clone() {
            self.collect_module_candidates(current_module_id, module_def_id, &mut candidates);
            return candidates;
        }

        let search_types = self.search_types(receiver_ty);
        for search_norm in search_types.iter() {
            match self.ctx.type_registry.get(search_norm).clone() {
                TypeKind::Def(def_id, generic_args) => {
                    self.collect_named_type_field_candidates(
                        current_module_id,
                        def_id,
                        &generic_args,
                        &mut candidates,
                    );
                }
                TypeKind::AnonymousStruct(_, fields) | TypeKind::AnonymousUnion(_, fields) => {
                    for field in fields {
                        push_member_candidate(
                            &mut candidates,
                            MemberCandidate {
                                name: field.name,
                                kind: SymbolKind::Var,
                                type_id: field.ty,
                                def_id: None,
                                definition_span: Span::default(),
                                is_mut: false,
                            },
                        );
                    }
                }
                TypeKind::TraitObject(trait_def_id, trait_args, assoc_bindings) => {
                    self.collect_trait_object_method_candidates(
                        trait_def_id,
                        &trait_args,
                        &assoc_bindings,
                        receiver_ty,
                        &mut candidates,
                    );
                }
                _ => {}
            }

            self.collect_bound_method_candidates(search_norm, receiver_ty, env, &mut candidates);
            self.collect_impl_method_candidates(search_norm, &mut candidates);
        }

        candidates
    }

    pub fn resolve_named_member(
        &mut self,
        current_module_id: Option<DefId>,
        receiver_ty: TypeId,
        member_name: SymbolId,
        env: &MemberQueryEnv<'_>,
        access_span: Span,
    ) -> Option<MemberResolution> {
        let base_norm = base_type(self.ctx, receiver_ty);

        if let TypeKind::Module(module_def_id) = self.ctx.type_registry.get(base_norm).clone() {
            return self.resolve_module_member(current_module_id, module_def_id, member_name);
        }

        let search_types = self.search_types(receiver_ty);
        for search_norm in search_types.iter() {
            if let Some(resolution) = self.resolve_named_member_in_type(
                current_module_id,
                search_norm,
                receiver_ty,
                member_name,
                env,
                access_span,
            ) {
                return Some(resolution);
            }
        }

        None
    }

    pub fn resolve_impl_applicability_for_type(
        &mut self,
        receiver_ty: TypeId,
        impl_id: DefId,
    ) -> Option<Vec<TypeId>> {
        let receiver_norm = self.ctx.type_registry.normalize(receiver_ty);
        self.resolve_impl_applicability(receiver_norm, impl_id)
    }

    fn search_types(&mut self, receiver_ty: TypeId) -> SearchTypes {
        let receiver_norm = self.ctx.type_registry.normalize(receiver_ty);
        let base_norm = base_type(self.ctx, receiver_ty);
        let mut search_tys = SearchTypes::new(receiver_norm);

        match self.ctx.type_registry.get(receiver_norm).clone() {
            TypeKind::Pointer { is_mut: true, elem } => {
                search_tys.push_if_absent(self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut: false,
                    elem,
                }));
            }
            TypeKind::VolatilePtr { is_mut: true, elem } => {
                search_tys.push_if_absent(self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                    is_mut: false,
                    elem,
                }));
            }
            TypeKind::Slice { is_mut: true, elem } => {
                search_tys.push_if_absent(self.ctx.type_registry.intern(TypeKind::Slice {
                    is_mut: false,
                    elem,
                }));
            }
            _ => {}
        }

        search_tys.push_if_absent(base_norm);

        search_tys
    }

    fn collect_module_candidates(
        &mut self,
        current_module_id: Option<DefId>,
        module_def_id: DefId,
        candidates: &mut Vec<MemberCandidate>,
    ) {
        let Def::Module(module_def) = &self.ctx.defs[module_def_id.0 as usize] else {
            return;
        };

        for (name, info) in self.ctx.scopes.symbols_in_scope(module_def.scope_id) {
            if !self
                .ctx
                .visibility_allows_access(info.vis, module_def_id, current_module_id)
            {
                continue;
            }

            let type_id = if info.kind == SymbolKind::Function {
                info.def_id
                    .map(|def_id| {
                        self.ctx
                            .type_registry
                            .intern(TypeKind::FnDef(def_id, vec![]))
                    })
                    .unwrap_or(info.type_id)
            } else if info.kind == SymbolKind::Module {
                info.def_id
                    .map(|def_id| self.ctx.type_registry.intern(TypeKind::Module(def_id)))
                    .unwrap_or(info.type_id)
            } else {
                info.type_id
            };

            push_member_candidate(
                candidates,
                MemberCandidate {
                    name,
                    kind: info.kind,
                    type_id,
                    def_id: info.def_id,
                    definition_span: info.span,
                    is_mut: info.is_mut,
                },
            );
        }
    }

    fn resolve_module_member(
        &mut self,
        current_module_id: Option<DefId>,
        module_def_id: DefId,
        member_name: SymbolId,
    ) -> Option<MemberResolution> {
        let Def::Module(module_def) = &self.ctx.defs[module_def_id.0 as usize] else {
            return None;
        };
        let info = self
            .ctx
            .scopes
            .resolve_in(module_def.scope_id, member_name)
            .cloned()?;
        if !self
            .ctx
            .visibility_allows_access(info.vis, module_def_id, current_module_id)
        {
            return None;
        }

        let type_id = if info.kind == SymbolKind::Function {
            info.def_id
                .map(|def_id| {
                    self.ctx
                        .type_registry
                        .intern(TypeKind::FnDef(def_id, vec![]))
                })
                .unwrap_or(info.type_id)
        } else if info.kind == SymbolKind::Module {
            info.def_id
                .map(|def_id| self.ctx.type_registry.intern(TypeKind::Module(def_id)))
                .unwrap_or(info.type_id)
        } else {
            info.type_id
        };

        Some(MemberResolution {
            candidate: MemberCandidate {
                name: member_name,
                kind: info.kind,
                type_id,
                def_id: info.def_id,
                definition_span: info.span,
                is_mut: info.is_mut,
            },
            owner_trait_ty: None,
        })
    }

    fn collect_named_type_field_candidates(
        &mut self,
        current_module_id: Option<DefId>,
        def_id: DefId,
        generic_args: &[TypeId],
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
                        if !field.is_pub
                            && def_owner_module_id(self.ctx, def_id) != current_module_id
                        {
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
                        if !field.is_pub
                            && def_owner_module_id(self.ctx, def_id) != current_module_id
                        {
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

    fn resolve_named_type_field(
        &mut self,
        current_module_id: Option<DefId>,
        def_id: DefId,
        generic_args: &[TypeId],
        member_name: SymbolId,
        access_span: Span,
    ) -> Option<MemberCandidate> {
        let cache_key = (
            current_module_id,
            def_id,
            generic_args.to_vec(),
            member_name,
        );
        if let Some(cached) = self.ctx.named_field_query_cache.get(&cache_key).cloned() {
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
                        self.ctx.named_field_query_cache.insert(cache_key, None);
                        return None;
                    };
                    if !field.is_pub && def_owner_module_id(self.ctx, def_id) != current_module_id {
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
                                "mark the field `pub`, or access it from within the defining module",
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
                        self.ctx.named_field_query_cache.insert(cache_key, None);
                        return None;
                    };
                    if !field.is_pub && def_owner_module_id(self.ctx, def_id) != current_module_id {
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
                                "mark the field `pub`, or access it from within the defining module",
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
                        .named_field_query_cache
                        .insert(cache_key, Some(candidate.clone()));
                    Some(candidate)
                }
                _ => None,
            }
        }
    }

    fn resolve_named_member_in_type(
        &mut self,
        current_module_id: Option<DefId>,
        search_norm: TypeId,
        receiver_ty: TypeId,
        member_name: SymbolId,
        env: &MemberQueryEnv,
        access_span: Span,
    ) -> Option<MemberResolution> {
        let started = Instant::now();
        if matches!(
            self.ctx.type_registry.get(search_norm),
            TypeKind::TraitObject(..)
        ) && let Some(resolution) = self.resolve_trait_object_method_named(
            search_norm,
            member_name,
            receiver_ty,
            Some(access_span),
        ) {
            self.ctx.expr_timing_stats.access_field_query_trait_object += started.elapsed();
            return Some(resolution);
        }
        self.ctx.expr_timing_stats.access_field_query_trait_object += started.elapsed();

        let started = Instant::now();
        if let TypeKind::Def(def_id, generic_args) = self.ctx.type_registry.get(search_norm).clone()
            && let Some(candidate) = self.resolve_named_type_field(
                current_module_id,
                def_id,
                &generic_args,
                member_name,
                access_span,
            )
        {
            self.ctx.expr_timing_stats.access_field_query_named_type += started.elapsed();
            return Some(MemberResolution {
                candidate,
                owner_trait_ty: None,
            });
        }
        self.ctx.expr_timing_stats.access_field_query_named_type += started.elapsed();

        if let TypeKind::AnonymousStruct(_, fields) | TypeKind::AnonymousUnion(_, fields) =
            self.ctx.type_registry.get(search_norm).clone()
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

        let started = Instant::now();
        if let Some(resolution) =
            self.resolve_bound_member(search_norm, receiver_ty, member_name, env, access_span)
        {
            self.ctx.expr_timing_stats.access_field_query_bound += started.elapsed();
            return Some(resolution);
        }
        self.ctx.expr_timing_stats.access_field_query_bound += started.elapsed();

        let started = Instant::now();
        let resolution = self
            .resolve_named_impl_method(search_norm, member_name)
            .map(|candidate| MemberResolution {
                candidate,
                owner_trait_ty: None,
            });
        self.ctx.expr_timing_stats.access_field_query_impl += started.elapsed();
        resolution
    }

    fn collect_bound_method_candidates(
        &mut self,
        search_norm: TypeId,
        receiver_ty: TypeId,
        env: &MemberQueryEnv<'_>,
        candidates: &mut Vec<MemberCandidate>,
    ) {
        self.for_each_matching_bound_trait_object(search_norm, env, |this, bound_norm| {
            if let TypeKind::TraitObject(trait_def_id, trait_args, assoc_bindings) =
                this.ctx.type_registry.get(bound_norm).clone()
            {
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

    fn resolve_bound_member(
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

    fn for_each_matching_bound_trait_object(
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

        let mut map = FastHashMap::default();
        let mut matched_bounds = if can_use_cache {
            Some(Vec::new())
        } else {
            None
        };
        for (env_target, bound_tys) in env.active_bounds() {
            map.clear();
            let matched = if *env_target == search_norm {
                true
            } else {
                let mut checker = ExprChecker::new(self.ctx, None);
                checker.unify(*env_target, search_norm, &mut map)
            };
            if !matched {
                continue;
            }

            if map.is_empty() {
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
                continue;
            }

            for bound_ty in bound_tys.iter().copied() {
                let substituted = {
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                    subst.substitute(bound_ty)
                };
                let bound_norm = self.ctx.type_registry.normalize(substituted);
                if matches!(
                    self.ctx.type_registry.get(bound_norm),
                    TypeKind::TraitObject(..)
                ) {
                    if let Some(bounds) = matched_bounds.as_mut() {
                        bounds.push(bound_norm);
                    }
                    if visit(self, bound_norm) {
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

    fn collect_impl_method_candidates(
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

    fn resolve_named_impl_method(
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

    fn resolve_impl_applicability(
        &mut self,
        receiver_norm: TypeId,
        impl_id: DefId,
    ) -> Option<Vec<TypeId>> {
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

            if impl_def.generics.is_empty() && impl_def.where_clauses.is_empty() {
                if checker.ctx.type_registry.normalize(impl_target_ty) == receiver_norm {
                    Some(Vec::new())
                } else {
                    None
                }
            } else {
                let mut map = FastHashMap::default();
                if !checker.unify(impl_target_ty, receiver_norm, &mut map)
                    || !impl_bounds_satisfied(&mut checker, &impl_def.where_clauses, &map)
                {
                    None
                } else {
                    Some(
                        impl_def
                            .generics
                            .iter()
                            .map(|param| map.get(&param.name).copied().unwrap_or(TypeId::ERROR))
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

    fn collect_trait_object_method_candidates(
        &mut self,
        trait_def_id: DefId,
        trait_args: &[TypeId],
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

    fn resolve_trait_object_method_named(
        &mut self,
        trait_object_ty: TypeId,
        member_name: SymbolId,
        receiver_ty: TypeId,
        diagnostic_span: Option<Span>,
    ) -> Option<MemberResolution> {
        let trait_object_ty = self.ctx.type_registry.normalize(trait_object_ty);
        let TypeKind::TraitObject(trait_def_id, trait_args, assoc_bindings) =
            self.ctx.type_registry.get(trait_object_ty).clone()
        else {
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

    fn collect_trait_methods_in_hierarchy(
        &mut self,
        trait_def_id: DefId,
        trait_args: &[TypeId],
        assoc_bindings: &[(DefId, TypeId)],
        receiver_ty: TypeId,
        visited: &mut FastHashSet<DefId>,
        candidates: &mut Vec<MemberCandidate>,
    ) {
        if !visited.insert(trait_def_id) {
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
            } = self.ctx.type_registry.get(method_ty).clone()
            {
                let mut new_params = params;
                if !new_params.is_empty() {
                    new_params[0] = receiver_ty;
                }
                method_ty = self.ctx.type_registry.intern(TypeKind::Function {
                    params: new_params,
                    ret,
                    is_variadic,
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
            let inst_super_norm = self.ctx.type_registry.normalize(inst_super_ty);

            if let TypeKind::TraitObject(super_def_id, super_args, super_assoc_bindings) =
                self.ctx.type_registry.get(inst_super_norm).clone()
            {
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

    fn resolve_trait_method_in_hierarchy(
        &mut self,
        trait_def_id: DefId,
        lookup: TraitMethodLookup<'_>,
        visited: &mut FastHashSet<DefId>,
    ) -> Option<MemberResolution> {
        let TraitMethodLookup {
            trait_args,
            assoc_bindings,
            member_name,
            receiver_ty,
            diagnostic_span,
        } = lookup;
        if !visited.insert(trait_def_id) {
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
            } = self.ctx.type_registry.get(method_ty).clone()
            {
                let mut new_params = params;
                if !new_params.is_empty() {
                    new_params[0] = receiver_ty;
                }
                method_ty = self.ctx.type_registry.intern(TypeKind::Function {
                    params: new_params,
                    ret,
                    is_variadic,
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
                owner_trait_ty: Some(
                    self.ctx
                        .type_registry
                        .intern(TypeKind::TraitObject(
                            trait_def_id,
                            trait_args.to_vec(),
                            assoc_bindings.to_vec(),
                        )),
                ),
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
            let inst_super_norm = self.ctx.type_registry.normalize(inst_super_ty);

            if let TypeKind::TraitObject(super_def_id, super_args, super_assoc_bindings) =
                self.ctx.type_registry.get(inst_super_norm).clone()
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

    fn materialize_trait_assoc_placeholders(
        &mut self,
        ty: TypeId,
        receiver_ty: TypeId,
        trait_def_id: DefId,
        trait_args: &[TypeId],
        assoc_bindings: &[(DefId, TypeId)],
    ) -> TypeId {
        let assoc_binding_map = assoc_bindings
            .iter()
            .copied()
            .collect::<FastHashMap<_, _>>();
        let substituted = crate::checker::substitute_associated_types(
            &mut self.ctx.type_registry,
            ty,
            &assoc_binding_map,
        );
        self.project_unbound_trait_assoc_types(
            substituted,
            receiver_ty,
            trait_def_id,
            trait_args,
        )
    }

    fn project_unbound_trait_assoc_types(
        &mut self,
        ty: TypeId,
        receiver_ty: TypeId,
        trait_def_id: DefId,
        trait_args: &[TypeId],
    ) -> TypeId {
        let kind = self.ctx.type_registry.get(ty).clone();
        match kind {
            TypeKind::Primitive(_)
            | TypeKind::Simd { .. }
            | TypeKind::Error
            | TypeKind::Module(_)
            | TypeKind::TypeVar(_)
            | TypeKind::Param(_) => ty,
            TypeKind::Associated(assoc_def_id, assoc_args) => {
                let new_assoc_args = assoc_args
                    .into_iter()
                    .map(|arg| {
                        self.project_unbound_trait_assoc_types(
                            arg,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect::<Vec<_>>();
                let belongs_to_trait = matches!(
                    self.ctx.defs.get(assoc_def_id.0 as usize),
                    Some(Def::AssociatedType(assoc_def))
                        if assoc_def.parent_trait == Some(trait_def_id)
                );
                if belongs_to_trait {
                    self.ctx.type_registry.intern(TypeKind::Projection {
                        target: receiver_ty,
                        trait_def_id,
                        trait_args: trait_args.to_vec(),
                        assoc_def_id,
                        assoc_args: new_assoc_args,
                    })
                } else {
                    self.ctx
                        .type_registry
                        .intern(TypeKind::Associated(assoc_def_id, new_assoc_args))
                }
            }
            TypeKind::Pointer { is_mut, elem } => {
                let new_elem = self.project_unbound_trait_assoc_types(
                    elem,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut,
                    elem: new_elem,
                })
            }
            TypeKind::VolatilePtr { is_mut, elem } => {
                let new_elem = self.project_unbound_trait_assoc_types(
                    elem,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                    is_mut,
                    elem: new_elem,
                })
            }
            TypeKind::Slice { is_mut, elem } => {
                let new_elem = self.project_unbound_trait_assoc_types(
                    elem,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx.type_registry.intern(TypeKind::Slice {
                    is_mut,
                    elem: new_elem,
                })
            }
            TypeKind::Array { is_mut, elem, len } => {
                let new_elem = self.project_unbound_trait_assoc_types(
                    elem,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx.type_registry.intern(TypeKind::Array {
                    is_mut,
                    elem: new_elem,
                    len,
                })
            }
            TypeKind::ArrayInfer { is_mut, elem } => {
                let new_elem = self.project_unbound_trait_assoc_types(
                    elem,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx.type_registry.intern(TypeKind::ArrayInfer {
                    is_mut,
                    elem: new_elem,
                })
            }
            TypeKind::Function {
                params,
                ret,
                is_variadic,
            } => {
                let new_params = params
                    .into_iter()
                    .map(|param| {
                        self.project_unbound_trait_assoc_types(
                            param,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                let new_ret = self.project_unbound_trait_assoc_types(
                    ret,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx.type_registry.intern(TypeKind::Function {
                    params: new_params,
                    ret: new_ret,
                    is_variadic,
                })
            }
            TypeKind::Def(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.project_unbound_trait_assoc_types(
                            arg,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                self.ctx.type_registry.intern(TypeKind::Def(def_id, new_args))
            }
            TypeKind::Enum(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.project_unbound_trait_assoc_types(
                            arg,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                self.ctx.type_registry.intern(TypeKind::Enum(def_id, new_args))
            }
            TypeKind::EnumPayload(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.project_unbound_trait_assoc_types(
                            arg,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::EnumPayload(def_id, new_args))
            }
            TypeKind::TraitObject(def_id, args, assoc_bindings) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.project_unbound_trait_assoc_types(
                            arg,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                let new_assoc_bindings = assoc_bindings
                    .into_iter()
                    .map(|(assoc_def_id, assoc_ty)| {
                        (
                            assoc_def_id,
                            self.project_unbound_trait_assoc_types(
                                assoc_ty,
                                receiver_ty,
                                trait_def_id,
                                trait_args,
                            ),
                        )
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::TraitObject(def_id, new_args, new_assoc_bindings))
            }
            TypeKind::Projection {
                target,
                trait_def_id: projection_trait_def_id,
                trait_args: projection_trait_args,
                assoc_def_id,
                assoc_args,
            } => {
                let new_target = self.project_unbound_trait_assoc_types(
                    target,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                let new_trait_args = projection_trait_args
                    .into_iter()
                    .map(|arg| {
                        self.project_unbound_trait_assoc_types(
                            arg,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                let new_assoc_args = assoc_args
                    .into_iter()
                    .map(|arg| {
                        self.project_unbound_trait_assoc_types(
                            arg,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                self.ctx.type_registry.intern(TypeKind::Projection {
                    target: new_target,
                    trait_def_id: projection_trait_def_id,
                    trait_args: new_trait_args,
                    assoc_def_id,
                    assoc_args: new_assoc_args,
                })
            }
            TypeKind::ClosureInterface { params, ret } => {
                let new_params = params
                    .into_iter()
                    .map(|param| {
                        self.project_unbound_trait_assoc_types(
                            param,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                let new_ret = self.project_unbound_trait_assoc_types(
                    ret,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx.type_registry.intern(TypeKind::ClosureInterface {
                    params: new_params,
                    ret: new_ret,
                })
            }
            TypeKind::AnonymousState {
                closure_node_id,
                captures,
                params,
                ret,
            } => {
                let new_captures = captures
                    .into_iter()
                    .map(|capture| {
                        self.project_unbound_trait_assoc_types(
                            capture,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                let new_params = params
                    .into_iter()
                    .map(|param| {
                        self.project_unbound_trait_assoc_types(
                            param,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                let new_ret = self.project_unbound_trait_assoc_types(
                    ret,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx.type_registry.intern(TypeKind::AnonymousState {
                    closure_node_id,
                    captures: new_captures,
                    params: new_params,
                    ret: new_ret,
                })
            }
            TypeKind::Alias(name, target) => {
                let new_target = self.project_unbound_trait_assoc_types(
                    target,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx.type_registry.intern(TypeKind::Alias(name, new_target))
            }
            TypeKind::FnDef(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.project_unbound_trait_assoc_types(
                            arg,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                self.ctx.type_registry.intern(TypeKind::FnDef(def_id, new_args))
            }
            TypeKind::AnonymousStruct(is_extern, fields) => {
                let new_fields = fields
                    .into_iter()
                    .map(|field| crate::ty::AnonymousField {
                        name: field.name,
                        ty: self.project_unbound_trait_assoc_types(
                            field.ty,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        ),
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousStruct(is_extern, new_fields))
            }
            TypeKind::AnonymousUnion(is_extern, fields) => {
                let new_fields = fields
                    .into_iter()
                    .map(|field| crate::ty::AnonymousField {
                        name: field.name,
                        ty: self.project_unbound_trait_assoc_types(
                            field.ty,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        ),
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousUnion(is_extern, new_fields))
            }
            TypeKind::AnonymousEnum(enum_def) => {
                let new_backing_ty = enum_def.backing_ty.map(|backing_ty| {
                    self.project_unbound_trait_assoc_types(
                        backing_ty,
                        receiver_ty,
                        trait_def_id,
                        trait_args,
                    )
                });
                let new_variants = enum_def
                    .variants
                    .into_iter()
                    .map(|variant| crate::ty::AnonymousVariant {
                        name: variant.name,
                        name_span: variant.name_span,
                        payload_ty: variant.payload_ty.map(|payload_ty| {
                            self.project_unbound_trait_assoc_types(
                                payload_ty,
                                receiver_ty,
                                trait_def_id,
                                trait_args,
                            )
                        }),
                        explicit_value: variant.explicit_value,
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousEnum(crate::ty::AnonymousEnum {
                        backing_ty: new_backing_ty,
                        builtin: enum_def.builtin,
                        variants: new_variants,
                    }))
            }
            TypeKind::AnonymousEnumPayload(enum_ty) => {
                let new_enum_ty = self.project_unbound_trait_assoc_types(
                    enum_ty,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousEnumPayload(new_enum_ty))
            }
        }
    }

    fn apply_generics_to_field(
        &mut self,
        generics: &[ast::GenericParam],
        args: &[TypeId],
        node_id: kernc_utils::NodeId,
    ) -> TypeId {
        if generics.is_empty() || args.is_empty() {
            return self
                .ctx
                .node_types
                .get(&node_id)
                .copied()
                .unwrap_or(TypeId::ERROR);
        }

        let cache_key = (node_id, args.to_vec());
        if let Some(&field_ty) = self.ctx.field_type_subst_cache.get(&cache_key) {
            return field_ty;
        }

        let mut field_ty = self
            .ctx
            .node_types
            .get(&node_id)
            .copied()
            .unwrap_or(TypeId::ERROR);

        let mut map = FastHashMap::default();
        for (index, param) in generics.iter().enumerate() {
            if let Some(arg) = args.get(index).copied() {
                map.insert(param.name, arg);
            }
        }
        let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
        field_ty = subst.substitute(field_ty);
        self.ctx.field_type_subst_cache.insert(cache_key, field_ty);

        field_ty
    }
}

fn base_type(ctx: &SemaContext<'_>, mut ty: TypeId) -> TypeId {
    loop {
        let norm = ctx.type_registry.normalize(ty);
        match ctx.type_registry.get(norm).clone() {
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => ty = elem,
            _ => return norm,
        }
    }
}

fn def_owner_module_id(ctx: &SemaContext<'_>, def_id: DefId) -> Option<DefId> {
    match ctx.defs.get(def_id.0 as usize) {
        Some(Def::Struct(def)) => def.parent_module,
        Some(Def::Union(def)) => def.parent_module,
        Some(Def::Enum(_)) | Some(Def::Trait(_)) | Some(Def::TypeAlias(_)) => {
            ctx.def_parent_module(def_id)
        }
        _ => None,
    }
}

fn trait_method_span(trait_def: &crate::def::TraitDef, method_name: SymbolId) -> Span {
    trait_def
        .methods
        .iter()
        .find(|method| method.name == method_name)
        .map(|method| method.name_span)
        .unwrap_or_default()
}

fn impl_bounds_satisfied(
    checker: &mut ExprChecker<'_, '_>,
    where_clauses: &[ast::WhereClause],
    map: &FastHashMap<SymbolId, TypeId>,
) -> bool {
    let mut pairs_to_check = Vec::new();

    {
        let mut subst = Substituter::new(&mut checker.ctx.type_registry, map);
        for clause in where_clauses {
            let original_target = checker
                .ctx
                .node_types
                .get(&clause.target_ty.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let sub_target = subst.substitute(original_target);

            for bound_ast in &clause.bounds {
                let original_bound = checker
                    .ctx
                    .node_types
                    .get(&bound_ast.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let sub_bound = subst.substitute(original_bound);
                pairs_to_check.push((sub_target, sub_bound));
            }
        }
    }

    for (sub_target, sub_bound) in pairs_to_check {
        if sub_target != TypeId::ERROR
            && sub_bound != TypeId::ERROR
            && !checker.check_trait_impl(sub_target, sub_bound)
        {
            return false;
        }
    }

    true
}

fn push_member_candidate(candidates: &mut Vec<MemberCandidate>, candidate: MemberCandidate) {
    if let Some(index) = candidates
        .iter()
        .position(|existing| existing.name == candidate.name)
    {
        candidates.remove(index);
    }
    candidates.push(candidate);
}
