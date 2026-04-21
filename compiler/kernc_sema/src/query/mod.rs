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
use crate::def::{Def, DefId, ImplDef};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImplSpecificity {
    LeftMoreSpecific,
    RightMoreSpecific,
    Ambiguous,
}

pub struct MemberQuery<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
}

#[derive(Debug, Clone, Copy)]
struct TraitMethodLookup<'a> {
    trait_args: &'a [crate::ty::GenericArg],
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
        _ => ImplSpecificity::Ambiguous,
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
    let (specialized_target, specialized_trait) = freshen_impl_head_types(
        &mut checker,
        &specialized_impl,
        specialized_target_ty,
        specialized_trait_ty,
        ImplHeadFreshness::Rigid,
    );
    let (general_target, general_trait) = freshen_impl_head_types(
        &mut checker,
        &general_impl,
        general_target_ty,
        general_trait_ty,
        ImplHeadFreshness::Flexible,
    );

    let mut type_map = FastHashMap::default();
    let mut const_map = FastHashMap::default();
    checker.match_available_type_against_requirement(
        general_target,
        specialized_target,
        &mut type_map,
        &mut const_map,
    ) && match (general_trait, specialized_trait) {
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
    let impl_def = ctx.defs.get(impl_id.0 as usize).and_then(|def| match def {
        Def::Impl(impl_def) => Some(impl_def.clone()),
        _ => None,
    })?;
    let impl_trait_node = impl_def.trait_type.as_ref()?;
    let impl_trait_ty = ctx
        .node_types
        .get(&impl_trait_node.id)
        .copied()
        .unwrap_or(TypeId::ERROR);
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
    let trait_impl_ids = ctx.trait_impls.clone();
    let mut selected: Option<(DefId, Vec<crate::ty::GenericArg>)> = None;

    for impl_id in trait_impl_ids {
        let Some(impl_args) =
            resolve_trait_impl_head_obligation(ctx, receiver_ty, trait_def_id, trait_args, impl_id)
        else {
            continue;
        };

        let replace = match selected {
            None => true,
            Some((selected_impl_id, _)) => matches!(
                compare_impl_specificity(ctx, impl_id, selected_impl_id),
                ImplSpecificity::LeftMoreSpecific
            ),
        };
        if replace {
            selected = Some((impl_id, impl_args));
        }
    }

    selected
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
    merged.extend(
        assoc_binding_map
            .iter()
            .map(|(assoc_id, assoc_ty)| (*assoc_id, *assoc_ty)),
    );

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
        return ctx
            .type_registry
            .intern(TypeKind::TraitObject(trait_def_id, trait_args, Vec::new()));
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

    for super_ty in trait_def.resolved_supertraits {
        let substituted = if trait_arg_map.is_empty() {
            super_ty
        } else {
            let mut subst = Substituter::new(&mut ctx.type_registry, &trait_arg_map);
            subst.substitute(super_ty)
        };
        let substituted = crate::checker::substitute_associated_types(
            &mut ctx.type_registry,
            substituted,
            &assoc_binding_map,
        );
        let enriched =
            augment_trait_object_assoc_bindings_from_map(ctx, substituted, &assoc_binding_map);
        if let Some(found) = trait_object_view_from_hierarchy_inner(
            ctx,
            enriched,
            target_trait_def_id,
            target_trait_args,
            visited,
        ) {
            return Some(found);
        }
    }

    None
}

fn collect_trait_hierarchy_assoc_bindings(
    ctx: &mut SemaContext<'_>,
    receiver_ty: TypeId,
    trait_ty: TypeId,
    assoc_binding_map: &mut FastHashMap<DefId, TypeId>,
    visited: &mut FastHashSet<TypeId>,
) {
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
        assoc_binding_map.extend(impl_assoc_bindings);
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
        let substituted = crate::checker::substitute_associated_types(
            &mut ctx.type_registry,
            substituted,
            assoc_binding_map,
        );
        let enriched =
            augment_trait_object_assoc_bindings_from_map(ctx, substituted, assoc_binding_map);
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
    let Some(impl_ptr) = checker
        .ctx
        .defs
        .get(impl_id.0 as usize)
        .and_then(|def| match def {
            Def::Impl(impl_def) => Some(std::ptr::from_ref(impl_def)),
            _ => None,
        })
    else {
        return None;
    };

    let impl_def = unsafe { &*impl_ptr };
    let Some(impl_trait_node) = &impl_def.trait_type else {
        return None;
    };

    let impl_target_ty = checker
        .ctx
        .node_types
        .get(&impl_def.target_type.id)
        .copied()
        .unwrap_or(TypeId::ERROR);
    let impl_trait_ty = checker
        .ctx
        .node_types
        .get(&impl_trait_node.id)
        .copied()
        .unwrap_or(TypeId::ERROR);

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

    if ignore_target_assoc_bindings {
        return Some(resolved_args);
    }

    let TypeKind::TraitObject(_, _, target_assoc_bindings) =
        checker.ctx.type_registry.get(target_trait_norm).clone()
    else {
        return None;
    };

    if !target_assoc_bindings.is_empty() {
        let Some(instantiated_impl_trait_ty) =
            instantiate_impl_trait_ty(checker.ctx, impl_id, &resolved_args)
        else {
            return None;
        };
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
            let Some(&impl_assoc_ty) = impl_assoc_bindings.get(&assoc_def_id) else {
                return None;
            };
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
    let target_ty = ctx
        .node_types
        .get(&impl_def.target_type.id)
        .copied()
        .unwrap_or(TypeId::ERROR);
    let trait_ty = impl_def
        .trait_type
        .as_ref()
        .and_then(|trait_ty| ctx.node_types.get(&trait_ty.id).copied());
    let trait_ty = trait_ty.map(|trait_ty| erase_trait_assoc_bindings(ctx, trait_ty));

    if target_ty == TypeId::ERROR || matches!(trait_ty, Some(TypeId::ERROR)) {
        return None;
    }

    Some((impl_def, target_ty, trait_ty))
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

fn freshen_impl_head_types(
    checker: &mut ExprChecker<'_, '_>,
    impl_def: &ImplDef,
    target_ty: TypeId,
    trait_ty: Option<TypeId>,
    freshness: ImplHeadFreshness,
) -> (TypeId, Option<TypeId>) {
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
                let const_ty = checker
                    .ctx
                    .node_types
                    .get(&ty.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                crate::ty::GenericArg::Const(crate::ty::ConstGeneric::Param(fresh_name, const_ty))
            }
        };
        subst_map.insert(param.name, fresh_arg);
    }

    let mut subst = Substituter::new(&mut checker.ctx.type_registry, &subst_map);
    (
        subst.substitute(target_ty),
        trait_ty.map(|trait_ty| subst.substitute(trait_ty)),
    )
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
        let original_target = checker
            .ctx
            .node_types
            .get(&clause.target_ty.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let sub_target =
            checker.substitute_type_with_unification_maps(original_target, type_map, const_map);

        for bound_ast in &clause.bounds {
            let original_bound = checker
                .ctx
                .node_types
                .get(&bound_ast.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
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
