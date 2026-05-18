mod cache;
mod code_actions;
mod completion;
mod diagnostics;
mod documents;
mod formatting;
pub(super) mod ide;
mod navigation;
mod queries;
mod semantic;
mod structure;
#[cfg(test)]
mod tests;
mod text;

use self::cache::{
    AnalysisCacheKey, DirtyDocumentsSnapshot, LexicalCacheKey, SemanticTokensCacheKey,
    hash_source_text,
};
use self::code_actions::{
    fallback_trait_impl_stub_code_action, ide_ranges_overlap, import_insertion_code_actions,
    import_insertion_code_actions_for_imported_structure, lightweight_quick_fix_for_diagnostic,
    quick_fix_for_diagnostic, ranges_overlap, workspace_edit_key,
};
use self::completion::{completion_sort_key, keyword_completion_item};
pub use self::diagnostics::cleared_uris;
use self::diagnostics::{
    convert_diagnostic_for_document, diagnostics_from_session, preserve_target_diagnostics,
};
use self::ide::{
    IdeCallHierarchyIncomingCall, IdeCallHierarchyItem, IdeCallHierarchyOutgoingCall,
    IdeCodeAction, IdeCodeLens, IdeCompletionItem, IdeDiagnostic, IdeDocumentHighlight,
    IdeDocumentLink, IdeDocumentSymbol, IdeFoldingRange, IdeFoldingRangeKind, IdeHover,
    IdeInlayHint, IdeLocation, IdePrepareRenameResult, IdeSelectionRange, IdeSemanticTokens,
    IdeSignatureHelp, IdeTextEdit, IdeWorkspaceEdit, IdeWorkspaceSymbol,
};
use self::navigation::{
    KnownReferenceLocationQuery, ReferenceLocationQuery, analysis_completion_to_ide_item,
    analysis_signature_help_to_ide_help, analysis_symbol_to_document_symbol,
    analysis_symbol_to_workspace_symbols_cancelable, analysis_type_hint_to_ide_hint,
    build_rename_changes, find_call_hierarchy_incoming_calls, find_call_hierarchy_item,
    find_call_hierarchy_outgoing_calls, find_definition_location, find_document_highlights,
    find_hover, find_implementation_locations, find_reference_locations_cancelable,
    find_reference_locations_for_definition_cancelable, find_rename_target,
    find_type_definition_location, navigation_definition_span_for_position,
};
use self::text::{
    LexicalIndex, apply_content_change, byte_offset_to_position, completion_context,
    completion_is_binding_name_context, completion_is_member_access,
    completion_member_access_has_receiver, completion_prefix, fallback_keyword_completion_labels,
    file_path_to_uri, has_following_call_paren, is_valid_identifier, keyword_completion_labels,
    match_position_in_file, normalize_path, position_to_byte_offset, span_contains_offset,
    span_to_range, trim_line_ending, uri_to_analysis_path,
};
pub(crate) use self::text::{single_server_diagnostic, uri_to_file_path};
use crate::defaults::default_analysis_compile_options;
use crate::protocol::{CodeActionResolveData, CompletionResolveData, Position, Range};
use crate::server::DiagnosticsAnalysisMode;
use craft::error::Error as CraftError;
use craft::project::{AnalysisProject, ResolvedAnalysis, resolve_project_manifest_path};
use kernc_driver::{
    AnalysisArtifact, AnalysisNavigationArtifact, AnalysisReport, AnalysisSemanticArtifact,
    AnalysisSurfaceArtifact, CompilerDriver, ImportedStructureArtifact, IncrementalDriverKey,
    ParsedModuleArtifact, SourceOverrides, StructureArtifact,
};
use kernc_utils::DiagnosticCode;
use kernc_utils::config::{
    CompileOptions, apply_configured_library_aliases, inject_driver_condition_defines,
};
use kernc_utils::{Session, SourceFile, Span};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisSettings {
    pub compile_options: CompileOptions,
}

impl Default for AnalysisSettings {
    fn default() -> Self {
        Self {
            compile_options: default_analysis_compile_options(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct OpenDocument {
    pub path: PathBuf,
    pub version: i64,
    pub text: String,
    pub is_dirty: bool,
    pub text_hash: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdePosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdeRange {
    pub start: IdePosition,
    pub end: IdePosition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdeTextDocumentChange {
    pub range: Option<IdeRange>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdeOpenDocument {
    pub uri: String,
    pub version: i64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdeChangeDocument {
    pub uri: String,
    pub version: i64,
    pub changes: Vec<IdeTextDocumentChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdeCloseDocument {
    pub uri: String,
}

pub trait IntoIdeOpenDocument {
    fn into_ide_open_document(self) -> IdeOpenDocument;
}

pub trait IntoIdeChangeDocument {
    fn into_ide_change_document(self) -> IdeChangeDocument;
}

pub trait IntoIdeCloseDocument {
    fn into_ide_close_document(self) -> IdeCloseDocument;
}

pub trait IntoIdePosition {
    fn into_ide_position(self) -> IdePosition;
}

pub trait IntoIdeRange {
    fn into_ide_range(self) -> IdeRange;
}

impl IntoIdeOpenDocument for IdeOpenDocument {
    fn into_ide_open_document(self) -> IdeOpenDocument {
        self
    }
}

impl IntoIdeChangeDocument for IdeChangeDocument {
    fn into_ide_change_document(self) -> IdeChangeDocument {
        self
    }
}

impl IntoIdeCloseDocument for IdeCloseDocument {
    fn into_ide_close_document(self) -> IdeCloseDocument {
        self
    }
}

impl IntoIdePosition for IdePosition {
    fn into_ide_position(self) -> IdePosition {
        self
    }
}

impl IntoIdeRange for IdeRange {
    fn into_ide_range(self) -> IdeRange {
        self
    }
}

impl From<IdePosition> for Position {
    fn from(value: IdePosition) -> Self {
        Self {
            line: value.line,
            character: value.character,
        }
    }
}

impl From<Position> for IdePosition {
    fn from(value: Position) -> Self {
        Self {
            line: value.line,
            character: value.character,
        }
    }
}

impl PartialEq<Position> for IdePosition {
    fn eq(&self, other: &Position) -> bool {
        self.line == other.line && self.character == other.character
    }
}

impl PartialEq<IdePosition> for Position {
    fn eq(&self, other: &IdePosition) -> bool {
        self.line == other.line && self.character == other.character
    }
}

impl From<IdeRange> for Range {
    fn from(value: IdeRange) -> Self {
        Self {
            start: value.start.into(),
            end: value.end.into(),
        }
    }
}

impl From<Range> for IdeRange {
    fn from(value: Range) -> Self {
        Self {
            start: value.start.into(),
            end: value.end.into(),
        }
    }
}

impl PartialEq<Range> for IdeRange {
    fn eq(&self, other: &Range) -> bool {
        self.start == other.start && self.end == other.end
    }
}

impl PartialEq<IdeRange> for Range {
    fn eq(&self, other: &IdeRange) -> bool {
        self.start == other.start && self.end == other.end
    }
}

#[derive(Debug, Clone)]
pub struct DiagnosticBundle {
    pub uri: String,
    pub diagnostics: Vec<IdeDiagnostic>,
}

pub struct AnalysisOutcome {
    pub bundles: Vec<DiagnosticBundle>,
}

pub struct WorkspaceIndexRefresh {
    pub targets: Vec<(String, DiagnosticsAnalysisMode)>,
    pub indexed_targets: usize,
    pub failed_targets: usize,
    pub generation: u64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AnalysisTrace {
    pub(crate) snapshot_generation: Option<u64>,
    cache_events: Vec<AnalysisCacheTraceEvent>,
    dirty_fallbacks: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AnalysisCacheTraceKind {
    ProjectResolution,
    Driver,
    ParseArtifact,
    SurfaceArtifact,
    StructureArtifact,
    SemanticArtifact,
    NavigationArtifact,
    SemanticClassificationArtifact,
    WorkspaceSymbolIndex,
    SemanticTokens,
    LexicalIndex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AnalysisCacheTraceOutcome {
    Hit,
    Miss,
    Store,
}

#[derive(Debug, Clone, Copy)]
struct AnalysisCacheTraceEvent {
    kind: AnalysisCacheTraceKind,
    outcome: AnalysisCacheTraceOutcome,
}

impl AnalysisTrace {
    pub(crate) fn cache_summary(&self) -> String {
        if self.cache_events.is_empty() && self.dirty_fallbacks.is_empty() {
            return "none".to_string();
        }

        let mut counts = BTreeMap::new();
        for event in &self.cache_events {
            *counts.entry((event.kind, event.outcome)).or_insert(0usize) += 1;
        }

        let mut parts = Vec::new();
        let kinds = [
            AnalysisCacheTraceKind::ProjectResolution,
            AnalysisCacheTraceKind::Driver,
            AnalysisCacheTraceKind::ParseArtifact,
            AnalysisCacheTraceKind::SurfaceArtifact,
            AnalysisCacheTraceKind::StructureArtifact,
            AnalysisCacheTraceKind::SemanticArtifact,
            AnalysisCacheTraceKind::NavigationArtifact,
            AnalysisCacheTraceKind::SemanticClassificationArtifact,
            AnalysisCacheTraceKind::WorkspaceSymbolIndex,
            AnalysisCacheTraceKind::SemanticTokens,
            AnalysisCacheTraceKind::LexicalIndex,
        ];
        for kind in kinds {
            let hit = counts
                .get(&(kind, AnalysisCacheTraceOutcome::Hit))
                .copied()
                .unwrap_or(0);
            let miss = counts
                .get(&(kind, AnalysisCacheTraceOutcome::Miss))
                .copied()
                .unwrap_or(0);
            let store = counts
                .get(&(kind, AnalysisCacheTraceOutcome::Store))
                .copied()
                .unwrap_or(0);
            if hit > 0 || miss > 0 || store > 0 {
                parts.push(format!(
                    "{}:hit={},miss={},store={}",
                    kind.as_str(),
                    hit,
                    miss,
                    store
                ));
            }
        }

        if !self.dirty_fallbacks.is_empty() {
            parts.push(format!("dirty-fallback={}", self.dirty_fallbacks.join(",")));
        }

        parts.join(";")
    }
}

impl AnalysisCacheTraceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::ProjectResolution => "project",
            Self::Driver => "driver",
            Self::ParseArtifact => "parse",
            Self::SurfaceArtifact => "surface",
            Self::StructureArtifact => "structure",
            Self::SemanticArtifact => "semantic",
            Self::NavigationArtifact => "navigation",
            Self::SemanticClassificationArtifact => "semantic-classification",
            Self::WorkspaceSymbolIndex => "workspace-symbol-index",
            Self::SemanticTokens => "semantic-tokens",
            Self::LexicalIndex => "lexical",
        }
    }
}

struct SurfaceSymbolIndex {
    document_symbols_by_path: BTreeMap<PathBuf, Arc<Vec<IdeDocumentSymbol>>>,
    workspace_symbols: Arc<Vec<IdeWorkspaceSymbol>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceIndexTarget {
    input_file: PathBuf,
    manifest_path: Option<PathBuf>,
    workspace_root: Option<PathBuf>,
    package_root: Option<PathBuf>,
    package_name: Option<String>,
    target_kind: Option<String>,
    target_name: Option<String>,
    analysis_context_path: Option<PathBuf>,
    source_roots: Vec<PathBuf>,
    generated_aliases: Vec<(PathBuf, PathBuf)>,
    module_aliases: Vec<(String, String)>,
    module_interface_aliases: Vec<(String, String)>,
}

impl WorkspaceIndexTarget {
    fn from_resolved(resolved: &ResolvedAnalysis) -> Self {
        let mut source_roots = vec![normalize_path(&resolved.input_file)];
        source_roots.extend(
            resolved
                .target_roots
                .iter()
                .map(|root| normalize_path(root)),
        );
        source_roots.sort();
        source_roots.dedup();

        let mut generated_aliases = resolved
            .source_path_aliases
            .iter()
            .map(|(source, generated)| (normalize_path(source), normalize_path(generated)))
            .collect::<Vec<_>>();
        generated_aliases.sort();

        let mut module_aliases = resolved
            .compile_options
            .module_aliases
            .iter()
            .map(|(name, path)| (name.clone(), path.clone()))
            .collect::<Vec<_>>();
        module_aliases.sort();

        let mut module_interface_aliases = resolved
            .compile_options
            .module_interface_aliases
            .iter()
            .map(|(name, path)| (name.clone(), path.clone()))
            .collect::<Vec<_>>();
        module_interface_aliases.sort();

        let target = resolved.target.as_ref();
        Self {
            input_file: normalize_path(&resolved.input_file),
            manifest_path: target.map(|target| normalize_path(&target.manifest_path)),
            workspace_root: target.map(|target| normalize_path(&target.workspace_root)),
            package_root: target.map(|target| normalize_path(&target.package_root)),
            package_name: target.map(|target| target.package_name.clone()),
            target_kind: target.and_then(|target| {
                target
                    .target_kind
                    .map(|kind| format!("{kind:?}").to_ascii_lowercase())
            }),
            target_name: target.and_then(|target| target.target_name.clone()),
            analysis_context_path: target
                .map(|target| normalize_path(&target.analysis_context_path)),
            source_roots,
            generated_aliases,
            module_aliases,
            module_interface_aliases,
        }
    }
}

#[derive(Default)]
struct WorkspaceIndex {
    generation: u64,
    symbol_indexes: BTreeMap<AnalysisCacheKey, Arc<SurfaceSymbolIndex>>,
    targets: BTreeMap<AnalysisCacheKey, WorkspaceIndexTarget>,
    last_refresh: Option<WorkspaceIndexStats>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkspaceIndexStats {
    generation: u64,
    indexed_targets: usize,
    failed_targets: usize,
}

pub enum DocumentSyncAction {
    ScheduleTarget {
        uri: String,
        mode: DiagnosticsAnalysisMode,
        prewarm: bool,
    },
    Immediate(AnalysisOutcome),
}

#[derive(Debug, Clone)]
pub struct AnalysisSnapshot {
    documents: BTreeMap<String, OpenDocument>,
    dirty_documents: Arc<DirtyDocumentsSnapshot>,
    open_uri_by_path: Arc<BTreeMap<PathBuf, String>>,
    workspace_roots: Vec<PathBuf>,
    cancellation: CancellationToken,
}

impl AnalysisSnapshot {
    fn check_canceled(&self) -> Result<(), String> {
        if self.cancellation.is_canceled() {
            return Err("request was canceled".to_string());
        }
        Ok(())
    }

    fn document(&self, uri: &str) -> Option<&OpenDocument> {
        self.documents.get(uri)
    }

    pub(crate) fn document_version(&self, uri: &str) -> Option<i64> {
        self.document(uri).map(|document| document.version)
    }

    fn document_source_file(&self, uri: &str) -> Option<SourceFile> {
        let document = self.document(uri)?;
        Some(SourceFile::new(
            document.path.clone(),
            document.text.clone(),
        ))
    }

    fn dirty_documents(&self) -> &DirtyDocumentsSnapshot {
        &self.dirty_documents
    }

    fn uri_by_normalized_path(&self) -> &BTreeMap<PathBuf, String> {
        &self.open_uri_by_path
    }

    fn workspace_roots(&self) -> &[PathBuf] {
        &self.workspace_roots
    }

    fn analysis_path_exists(&self, path: &Path) -> bool {
        let normalized = normalize_path(path);
        self.open_uri_by_path.contains_key(&normalized) || path.is_file()
    }
}

pub use kernc_driver::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnalysisTier {
    Lexical,
    ParseOnly,
    Surface,
    CleanSemantic,
    DirtySemantic,
}

impl AnalysisTier {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::ParseOnly => "parse-only",
            Self::Surface => "surface",
            Self::CleanSemantic => "clean-semantic",
            Self::DirtySemantic => "dirty-semantic",
        }
    }
}

#[derive(Debug, Clone)]
struct RenameTarget {
    query_span: Span,
    definition_span: Span,
    placeholder: String,
    behavior: RenameBehavior,
}

#[derive(Debug, Clone)]
enum RenameBehavior {
    Standard,
    ExpandPatternPun { field_name: String },
}

struct AnalysisRequestContext {
    resolved: ResolvedAnalysis,
    dirty_documents: DirtyDocumentsSnapshot,
    cache_key: AnalysisCacheKey,
    driver: AnalysisDriver,
    cancellation: CancellationToken,
}

impl AnalysisRequestContext {
    fn check_canceled(&self) -> Result<(), String> {
        if self.cancellation.is_canceled() {
            return Err("request was canceled".to_string());
        }
        Ok(())
    }

    fn with_driver<T>(&self, f: impl FnOnce(&CompilerDriver) -> T) -> T {
        self.driver.with(f)
    }
}

#[derive(Clone)]
struct AnalysisDriver {
    driver: Arc<CompilerDriver>,
    query_lock: Arc<Mutex<()>>,
}

impl AnalysisDriver {
    fn new(options: CompileOptions) -> Self {
        Self {
            driver: Arc::new(CompilerDriver::new(options)),
            query_lock: Arc::new(Mutex::new(())),
        }
    }

    fn with<T>(&self, f: impl FnOnce(&CompilerDriver) -> T) -> T {
        let _query_lock = self.query_lock.lock().unwrap();
        f(&self.driver)
    }
}

pub struct AnalysisEngine {
    documents: BTreeMap<String, OpenDocument>,
    settings: AnalysisSettings,
    project_cache: Arc<Mutex<BTreeMap<PathBuf, Option<AnalysisProject>>>>,
    driver_cache: Arc<Mutex<BTreeMap<IncrementalDriverKey, AnalysisDriver>>>,
    parse_cache: Arc<Mutex<BTreeMap<AnalysisCacheKey, Arc<ParsedModuleArtifact>>>>,
    surface_cache: Arc<Mutex<BTreeMap<AnalysisCacheKey, Arc<AnalysisSurfaceArtifact>>>>,
    structure_cache: Arc<Mutex<BTreeMap<AnalysisCacheKey, Arc<StructureArtifact>>>>,
    artifact_cache: Arc<Mutex<BTreeMap<AnalysisCacheKey, Arc<AnalysisArtifact>>>>,
    navigation_cache: Arc<Mutex<BTreeMap<AnalysisCacheKey, Arc<AnalysisNavigationArtifact>>>>,
    semantic_classification_cache:
        Arc<Mutex<BTreeMap<AnalysisCacheKey, Arc<AnalysisSemanticArtifact>>>>,
    workspace_index: Arc<Mutex<WorkspaceIndex>>,
    semantic_tokens_cache: Arc<Mutex<BTreeMap<SemanticTokensCacheKey, IdeSemanticTokens>>>,
    lexical_cache: Arc<Mutex<BTreeMap<LexicalCacheKey, Arc<LexicalIndex>>>>,
    dirty_documents_snapshot: Arc<Mutex<Option<Arc<DirtyDocumentsSnapshot>>>>,
    open_uri_by_path: Arc<Mutex<Option<Arc<BTreeMap<PathBuf, String>>>>>,
    last_analysis_tier: Arc<Mutex<Option<AnalysisTier>>>,
    next_snapshot_generation: Arc<AtomicU64>,
    last_analysis_trace: Arc<Mutex<AnalysisTrace>>,
}

impl Clone for AnalysisEngine {
    fn clone(&self) -> Self {
        Self {
            documents: self.documents.clone(),
            settings: self.settings.clone(),
            project_cache: self.project_cache.clone(),
            driver_cache: self.driver_cache.clone(),
            parse_cache: self.parse_cache.clone(),
            surface_cache: self.surface_cache.clone(),
            structure_cache: self.structure_cache.clone(),
            artifact_cache: self.artifact_cache.clone(),
            navigation_cache: self.navigation_cache.clone(),
            semantic_classification_cache: self.semantic_classification_cache.clone(),
            workspace_index: self.workspace_index.clone(),
            semantic_tokens_cache: self.semantic_tokens_cache.clone(),
            lexical_cache: self.lexical_cache.clone(),
            dirty_documents_snapshot: self.dirty_documents_snapshot.clone(),
            open_uri_by_path: self.open_uri_by_path.clone(),
            last_analysis_tier: Arc::new(Mutex::new(None)),
            next_snapshot_generation: self.next_snapshot_generation.clone(),
            last_analysis_trace: Arc::new(Mutex::new(AnalysisTrace::default())),
        }
    }
}

impl Default for AnalysisEngine {
    fn default() -> Self {
        Self::new(AnalysisSettings::default())
    }
}

impl AnalysisEngine {
    pub fn new(settings: AnalysisSettings) -> Self {
        Self {
            documents: BTreeMap::new(),
            settings,
            project_cache: Arc::new(Mutex::new(BTreeMap::new())),
            driver_cache: Arc::new(Mutex::new(BTreeMap::new())),
            parse_cache: Arc::new(Mutex::new(BTreeMap::new())),
            surface_cache: Arc::new(Mutex::new(BTreeMap::new())),
            structure_cache: Arc::new(Mutex::new(BTreeMap::new())),
            artifact_cache: Arc::new(Mutex::new(BTreeMap::new())),
            navigation_cache: Arc::new(Mutex::new(BTreeMap::new())),
            semantic_classification_cache: Arc::new(Mutex::new(BTreeMap::new())),
            workspace_index: Arc::new(Mutex::new(WorkspaceIndex::default())),
            semantic_tokens_cache: Arc::new(Mutex::new(BTreeMap::new())),
            lexical_cache: Arc::new(Mutex::new(BTreeMap::new())),
            dirty_documents_snapshot: Arc::new(Mutex::new(None)),
            open_uri_by_path: Arc::new(Mutex::new(None)),
            last_analysis_tier: Arc::new(Mutex::new(None)),
            next_snapshot_generation: Arc::new(AtomicU64::new(0)),
            last_analysis_trace: Arc::new(Mutex::new(AnalysisTrace::default())),
        }
    }

    pub fn settings(&self) -> &AnalysisSettings {
        &self.settings
    }

    pub fn replace_settings(&mut self, settings: AnalysisSettings) -> bool {
        if self.settings == settings {
            return false;
        }
        self.settings = settings;
        self.project_cache.lock().unwrap().clear();
        self.driver_cache.lock().unwrap().clear();
        self.invalidate_artifact_cache();
        self.invalidate_render_caches();
        true
    }

    fn record_analysis_tier(&self, tier: AnalysisTier) {
        self.last_analysis_tier.lock().unwrap().replace(tier);
    }

    fn record_cache_hit(&self, kind: AnalysisCacheTraceKind) {
        self.record_cache_event(kind, AnalysisCacheTraceOutcome::Hit);
    }

    fn record_cache_miss(&self, kind: AnalysisCacheTraceKind) {
        self.record_cache_event(kind, AnalysisCacheTraceOutcome::Miss);
    }

    fn record_cache_store(&self, kind: AnalysisCacheTraceKind) {
        self.record_cache_event(kind, AnalysisCacheTraceOutcome::Store);
    }

    fn record_cache_event(&self, kind: AnalysisCacheTraceKind, outcome: AnalysisCacheTraceOutcome) {
        self.last_analysis_trace
            .lock()
            .unwrap()
            .cache_events
            .push(AnalysisCacheTraceEvent { kind, outcome });
    }

    fn record_dirty_fallback(&self, fallback: &'static str) {
        self.last_analysis_trace
            .lock()
            .unwrap()
            .dirty_fallbacks
            .push(fallback);
    }

    pub(crate) fn start_analysis_trace(&self) -> u64 {
        let generation = self.next_snapshot_generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self.last_analysis_trace.lock().unwrap() = AnalysisTrace {
            snapshot_generation: Some(generation),
            ..AnalysisTrace::default()
        };
        generation
    }

    pub(crate) fn clear_last_analysis_tier(&self) {
        self.last_analysis_tier.lock().unwrap().take();
    }

    pub(crate) fn clear_last_analysis_trace(&self) {
        *self.last_analysis_trace.lock().unwrap() = AnalysisTrace::default();
    }

    pub(crate) fn last_analysis_trace(&self) -> AnalysisTrace {
        self.last_analysis_trace.lock().unwrap().clone()
    }

    pub(crate) fn snapshot(
        &self,
        workspace_roots: Vec<PathBuf>,
        cancellation: CancellationToken,
    ) -> AnalysisSnapshot {
        self.start_analysis_trace();
        AnalysisSnapshot {
            documents: self.documents.clone(),
            dirty_documents: self.dirty_documents_snapshot(),
            open_uri_by_path: self.open_uri_by_normalized_path(),
            workspace_roots,
            cancellation,
        }
    }

    fn analyze_document(&self, target_uri: &str) -> AnalysisOutcome {
        match self.analyze_targeted_dirty_outcome(target_uri) {
            Ok(Some(outcome)) => return outcome,
            Ok(None) => {}
            Err(message) => {
                return single_server_diagnostic(
                    target_uri.to_string(),
                    format!("analysis failed: {message}"),
                );
            }
        }

        match self.analyze_dirty_report(target_uri) {
            Ok(Some(report)) => {
                let mut bundles_by_uri = diagnostics_from_session(&report.session, &self.documents);
                bundles_by_uri.entry(target_uri.to_string()).or_default();
                self.retain_publishable_bundles(target_uri, &mut bundles_by_uri);

                return AnalysisOutcome {
                    bundles: bundles_by_uri
                        .into_iter()
                        .map(|(uri, diagnostics)| DiagnosticBundle { uri, diagnostics })
                        .collect(),
                };
            }
            Ok(None) => {}
            Err(message) => {
                return single_server_diagnostic(
                    target_uri.to_string(),
                    format!("analysis failed: {message}"),
                );
            }
        }

        let report = match self.analyze_diagnostic_report(target_uri) {
            Ok(report) => report,
            Err(message) => {
                return single_server_diagnostic(
                    target_uri.to_string(),
                    format!("analysis failed: {message}"),
                );
            }
        };

        let mut bundles_by_uri = diagnostics_from_session(&report.session, &self.documents);
        bundles_by_uri.entry(target_uri.to_string()).or_default();
        self.retain_publishable_bundles(target_uri, &mut bundles_by_uri);

        AnalysisOutcome {
            bundles: bundles_by_uri
                .into_iter()
                .map(|(uri, diagnostics)| DiagnosticBundle { uri, diagnostics })
                .collect(),
        }
    }

    fn analyze_document_structure(&self, target_uri: &str) -> AnalysisOutcome {
        if let Err(message) = self.resolve_analysis(target_uri) {
            return single_server_diagnostic(
                target_uri.to_string(),
                format!("analysis failed: {message}"),
            );
        }

        let session = match self.parse_open_document_session(target_uri) {
            Ok(session) => session,
            Err(message) => {
                return single_server_diagnostic(
                    target_uri.to_string(),
                    format!("analysis failed: {message}"),
                );
            }
        };

        let mut bundles_by_uri = diagnostics_from_session(&session, &self.documents);
        bundles_by_uri.entry(target_uri.to_string()).or_default();
        self.retain_publishable_bundles(target_uri, &mut bundles_by_uri);

        AnalysisOutcome {
            bundles: bundles_by_uri
                .into_iter()
                .map(|(uri, diagnostics)| DiagnosticBundle { uri, diagnostics })
                .collect(),
        }
    }

    fn parse_open_document_session(&self, target_uri: &str) -> Result<Session, String> {
        let target_doc = self
            .documents
            .get(target_uri)
            .ok_or_else(|| "document is not open".to_string())?;
        self.parse_open_document_session_for_document(target_doc)
    }

    fn parse_open_document_session_for_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        target_uri: &str,
    ) -> Result<Session, String> {
        let target_doc = snapshot
            .document(target_uri)
            .ok_or_else(|| "document is not open".to_string())?;
        self.parse_open_document_session_for_document(target_doc)
    }

    fn parse_open_document_session_for_document(
        &self,
        target_doc: &OpenDocument,
    ) -> Result<Session, String> {
        self.record_analysis_tier(AnalysisTier::ParseOnly);
        let mut session = kernc_utils::Session::new();
        session.apply_options(&self.settings.compile_options);
        let file_id = session.source_manager.add_file(
            target_doc.path.to_string_lossy().to_string(),
            target_doc.text.clone(),
        );
        let mut parser = kernc_parser::Parser::new(&target_doc.text, file_id, &mut session);
        let _ = parser.parse_module();
        Ok(session)
    }

    fn analyze_targeted_dirty_outcome(
        &self,
        target_uri: &str,
    ) -> Result<Option<AnalysisOutcome>, String> {
        let context = self.resolve_analysis_context(target_uri)?;
        let dirty_documents = self.dirty_documents_snapshot();
        if dirty_documents.len() != 1 {
            return Ok(None);
        }

        let clean_key = AnalysisCacheKey::clean(&context.resolved);
        let Some(clean_structure) = self
            .structure_cache
            .lock()
            .unwrap()
            .get(&clean_key)
            .cloned()
        else {
            self.record_dirty_fallback("targeted-missing-clean-structure");
            return Ok(None);
        };
        let Some(clean_artifact) = self.artifact_cache.lock().unwrap().get(&clean_key).cloned()
        else {
            self.record_dirty_fallback("targeted-missing-clean-artifact");
            return Ok(None);
        };
        let target_doc = self
            .documents
            .get(target_uri)
            .ok_or_else(|| "document is not open".to_string())?;
        let target_path = normalize_path(&target_doc.path);
        if clean_artifact.session.diagnostics.iter().any(|diagnostic| {
            diagnostic.level == kernc_utils::DiagnosticLevel::Error
                && span_in_path(
                    &clean_artifact.session,
                    diagnostic.primary_span,
                    &target_path,
                )
        }) {
            self.record_dirty_fallback("targeted-clean-errors");
            return Ok(None);
        }
        let mut bundles_by_uri = diagnostics_from_session(&clean_artifact.session, &self.documents);

        let parsed = self.parse_modules_for_context(&context)?;
        let Some(report) = context
            .with_driver(|driver| {
                driver.analyze_report_with_function_body_reuse(
                    &clean_artifact,
                    &clean_structure,
                    &parsed,
                    &context.cancellation,
                )
            })
            .map_err(|_| "request was canceled".to_string())?
        else {
            self.record_dirty_fallback("targeted-body-reuse-unavailable");
            return Ok(None);
        };

        let mut dirty_bundles = diagnostics_from_session(&report.report.session, &self.documents);
        let mut target_diagnostics = Vec::new();
        if bundles_by_uri
            .get(target_uri)
            .is_some_and(|diagnostics| !diagnostics.is_empty())
        {
            let clean_target_file = clean_artifact
                .session
                .diagnostics
                .iter()
                .find_map(|diagnostic| {
                    span_in_path(
                        &clean_artifact.session,
                        diagnostic.primary_span,
                        &target_path,
                    )
                    .then(|| {
                        clean_artifact
                            .session
                            .source_manager
                            .get_file(diagnostic.primary_span.file)
                    })
                    .flatten()
                })
                .cloned()
                .ok_or_else(|| "targeted analysis missing clean target file".to_string())?;
            let dirty_target_file =
                SourceFile::new(target_doc.path.clone(), target_doc.text.clone());
            target_diagnostics = preserve_target_diagnostics(
                &clean_artifact,
                &clean_target_file,
                &dirty_target_file,
                target_uri,
                &report,
            );
        }
        target_diagnostics.extend(dirty_bundles.remove(target_uri).unwrap_or_default());
        bundles_by_uri.insert(target_uri.to_string(), target_diagnostics);
        self.retain_publishable_bundles(target_uri, &mut bundles_by_uri);

        self.record_analysis_tier(AnalysisTier::DirtySemantic);
        Ok(Some(AnalysisOutcome {
            bundles: bundles_by_uri
                .into_iter()
                .map(|(uri, diagnostics)| DiagnosticBundle { uri, diagnostics })
                .collect(),
        }))
    }

    fn analyze_dirty_report(
        &self,
        target_uri: &str,
    ) -> Result<Option<kernc_driver::AnalysisReport>, String> {
        let context = self.resolve_analysis_context(target_uri)?;
        if context.dirty_documents.is_clean() {
            return Ok(None);
        }

        let clean_key = AnalysisCacheKey::clean(&context.resolved);
        let Some(clean_structure) = self
            .structure_cache
            .lock()
            .unwrap()
            .get(&clean_key)
            .cloned()
        else {
            self.record_dirty_fallback("dirty-report-missing-clean-structure");
            return Ok(None);
        };

        let parsed = self.parse_modules_for_context(&context)?;
        let report = context
            .with_driver(|driver| {
                driver.analyze_report_from_structure_and_parsed(
                    &clean_structure,
                    &parsed,
                    &context.cancellation,
                )
            })
            .map_err(|_| "request was canceled".to_string())?
            .filter(|_| !context.dirty_documents.is_clean());
        if report.is_some() {
            self.record_analysis_tier(AnalysisTier::DirtySemantic);
        }
        Ok(report)
    }

    #[cfg(test)]
    fn source_overrides(&self) -> SourceOverrides {
        self.dirty_documents_snapshot().overrides.clone()
    }

    pub(crate) fn last_analysis_tier(&self) -> Option<AnalysisTier> {
        *self.last_analysis_tier.lock().unwrap()
    }

    pub(crate) fn document_version(&self, uri: &str) -> Option<i64> {
        self.documents.get(uri).map(|document| document.version)
    }

    fn dirty_documents_snapshot(&self) -> Arc<DirtyDocumentsSnapshot> {
        if let Some(snapshot) = self.dirty_documents_snapshot.lock().unwrap().as_ref() {
            return Arc::clone(snapshot);
        }

        let mut overrides = SourceOverrides::default();
        let mut hashed_overrides = self
            .documents
            .values()
            .filter(|doc| doc.is_dirty)
            .map(|doc| {
                overrides.insert(doc.path.clone(), doc.text.clone());
                (normalize_path(&doc.path), doc.text_hash)
            })
            .collect::<Vec<_>>();
        hashed_overrides.sort();

        let snapshot = Arc::new(DirtyDocumentsSnapshot {
            overrides,
            hashed_overrides,
        });
        self.dirty_documents_snapshot
            .lock()
            .unwrap()
            .replace(Arc::clone(&snapshot));
        snapshot
    }

    fn open_uri_by_normalized_path(&self) -> Arc<BTreeMap<PathBuf, String>> {
        if let Some(uri_by_path) = self.open_uri_by_path.lock().unwrap().as_ref() {
            return Arc::clone(uri_by_path);
        }

        let uri_by_path = Arc::new(
            self.documents
                .iter()
                .map(|(uri, doc)| (normalize_path(&doc.path), uri.clone()))
                .collect(),
        );
        self.open_uri_by_path
            .lock()
            .unwrap()
            .replace(Arc::clone(&uri_by_path));
        uri_by_path
    }

    fn analyze_diagnostic_report(&self, target_uri: &str) -> Result<AnalysisReport, String> {
        let context = self.resolve_analysis_context(target_uri)?;
        if context.dirty_documents.is_clean() {
            let artifact = self.analyze_artifact_for_context(&context)?;
            return Ok(AnalysisReport {
                session: artifact.session.clone(),
                succeeded: artifact.succeeded,
            });
        }

        let structure =
            if let Some(structure) = self.structure_cache.lock().unwrap().get(&context.cache_key) {
                self.record_cache_hit(AnalysisCacheTraceKind::StructureArtifact);
                Some(Arc::clone(structure))
            } else {
                self.record_cache_miss(AnalysisCacheTraceKind::StructureArtifact);
                context
                    .with_driver(|driver| {
                        driver.analyze_structure_cancelable(
                            &context.resolved.input_file.to_string_lossy(),
                            &context.dirty_documents.overrides,
                            &context.cancellation,
                        )
                    })
                    .map_err(|_| "request was canceled".to_string())?
                    .map(Arc::new)
            };
        self.prune_cache_family_for_insert(&context.cache_key);
        if let Some(structure) = &structure {
            self.structure_cache
                .lock()
                .unwrap()
                .insert(context.cache_key.clone(), Arc::clone(structure));
            self.record_cache_store(AnalysisCacheTraceKind::StructureArtifact);
        }

        if let Some(structure) = structure {
            context
                .with_driver(|driver| {
                    driver.analyze_report_from_structure(&structure, &context.cancellation)
                })
                .map_err(|_| "request was canceled".to_string())
        } else {
            context
                .with_driver(|driver| {
                    driver.analyze_report(
                        &context.resolved.input_file.to_string_lossy(),
                        &context.dirty_documents.overrides,
                        &context.cancellation,
                    )
                })
                .map_err(|_| "request was canceled".to_string())
        }
    }

    fn analyze_interactive_artifact_for_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        target_uri: &str,
    ) -> Result<Arc<AnalysisArtifact>, String> {
        snapshot.check_canceled()?;
        let context = self.resolve_analysis_context_for_snapshot(snapshot, target_uri)?;
        self.analyze_interactive_artifact_for_context(&context)
    }

    fn analyze_interactive_artifact_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Result<Arc<AnalysisArtifact>, String> {
        context.check_canceled()?;
        if context.dirty_documents.is_clean() {
            self.record_analysis_tier(AnalysisTier::CleanSemantic);
            return self.analyze_artifact_for_context(context);
        }

        context.check_canceled()?;
        if !context.resolved.input_file.is_file() {
            self.record_analysis_tier(AnalysisTier::DirtySemantic);
            return self.analyze_artifact_for_context(context);
        }

        self.record_analysis_tier(AnalysisTier::CleanSemantic);
        self.analyze_clean_artifact_for_context(context)
    }

    fn analyze_interactive_navigation_artifact_for_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        target_uri: &str,
    ) -> Result<Arc<AnalysisNavigationArtifact>, String> {
        snapshot.check_canceled()?;
        let context = self.resolve_analysis_context_for_snapshot(snapshot, target_uri)?;
        self.analyze_interactive_navigation_artifact_for_context(&context)
    }

    fn analyze_interactive_navigation_artifact_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Result<Arc<AnalysisNavigationArtifact>, String> {
        context.check_canceled()?;
        if context.dirty_documents.is_clean() {
            self.record_analysis_tier(AnalysisTier::CleanSemantic);
            return self.analyze_navigation_artifact_for_context(context);
        }

        context.check_canceled()?;
        if !context.resolved.input_file.is_file() {
            self.record_analysis_tier(AnalysisTier::DirtySemantic);
            return self.analyze_navigation_artifact_for_context(context);
        }

        if !self.dirty_navigation_can_use_clean_artifact(context) {
            self.record_analysis_tier(AnalysisTier::DirtySemantic);
            return self.analyze_navigation_artifact_for_context(context);
        }

        self.record_analysis_tier(AnalysisTier::CleanSemantic);
        self.analyze_clean_navigation_artifact_for_context(context)
    }

    fn analyze_interactive_semantic_classification_artifact_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Result<Arc<AnalysisSemanticArtifact>, String> {
        context.check_canceled()?;
        if context.dirty_documents.is_clean() {
            self.record_analysis_tier(AnalysisTier::CleanSemantic);
            return self.analyze_semantic_classification_artifact_for_context(context);
        }

        context.check_canceled()?;
        if !context.resolved.input_file.is_file() {
            self.record_analysis_tier(AnalysisTier::DirtySemantic);
            return self.analyze_semantic_classification_artifact_for_context(context);
        }

        if !self.dirty_navigation_can_use_clean_artifact(context) {
            self.record_analysis_tier(AnalysisTier::DirtySemantic);
            return self.analyze_semantic_classification_artifact_for_context(context);
        }

        self.record_analysis_tier(AnalysisTier::CleanSemantic);
        self.analyze_clean_semantic_classification_artifact_for_context(context)
    }

    pub(crate) fn prewarm_interactive_artifacts_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        target_uri: &str,
    ) -> Result<(), String> {
        snapshot.check_canceled()?;
        let context = self.resolve_analysis_context_for_snapshot(snapshot, target_uri)?;
        let _ = self.analyze_interactive_navigation_artifact_for_context(&context)?;
        let _ = self.analyze_interactive_semantic_classification_artifact_for_context(&context)?;
        Ok(())
    }

    fn analyze_dirty_semantic_classification_artifact_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Result<Option<Arc<AnalysisSemanticArtifact>>, String> {
        context.check_canceled()?;
        if context.dirty_documents.is_clean() || !context.resolved.input_file.is_file() {
            return Ok(None);
        }

        if let Some(artifact) = self
            .semantic_classification_cache
            .lock()
            .unwrap()
            .get(&context.cache_key)
        {
            self.record_cache_hit(AnalysisCacheTraceKind::SemanticClassificationArtifact);
            return Ok(Some(Arc::clone(artifact)));
        }
        self.record_cache_miss(AnalysisCacheTraceKind::SemanticClassificationArtifact);

        let clean_key = AnalysisCacheKey::clean(&context.resolved);
        let Some(clean_structure) = self
            .structure_cache
            .lock()
            .unwrap()
            .get(&clean_key)
            .cloned()
        else {
            return Ok(None);
        };

        let Some(parsed) = context
            .with_driver(|driver| {
                driver.parse_modules(
                    &context.resolved.input_file.to_string_lossy(),
                    &context.dirty_documents.overrides,
                    &context.cancellation,
                )
            })
            .map_err(|_| "request was canceled".to_string())?
        else {
            return Ok(None);
        };

        let Some(artifact) = context
            .with_driver(|driver| {
                driver.analyze_semantic_artifact_from_structure_and_parsed(
                    &clean_structure,
                    &parsed,
                    &context.cancellation,
                )
            })
            .map_err(|_| "request was canceled".to_string())?
        else {
            return Ok(None);
        };

        let artifact = Arc::new(artifact);
        self.semantic_classification_cache
            .lock()
            .unwrap()
            .insert(context.cache_key.clone(), Arc::clone(&artifact));
        self.record_cache_store(AnalysisCacheTraceKind::SemanticClassificationArtifact);
        Ok(Some(artifact))
    }

    fn dirty_navigation_can_use_clean_artifact(&self, context: &AnalysisRequestContext) -> bool {
        let clean_key = AnalysisCacheKey::clean(&context.resolved);
        let Some(parsed) = context
            .with_driver(|driver| {
                driver.parse_modules(
                    &context.resolved.input_file.to_string_lossy(),
                    &context.dirty_documents.overrides,
                    &context.cancellation,
                )
            })
            .ok()
            .flatten()
        else {
            return true;
        };

        if let Some(clean_structure) = self
            .structure_cache
            .lock()
            .unwrap()
            .get(&clean_key)
            .cloned()
        {
            return context.with_driver(|driver| {
                driver.parsed_modules_match_structure_body_only(&clean_structure, &parsed)
            });
        }

        let Some(clean_parsed) = self
            .driver_for_resolved(&context.resolved)
            .with(|driver| {
                driver.parse_modules(
                    &context.resolved.input_file.to_string_lossy(),
                    &SourceOverrides::new(),
                    &context.cancellation,
                )
            })
            .ok()
            .flatten()
        else {
            return true;
        };
        context.with_driver(|driver| driver.parsed_modules_match_body_only(&clean_parsed, &parsed))
    }

    fn analyze_artifact_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Result<Arc<AnalysisArtifact>, String> {
        context.check_canceled()?;
        if let Some(artifact) = self.artifact_cache.lock().unwrap().get(&context.cache_key) {
            self.record_cache_hit(AnalysisCacheTraceKind::SemanticArtifact);
            return Ok(Arc::clone(artifact));
        }
        self.record_cache_miss(AnalysisCacheTraceKind::SemanticArtifact);

        context.check_canceled()?;
        let structure =
            if let Some(structure) = self.structure_cache.lock().unwrap().get(&context.cache_key) {
                self.record_cache_hit(AnalysisCacheTraceKind::StructureArtifact);
                Some(Arc::clone(structure))
            } else {
                self.record_cache_miss(AnalysisCacheTraceKind::StructureArtifact);
                context
                    .with_driver(|driver| {
                        driver.analyze_structure_cancelable(
                            &context.resolved.input_file.to_string_lossy(),
                            &context.dirty_documents.overrides,
                            &context.cancellation,
                        )
                    })
                    .map_err(|_| "request was canceled".to_string())?
                    .map(Arc::new)
            };
        self.prune_cache_family_for_insert(&context.cache_key);
        if let Some(structure) = &structure {
            self.structure_cache
                .lock()
                .unwrap()
                .insert(context.cache_key.clone(), Arc::clone(structure));
            self.record_cache_store(AnalysisCacheTraceKind::StructureArtifact);
        }

        context.check_canceled()?;
        let artifact = Arc::new(if let Some(structure) = structure {
            context
                .with_driver(|driver| {
                    driver.analyze_artifact_from_structure(&structure, &context.cancellation)
                })
                .map_err(|_| "request was canceled".to_string())?
        } else {
            context
                .with_driver(|driver| {
                    driver.analyze_artifact(
                        &context.resolved.input_file.to_string_lossy(),
                        &context.dirty_documents.overrides,
                        &context.cancellation,
                    )
                })
                .map_err(|_| "request was canceled".to_string())?
        });
        self.artifact_cache
            .lock()
            .unwrap()
            .insert(context.cache_key.clone(), Arc::clone(&artifact));
        self.record_cache_store(AnalysisCacheTraceKind::SemanticArtifact);
        self.store_derived_interactive_artifacts(&context.cache_key, &artifact);
        Ok(artifact)
    }

    fn store_derived_interactive_artifacts(
        &self,
        cache_key: &AnalysisCacheKey,
        artifact: &Arc<AnalysisArtifact>,
    ) {
        let navigation = Arc::new(AnalysisNavigationArtifact {
            session: artifact.session.clone(),
            succeeded: artifact.succeeded,
            symbols: artifact.symbols.clone(),
            references: artifact.references.clone(),
            hovers: artifact.hovers.clone(),
            type_hints: artifact.type_hints.clone(),
            definition_links: artifact.definition_links.clone(),
            semantic_entries: artifact.semantic_entries.clone(),
            calls: artifact.calls.clone(),
        });
        self.navigation_cache
            .lock()
            .unwrap()
            .insert(cache_key.clone(), navigation);
        self.record_cache_store(AnalysisCacheTraceKind::NavigationArtifact);

        let semantic = Arc::new(AnalysisSemanticArtifact {
            session: artifact.session.clone(),
            succeeded: artifact.succeeded,
            symbols: artifact.symbols.clone(),
            references: artifact.references.clone(),
            hovers: artifact.hovers.clone(),
            type_hints: artifact.type_hints.clone(),
            semantic_entries: artifact.semantic_entries.clone(),
        });
        self.semantic_classification_cache
            .lock()
            .unwrap()
            .insert(cache_key.clone(), semantic);
        self.record_cache_store(AnalysisCacheTraceKind::SemanticClassificationArtifact);
    }

    fn analyze_structure_artifact_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Result<Option<Arc<StructureArtifact>>, String> {
        context.check_canceled()?;
        let structure =
            if let Some(structure) = self.structure_cache.lock().unwrap().get(&context.cache_key) {
                self.record_cache_hit(AnalysisCacheTraceKind::StructureArtifact);
                Some(Arc::clone(structure))
            } else {
                self.record_cache_miss(AnalysisCacheTraceKind::StructureArtifact);
                context
                    .with_driver(|driver| {
                        driver.analyze_structure_cancelable(
                            &context.resolved.input_file.to_string_lossy(),
                            &context.dirty_documents.overrides,
                            &context.cancellation,
                        )
                    })
                    .map_err(|_| "request was canceled".to_string())?
                    .map(Arc::new)
            };
        self.prune_cache_family_for_insert(&context.cache_key);
        if let Some(structure) = &structure {
            self.structure_cache
                .lock()
                .unwrap()
                .insert(context.cache_key.clone(), Arc::clone(structure));
            self.record_cache_store(AnalysisCacheTraceKind::StructureArtifact);
        }
        Ok(structure)
    }

    fn analyze_navigation_artifact_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Result<Arc<AnalysisNavigationArtifact>, String> {
        context.check_canceled()?;
        if let Some(artifact) = self
            .navigation_cache
            .lock()
            .unwrap()
            .get(&context.cache_key)
        {
            self.record_cache_hit(AnalysisCacheTraceKind::NavigationArtifact);
            return Ok(Arc::clone(artifact));
        }
        self.record_cache_miss(AnalysisCacheTraceKind::NavigationArtifact);

        context.check_canceled()?;
        let structure =
            if let Some(structure) = self.structure_cache.lock().unwrap().get(&context.cache_key) {
                self.record_cache_hit(AnalysisCacheTraceKind::StructureArtifact);
                Some(Arc::clone(structure))
            } else {
                self.record_cache_miss(AnalysisCacheTraceKind::StructureArtifact);
                context
                    .with_driver(|driver| {
                        driver.analyze_structure_cancelable(
                            &context.resolved.input_file.to_string_lossy(),
                            &context.dirty_documents.overrides,
                            &context.cancellation,
                        )
                    })
                    .map_err(|_| "request was canceled".to_string())?
                    .map(Arc::new)
            };
        self.prune_cache_family_for_insert(&context.cache_key);
        if let Some(structure) = &structure {
            self.structure_cache
                .lock()
                .unwrap()
                .insert(context.cache_key.clone(), Arc::clone(structure));
            self.record_cache_store(AnalysisCacheTraceKind::StructureArtifact);
        }

        context.check_canceled()?;
        let artifact = Arc::new(if let Some(structure) = structure {
            context
                .with_driver(|driver| {
                    driver.analyze_navigation_artifact_from_structure(
                        &structure,
                        &context.cancellation,
                    )
                })
                .map_err(|_| "request was canceled".to_string())?
        } else {
            context
                .with_driver(|driver| {
                    driver.analyze_navigation_artifact(
                        &context.resolved.input_file.to_string_lossy(),
                        &context.dirty_documents.overrides,
                        &context.cancellation,
                    )
                })
                .map_err(|_| "request was canceled".to_string())?
        });
        self.navigation_cache
            .lock()
            .unwrap()
            .insert(context.cache_key.clone(), Arc::clone(&artifact));
        self.record_cache_store(AnalysisCacheTraceKind::NavigationArtifact);
        Ok(artifact)
    }

    fn analyze_semantic_classification_artifact_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Result<Arc<AnalysisSemanticArtifact>, String> {
        context.check_canceled()?;
        if let Some(artifact) = self
            .semantic_classification_cache
            .lock()
            .unwrap()
            .get(&context.cache_key)
        {
            self.record_cache_hit(AnalysisCacheTraceKind::SemanticClassificationArtifact);
            return Ok(Arc::clone(artifact));
        }
        self.record_cache_miss(AnalysisCacheTraceKind::SemanticClassificationArtifact);

        context.check_canceled()?;
        let structure =
            if let Some(structure) = self.structure_cache.lock().unwrap().get(&context.cache_key) {
                self.record_cache_hit(AnalysisCacheTraceKind::StructureArtifact);
                Some(Arc::clone(structure))
            } else {
                self.record_cache_miss(AnalysisCacheTraceKind::StructureArtifact);
                context
                    .with_driver(|driver| {
                        driver.analyze_structure_cancelable(
                            &context.resolved.input_file.to_string_lossy(),
                            &context.dirty_documents.overrides,
                            &context.cancellation,
                        )
                    })
                    .map_err(|_| "request was canceled".to_string())?
                    .map(Arc::new)
            };
        self.prune_cache_family_for_insert(&context.cache_key);
        if let Some(structure) = &structure {
            self.structure_cache
                .lock()
                .unwrap()
                .insert(context.cache_key.clone(), Arc::clone(structure));
            self.record_cache_store(AnalysisCacheTraceKind::StructureArtifact);
        }

        context.check_canceled()?;
        let artifact = Arc::new(if let Some(structure) = structure {
            context
                .with_driver(|driver| {
                    driver
                        .analyze_semantic_artifact_from_structure(&structure, &context.cancellation)
                })
                .map_err(|_| "request was canceled".to_string())?
        } else {
            context
                .with_driver(|driver| {
                    driver.analyze_semantic_artifact(
                        &context.resolved.input_file.to_string_lossy(),
                        &context.dirty_documents.overrides,
                        &context.cancellation,
                    )
                })
                .map_err(|_| "request was canceled".to_string())?
        });
        self.semantic_classification_cache
            .lock()
            .unwrap()
            .insert(context.cache_key.clone(), Arc::clone(&artifact));
        self.record_cache_store(AnalysisCacheTraceKind::SemanticClassificationArtifact);
        Ok(artifact)
    }

    fn analyze_surface_artifact_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Result<Option<Arc<AnalysisSurfaceArtifact>>, String> {
        context.check_canceled()?;
        if let Some(surface) = self.surface_cache.lock().unwrap().get(&context.cache_key) {
            self.record_cache_hit(AnalysisCacheTraceKind::SurfaceArtifact);
            return Ok(Some(Arc::clone(surface)));
        }
        self.record_cache_miss(AnalysisCacheTraceKind::SurfaceArtifact);

        context.check_canceled()?;
        let surface = match context
            .with_driver(|driver| {
                driver.analyze_surface(
                    &context.resolved.input_file.to_string_lossy(),
                    &context.dirty_documents.overrides,
                    &context.cancellation,
                )
            })
            .map_err(|_| "request was canceled".to_string())?
        {
            Some(surface) => Arc::new(surface),
            None => return Ok(None),
        };
        self.prune_cache_family_for_insert(&context.cache_key);
        self.surface_cache
            .lock()
            .unwrap()
            .insert(context.cache_key.clone(), Arc::clone(&surface));
        self.record_cache_store(AnalysisCacheTraceKind::SurfaceArtifact);
        Ok(Some(surface))
    }

    fn analyze_imported_structure_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Result<Option<Arc<ImportedStructureArtifact>>, String> {
        context.check_canceled()?;
        let imported = context
            .with_driver(|driver| {
                driver.analyze_imported_structure(
                    &context.resolved.input_file.to_string_lossy(),
                    &context.dirty_documents.overrides,
                    &context.cancellation,
                )
            })
            .map_err(|_| "request was canceled".to_string())?;
        Ok(imported.map(Arc::new))
    }

    fn analyze_clean_artifact_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Result<Arc<AnalysisArtifact>, String> {
        context.check_canceled()?;
        let clean_key = AnalysisCacheKey::clean(&context.resolved);
        if let Some(artifact) = self.artifact_cache.lock().unwrap().get(&clean_key) {
            self.record_cache_hit(AnalysisCacheTraceKind::SemanticArtifact);
            return Ok(Arc::clone(artifact));
        }
        self.record_cache_miss(AnalysisCacheTraceKind::SemanticArtifact);

        context.check_canceled()?;
        let artifact = Arc::new(
            context
                .with_driver(|driver| {
                    driver.analyze_artifact(
                        &context.resolved.input_file.to_string_lossy(),
                        &SourceOverrides::new(),
                        &context.cancellation,
                    )
                })
                .map_err(|_| "request was canceled".to_string())?,
        );
        self.artifact_cache
            .lock()
            .unwrap()
            .insert(clean_key, Arc::clone(&artifact));
        self.record_cache_store(AnalysisCacheTraceKind::SemanticArtifact);
        Ok(artifact)
    }

    fn analyze_clean_navigation_artifact_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Result<Arc<AnalysisNavigationArtifact>, String> {
        context.check_canceled()?;
        let clean_key = AnalysisCacheKey::clean(&context.resolved);
        if let Some(artifact) = self.navigation_cache.lock().unwrap().get(&clean_key) {
            self.record_cache_hit(AnalysisCacheTraceKind::NavigationArtifact);
            return Ok(Arc::clone(artifact));
        }
        self.record_cache_miss(AnalysisCacheTraceKind::NavigationArtifact);

        context.check_canceled()?;
        let artifact = Arc::new(
            context
                .with_driver(|driver| {
                    driver.analyze_navigation_artifact(
                        &context.resolved.input_file.to_string_lossy(),
                        &SourceOverrides::new(),
                        &context.cancellation,
                    )
                })
                .map_err(|_| "request was canceled".to_string())?,
        );
        self.navigation_cache
            .lock()
            .unwrap()
            .insert(clean_key, Arc::clone(&artifact));
        self.record_cache_store(AnalysisCacheTraceKind::NavigationArtifact);
        Ok(artifact)
    }

    fn analyze_clean_semantic_classification_artifact_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Result<Arc<AnalysisSemanticArtifact>, String> {
        context.check_canceled()?;
        let clean_key = AnalysisCacheKey::clean(&context.resolved);
        if let Some(artifact) = self
            .semantic_classification_cache
            .lock()
            .unwrap()
            .get(&clean_key)
        {
            self.record_cache_hit(AnalysisCacheTraceKind::SemanticClassificationArtifact);
            return Ok(Arc::clone(artifact));
        }
        self.record_cache_miss(AnalysisCacheTraceKind::SemanticClassificationArtifact);

        context.check_canceled()?;
        let artifact = Arc::new(
            context
                .with_driver(|driver| {
                    driver.analyze_semantic_artifact(
                        &context.resolved.input_file.to_string_lossy(),
                        &SourceOverrides::new(),
                        &context.cancellation,
                    )
                })
                .map_err(|_| "request was canceled".to_string())?,
        );
        self.semantic_classification_cache
            .lock()
            .unwrap()
            .insert(clean_key, Arc::clone(&artifact));
        self.record_cache_store(AnalysisCacheTraceKind::SemanticClassificationArtifact);
        Ok(artifact)
    }

    fn analyze_clean_surface_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Result<Option<Arc<AnalysisSurfaceArtifact>>, String> {
        context.check_canceled()?;
        let clean_key = AnalysisCacheKey::clean(&context.resolved);
        if let Some(surface) = self.surface_cache.lock().unwrap().get(&clean_key) {
            self.record_cache_hit(AnalysisCacheTraceKind::SurfaceArtifact);
            return Ok(Some(Arc::clone(surface)));
        }
        self.record_cache_miss(AnalysisCacheTraceKind::SurfaceArtifact);

        context.check_canceled()?;
        let surface = match context
            .with_driver(|driver| {
                driver.analyze_surface(
                    &context.resolved.input_file.to_string_lossy(),
                    &SourceOverrides::new(),
                    &context.cancellation,
                )
            })
            .map_err(|_| "request was canceled".to_string())?
        {
            Some(surface) => Arc::new(surface),
            None => return Ok(None),
        };
        self.surface_cache
            .lock()
            .unwrap()
            .insert(clean_key, Arc::clone(&surface));
        self.record_cache_store(AnalysisCacheTraceKind::SurfaceArtifact);
        Ok(Some(surface))
    }

    fn parse_modules_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Result<Arc<ParsedModuleArtifact>, String> {
        context.check_canceled()?;
        if let Some(parsed) = self.parse_cache.lock().unwrap().get(&context.cache_key) {
            self.record_cache_hit(AnalysisCacheTraceKind::ParseArtifact);
            return Ok(Arc::clone(parsed));
        }
        self.record_cache_miss(AnalysisCacheTraceKind::ParseArtifact);

        context.check_canceled()?;
        let parsed = context
            .with_driver(|driver| {
                driver.parse_modules(
                    &context.resolved.input_file.to_string_lossy(),
                    &context.dirty_documents.overrides,
                    &context.cancellation,
                )
            })
            .map_err(|_| "request was canceled".to_string())?;
        let Some(parsed) = parsed.map(Arc::new) else {
            return Err("parse analysis failed".to_string());
        };
        self.prune_cache_family_for_insert(&context.cache_key);
        self.parse_cache
            .lock()
            .unwrap()
            .insert(context.cache_key.clone(), Arc::clone(&parsed));
        self.record_cache_store(AnalysisCacheTraceKind::ParseArtifact);
        Ok(parsed)
    }

    fn resolve_analysis_context(&self, target_uri: &str) -> Result<AnalysisRequestContext, String> {
        let target_doc = self
            .documents
            .get(target_uri)
            .ok_or_else(|| "document is not open".to_string())?;
        self.resolve_analysis_context_for_document(target_doc)
    }

    fn resolve_analysis_context_for_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        target_uri: &str,
    ) -> Result<AnalysisRequestContext, String> {
        let target_doc = snapshot
            .document(target_uri)
            .ok_or_else(|| "document is not open".to_string())?;
        let resolved = self.resolve_analysis_for_snapshot_document(snapshot, target_doc)?;
        self.analysis_context_for_resolved_and_dirty(
            resolved,
            snapshot.dirty_documents(),
            snapshot.cancellation.clone(),
        )
    }

    fn resolve_analysis_context_for_document(
        &self,
        target_doc: &OpenDocument,
    ) -> Result<AnalysisRequestContext, String> {
        let resolved = self.resolve_analysis_for_document(target_doc)?;
        self.analysis_context_for_resolved(resolved)
    }

    fn analysis_context_for_resolved(
        &self,
        resolved: ResolvedAnalysis,
    ) -> Result<AnalysisRequestContext, String> {
        let dirty_documents = self.dirty_documents_snapshot();
        self.analysis_context_for_resolved_and_dirty(
            resolved,
            dirty_documents.as_ref(),
            CancellationToken::new(),
        )
    }

    fn analysis_context_for_resolved_and_dirty(
        &self,
        resolved: ResolvedAnalysis,
        dirty_snapshot: &DirtyDocumentsSnapshot,
        cancellation: CancellationToken,
    ) -> Result<AnalysisRequestContext, String> {
        let dirty_documents = dirty_snapshot
            .filter_for_resolved(&resolved)
            .remap_for(&resolved.source_path_aliases);
        let cache_key = AnalysisCacheKey::from_resolved_dirty_snapshot(&resolved, &dirty_documents);
        let driver = self.driver_for_resolved(&resolved);
        if cancellation.is_canceled() {
            return Err("request was canceled".to_string());
        }
        Ok(AnalysisRequestContext {
            resolved,
            dirty_documents,
            cache_key,
            driver,
            cancellation,
        })
    }

    fn driver_for_resolved(&self, resolved: &ResolvedAnalysis) -> AnalysisDriver {
        let family = IncrementalDriverKey::from_options(&resolved.compile_options);
        if let Some(driver) = self.driver_cache.lock().unwrap().get(&family) {
            self.record_cache_hit(AnalysisCacheTraceKind::Driver);
            return driver.clone();
        }
        self.record_cache_miss(AnalysisCacheTraceKind::Driver);

        let driver = AnalysisDriver::new(resolved.compile_options.clone());
        self.driver_cache
            .lock()
            .unwrap()
            .insert(family, driver.clone());
        self.record_cache_store(AnalysisCacheTraceKind::Driver);
        driver
    }

    fn resolve_analysis(&self, target_uri: &str) -> Result<ResolvedAnalysis, String> {
        let Some(target_doc) = self.documents.get(target_uri) else {
            return Err("document is not open".to_string());
        };
        self.resolve_analysis_for_document(target_doc)
    }

    fn resolve_analysis_for_document(
        &self,
        target_doc: &OpenDocument,
    ) -> Result<ResolvedAnalysis, String> {
        if let Some(project) = self.project_for_path(&target_doc.path)? {
            let mut resolved =
                project.resolve_for_file(&target_doc.path, &self.settings.compile_options);
            inject_driver_condition_defines(&mut resolved.compile_options);
            return Ok(resolved);
        }

        let mut compile_options = self.settings.compile_options.clone();
        apply_configured_library_aliases(&mut compile_options);
        inject_driver_condition_defines(&mut compile_options);
        Ok(ResolvedAnalysis {
            input_file: self.infer_standalone_analysis_root(&target_doc.path),
            compile_options,
            source_path_aliases: BTreeMap::new(),
            target_roots: Vec::new(),
            target: None,
        })
    }

    fn resolve_analysis_for_snapshot_document(
        &self,
        snapshot: &AnalysisSnapshot,
        target_doc: &OpenDocument,
    ) -> Result<ResolvedAnalysis, String> {
        if let Some(project) = self.project_for_path(&target_doc.path)? {
            let mut resolved =
                project.resolve_for_file(&target_doc.path, &self.settings.compile_options);
            inject_driver_condition_defines(&mut resolved.compile_options);
            return Ok(resolved);
        }

        let mut compile_options = self.settings.compile_options.clone();
        apply_configured_library_aliases(&mut compile_options);
        inject_driver_condition_defines(&mut compile_options);
        Ok(ResolvedAnalysis {
            input_file: self
                .infer_standalone_analysis_root_for_snapshot(snapshot, &target_doc.path),
            compile_options,
            source_path_aliases: BTreeMap::new(),
            target_roots: Vec::new(),
            target: None,
        })
    }

    fn project_for_path(&self, path: &Path) -> Result<Option<AnalysisProject>, String> {
        let start = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or_else(|| Path::new("."))
        };
        let manifest_path = match resolve_project_manifest_path(Some(start)) {
            Ok(path) => path,
            Err(CraftError::ManifestNotFound { .. }) => return Ok(None),
            Err(err) => {
                return Err(format!(
                    "failed to resolve Craft project for LSP analysis: {err}"
                ));
            }
        };

        if let Some(project) = self.project_cache.lock().unwrap().get(&manifest_path) {
            self.record_cache_hit(AnalysisCacheTraceKind::ProjectResolution);
            return Ok(project.clone());
        }
        self.record_cache_miss(AnalysisCacheTraceKind::ProjectResolution);

        let project = AnalysisProject::load_from_manifest(&manifest_path)
            .map(Some)
            .map_err(|err| {
                format!(
                    "failed to load Craft project `{}` for LSP analysis: {err}",
                    manifest_path.display()
                )
            })?;
        self.project_cache
            .lock()
            .unwrap()
            .insert(manifest_path, project.clone());
        self.record_cache_store(AnalysisCacheTraceKind::ProjectResolution);
        Ok(project)
    }

    fn infer_standalone_analysis_root(&self, path: &Path) -> PathBuf {
        self.infer_standalone_analysis_root_with(path, |candidate| {
            self.analysis_path_exists(candidate)
        })
    }

    fn infer_standalone_analysis_root_for_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        path: &Path,
    ) -> PathBuf {
        self.infer_standalone_analysis_root_with(path, |candidate| {
            snapshot.analysis_path_exists(candidate)
        })
    }

    fn infer_standalone_analysis_root_with(
        &self,
        path: &Path,
        mut path_exists: impl FnMut(&Path) -> bool,
    ) -> PathBuf {
        let normalized = normalize_path(path);
        let start = normalized.parent().unwrap_or_else(|| Path::new("."));

        for ancestor in start.ancestors() {
            let candidate = ancestor.join("mod.kn");
            if path_exists(&candidate) {
                return normalize_path(&candidate);
            }
        }

        normalized
    }

    fn analysis_path_exists(&self, path: &Path) -> bool {
        let normalized = normalize_path(path);
        self.open_uri_by_normalized_path().contains_key(&normalized) || path.is_file()
    }

    fn retain_publishable_bundles(
        &self,
        target_uri: &str,
        bundles_by_uri: &mut BTreeMap<String, Vec<IdeDiagnostic>>,
    ) {
        let Some(target_doc) = self.documents.get(target_uri) else {
            return;
        };
        let target_path = normalize_path(&target_doc.path);
        let workspace_root = self
            .project_for_path(&target_doc.path)
            .ok()
            .flatten()
            .map(|project| normalize_path(project.workspace_root()));
        let open_uri_by_path = self.open_uri_by_normalized_path();

        bundles_by_uri.retain(|uri, _| {
            if uri == target_uri {
                return true;
            }
            let Some(path) = uri_to_analysis_path(uri) else {
                return false;
            };
            let normalized = normalize_path(&path);
            normalized == target_path
                || open_uri_by_path.contains_key(&normalized)
                || workspace_root
                    .as_ref()
                    .is_some_and(|root| normalized.starts_with(root))
        });
    }

    fn invalidate_artifact_cache(&self) {
        self.parse_cache.lock().unwrap().clear();
        self.surface_cache.lock().unwrap().clear();
        self.structure_cache.lock().unwrap().clear();
        self.artifact_cache.lock().unwrap().clear();
        self.navigation_cache.lock().unwrap().clear();
        self.semantic_classification_cache.lock().unwrap().clear();
        self.invalidate_workspace_index();
    }

    fn invalidate_dirty_document_snapshot(&self) {
        self.dirty_documents_snapshot.lock().unwrap().take();
    }

    fn invalidate_open_path_index(&self) {
        self.open_uri_by_path.lock().unwrap().take();
    }

    fn invalidate_render_caches(&self) {
        self.semantic_tokens_cache.lock().unwrap().clear();
        self.lexical_cache.lock().unwrap().clear();
    }

    fn invalidate_render_caches_for_document(&self, uri: &str, path: &Path) {
        let target_path = normalize_path(path);
        self.semantic_tokens_cache
            .lock()
            .unwrap()
            .retain(|key, _| key.target_path != target_path);
        self.lexical_cache
            .lock()
            .unwrap()
            .retain(|key, _| key.uri != uri);
    }

    fn retain_semantic_tokens_for_document_text(&self, path: &Path, text_hash: u64) {
        let target_path = normalize_path(path);
        self.semantic_tokens_cache
            .lock()
            .unwrap()
            .retain(|key, _| key.target_path != target_path || key.text_hash == text_hash);
    }

    fn invalidate_lexical_cache_for_document(&self, uri: &str) {
        self.lexical_cache
            .lock()
            .unwrap()
            .retain(|key, _| key.uri != uri);
    }

    fn lexical_index_for_document(&self, uri: &str, document: &OpenDocument) -> Arc<LexicalIndex> {
        let key = LexicalCacheKey {
            uri: uri.to_string(),
            document_version: document.version,
            text_hash: document.text_hash,
        };
        if let Some(index) = self.lexical_cache.lock().unwrap().get(&key) {
            self.record_cache_hit(AnalysisCacheTraceKind::LexicalIndex);
            return Arc::clone(index);
        }
        self.record_cache_miss(AnalysisCacheTraceKind::LexicalIndex);

        let index = Arc::new(LexicalIndex::new(&document.text));
        self.lexical_cache
            .lock()
            .unwrap()
            .insert(key, Arc::clone(&index));
        self.record_cache_store(AnalysisCacheTraceKind::LexicalIndex);
        index
    }

    fn prune_cache_family_for_insert(&self, keep: &AnalysisCacheKey) {
        let family = keep.family();
        self.parse_cache
            .lock()
            .unwrap()
            .retain(|key, _| key.family() != family || key == keep || key.is_clean());
        self.surface_cache
            .lock()
            .unwrap()
            .retain(|key, _| key.family() != family || key == keep || key.is_clean());
        self.structure_cache
            .lock()
            .unwrap()
            .retain(|key, _| key.family() != family || key == keep || key.is_clean());
        self.artifact_cache
            .lock()
            .unwrap()
            .retain(|key, _| key.family() != family || key == keep || key.is_clean());
        self.navigation_cache
            .lock()
            .unwrap()
            .retain(|key, _| key.family() != family || key == keep || key.is_clean());
        self.semantic_classification_cache
            .lock()
            .unwrap()
            .retain(|key, _| key.family() != family || key == keep || key.is_clean());
        let mut workspace_index = self.workspace_index.lock().unwrap();
        workspace_index
            .targets
            .retain(|key, _| key.family() != family || key == keep || key.is_clean());
        workspace_index
            .symbol_indexes
            .retain(|key, _| key.family() != family || key == keep || key.is_clean());
    }

    fn invalidate_workspace_index(&self) {
        let mut index = self.workspace_index.lock().unwrap();
        index.generation = index.generation.saturating_add(1);
        index.symbol_indexes.clear();
        index.targets.clear();
        index.last_refresh = None;
    }

    fn finish_workspace_index_refresh(
        &self,
        indexed_targets: usize,
        failed_targets: usize,
    ) -> WorkspaceIndexStats {
        let mut index = self.workspace_index.lock().unwrap();
        index.generation = index.generation.saturating_add(1);
        let stats = WorkspaceIndexStats {
            generation: index.generation,
            indexed_targets,
            failed_targets,
        };
        index.last_refresh = Some(stats);
        stats
    }

    #[cfg(test)]
    fn cached_driver_count(&self) -> usize {
        self.driver_cache.lock().unwrap().len()
    }

    #[cfg(test)]
    fn cached_project_count(&self) -> usize {
        self.project_cache.lock().unwrap().len()
    }

    #[cfg(test)]
    pub(crate) fn cached_workspace_symbol_index_count(&self) -> usize {
        self.workspace_index.lock().unwrap().symbol_indexes.len()
    }

    #[cfg(test)]
    pub(crate) fn cached_workspace_index_target_count(&self) -> usize {
        self.workspace_index.lock().unwrap().targets.len()
    }

    #[cfg(test)]
    fn cached_workspace_index_targets(&self) -> Vec<WorkspaceIndexTarget> {
        self.workspace_index
            .lock()
            .unwrap()
            .targets
            .values()
            .cloned()
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn cached_document_symbol_index_count(&self) -> usize {
        self.workspace_index.lock().unwrap().symbol_indexes.len()
    }

    fn document_differs_from_disk(path: &Path, text: &str) -> bool {
        match fs::read_to_string(path) {
            Ok(on_disk) => on_disk != text,
            Err(_) => true,
        }
    }
}

fn span_in_path(session: &Session, span: Span, target_path: &Path) -> bool {
    session
        .source_manager
        .get_file_path(span.file)
        .map(|path| normalize_path(path) == target_path)
        .unwrap_or(false)
}
