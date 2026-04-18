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

    fn is_current_active_bounds(&self, ctx: &SemaContext<'_>) -> bool {
        let current_bounds = ctx.active_bounds.as_slice();
        matches!(
            &self.active_bounds,
            Cow::Borrowed(bounds)
                if bounds.len() == current_bounds.len()
                    && std::ptr::eq(bounds.as_ptr(), current_bounds.as_ptr())
        )
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
            if let Some(resolution) = self.resolve_named_member_in_type(
                current_module_id,
                search_norm,
                receiver_ty,
                member_name,
                env,
                access_span,
            ) {
                self.cache_member_resolution(cache_key, can_use_cache, &resolution);
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

pub(crate) fn impl_bounds_satisfied(
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
