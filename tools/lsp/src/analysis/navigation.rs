use super::RenameTarget;
use crate::protocol::{
    CompletionItem, DocumentHighlight, DocumentSymbol, Hover, Location, MarkupContent,
    ParameterInformation, Position, SignatureHelp, SignatureInformation, TextEdit,
};
use kernc_driver::{
    AnalysisCompletionItem, AnalysisCompletionKind, AnalysisHover, AnalysisReference,
    AnalysisSignatureHelp, AnalysisSymbol, AnalysisSymbolKind,
};
use std::collections::BTreeMap;
use std::path::Path;

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
        let Some(offset) = super::match_position_in_file(file, target_path, position) else {
            continue;
        };
        if !super::span_contains_offset(reference.reference_span, offset) {
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
    session: &kernc_utils::Session,
    hovers: &[AnalysisHover],
    target_path: &Path,
    position: &Position,
) -> Option<RenameTarget> {
    for hover in hovers {
        let Some(file) = session.source_manager.get_file(hover.span.file) else {
            continue;
        };
        let Some(offset) = super::match_position_in_file(file, target_path, position) else {
            continue;
        };
        if !super::span_contains_offset(hover.span, offset) {
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

pub(super) fn find_definition_location(
    session: &kernc_utils::Session,
    references: &[AnalysisReference],
    target_path: &Path,
    position: &Position,
) -> Option<Location> {
    let definition_span = find_target_definition_span(session, references, target_path, position)?;
    location_from_span(session, definition_span)
}

pub(super) fn find_reference_locations(
    session: &kernc_utils::Session,
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

pub(super) fn find_document_highlights(
    session: &kernc_utils::Session,
    references: &[AnalysisReference],
    hovers: &[AnalysisHover],
    target_path: &Path,
    position: &Position,
) -> Vec<DocumentHighlight> {
    let Some(definition_span) =
        find_target_definition_span(session, references, target_path, position).or_else(|| {
            hovers.iter().find_map(|hover| {
                let file = session.source_manager.get_file(hover.span.file)?;
                let offset = super::match_position_in_file(file, target_path, position)?;
                super::span_contains_offset(hover.span, offset).then_some(hover.span)
            })
        })
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

    for reference in references {
        if reference.definition_span != definition_span
            || !super::span_in_path(session, reference.reference_span, target_path)
        {
            continue;
        }

        highlights.push(DocumentHighlight {
            range: super::span_to_range(session, reference.reference_span),
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
    references: &[AnalysisReference],
    target_path: &Path,
    position: &Position,
) -> Option<kernc_utils::Span> {
    let mut best_match = None;

    for reference in references {
        let Some(reference_file) = session
            .source_manager
            .get_file(reference.reference_span.file)
        else {
            continue;
        };
        let reference_offset = super::match_position_in_file(reference_file, target_path, position);
        if let Some(offset) = reference_offset
            && super::span_contains_offset(reference.reference_span, offset)
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
        let definition_offset =
            super::match_position_in_file(definition_file, target_path, position);
        if let Some(offset) = definition_offset
            && super::span_contains_offset(reference.definition_span, offset)
        {
            best_match = Some(reference.definition_span);
            break;
        }
    }

    best_match
}

pub(super) fn find_hover(
    session: &kernc_utils::Session,
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
    session: &kernc_utils::Session,
    hovers: &[AnalysisHover],
    target_path: &Path,
    position: &Position,
) -> Option<Hover> {
    for hover in hovers {
        let Some(file) = session.source_manager.get_file(hover.span.file) else {
            continue;
        };
        let Some(offset) = super::match_position_in_file(file, target_path, position) else {
            continue;
        };
        if super::span_contains_offset(hover.span, offset) {
            return Some(analysis_hover_to_lsp_hover(session, hover));
        }
    }

    None
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
    references: &[AnalysisReference],
    definition_span: kernc_utils::Span,
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
    session: &kernc_utils::Session,
    span: kernc_utils::Span,
    new_name: &str,
) -> Option<(String, TextEdit)> {
    let path = session.source_manager.get_file_path(span.file)?;
    let uri = super::file_path_to_uri(path).ok()?;
    Some((
        uri,
        TextEdit {
            range: super::span_to_range(session, span),
            new_text: new_name.to_string(),
        },
    ))
}

pub(super) fn analysis_completion_to_lsp_item(item: AnalysisCompletionItem) -> CompletionItem {
    let insert_text_format = item.insert_text.as_ref().map(|_| 2);
    CompletionItem {
        label: item.label,
        kind: lsp_completion_kind(item.kind),
        detail: item.detail,
        insert_text: item.insert_text,
        insert_text_format,
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

fn location_from_span(session: &kernc_utils::Session, span: kernc_utils::Span) -> Option<Location> {
    let path = session.source_manager.get_file_path(span.file)?;
    let uri = super::file_path_to_uri(path).ok()?;
    Some(Location {
        uri,
        range: super::span_to_range(session, span),
    })
}

fn span_text(session: &kernc_utils::Session, span: kernc_utils::Span) -> Option<String> {
    let file = session.source_manager.get_file(span.file)?;
    Some(file.src.get(span.start..span.end)?.to_string())
}
