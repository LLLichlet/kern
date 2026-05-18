use super::ownership::ModuleOwnershipState;
use super::semantic_index::SemanticIndexState;
use super::*;
use std::time::Duration;

type NamedFieldQueryKey = (Option<DefId>, DefId, Vec<GenericArg>, SymbolId);
type NamedFieldQueryValue = Option<crate::query::MemberCandidate>;
type MemberResolutionQueryKey = (Option<DefId>, TypeId, SymbolId);
type MethodResolutionQueryKey = (TypeId, SymbolId);
type GenericBoundsCheckKey = (DefId, Vec<GenericArg>);

#[derive(Clone, Default)]
pub(crate) struct SemaQueryCacheState {
    // These caches only store facts derivable from the current semantic graph. Any structural
    // rollback must invalidate them so later passes do not observe stale trait or member results.
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
    pub(crate) active_impl_requirement_cycle_queries: FastHashSet<DefId>,
    pub(crate) impl_paterson_boundedness_cache:
        FastHashMap<DefId, Option<NonDecreasingImplRequirement>>,
    pub(crate) active_impl_paterson_boundedness_queries: FastHashSet<DefId>,
    pub(crate) generic_bounds_success_cache: FastHashSet<GenericBoundsCheckKey>,
    pub(crate) named_field_query_cache: FastHashMap<NamedFieldQueryKey, NamedFieldQueryValue>,
    pub(crate) member_resolution_query_cache:
        FastHashMap<MemberResolutionQueryKey, crate::query::MemberResolution>,
    pub(crate) method_resolution_query_cache:
        FastHashMap<MethodResolutionQueryKey, Option<crate::query::MemberResolution>>,
}

impl SemaQueryCacheState {
    pub(crate) fn clear_all(&mut self) {
        self.call_signature_instantiation_cache.clear();
        self.field_type_subst_cache.clear();
        self.trait_method_query_cache.clear();
        self.impl_method_query_cache.clear();
        self.bound_trait_match_cache.clear();
        self.impl_applicability_cache.clear();
        self.impl_requirement_cycle_cache.clear();
        self.active_impl_requirement_cycle_queries.clear();
        self.impl_paterson_boundedness_cache.clear();
        self.active_impl_paterson_boundedness_queries.clear();
        self.generic_bounds_success_cache.clear();
        self.named_field_query_cache.clear();
        self.member_resolution_query_cache.clear();
        self.method_resolution_query_cache.clear();
    }

    pub(crate) fn clear_active_bound_caches(&mut self) {
        self.bound_trait_match_cache.clear();
        self.impl_applicability_cache.clear();
        self.impl_method_query_cache.clear();
        self.generic_bounds_success_cache.clear();
        self.member_resolution_query_cache.clear();
        self.method_resolution_query_cache.clear();
    }
}

#[derive(Clone, Default)]
pub(crate) struct RecursiveReportState {
    pub(crate) reported_recursive_layout_types: FastHashSet<TypeId>,
    pub(crate) reported_recursive_projection_types: FastHashSet<TypeId>,
    pub(crate) reported_recursive_projection_assoc_defs: FastHashSet<DefId>,
}

#[derive(Clone, Default)]
pub struct SemaAnalysisState {
    pub active_bounds: Vec<(TypeId, Vec<TypeId>)>,
    pub(crate) expr_timing_stats: ExprTimingStats,
    pub(crate) query_caches: SemaQueryCacheState,
    pub(crate) recursive_reports: RecursiveReportState,
    pub(crate) semantic_index: SemanticIndexState,
    pub(crate) escape_summaries: FastHashMap<DefId, EscapeSummary>,
    pub(crate) pending_escape_checks: Vec<PendingEscapeCheck>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct EscapeSummary {
    pub(crate) stored_params: FastHashSet<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingEscapeCheck {
    pub(crate) callee: DefId,
    pub(crate) arg_index: usize,
    pub(crate) origin: crate::checker::expr::PointerOrigin,
}

#[derive(Clone, Default)]
pub struct SemaImplIndexState {
    pub global_impls: Vec<DefId>,
    pub trait_impls: Vec<DefId>,
    pub trait_impls_by_trait_key: FastHashMap<String, Vec<DefId>>,
    pub impl_methods_by_name: FastHashMap<SymbolId, Vec<DefId>>,
}

#[derive(Clone, Default)]
pub struct SemaResolutionState {
    pub builtin_defs: FastHashMap<SymbolId, DefId>,
    pub current_package_name: Option<SymbolId>,
    pub module_aliases: HashMap<String, String>,
    pub module_interface_aliases: HashMap<String, String>,
    pub(crate) module_ownership: ModuleOwnershipState,
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
