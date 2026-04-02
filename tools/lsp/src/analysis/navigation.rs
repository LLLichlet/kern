use super::RenameTarget;
use crate::protocol::{
    CompletionItem, DocumentHighlight, DocumentSymbol, Hover, Location, MarkupContent,
    ParameterInformation, Position, SignatureHelp, SignatureInformation, TextEdit,
};
use kernc_driver::{
    AnalysisCompletionItem, AnalysisCompletionKind, AnalysisHover, AnalysisSemanticEntry,
    AnalysisSemanticRole, AnalysisSignatureHelp, AnalysisSymbol, AnalysisSymbolKind,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(super) fn analysis_symbol_to_document_symbol(
    session: &kernc_utils::Session,
    symbol: &AnalysisSymbol,
) -> DocumentSymbol {
    DocumentSymbol {
        name: symbol.name.clone(),
        detail: symbol.detail.clone(),
        kind: lsp_symbol_kind(symbol.kind),
        range: super::span_to_range(session, symbol.span),
        selection_range: super::span_to_range(session, symbol.selection_span),
        children: symbol
            .children
            .iter()
            .map(|child| analysis_symbol_to_document_symbol(session, child))
            .collect(),
    }
}

pub(super) fn analysis_signature_help_to_lsp_help(help: AnalysisSignatureHelp) -> SignatureHelp {
    SignatureHelp {
        signatures: help
            .signatures
            .into_iter()
            .map(|signature| SignatureInformation {
                label: signature.label,
                parameters: signature
                    .parameters
                    .into_iter()
                    .map(|parameter| ParameterInformation {
                        label: parameter.label,
                    })
                    .collect(),
            })
            .collect(),
        active_signature: help.active_signature as u32,
        active_parameter: help.active_parameter as u32,
    }
}

pub(super) fn find_rename_target(
    session: &kernc_utils::Session,
    hovers: &[AnalysisHover],
    semantic_entries: &[AnalysisSemanticEntry],
    target_path: &Path,
    position: &Position,
) -> Option<RenameTarget> {
    let match_entry = semantic_entry_at_position(session, semantic_entries, target_path, position)?;
    if !hovers
        .iter()
        .any(|hover| hover.span == match_entry.definition_span)
    {
        return None;
    }

    Some(RenameTarget {
        query_span: match_entry.span,
        definition_span: match_entry.definition_span,
        placeholder: span_text(session, match_entry.span)?,
    })
}

pub(super) fn find_definition_location(
    session: &kernc_utils::Session,
    semantic_entries: &[AnalysisSemanticEntry],
    target_path: &Path,
    position: &Position,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> Option<Location> {
    let definition_span =
        find_target_definition_span(session, semantic_entries, target_path, position)?;
    location_from_span(session, definition_span, uri_by_path)
}

pub(super) fn find_reference_locations(
    session: &kernc_utils::Session,
    semantic_entries: &[AnalysisSemanticEntry],
    target_path: &Path,
    position: &Position,
    include_declaration: bool,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> Vec<Location> {
    let Some(definition_span) =
        find_target_definition_span(session, semantic_entries, target_path, position)
    else {
        return Vec::new();
    };

    let mut locations = Vec::new();
    if include_declaration
        && let Some(location) = location_from_span(session, definition_span, uri_by_path)
    {
        locations.push(location);
    }

    for entry in semantic_entries {
        if entry.role != AnalysisSemanticRole::Reference || entry.definition_span != definition_span
        {
            continue;
        }

        if let Some(location) = location_from_span(session, entry.span, uri_by_path) {
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

pub(super) fn find_document_highlights(
    session: &kernc_utils::Session,
    semantic_entries: &[AnalysisSemanticEntry],
    hovers: &[AnalysisHover],
    target_path: &Path,
    position: &Position,
) -> Vec<DocumentHighlight> {
    let Some(definition_span) =
        find_target_definition_span(session, semantic_entries, target_path, position).or_else(
            || {
                hovers.iter().find_map(|hover| {
                    let file = session.source_manager.get_file(hover.span.file)?;
                    let offset = super::match_position_in_file(file, target_path, position)?;
                    super::span_contains_offset(hover.span, offset).then_some(hover.span)
                })
            },
        )
    else {
        return Vec::new();
    };

    let mut highlights = Vec::new();

    if super::span_in_path(session, definition_span, target_path) {
        highlights.push(DocumentHighlight {
            range: super::span_to_range(session, definition_span),
            kind: Some(1),
        });
    }

    for entry in semantic_entries {
        if entry.role != AnalysisSemanticRole::Reference
            || entry.definition_span != definition_span
            || !super::span_in_path(session, entry.span, target_path)
        {
            continue;
        }

        highlights.push(DocumentHighlight {
            range: super::span_to_range(session, entry.span),
            kind: Some(1),
        });
    }

    highlights.sort_by_key(|highlight| {
        (
            highlight.range.start.line,
            highlight.range.start.character,
            highlight.range.end.line,
            highlight.range.end.character,
        )
    });
    highlights.dedup_by(|left, right| left.range == right.range);
    highlights
}

fn find_target_definition_span(
    session: &kernc_utils::Session,
    semantic_entries: &[AnalysisSemanticEntry],
    target_path: &Path,
    position: &Position,
) -> Option<kernc_utils::Span> {
    semantic_entry_at_position(session, semantic_entries, target_path, position)
        .map(|entry| entry.definition_span)
}

pub(super) fn find_hover(
    session: &kernc_utils::Session,
    hovers: &[AnalysisHover],
    semantic_entries: &[AnalysisSemanticEntry],
    target_path: &Path,
    position: &Position,
) -> Option<Hover> {
    if let Some(hover) = hover_at_definition_position(session, hovers, target_path, position) {
        return Some(hover);
    }

    let definition_span =
        find_target_definition_span(session, semantic_entries, target_path, position)?;
    let hover = hovers.iter().find(|hover| hover.span == definition_span)?;
    Some(analysis_hover_to_lsp_hover(session, hover))
}

fn hover_at_definition_position(
    session: &kernc_utils::Session,
    hovers: &[AnalysisHover],
    target_path: &Path,
    position: &Position,
) -> Option<Hover> {
    let mut best_match = None;

    for hover in hovers {
        let Some(file) = session.source_manager.get_file(hover.span.file) else {
            continue;
        };
        let Some(offset) = super::match_position_in_file(file, target_path, position) else {
            continue;
        };
        if super::span_contains_offset(hover.span, offset) {
            let replace = best_match
                .map(|current: &AnalysisHover| {
                    let current_len = current.span.end.saturating_sub(current.span.start);
                    let next_len = hover.span.end.saturating_sub(hover.span.start);
                    next_len < current_len
                })
                .unwrap_or(true);
            if replace {
                best_match = Some(hover);
            }
        }
    }

    best_match.map(|hover| analysis_hover_to_lsp_hover(session, hover))
}

fn analysis_hover_to_lsp_hover(session: &kernc_utils::Session, hover: &AnalysisHover) -> Hover {
    Hover {
        contents: MarkupContent {
            kind: "markdown",
            value: hover.contents.clone(),
        },
        range: Some(super::span_to_range(session, hover.span)),
    }
}

pub(super) fn build_rename_changes(
    session: &kernc_utils::Session,
    semantic_entries: &[AnalysisSemanticEntry],
    definition_span: kernc_utils::Span,
    new_name: &str,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> BTreeMap<String, Vec<TextEdit>> {
    let mut edits_by_uri = BTreeMap::<String, Vec<TextEdit>>::new();

    if let Some(edit) = rename_edit_from_span(session, definition_span, new_name, uri_by_path) {
        edits_by_uri.entry(edit.0).or_default().push(edit.1);
    }

    for entry in semantic_entries {
        if entry.role != AnalysisSemanticRole::Reference || entry.definition_span != definition_span
        {
            continue;
        }

        if let Some(edit) = rename_edit_from_span(session, entry.span, new_name, uri_by_path) {
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
    session: &kernc_utils::Session,
    span: kernc_utils::Span,
    new_name: &str,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> Option<(String, TextEdit)> {
    let path = session.source_manager.get_file_path(span.file)?;
    let uri = uri_for_path(path, uri_by_path)?;
    Some((
        uri,
        TextEdit {
            range: super::span_to_range(session, span),
            new_text: new_name.to_string(),
        },
    ))
}

pub(super) fn analysis_completion_to_lsp_item(item: AnalysisCompletionItem) -> CompletionItem {
    let insert_text_format = item.insert_text.as_ref().map(|text| {
        if completion_insert_uses_snippet(text) {
            2
        } else {
            1
        }
    });
    CompletionItem {
        label: item.label,
        kind: lsp_completion_kind(item.kind),
        detail: item.detail,
        insert_text: item.insert_text,
        insert_text_format,
    }
}

fn completion_insert_uses_snippet(text: &str) -> bool {
    text.contains('$')
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

fn location_from_span(
    session: &kernc_utils::Session,
    span: kernc_utils::Span,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> Option<Location> {
    let path = session.source_manager.get_file_path(span.file)?;
    let uri = uri_for_path(path, uri_by_path)?;
    Some(Location {
        uri,
        range: super::span_to_range(session, span),
    })
}

fn uri_for_path(path: &Path, uri_by_path: &BTreeMap<PathBuf, String>) -> Option<String> {
    let normalized = super::normalize_path(path);
    if let Some(uri) = uri_by_path.get(&normalized) {
        return Some(uri.clone());
    }

    super::file_path_to_uri(path).ok()
}

fn span_text(session: &kernc_utils::Session, span: kernc_utils::Span) -> Option<String> {
    let file = session.source_manager.get_file(span.file)?;
    Some(file.src.get(span.start..span.end)?.to_string())
}

fn semantic_entry_at_position<'a>(
    session: &kernc_utils::Session,
    semantic_entries: &'a [AnalysisSemanticEntry],
    target_path: &Path,
    position: &Position,
) -> Option<&'a AnalysisSemanticEntry> {
    let mut best_match = None;

    for entry in semantic_entries {
        let Some(file) = session.source_manager.get_file(entry.span.file) else {
            continue;
        };
        let Some(offset) = super::match_position_in_file(file, target_path, position) else {
            continue;
        };
        if !super::span_contains_offset(entry.span, offset) {
            continue;
        }

        let replace = best_match
            .map(|current: &AnalysisSemanticEntry| {
                let current_len = current.span.end.saturating_sub(current.span.start);
                let next_len = entry.span.end.saturating_sub(entry.span.start);
                next_len < current_len
                    || (next_len == current_len
                        && entry.role == AnalysisSemanticRole::Reference
                        && current.role == AnalysisSemanticRole::Definition)
            })
            .unwrap_or(true);
        if replace {
            best_match = Some(entry);
        }
    }

    best_match
}
