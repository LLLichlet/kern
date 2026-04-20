use kernc_utils::AtomicOrdering;
use kernc_utils::config::RuntimeEntry;
use kernc_utils::{
    DiagnosticBuilder, DiagnosticLevel, FastHashMap, FastHashSet, FileId, NodeId, Session, Span,
    SymbolId,
};
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use crate::checker::{ExprChecker, Substituter};
use crate::def::{Def, DefId, ImplDef};
use crate::passes::TypeResolver;
use crate::scope::{ScopeId, SymbolTable};
use crate::semantic::{SemanticDefinition, SemanticSymbolKind};
use crate::ty::{GenericArg, TypeFormatter, TypeId, TypeKind, TypeRegistry};
use kernc_ast::Visibility;

type NamedFieldQueryKey = (Option<DefId>, DefId, Vec<GenericArg>, SymbolId);
type NamedFieldQueryValue = Option<crate::query::MemberCandidate>;
type MemberResolutionQueryKey = (Option<DefId>, TypeId, SymbolId);

#[derive(Clone)]
pub struct SemaStructureSnapshot {
    pub type_registry: TypeRegistry,
    pub node_types: FastHashMap<NodeId, TypeId>,
    pub atomic_orderings: FastHashMap<NodeId, AtomicOrdering>,
    pub trait_method_owners: FastHashMap<NodeId, TypeId>,
    pub builtin_defs: FastHashMap<SymbolId, DefId>,
    pub current_package_name: Option<SymbolId>,
    pub defs: Vec<Def>,
    pub scopes: SymbolTable,
    pub global_impls: Vec<DefId>,
    pub trait_impls: Vec<DefId>,
    pub impl_methods_by_name: FastHashMap<SymbolId, Vec<DefId>>,
    pub alias_roots: FastHashMap<SymbolId, DefId>,
    pub root_module: Option<DefId>,
    pub root_module_package_names: FastHashMap<DefId, SymbolId>,
    pub module_defs_by_scope: FastHashMap<ScopeId, DefId>,
    pub parent_modules_by_def: FastHashMap<DefId, DefId>,
    pub defs_without_parent_module: FastHashSet<DefId>,
    pub owner_scopes_by_def: FastHashMap<DefId, ScopeId>,
    pub reported_recursive_layout_types: FastHashSet<TypeId>,
    pub reported_recursive_projection_types: FastHashSet<TypeId>,
    pub reported_recursive_projection_assoc_defs: FastHashSet<DefId>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ExprTimingStats {
    pub bindings: Duration,
    pub ops: Duration,
    pub access: Duration,
    pub access_identifier: Duration,
    pub access_field: Duration,
    pub access_field_module: Duration,
    pub access_field_enum_variant: Duration,
    pub access_field_member_query: Duration,
    pub access_field_query_trait_object: Duration,
    pub access_field_query_named_type: Duration,
    pub access_field_query_bound: Duration,
    pub access_field_query_impl: Duration,
    pub access_field_miss: Duration,
    pub access_index: Duration,
    pub access_slice: Duration,
    pub call: Duration,
    pub call_plain: Duration,
    pub call_signature: Duration,
    pub call_intrinsic: Duration,
    pub call_arguments: Duration,
    pub call_generic_instantiation: Duration,
    pub call_closure: Duration,
    pub aggregate: Duration,
    pub control: Duration,
    pub control_block: Duration,
    pub control_if: Duration,
    pub control_match: Duration,
    pub control_match_patterns: Duration,
    pub control_match_bodies: Duration,
    pub control_match_exhaustiveness: Duration,
    pub control_for: Duration,
    pub control_return: Duration,
    pub control_defer: Duration,
    pub dynamic_typeof: Duration,
}

pub struct SemaContext<'a> {
    // 1. Shared compiler services.
    pub sess: &'a mut Session,

    // 2. Type-system state.
    pub type_registry: TypeRegistry,
    // Final inferred type for each AST node.
    pub node_types: FastHashMap<NodeId, TypeId>,
    pub atomic_orderings: FastHashMap<NodeId, AtomicOrdering>,
    pub trait_method_owners: FastHashMap<NodeId, TypeId>,
    pub builtin_defs: FastHashMap<SymbolId, DefId>,
    pub current_package_name: Option<SymbolId>,
    // Active trait bounds introduced by the current generic scope.
    pub active_bounds: Vec<(TypeId, Vec<TypeId>)>,

    // 3. Symbol and scope state.
    pub defs: Vec<Def>,
    pub scopes: SymbolTable,
    pub global_impls: Vec<DefId>,
    pub trait_impls: Vec<DefId>,
    pub impl_methods_by_name: FastHashMap<SymbolId, Vec<DefId>>,

    // 4. Module and package resolution state.
    pub module_aliases: HashMap<String, String>,
    pub module_interface_aliases: HashMap<String, String>,
    pub alias_roots: FastHashMap<SymbolId, DefId>,
    pub root_module: Option<DefId>,
    pub root_module_package_names: FastHashMap<DefId, SymbolId>,
    pub module_defs_by_scope: FastHashMap<ScopeId, DefId>,
    pub parent_modules_by_def: FastHashMap<DefId, DefId>,
    pub defs_without_parent_module: FastHashSet<DefId>,
    pub owner_scopes_by_def: FastHashMap<DefId, ScopeId>,
    pub expr_timing_stats: ExprTimingStats,
    pub(crate) call_signature_instantiation_cache: FastHashMap<TypeId, TypeId>,
    pub(crate) field_type_subst_cache: FastHashMap<(NodeId, Vec<GenericArg>), TypeId>,
    pub(crate) trait_method_query_cache:
        FastHashMap<(TypeId, SymbolId, TypeId), crate::query::MemberResolution>,
    pub(crate) impl_method_query_cache:
        FastHashMap<(TypeId, SymbolId), Option<crate::query::MemberCandidate>>,
    pub(crate) bound_trait_match_cache: FastHashMap<TypeId, Vec<TypeId>>,
    pub(crate) impl_applicability_cache:
        FastHashMap<(TypeId, DefId), Option<Vec<crate::ty::GenericArg>>>,
    pub(crate) impl_requirement_cycle_cache: FastHashMap<DefId, Option<ImplRequirementCycle>>,
    pub(crate) impl_paterson_boundedness_cache:
        FastHashMap<DefId, Option<NonDecreasingImplRequirement>>,
    pub(crate) named_field_query_cache: FastHashMap<NamedFieldQueryKey, NamedFieldQueryValue>,
    pub(crate) member_resolution_query_cache:
        FastHashMap<MemberResolutionQueryKey, crate::query::MemberResolution>,
    pub(crate) reported_recursive_layout_types: FastHashSet<TypeId>,
    pub(crate) reported_recursive_projection_types: FastHashSet<TypeId>,
    pub(crate) reported_recursive_projection_assoc_defs: FastHashSet<DefId>,
    identifier_references: Vec<(Span, Span)>,
    semantic_definitions: BTreeMap<Span, SemanticDefinition>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SelfReferentialImplRequirement {
    pub bound_span: Span,
    pub target_ty: TypeId,
    pub trait_ty: TypeId,
}

#[derive(Debug, Clone)]
pub(crate) struct ImplRequirementCycle {
    pub start_bound_span: Span,
    pub target_ty: TypeId,
    pub trait_ty: TypeId,
    pub requirements: Vec<ImplRequirementEdge>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ImplRequirementEdge {
    pub impl_id: DefId,
    pub requirement_span: Span,
    pub target_ty: TypeId,
    pub trait_ty: TypeId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PatersonParam {
    Type(SymbolId),
    Const(SymbolId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PatersonBoundednessIssue {
    ConstructorCount {
        head: usize,
        requirement: usize,
    },
    VariableCount {
        param: PatersonParam,
        head: usize,
        requirement: usize,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PatersonMeasure {
    pub constructors: usize,
    pub params: FastHashMap<PatersonParam, usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct NonDecreasingImplRequirement {
    pub bound_span: Span,
    pub head_target_ty: TypeId,
    pub head_trait_ty: TypeId,
    pub requirement_target_ty: TypeId,
    pub requirement_trait_ty: TypeId,
    pub issue: PatersonBoundednessIssue,
}

impl<'a> SemaContext<'a> {
    /// Build semantic-analysis state around an existing session.
    pub fn new(sess: &'a mut Session) -> Self {
        Self {
            sess,
            type_registry: TypeRegistry::new(),
            node_types: FastHashMap::default(),
            atomic_orderings: FastHashMap::default(),
            trait_method_owners: FastHashMap::default(),
            builtin_defs: FastHashMap::default(),
            current_package_name: None,
            active_bounds: Vec::new(),
            defs: Vec::new(),
            scopes: SymbolTable::new(),
            module_aliases: HashMap::new(),
            module_interface_aliases: HashMap::new(),
            alias_roots: FastHashMap::default(),
            root_module: None,
            root_module_package_names: FastHashMap::default(),
            module_defs_by_scope: FastHashMap::default(),
            parent_modules_by_def: FastHashMap::default(),
            defs_without_parent_module: FastHashSet::default(),
            owner_scopes_by_def: FastHashMap::default(),
            expr_timing_stats: ExprTimingStats::default(),
            call_signature_instantiation_cache: FastHashMap::default(),
            field_type_subst_cache: FastHashMap::default(),
            trait_method_query_cache: FastHashMap::default(),
            impl_method_query_cache: FastHashMap::default(),
            bound_trait_match_cache: FastHashMap::default(),
            impl_applicability_cache: FastHashMap::default(),
            impl_requirement_cycle_cache: FastHashMap::default(),
            impl_paterson_boundedness_cache: FastHashMap::default(),
            named_field_query_cache: FastHashMap::default(),
            member_resolution_query_cache: FastHashMap::default(),
            reported_recursive_layout_types: FastHashSet::default(),
            reported_recursive_projection_types: FastHashSet::default(),
            reported_recursive_projection_assoc_defs: FastHashSet::default(),
            global_impls: Vec::new(),
            trait_impls: Vec::new(),
            impl_methods_by_name: FastHashMap::default(),
            identifier_references: Vec::new(),
            semantic_definitions: BTreeMap::new(),
        }
    }

    // ==========================================
    // Core operations
    // ==========================================

    pub fn add_def(&mut self, def: Def) -> DefId {
        let id = DefId(self.defs.len() as u32);
        self.defs.push(def);
        self.defs_without_parent_module.insert(id);
        id
    }

    pub fn collects_timings(&self) -> bool {
        self.sess.report_timings
    }

    pub fn ty_to_string(&self, ty: TypeId) -> String {
        TypeFormatter { ctx: self }.format(ty)
    }

    pub fn structure_snapshot(&self) -> SemaStructureSnapshot {
        SemaStructureSnapshot {
            type_registry: self.type_registry.clone(),
            node_types: self.node_types.clone(),
            atomic_orderings: self.atomic_orderings.clone(),
            trait_method_owners: self.trait_method_owners.clone(),
            builtin_defs: self.builtin_defs.clone(),
            current_package_name: self.current_package_name,
            defs: self.defs.clone(),
            scopes: self.scopes.clone(),
            global_impls: self.global_impls.clone(),
            trait_impls: self.trait_impls.clone(),
            impl_methods_by_name: self.impl_methods_by_name.clone(),
            alias_roots: self.alias_roots.clone(),
            root_module: self.root_module,
            root_module_package_names: self.root_module_package_names.clone(),
            module_defs_by_scope: self.module_defs_by_scope.clone(),
            parent_modules_by_def: self.parent_modules_by_def.clone(),
            defs_without_parent_module: self.defs_without_parent_module.clone(),
            owner_scopes_by_def: self.owner_scopes_by_def.clone(),
            reported_recursive_layout_types: self.reported_recursive_layout_types.clone(),
            reported_recursive_projection_types: self.reported_recursive_projection_types.clone(),
            reported_recursive_projection_assoc_defs: self
                .reported_recursive_projection_assoc_defs
                .clone(),
        }
    }

    pub fn into_structure_snapshot(self) -> SemaStructureSnapshot {
        SemaStructureSnapshot {
            type_registry: self.type_registry,
            node_types: self.node_types,
            atomic_orderings: self.atomic_orderings,
            trait_method_owners: self.trait_method_owners,
            builtin_defs: self.builtin_defs,
            current_package_name: self.current_package_name,
            defs: self.defs,
            scopes: self.scopes,
            global_impls: self.global_impls,
            trait_impls: self.trait_impls,
            impl_methods_by_name: self.impl_methods_by_name,
            alias_roots: self.alias_roots,
            root_module: self.root_module,
            root_module_package_names: self.root_module_package_names,
            module_defs_by_scope: self.module_defs_by_scope,
            parent_modules_by_def: self.parent_modules_by_def,
            defs_without_parent_module: self.defs_without_parent_module,
            owner_scopes_by_def: self.owner_scopes_by_def,
            reported_recursive_layout_types: self.reported_recursive_layout_types,
            reported_recursive_projection_types: self.reported_recursive_projection_types,
            reported_recursive_projection_assoc_defs: self.reported_recursive_projection_assoc_defs,
        }
    }

    pub fn restore_structure(&mut self, snapshot: SemaStructureSnapshot) {
        self.type_registry = snapshot.type_registry;
        self.node_types = snapshot.node_types;
        self.atomic_orderings = snapshot.atomic_orderings;
        self.trait_method_owners = snapshot.trait_method_owners;
        self.builtin_defs = snapshot.builtin_defs;
        self.current_package_name = snapshot.current_package_name;
        self.active_bounds.clear();
        self.clear_active_bound_caches();
        self.defs = snapshot.defs;
        self.scopes = snapshot.scopes;
        self.global_impls = snapshot.global_impls;
        self.trait_impls = snapshot.trait_impls;
        self.impl_methods_by_name = snapshot.impl_methods_by_name;
        self.alias_roots = snapshot.alias_roots;
        self.root_module = snapshot.root_module;
        self.root_module_package_names = snapshot.root_module_package_names;
        self.module_defs_by_scope = snapshot.module_defs_by_scope;
        self.parent_modules_by_def = snapshot.parent_modules_by_def;
        self.defs_without_parent_module = snapshot.defs_without_parent_module;
        self.owner_scopes_by_def = snapshot.owner_scopes_by_def;
        self.reported_recursive_layout_types = snapshot.reported_recursive_layout_types;
        self.reported_recursive_projection_types = snapshot.reported_recursive_projection_types;
        self.reported_recursive_projection_assoc_defs =
            snapshot.reported_recursive_projection_assoc_defs;
        self.expr_timing_stats = ExprTimingStats::default();
        self.call_signature_instantiation_cache.clear();
        self.field_type_subst_cache.clear();
        self.trait_method_query_cache.clear();
        self.impl_method_query_cache.clear();
        self.clear_active_bound_caches();
        self.impl_requirement_cycle_cache.clear();
        self.impl_paterson_boundedness_cache.clear();
        self.named_field_query_cache.clear();
        self.member_resolution_query_cache.clear();
        self.identifier_references.clear();
        self.semantic_definitions.clear();
    }

    pub fn clear_active_bound_caches(&mut self) {
        self.bound_trait_match_cache.clear();
        self.impl_applicability_cache.clear();
        self.impl_method_query_cache.clear();
        self.member_resolution_query_cache.clear();
    }

    pub(crate) fn direct_self_referential_impl_requirement(
        &self,
        impl_def: &ImplDef,
    ) -> Option<SelfReferentialImplRequirement> {
        let Some(trait_ty_node) = &impl_def.trait_type else {
            return None;
        };

        let impl_target_ty = self.type_registry.normalize(
            self.node_types
                .get(&impl_def.target_type.id)
                .copied()
                .unwrap_or(TypeId::ERROR),
        );
        let impl_trait_ty = self.type_registry.normalize(
            self.node_types
                .get(&trait_ty_node.id)
                .copied()
                .unwrap_or(TypeId::ERROR),
        );
        if impl_target_ty == TypeId::ERROR || impl_trait_ty == TypeId::ERROR {
            return None;
        }

        for clause in &impl_def.where_clauses {
            let clause_target_ty = self.type_registry.normalize(
                self.node_types
                    .get(&clause.target_ty.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR),
            );
            if clause_target_ty != impl_target_ty {
                continue;
            }

            for bound in &clause.bounds {
                let bound_ty = self.type_registry.normalize(
                    self.node_types
                        .get(&bound.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR),
                );
                if self.matches_impl_trait_obligation_head(bound_ty, impl_trait_ty) {
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

    fn matches_impl_trait_obligation_head(&self, bound_ty: TypeId, impl_trait_ty: TypeId) -> bool {
        let bound_norm = self.type_registry.normalize(bound_ty);
        let impl_norm = self.type_registry.normalize(impl_trait_ty);
        match (
            self.type_registry.get(bound_norm),
            self.type_registry.get(impl_norm),
        ) {
            (
                crate::ty::TypeKind::TraitObject(bound_def_id, bound_args, _),
                crate::ty::TypeKind::TraitObject(impl_def_id, impl_args, _),
            ) => bound_def_id == impl_def_id && bound_args == impl_args,
            _ => bound_norm == impl_norm,
        }
    }

    pub(crate) fn indirect_self_referential_impl_requirement(
        &mut self,
        impl_id: DefId,
    ) -> Option<ImplRequirementCycle> {
        if let Some(cached) = self.impl_requirement_cycle_cache.get(&impl_id).cloned() {
            return cached;
        }

        let cycle = self.compute_indirect_impl_requirement_cycle(impl_id);
        self.impl_requirement_cycle_cache
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
        if self
            .direct_self_referential_impl_requirement(&impl_def)
            .is_some()
        {
            return None;
        }

        let Some(trait_ty_node) = &impl_def.trait_type else {
            return None;
        };

        let start_target_ty = self.type_registry.normalize(
            self.node_types
                .get(&impl_def.target_type.id)
                .copied()
                .unwrap_or(TypeId::ERROR),
        );
        let start_trait_ty = self.type_registry.normalize(
            self.node_types
                .get(&trait_ty_node.id)
                .copied()
                .unwrap_or(TypeId::ERROR),
        );
        if start_target_ty == TypeId::ERROR || start_trait_ty == TypeId::ERROR {
            return None;
        }

        let initial_requirements = self.instantiated_impl_requirements(
            &impl_def,
            &FastHashMap::<SymbolId, crate::ty::GenericArg>::default(),
        );
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

        let trait_impl_ids = self.trait_impls.clone();
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

            let impl_target_ty = self
                .node_types
                .get(&candidate_impl.target_type.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let impl_trait_ty = self
                .node_types
                .get(&candidate_trait_ast.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            if impl_target_ty == TypeId::ERROR || impl_trait_ty == TypeId::ERROR {
                continue;
            }

            let mut head_type_map = FastHashMap::default();
            let mut head_const_map = FastHashMap::default();
            let applicable = {
                let mut checker = ExprChecker::new(self, None);
                if !checker.match_available_type_against_requirement(
                    impl_target_ty,
                    source_ty,
                    &mut head_type_map,
                    &mut head_const_map,
                ) {
                    false
                } else {
                    let instantiated_trait_ty = checker.substitute_type_with_unification_maps(
                        impl_trait_ty,
                        &head_type_map,
                        &head_const_map,
                    );
                    let instantiated_trait_ty =
                        checker.ctx.type_registry.normalize(instantiated_trait_ty);
                    let mut trait_type_map = FastHashMap::default();
                    let mut trait_const_map = FastHashMap::default();
                    let matches = instantiated_trait_ty == target_trait_ty
                        || checker.match_available_type_against_requirement(
                            instantiated_trait_ty,
                            target_trait_ty,
                            &mut trait_type_map,
                            &mut trait_const_map,
                        );
                    if matches {
                        for (name, ty) in trait_type_map {
                            head_type_map.entry(name).or_insert(ty);
                        }
                        for (name, value) in trait_const_map {
                            head_const_map.entry(name).or_insert(value);
                        }
                    }
                    matches
                }
            };
            if !applicable {
                continue;
            }

            let head_map = {
                let mut head_map = FastHashMap::default();
                for (&name, &ty) in &head_type_map {
                    head_map.insert(name, crate::ty::GenericArg::Type(ty));
                }
                for (&name, &value) in &head_const_map {
                    head_map.insert(name, crate::ty::GenericArg::Const(value));
                }
                head_map
            };
            let requirements = self.instantiated_impl_requirements(&candidate_impl, &head_map);
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

    fn instantiated_impl_requirements(
        &mut self,
        impl_def: &ImplDef,
        map: &FastHashMap<SymbolId, crate::ty::GenericArg>,
    ) -> Vec<ImplRequirementEdge> {
        let mut requirements = Vec::new();

        for clause in &impl_def.where_clauses {
            let target_ty = {
                let original_target = self
                    .node_types
                    .get(&clause.target_ty.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let substituted_target = {
                    let mut subst = Substituter::new(&mut self.type_registry, map);
                    subst.substitute(original_target)
                };
                self.type_registry.normalize(substituted_target)
            };

            for bound in &clause.bounds {
                let trait_ty = {
                    let original_bound = self
                        .node_types
                        .get(&bound.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR);
                    let substituted_bound = {
                        let mut subst = Substituter::new(&mut self.type_registry, map);
                        subst.substitute(original_bound)
                    };
                    self.type_registry.normalize(substituted_bound)
                };
                if !matches!(self.type_registry.get(trait_ty), TypeKind::TraitObject(..)) {
                    continue;
                }
                requirements.push(ImplRequirementEdge {
                    impl_id: impl_def.id,
                    requirement_span: bound.span,
                    target_ty,
                    trait_ty,
                });
            }
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
            && self.matches_impl_trait_obligation_head(trait_ty, impl_trait_ty)
    }

    pub(crate) fn non_decreasing_impl_requirement(
        &mut self,
        impl_id: DefId,
    ) -> Option<NonDecreasingImplRequirement> {
        if let Some(cached) = self.impl_paterson_boundedness_cache.get(&impl_id).cloned() {
            return cached;
        }

        let violation = self.compute_non_decreasing_impl_requirement(impl_id);
        self.impl_paterson_boundedness_cache
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

        let head_target_ty = self.type_registry.normalize(
            self.node_types
                .get(&impl_def.target_type.id)
                .copied()
                .unwrap_or(TypeId::ERROR),
        );
        let head_trait_ty = self.type_registry.normalize(
            self.node_types
                .get(&trait_ty_node.id)
                .copied()
                .unwrap_or(TypeId::ERROR),
        );
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

    pub(crate) fn compare_paterson_supertrait_against_generics(
        &self,
        head_generics: &[kernc_ast::GenericParam],
        requirement_trait_ty: TypeId,
    ) -> Option<PatersonBoundednessIssue> {
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

    /// Normalize aliases and any fully concrete associated-type projections that can be
    /// discharged from a concrete receiver type and the currently known global impls.
    ///
    /// This is intentionally shared between sema-side layout queries and lowering so both
    /// phases agree on what counts as a fully concrete type.
    pub fn normalize_concrete_type(&mut self, ty: TypeId) -> TypeId {
        let mut projection_stack = Vec::new();
        let mut projection_chain = Vec::new();
        let mut projection_assoc_chain = Vec::new();
        self.normalize_concrete_type_inner(
            ty,
            &mut projection_stack,
            &mut projection_chain,
            &mut projection_assoc_chain,
        )
    }

    fn normalize_concrete_type_inner(
        &mut self,
        ty: TypeId,
        projection_stack: &mut Vec<TypeId>,
        projection_chain: &mut Vec<TypeId>,
        projection_assoc_chain: &mut Vec<(DefId, TypeId)>,
    ) -> TypeId {
        let mut curr = ty;
        loop {
            let norm = self.type_registry.normalize(curr);
            let is_projection = matches!(self.type_registry.get(norm), TypeKind::Projection { .. });
            if is_projection {
                if let Some(ancestor_index) = projection_chain.iter().position(|seen| *seen == norm)
                {
                    let cycle = projection_chain[ancestor_index..]
                        .iter()
                        .copied()
                        .chain(std::iter::once(norm))
                        .collect::<Vec<_>>();
                    self.emit_projection_cycle_diagnostic(&cycle);
                    return TypeId::ERROR;
                }
                projection_chain.push(norm);
            }
            let Some(next) = self.try_normalize_projection_type(
                norm,
                projection_stack,
                projection_chain,
                projection_assoc_chain,
            ) else {
                return norm;
            };
            if next == norm {
                if is_projection {
                    self.emit_projection_cycle_diagnostic(&[norm, norm]);
                    return TypeId::ERROR;
                }
                return norm;
            }
            curr = next;
        }
    }

    fn try_normalize_projection_type(
        &mut self,
        ty: TypeId,
        projection_stack: &mut Vec<TypeId>,
        projection_chain: &mut Vec<TypeId>,
        projection_assoc_chain: &mut Vec<(DefId, TypeId)>,
    ) -> Option<TypeId> {
        let TypeKind::Projection {
            target,
            trait_def_id,
            trait_args,
            assoc_def_id,
            assoc_args,
        } = self.type_registry.get(ty).clone()
        else {
            return None;
        };

        if !assoc_args.is_empty() || projection_stack.contains(&ty) {
            return None;
        }
        projection_stack.push(ty);

        let result = (|| {
            let target_norm = self.normalize_concrete_type_inner(
                target,
                projection_stack,
                projection_chain,
                projection_assoc_chain,
            );
            if let TypeKind::TraitObject(target_trait_def_id, target_trait_args, assoc_bindings) =
                self.type_registry.get(target_norm).clone()
                && target_trait_def_id == trait_def_id
                && target_trait_args == trait_args
                && let Some((_, assoc_ty)) = assoc_bindings
                    .iter()
                    .find(|(bound_assoc_id, _)| *bound_assoc_id == assoc_def_id)
            {
                return Some(*assoc_ty);
            }

            let trait_impls = self.trait_impls.clone();
            let mut selected: Option<(DefId, DefId, TypeId)> = None;
            for impl_id in trait_impls {
                let Some(impl_ptr) = self.defs.get(impl_id.0 as usize).and_then(|def| match def {
                    Def::Impl(impl_def) => Some(std::ptr::from_ref(impl_def)),
                    _ => None,
                }) else {
                    continue;
                };

                let Some(impl_args) = crate::query::resolve_trait_impl_head_obligation(
                    self,
                    target_norm,
                    trait_def_id,
                    &trait_args,
                    impl_id,
                ) else {
                    continue;
                };

                let impl_def = unsafe { &*impl_ptr };
                let Some(trait_ast) = &impl_def.trait_type else {
                    continue;
                };
                let impl_trait_ty = self
                    .node_types
                    .get(&trait_ast.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                if impl_trait_ty == TypeId::ERROR {
                    continue;
                }

                let mut subst_map = HashMap::new();
                for (param, arg) in impl_def.generics.iter().zip(impl_args.iter().copied()) {
                    subst_map.insert(param.name, arg);
                }
                let inst_trait_ty = if subst_map.is_empty() {
                    impl_trait_ty
                } else {
                    let mut subst = Substituter::new(&mut self.type_registry, &subst_map);
                    subst.substitute(impl_trait_ty)
                };

                let TypeKind::TraitObject(bound_trait_def_id, bound_trait_args, assoc_bindings) =
                    self.type_registry
                        .get(self.type_registry.normalize(inst_trait_ty))
                        .clone()
                else {
                    continue;
                };
                if bound_trait_def_id != trait_def_id || bound_trait_args != trait_args {
                    continue;
                }

                if let Some((_, assoc_ty)) = assoc_bindings
                    .iter()
                    .find(|(bound_assoc_id, _)| *bound_assoc_id == assoc_def_id)
                {
                    let Some(impl_assoc_id) =
                        self.impl_assoc_def_for_trait_assoc(impl_id, assoc_def_id)
                    else {
                        continue;
                    };
                    let replace = match selected {
                        None => true,
                        Some((selected_impl_id, ..)) => matches!(
                            crate::query::compare_impl_specificity(self, impl_id, selected_impl_id),
                            crate::query::ImplSpecificity::LeftMoreSpecific
                        ),
                    };
                    if replace {
                        selected = Some((impl_id, impl_assoc_id, *assoc_ty));
                    }
                }
            }

            let Some((_, impl_assoc_id, assoc_ty)) = selected else {
                return None;
            };
            if let Some(ancestor_index) = projection_assoc_chain
                .iter()
                .position(|(seen_assoc_id, _)| *seen_assoc_id == impl_assoc_id)
            {
                let cycle = projection_assoc_chain[ancestor_index..]
                    .iter()
                    .copied()
                    .chain(std::iter::once((impl_assoc_id, ty)))
                    .collect::<Vec<_>>();
                self.emit_non_contractive_projection_cycle_diagnostic(&cycle);
                return Some(TypeId::ERROR);
            }
            projection_assoc_chain.push((impl_assoc_id, ty));
            Some(assoc_ty)
        })();

        let popped = projection_stack.pop();
        debug_assert_eq!(popped, Some(ty));
        result
    }

    pub(crate) fn emit_projection_cycle_diagnostic(&mut self, cycle: &[TypeId]) {
        if cycle
            .iter()
            .any(|cycle_ty| self.reported_recursive_projection_types.contains(cycle_ty))
        {
            return;
        }
        for cycle_ty in cycle {
            self.reported_recursive_projection_types.insert(*cycle_ty);
        }

        let head = cycle.first().copied().unwrap_or(TypeId::ERROR);
        let span = match self.type_registry.get(self.type_registry.normalize(head)) {
            TypeKind::Projection { assoc_def_id, .. } => self
                .defs
                .get(assoc_def_id.0 as usize)
                .and_then(|def| match def {
                    Def::AssociatedType(def) => Some(def.span),
                    _ => None,
                })
                .unwrap_or_default(),
            _ => Span::default(),
        };
        let chain = cycle
            .iter()
            .map(|ty| self.ty_to_string(*ty))
            .collect::<Vec<_>>()
            .join(" -> ");

        self.struct_error(
            span,
            format!(
                "recursive associated type projection cycle detected while normalizing `{}`",
                self.ty_to_string(head)
            ),
        )
        .with_hint(format!("projection cycle: {}", chain))
        .with_hint("break the cycle by giving one associated type a concrete non-projecting result")
        .emit();
    }

    fn impl_assoc_def_for_trait_assoc(
        &self,
        impl_id: DefId,
        trait_assoc_id: DefId,
    ) -> Option<DefId> {
        let trait_assoc_name =
            self.defs
                .get(trait_assoc_id.0 as usize)
                .and_then(|def| match def {
                    Def::AssociatedType(def) => Some(def.name),
                    _ => None,
                })?;
        let impl_def = self
            .defs
            .get(impl_id.0 as usize)
            .and_then(|def| match def {
                Def::Impl(def) => Some(def),
                _ => None,
            })?;
        impl_def.assoc_types.iter().copied().find(|assoc_id| {
            self.defs
                .get(assoc_id.0 as usize)
                .and_then(|def| match def {
                    Def::AssociatedType(def) => Some(def.name),
                    _ => None,
                })
                == Some(trait_assoc_name)
        })
    }

    pub(crate) fn emit_non_contractive_projection_cycle_diagnostic(
        &mut self,
        cycle: &[(DefId, TypeId)],
    ) {
        if cycle.iter().any(|(assoc_id, _)| {
            self.reported_recursive_projection_assoc_defs
                .contains(assoc_id)
        }) {
            return;
        }
        for (assoc_id, _) in cycle {
            self.reported_recursive_projection_assoc_defs
                .insert(*assoc_id);
        }

        let head_assoc_id = cycle.first().map(|(assoc_id, _)| *assoc_id);
        let head_ty = cycle.first().map(|(_, ty)| *ty).unwrap_or(TypeId::ERROR);
        let span = head_assoc_id
            .and_then(|assoc_id| self.defs.get(assoc_id.0 as usize))
            .and_then(|def| match def {
                Def::AssociatedType(def) => Some(def.span),
                _ => None,
            })
            .unwrap_or_default();
        let chain = cycle
            .iter()
            .map(|(assoc_id, ty)| {
                let assoc_name = self
                    .defs
                    .get(assoc_id.0 as usize)
                    .and_then(|def| def.name())
                    .map(|name| self.resolve(name).to_string())
                    .unwrap_or_else(|| "<assoc>".to_string());
                format!("{} via {}", assoc_name, self.ty_to_string(*ty))
            })
            .collect::<Vec<_>>()
            .join(" -> ");

        self.struct_error(
            span,
            format!(
                "recursive associated type projection cycle detected while normalizing `{}`",
                self.ty_to_string(head_ty)
            ),
        )
        .with_hint(format!(
            "projection repeatedly re-enters the same impl-associated type: {}",
            chain
        ))
        .with_hint("break the cycle by giving one associated type a concrete non-projecting result")
        .emit();
    }

    /// Inject CLI-provided module aliases such as `std` into the root scope.
    /// This lets code refer to `std.io` directly without an explicit `use std;`.
    pub fn inject_alias_roots(&mut self) {
        // Save and restore the current scope around the root-scope injection.
        let prev_scope = self.scopes.current_scope_id();

        // Scope 0 is the global builtin scope created by the symbol table.
        let global_scope = ScopeId(0);
        self.scopes.set_current_scope(global_scope);

        // Clone aliases up front to avoid borrow conflicts with `scopes`.
        let aliases: Vec<(SymbolId, DefId)> = self
            .alias_roots
            .iter()
            .map(|(&name, &mod_id)| (name, mod_id))
            .collect();

        let node_id = self.next_node_id();
        for (name, mod_id) in aliases {
            let info = crate::scope::SymbolInfo {
                kind: crate::scope::SymbolKind::Module,
                node_id,
                type_id: TypeId::ERROR,
                def_id: Some(mod_id),
                span: kernc_utils::Span::default(),
                vis: Visibility::Public,
                is_mut: false,
            };

            // Ignore duplicates here; later collection reports real conflicts.
            let _ = self.scopes.define(name, info);
        }

        // Restore the caller's scope.
        if let Some(prev) = prev_scope {
            self.scopes.set_current_scope(prev);
        }
    }

    // ==========================================
    // Convenience forwarders
    // ==========================================

    pub fn report(&mut self, span: Span, level: DiagnosticLevel, msg: String) {
        self.sess.report(span, level, msg);
    }

    pub fn has_errors(&self) -> bool {
        self.sess.has_errors()
    }

    pub fn struct_error(&mut self, span: Span, msg: impl Into<String>) -> DiagnosticBuilder<'_> {
        self.sess.struct_error(span, msg)
    }

    pub fn struct_warning(&mut self, span: Span, msg: impl Into<String>) -> DiagnosticBuilder<'_> {
        self.sess.struct_warning(span, msg)
    }

    pub fn emit_error(&mut self, span: Span, msg: impl Into<String>) {
        self.sess.emit_error(span, msg);
    }

    pub fn emit_warning(&mut self, span: Span, msg: impl Into<String>) {
        self.sess.emit_warning(span, msg.into());
    }

    pub fn emit_ice(&mut self, span: Span, msg: impl Into<String>) {
        self.sess.emit_ice(span, msg);
    }

    pub fn next_node_id(&mut self) -> NodeId {
        self.sess.next_node_id()
    }

    pub fn intern(&mut self, string: &str) -> SymbolId {
        self.sess.interner.intern(string)
    }

    pub fn resolve(&self, sym: SymbolId) -> &str {
        self.sess.interner.resolve(sym).unwrap_or("<unknown>")
    }

    pub fn load_file<P: AsRef<std::path::Path>>(&mut self, path: P) -> std::io::Result<FileId> {
        self.sess.load_file(path)
    }

    pub fn record_identifier_reference(&mut self, reference_span: Span, definition_span: Span) {
        self.identifier_references
            .push((reference_span, definition_span));
    }

    pub fn identifier_references(&self) -> &[(Span, Span)] {
        &self.identifier_references
    }

    pub fn record_symbol_definition(
        &mut self,
        span: Span,
        kind: SemanticSymbolKind,
        is_mut: bool,
        is_pub: bool,
    ) {
        self.semantic_definitions
            .entry(span)
            .or_insert(SemanticDefinition {
                span,
                kind,
                is_mut,
                is_pub,
            });
    }

    pub fn semantic_definitions(&self) -> impl Iterator<Item = &SemanticDefinition> {
        self.semantic_definitions.values()
    }

    pub fn module_parent(&self, module_id: DefId) -> Option<DefId> {
        match self.defs.get(module_id.0 as usize) {
            Some(Def::Module(module)) => module.parent,
            _ => None,
        }
    }

    pub fn module_is_same_or_descendant_of(
        &self,
        module_id: DefId,
        ancestor_module_id: DefId,
    ) -> bool {
        let mut current = Some(module_id);
        while let Some(module_id) = current {
            if module_id == ancestor_module_id {
                return true;
            }
            current = self.module_parent(module_id);
        }
        false
    }

    pub fn module_root(&self, module_id: DefId) -> DefId {
        let mut current = module_id;
        while let Some(parent) = self.module_parent(current) {
            current = parent;
        }
        current
    }

    pub fn root_module_package_name(&self, module_id: DefId) -> Option<SymbolId> {
        let root = self.module_root(module_id);
        self.root_module_package_names.get(&root).copied()
    }

    pub fn visibility_allows_access(
        &self,
        vis: Visibility,
        owner_module: DefId,
        current_module: Option<DefId>,
    ) -> bool {
        match vis {
            Visibility::Public => true,
            Visibility::Private => current_module == Some(owner_module),
            Visibility::Super => {
                let Some(current_module) = current_module else {
                    return false;
                };
                let Some(parent_module) = self.module_parent(owner_module) else {
                    return false;
                };
                self.module_is_same_or_descendant_of(current_module, parent_module)
            }
            Visibility::Package => {
                let Some(current_module) = current_module else {
                    return false;
                };
                let current_root = self.module_root(current_module);
                let owner_root = self.module_root(owner_module);
                match (
                    self.root_module_package_names.get(&current_root),
                    self.root_module_package_names.get(&owner_root),
                ) {
                    (Some(current_package), Some(owner_package)) => {
                        current_package == owner_package
                    }
                    _ => current_root == owner_root,
                }
            }
        }
    }

    pub fn register_builtin_def(&mut self, name: SymbolId, def_id: DefId) {
        self.builtin_defs.insert(name, def_id);
    }

    pub fn register_module_scope(&mut self, module_id: DefId, scope_id: ScopeId) {
        self.module_defs_by_scope.insert(scope_id, module_id);
    }

    pub fn register_def_owner(
        &mut self,
        def_id: DefId,
        parent_module: Option<DefId>,
        owner_scope: Option<ScopeId>,
    ) {
        if let Some(module_id) = parent_module {
            self.parent_modules_by_def.insert(def_id, module_id);
            self.defs_without_parent_module.remove(&def_id);
        }
        if let Some(scope_id) = owner_scope {
            self.owner_scopes_by_def.insert(def_id, scope_id);
        }
    }

    pub fn module_for_scope(&self, scope_id: ScopeId) -> Option<DefId> {
        let mut current = Some(scope_id);
        while let Some(scope_id) = current {
            if let Some(&module_id) = self.module_defs_by_scope.get(&scope_id) {
                return Some(module_id);
            }
            current = self.scopes.parent_scope(scope_id);
        }

        self.defs
            .iter()
            .filter_map(|def| {
                let Def::Module(module) = def else {
                    return None;
                };
                self.scopes
                    .distance_to_ancestor(scope_id, module.scope_id)
                    .map(|distance| (module.id, distance))
            })
            .min_by_key(|(_, distance)| *distance)
            .map(|(module_id, _)| module_id)
    }

    pub fn def_parent_module(&self, def_id: DefId) -> Option<DefId> {
        let parent = match self.defs.get(def_id.0 as usize) {
            Some(Def::Module(module)) => module.parent,
            Some(Def::Function(function)) => match function.parent {
                Some(parent_id) => match self.defs.get(parent_id.0 as usize) {
                    Some(Def::Module(_)) => Some(parent_id),
                    Some(Def::Impl(impl_def)) => impl_def.parent_module,
                    _ => None,
                },
                None => None,
            },
            Some(Def::Struct(def)) => def.parent_module,
            Some(Def::Union(def)) => def.parent_module,
            Some(Def::Enum(_)) | Some(Def::Trait(_)) | Some(Def::TypeAlias(_)) => {
                self.parent_modules_by_def.get(&def_id).copied()
            }
            Some(Def::AssociatedType(def)) => {
                if let Some(parent_impl) = def.parent_impl {
                    match self.defs.get(parent_impl.0 as usize) {
                        Some(Def::Impl(impl_def)) => impl_def.parent_module,
                        _ => None,
                    }
                } else if let Some(parent_trait) = def.parent_trait {
                    self.parent_modules_by_def.get(&parent_trait).copied()
                } else {
                    None
                }
            }
            Some(Def::Impl(def)) => def.parent_module,
            Some(Def::Global(global)) => match global.parent {
                Some(parent_id) => match self.defs.get(parent_id.0 as usize) {
                    Some(Def::Module(_)) => Some(parent_id),
                    Some(Def::Impl(impl_def)) => impl_def.parent_module,
                    _ => None,
                },
                None => None,
            },
            None => None,
        };

        if parent.is_some() {
            return parent;
        }
        if self.defs_without_parent_module.contains(&def_id) {
            return None;
        }

        // Legacy / malformed defs can still fall back to the module item scan, but the common
        // case should resolve directly from the def itself or the precomputed owner maps.
        parent.or_else(|| {
            self.defs.iter().find_map(|def| match def {
                Def::Module(module) if module.items.contains(&def_id) => Some(module.id),
                _ => None,
            })
        })
    }

    pub fn def_owner_scope(&self, def_id: DefId) -> Option<ScopeId> {
        self.owner_scopes_by_def.get(&def_id).copied().or_else(|| {
            match self.defs.get(def_id.0 as usize) {
                Some(Def::Function(function)) => {
                    let mut current_parent = function.parent;
                    while let Some(parent_id) = current_parent {
                        match self.defs.get(parent_id.0 as usize) {
                            Some(Def::Module(module)) => return Some(module.scope_id),
                            Some(Def::Impl(impl_def)) => current_parent = impl_def.parent_module,
                            _ => return None,
                        }
                    }
                    None
                }
                Some(Def::Global(_))
                | Some(Def::Struct(_))
                | Some(Def::Union(_))
                | Some(Def::Enum(_))
                | Some(Def::Trait(_))
                | Some(Def::AssociatedType(_))
                | Some(Def::TypeAlias(_))
                | Some(Def::Impl(_)) => self.def_parent_module(def_id).and_then(|module_id| {
                    match self.defs.get(module_id.0 as usize) {
                        Some(Def::Module(module)) => Some(module.scope_id),
                        _ => None,
                    }
                }),
                _ => None,
            }
        })
    }

    pub fn builtin_def(&mut self, name: &str) -> Option<DefId> {
        let symbol = self.intern(name);
        self.builtin_defs.get(&symbol).copied()
    }

    pub fn builtin_trait_ty(&mut self, name: &str, args: Vec<TypeId>) -> Option<TypeId> {
        let def_id = self.builtin_def(name)?;
        Some(self.type_registry.intern(crate::ty::TypeKind::TraitObject(
            def_id,
            crate::ty::wrap_type_args(args),
            Vec::new(),
        )))
    }

    pub fn builtin_trait_ty_with_assoc(
        &mut self,
        name: &str,
        generics: Vec<TypeId>,
        assoc_bindings: Vec<(&str, TypeId)>,
    ) -> Option<TypeId> {
        let def_id = self.builtin_def(name)?;
        let resolved_assoc_bindings = match self.defs.get(def_id.0 as usize) {
            Some(Def::Trait(trait_def)) => assoc_bindings
                .into_iter()
                .filter_map(|(assoc_name, ty)| {
                    trait_def.assoc_types.iter().copied().find_map(|assoc_id| {
                        let assoc_def = match self.defs.get(assoc_id.0 as usize) {
                            Some(Def::AssociatedType(assoc_def)) => assoc_def,
                            _ => return None,
                        };
                        if self.resolve(assoc_def.name) == assoc_name {
                            Some((assoc_id, ty))
                        } else {
                            None
                        }
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        Some(self.type_registry.intern(crate::ty::TypeKind::TraitObject(
            def_id,
            crate::ty::wrap_type_args(generics),
            resolved_assoc_bindings,
        )))
    }

    pub fn configured_runtime_entry(&self) -> RuntimeEntry {
        self.sess.runtime_entry
    }

    pub fn program_entry_enabled(&self) -> bool {
        !matches!(self.configured_runtime_entry(), RuntimeEntry::None)
    }

    pub fn main_argv_ptr_ty(&mut self) -> TypeId {
        let ptr_u8 = self.type_registry.intern(crate::ty::TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::U8,
        });
        self.type_registry.intern(crate::ty::TypeKind::Pointer {
            is_mut: false,
            elem: ptr_u8,
        })
    }

    fn def_source_name(&self, def_id: DefId) -> String {
        self.defs[def_id.0 as usize]
            .name()
            .map(|name_sym| self.resolve(name_sym).to_string())
            .unwrap_or_else(|| format!("AnonDef{}", def_id.0))
    }

    fn def_parent_for_path(&self, def_id: DefId) -> Option<DefId> {
        match &self.defs[def_id.0 as usize] {
            Def::Module(module) => module.parent,
            Def::Function(function) => function.parent,
            Def::Global(global) => global.parent,
            Def::Impl(impl_def) => impl_def.parent_module,
            Def::Struct(_)
            | Def::Union(_)
            | Def::Enum(_)
            | Def::Trait(_)
            | Def::AssociatedType(_)
            | Def::TypeAlias(_) => self.def_parent_module(def_id),
        }
    }

    fn parent_path_components(&self, mut parent_id: Option<DefId>) -> Vec<String> {
        let mut path_components = Vec::new();
        while let Some(def_id) = parent_id {
            match &self.defs[def_id.0 as usize] {
                Def::Module(module) => {
                    path_components.push(self.resolve(module.name).to_string());
                    parent_id = module.parent;
                }
                Def::Impl(impl_def) => {
                    let target_ty = self
                        .node_types
                        .get(&impl_def.target_type.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR);
                    path_components.push(self.mangle_type(target_ty));
                    if let Some(trait_ty) = &impl_def.trait_type {
                        let trait_ty = self
                            .node_types
                            .get(&trait_ty.id)
                            .copied()
                            .unwrap_or(TypeId::ERROR);
                        path_components.push(self.mangle_type(trait_ty));
                    }
                    parent_id = impl_def.parent_module;
                }
                _ => break,
            }
        }
        path_components
    }

    fn def_qualified_name(&self, def_id: DefId) -> String {
        let base_name = self.def_source_name(def_id);
        let path_components = self.parent_path_components(self.def_parent_for_path(def_id));
        if path_components.is_empty() {
            return base_name;
        }

        let mut qualified = String::from("Q");
        for component in path_components.into_iter().rev() {
            qualified.push_str(&format!("{}{}", component.len(), component));
        }
        qualified.push_str(&format!("{}{}", base_name.len(), base_name));
        qualified.push('E');
        qualified
    }

    fn mangle_const_generic(&self, value: crate::ty::ConstGeneric) -> String {
        match value {
            crate::ty::ConstGeneric::Value(value) => {
                let payload = match value.kind {
                    crate::ty::ConstGenericValueKind::Int(value) => format!("i{}", value),
                    crate::ty::ConstGenericValueKind::Bool(value) => {
                        if value {
                            "b1".to_string()
                        } else {
                            "b0".to_string()
                        }
                    }
                };
                format!("C{}{}", self.mangle_type(value.ty), payload)
            }
            crate::ty::ConstGeneric::Param(symbol, ty) => {
                format!("P{}{}", self.mangle_type(ty), symbol.0)
            }
            crate::ty::ConstGeneric::Expr(id) => match self.type_registry.const_expr(id) {
                crate::ty::ConstExprKind::Unary { op, expr, ty } => {
                    let op_code = match op {
                        crate::ty::ConstExprUnaryOp::Negate => "neg",
                        crate::ty::ConstExprUnaryOp::BitwiseNot => "not",
                    };
                    format!(
                        "Eu{}{}{}",
                        op_code,
                        self.mangle_type(*ty),
                        self.mangle_const_generic(*expr)
                    )
                }
                crate::ty::ConstExprKind::Binary { op, lhs, rhs, ty } => {
                    let op_code = match op {
                        crate::ty::ConstExprBinaryOp::Add => "add",
                        crate::ty::ConstExprBinaryOp::Subtract => "sub",
                        crate::ty::ConstExprBinaryOp::Multiply => "mul",
                        crate::ty::ConstExprBinaryOp::Divide => "div",
                        crate::ty::ConstExprBinaryOp::Modulo => "mod",
                        crate::ty::ConstExprBinaryOp::BitwiseAnd => "and",
                        crate::ty::ConstExprBinaryOp::BitwiseOr => "or",
                        crate::ty::ConstExprBinaryOp::BitwiseXor => "xor",
                        crate::ty::ConstExprBinaryOp::ShiftLeft => "shl",
                        crate::ty::ConstExprBinaryOp::ShiftRight => "shr",
                    };
                    format!(
                        "Eb{}{}{}{}",
                        op_code,
                        self.mangle_type(*ty),
                        self.mangle_const_generic(*lhs),
                        self.mangle_const_generic(*rhs)
                    )
                }
                crate::ty::ConstExprKind::Cast { expr, ty } => {
                    format!(
                        "Ec{}{}",
                        self.mangle_type(*ty),
                        self.mangle_const_generic(*expr)
                    )
                }
            },
            crate::ty::ConstGeneric::Error => "Cerror".to_string(),
        }
    }

    fn mangle_generic_arg(&self, arg: crate::ty::GenericArg) -> String {
        match arg {
            crate::ty::GenericArg::Type(ty) => self.mangle_type(ty),
            crate::ty::GenericArg::Const(value) => self.mangle_const_generic(value),
        }
    }

    /// Generate a deterministic mangling suffix for a semantic type.
    pub fn mangle_type(&self, ty: TypeId) -> String {
        let norm_ty = self.type_registry.normalize(ty);
        match self.type_registry.get(norm_ty).clone() {
            crate::ty::TypeKind::Primitive(p) => match p {
                crate::ty::PrimitiveType::Void => "void".to_string(),
                crate::ty::PrimitiveType::Bool => "bool".to_string(),
                crate::ty::PrimitiveType::I8 => "i8".to_string(),
                crate::ty::PrimitiveType::I16 => "i16".to_string(),
                crate::ty::PrimitiveType::I32 => "i32".to_string(),
                crate::ty::PrimitiveType::I64 => "i64".to_string(),
                crate::ty::PrimitiveType::I128 => "i128".to_string(),
                crate::ty::PrimitiveType::ISize => "isize".to_string(),
                crate::ty::PrimitiveType::U8 => "u8".to_string(),
                crate::ty::PrimitiveType::U16 => "u16".to_string(),
                crate::ty::PrimitiveType::U32 => "u32".to_string(),
                crate::ty::PrimitiveType::U64 => "u64".to_string(),
                crate::ty::PrimitiveType::U128 => "u128".to_string(),
                crate::ty::PrimitiveType::USize => "usize".to_string(),
                crate::ty::PrimitiveType::F32 => "f32".to_string(),
                crate::ty::PrimitiveType::F64 => "f64".to_string(),
                crate::ty::PrimitiveType::Str => "str".to_string(),
                crate::ty::PrimitiveType::Never => "never".to_string(),
            },
            crate::ty::TypeKind::Simd { elem, lanes } => {
                let inner = self.mangle_type(elem);
                format!("Simd{}x{}", inner, lanes)
            }
            crate::ty::TypeKind::Pointer { is_mut, elem } => {
                let inner = self.mangle_type(elem);
                if is_mut {
                    format!("Pmut{}", inner)
                } else {
                    format!("P{}", inner)
                }
            }
            crate::ty::TypeKind::VolatilePtr { is_mut, elem } => {
                let inner = self.mangle_type(elem);
                if is_mut {
                    format!("Vmut{}", inner)
                } else {
                    format!("V{}", inner)
                }
            }
            crate::ty::TypeKind::Slice { is_mut, elem } => {
                let inner = self.mangle_type(elem);
                if is_mut {
                    format!("S{}_mut", inner)
                } else {
                    format!("S{}", inner)
                }
            }
            crate::ty::TypeKind::Array { elem, len } => {
                let inner = self.mangle_type(elem);
                format!("A{}{}", len, inner)
            }
            crate::ty::TypeKind::Def(def_id, gen_args)
            | crate::ty::TypeKind::Enum(def_id, gen_args)
            | crate::ty::TypeKind::TraitObject(def_id, gen_args, _) => {
                let base_name = self.def_qualified_name(def_id);

                if gen_args.is_empty() {
                    base_name
                } else {
                    let mut s = format!("{}I", base_name);
                    for arg in gen_args {
                        let arg_mangled = self.mangle_generic_arg(arg);
                        s.push_str(&format!("{}{}", arg_mangled.len(), arg_mangled));
                    }
                    s.push('E');
                    s
                }
            }
            crate::ty::TypeKind::Function { params, ret, .. }
            | crate::ty::TypeKind::ClosureInterface { params, ret } => {
                let mut s = String::from("F");
                for p in params {
                    let p_str = self.mangle_type(p);
                    s.push_str(&format!("{}{}", p_str.len(), p_str));
                }
                s.push('R');
                let r_str = self.mangle_type(ret);
                s.push_str(&format!("{}{}", r_str.len(), r_str));
                s
            }
            crate::ty::TypeKind::FnDef(def_id, gen_args) => {
                let base_name = self.def_qualified_name(def_id);
                if gen_args.is_empty() {
                    base_name
                } else {
                    let mut s = format!("{}I", base_name);
                    for arg in gen_args {
                        let arg_mangled = self.mangle_generic_arg(arg);
                        s.push_str(&format!("{}{}", arg_mangled.len(), arg_mangled));
                    }
                    s.push('E');
                    s
                }
            }
            crate::ty::TypeKind::AnonymousState {
                closure_node_id, ..
            } => {
                format!("ClosureState{}", closure_node_id.0)
            }
            crate::ty::TypeKind::AnonymousStruct(is_extern, fields) => {
                // Encoding: `AStr` + (field name len + name) + (type len + type) + `E`.
                let mut s = if is_extern {
                    String::from("EStr")
                } else {
                    String::from("AStr")
                };
                for f in fields {
                    let name_str = self.resolve(f.name);
                    s.push_str(&format!("{}{}", name_str.len(), name_str));
                    let ty_str = self.mangle_type(f.ty);
                    s.push_str(&format!("{}{}", ty_str.len(), ty_str));
                }
                s.push('E');
                s
            }
            crate::ty::TypeKind::AnonymousUnion(is_extern, fields) => {
                let mut s = if is_extern {
                    String::from("EUni")
                } else {
                    String::from("AUni")
                };
                for f in fields {
                    let name_str = self.resolve(f.name);
                    s.push_str(&format!("{}{}", name_str.len(), name_str));
                    let ty_str = self.mangle_type(f.ty);
                    s.push_str(&format!("{}{}", ty_str.len(), ty_str));
                }
                s.push('E');
                s
            }
            crate::ty::TypeKind::AnonymousEnum(enum_def) => {
                let mut s = String::from("AEnum");
                if let Some(backing_ty) = enum_def.backing_ty {
                    let backing = self.mangle_type(backing_ty);
                    s.push_str(&format!("B{}{}", backing.len(), backing));
                }
                for variant in &enum_def.variants {
                    let name_str = self.resolve(variant.name);
                    s.push_str(&format!("{}{}", name_str.len(), name_str));
                    if let Some(payload_ty) = variant.payload_ty {
                        let payload = self.mangle_type(payload_ty);
                        s.push_str(&format!("P{}{}", payload.len(), payload));
                    } else {
                        s.push('N');
                    }
                    if let Some(value) = variant.explicit_value {
                        s.push_str(&format!("V{}", value));
                    }
                    s.push('_');
                }
                s.push('E');
                s
            }
            crate::ty::TypeKind::AnonymousEnumPayload(enum_ty) => {
                let inner = self.mangle_type(enum_ty);
                format!("AEnumPayload{}{}", inner.len(), inner)
            }
            _ => "unknown".to_string(),
        }
    }

    /// Compute the final exported linker symbol for a definition instance.
    pub fn get_export_name_for_generic_args(
        &self,
        def_id: DefId,
        args: &[crate::ty::GenericArg],
    ) -> String {
        let def = &self.defs[def_id.0 as usize];
        let name_str = self.def_source_name(def_id);

        let empty_attrs: &[kernc_ast::Attribute] = &[]; // Reusable empty attribute slice.
        let (is_extern, attrs) = match def {
            Def::Function(f) => (f.is_extern, f.attributes.as_slice()),
            Def::Global(g) => (g.is_extern, g.attributes.as_slice()),
            Def::Struct(s) => (s.is_extern, s.attributes.as_slice()),
            Def::Enum(_) => (false, empty_attrs),
            Def::Union(u) => (u.is_extern, empty_attrs),
            _ => return name_str,
        };
        let parent_id = self.def_parent_for_path(def_id);

        // 1. `export_name` overrides the default symbol for monomorphic items.
        if args.is_empty() {
            for attr in attrs {
                if let kernc_ast::AttributeKind::Meta(items) = &attr.kind {
                    for item in items {
                        if let kernc_ast::MetaItem::Call(sym_id, arg_expr) = item
                            && self.resolve(*sym_id) == "export_name"
                            && let kernc_ast::ExprKind::String(ref s) = arg_expr.kind
                        {
                            return s.clone();
                        }
                    }
                }
            }
        }

        // 2. Plain extern items keep their source name.
        if is_extern && args.is_empty() {
            return name_str;
        }

        // 3. Build an Itanium-like path prefix.
        let mut mangled = String::from("_K");
        for comp in self.parent_path_components(parent_id).into_iter().rev() {
            mangled.push_str(&format!("{}{}", comp.len(), comp));
        }

        // 4. Append the item name and instantiated generic arguments.
        mangled.push_str(&format!("{}{}", name_str.len(), name_str));

        if !args.is_empty() {
            mangled.push('I');
            for &arg in args {
                let arg_mangled = self.mangle_generic_arg(arg);
                mangled.push_str(&format!("{}{}", arg_mangled.len(), arg_mangled));
            }
            mangled.push('E');
        }

        mangled
    }

    pub fn get_export_name(&self, def_id: DefId, args: &[TypeId]) -> String {
        self.get_export_name_for_generic_args(
            def_id,
            &crate::ty::wrap_type_args(args.iter().copied()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::SemaContext;
    use crate::def::{Def, DefId, FunctionDef, ModuleDef, StructDef, Visibility};
    use crate::scope::ScopeId;
    use crate::ty::TypeKind;
    use kernc_ast::{Attribute, TypeNode};
    use kernc_utils::{FileId, Session, Span};
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn mangled_named_types_include_module_qualification() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);

        let root_id = add_module(&mut ctx, "root", None);
        let left_id = add_module(&mut ctx, "left", Some(root_id));
        let right_id = add_module(&mut ctx, "right", Some(root_id));
        let left_seen = add_struct(&mut ctx, "SeenItem", Some(left_id));
        let right_seen = add_struct(&mut ctx, "SeenItem", Some(right_id));

        let left_ty = ctx
            .type_registry
            .intern(TypeKind::Def(left_seen, Vec::new()));
        let right_ty = ctx
            .type_registry
            .intern(TypeKind::Def(right_seen, Vec::new()));

        assert_ne!(ctx.mangle_type(left_ty), ctx.mangle_type(right_ty));
    }

    #[test]
    fn generic_export_names_distinguish_same_short_name_types() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);

        let root_id = add_module(&mut ctx, "root", None);
        let left_id = add_module(&mut ctx, "left", Some(root_id));
        let right_id = add_module(&mut ctx, "right", Some(root_id));
        let left_seen = add_struct(&mut ctx, "SeenItem", Some(left_id));
        let right_seen = add_struct(&mut ctx, "SeenItem", Some(right_id));
        let parse_id = add_function(&mut ctx, "parse", Some(root_id));

        let left_ty = ctx
            .type_registry
            .intern(TypeKind::Def(left_seen, Vec::new()));
        let right_ty = ctx
            .type_registry
            .intern(TypeKind::Def(right_seen, Vec::new()));

        assert_ne!(
            ctx.get_export_name(parse_id, &[left_ty]),
            ctx.get_export_name(parse_id, &[right_ty])
        );
    }

    #[test]
    fn exported_named_types_include_module_qualification() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);

        let root_id = add_module(&mut ctx, "root", None);
        let left_id = add_module(&mut ctx, "left", Some(root_id));
        let right_id = add_module(&mut ctx, "right", Some(root_id));
        let left_error = add_struct(&mut ctx, "Error", Some(left_id));
        let right_error = add_struct(&mut ctx, "Error", Some(right_id));

        assert_ne!(
            ctx.get_export_name(left_error, &[]),
            ctx.get_export_name(right_error, &[])
        );
    }

    #[test]
    fn mangled_fn_defs_include_module_qualification() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);

        let root_id = add_module(&mut ctx, "root", None);
        let left_id = add_module(&mut ctx, "left", Some(root_id));
        let right_id = add_module(&mut ctx, "right", Some(root_id));
        let left_parse = add_function(&mut ctx, "parse", Some(left_id));
        let right_parse = add_function(&mut ctx, "parse", Some(right_id));

        let left_ty = ctx
            .type_registry
            .intern(TypeKind::FnDef(left_parse, Vec::new()));
        let right_ty = ctx
            .type_registry
            .intern(TypeKind::FnDef(right_parse, Vec::new()));

        assert_ne!(ctx.mangle_type(left_ty), ctx.mangle_type(right_ty));
    }

    #[test]
    fn package_visibility_allows_same_package_across_module_roots() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);

        let app_root = add_module(&mut ctx, "app", None);
        let dep_root = add_module(&mut ctx, "dep", None);
        let dep_inner = add_module(&mut ctx, "inner", Some(dep_root));
        let package = ctx.intern("bed");
        ctx.root_module_package_names.insert(app_root, package);
        ctx.root_module_package_names.insert(dep_root, package);

        assert!(ctx.visibility_allows_access(Visibility::Package, dep_inner, Some(app_root)));
    }

    #[test]
    fn package_visibility_denies_different_packages_across_module_roots() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);

        let app_root = add_module(&mut ctx, "app", None);
        let dep_root = add_module(&mut ctx, "dep", None);
        let dep_inner = add_module(&mut ctx, "inner", Some(dep_root));
        let app_package = ctx.intern("app");
        let dep_package = ctx.intern("dep");
        ctx.root_module_package_names.insert(app_root, app_package);
        ctx.root_module_package_names.insert(dep_root, dep_package);

        assert!(!ctx.visibility_allows_access(Visibility::Package, dep_inner, Some(app_root)));
    }

    fn add_module(ctx: &mut SemaContext<'_>, name: &str, parent: Option<DefId>) -> DefId {
        let id = DefId(ctx.defs.len() as u32);
        let scope_id = ScopeId(id.0 as usize);
        let name = ctx.intern(name);
        let def_id = ctx.add_def(Def::Module(ModuleDef {
            id,
            name,
            parent,
            is_imported: false,
            scope_id,
            dir_path: PathBuf::new(),
            file_id: FileId(0),
            submodules: HashMap::new(),
            items: Vec::new(),
            imports: Vec::new(),
            is_init: parent.is_none(),
            docs: None,
        }));
        ctx.register_module_scope(def_id, scope_id);
        def_id
    }

    fn add_struct(ctx: &mut SemaContext<'_>, name: &str, parent_module: Option<DefId>) -> DefId {
        let id = DefId(ctx.defs.len() as u32);
        let name = ctx.intern(name);
        let def_id = ctx.add_def(Def::Struct(StructDef {
            id,
            name,
            vis: Visibility::Private,
            parent_module,
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            fields: Vec::new(),
            is_extern: false,
            span: Span::default(),
            docs: None,
            attributes: Vec::new(),
        }));
        ctx.register_def_owner(def_id, parent_module, None);
        def_id
    }

    fn add_function(ctx: &mut SemaContext<'_>, name: &str, parent: Option<DefId>) -> DefId {
        let id = DefId(ctx.defs.len() as u32);
        let name = ctx.intern(name);
        let type_node = TypeNode {
            id: ctx.next_node_id(),
            span: Span::default(),
            kind: kernc_ast::TypeKind::Infer,
        };
        let def_id = ctx.add_def(Def::Function(FunctionDef {
            id,
            name,
            name_span: Span::default(),
            vis: Visibility::Private,
            parent,
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            params: Vec::new(),
            ret_type: type_node,
            body: None,
            is_const: false,
            is_extern: false,
            is_variadic: false,
            is_intrinsic: false,
            span: Span::default(),
            resolved_sig: None,
            docs: None,
            attributes: Vec::<Attribute>::new(),
        }));
        ctx.register_def_owner(def_id, None, None);
        def_id
    }
}
