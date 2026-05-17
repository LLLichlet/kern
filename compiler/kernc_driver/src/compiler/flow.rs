mod cfg;
mod collect;
mod control;
mod dataflow;
mod optimize;

use super::{
    AnalysisDeadStore, AnalysisDeadStoreKind, AnalysisFlowBindingId, AnalysisFlowBindingKind,
    AnalysisFlowCfg, AnalysisFlowCfgEdge, AnalysisFlowCfgEdgeKind, AnalysisFlowCfgNode,
    AnalysisFlowCfgNodeKind, AnalysisFlowDefinitionFacts, AnalysisFlowDefinitionKind,
    AnalysisFlowNodeEffects, AnalysisFlowNodeId, AnalysisFlowOwnerKind, AnalysisFlowRegionKind,
    AnalysisFlowSummary,
};
use kernc_ast as ast;
use kernc_flow::{ComputedLiveness, FlowBindingFacts, FlowOwnerFacts, FlowRegionFacts};
use kernc_sema::SemaContext;
use kernc_sema::def::DefId;
use kernc_utils::{Canceled, CancellationToken, Span};
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
