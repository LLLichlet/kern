mod code_actions;
mod diagnostics;
mod navigation;
mod semantic;
#[cfg(test)]
mod tests;
mod text;

use self::code_actions::{quick_fix_for_diagnostic, ranges_overlap, workspace_edit_key};
use self::diagnostics::{convert_diagnostic, diagnostics_from_session};
use self::navigation::{
    analysis_completion_to_lsp_item, analysis_signature_help_to_lsp_help,
    analysis_symbol_to_document_symbol, build_rename_changes, find_definition_location,
    find_document_highlights, find_hover, find_reference_locations, find_rename_target,
};
use self::text::{
    CompletionContext, apply_content_change, byte_offset_to_position, completion_context,
    completion_is_member_access, completion_prefix, file_path_to_uri, has_following_call_paren,
    is_valid_identifier, keyword_completion_labels, match_position_in_file, normalize_path,
    position_to_byte_offset, single_server_diagnostic, span_contains_offset, span_to_range,
    trim_line_ending, uri_to_file_path,
};
use crate::protocol::{
    CodeAction, CompletionItem, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentHighlight, DocumentSymbol, Hover, Location, Position,
    PrepareRenameResult, Range, SemanticTokens, SignatureHelp, TextDocumentContentChangeEvent,
    WorkspaceEdit,
};
use craft::project::{AnalysisProject, ResolvedAnalysis, resolve_project_manifest_path};
use kernc_driver::{
    AnalysisArtifact, AnalysisSurfaceArtifact, CompilerDriver, ParsedModuleArtifact,
    SourceOverrides, StructureArtifact, TargetedAnalysisReport,
};
use kernc_utils::config::{
    CompileOptions, inject_driver_condition_defines, maybe_inject_std_alias,
};
use kernc_utils::{Session, SourceFile, Span};
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Debug, Clone)]
pub struct AnalysisSettings {
    pub compile_options: CompileOptions,
}

impl Default for AnalysisSettings {
    fn default() -> Self {
        Self {
            compile_options: CompileOptions {
                use_std: true,
                ..CompileOptions::default()
            },
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

#[derive(Debug, Clone, Copy)]
struct OffsetReplacement {
    clean_start: usize,
    clean_end: usize,
    dirty_start: usize,
    dirty_end: usize,
}

#[derive(Debug, Clone, Default)]
struct DirtyDocumentsSnapshot {
    overrides: SourceOverrides,
    hashed_overrides: Vec<(PathBuf, u64)>,
}

impl DirtyDocumentsSnapshot {
    fn is_clean(&self) -> bool {
        self.hashed_overrides.is_empty()
    }

    fn len(&self) -> usize {
        self.hashed_overrides.len()
    }

    fn remap_for(&self, aliases: &BTreeMap<PathBuf, PathBuf>) -> Self {
        if aliases.is_empty() || self.overrides.is_empty() {
            return self.clone();
        }

        let mut overrides = self.overrides.clone();
        for (source_path, generated_path) in aliases {
            let normalized_source = normalize_path(source_path);
            let normalized_generated = normalize_path(generated_path);
            if overrides.contains_key(&normalized_generated) {
                continue;
            }
            let Some(source) = overrides.get(&normalized_source).cloned() else {
                continue;
            };
            overrides.insert(normalized_generated, source);
        }

        let mut hashed_overrides = overrides
            .iter()
            .map(|(path, text)| (normalize_path(path), hash_source_text(text)))
            .collect::<Vec<_>>();
        hashed_overrides.sort();

        Self {
            overrides,
            hashed_overrides,
        }
    }
}

pub struct AnalysisEngine {
    documents: BTreeMap<String, OpenDocument>,
    settings: AnalysisSettings,
    project_cache: RefCell<BTreeMap<PathBuf, Option<AnalysisProject>>>,
    parse_cache: RefCell<BTreeMap<AnalysisCacheKey, Rc<ParsedModuleArtifact>>>,
    surface_cache: RefCell<BTreeMap<AnalysisCacheKey, Rc<AnalysisSurfaceArtifact>>>,
    structure_cache: RefCell<BTreeMap<AnalysisCacheKey, Rc<StructureArtifact>>>,
    artifact_cache: RefCell<BTreeMap<AnalysisCacheKey, Rc<AnalysisArtifact>>>,
    semantic_tokens_cache: RefCell<BTreeMap<SemanticTokensCacheKey, SemanticTokens>>,
    dirty_documents_snapshot: RefCell<Option<Rc<DirtyDocumentsSnapshot>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct AnalysisCacheKey {
    input_file: PathBuf,
    root_module_name: Option<String>,
    target_triple: String,
    custom_defines: Vec<(String, String)>,
    module_aliases: Vec<(String, String)>,
    module_interface_aliases: Vec<(String, String)>,
    source_overrides: Vec<(PathBuf, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct AnalysisCacheFamilyKey {
    input_file: PathBuf,
    root_module_name: Option<String>,
    target_triple: String,
    custom_defines: Vec<(String, String)>,
    module_aliases: Vec<(String, String)>,
    module_interface_aliases: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SemanticTokensCacheKey {
    analysis: AnalysisCacheKey,
    target_path: PathBuf,
    document_version: i64,
}

impl AnalysisCacheKey {
    #[cfg(test)]
    fn from_resolved(resolved: &ResolvedAnalysis, source_overrides: &SourceOverrides) -> Self {
        let mut hashed_overrides = source_overrides
            .iter()
            .map(|(path, text)| (normalize_path(path), hash_source_text(text)))
            .collect::<Vec<_>>();
        hashed_overrides.sort();
        Self::from_resolved_hashed(resolved, hashed_overrides)
    }

    fn from_resolved_dirty_snapshot(
        resolved: &ResolvedAnalysis,
        dirty_documents: &DirtyDocumentsSnapshot,
    ) -> Self {
        Self::from_resolved_hashed(resolved, dirty_documents.hashed_overrides.clone())
    }

    fn clean(resolved: &ResolvedAnalysis) -> Self {
        Self::from_resolved_hashed(resolved, Vec::new())
    }

    fn from_resolved_hashed(
        resolved: &ResolvedAnalysis,
        source_overrides: Vec<(PathBuf, u64)>,
    ) -> Self {
        let mut custom_defines = resolved
            .compile_options
            .custom_defines
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        custom_defines.sort();

        let mut module_aliases = resolved
            .compile_options
            .module_aliases
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        module_aliases.sort();

        let mut module_interface_aliases = resolved
            .compile_options
            .module_interface_aliases
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        module_interface_aliases.sort();

        Self {
            input_file: normalize_path(&resolved.input_file),
            root_module_name: resolved.compile_options.root_module_name.clone(),
            target_triple: resolved.compile_options.target.triple.to_string(),
            custom_defines,
            module_aliases,
            module_interface_aliases,
            source_overrides,
        }
    }

    fn family(&self) -> AnalysisCacheFamilyKey {
        AnalysisCacheFamilyKey {
            input_file: self.input_file.clone(),
            root_module_name: self.root_module_name.clone(),
            target_triple: self.target_triple.clone(),
            custom_defines: self.custom_defines.clone(),
            module_aliases: self.module_aliases.clone(),
            module_interface_aliases: self.module_interface_aliases.clone(),
        }
    }

    fn is_clean(&self) -> bool {
        self.source_overrides.is_empty()
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
            project_cache: RefCell::new(BTreeMap::new()),
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

            let lsp_diagnostic = convert_diagnostic(&artifact.session, diagnostic);
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
            let target_doc = self
                .documents
                .get(target_uri)
                .ok_or_else(|| "document is not open".to_string())?;
            let target_path = normalize_path(&target_doc.path);
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

        let driver = CompilerDriver::new(resolved.compile_options);
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

        let driver = CompilerDriver::new(resolved.compile_options);
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
    ) -> Result<(Rc<ParsedModuleArtifact>, CompilerDriver), String> {
        let resolved = self.resolve_analysis(target_uri)?;
        let dirty_documents = self
            .dirty_documents_snapshot()
            .remap_for(&resolved.source_path_aliases);
        let cache_key = AnalysisCacheKey::from_resolved_dirty_snapshot(&resolved, &dirty_documents);
        let driver = CompilerDriver::new(resolved.compile_options);

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
        maybe_inject_std_alias(&mut compile_options);
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

    fn document_differs_from_disk(path: &Path, text: &str) -> bool {
        match fs::read_to_string(path) {
            Ok(on_disk) => on_disk != text,
            Err(_) => true,
        }
    }
}

fn completion_sort_key(
    item: &kernc_driver::AnalysisCompletionItem,
    prefix: &str,
    context: CompletionContext,
) -> (u8, u8, usize, String) {
    let exact = (!prefix.is_empty() && item.label == prefix) as u8;
    (
        completion_context_rank(item.kind, context),
        1_u8.saturating_sub(exact),
        item.label.len(),
        item.label.to_ascii_lowercase(),
    )
}

fn completion_context_rank(
    kind: kernc_driver::AnalysisCompletionKind,
    context: CompletionContext,
) -> u8 {
    match context {
        CompletionContext::Type => {
            (!matches!(
                kind,
                kernc_driver::AnalysisCompletionKind::Struct
                    | kernc_driver::AnalysisCompletionKind::Union
                    | kernc_driver::AnalysisCompletionKind::Enum
                    | kernc_driver::AnalysisCompletionKind::Trait
                    | kernc_driver::AnalysisCompletionKind::TypeAlias
                    | kernc_driver::AnalysisCompletionKind::TypeParameter
            )) as u8
        }
        CompletionContext::Value => {
            (!matches!(
                kind,
                kernc_driver::AnalysisCompletionKind::Variable
                    | kernc_driver::AnalysisCompletionKind::Function
                    | kernc_driver::AnalysisCompletionKind::Constant
                    | kernc_driver::AnalysisCompletionKind::Static
            )) as u8
        }
    }
}

fn keyword_completion_item(label: &str) -> CompletionItem {
    let insert_text = keyword_completion_insert_text(label);
    let insert_text_format = insert_text
        .as_deref()
        .map(|text| if text.contains('$') { 2 } else { 1 });

    CompletionItem {
        label: label.to_string(),
        kind: 14,
        detail: Some("keyword".to_string()),
        insert_text,
        insert_text_format,
    }
}

fn keyword_completion_insert_text(label: &str) -> Option<String> {
    match label {
        "extern" => Some("extern fn ${1:name}(${2:args}) ${3:i32} {\n    $0\n}".to_string()),
        "fn" => Some("fn ${1:name}(${2:args}) ${3:void} {\n    $0\n}".to_string()),
        "let" => Some("let ${1:name} = ${0};".to_string()),
        "const" => Some("const ${1:name}: ${2:Type} = ${0};".to_string()),
        "static" => Some("static ${1:name}: ${2:Type} = ${0};".to_string()),
        "type" => Some("type ${1:Name} = ${0};".to_string()),
        "if" => Some("if (${1:cond}) {\n    $0\n}".to_string()),
        "for" => Some("for (${1:item}) {\n    $0\n}".to_string()),
        "match" => Some("match (${1:value}) {\n    $0\n}".to_string()),
        "use" => Some("use ${1:path};".to_string()),
        "impl" => Some("impl ${1:Type} {\n    $0\n}".to_string()),
        "mod" => Some("mod ${1:name};".to_string()),
        "defer" => Some("defer {\n    $0\n}".to_string()),
        "struct" => Some("struct {\n    $0\n}".to_string()),
        "union" => Some("union {\n    $0\n}".to_string()),
        "enum" => Some("enum {\n    $0\n}".to_string()),
        "trait" => Some("trait {\n    $0\n}".to_string()),
        _ => None,
    }
}

fn preserve_target_diagnostics(
    clean_artifact: &AnalysisArtifact,
    clean_file: &SourceFile,
    dirty_file: &SourceFile,
    target_uri: &str,
    report: &TargetedAnalysisReport,
) -> Vec<crate::protocol::Diagnostic> {
    let target_path = normalize_path(&dirty_file.path);
    let mut replacements = report
        .replaced_spans
        .iter()
        .map(|replacement| OffsetReplacement {
            clean_start: replacement.clean.start,
            clean_end: replacement.clean.end,
            dirty_start: replacement.dirty.start,
            dirty_end: replacement.dirty.end,
        })
        .collect::<Vec<_>>();
    replacements.sort_by_key(|replacement| replacement.clean_start);

    clean_artifact
        .session
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            span_in_path(
                &clean_artifact.session,
                diagnostic.primary_span,
                &target_path,
            )
        })
        .filter_map(|diagnostic| {
            remap_clean_diagnostic(
                &clean_artifact.session,
                diagnostic,
                clean_file,
                dirty_file,
                target_uri,
                &target_path,
                &replacements,
            )
        })
        .collect()
}

fn remap_clean_diagnostic(
    session: &Session,
    diagnostic: &kernc_utils::Diagnostic,
    clean_file: &SourceFile,
    dirty_file: &SourceFile,
    target_uri: &str,
    target_path: &Path,
    replacements: &[OffsetReplacement],
) -> Option<crate::protocol::Diagnostic> {
    let mut converted = convert_diagnostic(session, diagnostic);
    converted.range = remap_span_to_range(
        clean_file,
        dirty_file,
        diagnostic.primary_span,
        replacements,
    )?;

    if let Some(related_information) = converted.related_information.as_mut() {
        for (related, (span, _)) in related_information
            .iter_mut()
            .zip(&diagnostic.related_spans)
        {
            if !span_in_path(session, *span, target_path) {
                continue;
            }
            related.location.uri = target_uri.to_string();
            related.location.range =
                remap_span_to_range(clean_file, dirty_file, *span, replacements)?;
        }
    }

    Some(converted)
}

fn remap_span_to_range(
    clean_file: &SourceFile,
    dirty_file: &SourceFile,
    span: Span,
    replacements: &[OffsetReplacement],
) -> Option<Range> {
    if span.end > clean_file.src.len() {
        return None;
    }

    let start = remap_offset(span.start, replacements)?;
    let end = remap_offset(span.end, replacements)?;
    Some(Range {
        start: byte_offset_to_position(dirty_file, start),
        end: byte_offset_to_position(dirty_file, end),
    })
}

fn remap_offset(offset: usize, replacements: &[OffsetReplacement]) -> Option<usize> {
    let mut delta = 0_i64;

    for replacement in replacements {
        if offset < replacement.clean_start {
            break;
        }
        if offset > replacement.clean_end {
            delta += replacement.dirty_end as i64 - replacement.dirty_start as i64;
            delta -= replacement.clean_end as i64 - replacement.clean_start as i64;
            continue;
        }
        if offset == replacement.clean_start {
            return Some(replacement.dirty_start);
        }
        if offset == replacement.clean_end {
            return Some(replacement.dirty_end);
        }
        return None;
    }

    offset.checked_add_signed(delta as isize)
}

pub fn cleared_uris(previous: &BTreeSet<String>, current: &[DiagnosticBundle]) -> Vec<String> {
    let current_uris: BTreeSet<_> = current.iter().map(|bundle| bundle.uri.clone()).collect();
    previous
        .iter()
        .filter(|uri| !current_uris.contains(*uri))
        .cloned()
        .collect()
}

fn span_in_path(session: &Session, span: Span, target_path: &Path) -> bool {
    session
        .source_manager
        .get_file_path(span.file)
        .map(|path| normalize_path(path) == target_path)
        .unwrap_or(false)
}

fn hash_source_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}
