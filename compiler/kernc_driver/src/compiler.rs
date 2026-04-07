mod analysis;
mod completion;
mod flow;
mod link;
mod pipeline;
mod signature;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use kernc_ast as ast;
use kernc_db::Memo;
use kernc_sema::SemaStructureSnapshot;
use kernc_sema::def::DefId;
use kernc_sema::scope::ScopeId;
use kernc_sema::ty::TypeId;
use kernc_utils::Session;
use kernc_utils::SymbolId;
use kernc_utils::config::CompileOptions;

use crate::frontend::FrontendDatabase;

pub type SourceOverrides = HashMap<PathBuf, String>;

pub struct AnalysisReport {
    pub session: Session,
    pub succeeded: bool,
}

#[derive(Debug, Clone)]
pub struct CompileReport {
    pub loaded_sources: Vec<PathBuf>,
    pub phase_timings: Vec<PhaseTiming>,
    pub mast_workload: Option<kernc_mast::MastWorkloadStats>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhaseTiming {
    pub name: &'static str,
    pub duration: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisSpanReplacement {
    pub clean: kernc_utils::Span,
    pub dirty: kernc_utils::Span,
}

pub struct TargetedAnalysisReport {
    pub report: AnalysisReport,
    pub replaced_spans: Vec<AnalysisSpanReplacement>,
}

#[derive(Debug, Clone)]
pub struct AnalysisReference {
    pub reference_span: kernc_utils::Span,
    pub definition_span: kernc_utils::Span,
}

#[derive(Debug, Clone)]
pub struct AnalysisHover {
    pub span: kernc_utils::Span,
    pub contents: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisSemanticRole {
    Definition,
    Reference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisSemanticKind {
    Module,
    Namespace,
    Struct,
    Enum,
    Interface,
    Type,
    TypeParameter,
    Property,
    Variable,
    Parameter,
    Function,
    Method,
    Constant,
    Static,
}

#[derive(Debug, Clone)]
pub struct AnalysisSemanticEntry {
    pub span: kernc_utils::Span,
    pub definition_span: kernc_utils::Span,
    pub kind: AnalysisSemanticKind,
    pub role: AnalysisSemanticRole,
    pub is_mut: bool,
    pub is_pub: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisCompletionKind {
    Variable,
    Function,
    Module,
    Struct,
    Union,
    Enum,
    Trait,
    TypeAlias,
    Constant,
    Static,
    TypeParameter,
}

#[derive(Debug, Clone)]
pub struct AnalysisCompletionItem {
    pub label: String,
    pub kind: AnalysisCompletionKind,
    pub detail: Option<String>,
    pub insert_text: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AnalysisParameterInformation {
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct AnalysisSignatureInformation {
    pub label: String,
    pub parameters: Vec<AnalysisParameterInformation>,
}

#[derive(Debug, Clone)]
pub struct AnalysisSignatureHelp {
    pub signatures: Vec<AnalysisSignatureInformation>,
    pub active_signature: usize,
    pub active_parameter: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisSymbolKind {
    Module,
    Namespace,
    Function,
    Method,
    Struct,
    Union,
    Enum,
    Trait,
    TypeAlias,
    Constant,
    Static,
}

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
    pub span: kernc_utils::Span,
    pub kind: AnalysisFlowCfgNodeKind,
    pub ast_node_id: Option<kernc_utils::NodeId>,
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
    pub span: kernc_utils::Span,
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
    pub definition_span: kernc_utils::Span,
    pub kind: AnalysisFlowBindingKind,
    pub is_mut: bool,
    pub reference_spans: Vec<kernc_utils::Span>,
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
    pub definition_span: kernc_utils::Span,
    pub body_span: kernc_utils::Span,
    pub kind: AnalysisFlowOwnerKind,
    pub referenced_definition_spans: Vec<kernc_utils::Span>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisUnusedItemKind {
    Function,
    Constant,
    Static,
}

#[derive(Debug, Clone)]
pub struct AnalysisUnusedItem {
    pub definition_span: kernc_utils::Span,
    pub kind: AnalysisUnusedItemKind,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisUnusedBindingKind {
    Variable,
    Parameter,
}

#[derive(Debug, Clone)]
pub struct AnalysisUnusedBinding {
    pub definition_span: kernc_utils::Span,
    pub kind: AnalysisUnusedBindingKind,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnalysisDeadStoreKind {
    Initializer,
    Assignment,
}

#[derive(Debug, Clone)]
pub struct AnalysisDeadStore {
    pub span: kernc_utils::Span,
    pub node_id: AnalysisFlowNodeId,
    pub binding_id: AnalysisFlowBindingId,
    pub binding_definition_span: kernc_utils::Span,
    pub kind: AnalysisDeadStoreKind,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct AnalysisSymbol {
    pub name: String,
    pub kind: AnalysisSymbolKind,
    pub span: kernc_utils::Span,
    pub selection_span: kernc_utils::Span,
    pub detail: Option<String>,
    pub children: Vec<AnalysisSymbol>,
}

pub struct AnalysisArtifact {
    pub session: Session,
    pub succeeded: bool,
    pub symbols: Vec<AnalysisSymbol>,
    pub references: Vec<AnalysisReference>,
    pub hovers: Vec<AnalysisHover>,
    pub semantic_entries: Vec<AnalysisSemanticEntry>,
    asts: Vec<(DefId, ast::Module)>,
    resolved_globals: Vec<ResolvedGlobalType>,
    completion_model: completion::CompletionModel,
    signature_model: signature::SignatureModel,
    flow_model: flow::FlowModel,
    unused_items: Vec<AnalysisUnusedItem>,
    unused_bindings: Vec<AnalysisUnusedBinding>,
    dead_stores: Vec<AnalysisDeadStore>,
}

pub struct AnalysisOutline {
    pub session: Session,
    pub symbols: Vec<AnalysisSymbol>,
}

pub struct AnalysisSurfaceArtifact {
    pub session: Session,
    pub symbols: Vec<AnalysisSymbol>,
    completion_model: completion::CompletionModel,
}

pub struct ParsedModuleArtifact {
    session: Session,
    modules: Vec<ParsedModule>,
}

#[derive(Clone)]
struct ParsedModule {
    name: String,
    file_id: kernc_utils::FileId,
    path: PathBuf,
    body_regions: Vec<kernc_utils::Span>,
    ast: ast::Module,
}

#[derive(Clone)]
struct CollectedStructureArtifact {
    session: Session,
    asts: Vec<(DefId, ast::Module)>,
    symbols: Vec<AnalysisSymbol>,
    snapshot: SemaStructureSnapshot,
}

#[derive(Clone)]
pub struct ImportedStructureArtifact {
    session: Session,
    asts: Vec<(DefId, ast::Module)>,
    symbols: Vec<AnalysisSymbol>,
    snapshot: SemaStructureSnapshot,
    completion_model: completion::CompletionModel,
}

#[derive(Clone)]
pub struct StructureArtifact {
    session: Session,
    asts: Vec<(DefId, ast::Module)>,
    symbols: Vec<AnalysisSymbol>,
    snapshot: SemaStructureSnapshot,
    completion_model: completion::CompletionModel,
}

#[derive(Clone)]
pub(super) struct CompileStructureArtifact {
    pub(super) session: Session,
    pub(super) snapshot: SemaStructureSnapshot,
    pub(super) phase_timings: Vec<PhaseTiming>,
}

pub struct CompilerDriver {
    pub options: CompileOptions,
    frontend: FrontendDatabase,
    collected_artifacts: Memo<StructureCacheKey, Option<CollectedStructureArtifact>>,
    imported_artifacts: Memo<StructureCacheKey, Option<ImportedStructureArtifact>>,
    structure_artifacts: Memo<StructureCacheKey, Option<StructureArtifact>>,
}

#[derive(Debug, Clone)]
struct ResolvedGlobalType {
    scope_id: ScopeId,
    name: SymbolId,
    ty: TypeId,
}

struct TempFileGuard {
    path: String,
}

struct LinkTarget {
    triple: String,
    is_windows: bool,
    is_darwin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StructureCacheKey {
    input_file: PathBuf,
    overrides: Vec<(PathBuf, u64)>,
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl AnalysisArtifact {
    pub fn completion_items(
        &self,
        target_path: &Path,
        offset: usize,
    ) -> Vec<AnalysisCompletionItem> {
        self.completion_model.completion_items(target_path, offset)
    }

    pub fn signature_help(
        &self,
        target_path: &Path,
        offset: usize,
    ) -> Option<AnalysisSignatureHelp> {
        self.signature_model
            .signature_help(&self.session, target_path, offset)
    }

    pub fn flow_owners(&self) -> Vec<AnalysisFlowOwner> {
        self.flow_model.public_owners()
    }

    pub fn unused_private_items(&self) -> Vec<AnalysisUnusedItem> {
        self.unused_items.clone()
    }

    pub fn unused_bindings(&self) -> Vec<AnalysisUnusedBinding> {
        self.unused_bindings.clone()
    }

    pub fn dead_stores(&self) -> Vec<AnalysisDeadStore> {
        self.dead_stores.clone()
    }
}

impl ParsedModuleArtifact {
    pub fn requires_body_completion(&self, target_path: &Path, offset: usize) -> bool {
        completion::parsed_requires_body_completion(&self.modules, target_path, offset)
    }
}

impl AnalysisSurfaceArtifact {
    pub fn requires_body_completion(&self, target_path: &Path, offset: usize) -> bool {
        self.completion_model
            .requires_body_completion(target_path, offset)
    }

    pub fn completion_items(
        &self,
        target_path: &Path,
        offset: usize,
    ) -> Vec<AnalysisCompletionItem> {
        self.completion_model
            .surface_completion_items(target_path, offset)
    }
}

impl StructureArtifact {
    pub fn completion_items(
        &self,
        target_path: &Path,
        offset: usize,
    ) -> Vec<AnalysisCompletionItem> {
        self.completion_model.completion_items(target_path, offset)
    }
}

impl ImportedStructureArtifact {
    pub fn completion_items(
        &self,
        target_path: &Path,
        offset: usize,
    ) -> Vec<AnalysisCompletionItem> {
        self.completion_model.completion_items(target_path, offset)
    }
}

#[cfg(test)]
mod tests;
