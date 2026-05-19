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
use crate::checker::ExprChecker;
use crate::def::{Def, DefId, ImplDef};
use crate::passes::TypeResolver;
use crate::scope::SymbolKind;
use crate::ty::{Substituter, TypeId, TypeKind};
use kernc_ast as ast;

mod methods;
mod module;
mod named;
mod projection;

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

    pub fn from_current_active_bounds(ctx: &SemaContext<'_>) -> Self {
        Self::from_active_bounds_owned(&ctx.analysis.active_bounds)
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
            let target_ty = ctx.normalized_node_type_or_error(clause.target_ty.id);
            let bounds = clause
                .bounds
                .iter()
                .filter_map(|bound| {
                    ctx.node_type(bound.id)
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

    fn is_current_active_bounds(&self, ctx: &SemaContext<'_>) -> bool {
        let current_bounds = ctx.analysis.active_bounds.as_slice();
        self.active_bounds.as_ref() == current_bounds
    }
}

pub(crate) fn instantiated_env_trait_bounds(
    ctx: &mut SemaContext<'_>,
    search_ty: TypeId,
    env_bounds: &[(TypeId, Vec<TypeId>)],
) -> Vec<TypeId> {
    if env_bounds.is_empty() {
        return Vec::new();
    }

    let mut checker = ExprChecker::new(ctx, None);
    let search_norm = checker.resolve_tv(search_ty);
    let mut type_map = FastHashMap::default();
    let mut const_map = FastHashMap::default();
    let mut matches = Vec::new();

    for (env_target, bound_tys) in env_bounds {
        type_map.clear();
        const_map.clear();

        let matched = if *env_target == search_norm {
            true
        } else {
            checker.match_available_type_against_requirement(
                *env_target,
                search_ty,
                &mut type_map,
                &mut const_map,
            )
        };
        if !matched {
            continue;
        }

        for &bound_ty in bound_tys {
            let inst_bound = if type_map.is_empty() && const_map.is_empty() {
                bound_ty
            } else {
                checker.substitute_type_with_unification_maps(bound_ty, &type_map, &const_map)
            };
            let inst_bound_norm = checker.resolve_tv(inst_bound);
            if matches!(
                checker.ctx.type_registry.get(inst_bound_norm),
                TypeKind::TraitObject(..)
            ) {
                matches.push(inst_bound_norm);
            }
        }
    }

    matches
}

#[derive(Debug, Clone)]
pub struct MemberResolution {
    pub candidate: MemberCandidate,
    pub owner_trait_ty: Option<TypeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImplSpecificity {
    LeftMoreSpecific,
    RightMoreSpecific,
    Ambiguous,
}

#[derive(Debug, Clone)]
pub(crate) struct ApplicableTraitImplHeadCandidate {
    pub(crate) impl_id: DefId,
    pub(crate) impl_args: Vec<crate::ty::GenericArg>,
}

pub struct MemberQuery<'a, 'ctx> {
    /// Mutable because lookup may instantiate types, update caches, and emit
    /// targeted ambiguity diagnostics.
    ctx: &'a mut SemaContext<'ctx>,
}

#[derive(Debug, Clone, Copy)]
pub struct TraitMethodLookup<'a> {
    pub trait_args: &'a [crate::ty::GenericArg],
    pub assoc_bindings: &'a [(DefId, TypeId)],
    pub member_name: SymbolId,
    pub receiver_ty: TypeId,
    pub diagnostic_span: Option<Span>,
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
            // Search sets are intentionally tiny: receiver, normalized receiver,
            // and possibly immutable/shared view.
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

        if let TypeKind::Module(module_def_id) = self.ctx.type_registry.get(base_norm) {
            self.collect_module_candidates(current_module_id, *module_def_id, &mut candidates);
            return candidates;
        }

        let search_types = self.search_types(receiver_ty);
        for search_norm in search_types.iter() {
            if let Some((def_id, generic_args)) = match self.ctx.type_registry.get(search_norm) {
                TypeKind::Def(def_id, generic_args) => Some((*def_id, generic_args.to_vec())),
                _ => None,
            } {
                self.collect_named_type_field_candidates(
                    current_module_id,
                    def_id,
                    &generic_args,
                    &mut candidates,
                );
            } else if let TypeKind::AnonymousStruct(_, fields)
            | TypeKind::AnonymousUnion(_, fields) =
                self.ctx.type_registry.get(search_norm)
            {
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
            } else if let TypeKind::Range { start, end, .. } =
                self.ctx.type_registry.get(search_norm).clone()
            {
                self.collect_range_field_candidates(start, end, &mut candidates);
            } else if let Some((trait_def_id, trait_args, assoc_bindings)) =
                match self.ctx.type_registry.get(search_norm) {
                    TypeKind::TraitObject(trait_def_id, trait_args, assoc_bindings) => {
                        Some((*trait_def_id, trait_args.to_vec(), assoc_bindings.to_vec()))
                    }
                    _ => None,
                }
            {
                self.collect_trait_object_method_candidates(
                    trait_def_id,
                    &trait_args,
                    &assoc_bindings,
                    receiver_ty,
                    &mut candidates,
                );
            }

            self.collect_bound_method_candidates(search_norm, receiver_ty, env, &mut candidates);
            self.collect_impl_method_candidates(search_norm, &mut candidates);
        }

        candidates
    }

    fn collect_range_field_candidates(
        &mut self,
        start: Option<TypeId>,
        end: Option<TypeId>,
        candidates: &mut Vec<MemberCandidate>,
    ) {
        if let Some(start) = start {
            push_member_candidate(
                candidates,
                MemberCandidate {
                    name: self.ctx.intern("start"),
                    kind: SymbolKind::Var,
                    type_id: start,
                    def_id: None,
                    definition_span: Span::default(),
                    is_mut: false,
                },
            );
        }
        if let Some(end) = end {
            push_member_candidate(
                candidates,
                MemberCandidate {
                    name: self.ctx.intern("end"),
                    kind: SymbolKind::Var,
                    type_id: end,
                    def_id: None,
                    definition_span: Span::default(),
                    is_mut: false,
                },
            );
        }
    }

    pub fn resolve_named_member(
        &mut self,
        current_module_id: Option<DefId>,
        receiver_ty: TypeId,
        member_name: SymbolId,
        env: &MemberQueryEnv<'_>,
        access_span: Span,
    ) -> Option<MemberResolution> {
        let cache_key = (current_module_id, receiver_ty, member_name);
        let can_use_cache = env.is_current_active_bounds(self.ctx);
        if can_use_cache
            && let Some(cached) = self
                .ctx
                .analysis
                .query_caches
                .member_resolution_query_cache
                .get(&cache_key)
                .cloned()
        {
            return Some(cached);
        }

        let base_norm = base_type(self.ctx, receiver_ty);

        if let TypeKind::Module(module_def_id) = self.ctx.type_registry.get(base_norm) {
            let resolution =
                self.resolve_module_member(current_module_id, *module_def_id, member_name);
            if let Some(resolution) = resolution {
                self.cache_member_resolution(cache_key, can_use_cache, &resolution);
                return Some(resolution);
            }
            return None;
        }

        let search_types = self.search_types(receiver_ty);
        for search_norm in search_types.iter() {
            if let Some(resolution) = self.resolve_named_field_in_type(
                current_module_id,
                search_norm,
                member_name,
                access_span,
            ) {
                self.cache_member_resolution(cache_key, can_use_cache, &resolution);
                return Some(resolution);
            }
        }

        if let Some(resolution) =
            self.resolve_named_method(receiver_ty, member_name, env, Some(access_span))
        {
            self.cache_member_resolution(cache_key, can_use_cache, &resolution);
            return Some(resolution);
        }

        None
    }

    pub fn resolve_named_method(
        &mut self,
        receiver_ty: TypeId,
        member_name: SymbolId,
        env: &MemberQueryEnv<'_>,
        diagnostic_span: Option<Span>,
    ) -> Option<MemberResolution> {
        let cache_key = (receiver_ty, member_name);
        let can_use_cache = env.is_current_active_bounds(self.ctx);
        if can_use_cache
            && let Some(cached) = self
                .ctx
                .analysis
                .query_caches
                .method_resolution_query_cache
                .get(&cache_key)
                .cloned()
        {
            return cached;
        }

        let search_types = self.search_types(receiver_ty);
        for search_norm in search_types.iter() {
            if let Some(resolution) = self.resolve_named_method_in_type(
                search_norm,
                receiver_ty,
                member_name,
                env,
                diagnostic_span,
            ) {
                if can_use_cache && resolution.candidate.type_id != TypeId::ERROR {
                    self.ctx
                        .analysis
                        .query_caches
                        .method_resolution_query_cache
                        .insert(cache_key, Some(resolution.clone()));
                }
                return Some(resolution);
            }
        }

        if can_use_cache && diagnostic_span.is_some() {
            self.ctx
                .analysis
                .query_caches
                .method_resolution_query_cache
                .insert(cache_key, None);
        }
        None
    }

    pub fn resolve_impl_applicability_for_type(
        &mut self,
        receiver_ty: TypeId,
        impl_id: DefId,
    ) -> Option<Vec<crate::ty::GenericArg>> {
        let receiver_norm = self.ctx.type_registry.normalize(receiver_ty);
        self.resolve_impl_applicability(receiver_norm, impl_id)
    }

    fn search_types(&mut self, receiver_ty: TypeId) -> SearchTypes {
        let receiver_norm = self.ctx.type_registry.normalize(receiver_ty);
        let base_norm = base_type(self.ctx, receiver_ty);
        let mut search_tys = SearchTypes::new(receiver_norm);

        let downgraded = match self.ctx.type_registry.get(receiver_norm) {
            TypeKind::Pointer { is_mut: true, elem } => Some(TypeKind::Pointer {
                is_mut: false,
                elem: *elem,
            }),
            TypeKind::VolatilePtr { is_mut: true, elem } => Some(TypeKind::VolatilePtr {
                is_mut: false,
                elem: *elem,
            }),
            TypeKind::Slice { is_mut: true, elem } => Some(TypeKind::Slice {
                is_mut: false,
                elem: *elem,
            }),
            _ => None,
        };
        if let Some(ty) = downgraded {
            search_tys.push_if_absent(self.ctx.type_registry.intern(ty));
        }

        search_tys.push_if_absent(base_norm);

        search_tys
    }

    fn cache_member_resolution(
        &mut self,
        key: (Option<DefId>, TypeId, SymbolId),
        enabled: bool,
        resolution: &MemberResolution,
    ) {
        if enabled && resolution.candidate.type_id != TypeId::ERROR {
            self.ctx
                .analysis
                .query_caches
                .member_resolution_query_cache
                .insert(key, resolution.clone());
        }
    }
}

fn base_type(ctx: &SemaContext<'_>, mut ty: TypeId) -> TypeId {
    loop {
        let norm = ctx.type_registry.normalize(ty);
        match ctx.type_registry.get(norm) {
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => ty = *elem,
            _ => return norm,
        }
    }
}

fn field_visibility_allows_access(
    ctx: &SemaContext<'_>,
    field: &ast::StructFieldDef,
    def_id: DefId,
    current_module_id: Option<DefId>,
) -> bool {
    ctx.field_visibility_allows_access(field.vis, def_id, current_module_id)
}

fn trait_method_span(trait_def: &crate::def::TraitDef, method_name: SymbolId) -> Span {
    trait_def
        .methods
        .iter()
        .find(|method| method.signature.name == method_name)
        .map(|method| method.signature.name_span)
        .unwrap_or_default()
}

pub fn compare_impl_specificity(
    ctx: &mut SemaContext<'_>,
    left_impl_id: DefId,
    right_impl_id: DefId,
) -> ImplSpecificity {
    let left_specializes_right = impl_head_specializes(ctx, left_impl_id, right_impl_id);
    let right_specializes_left = impl_head_specializes(ctx, right_impl_id, left_impl_id);

    match (left_specializes_right, right_specializes_left) {
        (true, false) => ImplSpecificity::LeftMoreSpecific,
        (false, true) => ImplSpecificity::RightMoreSpecific,
        (true, true) => compare_impl_where_specificity(ctx, left_impl_id, right_impl_id),
        _ => ImplSpecificity::Ambiguous,
    }
}

pub(crate) fn compare_method_impl_specificity(
    ctx: &mut SemaContext<'_>,
    left_impl_id: DefId,
    right_impl_id: DefId,
) -> ImplSpecificity {
    match (
        impl_is_inherent(ctx, left_impl_id),
        impl_is_inherent(ctx, right_impl_id),
    ) {
        (true, false) => return ImplSpecificity::LeftMoreSpecific,
        (false, true) => return ImplSpecificity::RightMoreSpecific,
        _ => {}
    }

    compare_impl_specificity(ctx, left_impl_id, right_impl_id)
}

fn impl_is_inherent(ctx: &SemaContext<'_>, impl_id: DefId) -> bool {
    match ctx.defs.get(impl_id.0 as usize) {
        Some(Def::Impl(impl_def)) => impl_def.trait_type.is_none(),
        _ => false,
    }
}

pub fn impl_head_specializes(
    ctx: &mut SemaContext<'_>,
    specialized_impl_id: DefId,
    general_impl_id: DefId,
) -> bool {
    if specialized_impl_id == general_impl_id {
        return false;
    }

    let Some((specialized_impl, specialized_target_ty, specialized_trait_ty)) =
        impl_head_signature(ctx, specialized_impl_id)
    else {
        return false;
    };
    let Some((general_impl, general_target_ty, general_trait_ty)) =
        impl_head_signature(ctx, general_impl_id)
    else {
        return false;
    };

    if specialized_trait_ty.is_some() != general_trait_ty.is_some() {
        return false;
    }

    let mut checker = ExprChecker::new(ctx, None);
    let specialized = freshen_impl_head_types(
        &mut checker,
        &specialized_impl,
        specialized_target_ty,
        specialized_trait_ty,
        ImplHeadFreshness::Rigid,
    );
    let general = freshen_impl_head_types(
        &mut checker,
        &general_impl,
        general_target_ty,
        general_trait_ty,
        ImplHeadFreshness::Flexible,
    );

    let mut type_map = FastHashMap::default();
    let mut const_map = FastHashMap::default();
    checker.match_available_type_against_requirement(
        general.target_ty,
        specialized.target_ty,
        &mut type_map,
        &mut const_map,
    ) && match (general.trait_ty, specialized.trait_ty) {
        (Some(general_trait), Some(specialized_trait)) => checker
            .match_available_type_against_requirement(
                general_trait,
                specialized_trait,
                &mut type_map,
                &mut const_map,
            ),
        (None, None) => true,
        _ => false,
    }
}

fn compare_impl_where_specificity(
    ctx: &mut SemaContext<'_>,
    left_impl_id: DefId,
    right_impl_id: DefId,
) -> ImplSpecificity {
    let left_extends_right =
        impl_where_requirements_strictly_extend(ctx, left_impl_id, right_impl_id);
    let right_extends_left =
        impl_where_requirements_strictly_extend(ctx, right_impl_id, left_impl_id);

    match (left_extends_right, right_extends_left) {
        (true, false) => ImplSpecificity::LeftMoreSpecific,
        (false, true) => ImplSpecificity::RightMoreSpecific,
        _ => ImplSpecificity::Ambiguous,
    }
}

fn impl_where_requirements_strictly_extend(
    ctx: &mut SemaContext<'_>,
    specialized_impl_id: DefId,
    general_impl_id: DefId,
) -> bool {
    let Some((specialized_impl, specialized_target_ty, specialized_trait_ty)) =
        impl_head_signature(ctx, specialized_impl_id)
    else {
        return false;
    };
    let Some((general_impl, general_target_ty, general_trait_ty)) =
        impl_head_signature(ctx, general_impl_id)
    else {
        return false;
    };

    let mut checker = ExprChecker::new(ctx, None);
    let specialized = freshen_impl_head_types(
        &mut checker,
        &specialized_impl,
        specialized_target_ty,
        specialized_trait_ty,
        ImplHeadFreshness::Rigid,
    );
    let general = freshen_impl_head_types(
        &mut checker,
        &general_impl,
        general_target_ty,
        general_trait_ty,
        ImplHeadFreshness::Flexible,
    );

    let mut type_map = FastHashMap::default();
    let mut const_map = FastHashMap::default();
    if !checker.match_available_type_against_requirement(
        general.target_ty,
        specialized.target_ty,
        &mut type_map,
        &mut const_map,
    ) || !match (general.trait_ty, specialized.trait_ty) {
        (Some(general_trait), Some(specialized_trait)) => checker
            .match_available_type_against_requirement(
                general_trait,
                specialized_trait,
                &mut type_map,
                &mut const_map,
            ),
        (None, None) => true,
        _ => false,
    } {
        return false;
    }

    let specialized_requirements =
        instantiate_where_requirements(&mut checker, &specialized_impl, &specialized.subst_map);
    let general_requirements =
        instantiate_where_requirements(&mut checker, &general_impl, &general.subst_map);
    if specialized_requirements.len() <= general_requirements.len() {
        return false;
    }

    general_requirements
        .iter()
        .all(|requirement| specialized_requirements.contains(requirement))
}

pub fn resolve_trait_impl_obligation(
    ctx: &mut SemaContext<'_>,
    receiver_ty: TypeId,
    target_trait_ty: TypeId,
    impl_id: DefId,
) -> Option<Vec<crate::ty::GenericArg>> {
    resolve_trait_impl_obligation_inner(ctx, receiver_ty, target_trait_ty, impl_id, false)
}

pub fn resolve_trait_impl_head_obligation(
    ctx: &mut SemaContext<'_>,
    receiver_ty: TypeId,
    trait_def_id: DefId,
    trait_args: &[crate::ty::GenericArg],
    impl_id: DefId,
) -> Option<Vec<crate::ty::GenericArg>> {
    let target_trait_ty = ctx.type_registry.intern(TypeKind::TraitObject(
        trait_def_id,
        trait_args.to_vec(),
        Vec::new(),
    ));
    resolve_trait_impl_obligation_inner(ctx, receiver_ty, target_trait_ty, impl_id, true)
}

pub(crate) fn instantiate_impl_trait_ty(
    ctx: &mut SemaContext<'_>,
    impl_id: DefId,
    impl_args: &[crate::ty::GenericArg],
) -> Option<TypeId> {
    if !impl_generic_args_fully_resolved(impl_args) {
        return None;
    }

    let impl_def = ctx.defs.get(impl_id.0 as usize).and_then(|def| match def {
        Def::Impl(impl_def) => Some(impl_def.clone()),
        _ => None,
    })?;
    let impl_trait_node = impl_def.trait_type.as_ref()?;
    let impl_trait_ty = ctx.node_type_or_error(impl_trait_node.id);
    if impl_trait_ty == TypeId::ERROR {
        return None;
    }

    let subst_map = impl_def
        .generics
        .iter()
        .zip(impl_args.iter().copied())
        .map(|(param, arg)| (param.name, arg))
        .collect::<FastHashMap<_, _>>();
    let inst_trait_ty = if subst_map.is_empty() {
        impl_trait_ty
    } else {
        let mut subst = Substituter::new(&mut ctx.type_registry, &subst_map);
        subst.substitute(impl_trait_ty)
    };

    Some(ctx.type_registry.normalize(inst_trait_ty))
}

pub(crate) fn select_most_specific_trait_impl_head(
    ctx: &mut SemaContext<'_>,
    receiver_ty: TypeId,
    trait_def_id: DefId,
    trait_args: &[crate::ty::GenericArg],
) -> Option<(DefId, Vec<crate::ty::GenericArg>)> {
    let mut candidates = collect_specificity_maximal_trait_impl_head_candidates(
        ctx,
        receiver_ty,
        trait_def_id,
        trait_args,
    );
    // Query clients use this helper to continue proof search, inject associated bindings, or pick
    // method owners. Returning an arbitrary impl on ambiguity would fabricate solver facts in
    // erroneous code before coherence diagnostics have a chance to fire.
    if candidates.len() != 1 {
        return None;
    }

    let candidate = candidates.pop().expect("length checked above");
    Some((candidate.impl_id, candidate.impl_args))
}

pub(crate) fn collect_specificity_maximal_trait_impl_head_candidates(
    ctx: &mut SemaContext<'_>,
    receiver_ty: TypeId,
    trait_def_id: DefId,
    trait_args: &[crate::ty::GenericArg],
) -> Vec<ApplicableTraitImplHeadCandidate> {
    // Snapshot the impl list so candidate matching can resolve signatures and compare
    // specificity without keeping a borrow of the index alive across recursive queries.
    let trait_impl_ids = ctx.trait_impl_ids_for_trait(trait_def_id);
    let mut applicable = Vec::new();

    for impl_id in trait_impl_ids {
        {
            let mut resolver = TypeResolver::new(ctx);
            resolver.ensure_impl_signature_types_resolved(impl_id);
        }

        let Some(impl_args) =
            resolve_trait_impl_head_obligation(ctx, receiver_ty, trait_def_id, trait_args, impl_id)
        else {
            continue;
        };

        applicable.push(ApplicableTraitImplHeadCandidate { impl_id, impl_args });
    }

    // Keep every undominated candidate. Valid coherent code should usually leave a single impl,
    // but retaining incomparable survivors lets callers distinguish "no proof" from "proof would
    // be ambiguous if we kept going".
    applicable
        .iter()
        .enumerate()
        .filter(|(index, candidate)| {
            !applicable.iter().enumerate().any(|(other_index, other)| {
                other_index != *index
                    && matches!(
                        compare_impl_specificity(ctx, other.impl_id, candidate.impl_id),
                        ImplSpecificity::LeftMoreSpecific
                    )
            })
        })
        .map(|(_, candidate)| candidate.clone())
        .collect()
}

pub(crate) fn augment_trait_object_assoc_bindings_from_map(
    ctx: &mut SemaContext<'_>,
    trait_ty: TypeId,
    assoc_binding_map: &FastHashMap<DefId, TypeId>,
) -> TypeId {
    let trait_ty = ctx.type_registry.normalize(trait_ty);
    let TypeKind::TraitObject(trait_def_id, trait_args, assoc_bindings) =
        ctx.type_registry.get(trait_ty).clone()
    else {
        return trait_ty;
    };

    let mut merged = assoc_bindings.into_iter().collect::<FastHashMap<_, _>>();
    // Internal supertrait traversal must keep inherited bindings alive even on
    // intermediate traits that declare no assoc types of their own. Otherwise
    // multi-hop chains like `Leaf -> Mid -> Base` lose `Base::Assoc` before the
    // next recursive step can see it.
    //
    // However, bindings already written on the current trait view are more specific than the
    // inherited fallback map. Traversal should therefore fill missing assoc equalities, not
    // overwrite explicit ones on the edge we are currently following.
    for (&assoc_id, &assoc_ty) in assoc_binding_map {
        merged.entry(assoc_id).or_insert(assoc_ty);
    }

    let mut merged = merged.into_iter().collect::<Vec<_>>();
    merged.sort_by_key(|(assoc_id, _)| assoc_id.0);
    ctx.type_registry
        .intern(TypeKind::TraitObject(trait_def_id, trait_args, merged))
}

pub(crate) fn enrich_trait_object_assoc_bindings(
    ctx: &mut SemaContext<'_>,
    receiver_ty: TypeId,
    trait_ty: TypeId,
) -> TypeId {
    let trait_ty = ctx.type_registry.normalize(trait_ty);
    let TypeKind::TraitObject(trait_def_id, trait_args, assoc_bindings) =
        ctx.type_registry.get(trait_ty).clone()
    else {
        return trait_ty;
    };

    let mut assoc_binding_map = assoc_bindings.into_iter().collect::<FastHashMap<_, _>>();
    let head_ty = ctx.type_registry.intern(TypeKind::TraitObject(
        trait_def_id,
        trait_args.clone(),
        Vec::new(),
    ));
    let mut visited = FastHashSet::default();
    collect_trait_hierarchy_assoc_bindings(
        ctx,
        receiver_ty,
        head_ty,
        &mut assoc_binding_map,
        &mut visited,
    );

    let mut merged = assoc_binding_map.into_iter().collect::<Vec<_>>();
    merged.sort_by_key(|(assoc_id, _)| assoc_id.0);
    ctx.type_registry
        .intern(TypeKind::TraitObject(trait_def_id, trait_args, merged))
}

pub fn retain_declared_trait_object_assoc_bindings(
    ctx: &mut SemaContext<'_>,
    trait_ty: TypeId,
) -> TypeId {
    let trait_ty = ctx.type_registry.normalize(trait_ty);
    let TypeKind::TraitObject(trait_def_id, trait_args, assoc_bindings) =
        ctx.type_registry.get(trait_ty).clone()
    else {
        return trait_ty;
    };

    if assoc_bindings.is_empty() {
        return trait_ty;
    }

    let Some(Def::Trait(trait_def)) = ctx.defs.get(trait_def_id.0 as usize).cloned() else {
        return trait_ty;
    };
    let declared_assoc = trait_def
        .assoc_types
        .into_iter()
        .collect::<FastHashSet<_>>();
    let filtered = assoc_bindings
        .into_iter()
        .filter(|(assoc_def_id, _)| declared_assoc.contains(assoc_def_id))
        .collect::<Vec<_>>();

    if filtered.is_empty() {
        return ctx.type_registry.intern(TypeKind::TraitObject(
            trait_def_id,
            trait_args,
            Vec::new(),
        ));
    }

    ctx.type_registry
        .intern(TypeKind::TraitObject(trait_def_id, trait_args, filtered))
}

pub fn declared_trait_object_view_from_hierarchy(
    ctx: &mut SemaContext<'_>,
    trait_ty: TypeId,
    target_trait_def_id: DefId,
    target_trait_args: &[crate::ty::GenericArg],
) -> Option<TypeId> {
    let trait_view =
        trait_object_view_from_hierarchy(ctx, trait_ty, target_trait_def_id, target_trait_args)?;
    Some(retain_declared_trait_object_assoc_bindings(ctx, trait_view))
}

pub fn trait_object_view_from_hierarchy(
    ctx: &mut SemaContext<'_>,
    trait_ty: TypeId,
    target_trait_def_id: DefId,
    target_trait_args: &[crate::ty::GenericArg],
) -> Option<TypeId> {
    let trait_ty = match ctx
        .type_registry
        .get(ctx.type_registry.normalize(trait_ty))
        .clone()
    {
        TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. }
            if matches!(
                ctx.type_registry.get(ctx.type_registry.normalize(elem)),
                TypeKind::TraitObject(..)
            ) =>
        {
            elem
        }
        _ => trait_ty,
    };
    let mut visited = FastHashSet::default();
    trait_object_view_from_hierarchy_inner(
        ctx,
        trait_ty,
        target_trait_def_id,
        target_trait_args,
        &mut visited,
    )
}

pub(crate) fn trait_object_assoc_from_hierarchy(
    ctx: &mut SemaContext<'_>,
    trait_ty: TypeId,
    target_trait_def_id: DefId,
    target_trait_args: &[crate::ty::GenericArg],
    assoc_def_id: DefId,
) -> Option<TypeId> {
    let trait_view =
        trait_object_view_from_hierarchy(ctx, trait_ty, target_trait_def_id, target_trait_args)?;
    let TypeKind::TraitObject(_, _, assoc_bindings) = ctx.type_registry.get(trait_view).clone()
    else {
        return None;
    };
    assoc_bindings
        .into_iter()
        .find(|(bound_assoc_id, _)| *bound_assoc_id == assoc_def_id)
        .map(|(_, assoc_ty)| assoc_ty)
}

fn trait_object_view_from_hierarchy_inner(
    ctx: &mut SemaContext<'_>,
    trait_ty: TypeId,
    target_trait_def_id: DefId,
    target_trait_args: &[crate::ty::GenericArg],
    visited: &mut FastHashSet<TypeId>,
) -> Option<TypeId> {
    let trait_ty = ctx.type_registry.normalize(trait_ty);
    if !visited.insert(trait_ty) {
        return None;
    }

    let TypeKind::TraitObject(trait_def_id, trait_args, assoc_bindings) =
        ctx.type_registry.get(trait_ty).clone()
    else {
        return None;
    };

    if trait_def_id == target_trait_def_id && trait_args == target_trait_args {
        return Some(trait_ty);
    }

    let Some(Def::Trait(trait_def)) = ctx.defs.get(trait_def_id.0 as usize).cloned() else {
        return None;
    };
    let trait_arg_map = trait_def
        .generics
        .iter()
        .zip(trait_args.iter())
        .map(|(param, arg)| (param.name, *arg))
        .collect::<FastHashMap<_, _>>();
    let assoc_binding_map = assoc_bindings.into_iter().collect::<FastHashMap<_, _>>();
    let mut found_view = None;

    for super_ty in trait_def.resolved_supertraits {
        let substituted = if trait_arg_map.is_empty() {
            super_ty
        } else {
            let mut subst = Substituter::new(&mut ctx.type_registry, &trait_arg_map);
            subst.substitute(super_ty)
        };
        let substituted = crate::ty::substitute_associated_types(
            &mut ctx.type_registry,
            &ctx.defs,
            substituted,
            &assoc_binding_map,
        );
        // This walk returns the first matching target view. That is only sound
        // because trait objects reaching this point must already come from a
        // coherent impl proof: two distinct paths to the same `(trait, args)`
        // head are expected to agree on every surviving associated binding.
        //
        // Historical sema bugs have violated that invariant before. Refuse to
        // pick one path silently if two inherited views disagree on the target
        // trait's declared assoc bindings.
        let enriched =
            augment_trait_object_assoc_bindings_from_map(ctx, substituted, &assoc_binding_map);
        let enriched = ctx.normalize_concrete_type(enriched);
        let enriched = ctx.type_registry.normalize(enriched);
        if let Some(found) = trait_object_view_from_hierarchy_inner(
            ctx,
            enriched,
            target_trait_def_id,
            target_trait_args,
            visited,
        ) {
            if let Some(existing) = found_view {
                if !target_trait_views_equivalent(ctx, existing, found) {
                    return None;
                }
            } else {
                found_view = Some(found);
            }
        }
    }

    found_view
}

fn target_trait_views_equivalent(ctx: &mut SemaContext<'_>, left: TypeId, right: TypeId) -> bool {
    let left = retain_declared_trait_object_assoc_bindings(ctx, left);
    let right = retain_declared_trait_object_assoc_bindings(ctx, right);
    let left = ctx.normalize_concrete_type(left);
    let left = ctx.type_registry.normalize(left);
    let right = ctx.normalize_concrete_type(right);
    let right = ctx.type_registry.normalize(right);
    left == right
}

fn collect_trait_hierarchy_assoc_bindings(
    ctx: &mut SemaContext<'_>,
    receiver_ty: TypeId,
    trait_ty: TypeId,
    assoc_binding_map: &mut FastHashMap<DefId, TypeId>,
    visited: &mut FastHashSet<TypeId>,
) {
    let trait_ty = ctx.normalize_concrete_type(trait_ty);
    let trait_ty = ctx.type_registry.normalize(trait_ty);
    if !visited.insert(trait_ty) {
        return;
    }

    let TypeKind::TraitObject(trait_def_id, trait_args, _) =
        ctx.type_registry.get(trait_ty).clone()
    else {
        return;
    };

    if let Some((impl_id, impl_args)) =
        select_most_specific_trait_impl_head(ctx, receiver_ty, trait_def_id, &trait_args)
        && let Some(inst_trait_ty) = instantiate_impl_trait_ty(ctx, impl_id, &impl_args)
        && let TypeKind::TraitObject(_, _, impl_assoc_bindings) =
            ctx.type_registry.get(inst_trait_ty).clone()
    {
        // The receiver may already carry explicit assoc equalities that are more specific than
        // whatever the selected impl would infer for the same head. Enrichment should therefore
        // complete missing bindings, not overwrite the ones written on the current trait view.
        for (assoc_id, assoc_ty) in impl_assoc_bindings {
            assoc_binding_map.entry(assoc_id).or_insert(assoc_ty);
        }
    }

    let Some(Def::Trait(trait_def)) = ctx.defs.get(trait_def_id.0 as usize).cloned() else {
        return;
    };
    let trait_arg_map = trait_def
        .generics
        .iter()
        .zip(trait_args.iter())
        .map(|(param, arg)| (param.name, *arg))
        .collect::<FastHashMap<_, _>>();

    for super_ty in trait_def.resolved_supertraits {
        let substituted = if trait_arg_map.is_empty() {
            super_ty
        } else {
            let mut subst = Substituter::new(&mut ctx.type_registry, &trait_arg_map);
            subst.substitute(super_ty)
        };
        let substituted = crate::ty::substitute_associated_types(
            &mut ctx.type_registry,
            &ctx.defs,
            substituted,
            assoc_binding_map,
        );
        let enriched =
            augment_trait_object_assoc_bindings_from_map(ctx, substituted, assoc_binding_map);
        let enriched = ctx.normalize_concrete_type(enriched);
        let enriched = ctx.type_registry.normalize(enriched);
        collect_trait_hierarchy_assoc_bindings(
            ctx,
            receiver_ty,
            enriched,
            assoc_binding_map,
            visited,
        );
    }
}

fn resolve_trait_impl_obligation_inner(
    ctx: &mut SemaContext<'_>,
    receiver_ty: TypeId,
    target_trait_ty: TypeId,
    impl_id: DefId,
    ignore_target_assoc_bindings: bool,
) -> Option<Vec<crate::ty::GenericArg>> {
    let receiver_norm = ctx.type_registry.normalize(receiver_ty);
    let target_trait_norm = ctx.type_registry.normalize(target_trait_ty);

    let mut checker = ExprChecker::new(ctx, None);
    let impl_ptr = checker
        .ctx
        .defs
        .get(impl_id.0 as usize)
        .and_then(|def| match def {
            Def::Impl(impl_def) => Some(std::ptr::from_ref(impl_def)),
            _ => None,
        })?;

    // SAFETY: semantic definition storage is stable during this obligation
    // query.  A raw pointer avoids holding an immutable borrow of `defs` while
    // `ExprChecker` mutates inference and query caches through `ctx`.
    let impl_def = unsafe { &*impl_ptr };
    let Some(impl_trait_node) = &impl_def.trait_type else {
        return None;
    };

    let impl_target_ty = checker.ctx.node_type_or_error(impl_def.target_type.id);
    let impl_trait_ty = checker.ctx.node_type_or_error(impl_trait_node.id);

    if impl_target_ty == TypeId::ERROR || impl_trait_ty == TypeId::ERROR {
        return None;
    }

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
        return None;
    }

    let mut type_map = FastHashMap::default();
    let mut const_map = FastHashMap::default();
    // `impl Trait for Type { type Assoc = ...; }` satisfies a plain `Type: Trait`
    // obligation even though the instantiated impl trait carries concrete
    // associated-type bindings internally. Match the trait head first, then
    // verify only the bindings explicitly requested by the obligation.
    let impl_trait_head_ty = erase_trait_assoc_bindings(checker.ctx, impl_trait_ty);
    let target_trait_head_ty = erase_trait_assoc_bindings(checker.ctx, target_trait_norm);
    if !checker.match_available_type_against_requirement(
        impl_target_ty,
        receiver_norm,
        &mut type_map,
        &mut const_map,
    ) || !checker.match_available_type_against_requirement(
        impl_trait_head_ty,
        target_trait_head_ty,
        &mut type_map,
        &mut const_map,
    ) || !impl_bounds_satisfied(&mut checker, &impl_def.where_clauses, &type_map, &const_map)
    {
        return None;
    }

    let resolved_args: Vec<_> = impl_def
        .generics
        .iter()
        .map(|param| match &param.kind {
            ast::GenericParamKind::Type => crate::ty::GenericArg::Type(
                type_map.get(&param.name).copied().unwrap_or(TypeId::ERROR),
            ),
            ast::GenericParamKind::Const { .. } => crate::ty::GenericArg::Const(
                const_map
                    .get(&param.name)
                    .copied()
                    .unwrap_or(crate::ty::ConstGeneric::Error),
            ),
        })
        .collect();

    // A proof candidate is only usable once every impl generic has been determined from the head
    // match and any validated where-clause obligations. Leaving an impl-local generic as
    // `ERROR`/`ConstGeneric::Error` would let unconstrained impl parameters masquerade as a real
    // proof.
    if !impl_generic_args_fully_resolved(&resolved_args) {
        return None;
    }

    if ignore_target_assoc_bindings {
        return Some(resolved_args);
    }

    let TypeKind::TraitObject(_, _, target_assoc_bindings) =
        checker.ctx.type_registry.get(target_trait_norm).clone()
    else {
        return None;
    };

    if !target_assoc_bindings.is_empty() {
        let instantiated_impl_trait_ty =
            instantiate_impl_trait_ty(checker.ctx, impl_id, &resolved_args)?;
        // Obligations can mention associated types inherited from supertraits.
        // Validate against the receiver's full resolved trait hierarchy, not
        // just the direct assoc bindings written on the impl head itself.
        let instantiated_impl_trait_ty = enrich_trait_object_assoc_bindings(
            checker.ctx,
            receiver_norm,
            instantiated_impl_trait_ty,
        );
        let TypeKind::TraitObject(_, _, impl_assoc_bindings) = checker
            .ctx
            .type_registry
            .get(instantiated_impl_trait_ty)
            .clone()
        else {
            return None;
        };
        let impl_assoc_bindings = impl_assoc_bindings
            .into_iter()
            .collect::<FastHashMap<_, _>>();

        for (assoc_def_id, target_assoc_ty) in target_assoc_bindings {
            let &impl_assoc_ty = impl_assoc_bindings.get(&assoc_def_id)?;
            if !checker.match_available_type_against_requirement(
                impl_assoc_ty,
                target_assoc_ty,
                &mut type_map,
                &mut const_map,
            ) {
                return None;
            }
        }
    }

    Some(resolved_args)
}

fn impl_head_signature(
    ctx: &mut SemaContext<'_>,
    impl_id: DefId,
) -> Option<(ImplDef, TypeId, Option<TypeId>)> {
    let impl_def = ctx.defs.get(impl_id.0 as usize).and_then(|def| match def {
        Def::Impl(impl_def) => Some(impl_def.clone()),
        _ => None,
    })?;
    let target_ty = ctx.node_type_or_error(impl_def.target_type.id);
    let trait_ty = impl_def
        .trait_type
        .as_ref()
        .and_then(|trait_ty| ctx.node_type(trait_ty.id));
    let trait_ty = trait_ty.map(|trait_ty| erase_trait_assoc_bindings(ctx, trait_ty));

    if target_ty == TypeId::ERROR || matches!(trait_ty, Some(TypeId::ERROR)) {
        return None;
    }

    Some((impl_def, target_ty, trait_ty))
}

pub(crate) fn impl_generic_args_fully_resolved(args: &[crate::ty::GenericArg]) -> bool {
    args.iter().all(|arg| match *arg {
        crate::ty::GenericArg::Type(ty) => ty != TypeId::ERROR,
        crate::ty::GenericArg::Const(value) => value != crate::ty::ConstGeneric::Error,
    })
}

pub(crate) fn erase_trait_assoc_bindings(ctx: &mut SemaContext<'_>, ty: TypeId) -> TypeId {
    match ctx
        .type_registry
        .get(ctx.type_registry.normalize(ty))
        .clone()
    {
        TypeKind::TraitObject(def_id, args, _) => {
            ctx.type_registry
                .intern(TypeKind::TraitObject(def_id, args, Vec::new()))
        }
        _ => ty,
    }
}

struct FreshImplHead {
    target_ty: TypeId,
    trait_ty: Option<TypeId>,
    subst_map: FastHashMap<SymbolId, crate::ty::GenericArg>,
}

fn freshen_impl_head_types(
    checker: &mut ExprChecker<'_, '_>,
    impl_def: &ImplDef,
    target_ty: TypeId,
    trait_ty: Option<TypeId>,
    freshness: ImplHeadFreshness,
) -> FreshImplHead {
    let mut subst_map = FastHashMap::default();

    for (index, param) in impl_def.generics.iter().enumerate() {
        let fresh_name = checker.ctx.intern(&format!(
            "__impl_specialization_{}_{}_{}",
            impl_def.id.0,
            index,
            checker.ctx.resolve(param.name)
        ));
        let fresh_arg = match &param.kind {
            ast::GenericParamKind::Type => match freshness {
                ImplHeadFreshness::Flexible => {
                    crate::ty::GenericArg::Type(checker.fresh_type_var())
                }
                ImplHeadFreshness::Rigid => crate::ty::GenericArg::Type(
                    checker
                        .ctx
                        .type_registry
                        .intern(TypeKind::Param(fresh_name)),
                ),
            },
            ast::GenericParamKind::Const { ty } => {
                let const_ty = checker.ctx.node_type_or_error(ty.id);
                crate::ty::GenericArg::Const(crate::ty::ConstGeneric::Param(fresh_name, const_ty))
            }
        };
        subst_map.insert(param.name, fresh_arg);
    }

    let mut subst = Substituter::new(&mut checker.ctx.type_registry, &subst_map);
    FreshImplHead {
        target_ty: subst.substitute(target_ty),
        trait_ty: trait_ty.map(|trait_ty| subst.substitute(trait_ty)),
        subst_map,
    }
}

fn instantiate_where_requirements(
    checker: &mut ExprChecker<'_, '_>,
    impl_def: &ImplDef,
    subst_map: &FastHashMap<SymbolId, crate::ty::GenericArg>,
) -> Vec<(TypeId, TypeId)> {
    let mut requirements = Vec::new();
    for clause in &impl_def.where_clauses {
        let original_target = checker.ctx.node_type_or_error(clause.target_ty.id);
        let target_ty = {
            let mut subst = Substituter::new(&mut checker.ctx.type_registry, subst_map);
            let substituted = subst.substitute(original_target);
            checker.resolve_tv(substituted)
        };

        for bound in &clause.bounds {
            let original_bound = checker.ctx.node_type_or_error(bound.id);
            let trait_ty = {
                let mut subst = Substituter::new(&mut checker.ctx.type_registry, subst_map);
                let substituted = subst.substitute(original_bound);
                checker.resolve_tv(substituted)
            };
            if target_ty != TypeId::ERROR
                && trait_ty != TypeId::ERROR
                && matches!(
                    checker.ctx.type_registry.get(trait_ty),
                    TypeKind::TraitObject(..)
                )
            {
                let requirement = (target_ty, trait_ty);
                if !requirements.contains(&requirement) {
                    requirements.push(requirement);
                }
            }
        }
    }
    requirements
}

#[derive(Clone, Copy)]
enum ImplHeadFreshness {
    Flexible,
    Rigid,
}

pub(crate) fn impl_bounds_satisfied(
    checker: &mut ExprChecker<'_, '_>,
    where_clauses: &[ast::WhereClause],
    type_map: &FastHashMap<SymbolId, TypeId>,
    const_map: &FastHashMap<SymbolId, crate::ty::ConstGeneric>,
) -> bool {
    let mut pairs_to_check = Vec::new();

    for clause in where_clauses {
        let original_target = checker.ctx.node_type_or_error(clause.target_ty.id);
        let sub_target =
            checker.substitute_type_with_unification_maps(original_target, type_map, const_map);

        for bound_ast in &clause.bounds {
            let original_bound = checker.ctx.node_type_or_error(bound_ast.id);
            let sub_bound =
                checker.substitute_type_with_unification_maps(original_bound, type_map, const_map);
            pairs_to_check.push((sub_target, sub_bound));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def::{AssociatedTypeDef, Def, DefId, FunctionDef, ImplDef, TraitDef};
    use kernc_ast::Visibility;
    use kernc_utils::Session;

    #[test]
    fn augment_trait_object_assoc_bindings_preserves_local_binding() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_id, assoc_id) = add_trait_with_assoc(&mut ctx, "Base", "Out");

        let local_trait_ty = ctx.type_registry.intern(TypeKind::TraitObject(
            trait_id,
            vec![],
            vec![(assoc_id, TypeId::I64)],
        ));
        let inherited = FastHashMap::from_iter([(assoc_id, TypeId::I32)]);

        let augmented =
            augment_trait_object_assoc_bindings_from_map(&mut ctx, local_trait_ty, &inherited);

        let TypeKind::TraitObject(_, _, assoc_bindings) = ctx.type_registry.get(augmented).clone()
        else {
            panic!("expected trait object");
        };
        assert_eq!(assoc_bindings, vec![(assoc_id, TypeId::I64)]);
    }

    #[test]
    fn augment_trait_object_assoc_bindings_adds_missing_binding() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_id, assoc_id) = add_trait_with_assoc(&mut ctx, "Base", "Out");

        let bare_trait_ty =
            ctx.type_registry
                .intern(TypeKind::TraitObject(trait_id, vec![], Vec::new()));
        let inherited = FastHashMap::from_iter([(assoc_id, TypeId::I32)]);

        let augmented =
            augment_trait_object_assoc_bindings_from_map(&mut ctx, bare_trait_ty, &inherited);

        let TypeKind::TraitObject(_, _, assoc_bindings) = ctx.type_registry.get(augmented).clone()
        else {
            panic!("expected trait object");
        };
        assert_eq!(assoc_bindings, vec![(assoc_id, TypeId::I32)]);
    }

    #[test]
    fn bound_trait_lookup_instantiates_matched_env_target() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_id, assoc_id) = add_trait_with_assoc(&mut ctx, "Base", "Out");
        let wrap_id = DefId(400);
        let param_t = ctx.intern("T");
        let param_ty = ctx.type_registry.intern(TypeKind::Param(param_t));

        let env_target = ctx.type_registry.intern(TypeKind::Def(
            wrap_id,
            vec![crate::ty::GenericArg::Type(param_ty)],
        ));
        let env_bound = ctx.type_registry.intern(TypeKind::TraitObject(
            trait_id,
            vec![],
            vec![(assoc_id, param_ty)],
        ));
        let search_ty = ctx.type_registry.intern(TypeKind::Def(
            wrap_id,
            vec![crate::ty::GenericArg::Type(TypeId::I32)],
        ));
        let env = MemberQueryEnv::from_active_bounds_owned(&[(env_target, vec![env_bound])]);

        let mut query = MemberQuery::new(&mut ctx);
        let mut matches = Vec::new();
        query.for_each_matching_bound_trait_object(search_ty, &env, |_, bound_norm| {
            matches.push(bound_norm);
            false
        });

        assert_eq!(matches.len(), 1);
        let TypeKind::TraitObject(_, _, assoc_bindings) =
            query.context().type_registry.get(matches[0]).clone()
        else {
            panic!("expected trait object");
        };
        assert_eq!(assoc_bindings, vec![(assoc_id, TypeId::I32)]);
    }

    #[test]
    fn trait_object_view_from_hierarchy_keeps_assoc_binding_through_coherent_diamond() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (base_id, assoc_id) = add_trait_with_assoc(&mut ctx, "Base", "Out");
        let left_id = add_trait(&mut ctx, "Left");
        let right_id = add_trait(&mut ctx, "Right");
        let leaf_id = add_trait(&mut ctx, "Leaf");

        let base_i64 = ctx.type_registry.intern(TypeKind::TraitObject(
            base_id,
            vec![],
            vec![(assoc_id, TypeId::I64)],
        ));
        let bare_left =
            ctx.type_registry
                .intern(TypeKind::TraitObject(left_id, vec![], Vec::new()));
        let bare_right =
            ctx.type_registry
                .intern(TypeKind::TraitObject(right_id, vec![], Vec::new()));

        set_resolved_supertraits(&mut ctx, left_id, vec![base_i64]);
        set_resolved_supertraits(&mut ctx, right_id, vec![base_i64]);
        set_resolved_supertraits(&mut ctx, leaf_id, vec![bare_left, bare_right]);

        let leaf_ty = ctx
            .type_registry
            .intern(TypeKind::TraitObject(leaf_id, vec![], Vec::new()));
        let found = trait_object_view_from_hierarchy(&mut ctx, leaf_ty, base_id, &[])
            .expect("expected Base view through diamond");

        let TypeKind::TraitObject(found_def_id, found_args, assoc_bindings) =
            ctx.type_registry.get(found).clone()
        else {
            panic!("expected trait object");
        };
        assert_eq!(found_def_id, base_id);
        assert!(found_args.is_empty());
        assert_eq!(assoc_bindings, vec![(assoc_id, TypeId::I64)]);
    }

    #[test]
    fn trait_object_view_from_hierarchy_rejects_conflicting_assoc_binding_diamond() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (base_id, assoc_id) = add_trait_with_assoc(&mut ctx, "Base", "Out");
        let left_id = add_trait(&mut ctx, "Left");
        let right_id = add_trait(&mut ctx, "Right");
        let leaf_id = add_trait(&mut ctx, "Leaf");

        let base_i32 = ctx.type_registry.intern(TypeKind::TraitObject(
            base_id,
            vec![],
            vec![(assoc_id, TypeId::I32)],
        ));
        let base_bool = ctx.type_registry.intern(TypeKind::TraitObject(
            base_id,
            vec![],
            vec![(assoc_id, TypeId::BOOL)],
        ));
        let bare_left =
            ctx.type_registry
                .intern(TypeKind::TraitObject(left_id, vec![], Vec::new()));
        let bare_right =
            ctx.type_registry
                .intern(TypeKind::TraitObject(right_id, vec![], Vec::new()));

        set_resolved_supertraits(&mut ctx, left_id, vec![base_i32]);
        set_resolved_supertraits(&mut ctx, right_id, vec![base_bool]);
        set_resolved_supertraits(&mut ctx, leaf_id, vec![bare_left, bare_right]);

        let leaf_ty = ctx
            .type_registry
            .intern(TypeKind::TraitObject(leaf_id, vec![], Vec::new()));

        assert!(trait_object_view_from_hierarchy(&mut ctx, leaf_ty, base_id, &[]).is_none());
        assert!(
            trait_object_assoc_from_hierarchy(&mut ctx, leaf_ty, base_id, &[], assoc_id).is_none()
        );
    }

    #[test]
    fn declared_trait_object_view_drops_inherited_assoc_bindings() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (base_id, base_assoc_id) = add_trait_with_assoc(&mut ctx, "Base", "Out");
        let (derived_id, derived_assoc_id) = add_trait_with_assoc(&mut ctx, "Derived", "Item");

        let bare_base =
            ctx.type_registry
                .intern(TypeKind::TraitObject(base_id, vec![], Vec::new()));
        set_resolved_supertraits(&mut ctx, derived_id, vec![bare_base]);

        let derived_ty = ctx.type_registry.intern(TypeKind::TraitObject(
            derived_id,
            vec![],
            vec![
                (base_assoc_id, TypeId::I32),
                (derived_assoc_id, TypeId::I64),
            ],
        ));
        let declared =
            declared_trait_object_view_from_hierarchy(&mut ctx, derived_ty, derived_id, &[])
                .expect("expected declared Derived view");

        let TypeKind::TraitObject(found_def_id, found_args, assoc_bindings) =
            ctx.type_registry.get(declared).clone()
        else {
            panic!("expected trait object");
        };
        assert_eq!(found_def_id, derived_id);
        assert!(found_args.is_empty());
        assert_eq!(assoc_bindings, vec![(derived_assoc_id, TypeId::I64)]);
    }

    #[test]
    fn select_most_specific_trait_impl_head_rejects_ambiguous_overlap() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let trait_id = add_trait(&mut ctx, "Ambiguous");
        let param_t = ctx.intern("T");
        let generic_target_ty = ctx.type_registry.intern(TypeKind::Param(param_t));
        let generics = [kernc_ast::GenericParam {
            name: param_t,
            span: Span::default(),
            kind: kernc_ast::GenericParamKind::Type,
        }];

        add_trait_impl(&mut ctx, &generics, generic_target_ty, trait_id, Vec::new());
        add_trait_impl(&mut ctx, &generics, generic_target_ty, trait_id, Vec::new());

        assert_eq!(
            select_most_specific_trait_impl_head(&mut ctx, TypeId::I32, trait_id, &[]),
            None
        );
    }

    #[test]
    fn select_most_specific_trait_impl_head_prefers_concrete_const_arg() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_id, assoc_id) = add_trait_with_assoc(&mut ctx, "Base", "Out");
        let param_n = ctx.intern("N");
        let n_param = crate::ty::ConstGeneric::Param(param_n, TypeId::USIZE);
        let four = crate::ty::ConstGeneric::Value(crate::ty::ConstGenericValue {
            ty: TypeId::USIZE,
            kind: crate::ty::ConstGenericValueKind::Int(4),
        });
        let n_ty_node_id = ctx.next_node_id();
        ctx.set_node_type(n_ty_node_id, TypeId::USIZE);
        let generics = [kernc_ast::GenericParam {
            name: param_n,
            span: Span::default(),
            kind: kernc_ast::GenericParamKind::Const {
                ty: kernc_ast::TypeNode {
                    id: n_ty_node_id,
                    kind: kernc_ast::TypeKind::Infer,
                    span: Span::default(),
                },
            },
        }];

        let concrete_impl = add_trait_impl_with_args(
            &mut ctx,
            &[],
            TypeId::I32,
            trait_id,
            vec![crate::ty::GenericArg::Const(four)],
            vec![(assoc_id, TypeId::I64)],
        );
        add_trait_impl_with_args(
            &mut ctx,
            &generics,
            TypeId::I32,
            trait_id,
            vec![crate::ty::GenericArg::Const(n_param)],
            vec![(assoc_id, TypeId::BOOL)],
        );

        assert_eq!(
            select_most_specific_trait_impl_head(
                &mut ctx,
                TypeId::I32,
                trait_id,
                &[crate::ty::GenericArg::Const(four)]
            ),
            Some((concrete_impl, Vec::new()))
        );
    }

    #[test]
    fn enrich_trait_object_assoc_bindings_skips_ambiguous_impl_heads() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_id, assoc_id) = add_trait_with_assoc(&mut ctx, "Base", "Out");
        let param_t = ctx.intern("T");
        let generic_target_ty = ctx.type_registry.intern(TypeKind::Param(param_t));
        let generics = [kernc_ast::GenericParam {
            name: param_t,
            span: Span::default(),
            kind: kernc_ast::GenericParamKind::Type,
        }];

        add_trait_impl(
            &mut ctx,
            &generics,
            generic_target_ty,
            trait_id,
            vec![(assoc_id, TypeId::I32)],
        );
        add_trait_impl(
            &mut ctx,
            &generics,
            generic_target_ty,
            trait_id,
            vec![(assoc_id, TypeId::BOOL)],
        );

        let bare_trait_ty =
            ctx.type_registry
                .intern(TypeKind::TraitObject(trait_id, vec![], Vec::new()));
        let enriched = enrich_trait_object_assoc_bindings(&mut ctx, TypeId::I32, bare_trait_ty);

        let TypeKind::TraitObject(_, _, assoc_bindings) = ctx.type_registry.get(enriched).clone()
        else {
            panic!("expected trait object");
        };
        assert!(assoc_bindings.is_empty());
    }

    #[test]
    fn enrich_trait_object_assoc_bindings_preserves_explicit_head_binding() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_id, assoc_id) = add_trait_with_assoc(&mut ctx, "Base", "Out");

        add_trait_impl(
            &mut ctx,
            &[],
            TypeId::I32,
            trait_id,
            vec![(assoc_id, TypeId::BOOL)],
        );

        let explicit_trait_ty = ctx.type_registry.intern(TypeKind::TraitObject(
            trait_id,
            vec![],
            vec![(assoc_id, TypeId::I64)],
        ));
        let enriched = enrich_trait_object_assoc_bindings(&mut ctx, TypeId::I32, explicit_trait_ty);

        let TypeKind::TraitObject(_, _, assoc_bindings) = ctx.type_registry.get(enriched).clone()
        else {
            panic!("expected trait object");
        };
        assert_eq!(assoc_bindings, vec![(assoc_id, TypeId::I64)]);
    }

    #[test]
    fn ambiguous_impl_method_reports_clear_diagnostic() {
        let mut session = Session::new();
        let resolution_type = {
            let mut ctx = SemaContext::new(&mut session);
            let method_name = ctx.intern("run");
            let param_t = ctx.intern("T");
            let generic_target_ty = ctx.type_registry.intern(TypeKind::Param(param_t));
            let generics = [kernc_ast::GenericParam {
                name: param_t,
                span: Span::default(),
                kind: kernc_ast::GenericParamKind::Type,
            }];

            add_inherent_impl_with_method(&mut ctx, &generics, generic_target_ty, method_name);
            add_inherent_impl_with_method(&mut ctx, &generics, generic_target_ty, method_name);

            let mut query = MemberQuery::new(&mut ctx);
            let resolution = query.resolve_named_member(
                None,
                TypeId::I32,
                method_name,
                &MemberQueryEnv::default(),
                Span::default(),
            );

            resolution
                .expect("expected ambiguity candidate")
                .candidate
                .type_id
        };

        assert_eq!(resolution_type, TypeId::ERROR);

        let diag = session.diagnostics.last().expect("expected diagnostic");
        assert_eq!(diag.message, "ambiguous impl method `run`");
        assert!(
            diag.hints
                .iter()
                .any(|hint| hint.contains("equally specific impl methods"))
        );
        assert!(
            diag.hints
                .iter()
                .any(|hint| hint.contains("conflicting impl heads"))
        );
    }

    #[test]
    fn ambiguous_builtin_eq_method_suggests_operator_syntax() {
        let mut session = Session::new();
        let resolution_type = {
            let mut ctx = SemaContext::new(&mut session);
            crate::BuiltinInjector::new(&mut ctx).inject();
            let eq_trait_id = ctx.builtin_def("Eq").expect("expected builtin Eq trait");
            let method_name = ctx.intern("eq");

            add_trait_impl_with_method(
                &mut ctx,
                &[],
                TypeId::I32,
                eq_trait_id,
                vec![crate::ty::wrap_type_arg(TypeId::BOOL)],
                method_name,
            );
            add_trait_impl_with_method(
                &mut ctx,
                &[],
                TypeId::I32,
                eq_trait_id,
                vec![crate::ty::wrap_type_arg(TypeId::I64)],
                method_name,
            );

            let mut query = MemberQuery::new(&mut ctx);
            let resolution = query.resolve_named_member(
                None,
                TypeId::I32,
                method_name,
                &MemberQueryEnv::default(),
                Span::default(),
            );

            resolution
                .expect("expected ambiguity candidate")
                .candidate
                .type_id
        };

        assert_eq!(resolution_type, TypeId::ERROR);

        let diag = session.diagnostics.last().expect("expected diagnostic");
        assert_eq!(diag.message, "ambiguous impl method `eq`");
        assert!(diag.hints.iter().any(|hint| {
            hint.contains("builtin operator trait methods still use ordinary member lookup")
        }));
        assert!(
            diag.hints
                .iter()
                .any(|hint| { hint.contains("write `lhs == rhs` instead of `lhs.eq(rhs)`") })
        );
    }

    #[test]
    fn inherited_method_lookup_deduplicates_same_owner_reached_through_richer_views() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (top_id, top_assoc_id) = add_trait_with_assoc(&mut ctx, "Top", "Aux");
        let base_id = add_trait_with_method(&mut ctx, "Base", "get");
        let left_id = add_trait(&mut ctx, "Left");
        let right_id = add_trait(&mut ctx, "Right");
        let leaf_id = add_trait(&mut ctx, "Leaf");

        let bare_top = ctx
            .type_registry
            .intern(TypeKind::TraitObject(top_id, vec![], Vec::new()));
        set_resolved_supertraits(&mut ctx, base_id, vec![bare_top]);

        let base_with_i32_aux = ctx.type_registry.intern(TypeKind::TraitObject(
            base_id,
            vec![],
            vec![(top_assoc_id, TypeId::I32)],
        ));
        let base_with_bool_aux = ctx.type_registry.intern(TypeKind::TraitObject(
            base_id,
            vec![],
            vec![(top_assoc_id, TypeId::BOOL)],
        ));
        let bare_left =
            ctx.type_registry
                .intern(TypeKind::TraitObject(left_id, vec![], Vec::new()));
        let bare_right =
            ctx.type_registry
                .intern(TypeKind::TraitObject(right_id, vec![], Vec::new()));

        set_resolved_supertraits(&mut ctx, left_id, vec![base_with_i32_aux]);
        set_resolved_supertraits(&mut ctx, right_id, vec![base_with_bool_aux]);
        set_resolved_supertraits(&mut ctx, leaf_id, vec![bare_left, bare_right]);

        let leaf_ty = ctx
            .type_registry
            .intern(TypeKind::TraitObject(leaf_id, vec![], Vec::new()));
        let method_name = ctx.intern("get");

        let mut query = MemberQuery::new(&mut ctx);
        let resolution = query.resolve_named_member(
            None,
            leaf_ty,
            method_name,
            &MemberQueryEnv::default(),
            Span::default(),
        );

        assert!(resolution.is_some());
        assert!(session.diagnostics.is_empty());
    }

    fn add_trait(ctx: &mut SemaContext<'_>, trait_name: &str) -> DefId {
        let trait_id_value = ctx.defs.next_id();
        let trait_name = ctx.intern(trait_name);
        ctx.add_def(Def::Trait(TraitDef {
            id: trait_id_value,
            name: trait_name,
            name_span: Span::default(),
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
        }))
    }

    fn set_resolved_supertraits(
        ctx: &mut SemaContext<'_>,
        trait_id: DefId,
        resolved_supertraits: Vec<TypeId>,
    ) {
        let Def::Trait(trait_def) = &mut ctx.defs[trait_id.0 as usize] else {
            panic!("expected trait");
        };
        trait_def.resolved_supertraits = resolved_supertraits;
    }

    fn add_trait_with_method(
        ctx: &mut SemaContext<'_>,
        trait_name: &str,
        method_name: &str,
    ) -> DefId {
        let trait_id = add_trait(ctx, trait_name);
        let method_name = ctx.intern(method_name);
        let method_ty = ctx.type_registry.intern(TypeKind::Function {
            params: Vec::new(),
            ret: TypeId::I32,
            is_variadic: false,
        });
        let type_node_id = ctx.next_node_id();

        let Def::Trait(trait_def) = &mut ctx.defs[trait_id.0 as usize] else {
            panic!("expected trait");
        };
        trait_def.methods.push(crate::def::TraitMethodDef {
            signature: kernc_ast::StructFieldDef {
                name: method_name,
                name_span: Span::default(),
                vis: kernc_ast::Visibility::Private,
                docs: None,
                type_node: kernc_ast::TypeNode {
                    id: type_node_id,
                    kind: kernc_ast::TypeKind::Infer,
                    span: Span::default(),
                },
                default_value: None,
                span: Span::default(),
            },
            params: Vec::new(),
            default_impl: None,
        });
        trait_def.resolved_methods.push((method_name, method_ty));
        trait_id
    }

    fn add_trait_with_assoc(
        ctx: &mut SemaContext<'_>,
        trait_name: &str,
        assoc_name: &str,
    ) -> (DefId, DefId) {
        let trait_id = add_trait(ctx, trait_name);
        let assoc_id_value = ctx.defs.next_id();
        let assoc_name = ctx.intern(assoc_name);
        let assoc_id = ctx.add_def(Def::AssociatedType(AssociatedTypeDef {
            id: assoc_id_value,
            name: assoc_name,
            name_span: Span::default(),
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
        trait_id: DefId,
        assoc_bindings: Vec<(DefId, TypeId)>,
    ) -> DefId {
        add_trait_impl_with_args(
            ctx,
            generics,
            target_ty,
            trait_id,
            Vec::new(),
            assoc_bindings,
        )
    }

    fn add_trait_impl_with_args(
        ctx: &mut SemaContext<'_>,
        generics: &[kernc_ast::GenericParam],
        target_ty: TypeId,
        trait_id: DefId,
        trait_args: Vec<crate::ty::GenericArg>,
        assoc_bindings: Vec<(DefId, TypeId)>,
    ) -> DefId {
        let target_node_id = ctx.next_node_id();
        let trait_node_id = ctx.next_node_id();
        let impl_id_value = ctx.defs.next_id();
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
            resolved_trait_ty: None,
            assoc_types: Vec::new(),
            methods: Vec::new(),
            span: Span::default(),
        }));

        let trait_ty =
            ctx.type_registry
                .intern(TypeKind::TraitObject(trait_id, trait_args, assoc_bindings));
        ctx.set_node_type(target_node_id, target_ty);
        ctx.set_node_type(trait_node_id, trait_ty);
        ctx.register_trait_impl(impl_id);
        impl_id
    }

    fn add_trait_impl_with_method(
        ctx: &mut SemaContext<'_>,
        generics: &[kernc_ast::GenericParam],
        target_ty: TypeId,
        trait_id: DefId,
        trait_args: Vec<crate::ty::GenericArg>,
        method_name: SymbolId,
    ) -> DefId {
        let target_node_id = ctx.next_node_id();
        let trait_node_id = ctx.next_node_id();
        let impl_id_value = ctx.defs.next_id();
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
            resolved_trait_ty: None,
            assoc_types: Vec::new(),
            methods: Vec::new(),
            span: Span::default(),
        }));

        let trait_ty =
            ctx.type_registry
                .intern(TypeKind::TraitObject(trait_id, trait_args, Vec::new()));
        ctx.set_node_type(target_node_id, target_ty);
        ctx.set_node_type(trait_node_id, trait_ty);
        ctx.register_trait_impl(impl_id);

        let method_id_value = ctx.defs.next_id();
        let ret_node_id = ctx.next_node_id();
        let method_id = ctx.add_def(Def::Function(FunctionDef {
            id: method_id_value,
            name: method_name,
            name_span: Span::default(),
            vis: Visibility::Private,
            parent: Some(impl_id),
            default_trait_method: None,
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            params: Vec::new(),
            ret_type: kernc_ast::TypeNode {
                id: ret_node_id,
                kind: kernc_ast::TypeKind::Infer,
                span: Span::default(),
            },
            body: None,
            is_const: false,
            is_extern: false,
            is_variadic: false,
            is_intrinsic: false,
            span: Span::default(),
            resolved_sig: None,
            docs: None,
            attributes: Vec::new(),
        }));

        let Def::Impl(impl_def) = &mut ctx.defs[impl_id.0 as usize] else {
            panic!("expected impl");
        };
        impl_def.methods.push(method_id);
        ctx.register_impl_method(method_name, method_id);
        impl_id
    }

    fn add_inherent_impl_with_method(
        ctx: &mut SemaContext<'_>,
        generics: &[kernc_ast::GenericParam],
        target_ty: TypeId,
        method_name: SymbolId,
    ) -> DefId {
        let target_node_id = ctx.next_node_id();
        let impl_id_value = ctx.defs.next_id();
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
            trait_type: None,
            resolved_trait_ty: None,
            assoc_types: Vec::new(),
            methods: Vec::new(),
            span: Span::default(),
        }));
        ctx.set_node_type(target_node_id, target_ty);
        ctx.register_global_impl(impl_id);

        let method_id_value = ctx.defs.next_id();
        let ret_node_id = ctx.next_node_id();
        let method_id = ctx.add_def(Def::Function(FunctionDef {
            id: method_id_value,
            name: method_name,
            name_span: Span::default(),
            vis: Visibility::Private,
            parent: Some(impl_id),
            default_trait_method: None,
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            params: Vec::new(),
            ret_type: kernc_ast::TypeNode {
                id: ret_node_id,
                kind: kernc_ast::TypeKind::Infer,
                span: Span::default(),
            },
            body: None,
            is_const: false,
            is_extern: false,
            is_variadic: false,
            is_intrinsic: false,
            span: Span::default(),
            resolved_sig: None,
            docs: None,
            attributes: Vec::new(),
        }));

        let Def::Impl(impl_def) = &mut ctx.defs[impl_id.0 as usize] else {
            panic!("expected impl");
        };
        impl_def.methods.push(method_id);
        ctx.register_impl_method(method_name, method_id);
        impl_id
    }
}
