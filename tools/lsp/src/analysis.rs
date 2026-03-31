use crate::protocol::{
    CompletionItem, Diagnostic, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentSymbol, Hover, Location, MarkupContent, Position,
    PrepareRenameResult, Range, TextDocumentContentChangeEvent, TextEdit, WorkspaceEdit,
};
use kernc_driver::{
    AnalysisArtifact, AnalysisCompletionItem, AnalysisCompletionKind, AnalysisHover,
    AnalysisReference, AnalysisSymbol, AnalysisSymbolKind, CompilerDriver, SourceOverrides,
};
use kernc_utils::config::CompileOptions;
use kernc_utils::{DiagnosticLevel, FileId, Session, Span};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::{fs, io};

#[derive(Debug, Clone)]
pub struct OpenDocument {
    pub path: PathBuf,
    pub version: i64,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct DiagnosticBundle {
    pub uri: String,
    pub diagnostics: Vec<Diagnostic>,
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

fn apply_content_change(
    path: &Path,
    text: &mut String,
    change: &TextDocumentContentChangeEvent,
) -> Result<(), String> {
    let Some(range) = &change.range else {
        text.clear();
        text.push_str(&change.text);
        return Ok(());
    };

    let file = kernc_utils::SourceFile::new(path.to_path_buf(), text.clone());
    let Some(start) = position_to_byte_offset(&file, &range.start) else {
        return Err(format!(
            "received incremental change with invalid start position {}:{}",
            range.start.line, range.start.character
        ));
    };
    let Some(end) = position_to_byte_offset(&file, &range.end) else {
        return Err(format!(
            "received incremental change with invalid end position {}:{}",
            range.end.line, range.end.character
        ));
    };
    if start > end {
        return Err("received incremental change with a reversed range".to_string());
    }

    text.replace_range(start..end, &change.text);
    Ok(())
}

pub fn cleared_uris(previous: &BTreeSet<String>, current: &[DiagnosticBundle]) -> Vec<String> {
    let current_uris: BTreeSet<_> = current.iter().map(|bundle| bundle.uri.clone()).collect();
    previous
        .iter()
        .filter(|uri| !current_uris.contains(*uri))
        .cloned()
        .collect()
}

fn diagnostics_from_session(
    session: &Session,
    open_documents: &BTreeMap<String, OpenDocument>,
) -> BTreeMap<String, Vec<Diagnostic>> {
    let uri_by_path: BTreeMap<_, _> = open_documents
        .iter()
        .map(|(uri, doc)| (normalize_path(&doc.path), uri.clone()))
        .collect();

    let mut bundles = BTreeMap::<String, Vec<Diagnostic>>::new();

    for diag in &session.diagnostics {
        let uri = diagnostic_uri(session, diag.primary_span.file, &uri_by_path)
            .unwrap_or_else(|| "kern-lsp:/unknown".to_string());

        bundles
            .entry(uri)
            .or_default()
            .push(convert_diagnostic(session, diag));
    }

    bundles
}

fn diagnostic_uri(
    session: &Session,
    file_id: FileId,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> Option<String> {
    let path = session.source_manager.get_file_path(file_id)?;
    let normalized = normalize_path(path);
    if let Some(uri) = uri_by_path.get(&normalized) {
        return Some(uri.clone());
    }

    file_path_to_uri(path).ok()
}

fn convert_diagnostic(session: &Session, diagnostic: &kernc_utils::Diagnostic) -> Diagnostic {
    Diagnostic {
        range: span_to_range(session, diagnostic.primary_span),
        severity: diagnostic_severity(diagnostic.level),
        source: "kernc",
        message: diagnostic.message.clone(),
    }
}

fn analysis_symbol_to_document_symbol(
    session: &Session,
    symbol: &AnalysisSymbol,
) -> DocumentSymbol {
    DocumentSymbol {
        name: symbol.name.clone(),
        detail: symbol.detail.clone(),
        kind: lsp_symbol_kind(symbol.kind),
        range: span_to_range(session, symbol.span),
        selection_range: span_to_range(session, symbol.selection_span),
        children: symbol
            .children
            .iter()
            .map(|child| analysis_symbol_to_document_symbol(session, child))
            .collect(),
    }
}

fn find_rename_target(
    session: &Session,
    hovers: &[AnalysisHover],
    references: &[AnalysisReference],
    target_path: &Path,
    position: &Position,
) -> Option<RenameTarget> {
    if let Some(target) =
        rename_target_at_definition_position(session, hovers, target_path, position)
    {
        return Some(target);
    }

    for reference in references {
        let Some(file) = session
            .source_manager
            .get_file(reference.reference_span.file)
        else {
            continue;
        };
        let Some(offset) = match_position_in_file(file, target_path, position) else {
            continue;
        };
        if !span_contains_offset(reference.reference_span, offset) {
            continue;
        }

        let placeholder = span_text(session, reference.reference_span)?;
        return Some(RenameTarget {
            query_span: reference.reference_span,
            definition_span: reference.definition_span,
            placeholder,
        });
    }

    None
}

fn rename_target_at_definition_position(
    session: &Session,
    hovers: &[AnalysisHover],
    target_path: &Path,
    position: &Position,
) -> Option<RenameTarget> {
    for hover in hovers {
        let Some(file) = session.source_manager.get_file(hover.span.file) else {
            continue;
        };
        let Some(offset) = match_position_in_file(file, target_path, position) else {
            continue;
        };
        if !span_contains_offset(hover.span, offset) {
            continue;
        }

        return Some(RenameTarget {
            query_span: hover.span,
            definition_span: hover.span,
            placeholder: span_text(session, hover.span)?,
        });
    }

    None
}

fn find_definition_location(
    session: &Session,
    references: &[AnalysisReference],
    target_path: &Path,
    position: &Position,
) -> Option<Location> {
    let definition_span = find_target_definition_span(session, references, target_path, position)?;
    location_from_span(session, definition_span)
}

fn find_reference_locations(
    session: &Session,
    references: &[AnalysisReference],
    target_path: &Path,
    position: &Position,
    include_declaration: bool,
) -> Vec<Location> {
    let Some(definition_span) =
        find_target_definition_span(session, references, target_path, position)
    else {
        return Vec::new();
    };

    let mut locations = Vec::new();
    if include_declaration && let Some(location) = location_from_span(session, definition_span) {
        locations.push(location);
    }

    for reference in references {
        if reference.definition_span != definition_span {
            continue;
        }

        if let Some(location) = location_from_span(session, reference.reference_span) {
            locations.push(location);
        }
    }

    locations.sort_by_key(|location| {
        let range = &location.range;
        (
            location.uri.clone(),
            range.start.line,
            range.start.character,
            range.end.line,
            range.end.character,
        )
    });
    locations.dedup();
    locations
}

fn find_target_definition_span(
    session: &Session,
    references: &[AnalysisReference],
    target_path: &Path,
    position: &Position,
) -> Option<Span> {
    let mut best_match = None;

    for reference in references {
        let Some(reference_file) = session
            .source_manager
            .get_file(reference.reference_span.file)
        else {
            continue;
        };
        let reference_offset = match_position_in_file(reference_file, target_path, position);
        if let Some(offset) = reference_offset
            && span_contains_offset(reference.reference_span, offset)
        {
            best_match = Some(reference.definition_span);
            break;
        }

        let Some(definition_file) = session
            .source_manager
            .get_file(reference.definition_span.file)
        else {
            continue;
        };
        let definition_offset = match_position_in_file(definition_file, target_path, position);
        if let Some(offset) = definition_offset
            && span_contains_offset(reference.definition_span, offset)
        {
            best_match = Some(reference.definition_span);
            break;
        }
    }

    best_match
}

fn find_hover(
    session: &Session,
    hovers: &[AnalysisHover],
    references: &[AnalysisReference],
    target_path: &Path,
    position: &Position,
) -> Option<Hover> {
    if let Some(hover) = hover_at_definition_position(session, hovers, target_path, position) {
        return Some(hover);
    }

    let definition_span = find_target_definition_span(session, references, target_path, position)?;
    let hover = hovers.iter().find(|hover| hover.span == definition_span)?;
    Some(analysis_hover_to_lsp_hover(session, hover))
}

fn hover_at_definition_position(
    session: &Session,
    hovers: &[AnalysisHover],
    target_path: &Path,
    position: &Position,
) -> Option<Hover> {
    for hover in hovers {
        let Some(file) = session.source_manager.get_file(hover.span.file) else {
            continue;
        };
        let Some(offset) = match_position_in_file(file, target_path, position) else {
            continue;
        };
        if span_contains_offset(hover.span, offset) {
            return Some(analysis_hover_to_lsp_hover(session, hover));
        }
    }

    None
}

fn analysis_hover_to_lsp_hover(session: &Session, hover: &AnalysisHover) -> Hover {
    Hover {
        contents: MarkupContent {
            kind: "markdown",
            value: hover.contents.clone(),
        },
        range: Some(span_to_range(session, hover.span)),
    }
}

fn build_rename_changes(
    session: &Session,
    references: &[AnalysisReference],
    definition_span: Span,
    new_name: &str,
) -> BTreeMap<String, Vec<TextEdit>> {
    let mut edits_by_uri = BTreeMap::<String, Vec<TextEdit>>::new();

    if let Some(edit) = rename_edit_from_span(session, definition_span, new_name) {
        edits_by_uri.entry(edit.0).or_default().push(edit.1);
    }

    for reference in references {
        if reference.definition_span != definition_span {
            continue;
        }

        if let Some(edit) = rename_edit_from_span(session, reference.reference_span, new_name) {
            edits_by_uri.entry(edit.0).or_default().push(edit.1);
        }
    }

    for edits in edits_by_uri.values_mut() {
        edits.sort_by_key(|edit| {
            (
                edit.range.start.line,
                edit.range.start.character,
                edit.range.end.line,
                edit.range.end.character,
            )
        });
        edits.dedup();
    }

    edits_by_uri
}

fn rename_edit_from_span(
    session: &Session,
    span: Span,
    new_name: &str,
) -> Option<(String, TextEdit)> {
    let path = session.source_manager.get_file_path(span.file)?;
    let uri = file_path_to_uri(path).ok()?;
    Some((
        uri,
        TextEdit {
            range: span_to_range(session, span),
            new_text: new_name.to_string(),
        },
    ))
}

fn match_position_in_file(
    file: &kernc_utils::SourceFile,
    target_path: &Path,
    position: &Position,
) -> Option<usize> {
    if normalize_path(&file.path) != target_path {
        return None;
    }

    position_to_byte_offset(file, position)
}

fn lsp_symbol_kind(kind: AnalysisSymbolKind) -> u8 {
    match kind {
        AnalysisSymbolKind::Module => 2,
        AnalysisSymbolKind::Namespace => 3,
        AnalysisSymbolKind::Struct => 23,
        AnalysisSymbolKind::Union => 23,
        AnalysisSymbolKind::Trait => 11,
        AnalysisSymbolKind::Method => 6,
        AnalysisSymbolKind::Function => 12,
        AnalysisSymbolKind::Enum => 10,
        AnalysisSymbolKind::TypeAlias => 13,
        AnalysisSymbolKind::Constant => 14,
        AnalysisSymbolKind::Static => 13,
    }
}

fn analysis_completion_to_lsp_item(item: AnalysisCompletionItem) -> CompletionItem {
    CompletionItem {
        label: item.label,
        kind: lsp_completion_kind(item.kind),
        detail: item.detail,
    }
}

fn lsp_completion_kind(kind: AnalysisCompletionKind) -> u8 {
    match kind {
        AnalysisCompletionKind::Variable => 6,
        AnalysisCompletionKind::Function => 3,
        AnalysisCompletionKind::Module => 9,
        AnalysisCompletionKind::Struct => 22,
        AnalysisCompletionKind::Union => 22,
        AnalysisCompletionKind::Enum => 13,
        AnalysisCompletionKind::Trait => 8,
        AnalysisCompletionKind::TypeAlias => 25,
        AnalysisCompletionKind::Constant => 21,
        AnalysisCompletionKind::Static => 6,
        AnalysisCompletionKind::TypeParameter => 25,
    }
}

fn diagnostic_severity(level: DiagnosticLevel) -> u8 {
    match level {
        DiagnosticLevel::Error | DiagnosticLevel::Ice => 1,
        DiagnosticLevel::Warning => 2,
        DiagnosticLevel::Note => 3,
    }
}

fn span_to_range(session: &Session, span: Span) -> Range {
    let Some(file) = session.source_manager.get_file(span.file) else {
        return empty_range();
    };

    Range {
        start: byte_offset_to_position(file, span.start),
        end: byte_offset_to_position(file, span.end),
    }
}

fn location_from_span(session: &Session, span: Span) -> Option<Location> {
    let path = session.source_manager.get_file_path(span.file)?;
    let uri = file_path_to_uri(path).ok()?;
    Some(Location {
        uri,
        range: span_to_range(session, span),
    })
}

fn span_text(session: &Session, span: Span) -> Option<String> {
    let file = session.source_manager.get_file(span.file)?;
    Some(file.src.get(span.start..span.end)?.to_string())
}

fn byte_offset_to_position(file: &kernc_utils::SourceFile, offset: usize) -> Position {
    let clamped = offset.min(file.src.len());
    let line = file.lookup_line(clamped);
    let line_start = file.line_starts[line.saturating_sub(1)];
    let character = file.src[line_start..clamped].encode_utf16().count() as u32;

    Position {
        line: line.saturating_sub(1) as u32,
        character,
    }
}

fn position_to_byte_offset(file: &kernc_utils::SourceFile, position: &Position) -> Option<usize> {
    let line_index = usize::try_from(position.line).ok()?;
    let line_start = *file.line_starts.get(line_index)?;
    let next_line_start = file
        .line_starts
        .get(line_index + 1)
        .copied()
        .unwrap_or(file.src.len());
    let line_end = trim_line_ending(&file.src, line_start, next_line_start);
    let line = &file.src[line_start..line_end];
    let target_units = position.character;

    let mut utf16_units = 0;
    for (byte_idx, ch) in line.char_indices() {
        if utf16_units == target_units {
            return Some(line_start + byte_idx);
        }

        utf16_units += ch.len_utf16() as u32;
        if utf16_units > target_units {
            return None;
        }
    }

    if utf16_units == target_units {
        Some(line_start + line.len())
    } else {
        None
    }
}

fn trim_line_ending(source: &str, start: usize, end: usize) -> usize {
    let mut trimmed_end = end;

    if trimmed_end > start && source.as_bytes()[trimmed_end - 1] == b'\n' {
        trimmed_end -= 1;
    }
    if trimmed_end > start && source.as_bytes()[trimmed_end - 1] == b'\r' {
        trimmed_end -= 1;
    }

    trimmed_end
}

fn span_contains_offset(span: Span, offset: usize) -> bool {
    let end = if span.end > span.start {
        span.end
    } else {
        span.start.saturating_add(1)
    };
    offset >= span.start && offset < end
}

fn empty_range() -> Range {
    Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: 0,
            character: 0,
        },
    }
}

fn single_server_diagnostic(uri: String, message: impl Into<String>) -> AnalysisOutcome {
    AnalysisOutcome {
        bundles: vec![DiagnosticBundle {
            uri,
            diagnostics: vec![Diagnostic {
                range: empty_range(),
                severity: 2,
                source: "kern-lsp",
                message: message.into(),
            }],
        }],
    }
}

fn uri_to_file_path(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file://")?;
    let decoded = percent_decode(raw).ok()?;

    #[cfg(windows)]
    {
        let trimmed = decoded.strip_prefix('/').unwrap_or(&decoded);
        let with_separators = trimmed.replace('/', "\\");
        return Some(PathBuf::from(with_separators));
    }

    #[cfg(not(windows))]
    {
        Some(PathBuf::from(decoded))
    }
}

fn file_path_to_uri(path: &Path) -> io::Result<String> {
    let normalized = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let raw = normalized.to_string_lossy();

    #[cfg(windows)]
    {
        let slash_path = raw.replace('\\', "/");
        Ok(format!("file:///{}", percent_encode(&slash_path)))
    }

    #[cfg(not(windows))]
    {
        Ok(format!("file://{}", percent_encode(&raw)))
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn percent_decode(input: &str) -> Result<String, ()> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;

    while idx < bytes.len() {
        match bytes[idx] {
            b'%' => {
                if idx + 2 >= bytes.len() {
                    return Err(());
                }
                let hi = hex_value(bytes[idx + 1]).ok_or(())?;
                let lo = hex_value(bytes[idx + 2]).ok_or(())?;
                out.push((hi << 4) | lo);
                idx += 3;
            }
            b => {
                out.push(b);
                idx += 1;
            }
        }
    }

    String::from_utf8(out).map_err(|_| ())
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());

    for byte in input.bytes() {
        if is_unreserved_uri_byte(byte) || byte == b'/' || byte == b':' {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(hex_digit(byte >> 4));
            out.push(hex_digit(byte & 0x0f));
        }
    }

    out
}

fn is_unreserved_uri_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn hex_digit(value: u8) -> char {
    match value & 0x0f {
        0..=9 => (b'0' + (value & 0x0f)) as char,
        10..=15 => (b'A' + ((value & 0x0f) - 10)) as char,
        _ => unreachable!(),
    }
}

fn is_valid_identifier(name: &str) -> bool {
    if name.is_empty() || name == "_" || is_keyword(name) {
        return false;
    }

    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !is_identifier_start(first) {
        return false;
    }

    bytes.all(is_identifier_continue)
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_identifier_continue(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

fn is_keyword(name: &str) -> bool {
    matches!(
        name,
        "fn" | "let"
            | "mut"
            | "const"
            | "static"
            | "type"
            | "struct"
            | "union"
            | "enum"
            | "trait"
            | "if"
            | "else"
            | "for"
            | "break"
            | "continue"
            | "return"
            | "defer"
            | "pub"
            | "extern"
            | "use"
            | "impl"
            | "true"
            | "false"
            | "undef"
            | "as"
            | "and"
            | "or"
            | "Self"
            | "self"
            | "match"
            | "mod"
            | "where"
            | "void"
            | "Fn"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        AnalysisEngine, byte_offset_to_position, cleared_uris, file_path_to_uri,
        position_to_byte_offset, uri_to_file_path,
    };
    use crate::protocol::{
        DidChangeTextDocumentParams, DidOpenTextDocumentParams, Position, Range,
        TextDocumentContentChangeEvent, TextDocumentItem, VersionedTextDocumentIdentifier,
    };
    use kernc_utils::SourceFile;
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn full_sync_replaces_document_text() {
        let mut analysis = AnalysisEngine::default();
        let uri = temp_file_uri("full_sync", "let x = 1;");

        let outcome = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: "let x = 1;".to_string(),
            },
        });

        assert!(!outcome.bundles.is_empty());

        let outcome = analysis.change_document(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                text: "let x = 2;".to_string(),
            }],
        });

        assert!(outcome.bundles.iter().any(|bundle| bundle.uri == uri));
        let doc = analysis.documents.get(&uri).unwrap();
        assert_eq!(doc.version, 2);
        assert_eq!(doc.text, "let x = 2;");
    }

    #[test]
    fn incremental_sync_inserts_text() {
        let mut analysis = AnalysisEngine::default();
        let uri = temp_file_uri("incremental_insert", "let value = 1;");

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: "let value = 1;".to_string(),
            },
        });

        let outcome = analysis.change_document(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: Some(Range {
                    start: Position {
                        line: 0,
                        character: 13,
                    },
                    end: Position {
                        line: 0,
                        character: 13,
                    },
                }),
                text: " + 1".to_string(),
            }],
        });

        assert!(outcome.bundles.iter().any(|bundle| bundle.uri == uri));
        assert_eq!(
            analysis.documents.get(&uri).unwrap().text,
            "let value = 1 + 1;"
        );
    }

    #[test]
    fn incremental_sync_replaces_text() {
        let mut analysis = AnalysisEngine::default();
        let uri = temp_file_uri("incremental_replace", "let value = 1;");

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: "let value = 1;".to_string(),
            },
        });

        let outcome = analysis.change_document(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: Some(Range {
                    start: Position {
                        line: 0,
                        character: 12,
                    },
                    end: Position {
                        line: 0,
                        character: 13,
                    },
                }),
                text: "42".to_string(),
            }],
        });

        assert!(outcome.bundles.iter().any(|bundle| bundle.uri == uri));
        assert_eq!(
            analysis.documents.get(&uri).unwrap().text,
            "let value = 42;"
        );
    }

    #[test]
    fn incremental_sync_deletes_text() {
        let mut analysis = AnalysisEngine::default();
        let uri = temp_file_uri("incremental_delete", "let value = 123;");

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: "let value = 123;".to_string(),
            },
        });

        let outcome = analysis.change_document(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: Some(Range {
                    start: Position {
                        line: 0,
                        character: 12,
                    },
                    end: Position {
                        line: 0,
                        character: 14,
                    },
                }),
                text: String::new(),
            }],
        });

        assert!(outcome.bundles.iter().any(|bundle| bundle.uri == uri));
        assert_eq!(analysis.documents.get(&uri).unwrap().text, "let value = 3;");
    }

    #[test]
    fn incremental_sync_respects_utf16_positions() {
        let mut analysis = AnalysisEngine::default();
        let uri = temp_file_uri("incremental_utf16", "let face = \"😀x\";");

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: "let face = \"😀x\";".to_string(),
            },
        });

        let outcome = analysis.change_document(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: Some(Range {
                    start: Position {
                        line: 0,
                        character: 14,
                    },
                    end: Position {
                        line: 0,
                        character: 15,
                    },
                }),
                text: "!".to_string(),
            }],
        });

        assert!(outcome.bundles.iter().any(|bundle| bundle.uri == uri));
        assert_eq!(
            analysis.documents.get(&uri).unwrap().text,
            "let face = \"😀!\";"
        );
    }

    #[test]
    fn invalid_incremental_sync_range_keeps_previous_text() {
        let mut analysis = AnalysisEngine::default();
        let uri = temp_file_uri("incremental_invalid", "let value = 1;");

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: "let value = 1;".to_string(),
            },
        });

        let outcome = analysis.change_document(DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version: 2,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: Some(Range {
                    start: Position {
                        line: 1,
                        character: 0,
                    },
                    end: Position {
                        line: 1,
                        character: 1,
                    },
                }),
                text: "x".to_string(),
            }],
        });

        let bundle = outcome
            .bundles
            .iter()
            .find(|bundle| bundle.uri == uri)
            .unwrap();
        assert_eq!(analysis.documents.get(&uri).unwrap().text, "let value = 1;");
        assert!(
            bundle
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("invalid start position"))
        );
    }

    #[test]
    fn overlay_text_is_used_for_compiler_diagnostics() {
        let mut analysis = AnalysisEngine::default();
        let uri = temp_file_uri("overlay_diag", "extern fn main() i32 { 0 }");

        let outcome = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: "extern fn main( ".to_string(),
            },
        });

        let bundle = outcome
            .bundles
            .iter()
            .find(|bundle| bundle.uri == uri)
            .unwrap();
        assert!(
            !bundle.diagnostics.is_empty(),
            "expected diagnostics from in-memory overlay"
        );
    }

    #[test]
    fn file_uri_roundtrips() {
        let path = unique_temp_file_path("uri_roundtrip");
        let uri = file_path_to_uri(&path).unwrap();
        let parsed = uri_to_file_path(&uri).unwrap();
        assert_eq!(parsed, path);
    }

    #[test]
    fn computes_cleared_uris() {
        let previous = BTreeSet::from(["file:///one.rn".to_string(), "file:///two.rn".to_string()]);
        let current = vec![super::DiagnosticBundle {
            uri: "file:///one.rn".to_string(),
            diagnostics: Vec::new(),
        }];

        let cleared = cleared_uris(&previous, &current);
        assert_eq!(cleared, vec!["file:///two.rn".to_string()]);
    }

    #[test]
    fn extracts_document_symbols_from_compiler_artifact() {
        let mut analysis = AnalysisEngine::default();
        let uri = temp_file_uri(
            "document_symbols",
            "type Point = struct { x: i32, y: i32 };\nfn helper() i32 { return 1; }\n",
        );

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: "type Point = struct { x: i32, y: i32 };\nfn helper() i32 { return 1; }\n"
                    .to_string(),
            },
        });

        let symbols = analysis.document_symbols(&uri).unwrap();
        let names = symbols
            .iter()
            .map(|symbol| symbol.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"Point"));
        assert!(names.contains(&"helper"));
    }

    #[test]
    fn goto_definition_resolves_local_identifier_references() {
        let mut analysis = AnalysisEngine::default();
        let source = "fn helper() i32 {\n    let value = i32.{1};\n    return value;\n}\n";
        let uri = temp_file_uri("goto_definition_local", source);

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: source.to_string(),
            },
        });

        let query_position = position_of_nth(source, "value", 1, 2);
        let definition = analysis
            .goto_definition(&uri, query_position)
            .unwrap()
            .unwrap();

        assert_eq!(definition.uri, uri);
        assert_eq!(
            definition.range.start,
            position_of_nth(source, "value", 0, 0)
        );
    }

    #[test]
    fn goto_definition_resolves_function_identifier_references() {
        let mut analysis = AnalysisEngine::default();
        let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper(); }\n";
        let uri = temp_file_uri("goto_definition_function", source);

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: source.to_string(),
            },
        });

        let query_position = position_of_nth(source, "helper", 1, 1);
        let definition = analysis
            .goto_definition(&uri, query_position)
            .unwrap()
            .unwrap();

        assert_eq!(definition.uri, uri);
        assert_eq!(
            definition.range.start,
            position_of_nth(source, "helper", 0, 0)
        );
    }

    #[test]
    fn finds_references_from_identifier_reference_position() {
        let mut analysis = AnalysisEngine::default();
        let source =
            "fn helper() i32 { return 1; }\nfn main() i32 { return helper() + helper(); }\n";
        let uri = temp_file_uri("references_from_ref", source);

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: source.to_string(),
            },
        });

        let query_position = position_of_nth(source, "helper", 1, 1);
        let locations = analysis.references(&uri, query_position, false).unwrap();

        assert_eq!(locations.len(), 2);
        assert_eq!(
            locations[0].range.start,
            position_of_nth(source, "helper", 1, 0)
        );
        assert_eq!(
            locations[1].range.start,
            position_of_nth(source, "helper", 2, 0)
        );
    }

    #[test]
    fn finds_references_from_definition_position_including_declaration() {
        let mut analysis = AnalysisEngine::default();
        let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper(); }\n";
        let uri = temp_file_uri("references_from_def", source);

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: source.to_string(),
            },
        });

        let query_position = position_of_nth(source, "helper", 0, 1);
        let locations = analysis.references(&uri, query_position, true).unwrap();

        assert_eq!(locations.len(), 2);
        assert_eq!(
            locations[0].range.start,
            position_of_nth(source, "helper", 0, 0)
        );
        assert_eq!(
            locations[1].range.start,
            position_of_nth(source, "helper", 1, 0)
        );
    }

    #[test]
    fn hover_resolves_function_signature_from_reference() {
        let mut analysis = AnalysisEngine::default();
        let source = "fn helper(x: i32) i32 { return x; }\nfn main() i32 { return helper(1); }\n";
        let uri = temp_file_uri("hover_function", source);

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: source.to_string(),
            },
        });

        let hover = analysis
            .hover(&uri, position_of_nth(source, "helper", 1, 1))
            .unwrap()
            .unwrap();

        assert!(hover.contents.value.contains("fn helper: fn(i32) i32"));
    }

    #[test]
    fn hover_resolves_local_definition_without_references() {
        let mut analysis = AnalysisEngine::default();
        let source = "fn main() i32 {\n    let value = i32.{1};\n    return 0;\n}\n";
        let uri = temp_file_uri("hover_local_definition", source);

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: source.to_string(),
            },
        });

        let hover = analysis
            .hover(&uri, position_of_nth(source, "value", 0, 1))
            .unwrap()
            .unwrap();

        assert!(hover.contents.value.contains("var value: i32"));
    }

    #[test]
    fn prepare_rename_returns_placeholder_for_reference() {
        let mut analysis = AnalysisEngine::default();
        let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper(); }\n";
        let uri = temp_file_uri("prepare_rename", source);

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: source.to_string(),
            },
        });

        let result = analysis
            .prepare_rename(&uri, position_of_nth(source, "helper", 1, 1))
            .unwrap()
            .unwrap();

        assert_eq!(result.placeholder, "helper");
        assert_eq!(result.range.start, position_of_nth(source, "helper", 1, 0));
    }

    #[test]
    fn rename_updates_definition_and_references() {
        let mut analysis = AnalysisEngine::default();
        let source =
            "fn helper() i32 { return 1; }\nfn main() i32 { return helper() + helper(); }\n";
        let uri = temp_file_uri("rename_function", source);

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: source.to_string(),
            },
        });

        let edit = analysis
            .rename(&uri, position_of_nth(source, "helper", 1, 1), "assist")
            .unwrap();
        let edits = edit.changes.get(&uri).unwrap();

        assert_eq!(edits.len(), 3);
        assert!(edits.iter().all(|edit| edit.new_text == "assist"));
        assert_eq!(
            edits[0].range.start,
            position_of_nth(source, "helper", 0, 0)
        );
        assert_eq!(
            edits[1].range.start,
            position_of_nth(source, "helper", 1, 0)
        );
        assert_eq!(
            edits[2].range.start,
            position_of_nth(source, "helper", 2, 0)
        );
    }

    #[test]
    fn rename_rejects_invalid_identifiers() {
        let mut analysis = AnalysisEngine::default();
        let source = "fn helper() i32 { return 1; }\n";
        let uri = temp_file_uri("rename_invalid", source);

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: source.to_string(),
            },
        });

        let error = analysis
            .rename(&uri, position_of_nth(source, "helper", 0, 1), "fn")
            .unwrap_err();

        assert!(error.contains("not a valid Kern identifier"));
    }

    #[test]
    fn byte_offsets_roundtrip_through_utf16_positions() {
        let file = SourceFile::new(PathBuf::from("utf16.rn"), "a😀b\n".to_string());
        let offset = "a😀".len();
        let position = byte_offset_to_position(&file, offset);

        assert_eq!(
            position,
            Position {
                line: 0,
                character: 3,
            }
        );
        assert_eq!(position_to_byte_offset(&file, &position), Some(offset));
    }

    #[test]
    fn completion_in_function_body_includes_visible_symbols() {
        let mut analysis = AnalysisEngine::default();
        let source = concat!(
            "type Point = struct { x: i32 };\n",
            "fn helper(param: i32) i32 {\n",
            "    let value = param;\n",
            "    return value;\n",
            "}\n",
        );
        let uri = temp_file_uri("completion_function", source);

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: source.to_string(),
            },
        });

        let items = analysis
            .completion(&uri, position_of_nth(source, "return", 0, 0))
            .unwrap();
        let labels = completion_labels(&items);

        assert!(labels.contains(&"Point".to_string()));
        assert!(labels.contains(&"helper".to_string()));
        assert!(labels.contains(&"param".to_string()));
        assert!(labels.contains(&"value".to_string()));
    }

    #[test]
    fn completion_in_method_body_includes_self() {
        let mut analysis = AnalysisEngine::default();
        let source = concat!(
            "type Counter = struct { value: i32 };\n",
            "impl Counter {\n",
            "    fn get() i32 {\n",
            "        return self.value;\n",
            "    }\n",
            "}\n",
        );
        let uri = temp_file_uri("completion_method", source);

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: source.to_string(),
            },
        });

        let items = analysis
            .completion(&uri, position_of_nth(source, "self", 0, 0))
            .unwrap();
        let labels = completion_labels(&items);

        assert!(labels.contains(&"self".to_string()));
        assert!(labels.contains(&"Counter".to_string()));
    }

    #[test]
    fn completion_on_field_access_returns_member_items() {
        let mut analysis = AnalysisEngine::default();
        let source = concat!(
            "type Point = struct { x: i32, y: i32 };\n",
            "fn main() i32 {\n",
            "    let point = Point.{ x: 1, y: 2 };\n",
            "    return point.x;\n",
            "}\n",
        );
        let uri = temp_file_uri("completion_field_access", source);

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: source.to_string(),
            },
        });

        let items = analysis
            .completion(&uri, position_of_nth(source, "point", 1, 5))
            .unwrap();
        let labels = completion_labels(&items);

        assert_eq!(labels, vec!["x".to_string(), "y".to_string()]);
    }

    #[test]
    fn completion_on_generic_bound_receiver_includes_trait_methods() {
        let mut analysis = AnalysisEngine::default();
        let source = concat!(
            "type HasLen = trait { len: fn() i32, };\n",
            "impl *i32 : HasLen {\n",
            "    pub fn len() i32 { return self.*; }\n",
            "}\n",
            "fn use_it[T](x: *T) i32\n",
            "    where *T: HasLen,\n",
            "{\n",
            "    return x.len();\n",
            "}\n",
        );
        let uri = temp_file_uri("completion_generic_bound", source);

        let _ = analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: source.to_string(),
            },
        });

        let items = analysis
            .completion(&uri, position_of_nth(source, "x", 1, 1))
            .unwrap();
        let labels = completion_labels(&items);

        assert!(labels.contains(&"len".to_string()));
    }

    fn temp_file_uri(prefix: &str, initial_text: &str) -> String {
        let path = unique_temp_file_path(prefix);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, initial_text).unwrap();
        file_path_to_uri(&path).unwrap()
    }

    fn unique_temp_file_path(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let counter = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "{}_{}_{}_{}.rn",
            prefix,
            std::process::id(),
            nanos,
            counter
        ))
    }

    fn position_of_nth(
        source: &str,
        needle: &str,
        occurrence: usize,
        char_offset: u32,
    ) -> Position {
        let byte_offset = nth_match_offset(source, needle, occurrence) + char_offset as usize;
        let prefix = &source[..byte_offset];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32;
        let line_start = prefix.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
        let character = source[line_start..byte_offset].encode_utf16().count() as u32;

        Position { line, character }
    }

    fn nth_match_offset(source: &str, needle: &str, occurrence: usize) -> usize {
        source
            .match_indices(needle)
            .nth(occurrence)
            .map(|(offset, _)| offset)
            .unwrap()
    }

    fn completion_labels(items: &[crate::protocol::CompletionItem]) -> Vec<String> {
        items.iter().map(|item| item.label.clone()).collect()
    }
}
