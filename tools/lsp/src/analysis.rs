mod cache;
mod code_actions;
mod completion;
mod diagnostics;
mod navigation;
mod semantic;
#[cfg(test)]
mod tests;
mod text;

use self::cache::{
    AnalysisCacheKey, DirtyDocumentsSnapshot, SemanticTokensCacheKey, hash_source_text,
};
use self::code_actions::{quick_fix_for_diagnostic, ranges_overlap, workspace_edit_key};
use self::completion::{completion_sort_key, keyword_completion_item};
pub use self::diagnostics::cleared_uris;
use self::diagnostics::{
    convert_diagnostic_for_document, diagnostics_from_session, preserve_target_diagnostics,
};
use self::navigation::{
    analysis_completion_to_lsp_item, analysis_signature_help_to_lsp_help,
    analysis_symbol_to_document_symbol, build_rename_changes, find_definition_location,
    find_document_highlights, find_hover, find_reference_locations, find_rename_target,
};
use self::text::{
    apply_content_change, byte_offset_to_position, completion_context, completion_is_member_access,
    completion_prefix, file_path_to_uri, has_following_call_paren, is_valid_identifier,
    keyword_completion_labels, match_position_in_file, normalize_path, position_to_byte_offset,
    single_server_diagnostic, span_contains_offset, span_to_range, trim_line_ending,
    uri_to_file_path,
};
use crate::defaults::default_analysis_compile_options;
use crate::protocol::{
    CodeAction, CompletionItem, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentHighlight, DocumentSymbol, Hover, Location, Position,
    PrepareRenameResult, Range, SemanticTokens, SignatureHelp, TextDocumentContentChangeEvent,
    WorkspaceEdit,
};
use craft::project::{AnalysisProject, ResolvedAnalysis, resolve_project_manifest_path};
use kernc_driver::{
    AnalysisArtifact, AnalysisSurfaceArtifact, CompilerDriver, IncrementalDriverKey,
    ParsedModuleArtifact, SourceOverrides, StructureArtifact,
};
use kernc_utils::config::{
    CompileOptions, inject_default_library_aliases, inject_driver_condition_defines,
};
use kernc_utils::{Session, SourceFile, Span};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
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
    ScheduleTarget(String),
    Immediate(AnalysisOutcome),
}

#[derive(Debug, Clone)]
struct RenameTarget {
    query_span: Span,
    definition_span: Span,
    placeholder: String,
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
        }
    }

    #[cfg(test)]
    pub fn open_document(&mut self, params: DidOpenTextDocumentParams) -> AnalysisOutcome {
        match self.open_document_state(params) {
            DocumentSyncAction::ScheduleTarget(uri) => self.analyze_document(&uri),
            DocumentSyncAction::Immediate(outcome) => outcome,
        }
    }

    pub fn open_document_state(&mut self, params: DidOpenTextDocumentParams) -> DocumentSyncAction {
        let doc = params.text_document;
        let uri = doc.uri.clone();
        let Some(path) = uri_to_file_path(&uri) else {
            return DocumentSyncAction::Immediate(single_server_diagnostic(
                uri,
                "only file:// URIs are supported",
            ));
        };

        self.documents.insert(
            uri.clone(),
            OpenDocument {
                is_dirty: Self::document_differs_from_disk(&path, &doc.text),
                text_hash: hash_source_text(&doc.text),
                path,
                version: doc.version,
                text: doc.text,
            },
        );
        self.invalidate_dirty_document_snapshot();
        self.invalidate_render_caches();

        DocumentSyncAction::ScheduleTarget(uri)
    }

    #[cfg(test)]
    pub fn change_document(&mut self, params: DidChangeTextDocumentParams) -> AnalysisOutcome {
        match self.change_document_state(params) {
            DocumentSyncAction::ScheduleTarget(uri) => self.analyze_document(&uri),
            DocumentSyncAction::Immediate(outcome) => outcome,
        }
    }

    pub fn change_document_state(
        &mut self,
        params: DidChangeTextDocumentParams,
    ) -> DocumentSyncAction {
        let Some(doc) = self.documents.get_mut(&params.text_document.uri) else {
            return DocumentSyncAction::Immediate(single_server_diagnostic(
                params.text_document.uri,
                "received didChange for a document that is not open",
            ));
        };

        let mut updated_text = doc.text.clone();
        for change in params.content_changes {
            if let Err(message) = apply_content_change(&doc.path, &mut updated_text, &change) {
                return DocumentSyncAction::Immediate(single_server_diagnostic(
                    params.text_document.uri.clone(),
                    message,
                ));
            }
        }

        doc.text = updated_text;
        doc.version = params.text_document.version;
        doc.is_dirty = Self::document_differs_from_disk(&doc.path, &doc.text);
        doc.text_hash = hash_source_text(&doc.text);
        self.invalidate_dirty_document_snapshot();
        self.invalidate_render_caches();

        DocumentSyncAction::ScheduleTarget(params.text_document.uri)
    }

    #[cfg(test)]
    pub fn close_document(&mut self, params: DidCloseTextDocumentParams) -> AnalysisOutcome {
        match self.close_document_state(params) {
            DocumentSyncAction::ScheduleTarget(uri) => self.analyze_document(&uri),
            DocumentSyncAction::Immediate(outcome) => outcome,
        }
    }

    pub fn close_document_state(
        &mut self,
        params: DidCloseTextDocumentParams,
    ) -> DocumentSyncAction {
        let _was_dirty = self
            .documents
            .remove(&params.text_document.uri)
            .map(|doc| doc.is_dirty)
            .unwrap_or(false);
        self.invalidate_dirty_document_snapshot();
        self.invalidate_render_caches();
        DocumentSyncAction::Immediate(AnalysisOutcome {
            bundles: vec![DiagnosticBundle {
                uri: params.text_document.uri,
                diagnostics: Vec::new(),
            }],
        })
    }

    pub fn refresh_workspace(&mut self) -> Vec<(String, AnalysisOutcome)> {
        self.project_cache.get_mut().clear();
        self.driver_cache.get_mut().clear();
        self.invalidate_artifact_cache();
        self.invalidate_render_caches();
        self.documents
            .keys()
            .cloned()
            .map(|uri| {
                let outcome = self.analyze_document(&uri);
                (uri, outcome)
            })
            .collect()
    }

    pub fn document_uris(&self) -> Vec<String> {
        self.documents.keys().cloned().collect()
    }

    pub fn analyze_document_uri(&self, target_uri: &str) -> AnalysisOutcome {
        self.analyze_document(target_uri)
    }

    pub fn document_symbols(&self, uri: &str) -> Result<Vec<DocumentSymbol>, String> {
        let surface = match self.analyze_surface_artifact(uri) {
            Ok(surface) => surface,
            Err(_) => return Ok(Vec::new()),
        };
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested document symbols for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);

        let mut symbols = Vec::new();
        for module_symbol in &surface.symbols {
            let Some(path) = surface
                .session
                .source_manager
                .get_file_path(module_symbol.span.file)
            else {
                continue;
            };
            if normalize_path(path) == target_path {
                symbols.extend(
                    module_symbol
                        .children
                        .iter()
                        .map(|symbol| analysis_symbol_to_document_symbol(&surface.session, symbol)),
                );
            }
        }

        Ok(symbols)
    }

    pub fn goto_definition(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<Option<Location>, String> {
        let artifact = match self.analyze_artifact(uri) {
            Ok(artifact) => artifact,
            Err(_) => return Ok(None),
        };
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested definition for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let uri_by_path = self.uri_by_normalized_path();

        Ok(find_definition_location(
            &artifact.session,
            &artifact.semantic_entries,
            &target_path,
            &position,
            &uri_by_path,
        ))
    }

    pub fn references(
        &self,
        uri: &str,
        position: Position,
        include_declaration: bool,
    ) -> Result<Vec<Location>, String> {
        let artifact = match self.analyze_artifact(uri) {
            Ok(artifact) => artifact,
            Err(_) => return Ok(Vec::new()),
        };
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested references for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let uri_by_path = self.uri_by_normalized_path();

        Ok(find_reference_locations(
            &artifact.session,
            &artifact.semantic_entries,
            &target_path,
            &position,
            include_declaration,
            &uri_by_path,
        ))
    }

    pub fn document_highlights(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<Vec<DocumentHighlight>, String> {
        let artifact = match self.analyze_artifact(uri) {
            Ok(artifact) => artifact,
            Err(_) => return Ok(Vec::new()),
        };
        let Some(target_doc) = self.documents.get(uri) else {
            return Err(
                "requested document highlights for a document that is not open".to_string(),
            );
        };
        let target_path = normalize_path(&target_doc.path);

        Ok(find_document_highlights(
            &artifact.session,
            &artifact.semantic_entries,
            &artifact.hovers,
            &target_path,
            &position,
        ))
    }

    pub fn hover(&self, uri: &str, position: Position) -> Result<Option<Hover>, String> {
        let artifact = match self.analyze_artifact(uri) {
            Ok(artifact) => artifact,
            Err(_) => return Ok(None),
        };
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested hover for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);

        Ok(find_hover(
            &artifact.session,
            &artifact.hovers,
            &artifact.semantic_entries,
            &target_path,
            &position,
        ))
    }

    pub fn signature_help(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<Option<SignatureHelp>, String> {
        let artifact = match self.analyze_artifact(uri) {
            Ok(artifact) => artifact,
            Err(_) => return Ok(None),
        };
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested signature help for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let file = kernc_utils::SourceFile::new(target_doc.path.clone(), target_doc.text.clone());
        let Some(offset) = position_to_byte_offset(&file, &position) else {
            return Ok(None);
        };

        Ok(artifact
            .signature_help(&target_path, offset)
            .map(analysis_signature_help_to_lsp_help))
    }

    pub fn completion(&self, uri: &str, position: Position) -> Result<Vec<CompletionItem>, String> {
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested completion for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let file = kernc_utils::SourceFile::new(target_doc.path.clone(), target_doc.text.clone());
        let Some(offset) = position_to_byte_offset(&file, &position) else {
            return Ok(Vec::new());
        };
        let prefix = completion_prefix(&target_doc.text, offset);
        let has_call_paren = has_following_call_paren(&target_doc.text, offset);
        let context = completion_context(&target_doc.text, offset);
        let member_access = completion_is_member_access(&target_doc.text, offset);

        let mut items = if let Ok(surface) = self.analyze_surface_artifact(uri) {
            if !surface.requires_body_completion(&target_path, offset) {
                surface.completion_items(&target_path, offset)
            } else {
                match self.analyze_artifact(uri) {
                    Ok(artifact) => artifact.completion_items(&target_path, offset),
                    Err(_) => Vec::new(),
                }
            }
        } else {
            match self.analyze_artifact(uri) {
                Ok(artifact) => artifact.completion_items(&target_path, offset),
                Err(_) => Vec::new(),
            }
        };
        if !prefix.is_empty() {
            items.retain(|item| item.label.starts_with(prefix));
        }
        if has_call_paren {
            for item in &mut items {
                if item.insert_text.is_some() {
                    item.insert_text = None;
                }
            }
        }
        items.sort_by(|left, right| {
            completion_sort_key(left, prefix, context)
                .cmp(&completion_sort_key(right, prefix, context))
        });

        let mut completions = items
            .into_iter()
            .map(analysis_completion_to_lsp_item)
            .collect::<Vec<_>>();
        let mut seen_labels = completions
            .iter()
            .map(|item| item.label.clone())
            .collect::<BTreeSet<_>>();
        for keyword in keyword_completion_labels(prefix, context, member_access) {
            if seen_labels.insert(keyword.to_string()) {
                completions.push(keyword_completion_item(keyword));
            }
        }

        Ok(completions)
    }

    pub fn prepare_rename(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<Option<PrepareRenameResult>, String> {
        let artifact = match self.analyze_artifact(uri) {
            Ok(artifact) => artifact,
            Err(_) => return Ok(None),
        };
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested prepareRename for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let Some(target) = find_rename_target(
            &artifact.session,
            &artifact.hovers,
            &artifact.semantic_entries,
            &target_path,
            &position,
        ) else {
            return Ok(None);
        };

        Ok(Some(PrepareRenameResult {
            range: span_to_range(&artifact.session, target.query_span),
            placeholder: target.placeholder,
        }))
    }

    pub fn rename(
        &self,
        uri: &str,
        position: Position,
        new_name: &str,
    ) -> Result<WorkspaceEdit, String> {
        if !is_valid_identifier(new_name) {
            return Err(format!("`{}` is not a valid Kern identifier", new_name));
        }

        let artifact = self
            .analyze_artifact(uri)
            .map_err(|message| format!("rename analysis failed: {message}"))?;
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested rename for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let Some(target) = find_rename_target(
            &artifact.session,
            &artifact.hovers,
            &artifact.semantic_entries,
            &target_path,
            &position,
        ) else {
            return Err("rename target is not a supported identifier".to_string());
        };
        let uri_by_path = self.uri_by_normalized_path();

        let changes = build_rename_changes(
            &artifact.session,
            &artifact.semantic_entries,
            target.definition_span,
            new_name,
            &uri_by_path,
        );

        Ok(WorkspaceEdit { changes })
    }

    pub fn semantic_tokens(&self, uri: &str) -> Result<SemanticTokens, String> {
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested semantic tokens for a document that is not open".to_string());
        };
        let file = kernc_utils::SourceFile::new(target_doc.path.clone(), target_doc.text.clone());
        if target_doc.is_dirty {
            // Semantic tokens are requested frequently while the user types.
            // Prefer a cheap lexical pass for dirty buffers so highlighting
            // stays responsive even when compiler analysis would be transient.
            return Ok(semantic::lexical_semantic_tokens(&file));
        }

        let resolved = self.resolve_analysis(uri)?;
        let dirty_documents = self.dirty_documents_snapshot();
        let analysis_key =
            AnalysisCacheKey::from_resolved_dirty_snapshot(&resolved, &dirty_documents);
        let target_path = normalize_path(&target_doc.path);
        let token_key = SemanticTokensCacheKey {
            analysis: analysis_key,
            target_path: target_path.clone(),
            document_version: target_doc.version,
        };
        if let Some(tokens) = self.semantic_tokens_cache.borrow().get(&token_key) {
            return Ok(tokens.clone());
        }

        let artifact = match self.analyze_artifact(uri) {
            Ok(artifact) => artifact,
            Err(_) => return Ok(semantic::lexical_semantic_tokens(&file)),
        };
        let tokens = semantic::semantic_tokens(&artifact, &file, &target_path);
        self.semantic_tokens_cache
            .borrow_mut()
            .insert(token_key, tokens.clone());
        Ok(tokens)
    }

    pub fn code_actions(&self, uri: &str, range: Range) -> Result<Vec<CodeAction>, String> {
        let artifact = match self.analyze_artifact(uri) {
            Ok(artifact) => artifact,
            Err(_) => return Ok(Vec::new()),
        };
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested code actions for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);

        let mut actions = Vec::new();
        let mut seen = BTreeSet::new();
        for diagnostic in &artifact.session.diagnostics {
            let Some(path) = artifact
                .session
                .source_manager
                .get_file_path(diagnostic.primary_span.file)
            else {
                continue;
            };
            if normalize_path(path) != target_path {
                continue;
            }

            let lsp_diagnostic =
                convert_diagnostic_for_document(&artifact.session, diagnostic, target_doc);
            if !ranges_overlap(&lsp_diagnostic.range, &range) {
                continue;
            }

            let Some(action) =
                quick_fix_for_diagnostic(uri, &artifact, diagnostic, lsp_diagnostic.clone())
            else {
                continue;
            };

            let edit_key = action
                .edit
                .as_ref()
                .map(workspace_edit_key)
                .unwrap_or_default();
            let dedup_key = (action.title.clone(), edit_key);
            if seen.insert(dedup_key) {
                actions.push(action);
            }
        }

        Ok(actions)
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

        let Ok(artifact) = self.analyze_artifact(target_uri) else {
            return single_server_diagnostic(
                target_uri.to_string(),
                "received analysis request for a document that is not open",
            );
        };

        let mut bundles_by_uri = diagnostics_from_session(&artifact.session, &self.documents);
        bundles_by_uri.entry(target_uri.to_string()).or_default();
        self.retain_publishable_bundles(target_uri, &mut bundles_by_uri);

        AnalysisOutcome {
            bundles: bundles_by_uri
                .into_iter()
                .map(|(uri, diagnostics)| DiagnosticBundle { uri, diagnostics })
                .collect(),
        }
    }

    fn analyze_targeted_dirty_outcome(
        &self,
        target_uri: &str,
    ) -> Result<Option<AnalysisOutcome>, String> {
        let resolved = self.resolve_analysis(target_uri)?;
        let dirty_documents = self.dirty_documents_snapshot();
        if dirty_documents.len() != 1 {
            return Ok(None);
        }

        let clean_key = AnalysisCacheKey::clean(&resolved);
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

        let (parsed, driver) = self.parse_modules(target_uri)?;
        let Some(report) = driver.analyze_report_with_function_body_reuse(
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
        let resolved = self.resolve_analysis(target_uri)?;
        let dirty_documents = self.dirty_documents_snapshot();
        if dirty_documents.is_clean() {
            return Ok(None);
        }

        let clean_key = AnalysisCacheKey::clean(&resolved);
        let Some(clean_structure) = self.structure_cache.borrow().get(&clean_key).cloned() else {
            return Ok(None);
        };

        let (parsed, driver) = self.parse_modules(target_uri)?;
        Ok(driver
            .analyze_report_from_structure_and_parsed(&clean_structure, &parsed)
            .filter(|_| !dirty_documents.is_clean()))
    }

    #[cfg(test)]
    fn source_overrides(&self) -> SourceOverrides {
        self.dirty_documents_snapshot().overrides.clone()
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

    fn uri_by_normalized_path(&self) -> BTreeMap<PathBuf, String> {
        self.documents
            .iter()
            .map(|(uri, doc)| (normalize_path(&doc.path), uri.clone()))
            .collect()
    }

    fn analyze_artifact(&self, target_uri: &str) -> Result<Rc<AnalysisArtifact>, String> {
        let resolved = self.resolve_analysis(target_uri)?;
        let dirty_documents = self
            .dirty_documents_snapshot()
            .remap_for(&resolved.source_path_aliases);
        let cache_key = AnalysisCacheKey::from_resolved_dirty_snapshot(&resolved, &dirty_documents);
        if let Some(artifact) = self.artifact_cache.borrow().get(&cache_key) {
            return Ok(Rc::clone(artifact));
        }

        let driver = self.driver_for_resolved(&resolved);
        let structure = if let Some(structure) = self.structure_cache.borrow().get(&cache_key) {
            Some(Rc::clone(structure))
        } else {
            driver
                .analyze_structure(
                    &resolved.input_file.to_string_lossy(),
                    &dirty_documents.overrides,
                )
                .map(Rc::new)
        };
        self.prune_cache_family_for_insert(&cache_key);
        if let Some(structure) = &structure {
            self.structure_cache
                .borrow_mut()
                .insert(cache_key.clone(), Rc::clone(structure));
        }

        let artifact = Rc::new(if let Some(structure) = structure {
            driver.analyze_artifact_from_structure(&structure)
        } else {
            driver.analyze_artifact(
                &resolved.input_file.to_string_lossy(),
                &dirty_documents.overrides,
            )
        });
        self.artifact_cache
            .borrow_mut()
            .insert(cache_key, Rc::clone(&artifact));
        Ok(artifact)
    }

    fn analyze_surface_artifact(
        &self,
        target_uri: &str,
    ) -> Result<Rc<AnalysisSurfaceArtifact>, String> {
        let resolved = self.resolve_analysis(target_uri)?;
        let dirty_documents = self
            .dirty_documents_snapshot()
            .remap_for(&resolved.source_path_aliases);
        let cache_key = AnalysisCacheKey::from_resolved_dirty_snapshot(&resolved, &dirty_documents);
        if let Some(surface) = self.surface_cache.borrow().get(&cache_key) {
            return Ok(Rc::clone(surface));
        }

        let driver = self.driver_for_resolved(&resolved);
        let Some(surface) = driver
            .analyze_surface(
                &resolved.input_file.to_string_lossy(),
                &dirty_documents.overrides,
            )
            .map(Rc::new)
        else {
            return Err("surface analysis failed".to_string());
        };
        self.prune_cache_family_for_insert(&cache_key);
        self.surface_cache
            .borrow_mut()
            .insert(cache_key, Rc::clone(&surface));
        Ok(surface)
    }

    fn parse_modules(
        &self,
        target_uri: &str,
    ) -> Result<(Rc<ParsedModuleArtifact>, Rc<CompilerDriver>), String> {
        let resolved = self.resolve_analysis(target_uri)?;
        let dirty_documents = self
            .dirty_documents_snapshot()
            .remap_for(&resolved.source_path_aliases);
        let cache_key = AnalysisCacheKey::from_resolved_dirty_snapshot(&resolved, &dirty_documents);
        let driver = self.driver_for_resolved(&resolved);

        if let Some(parsed) = self.parse_cache.borrow().get(&cache_key) {
            return Ok((Rc::clone(parsed), driver));
        }

        let Some(parsed) = driver
            .parse_modules(
                &resolved.input_file.to_string_lossy(),
                &dirty_documents.overrides,
            )
            .map(Rc::new)
        else {
            return Err("parse analysis failed".to_string());
        };
        self.prune_cache_family_for_insert(&cache_key);
        self.parse_cache
            .borrow_mut()
            .insert(cache_key, Rc::clone(&parsed));
        Ok((parsed, driver))
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
            inject_default_library_aliases(&mut resolved.compile_options);
            inject_driver_condition_defines(&mut resolved.compile_options);
            return Ok(resolved);
        }

        let mut compile_options = self.settings.compile_options.clone();
        inject_default_library_aliases(&mut compile_options);
        inject_driver_condition_defines(&mut compile_options);
        Ok(ResolvedAnalysis {
            input_file: self.infer_standalone_analysis_root(&target_doc.path),
            compile_options,
            source_path_aliases: BTreeMap::new(),
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
        self.documents
            .values()
            .any(|doc| normalize_path(&doc.path) == normalized)
            || path.is_file()
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
        let open_paths = self
            .documents
            .values()
            .map(|doc| normalize_path(&doc.path))
            .collect::<BTreeSet<_>>();

        bundles_by_uri.retain(|uri, _| {
            if uri == target_uri {
                return true;
            }
            let Some(path) = uri_to_file_path(uri) else {
                return false;
            };
            let normalized = normalize_path(&path);
            normalized == target_path
                || open_paths.contains(&normalized)
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
