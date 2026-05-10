mod cache;
mod code_actions;
mod completion;
mod diagnostics;
mod documents;
mod navigation;
mod queries;
mod semantic;
#[cfg(test)]
mod tests;
mod text;

use self::cache::{
    AnalysisCacheKey, DirtyDocumentsSnapshot, SemanticTokensCacheKey, hash_source_text,
};
use self::code_actions::{
    lightweight_quick_fix_for_diagnostic, quick_fix_for_diagnostic, ranges_overlap,
    workspace_edit_key,
};
use self::completion::{completion_sort_key, keyword_completion_item};
pub use self::diagnostics::cleared_uris;
use self::diagnostics::{
    convert_diagnostic_for_document, diagnostics_from_session, preserve_target_diagnostics,
};
use self::navigation::{
    ReferenceLocationQuery, analysis_completion_to_lsp_item, analysis_signature_help_to_lsp_help,
    analysis_symbol_to_document_symbol, build_rename_changes, find_definition_location,
    find_document_highlights, find_hover, find_reference_locations, find_rename_target,
};
pub(crate) use self::text::single_server_diagnostic;
#[cfg(test)]
pub(crate) use self::text::uri_to_file_path;
use self::text::{
    apply_content_change, byte_offset_to_position, completion_context,
    completion_is_binding_name_context, completion_is_in_comment_or_literal,
    completion_is_member_access, completion_member_access_has_receiver, completion_prefix,
    fallback_keyword_completion_labels, file_path_to_uri, has_following_call_paren,
    is_valid_identifier, keyword_completion_labels, match_position_in_file, normalize_path,
    position_to_byte_offset, span_contains_offset, span_to_range, trim_line_ending,
    uri_to_analysis_path,
};
use crate::defaults::default_analysis_compile_options;
use crate::protocol::{
    CodeAction, CompletionItem, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentHighlight, DocumentSymbol, Hover, Location, Position,
    PrepareRenameResult, Range, SemanticTokens, SignatureHelp, TextDocumentContentChangeEvent,
    WorkspaceEdit,
};
use crate::server::DiagnosticsAnalysisMode;
use craft::project::{AnalysisProject, ResolvedAnalysis, resolve_project_manifest_path};
use kernc_driver::{
    AnalysisArtifact, AnalysisReport, AnalysisSurfaceArtifact, CompilerDriver,
    IncrementalDriverKey, ParsedModuleArtifact, SourceOverrides, StructureArtifact,
};
use kernc_utils::config::{
    CompileOptions, apply_configured_library_aliases, inject_driver_condition_defines,
};
use kernc_utils::{Session, SourceFile, Span};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
pub struct DiagnosticBundle {
    pub uri: String,
    pub diagnostics: Vec<crate::protocol::Diagnostic>,
}

pub struct AnalysisOutcome {
    pub bundles: Vec<DiagnosticBundle>,
}

pub enum DocumentSyncAction {
    ScheduleTarget {
        uri: String,
        mode: DiagnosticsAnalysisMode,
    },
    Immediate(AnalysisOutcome),
}

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
    driver: Rc<CompilerDriver>,
}

pub struct AnalysisEngine {
    documents: BTreeMap<String, OpenDocument>,
    settings: AnalysisSettings,
    project_cache: RefCell<BTreeMap<PathBuf, Option<AnalysisProject>>>,
    driver_cache: RefCell<BTreeMap<IncrementalDriverKey, Rc<CompilerDriver>>>,
    parse_cache: RefCell<BTreeMap<AnalysisCacheKey, Rc<ParsedModuleArtifact>>>,
    surface_cache: RefCell<BTreeMap<AnalysisCacheKey, Rc<AnalysisSurfaceArtifact>>>,
    structure_cache: RefCell<BTreeMap<AnalysisCacheKey, Rc<StructureArtifact>>>,
    artifact_cache: RefCell<BTreeMap<AnalysisCacheKey, Rc<AnalysisArtifact>>>,
    semantic_tokens_cache: RefCell<BTreeMap<SemanticTokensCacheKey, SemanticTokens>>,
    dirty_documents_snapshot: RefCell<Option<Rc<DirtyDocumentsSnapshot>>>,
    open_uri_by_path: RefCell<Option<Rc<BTreeMap<PathBuf, String>>>>,
    last_analysis_tier: RefCell<Option<AnalysisTier>>,
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
            project_cache: RefCell::new(BTreeMap::new()),
            driver_cache: RefCell::new(BTreeMap::new()),
            parse_cache: RefCell::new(BTreeMap::new()),
            surface_cache: RefCell::new(BTreeMap::new()),
            structure_cache: RefCell::new(BTreeMap::new()),
            artifact_cache: RefCell::new(BTreeMap::new()),
            semantic_tokens_cache: RefCell::new(BTreeMap::new()),
            dirty_documents_snapshot: RefCell::new(None),
            open_uri_by_path: RefCell::new(None),
            last_analysis_tier: RefCell::new(None),
        }
    }

    fn record_analysis_tier(&self, tier: AnalysisTier) {
        self.last_analysis_tier.borrow_mut().replace(tier);
    }

    pub(crate) fn clear_last_analysis_tier(&self) {
        self.last_analysis_tier.borrow_mut().take();
    }

    fn analyze_document(&self, target_uri: &str) -> AnalysisOutcome {
        if let Ok(Some(outcome)) = self.analyze_targeted_dirty_outcome(target_uri) {
            return outcome;
        }

        if let Ok(Some(report)) = self.analyze_dirty_report(target_uri) {
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

        let Ok(report) = self.analyze_diagnostic_report(target_uri) else {
            return single_server_diagnostic(
                target_uri.to_string(),
                "received analysis request for a document that is not open",
            );
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
        let Ok(session) = self.parse_open_document_session(target_uri) else {
            return single_server_diagnostic(
                target_uri.to_string(),
                "received analysis request for a document that is not open",
            );
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
        self.record_analysis_tier(AnalysisTier::ParseOnly);
        let target_doc = self
            .documents
            .get(target_uri)
            .ok_or_else(|| "document is not open".to_string())?;
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
        let Some(clean_structure) = self.structure_cache.borrow().get(&clean_key).cloned() else {
            return Ok(None);
        };
        let Some(clean_artifact) = self.artifact_cache.borrow().get(&clean_key).cloned() else {
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
            return Ok(None);
        }
        let mut bundles_by_uri = diagnostics_from_session(&clean_artifact.session, &self.documents);

        let parsed = self.parse_modules_for_context(&context)?;
        let Some(report) = context.driver.analyze_report_with_function_body_reuse(
            &clean_artifact,
            &clean_structure,
            &parsed,
        ) else {
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
        let Some(clean_structure) = self.structure_cache.borrow().get(&clean_key).cloned() else {
            return Ok(None);
        };

        let parsed = self.parse_modules_for_context(&context)?;
        let report = context
            .driver
            .analyze_report_from_structure_and_parsed(&clean_structure, &parsed)
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
        *self.last_analysis_tier.borrow()
    }

    fn dirty_documents_snapshot(&self) -> Rc<DirtyDocumentsSnapshot> {
        if let Some(snapshot) = self.dirty_documents_snapshot.borrow().as_ref() {
            return Rc::clone(snapshot);
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

        let snapshot = Rc::new(DirtyDocumentsSnapshot {
            overrides,
            hashed_overrides,
        });
        self.dirty_documents_snapshot
            .borrow_mut()
            .replace(Rc::clone(&snapshot));
        snapshot
    }

    fn open_uri_by_normalized_path(&self) -> Rc<BTreeMap<PathBuf, String>> {
        if let Some(uri_by_path) = self.open_uri_by_path.borrow().as_ref() {
            return Rc::clone(uri_by_path);
        }

        let uri_by_path = Rc::new(
            self.documents
                .iter()
                .map(|(uri, doc)| (normalize_path(&doc.path), uri.clone()))
                .collect(),
        );
        self.open_uri_by_path
            .borrow_mut()
            .replace(Rc::clone(&uri_by_path));
        uri_by_path
    }

    fn uri_by_normalized_path(&self) -> Rc<BTreeMap<PathBuf, String>> {
        self.open_uri_by_normalized_path()
    }

    fn analyze_diagnostic_report(&self, target_uri: &str) -> Result<AnalysisReport, String> {
        let context = self.resolve_analysis_context(target_uri)?;
        if let Some(artifact) = self.artifact_cache.borrow().get(&context.cache_key) {
            return Ok(AnalysisReport {
                session: artifact.session.clone(),
                succeeded: artifact.succeeded,
            });
        }

        let structure =
            if let Some(structure) = self.structure_cache.borrow().get(&context.cache_key) {
                Some(Rc::clone(structure))
            } else {
                context
                    .driver
                    .analyze_structure(
                        &context.resolved.input_file.to_string_lossy(),
                        &context.dirty_documents.overrides,
                    )
                    .map(Rc::new)
            };
        self.prune_cache_family_for_insert(&context.cache_key);
        if let Some(structure) = &structure {
            self.structure_cache
                .borrow_mut()
                .insert(context.cache_key.clone(), Rc::clone(structure));
        }

        Ok(if let Some(structure) = structure {
            context.driver.analyze_report_from_structure(&structure)
        } else {
            context.driver.analyze_report(
                &context.resolved.input_file.to_string_lossy(),
                &context.dirty_documents.overrides,
            )
        })
    }

    fn analyze_interactive_artifact(
        &self,
        target_uri: &str,
    ) -> Result<Rc<AnalysisArtifact>, String> {
        let context = self.resolve_analysis_context(target_uri)?;
        if context.dirty_documents.is_clean() {
            self.record_analysis_tier(AnalysisTier::CleanSemantic);
            return Ok(self.analyze_artifact_for_context(&context));
        }

        if !context.resolved.input_file.is_file() {
            self.record_analysis_tier(AnalysisTier::DirtySemantic);
            return Ok(self.analyze_artifact_for_context(&context));
        }

        self.record_analysis_tier(AnalysisTier::CleanSemantic);
        Ok(self.analyze_clean_artifact_for_context(&context))
    }

    fn analyze_artifact_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Rc<AnalysisArtifact> {
        if let Some(artifact) = self.artifact_cache.borrow().get(&context.cache_key) {
            return Rc::clone(artifact);
        }

        let structure =
            if let Some(structure) = self.structure_cache.borrow().get(&context.cache_key) {
                Some(Rc::clone(structure))
            } else {
                context
                    .driver
                    .analyze_structure(
                        &context.resolved.input_file.to_string_lossy(),
                        &context.dirty_documents.overrides,
                    )
                    .map(Rc::new)
            };
        self.prune_cache_family_for_insert(&context.cache_key);
        if let Some(structure) = &structure {
            self.structure_cache
                .borrow_mut()
                .insert(context.cache_key.clone(), Rc::clone(structure));
        }

        let artifact = Rc::new(if let Some(structure) = structure {
            context.driver.analyze_artifact_from_structure(&structure)
        } else {
            context.driver.analyze_artifact(
                &context.resolved.input_file.to_string_lossy(),
                &context.dirty_documents.overrides,
            )
        });
        self.artifact_cache
            .borrow_mut()
            .insert(context.cache_key.clone(), Rc::clone(&artifact));
        artifact
    }

    fn analyze_surface_artifact(
        &self,
        target_uri: &str,
    ) -> Result<Rc<AnalysisSurfaceArtifact>, String> {
        let context = self.resolve_analysis_context(target_uri)?;
        if let Some(surface) = self.surface_cache.borrow().get(&context.cache_key) {
            return Ok(Rc::clone(surface));
        }

        let Some(surface) = context
            .driver
            .analyze_surface(
                &context.resolved.input_file.to_string_lossy(),
                &context.dirty_documents.overrides,
            )
            .map(Rc::new)
        else {
            return Err("surface analysis failed".to_string());
        };
        self.prune_cache_family_for_insert(&context.cache_key);
        self.surface_cache
            .borrow_mut()
            .insert(context.cache_key.clone(), Rc::clone(&surface));
        Ok(surface)
    }

    fn analyze_clean_artifact_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Rc<AnalysisArtifact> {
        let clean_key = AnalysisCacheKey::clean(&context.resolved);
        if let Some(artifact) = self.artifact_cache.borrow().get(&clean_key) {
            return Rc::clone(artifact);
        }

        let artifact = Rc::new(context.driver.analyze_artifact(
            &context.resolved.input_file.to_string_lossy(),
            &SourceOverrides::new(),
        ));
        self.artifact_cache
            .borrow_mut()
            .insert(clean_key, Rc::clone(&artifact));
        artifact
    }

    fn analyze_clean_surface_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Option<Rc<AnalysisSurfaceArtifact>> {
        let clean_key = AnalysisCacheKey::clean(&context.resolved);
        if let Some(surface) = self.surface_cache.borrow().get(&clean_key) {
            return Some(Rc::clone(surface));
        }

        let surface = context
            .driver
            .analyze_surface(
                &context.resolved.input_file.to_string_lossy(),
                &SourceOverrides::new(),
            )
            .map(Rc::new)?;
        self.surface_cache
            .borrow_mut()
            .insert(clean_key, Rc::clone(&surface));
        Some(surface)
    }

    fn parse_modules_for_context(
        &self,
        context: &AnalysisRequestContext,
    ) -> Result<Rc<ParsedModuleArtifact>, String> {
        if let Some(parsed) = self.parse_cache.borrow().get(&context.cache_key) {
            return Ok(Rc::clone(parsed));
        }

        let Some(parsed) = context
            .driver
            .parse_modules(
                &context.resolved.input_file.to_string_lossy(),
                &context.dirty_documents.overrides,
            )
            .map(Rc::new)
        else {
            return Err("parse analysis failed".to_string());
        };
        self.prune_cache_family_for_insert(&context.cache_key);
        self.parse_cache
            .borrow_mut()
            .insert(context.cache_key.clone(), Rc::clone(&parsed));
        Ok(parsed)
    }

    fn resolve_analysis_context(&self, target_uri: &str) -> Result<AnalysisRequestContext, String> {
        let resolved = self.resolve_analysis(target_uri)?;
        let dirty_documents = self
            .dirty_documents_snapshot()
            .filter_for_resolved(&resolved)
            .remap_for(&resolved.source_path_aliases);
        let cache_key = AnalysisCacheKey::from_resolved_dirty_snapshot(&resolved, &dirty_documents);
        let driver = self.driver_for_resolved(&resolved);
        Ok(AnalysisRequestContext {
            resolved,
            dirty_documents,
            cache_key,
            driver,
        })
    }

    fn driver_for_resolved(&self, resolved: &ResolvedAnalysis) -> Rc<CompilerDriver> {
        let family = IncrementalDriverKey::from_options(&resolved.compile_options);
        if let Some(driver) = self.driver_cache.borrow().get(&family) {
            return Rc::clone(driver);
        }

        let driver = Rc::new(CompilerDriver::new(resolved.compile_options.clone()));
        self.driver_cache
            .borrow_mut()
            .insert(family, Rc::clone(&driver));
        driver
    }

    fn resolve_analysis(&self, target_uri: &str) -> Result<ResolvedAnalysis, String> {
        let Some(target_doc) = self.documents.get(target_uri) else {
            return Err("document is not open".to_string());
        };

        if let Some(project) = self.project_for_path(&target_doc.path) {
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
        })
    }

    fn project_for_path(&self, path: &Path) -> Option<AnalysisProject> {
        let start = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or_else(|| Path::new("."))
        };
        let manifest_path = resolve_project_manifest_path(Some(start)).ok()?;

        if let Some(project) = self.project_cache.borrow().get(&manifest_path) {
            return project.clone();
        }

        let project = AnalysisProject::load_from_manifest(&manifest_path).ok();
        self.project_cache
            .borrow_mut()
            .insert(manifest_path, project.clone());
        project
    }

    fn infer_standalone_analysis_root(&self, path: &Path) -> PathBuf {
        let normalized = normalize_path(path);
        let start = normalized.parent().unwrap_or_else(|| Path::new("."));

        for ancestor in start.ancestors() {
            let candidate = ancestor.join("init.rn");
            if self.analysis_path_exists(&candidate) {
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
        bundles_by_uri: &mut BTreeMap<String, Vec<crate::protocol::Diagnostic>>,
    ) {
        let Some(target_doc) = self.documents.get(target_uri) else {
            return;
        };
        let target_path = normalize_path(&target_doc.path);
        let workspace_root = self
            .project_for_path(&target_doc.path)
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
        self.parse_cache.borrow_mut().clear();
        self.surface_cache.borrow_mut().clear();
        self.structure_cache.borrow_mut().clear();
        self.artifact_cache.borrow_mut().clear();
    }

    fn invalidate_dirty_document_snapshot(&self) {
        self.dirty_documents_snapshot.borrow_mut().take();
    }

    fn invalidate_open_path_index(&self) {
        self.open_uri_by_path.borrow_mut().take();
    }

    fn invalidate_render_caches(&self) {
        self.semantic_tokens_cache.borrow_mut().clear();
    }

    fn prune_cache_family_for_insert(&self, keep: &AnalysisCacheKey) {
        let family = keep.family();
        self.parse_cache
            .borrow_mut()
            .retain(|key, _| key.family() != family || key == keep || key.is_clean());
        self.surface_cache
            .borrow_mut()
            .retain(|key, _| key.family() != family || key == keep || key.is_clean());
        self.structure_cache
            .borrow_mut()
            .retain(|key, _| key.family() != family || key == keep || key.is_clean());
        self.artifact_cache
            .borrow_mut()
            .retain(|key, _| key.family() != family || key == keep || key.is_clean());
    }

    #[cfg(test)]
    fn cached_driver_count(&self) -> usize {
        self.driver_cache.borrow().len()
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
