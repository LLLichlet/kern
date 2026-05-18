mod analysis;
mod codegen_units;
mod completion;
mod flow;
mod link;
mod pipeline;
mod signature;
mod type_hints;

pub use self::codegen_units::{CodegenImportPlanReport, CodegenPlanFallback, CodegenPlanReport};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kernc_ast as ast;
use kernc_db::Memo;
pub use kernc_flow::{
    AnalysisFlowBinding, AnalysisFlowBindingId, AnalysisFlowBindingKind,
    AnalysisFlowBindingSummary, AnalysisFlowCfg, AnalysisFlowCfgEdge, AnalysisFlowCfgEdgeKind,
    AnalysisFlowCfgNode, AnalysisFlowCfgNodeKind, AnalysisFlowDefUse, AnalysisFlowDefinitionFacts,
    AnalysisFlowDefinitionKind, AnalysisFlowDefinitionRef, AnalysisFlowLiveness,
    AnalysisFlowNodeEffects, AnalysisFlowNodeFacts, AnalysisFlowNodeId, AnalysisFlowNodeTransfer,
    AnalysisFlowOwner, AnalysisFlowOwnerKind, AnalysisFlowReaching, AnalysisFlowRegion,
    AnalysisFlowRegionKind, AnalysisFlowResolvedUse, AnalysisFlowResolvedUseKind,
    AnalysisFlowSingleSourceUse, AnalysisFlowSummary, AnalysisFlowUseDef,
};
use kernc_sema::SemaStructureSnapshot;
use kernc_sema::def::{Def, DefId};
use kernc_sema::scope::{ScopeId, SymbolInfo, SymbolKind};
use kernc_sema::ty::TypeId;
use kernc_utils::config::CompileOptions;
pub use kernc_utils::{Canceled, CancellationToken};
use kernc_utils::{Session, SymbolId};

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
    pub mir_workload: Option<kernc_mir::MirWorkloadStats>,
    pub codegen_plan: Option<CodegenPlanReport>,
    pub ir_instruction_stats: Option<kernc_codegen::IrInstructionStats>,
    pub ir_cleanup_stats: Option<kernc_codegen::IrCleanupStats>,
    pub remaining_alloca_stats: Option<kernc_codegen::CodegenAllocaStats>,
    pub remaining_alloca_names: Vec<kernc_codegen::AllocaNameStat>,
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

#[derive(Debug, Clone)]
pub struct AnalysisDefinitionLink {
    pub definition_span: kernc_utils::Span,
    pub linked_definition_span: kernc_utils::Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisDocumentLink {
    pub origin_span: kernc_utils::Span,
    pub target_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisCall {
    pub kind: AnalysisCallKind,
    pub call_span: kernc_utils::Span,
    pub callee_span: kernc_utils::Span,
    pub callee_definition_span: Option<kernc_utils::Span>,
    pub caller_definition_span: kernc_utils::Span,
    pub dynamic_dispatch_targets: Vec<kernc_utils::Span>,
    pub indirect_targets: Vec<kernc_utils::Span>,
    pub indirect_target_completeness: AnalysisCallTargetCompleteness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisCallKind {
    Direct,
    DynamicDispatch,
    Indirect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisCallTargetCompleteness {
    Exact,
    Partial,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisTypeHintKind {
    Variable,
    Expression,
    ConstructorPrefix,
}

#[derive(Debug, Clone)]
pub struct AnalysisTypeHint {
    pub span: kernc_utils::Span,
    pub label: String,
    pub kind: AnalysisTypeHintKind,
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
    EnumMember,
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
    pub documentation: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisTraitImplStub {
    pub impl_span: kernc_utils::Span,
    pub method_name: String,
    pub insertion_offset: usize,
    pub insert_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisImportCandidate {
    pub name: String,
    pub path: String,
    pub insertion_offset: usize,
    pub insert_text: String,
    pub definition_span: kernc_utils::Span,
}

#[derive(Debug, Clone, Copy)]
struct ImportInsertionSite {
    module_id: DefId,
    insertion_offset: usize,
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
    pub type_hints: Vec<AnalysisTypeHint>,
    pub definition_links: Vec<AnalysisDefinitionLink>,
    pub semantic_entries: Vec<AnalysisSemanticEntry>,
    pub calls: Vec<AnalysisCall>,
    asts: Vec<(DefId, ast::Module)>,
    resolved_globals: Vec<ResolvedGlobalType>,
    completion_model: completion::CompletionModel,
    signature_model: signature::SignatureModel,
    flow_model: flow::FlowModel,
    unused_items: Vec<AnalysisUnusedItem>,
    unused_bindings: Vec<AnalysisUnusedBinding>,
    dead_stores: Vec<AnalysisDeadStore>,
    trait_impl_stubs: Vec<AnalysisTraitImplStub>,
}

pub struct AnalysisNavigationArtifact {
    pub session: Session,
    pub succeeded: bool,
    pub symbols: Vec<AnalysisSymbol>,
    pub references: Vec<AnalysisReference>,
    pub hovers: Vec<AnalysisHover>,
    pub type_hints: Vec<AnalysisTypeHint>,
    pub definition_links: Vec<AnalysisDefinitionLink>,
    pub semantic_entries: Vec<AnalysisSemanticEntry>,
    pub calls: Vec<AnalysisCall>,
}

pub struct AnalysisSemanticArtifact {
    pub session: Session,
    pub succeeded: bool,
    pub symbols: Vec<AnalysisSymbol>,
    pub references: Vec<AnalysisReference>,
    pub hovers: Vec<AnalysisHover>,
    pub type_hints: Vec<AnalysisTypeHint>,
    pub semantic_entries: Vec<AnalysisSemanticEntry>,
}

pub struct AnalysisSemanticTokenArtifact {
    pub session: Session,
    pub succeeded: bool,
    pub symbols: Vec<AnalysisSymbol>,
    pub references: Vec<AnalysisReference>,
    pub hovers: Vec<AnalysisHover>,
    pub semantic_entries: Vec<AnalysisSemanticEntry>,
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
    file_id: kernc_utils::FileId,
    source_path: PathBuf,
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
    trait_impl_stubs: Vec<AnalysisTraitImplStub>,
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
    runtime_entry: String,
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
            runtime_entry: options.runtime_entry.as_str().to_string(),
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
    clean_collected_reuse_artifacts: Arc<Mutex<HashMap<PathBuf, CollectedStructureArtifact>>>,
    clean_imported_reuse_artifacts: Arc<Mutex<HashMap<PathBuf, ImportedStructureArtifact>>>,
    clean_structure_reuse_artifacts: Arc<Mutex<HashMap<PathBuf, StructureArtifact>>>,
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

struct TempDirGuard {
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
    body_only_collected_reuses: AtomicUsize,
    body_only_imported_reuses: AtomicUsize,
    body_only_structure_reuses: AtomicUsize,
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

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
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

    pub fn trait_impl_stubs(&self) -> Vec<AnalysisTraitImplStub> {
        self.trait_impl_stubs.clone()
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
            clean_collected_reuse_artifacts: Arc::clone(&self.clean_collected_reuse_artifacts),
            clean_imported_reuse_artifacts: Arc::clone(&self.clean_imported_reuse_artifacts),
            clean_structure_reuse_artifacts: Arc::clone(&self.clean_structure_reuse_artifacts),
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

    pub(super) fn record_body_only_collected_reuse(&self) {
        self.cache_counters
            .body_only_collected_reuses
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn record_body_only_imported_reuse(&self) {
        self.cache_counters
            .body_only_imported_reuses
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn record_body_only_structure_reuse(&self) {
        self.cache_counters
            .body_only_structure_reuses
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

    #[cfg(test)]
    pub(crate) fn body_only_collected_reuse_count(&self) -> usize {
        self.cache_counters
            .body_only_collected_reuses
            .load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn body_only_imported_reuse_count(&self) -> usize {
        self.cache_counters
            .body_only_imported_reuses
            .load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn body_only_structure_reuse_count(&self) -> usize {
        self.cache_counters
            .body_only_structure_reuses
            .load(Ordering::Relaxed)
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
    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn trait_impl_stubs(&self) -> &[AnalysisTraitImplStub] {
        &self.trait_impl_stubs
    }

    pub fn completion_items(
        &self,
        target_path: &Path,
        offset: usize,
    ) -> Vec<AnalysisCompletionItem> {
        self.completion_model.completion_items(target_path, offset)
    }

    pub fn document_links(&self, target_path: &Path) -> Vec<AnalysisDocumentLink> {
        let mut links = Vec::new();
        for (module_id, module_ast) in &self.asts {
            let Some(kernc_sema::def::Def::Module(module_def)) =
                self.snapshot.defs.get(module_id.0 as usize)
            else {
                continue;
            };
            let Some(module_path) = self
                .session
                .source_manager
                .get_file_path(module_def.file_id)
            else {
                continue;
            };
            if module_path != target_path {
                continue;
            }
            let import_spans = import_binding_spans(module_ast);

            for decl in &module_ast.decls {
                if !matches!(decl.kind, ast::DeclKind::Mod { decls: None }) {
                    continue;
                }
                let Some(submodule_id) = module_def.submodules.get(&decl.name) else {
                    continue;
                };
                let Some(kernc_sema::def::Def::Module(submodule_def)) =
                    self.snapshot.defs.get(submodule_id.0 as usize)
                else {
                    continue;
                };
                let Some(target_path) = self
                    .session
                    .source_manager
                    .get_file_path(submodule_def.file_id)
                else {
                    continue;
                };
                links.push(AnalysisDocumentLink {
                    origin_span: decl.name_span,
                    target_path: target_path.clone(),
                });
            }
            for (_name, info) in self.snapshot.scopes.symbols_in_scope(module_def.scope_id) {
                if !import_spans.contains(&info.span) {
                    continue;
                }
                if info.kind != kernc_sema::scope::SymbolKind::Module {
                    continue;
                }
                let Some(def_id) = info.def_id else {
                    continue;
                };
                let Some(target_module_id) = document_link_target_module(&self.snapshot, def_id)
                else {
                    continue;
                };
                if target_module_id == *module_id {
                    continue;
                }
                let Some(kernc_sema::def::Def::Module(target_module)) =
                    self.snapshot.defs.get(target_module_id.0 as usize)
                else {
                    continue;
                };
                let Some(target_path) = self
                    .session
                    .source_manager
                    .get_file_path(target_module.file_id)
                else {
                    continue;
                };
                links.push(AnalysisDocumentLink {
                    origin_span: info.span,
                    target_path: target_path.clone(),
                });
            }
        }
        links.sort_by_key(|link| (link.origin_span.file.0, link.origin_span.start));
        links.dedup();
        links
    }

    pub fn import_candidates_for_unresolved_name(
        &self,
        target_path: &Path,
        unresolved_span: kernc_utils::Span,
        name: &str,
        type_only: bool,
    ) -> Vec<AnalysisImportCandidate> {
        import_candidates_for_unresolved_name_from_structure(
            &self.session,
            &self.asts,
            &self.snapshot,
            target_path,
            unresolved_span,
            name,
            type_only,
        )
    }
}

impl ImportedStructureArtifact {
    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn import_candidates_for_unresolved_name(
        &self,
        target_path: &Path,
        unresolved_span: kernc_utils::Span,
        name: &str,
        type_only: bool,
    ) -> Vec<AnalysisImportCandidate> {
        import_candidates_for_unresolved_name_from_structure(
            &self.session,
            &self.asts,
            &self.snapshot,
            target_path,
            unresolved_span,
            name,
            type_only,
        )
    }

    pub fn completion_items(
        &self,
        target_path: &Path,
        offset: usize,
    ) -> Vec<AnalysisCompletionItem> {
        self.completion_model.completion_items(target_path, offset)
    }
}

fn import_candidates_for_unresolved_name_from_structure(
    session: &Session,
    asts: &[(DefId, ast::Module)],
    snapshot: &SemaStructureSnapshot,
    target_path: &Path,
    unresolved_span: kernc_utils::Span,
    name: &str,
    type_only: bool,
) -> Vec<AnalysisImportCandidate> {
    let Some(site) = import_insertion_site_for_path_and_span(
        session,
        asts,
        snapshot,
        target_path,
        unresolved_span,
    ) else {
        return Vec::new();
    };
    let Some(target_module) = structure_module_def(snapshot, site.module_id) else {
        return Vec::new();
    };
    let Some(name_symbol) = lookup_structure_symbol(session, snapshot, name) else {
        return Vec::new();
    };
    if snapshot
        .scopes
        .resolve_from(target_module.scope_id, name_symbol)
        .is_some()
    {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    for def in snapshot.defs.iter() {
        let Def::Module(owner_module) = def else {
            continue;
        };
        if owner_module.id == site.module_id {
            continue;
        }
        let Some(info) = snapshot
            .scopes
            .resolve_in(owner_module.scope_id, name_symbol)
        else {
            continue;
        };
        if !import_candidate_kind_matches(info.kind, type_only) {
            continue;
        }
        if !symbol_visible_from_module(snapshot, info, owner_module.id, site.module_id) {
            continue;
        }
        let Some(path) =
            import_path_between_modules(session, snapshot, site.module_id, owner_module.id, name)
        else {
            continue;
        };
        candidates.push(AnalysisImportCandidate {
            name: name.to_string(),
            path: path.clone(),
            insertion_offset: site.insertion_offset,
            insert_text: format!("use {};\n", path),
            definition_span: info.span,
        });
    }

    candidates.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.definition_span.file.cmp(&right.definition_span.file))
            .then_with(|| left.definition_span.start.cmp(&right.definition_span.start))
    });
    candidates.dedup_by(|left, right| left.path == right.path);
    candidates
}

fn import_insertion_site_for_path_and_span(
    session: &Session,
    asts: &[(DefId, ast::Module)],
    snapshot: &SemaStructureSnapshot,
    target_path: &Path,
    span: kernc_utils::Span,
) -> Option<ImportInsertionSite> {
    asts.iter()
        .filter_map(|(module_id, module_ast)| {
            let module = structure_module_def(snapshot, *module_id)?;
            let path = session.source_manager.get_file_path(module.file_id)?;
            if path != target_path {
                return None;
            }
            let module_span = module_span(module_ast, module.file_id);
            if !span_within(module_span, span) {
                return None;
            }
            let file = session.source_manager.get_file(module.file_id)?;
            let site = find_import_insertion_site_in_decls(
                snapshot,
                file,
                *module_id,
                &module_ast.decls,
                span,
                module_span.start,
            )?;
            Some((site, module_span))
        })
        .min_by_key(|(_, module_span)| module_span.end.saturating_sub(module_span.start))
        .map(|(site, _)| site)
        .or_else(|| {
            snapshot.defs.iter().find_map(|def| {
                let Def::Module(module) = def else {
                    return None;
                };
                let path = session.source_manager.get_file_path(module.file_id)?;
                (path == target_path).then_some(ImportInsertionSite {
                    module_id: module.id,
                    insertion_offset: 0,
                })
            })
        })
}

fn lookup_structure_symbol(
    session: &Session,
    snapshot: &SemaStructureSnapshot,
    name: &str,
) -> Option<SymbolId> {
    snapshot
        .scopes
        .all_symbols()
        .map(|(symbol, _)| symbol)
        .find(|symbol| session.resolve(*symbol) == name)
}

fn find_import_insertion_site_in_decls(
    snapshot: &SemaStructureSnapshot,
    file: &kernc_utils::SourceFile,
    module_id: DefId,
    decls: &[ast::Decl],
    span: kernc_utils::Span,
    module_start: usize,
) -> Option<ImportInsertionSite> {
    for decl in decls {
        let ast::DeclKind::Mod {
            decls: Some(child_decls),
        } = &decl.kind
        else {
            continue;
        };
        if !span_within(decl.span, span) {
            continue;
        }
        let module = structure_module_def(snapshot, module_id)?;
        let child_module_id = module.submodules.get(&decl.name).copied()?;
        let child_start = inline_module_body_start(file, decl)?;
        return find_import_insertion_site_in_decls(
            snapshot,
            file,
            child_module_id,
            child_decls,
            span,
            child_start,
        );
    }

    Some(ImportInsertionSite {
        module_id,
        insertion_offset: import_insertion_offset_in_decls(file, decls, module_start),
    })
}

fn import_insertion_offset_in_decls(
    file: &kernc_utils::SourceFile,
    decls: &[ast::Decl],
    module_start: usize,
) -> usize {
    let mut offset = module_start;
    for decl in decls {
        if matches!(decl.kind, ast::DeclKind::Use { .. }) {
            offset = decl.span.end;
            if file.src.as_bytes().get(offset) == Some(&b'\n') {
                offset += 1;
            }
        }
    }
    offset
}

fn inline_module_body_start(file: &kernc_utils::SourceFile, decl: &ast::Decl) -> Option<usize> {
    let search_start = decl.name_span.end.min(file.src.len());
    let search_end = decl.span.end.min(file.src.len());
    let relative = file.src.get(search_start..search_end)?.find('{')?;
    let mut offset = search_start + relative + 1;
    if file.src.as_bytes().get(offset) == Some(&b'\n') {
        offset += 1;
    }
    Some(offset)
}

fn import_candidate_kind_matches(kind: SymbolKind, type_only: bool) -> bool {
    if type_only {
        return matches!(
            kind,
            SymbolKind::Struct
                | SymbolKind::Union
                | SymbolKind::Enum
                | SymbolKind::Trait
                | SymbolKind::TypeAlias
        );
    }
    matches!(
        kind,
        SymbolKind::Function
            | SymbolKind::Const
            | SymbolKind::Static
            | SymbolKind::Struct
            | SymbolKind::Union
            | SymbolKind::Enum
            | SymbolKind::Trait
            | SymbolKind::TypeAlias
            | SymbolKind::Module
    )
}

fn import_path_between_modules(
    session: &Session,
    snapshot: &SemaStructureSnapshot,
    from_module_id: DefId,
    owner_module_id: DefId,
    name: &str,
) -> Option<String> {
    let from_root = structure_module_root(snapshot, from_module_id);
    let owner_root = structure_module_root(snapshot, owner_module_id);
    if from_root == owner_root {
        let mut components = structure_module_path_below_root(session, snapshot, owner_module_id)?;
        components.push(name.to_string());
        return Some(format!("/{}", components.join(".")));
    }

    let mut components = vec![
        session
            .resolve(structure_module_def(snapshot, owner_root)?.name)
            .to_string(),
    ];
    components.extend(structure_module_path_below_root(
        session,
        snapshot,
        owner_module_id,
    )?);
    components.push(name.to_string());
    Some(components.join("."))
}

fn structure_module_path_below_root(
    session: &Session,
    snapshot: &SemaStructureSnapshot,
    module_id: DefId,
) -> Option<Vec<String>> {
    let root = structure_module_root(snapshot, module_id);
    let mut ids = Vec::new();
    let mut current = Some(module_id);
    while let Some(id) = current {
        ids.push(id);
        if id == root {
            break;
        }
        current = structure_module_def(snapshot, id)?.parent;
    }
    ids.reverse();
    if !ids.is_empty() {
        ids.remove(0);
    }
    Some(
        ids.into_iter()
            .filter_map(|id| {
                structure_module_def(snapshot, id)
                    .map(|module| session.resolve(module.name).to_string())
            })
            .collect(),
    )
}

fn structure_module_root(snapshot: &SemaStructureSnapshot, module_id: DefId) -> DefId {
    let mut current = module_id;
    while let Some(parent) =
        structure_module_def(snapshot, current).and_then(|module| module.parent)
    {
        current = parent;
    }
    current
}

fn structure_module_def(
    snapshot: &SemaStructureSnapshot,
    module_id: DefId,
) -> Option<&kernc_sema::def::ModuleDef> {
    match snapshot.defs.get(module_id.0 as usize) {
        Some(Def::Module(module)) => Some(module),
        _ => None,
    }
}

fn symbol_visible_from_module(
    snapshot: &SemaStructureSnapshot,
    info: &SymbolInfo,
    owner_module: DefId,
    current_module: DefId,
) -> bool {
    match info.vis {
        kernc_ast::Visibility::Public => true,
        kernc_ast::Visibility::Private => current_module == owner_module,
        kernc_ast::Visibility::Super => {
            let Some(parent_module) =
                structure_module_def(snapshot, owner_module).and_then(|module| module.parent)
            else {
                return false;
            };
            module_is_same_or_descendant_of(snapshot, current_module, parent_module)
        }
        kernc_ast::Visibility::Package => {
            structure_module_root(snapshot, current_module)
                == structure_module_root(snapshot, owner_module)
        }
    }
}

fn module_is_same_or_descendant_of(
    snapshot: &SemaStructureSnapshot,
    module_id: DefId,
    ancestor_module_id: DefId,
) -> bool {
    let mut current = Some(module_id);
    while let Some(id) = current {
        if id == ancestor_module_id {
            return true;
        }
        current = structure_module_def(snapshot, id).and_then(|module| module.parent);
    }
    false
}

fn module_span(module: &ast::Module, file_id: kernc_utils::FileId) -> kernc_utils::Span {
    let start = module
        .decls
        .first()
        .map(|decl| decl.span.start)
        .unwrap_or(0);
    let end = module
        .decls
        .last()
        .map(|decl| decl.span.end)
        .unwrap_or(start);
    kernc_utils::Span {
        file: file_id,
        start,
        end,
    }
}

fn span_within(outer: kernc_utils::Span, inner: kernc_utils::Span) -> bool {
    outer.file == inner.file && outer.start <= inner.start && inner.end <= outer.end
}

fn import_binding_spans(module: &ast::Module) -> std::collections::BTreeSet<kernc_utils::Span> {
    let mut spans = std::collections::BTreeSet::new();
    for decl in &module.decls {
        let ast::DeclKind::Use { target, .. } = &decl.kind else {
            continue;
        };
        collect_import_binding_spans(target, decl.name_span, &mut spans);
    }
    spans
}

fn collect_import_binding_spans(
    target: &ast::UseTarget,
    module_binding_span: kernc_utils::Span,
    spans: &mut std::collections::BTreeSet<kernc_utils::Span>,
) {
    match target {
        ast::UseTarget::Module(_) => {
            spans.insert(module_binding_span);
        }
        ast::UseTarget::Tree(items) => {
            for item in items {
                collect_import_tree_binding_spans(item, spans);
            }
        }
    }
}

fn collect_import_tree_binding_spans(
    tree: &ast::UseTree,
    spans: &mut std::collections::BTreeSet<kernc_utils::Span>,
) {
    match tree {
        ast::UseTree::SelfModule { binding_span, .. } => {
            spans.insert(*binding_span);
        }
        ast::UseTree::Path {
            nested,
            binding_span,
            ..
        } => {
            spans.insert(*binding_span);
            if let Some(nested) = nested {
                for child in nested {
                    collect_import_tree_binding_spans(child, spans);
                }
            }
        }
    }
}

fn document_link_target_module(snapshot: &SemaStructureSnapshot, def_id: DefId) -> Option<DefId> {
    match snapshot.defs.get(def_id.0 as usize)? {
        kernc_sema::def::Def::Module(_) => Some(def_id),
        _ => document_link_def_parent_module(snapshot, def_id),
    }
}

fn document_link_def_parent_module(
    snapshot: &SemaStructureSnapshot,
    def_id: DefId,
) -> Option<DefId> {
    let parent = match snapshot.defs.get(def_id.0 as usize)? {
        kernc_sema::def::Def::Module(module) => module.parent,
        kernc_sema::def::Def::Function(function) => match function.parent {
            Some(parent_id) => match snapshot.defs.get(parent_id.0 as usize) {
                Some(kernc_sema::def::Def::Module(_)) => Some(parent_id),
                Some(kernc_sema::def::Def::Impl(impl_def)) => impl_def.parent_module,
                _ => None,
            },
            None => None,
        },
        kernc_sema::def::Def::Struct(def) => def.parent_module,
        kernc_sema::def::Def::Union(def) => def.parent_module,
        kernc_sema::def::Def::Impl(def) => def.parent_module,
        kernc_sema::def::Def::Global(global) => match global.parent {
            Some(parent_id) => match snapshot.defs.get(parent_id.0 as usize) {
                Some(kernc_sema::def::Def::Module(_)) => Some(parent_id),
                Some(kernc_sema::def::Def::Impl(impl_def)) => impl_def.parent_module,
                _ => None,
            },
            None => None,
        },
        kernc_sema::def::Def::AssociatedType(def) => {
            if let Some(parent_impl) = def.parent_impl {
                match snapshot.defs.get(parent_impl.0 as usize) {
                    Some(kernc_sema::def::Def::Impl(impl_def)) => impl_def.parent_module,
                    _ => None,
                }
            } else {
                def.parent_trait
                    .and_then(|trait_id| document_link_def_parent_module(snapshot, trait_id))
            }
        }
        kernc_sema::def::Def::Enum(_)
        | kernc_sema::def::Def::Trait(_)
        | kernc_sema::def::Def::TypeAlias(_) => None,
    };
    parent.or_else(|| {
        snapshot.defs.iter().find_map(|def| match def {
            kernc_sema::def::Def::Module(module) if module.items.contains(&def_id) => {
                Some(module.id)
            }
            _ => None,
        })
    })
}

#[cfg(test)]
mod tests;
