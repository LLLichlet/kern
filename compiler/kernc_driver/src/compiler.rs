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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
pub struct CancellationToken {
    canceled: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Canceled;

impl CancellationToken {
    pub fn new() -> Self {
        Self {
            canceled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn from_shared(canceled: Arc<AtomicBool>) -> Self {
        Self { canceled }
    }

    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::SeqCst);
    }

    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::SeqCst)
    }

    pub fn check(&self) -> Result<(), Canceled> {
        if self.is_canceled() {
            Err(Canceled)
        } else {
            Ok(())
        }
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisCall {
    pub kind: AnalysisCallKind,
    pub call_span: kernc_utils::Span,
    pub callee_span: kernc_utils::Span,
    pub callee_definition_span: kernc_utils::Span,
    pub caller_definition_span: kernc_utils::Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisCallKind {
    Direct,
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
