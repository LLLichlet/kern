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
    apply_content_change, byte_offset_to_position, file_path_to_uri, is_valid_identifier,
    match_position_in_file, normalize_path, position_to_byte_offset, single_server_diagnostic,
    span_contains_offset, span_to_range, trim_line_ending, uri_to_file_path,
};
use crate::protocol::{
    CodeAction, CompletionItem, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentHighlight, DocumentSymbol, Hover, Location, Position,
    PrepareRenameResult, Range, SemanticTokens, SignatureHelp, TextDocumentContentChangeEvent,
    WorkspaceEdit,
};
use kernc_driver::{AnalysisArtifact, CompilerDriver, SourceOverrides};
use kernc_utils::config::CompileOptions;
use kernc_utils::{Session, Span};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct OpenDocument {
    pub path: PathBuf,
    pub version: i64,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct DiagnosticBundle {
    pub uri: String,
    pub diagnostics: Vec<crate::protocol::Diagnostic>,
}

pub struct AnalysisOutcome {
    pub bundles: Vec<DiagnosticBundle>,
}

#[derive(Debug, Clone)]
struct RenameTarget {
    query_span: Span,
    definition_span: Span,
    placeholder: String,
}

#[derive(Default)]
pub struct AnalysisEngine {
    documents: BTreeMap<String, OpenDocument>,
}

impl AnalysisEngine {
    pub fn open_document(&mut self, params: DidOpenTextDocumentParams) -> AnalysisOutcome {
        let doc = params.text_document;
        let uri = doc.uri.clone();
        let Some(path) = uri_to_file_path(&uri) else {
            return single_server_diagnostic(uri, "only file:// URIs are supported");
        };

        self.documents.insert(
            uri.clone(),
            OpenDocument {
                path,
                version: doc.version,
                text: doc.text,
            },
        );

        self.analyze_document(&uri)
    }

    pub fn change_document(&mut self, params: DidChangeTextDocumentParams) -> AnalysisOutcome {
        let Some(doc) = self.documents.get_mut(&params.text_document.uri) else {
            return single_server_diagnostic(
                params.text_document.uri,
                "received didChange for a document that is not open",
            );
        };

        let mut updated_text = doc.text.clone();
        for change in params.content_changes {
            if let Err(message) = apply_content_change(&doc.path, &mut updated_text, &change) {
                return single_server_diagnostic(params.text_document.uri.clone(), message);
            }
        }

        doc.text = updated_text;
        doc.version = params.text_document.version;

        self.analyze_document(&params.text_document.uri)
    }

    pub fn close_document(&mut self, params: DidCloseTextDocumentParams) -> AnalysisOutcome {
        self.documents.remove(&params.text_document.uri);
        AnalysisOutcome {
            bundles: vec![DiagnosticBundle {
                uri: params.text_document.uri,
                diagnostics: Vec::new(),
            }],
        }
    }

    pub fn document_symbols(&self, uri: &str) -> Result<Vec<DocumentSymbol>, String> {
        let artifact = self
            .analyze_artifact(uri)
            .map_err(|message| format!("document symbol analysis failed: {message}"))?;
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested document symbols for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);

        let mut symbols = Vec::new();
        for module_symbol in &artifact.symbols {
            let Some(path) = artifact
                .session
                .source_manager
                .get_file_path(module_symbol.span.file)
            else {
                continue;
            };
            if normalize_path(path) == target_path {
                symbols.extend(
                    module_symbol.children.iter().map(|symbol| {
                        analysis_symbol_to_document_symbol(&artifact.session, symbol)
                    }),
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
        let artifact = self
            .analyze_artifact(uri)
            .map_err(|message| format!("definition analysis failed: {message}"))?;
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested definition for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);

        Ok(find_definition_location(
            &artifact.session,
            &artifact.references,
            &target_path,
            &position,
        ))
    }

    pub fn references(
        &self,
        uri: &str,
        position: Position,
        include_declaration: bool,
    ) -> Result<Vec<Location>, String> {
        let artifact = self
            .analyze_artifact(uri)
            .map_err(|message| format!("reference analysis failed: {message}"))?;
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested references for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);

        Ok(find_reference_locations(
            &artifact.session,
            &artifact.references,
            &target_path,
            &position,
            include_declaration,
        ))
    }

    pub fn document_highlights(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<Vec<DocumentHighlight>, String> {
        let artifact = self
            .analyze_artifact(uri)
            .map_err(|message| format!("document highlight analysis failed: {message}"))?;
        let Some(target_doc) = self.documents.get(uri) else {
            return Err(
                "requested document highlights for a document that is not open".to_string(),
            );
        };
        let target_path = normalize_path(&target_doc.path);

        Ok(find_document_highlights(
            &artifact.session,
            &artifact.references,
            &artifact.hovers,
            &target_path,
            &position,
        ))
    }

    pub fn hover(&self, uri: &str, position: Position) -> Result<Option<Hover>, String> {
        let artifact = self
            .analyze_artifact(uri)
            .map_err(|message| format!("hover analysis failed: {message}"))?;
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested hover for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);

        Ok(find_hover(
            &artifact.session,
            &artifact.hovers,
            &artifact.references,
            &target_path,
            &position,
        ))
    }

    pub fn signature_help(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<Option<SignatureHelp>, String> {
        let artifact = self
            .analyze_artifact(uri)
            .map_err(|message| format!("signature help analysis failed: {message}"))?;
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
        let artifact = self
            .analyze_artifact(uri)
            .map_err(|message| format!("completion analysis failed: {message}"))?;
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested completion for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let file = kernc_utils::SourceFile::new(target_doc.path.clone(), target_doc.text.clone());
        let Some(offset) = position_to_byte_offset(&file, &position) else {
            return Ok(Vec::new());
        };

        Ok(artifact
            .completion_items(&target_path, offset)
            .into_iter()
            .map(analysis_completion_to_lsp_item)
            .collect())
    }

    pub fn prepare_rename(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<Option<PrepareRenameResult>, String> {
        let artifact = self
            .analyze_artifact(uri)
            .map_err(|message| format!("rename analysis failed: {message}"))?;
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested prepareRename for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let Some(target) = find_rename_target(
            &artifact.session,
            &artifact.hovers,
            &artifact.references,
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
            &artifact.references,
            &target_path,
            &position,
        ) else {
            return Err("rename target is not a supported identifier".to_string());
        };

        let changes = build_rename_changes(
            &artifact.session,
            &artifact.references,
            target.definition_span,
            new_name,
        );

        Ok(WorkspaceEdit { changes })
    }

    pub fn semantic_tokens(&self, uri: &str) -> Result<SemanticTokens, String> {
        let artifact = self
            .analyze_artifact(uri)
            .map_err(|message| format!("semantic token analysis failed: {message}"))?;
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested semantic tokens for a document that is not open".to_string());
        };
        let file = kernc_utils::SourceFile::new(target_doc.path.clone(), target_doc.text.clone());
        let target_path = normalize_path(&target_doc.path);

        Ok(semantic::semantic_tokens(&artifact, &file, &target_path))
    }

    pub fn code_actions(&self, uri: &str, range: Range) -> Result<Vec<CodeAction>, String> {
        let artifact = self
            .analyze_artifact(uri)
            .map_err(|message| format!("code action analysis failed: {message}"))?;
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

    fn source_overrides(&self) -> SourceOverrides {
        self.documents
            .values()
            .map(|doc| (doc.path.clone(), doc.text.clone()))
            .collect()
    }

    fn analyze_artifact(&self, target_uri: &str) -> Result<AnalysisArtifact, String> {
        let Some(target_doc) = self.documents.get(target_uri) else {
            return Err("document is not open".to_string());
        };

        let mut options = CompileOptions::default();
        options.use_std = true;

        let input_file = target_doc.path.to_string_lossy().into_owned();
        let overrides = self.source_overrides();
        let driver = CompilerDriver::new(options);
        Ok(driver.analyze_artifact(&input_file, &overrides))
    }
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
