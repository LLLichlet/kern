mod analysis;
mod completion;
mod flow;
mod link;
mod pipeline;
mod signature;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    pub cache_stats: CompileCacheStats,
    pub lower_cache_stats: Option<kernc_lower::LowerCacheStats>,
    pub mast_workload: Option<kernc_mast::MastWorkloadStats>,
    pub ir_instruction_stats: Option<kernc_codegen::IrInstructionStats>,
    pub ir_hot_functions: Vec<kernc_codegen::IrFunctionStats>,
    pub codegen_alloca_stats: kernc_codegen::CodegenAllocaStats,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompileCacheStats {
    pub compile_structure_hits: usize,
    pub compile_structure_misses: usize,
    pub structure_hits: usize,
    pub structure_misses: usize,
    pub imported_hits: usize,
    pub imported_misses: usize,
    pub collected_hits: usize,
    pub collected_misses: usize,
    pub fresh_frontend_parses: usize,
}

impl CompileCacheStats {
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }

    pub fn absorb(&mut self, other: Self) {
        self.compile_structure_hits += other.compile_structure_hits;
        self.compile_structure_misses += other.compile_structure_misses;
        self.structure_hits += other.structure_hits;
        self.structure_misses += other.structure_misses;
        self.imported_hits += other.imported_hits;
        self.imported_misses += other.imported_misses;
        self.collected_hits += other.collected_hits;
        self.collected_misses += other.collected_misses;
        self.fresh_frontend_parses += other.fresh_frontend_parses;
    }
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IncrementalDriverKey {
    target_triple: String,
    root_module_name: Option<String>,
    collect_docs: bool,
    custom_defines: Vec<(String, String)>,
    module_aliases: Vec<(String, String)>,
    module_interface_aliases: Vec<(String, String)>,
}

impl IncrementalDriverKey {
    pub fn from_options(options: &CompileOptions) -> Self {
        let mut custom_defines = options
            .custom_defines
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        custom_defines.sort();

        let mut module_aliases = options
            .module_aliases
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        module_aliases.sort();

        let mut module_interface_aliases = options
            .module_interface_aliases
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        module_interface_aliases.sort();

        Self {
            target_triple: options.target.triple.to_string(),
            root_module_name: options.root_module_name.clone(),
            collect_docs: options.metadata_output.is_some(),
            custom_defines,
            module_aliases,
            module_interface_aliases,
        }
    }
}

pub struct CompilerDriver {
    pub options: CompileOptions,
    frontend: FrontendDatabase,
    compile_structure_artifacts: Memo<StructureCacheKey, Option<CompileStructureArtifact>>,
    collected_artifacts: Memo<StructureCacheKey, Option<CollectedStructureArtifact>>,
    imported_artifacts: Memo<StructureCacheKey, Option<ImportedStructureArtifact>>,
    structure_artifacts: Memo<StructureCacheKey, Option<StructureArtifact>>,
    cache_counters: Arc<CacheCounters>,
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

#[derive(Default)]
struct CacheCounters {
    compile_structure_hits: AtomicUsize,
    compile_structure_misses: AtomicUsize,
    structure_hits: AtomicUsize,
    structure_misses: AtomicUsize,
    imported_hits: AtomicUsize,
    imported_misses: AtomicUsize,
    collected_hits: AtomicUsize,
    collected_misses: AtomicUsize,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct CacheCounterSnapshot {
    pub(super) compile_structure_hits: usize,
    pub(super) compile_structure_misses: usize,
    pub(super) structure_hits: usize,
    pub(super) structure_misses: usize,
    pub(super) imported_hits: usize,
    pub(super) imported_misses: usize,
    pub(super) collected_hits: usize,
    pub(super) collected_misses: usize,
    pub(super) frontend_uncached_parses: usize,
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

impl CompilerDriver {
    pub fn incremental_key(&self) -> IncrementalDriverKey {
        IncrementalDriverKey::from_options(&self.options)
    }

    pub fn share_incremental_state(&self, options: CompileOptions) -> Option<Self> {
        (self.incremental_key() == IncrementalDriverKey::from_options(&options)).then(|| Self {
            options,
            frontend: self.frontend.clone(),
            compile_structure_artifacts: self.compile_structure_artifacts.clone(),
            collected_artifacts: self.collected_artifacts.clone(),
            imported_artifacts: self.imported_artifacts.clone(),
            structure_artifacts: self.structure_artifacts.clone(),
            cache_counters: Arc::clone(&self.cache_counters),
        })
    }

    pub(super) fn cache_counter_snapshot(&self) -> CacheCounterSnapshot {
        CacheCounterSnapshot {
            compile_structure_hits: self
                .cache_counters
                .compile_structure_hits
                .load(Ordering::Relaxed),
            compile_structure_misses: self
                .cache_counters
                .compile_structure_misses
                .load(Ordering::Relaxed),
            structure_hits: self.cache_counters.structure_hits.load(Ordering::Relaxed),
            structure_misses: self.cache_counters.structure_misses.load(Ordering::Relaxed),
            imported_hits: self.cache_counters.imported_hits.load(Ordering::Relaxed),
            imported_misses: self.cache_counters.imported_misses.load(Ordering::Relaxed),
            collected_hits: self.cache_counters.collected_hits.load(Ordering::Relaxed),
            collected_misses: self.cache_counters.collected_misses.load(Ordering::Relaxed),
            frontend_uncached_parses: self.frontend.uncached_parse_count(),
        }
    }

    pub(super) fn cache_stats_since(&self, before: CacheCounterSnapshot) -> CompileCacheStats {
        let after = self.cache_counter_snapshot();
        CompileCacheStats {
            compile_structure_hits: after
                .compile_structure_hits
                .saturating_sub(before.compile_structure_hits),
            compile_structure_misses: after
                .compile_structure_misses
                .saturating_sub(before.compile_structure_misses),
            structure_hits: after.structure_hits.saturating_sub(before.structure_hits),
            structure_misses: after
                .structure_misses
                .saturating_sub(before.structure_misses),
            imported_hits: after.imported_hits.saturating_sub(before.imported_hits),
            imported_misses: after.imported_misses.saturating_sub(before.imported_misses),
            collected_hits: after.collected_hits.saturating_sub(before.collected_hits),
            collected_misses: after
                .collected_misses
                .saturating_sub(before.collected_misses),
            fresh_frontend_parses: after
                .frontend_uncached_parses
                .saturating_sub(before.frontend_uncached_parses),
        }
    }

    pub(super) fn record_structure_cache_hit(&self) {
        self.cache_counters
            .structure_hits
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn record_structure_cache_miss(&self) {
        self.cache_counters
            .structure_misses
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn record_imported_cache_hit(&self) {
        self.cache_counters
            .imported_hits
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn record_imported_cache_miss(&self) {
        self.cache_counters
            .imported_misses
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn record_collected_cache_hit(&self) {
        self.cache_counters
            .collected_hits
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn record_collected_cache_miss(&self) {
        self.cache_counters
            .collected_misses
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn record_compile_structure_cache_hit(&self) {
        self.cache_counters
            .compile_structure_hits
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn record_compile_structure_cache_miss(&self) {
        self.cache_counters
            .compile_structure_misses
            .fetch_add(1, Ordering::Relaxed);
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
