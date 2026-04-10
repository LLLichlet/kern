mod cfg;
mod collect;
mod control;
mod dataflow;
mod optimize;

use self::dataflow::ComputedLiveness;
use super::{
    AnalysisDeadStore, AnalysisDeadStoreKind, AnalysisFlowBinding, AnalysisFlowBindingId,
    AnalysisFlowBindingKind, AnalysisFlowBindingSummary, AnalysisFlowCfg, AnalysisFlowCfgEdge,
    AnalysisFlowCfgEdgeKind, AnalysisFlowCfgNode, AnalysisFlowCfgNodeKind, AnalysisFlowDefUse,
    AnalysisFlowDefinitionFacts, AnalysisFlowDefinitionKind, AnalysisFlowDefinitionRef,
    AnalysisFlowLiveness, AnalysisFlowNodeEffects, AnalysisFlowNodeFacts, AnalysisFlowNodeId,
    AnalysisFlowNodeTransfer, AnalysisFlowOwnerKind, AnalysisFlowReaching,
    AnalysisFlowRegionKind, AnalysisFlowResolvedUse, AnalysisFlowSingleSourceUse,
    AnalysisFlowSummary, AnalysisFlowUseDef,
};
use kernc_ast as ast;
use kernc_sema::SemaContext;
use kernc_sema::def::DefId;
use kernc_utils::Span;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlowTiming {
    pub name: &'static str,
    pub duration: Duration,
}

#[derive(Clone, Default)]
pub struct FlowModel {
    owners: Vec<FlowOwnerFacts>,
    owner_body_lookup_by_file: HashMap<kernc_utils::FileId, Vec<(Span, DefId)>>,
    referenced_item_edges: Vec<(DefId, DefId)>,
    phase_timings: Vec<FlowTiming>,
}

#[derive(Clone)]
struct FlowOwnerFacts {
    def_id: DefId,
    definition_span: Span,
    owner_span: Span,
    body_span: Span,
    kind: AnalysisFlowOwnerKind,
    referenced_def_ids: Vec<DefId>,
    referenced_definition_spans: Vec<Span>,
    cfg: AnalysisFlowCfg,
    node_facts: Vec<AnalysisFlowNodeFacts>,
    node_effects: Vec<AnalysisFlowNodeEffects>,
    node_transfers: Vec<AnalysisFlowNodeTransfer>,
    use_defs: Vec<AnalysisFlowUseDef>,
    def_uses: Vec<super::AnalysisFlowDefUse>,
    definition_facts: Vec<AnalysisFlowDefinitionFacts>,
    resolved_uses: Vec<AnalysisFlowResolvedUse>,
    single_source_uses: Vec<AnalysisFlowSingleSourceUse>,
    liveness: Vec<AnalysisFlowLiveness>,
    computed_liveness: Option<ComputedLiveness>,
    reaching_definitions: Vec<AnalysisFlowReaching>,
    control_regions: Vec<FlowRegionFacts>,
    summary: AnalysisFlowSummary,
    bindings: Vec<FlowBindingFacts>,
    binding_summaries: Vec<AnalysisFlowBindingSummary>,
}

#[derive(Clone)]
struct FlowBindingFacts {
    id: AnalysisFlowBindingId,
    definition_span: Span,
    kind: AnalysisFlowBindingKind,
    is_mut: bool,
    reference_spans: Vec<Span>,
}

#[derive(Clone, Copy)]
struct FlowRegionFacts {
    span: Span,
    kind: AnalysisFlowRegionKind,
}

#[derive(Clone, Copy)]
struct PendingEdge {
    from: AnalysisFlowNodeId,
    kind: AnalysisFlowCfgEdgeKind,
}

#[derive(Clone, Copy)]
struct LoopContext {
    break_target: AnalysisFlowNodeId,
    continue_target: AnalysisFlowNodeId,
}

struct FlowCfgBuilder<'a> {
    nodes: Vec<AnalysisFlowCfgNode>,
    edges: Vec<AnalysisFlowCfgEdge>,
    incoming_counts: Vec<usize>,
    node_uses: Vec<Vec<AnalysisFlowBindingId>>,
    node_value_uses: Vec<Vec<AnalysisFlowBindingId>>,
    node_defs: Vec<Vec<AnalysisFlowBindingId>>,
    node_def_kinds: Vec<Option<AnalysisFlowDefinitionKind>>,
    node_copy_sources: Vec<Option<AnalysisFlowBindingId>>,
    node_effects: Vec<AnalysisFlowNodeEffects>,
    local_bindings_by_span: &'a HashMap<Span, AnalysisFlowBindingId>,
    reference_to_binding: &'a HashMap<Span, AnalysisFlowBindingId>,
    entry: AnalysisFlowNodeId,
    exit: AnalysisFlowNodeId,
}

struct FlowCfgBuildResult {
    cfg: AnalysisFlowCfg,
    node_uses: Vec<Vec<AnalysisFlowBindingId>>,
    node_value_uses: Vec<Vec<AnalysisFlowBindingId>>,
    node_defs: Vec<Vec<AnalysisFlowBindingId>>,
    node_def_kinds: Vec<Option<AnalysisFlowDefinitionKind>>,
    node_copy_sources: Vec<Option<AnalysisFlowBindingId>>,
    node_effects: Vec<AnalysisFlowNodeEffects>,
}
