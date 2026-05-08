#![doc = include_str!("../README.md")]

mod dataflow;

pub use dataflow::{
    CfgTopology, ComputedLiveness, ComputedReaching, collect_binding_summaries, collect_def_uses,
    collect_definition_facts, collect_node_facts, collect_node_transfers, collect_resolved_uses,
    collect_single_source_uses, collect_use_defs, compute_liveness, compute_reaching_definitions,
    materialize_liveness, materialize_reaching_definitions,
};
pub use kernc_middle::NodeFacts;
use kernc_ty::DefId;
use kernc_utils::{NodeId, Span};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisFlowOwnerKind {
    Function,
    Constant,
    Static,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisFlowBindingKind {
    Variable,
    Parameter,
    Static,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisFlowRegionKind {
    Block,
    If,
    Match,
    MatchArm,
    Loop,
    Closure,
    Defer,
    Return,
    Break,
    Continue,
    LetElse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisFlowCfgNodeKind {
    Entry,
    Exit,
    Eval,
    Branch,
    Match,
    MatchArm,
    LoopHead,
    LoopLatch,
    Join,
    Return,
    Break,
    Continue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisFlowCfgEdgeKind {
    Next,
    TrueBranch,
    FalseBranch,
    CaseBranch,
    LoopBack,
    BreakFlow,
    ContinueFlow,
    ReturnFlow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AnalysisFlowNodeId(pub usize);

impl AnalysisFlowNodeId {
    pub const fn index(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AnalysisFlowBindingId(pub usize);

impl AnalysisFlowBindingId {
    pub const fn index(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone)]
pub struct AnalysisFlowCfgNode {
    pub id: AnalysisFlowNodeId,
    pub span: Span,
    pub kind: AnalysisFlowCfgNodeKind,
    pub ast_node_id: Option<NodeId>,
}

#[derive(Debug, Clone)]
pub struct AnalysisFlowCfgEdge {
    pub from: AnalysisFlowNodeId,
    pub to: AnalysisFlowNodeId,
    pub kind: AnalysisFlowCfgEdgeKind,
}

#[derive(Debug, Clone)]
pub struct AnalysisFlowCfg {
    pub entry: AnalysisFlowNodeId,
    pub exit: AnalysisFlowNodeId,
    pub nodes: Vec<AnalysisFlowCfgNode>,
    pub edges: Vec<AnalysisFlowCfgEdge>,
}

#[derive(Debug, Clone)]
pub struct AnalysisFlowLiveness {
    pub node_id: AnalysisFlowNodeId,
    pub live_in: Vec<AnalysisFlowBindingId>,
    pub live_out: Vec<AnalysisFlowBindingId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AnalysisFlowDefinitionRef {
    pub binding_id: AnalysisFlowBindingId,
    pub node_id: AnalysisFlowNodeId,
}

#[derive(Debug, Clone)]
pub struct AnalysisFlowReaching {
    pub node_id: AnalysisFlowNodeId,
    pub reaching_in: Vec<AnalysisFlowDefinitionRef>,
    pub reaching_out: Vec<AnalysisFlowDefinitionRef>,
}

#[derive(Debug, Clone)]
pub struct AnalysisFlowRegion {
    pub span: Span,
    pub kind: AnalysisFlowRegionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnalysisFlowSummary {
    pub block_count: usize,
    pub branch_count: usize,
    pub loop_count: usize,
    pub closure_count: usize,
    pub defer_count: usize,
    pub return_count: usize,
    pub break_count: usize,
    pub continue_count: usize,
    pub let_else_count: usize,
}

#[derive(Debug, Clone)]
pub struct AnalysisFlowBinding {
    pub id: AnalysisFlowBindingId,
    pub definition_span: Span,
    pub kind: AnalysisFlowBindingKind,
    pub is_mut: bool,
    pub reference_spans: Vec<Span>,
}

#[derive(Debug, Clone)]
pub struct AnalysisFlowBindingSummary {
    pub binding_id: AnalysisFlowBindingId,
    pub definition_node_ids: Vec<AnalysisFlowNodeId>,
    pub use_node_ids: Vec<AnalysisFlowNodeId>,
    pub live_node_ids: Vec<AnalysisFlowNodeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisFlowDefinitionKind {
    Initializer,
    Assignment,
}

#[derive(Debug, Clone)]
pub struct AnalysisFlowNodeFacts {
    pub node_id: AnalysisFlowNodeId,
    pub use_binding_ids: Vec<AnalysisFlowBindingId>,
    pub define_binding_ids: Vec<AnalysisFlowBindingId>,
    pub definition_kind: Option<AnalysisFlowDefinitionKind>,
}

#[derive(Debug, Clone)]
pub struct AnalysisFlowNodeTransfer {
    pub node_id: AnalysisFlowNodeId,
    pub use_binding_ids: Vec<AnalysisFlowBindingId>,
    pub kill_binding_ids: Vec<AnalysisFlowBindingId>,
    pub generate_definitions: Vec<AnalysisFlowDefinitionRef>,
}

#[derive(Debug, Clone)]
pub struct AnalysisFlowNodeEffects {
    pub node_id: AnalysisFlowNodeId,
    pub has_call: bool,
    pub has_memory_read: bool,
    pub has_memory_write: bool,
    pub has_control_flow: bool,
    pub is_pure: bool,
}

#[derive(Debug, Clone)]
pub struct AnalysisFlowUseDef {
    pub node_id: AnalysisFlowNodeId,
    pub binding_id: AnalysisFlowBindingId,
    pub reaching_definitions: Vec<AnalysisFlowDefinitionRef>,
}

#[derive(Debug, Clone)]
pub struct AnalysisFlowDefUse {
    pub definition: AnalysisFlowDefinitionRef,
    pub use_node_ids: Vec<AnalysisFlowNodeId>,
}

#[derive(Debug, Clone)]
pub struct AnalysisFlowDefinitionFacts {
    pub definition: AnalysisFlowDefinitionRef,
    pub kind: AnalysisFlowDefinitionKind,
    pub use_binding_ids: Vec<AnalysisFlowBindingId>,
    pub copy_source_binding_id: Option<AnalysisFlowBindingId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisFlowResolvedUseKind {
    Missing,
    Unique,
    Ambiguous,
}

#[derive(Debug, Clone)]
pub struct AnalysisFlowResolvedUse {
    pub node_id: AnalysisFlowNodeId,
    pub binding_id: AnalysisFlowBindingId,
    pub kind: AnalysisFlowResolvedUseKind,
    pub candidate_definitions: Vec<AnalysisFlowDefinitionRef>,
}

#[derive(Debug, Clone)]
pub struct AnalysisFlowSingleSourceUse {
    pub node_id: AnalysisFlowNodeId,
    pub binding_id: AnalysisFlowBindingId,
    pub definition: AnalysisFlowDefinitionRef,
    pub definition_kind: AnalysisFlowDefinitionKind,
    pub copy_source_binding_id: Option<AnalysisFlowBindingId>,
}

#[derive(Debug, Clone)]
pub struct AnalysisFlowOwner {
    pub definition_span: Span,
    pub body_span: Span,
    pub kind: AnalysisFlowOwnerKind,
    pub referenced_definition_spans: Vec<Span>,
    pub cfg: AnalysisFlowCfg,
    pub node_facts: Vec<AnalysisFlowNodeFacts>,
    pub node_effects: Vec<AnalysisFlowNodeEffects>,
    pub node_transfers: Vec<AnalysisFlowNodeTransfer>,
    pub use_defs: Vec<AnalysisFlowUseDef>,
    pub def_uses: Vec<AnalysisFlowDefUse>,
    pub definition_facts: Vec<AnalysisFlowDefinitionFacts>,
    pub resolved_uses: Vec<AnalysisFlowResolvedUse>,
    pub single_source_uses: Vec<AnalysisFlowSingleSourceUse>,
    pub liveness: Vec<AnalysisFlowLiveness>,
    pub reaching_definitions: Vec<AnalysisFlowReaching>,
    pub control_regions: Vec<AnalysisFlowRegion>,
    pub summary: AnalysisFlowSummary,
    pub bindings: Vec<AnalysisFlowBinding>,
    pub binding_summaries: Vec<AnalysisFlowBindingSummary>,
}

#[derive(Debug, Clone, Default)]
pub struct FlowLoweringHints {
    owners: HashMap<DefId, FlowLoweringOwnerHints>,
}

#[derive(Debug, Clone, Default)]
pub struct FlowLoweringOwnerHints {
    pub elision: FlowLoweringElisionHints,
    pub forwarding: FlowLoweringForwardingHints,
}

#[derive(Debug, Clone, Default)]
pub struct FlowLoweringElisionHints {
    pub pure_dead_initializer_expr_ids: HashSet<NodeId>,
    pub pure_dead_assignment_expr_ids: HashSet<NodeId>,
    pub elidable_binding_expr_ids: HashSet<NodeId>,
}

#[derive(Debug, Clone, Default)]
pub struct FlowLoweringForwardingHints {
    pub identifier_copy_sources: HashMap<NodeId, String>,
    pub forwardable_binding_sources: HashMap<NodeId, String>,
    pub forwardable_value_expr_ids: HashSet<NodeId>,
}

impl FlowLoweringHints {
    pub fn insert_owner(&mut self, def_id: DefId, hints: FlowLoweringOwnerHints) {
        self.owners.insert(def_id, hints);
    }

    pub fn owner(&self, def_id: DefId) -> Option<&FlowLoweringOwnerHints> {
        self.owners.get(&def_id)
    }
}
