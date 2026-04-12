mod materialize;
mod plan;
mod refs;
#[cfg(test)]
mod tests;
mod workload;

use kernc_mast::{MastFunction, MastGlobal, MastLinkage, MastModule};
use kernc_mono::MonoId;
use std::collections::{HashMap, HashSet};

pub(in crate::compiler) use materialize::materialize_codegen_unit;
pub(in crate::compiler) use plan::{
    plan_codegen_units_with_mir_summary, plan_codegen_units_with_mir_workload,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodegenPlanReport {
    pub requested_units: usize,
    pub root_count: usize,
    pub cluster_count: usize,
    pub planned_units: usize,
    pub total_workload: usize,
    pub min_cluster_workload: usize,
    pub max_cluster_workload: usize,
    pub min_unit_workload: usize,
    pub max_unit_workload: usize,
    pub promoted_function_count: usize,
    pub promoted_global_count: usize,
    pub imported_function_count: usize,
    pub import_plan: Option<CodegenImportPlanReport>,
    pub fallback_reason: Option<CodegenPlanFallback>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CodegenImportPlanReport {
    pub candidate_function_count: usize,
    pub accepted_candidate_count: usize,
    pub rejected_for_budget_count: usize,
    pub total_budget: usize,
    pub min_unit_budget: usize,
    pub max_unit_budget: usize,
    pub total_candidate_score: usize,
    pub imported_score: usize,
    pub imported_workload: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodegenPlanFallback {
    RequestedSingleUnit,
    ContainsControlFlowAsm {
        function_name: String,
    },
    NameCollision {
        item_kind: &'static str,
        name: String,
    },
    TooFewRoots,
    TooFewTargetUnits,
    TooFewMaterializedUnits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CodegenPlanOutcome {
    pub(super) units: Vec<CodegenUnitPlan>,
    pub(super) report: CodegenPlanReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ItemKey {
    Function(MonoId),
    Global(MonoId),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ItemRefs {
    functions: HashSet<MonoId>,
    globals: HashSet<MonoId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CodegenUnitPlan {
    pub(super) name: String,
    root_keys: Vec<ItemKey>,
    pub(super) function_ids: HashSet<MonoId>,
    pub(super) global_ids: HashSet<MonoId>,
    pub(super) imported_function_ids: HashSet<MonoId>,
    pub(super) promoted_function_ids: HashSet<MonoId>,
    pub(super) promoted_global_ids: HashSet<MonoId>,
    pub(super) workload: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClusterPlan {
    root_keys: Vec<ItemKey>,
    function_ids: HashSet<MonoId>,
    global_ids: HashSet<MonoId>,
    promoted_function_ids: HashSet<MonoId>,
    promoted_global_ids: HashSet<MonoId>,
    workload: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodegenNameCollision {
    item_kind: &'static str,
    name: String,
}
