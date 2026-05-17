mod cfg;
mod collect;
mod control;
mod dataflow;
mod optimize;

use super::{
    AnalysisDeadStore, AnalysisDeadStoreKind, AnalysisFlowBindingId, AnalysisFlowBindingKind,
    AnalysisFlowBindingSummary, AnalysisFlowCfg, AnalysisFlowCfgEdge, AnalysisFlowCfgEdgeKind,
    AnalysisFlowCfgNode, AnalysisFlowCfgNodeKind, AnalysisFlowDefinitionFacts,
    AnalysisFlowDefinitionKind, AnalysisFlowNodeEffects, AnalysisFlowNodeId, AnalysisFlowOwnerKind,
    AnalysisFlowRegionKind, AnalysisFlowSingleSourceUse, AnalysisFlowSummary,
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
    owner_lookup_by_def_id: HashMap<DefId, usize>,
    referenced_item_edges: Vec<(DefId, DefId)>,
    phase_timings: Vec<FlowTiming>,
}

pub(in crate::compiler) struct FlowFunctionValueFacts<'a> {
    pub owner: &'a FlowOwnerFacts,
    binding_by_id: HashMap<AnalysisFlowBindingId, &'a FlowBindingFacts>,
    binding_summary_by_id: HashMap<AnalysisFlowBindingId, &'a AnalysisFlowBindingSummary>,
    single_source_by_node_binding:
        HashMap<(AnalysisFlowNodeId, AnalysisFlowBindingId), &'a AnalysisFlowSingleSourceUse>,
}

impl<'a> FlowFunctionValueFacts<'a> {
    fn new(owner: &'a FlowOwnerFacts) -> Self {
        Self {
            owner,
            binding_by_id: owner
                .bindings
                .iter()
                .map(|binding| (binding.id, binding))
                .collect(),
            binding_summary_by_id: owner
                .binding_summaries
                .iter()
                .map(|summary| (summary.binding_id, summary))
                .collect(),
            single_source_by_node_binding: owner
                .single_source_uses
                .iter()
                .map(|single| ((single.node_id, single.binding_id), single))
                .collect(),
        }
    }

    pub fn single_source_use_for(
        &self,
        node_id: AnalysisFlowNodeId,
        binding_id: AnalysisFlowBindingId,
    ) -> Option<&AnalysisFlowSingleSourceUse> {
        self.single_source_by_node_binding
            .get(&(node_id, binding_id))
            .copied()
    }

    pub fn binding(&self, binding_id: AnalysisFlowBindingId) -> Option<&FlowBindingFacts> {
        self.binding_by_id.get(&binding_id).copied()
    }

    pub fn binding_summary(
        &self,
        binding_id: AnalysisFlowBindingId,
    ) -> Option<&AnalysisFlowBindingSummary> {
        self.binding_summary_by_id.get(&binding_id).copied()
    }
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
