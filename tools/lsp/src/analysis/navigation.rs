//! Semantic navigation and symbol query helpers.
//!
//! Navigation converts compiler analysis artifacts into LSP-facing definitions,
//! references, hovers, rename edits, call hierarchy entries, document symbols,
//! workspace symbols, and inlay hints.

use super::ide::{
    IdeCallHierarchyIncomingCall, IdeCallHierarchyItem, IdeCallHierarchyOutgoingCall,
    IdeCompletionItem, IdeCompletionKind, IdeDocumentHighlight, IdeDocumentHighlightKind,
    IdeDocumentSymbol, IdeHover, IdeInlayHint, IdeInlayHintKind, IdeLocation,
    IdeParameterInformation, IdeSignatureHelp, IdeSignatureInformation, IdeSymbolKind, IdeTextEdit,
    IdeWorkspaceSymbol,
};
use super::{IdePosition, IdeRange, RenameBehavior, RenameTarget};
use kernc_driver::{
    AnalysisCall, AnalysisCallKind, AnalysisCompletionItem, AnalysisCompletionKind,
    AnalysisDefinitionLink, AnalysisHover, AnalysisSemanticEntry, AnalysisSemanticKind,
    AnalysisSemanticRole, AnalysisSignatureHelp, AnalysisSymbol, AnalysisSymbolKind,
    AnalysisTypeHint, AnalysisTypeHintKind,
};
use kernc_utils::CancellationToken;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub(super) struct ReferenceLocationQuery<'a> {
    pub session: &'a kernc_utils::Session,
    pub hovers: &'a [AnalysisHover],
    pub definition_links: &'a [AnalysisDefinitionLink],
    pub semantic_entries: &'a [AnalysisSemanticEntry],
    pub target_path: &'a Path,
    pub position: &'a IdePosition,
    pub include_declaration: bool,
    pub uri_by_path: &'a BTreeMap<PathBuf, String>,
}

pub(super) struct KnownReferenceLocationQuery<'a> {
    pub session: &'a kernc_utils::Session,
    pub definition_links: &'a [AnalysisDefinitionLink],
    pub semantic_entries: &'a [AnalysisSemanticEntry],
    pub definition_span: kernc_utils::Span,
    pub include_declaration: bool,
    pub uri_by_path: &'a BTreeMap<PathBuf, String>,
}

pub(super) fn analysis_symbol_to_document_symbol(
    session: &kernc_utils::Session,
    symbol: &AnalysisSymbol,
) -> IdeDocumentSymbol {
    IdeDocumentSymbol {
        name: symbol.name.clone(),
        detail: symbol.detail.clone(),
        kind: ide_symbol_kind(symbol.kind),
        range: super::span_to_range(session, symbol.span).into(),
        selection_range: super::span_to_range(session, symbol.selection_span).into(),
        children: symbol
            .children
            .iter()
            .map(|child| analysis_symbol_to_document_symbol(session, child))
            .collect(),
    }
}

pub(super) fn analysis_symbol_to_workspace_symbols_cancelable(
    session: &kernc_utils::Session,
    symbol: &AnalysisSymbol,
    container_name: Option<&str>,
    uri_by_path: &BTreeMap<PathBuf, String>,
    out: &mut Vec<IdeWorkspaceSymbol>,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    cancellation
        .check()
        .map_err(|_| "request was canceled".to_string())?;
    if !matches!(
        symbol.kind,
        AnalysisSymbolKind::Module | AnalysisSymbolKind::Namespace
    ) && let Some(location) = location_from_span(session, symbol.selection_span, uri_by_path)
    {
        out.push(IdeWorkspaceSymbol {
            name: symbol.name.clone(),
            kind: ide_symbol_kind(symbol.kind),
            location,
            container_name: container_name.map(str::to_string),
        });
    }

    for child in &symbol.children {
        analysis_symbol_to_workspace_symbols_cancelable(
            session,
            child,
            Some(symbol.name.as_str()),
            uri_by_path,
            out,
            cancellation,
        )?;
    }
    Ok(())
}

pub(super) fn analysis_signature_help_to_ide_help(help: AnalysisSignatureHelp) -> IdeSignatureHelp {
    IdeSignatureHelp {
        signatures: help
            .signatures
            .into_iter()
            .map(|signature| IdeSignatureInformation {
                label: signature.label,
                parameters: signature
                    .parameters
                    .into_iter()
                    .map(|parameter| IdeParameterInformation {
                        label: parameter.label,
                    })
                    .collect(),
            })
            .collect(),
        active_signature: help.active_signature as u32,
        active_parameter: help.active_parameter as u32,
    }
}

pub(super) fn analysis_type_hint_to_ide_hint(
    session: &kernc_utils::Session,
    hint: &AnalysisTypeHint,
) -> IdeInlayHint {
    let range = super::span_to_range(session, hint.span);
    let (position, padding_right) = match hint.kind {
        AnalysisTypeHintKind::ConstructorPrefix => (range.start, Some(false)),
        AnalysisTypeHintKind::Variable | AnalysisTypeHintKind::Expression => {
            (range.end, Some(true))
        }
    };
    IdeInlayHint {
        position: position.into(),
        label: hint.label.clone(),
        kind: Some(ide_inlay_hint_kind(hint.kind)),
        padding_left: Some(false),
        padding_right,
    }
}

pub(super) fn analysis_type_hint_to_ide_hint_for_source(
    session: &kernc_utils::Session,
    hint: &AnalysisTypeHint,
    source: &str,
) -> IdeInlayHint {
    let mut ide_hint = analysis_type_hint_to_ide_hint(session, hint);
    if hint.kind == AnalysisTypeHintKind::Variable
        && let Some(position) = variable_type_hint_position_in_source(session, hint, source)
    {
        ide_hint.position = position;
    }
    ide_hint
}

fn variable_type_hint_position_in_source(
    session: &kernc_utils::Session,
    hint: &AnalysisTypeHint,
    source: &str,
) -> Option<IdePosition> {
    let file = session.source_manager.get_file(hint.span.file)?;
    let compiler_source = file.src.as_ref();
    let span_text = compiler_source.get(hint.span.start..hint.span.end)?;
    if !is_identifier(span_text) {
        return None;
    }

    let mut search_start = hint.span.start.min(source.len());
    while search_start > 0 && !source.is_char_boundary(search_start) {
        search_start -= 1;
    }

    for offset in same_line_identifier_offsets(source, span_text, search_start) {
        return Some(super::byte_offset_to_position(
            &kernc_utils::SourceFile::new(file.path.clone(), source.to_string()),
            offset + span_text.len(),
        ));
    }

    None
}

fn same_line_identifier_offsets<'a>(
    source: &'a str,
    ident: &'a str,
    preferred_offset: usize,
) -> impl Iterator<Item = usize> + 'a {
    let line_start = source[..preferred_offset]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let line_end = source[preferred_offset..]
        .find('\n')
        .map(|index| preferred_offset + index)
        .unwrap_or(source.len());
    source[line_start..line_end]
        .match_indices(ident)
        .filter_map(move |(relative, _)| {
            let offset = line_start + relative;
            identifier_at(source, offset, ident.len()).then_some(offset)
        })
}

fn identifier_at(source: &str, offset: usize, len: usize) -> bool {
    let before = source[..offset].chars().next_back();
    let after = source[offset + len..].chars().next();
    !before.is_some_and(is_identifier_continue) && !after.is_some_and(is_identifier_continue)
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic()) && chars.all(is_identifier_continue)
}

fn is_identifier_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn ide_inlay_hint_kind(kind: AnalysisTypeHintKind) -> IdeInlayHintKind {
    match kind {
        AnalysisTypeHintKind::Variable
        | AnalysisTypeHintKind::Expression
        | AnalysisTypeHintKind::ConstructorPrefix => IdeInlayHintKind::Type,
    }
}

pub(super) fn find_rename_target(
    session: &kernc_utils::Session,
    hovers: &[AnalysisHover],
    semantic_entries: &[AnalysisSemanticEntry],
    target_path: &Path,
    position: &IdePosition,
) -> Option<RenameTarget> {
    let matching_entries =
        semantic_entries_at_position(session, semantic_entries, target_path, position);
    let match_entry = best_target_entry(session, hovers, &matching_entries)?;
    if !hovers
        .iter()
        .any(|hover| hover.span == match_entry.definition_span)
    {
        return None;
    }

    let behavior = rename_behavior_for_entry(session, match_entry, &matching_entries)?;

    Some(RenameTarget {
        query_span: match_entry.span,
        definition_span: match_entry.definition_span,
        placeholder: span_text(session, match_entry.span)?,
        behavior,
    })
}

pub(super) fn find_definition_location(
    session: &kernc_utils::Session,
    hovers: &[AnalysisHover],
    semantic_entries: &[AnalysisSemanticEntry],
    target_path: &Path,
    position: &IdePosition,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> Option<IdeLocation> {
    let definition_span =
        find_target_definition_span(session, hovers, semantic_entries, target_path, position)?;
    location_from_span(session, definition_span, uri_by_path)
}

pub(super) fn find_type_definition_location(
    session: &kernc_utils::Session,
    semantic_entries: &[AnalysisSemanticEntry],
    target_path: &Path,
    position: &IdePosition,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> Option<IdeLocation> {
    let matching_entries =
        semantic_entries_at_position(session, semantic_entries, target_path, position);
    let entry = best_semantic_entry(&matching_entries)?;
    if !semantic_kind_is_type_definition_target(entry.kind) {
        return None;
    }

    location_from_span(session, entry.definition_span, uri_by_path)
}

pub(super) fn find_implementation_locations(
    session: &kernc_utils::Session,
    hovers: &[AnalysisHover],
    definition_links: &[AnalysisDefinitionLink],
    semantic_entries: &[AnalysisSemanticEntry],
    target_path: &Path,
    position: &IdePosition,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> Vec<IdeLocation> {
    let Some(definition_span) =
        find_target_definition_span(session, hovers, semantic_entries, target_path, position)
    else {
        return Vec::new();
    };

    let mut locations = Vec::new();
    for link in definition_links {
        if link.definition_span != definition_span || link.linked_definition_span == definition_span
        {
            continue;
        }
        if let Some(location) =
            location_from_span(session, link.linked_definition_span, uri_by_path)
        {
            locations.push(location);
        }
    }

    locations.sort_by_key(location_order_key);
    locations.dedup();
    locations
}

pub(super) fn find_call_hierarchy_item(
    session: &kernc_utils::Session,
    hovers: &[AnalysisHover],
    semantic_entries: &[AnalysisSemanticEntry],
    calls: &[AnalysisCall],
    target_path: &Path,
    position: &IdePosition,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> Option<IdeCallHierarchyItem> {
    let definition_span =
        find_target_definition_span(session, hovers, semantic_entries, target_path, position)
            .or_else(|| {
                hovers.iter().find_map(|hover| {
                    let file = session.source_manager.get_file(hover.span.file)?;
                    let offset = super::match_position_in_file(file, target_path, position)?;
                    super::span_contains_offset(hover.span, offset).then_some(hover.span)
                })
            })?;
    let callable_value_targets = callable_value_targets(calls);
    call_hierarchy_item_for_definition(
        session,
        semantic_entries,
        definition_span,
        uri_by_path,
        &callable_value_targets,
    )
}

pub(super) fn find_call_hierarchy_incoming_calls(
    session: &kernc_utils::Session,
    semantic_entries: &[AnalysisSemanticEntry],
    calls: &[AnalysisCall],
    target_uri: &str,
    target_range: &IdeRange,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> Vec<IdeCallHierarchyIncomingCall> {
    let Some(target_span) = span_for_ide_range(session, target_uri, target_range, uri_by_path)
    else {
        return Vec::new();
    };
    let callable_value_targets = callable_value_targets(calls);
    let Some(target_entry) = call_hierarchy_definition_entry(semantic_entries, target_span) else {
        return Vec::new();
    };
    if !call_hierarchy_entry_is_supported(target_entry, &callable_value_targets) {
        return Vec::new();
    }

    let mut grouped = BTreeMap::<kernc_utils::Span, (IdeCallHierarchyItem, Vec<IdeRange>)>::new();
    for call in calls.iter().filter(|call| match call.kind {
        AnalysisCallKind::Direct => {
            call.callee_definition_span == Some(target_entry.definition_span)
        }
        AnalysisCallKind::DynamicDispatch => {
            call.callee_definition_span == Some(target_entry.definition_span)
                || call
                    .dynamic_dispatch_targets
                    .contains(&target_entry.definition_span)
        }
        AnalysisCallKind::Indirect => call
            .indirect_targets
            .contains(&target_entry.definition_span),
    }) {
        let Some(from) = call_hierarchy_item_for_definition(
            session,
            semantic_entries,
            call.caller_definition_span,
            uri_by_path,
            &callable_value_targets,
        ) else {
            continue;
        };
        grouped
            .entry(call.caller_definition_span)
            .or_insert((from, Vec::new()))
            .1
            .push(super::span_to_range(session, call.callee_span).into());
    }

    grouped
        .into_values()
        .map(|(from, mut from_ranges)| {
            from_ranges.sort_by_key(range_order_key);
            from_ranges.dedup();
            IdeCallHierarchyIncomingCall { from, from_ranges }
        })
        .collect()
}

pub(super) fn find_call_hierarchy_outgoing_calls(
    session: &kernc_utils::Session,
    semantic_entries: &[AnalysisSemanticEntry],
    calls: &[AnalysisCall],
    target_uri: &str,
    target_range: &IdeRange,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> Vec<IdeCallHierarchyOutgoingCall> {
    let Some(target_span) = span_for_ide_range(session, target_uri, target_range, uri_by_path)
    else {
        return Vec::new();
    };
    let callable_value_targets = callable_value_targets(calls);
    let Some(target_entry) = call_hierarchy_definition_entry(semantic_entries, target_span) else {
        return Vec::new();
    };
    if !call_hierarchy_entry_is_supported(target_entry, &callable_value_targets) {
        return Vec::new();
    }

    let mut grouped = BTreeMap::<kernc_utils::Span, (IdeCallHierarchyItem, Vec<IdeRange>)>::new();
    for call in calls
        .iter()
        .filter(|call| call.caller_definition_span == target_entry.definition_span)
    {
        match call.kind {
            AnalysisCallKind::Direct => {
                let Some(callee_definition_span) = call.callee_definition_span else {
                    continue;
                };
                add_outgoing_call_target(
                    session,
                    semantic_entries,
                    uri_by_path,
                    &mut grouped,
                    callee_definition_span,
                    call.callee_span,
                    &callable_value_targets,
                );
            }
            AnalysisCallKind::DynamicDispatch => {
                for target in &call.dynamic_dispatch_targets {
                    add_outgoing_call_target(
                        session,
                        semantic_entries,
                        uri_by_path,
                        &mut grouped,
                        *target,
                        call.callee_span,
                        &callable_value_targets,
                    );
                }
            }
            AnalysisCallKind::Indirect => {
                for target in &call.indirect_targets {
                    add_outgoing_call_target(
                        session,
                        semantic_entries,
                        uri_by_path,
                        &mut grouped,
                        *target,
                        call.callee_span,
                        &callable_value_targets,
                    );
                }
            }
        }
    }

    grouped
        .into_values()
        .map(|(to, mut from_ranges)| {
            from_ranges.sort_by_key(range_order_key);
            from_ranges.dedup();
            IdeCallHierarchyOutgoingCall { to, from_ranges }
        })
        .collect()
}

fn add_outgoing_call_target(
    session: &kernc_utils::Session,
    semantic_entries: &[AnalysisSemanticEntry],
    uri_by_path: &BTreeMap<PathBuf, String>,
    grouped: &mut BTreeMap<kernc_utils::Span, (IdeCallHierarchyItem, Vec<IdeRange>)>,
    callee_definition_span: kernc_utils::Span,
    callee_span: kernc_utils::Span,
    callable_value_targets: &BTreeSet<kernc_utils::Span>,
) {
    let Some(to) = call_hierarchy_item_for_definition(
        session,
        semantic_entries,
        callee_definition_span,
        uri_by_path,
        callable_value_targets,
    ) else {
        return;
    };
    grouped
        .entry(callee_definition_span)
        .or_insert((to, Vec::new()))
        .1
        .push(super::span_to_range(session, callee_span).into());
}

fn semantic_kind_is_type_definition_target(kind: AnalysisSemanticKind) -> bool {
    matches!(
        kind,
        AnalysisSemanticKind::Module
            | AnalysisSemanticKind::Namespace
            | AnalysisSemanticKind::Struct
            | AnalysisSemanticKind::Enum
            | AnalysisSemanticKind::Interface
            | AnalysisSemanticKind::Type
            | AnalysisSemanticKind::TypeParameter
    )
}

pub(super) fn find_reference_locations_cancelable(
    query: ReferenceLocationQuery<'_>,
    cancellation: &CancellationToken,
) -> Result<Vec<IdeLocation>, String> {
    let Some(definition_span) = find_target_definition_span(
        query.session,
        query.hovers,
        query.semantic_entries,
        query.target_path,
        query.position,
    ) else {
        return Ok(Vec::new());
    };
    let definition_spans = rename_definition_span_group(definition_span, query.definition_links);

    let mut locations = Vec::new();
    if query.include_declaration {
        for definition_span in &definition_spans {
            cancellation
                .check()
                .map_err(|_| "request was canceled".to_string())?;
            if let Some(location) =
                location_from_span(query.session, *definition_span, query.uri_by_path)
            {
                locations.push(location);
            }
        }
    }

    for entry in query.semantic_entries {
        cancellation
            .check()
            .map_err(|_| "request was canceled".to_string())?;
        if entry.role != AnalysisSemanticRole::Reference
            || !definition_spans.contains(&entry.definition_span)
        {
            continue;
        }

        if let Some(location) = location_from_span(query.session, entry.span, query.uri_by_path) {
            locations.push(location);
        }
    }

    locations.sort_by_key(location_order_key);
    locations.dedup();
    Ok(locations)
}

pub(super) fn find_reference_locations_for_definition_cancelable(
    query: KnownReferenceLocationQuery<'_>,
    cancellation: &CancellationToken,
) -> Result<Vec<IdeLocation>, String> {
    let definition_spans =
        rename_definition_span_group(query.definition_span, query.definition_links);

    let mut locations = Vec::new();
    if query.include_declaration {
        for definition_span in &definition_spans {
            cancellation
                .check()
                .map_err(|_| "request was canceled".to_string())?;
            if let Some(location) =
                location_from_span(query.session, *definition_span, query.uri_by_path)
            {
                locations.push(location);
            }
        }
    }

    for entry in query.semantic_entries {
        cancellation
            .check()
            .map_err(|_| "request was canceled".to_string())?;
        if entry.role != AnalysisSemanticRole::Reference
            || !definition_spans.contains(&entry.definition_span)
        {
            continue;
        }

        if let Some(location) = location_from_span(query.session, entry.span, query.uri_by_path) {
            locations.push(location);
        }
    }

    locations.sort_by_key(location_order_key);
    locations.dedup();
    Ok(locations)
}

fn location_order_key(location: &IdeLocation) -> (String, u32, u32, u32, u32) {
    let range = &location.range;
    (
        location.uri.clone(),
        range.start.line,
        range.start.character,
        range.end.line,
        range.end.character,
    )
}

pub(super) fn find_document_highlights(
    session: &kernc_utils::Session,
    definition_links: &[AnalysisDefinitionLink],
    semantic_entries: &[AnalysisSemanticEntry],
    hovers: &[AnalysisHover],
    target_path: &Path,
    position: &IdePosition,
) -> Vec<IdeDocumentHighlight> {
    let Some(definition_span) =
        find_target_definition_span(session, hovers, semantic_entries, target_path, position)
            .or_else(|| {
                hovers.iter().find_map(|hover| {
                    let file = session.source_manager.get_file(hover.span.file)?;
                    let offset = super::match_position_in_file(file, target_path, position)?;
                    super::span_contains_offset(hover.span, offset).then_some(hover.span)
                })
            })
    else {
        return Vec::new();
    };
    let definition_spans = rename_definition_span_group(definition_span, definition_links);

    let mut highlights = Vec::new();

    for definition_span in &definition_spans {
        if !super::span_in_path(session, *definition_span, target_path) {
            continue;
        }
        highlights.push(IdeDocumentHighlight {
            range: super::span_to_range(session, *definition_span).into(),
            kind: Some(IdeDocumentHighlightKind::Text),
        });
    }

    for entry in semantic_entries {
        if entry.role != AnalysisSemanticRole::Reference
            || !definition_spans.contains(&entry.definition_span)
            || !super::span_in_path(session, entry.span, target_path)
        {
            continue;
        }

        highlights.push(IdeDocumentHighlight {
            range: super::span_to_range(session, entry.span).into(),
            kind: Some(IdeDocumentHighlightKind::Text),
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
    hovers: &[AnalysisHover],
    semantic_entries: &[AnalysisSemanticEntry],
    target_path: &Path,
    position: &IdePosition,
) -> Option<kernc_utils::Span> {
    let matching_entries =
        semantic_entries_at_position(session, semantic_entries, target_path, position);
    best_target_entry(session, hovers, &matching_entries).map(|entry| entry.definition_span)
}

pub(super) fn navigation_definition_span_for_position(
    session: &kernc_utils::Session,
    hovers: &[AnalysisHover],
    semantic_entries: &[AnalysisSemanticEntry],
    target_path: &Path,
    position: &IdePosition,
) -> Option<kernc_utils::Span> {
    find_target_definition_span(session, hovers, semantic_entries, target_path, position)
}

pub(super) fn find_hover(
    session: &kernc_utils::Session,
    hovers: &[AnalysisHover],
    semantic_entries: &[AnalysisSemanticEntry],
    target_path: &Path,
    position: &IdePosition,
) -> Option<IdeHover> {
    let matching_entries =
        semantic_entries_at_position(session, semantic_entries, target_path, position);

    if let Some(entry) = best_target_entry(session, hovers, &matching_entries)
        && let Some(hover) =
            hover_for_definition_span_with_range(session, hovers, entry.definition_span, entry.span)
    {
        return Some(hover);
    }

    if let Some(hover) = hover_at_definition_position(session, hovers, target_path, position) {
        return Some(hover);
    }

    None
}

fn hover_for_definition_span(
    session: &kernc_utils::Session,
    hovers: &[AnalysisHover],
    definition_span: kernc_utils::Span,
) -> Option<IdeHover> {
    hover_for_definition_span_with_range(session, hovers, definition_span, definition_span)
}

fn hover_for_definition_span_with_range(
    session: &kernc_utils::Session,
    hovers: &[AnalysisHover],
    definition_span: kernc_utils::Span,
    display_span: kernc_utils::Span,
) -> Option<IdeHover> {
    let hover = hovers.iter().find(|hover| hover.span == definition_span)?;
    Some(analysis_hover_to_ide_hover_with_range(
        session,
        hover,
        display_span,
    ))
}

fn hover_at_definition_position(
    session: &kernc_utils::Session,
    hovers: &[AnalysisHover],
    target_path: &Path,
    position: &IdePosition,
) -> Option<IdeHover> {
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

    best_match.map(|hover| analysis_hover_to_ide_hover(session, hover))
}

fn analysis_hover_to_ide_hover(session: &kernc_utils::Session, hover: &AnalysisHover) -> IdeHover {
    analysis_hover_to_ide_hover_with_range(session, hover, hover.span)
}

fn analysis_hover_to_ide_hover_with_range(
    session: &kernc_utils::Session,
    hover: &AnalysisHover,
    range_span: kernc_utils::Span,
) -> IdeHover {
    IdeHover {
        contents: hover.contents.clone(),
        range: Some(super::span_to_range(session, range_span).into()),
    }
}

pub(super) fn build_rename_changes(
    session: &kernc_utils::Session,
    definition_links: &[AnalysisDefinitionLink],
    semantic_entries: &[AnalysisSemanticEntry],
    target: &RenameTarget,
    new_name: &str,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> BTreeMap<String, Vec<IdeTextEdit>> {
    let mut edits_by_uri = BTreeMap::<String, Vec<IdeTextEdit>>::new();
    let definition_spans = rename_definition_span_group(target.definition_span, definition_links);

    let definition_edit = match &target.behavior {
        RenameBehavior::Standard => None,
        RenameBehavior::ExpandPatternPun { field_name } => rename_edit_from_span(
            session,
            target.query_span,
            &format!("{field_name}: {new_name}"),
            uri_by_path,
        ),
    };

    if matches!(target.behavior, RenameBehavior::Standard) {
        for definition_span in &definition_spans {
            if let Some(edit) =
                rename_edit_from_span(session, *definition_span, new_name, uri_by_path)
            {
                edits_by_uri.entry(edit.0).or_default().push(edit.1);
            }
        }
    } else if let Some(edit) = definition_edit {
        edits_by_uri.entry(edit.0).or_default().push(edit.1);
    }

    for entry in semantic_entries {
        if entry.role != AnalysisSemanticRole::Reference
            || !definition_spans.contains(&entry.definition_span)
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

fn rename_definition_span_group(
    root: kernc_utils::Span,
    definition_links: &[AnalysisDefinitionLink],
) -> BTreeSet<kernc_utils::Span> {
    let mut visited = BTreeSet::new();
    let mut worklist = vec![root];

    while let Some(span) = worklist.pop() {
        if !visited.insert(span) {
            continue;
        }

        for link in definition_links {
            if link.definition_span == span && !visited.contains(&link.linked_definition_span) {
                worklist.push(link.linked_definition_span);
            }
        }
    }

    visited
}

fn rename_edit_from_span(
    session: &kernc_utils::Session,
    span: kernc_utils::Span,
    new_name: &str,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> Option<(String, IdeTextEdit)> {
    let path = session.source_manager.get_file_path(span.file)?;
    let uri = uri_for_path(path, uri_by_path)?;
    Some((
        uri,
        IdeTextEdit {
            range: super::span_to_range(session, span).into(),
            new_text: new_name.to_string(),
        },
    ))
}

pub(super) fn analysis_completion_to_ide_item(item: AnalysisCompletionItem) -> IdeCompletionItem {
    IdeCompletionItem {
        label: item.label,
        kind: ide_completion_kind(item.kind),
        detail: item.detail,
        insert_text: item.insert_text,
        documentation: item.documentation,
        resolve_data: None,
    }
}

fn ide_completion_kind(kind: AnalysisCompletionKind) -> IdeCompletionKind {
    match kind {
        AnalysisCompletionKind::Variable => IdeCompletionKind::Variable,
        AnalysisCompletionKind::Function => IdeCompletionKind::Function,
        AnalysisCompletionKind::Module => IdeCompletionKind::Module,
        AnalysisCompletionKind::Struct => IdeCompletionKind::Struct,
        AnalysisCompletionKind::Union => IdeCompletionKind::Union,
        AnalysisCompletionKind::Enum => IdeCompletionKind::Enum,
        AnalysisCompletionKind::Trait => IdeCompletionKind::Trait,
        AnalysisCompletionKind::TypeAlias => IdeCompletionKind::TypeAlias,
        AnalysisCompletionKind::Constant => IdeCompletionKind::Constant,
        AnalysisCompletionKind::Static => IdeCompletionKind::Static,
        AnalysisCompletionKind::TypeParameter => IdeCompletionKind::TypeParameter,
    }
}

fn ide_symbol_kind(kind: AnalysisSymbolKind) -> IdeSymbolKind {
    match kind {
        AnalysisSymbolKind::Module => IdeSymbolKind::Module,
        AnalysisSymbolKind::Namespace => IdeSymbolKind::Namespace,
        AnalysisSymbolKind::Struct | AnalysisSymbolKind::Union => IdeSymbolKind::Struct,
        AnalysisSymbolKind::Trait => IdeSymbolKind::Trait,
        AnalysisSymbolKind::Method => IdeSymbolKind::Method,
        AnalysisSymbolKind::Function => IdeSymbolKind::Function,
        AnalysisSymbolKind::Enum => IdeSymbolKind::Enum,
        AnalysisSymbolKind::TypeAlias => IdeSymbolKind::TypeAlias,
        AnalysisSymbolKind::Constant => IdeSymbolKind::Constant,
        AnalysisSymbolKind::Static => IdeSymbolKind::Static,
    }
}

fn location_from_span(
    session: &kernc_utils::Session,
    span: kernc_utils::Span,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> Option<IdeLocation> {
    let path = session.source_manager.get_file_path(span.file)?;
    let uri = uri_for_path(path, uri_by_path)?;
    Some(IdeLocation {
        uri,
        range: super::span_to_range(session, span).into(),
    })
}

fn uri_for_path(path: &Path, uri_by_path: &BTreeMap<PathBuf, String>) -> Option<String> {
    let normalized = super::normalize_path(path);
    if let Some(uri) = uri_by_path.get(&normalized) {
        return Some(uri.clone());
    }

    super::file_path_to_uri(path).ok()
}

fn call_hierarchy_item_for_definition(
    session: &kernc_utils::Session,
    semantic_entries: &[AnalysisSemanticEntry],
    definition_span: kernc_utils::Span,
    uri_by_path: &BTreeMap<PathBuf, String>,
    callable_value_targets: &BTreeSet<kernc_utils::Span>,
) -> Option<IdeCallHierarchyItem> {
    let entry = call_hierarchy_definition_entry(semantic_entries, definition_span)?;
    if !call_hierarchy_entry_is_supported(entry, callable_value_targets) {
        return None;
    }
    let path = session.source_manager.get_file_path(entry.span.file)?;
    let uri = uri_for_path(path, uri_by_path)?;
    Some(IdeCallHierarchyItem {
        name: span_text(session, entry.span)?,
        kind: match entry.kind {
            AnalysisSemanticKind::Method => IdeSymbolKind::Method,
            AnalysisSemanticKind::Function => IdeSymbolKind::Function,
            AnalysisSemanticKind::Variable | AnalysisSemanticKind::Parameter => {
                IdeSymbolKind::Variable
            }
            _ => return None,
        },
        uri,
        range: super::span_to_range(session, entry.span).into(),
        selection_range: super::span_to_range(session, entry.span).into(),
    })
}

fn call_hierarchy_entry_is_supported(
    entry: &AnalysisSemanticEntry,
    callable_value_targets: &BTreeSet<kernc_utils::Span>,
) -> bool {
    matches!(
        entry.kind,
        AnalysisSemanticKind::Function | AnalysisSemanticKind::Method
    ) || (matches!(
        entry.kind,
        AnalysisSemanticKind::Variable | AnalysisSemanticKind::Parameter
    ) && callable_value_targets.contains(&entry.definition_span))
}

fn callable_value_targets(calls: &[AnalysisCall]) -> BTreeSet<kernc_utils::Span> {
    calls
        .iter()
        .flat_map(|call| call.indirect_targets.iter().copied())
        .collect()
}

fn call_hierarchy_definition_entry(
    semantic_entries: &[AnalysisSemanticEntry],
    definition_span: kernc_utils::Span,
) -> Option<&AnalysisSemanticEntry> {
    semantic_entries.iter().find(|entry| {
        entry.role == AnalysisSemanticRole::Definition && entry.definition_span == definition_span
    })
}

fn span_for_ide_range(
    session: &kernc_utils::Session,
    uri: &str,
    range: &IdeRange,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> Option<kernc_utils::Span> {
    let path = uri_by_path
        .iter()
        .find_map(|(path, open_uri)| (open_uri == uri).then_some(path.clone()))
        .or_else(|| super::uri_to_file_path(uri).map(|path| super::normalize_path(&path)))?;
    let (file_id, file) =
        session
            .source_manager
            .files()
            .iter()
            .enumerate()
            .find(|(index, _file)| {
                let file_id = kernc_utils::FileId(*index);
                session
                    .source_manager
                    .get_file_path(file_id)
                    .is_some_and(|file_path| super::normalize_path(file_path) == path)
            })?;
    let file_id = kernc_utils::FileId(file_id);
    let start = super::position_to_byte_offset(file, &range.start)?;
    let end = super::position_to_byte_offset(file, &range.end)?;
    Some(kernc_utils::Span {
        file: file_id,
        start,
        end,
    })
}

fn range_order_key(range: &IdeRange) -> (u32, u32, u32, u32) {
    (
        range.start.line,
        range.start.character,
        range.end.line,
        range.end.character,
    )
}

fn span_text(session: &kernc_utils::Session, span: kernc_utils::Span) -> Option<String> {
    let file = session.source_manager.get_file(span.file)?;
    Some(file.src.get(span.start..span.end)?.to_string())
}

fn best_semantic_entry<'a>(
    matching_entries: &[&'a AnalysisSemanticEntry],
) -> Option<&'a AnalysisSemanticEntry> {
    let mut best_match = None;

    for entry in matching_entries {
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

fn best_target_entry<'a>(
    session: &kernc_utils::Session,
    hovers: &[AnalysisHover],
    matching_entries: &[&'a AnalysisSemanticEntry],
) -> Option<&'a AnalysisSemanticEntry> {
    let semantic = best_semantic_entry(matching_entries).filter(|entry| {
        hover_for_definition_span(session, hovers, entry.definition_span).is_some()
    });
    let definition = best_definition_entry_with_hover(session, hovers, matching_entries);

    match (definition, semantic) {
        (Some(definition), Some(semantic))
            if span_len(definition.span) <= span_len(semantic.span) =>
        {
            Some(definition)
        }
        (Some(_), Some(semantic)) => Some(semantic),
        (Some(definition), None) => Some(definition),
        (None, Some(semantic)) => Some(semantic),
        (None, None) => None,
    }
}

fn span_len(span: kernc_utils::Span) -> usize {
    span.end.saturating_sub(span.start)
}

fn semantic_entries_at_position<'a>(
    session: &kernc_utils::Session,
    semantic_entries: &'a [AnalysisSemanticEntry],
    target_path: &Path,
    position: &IdePosition,
) -> Vec<&'a AnalysisSemanticEntry> {
    let mut matching_entries = Vec::new();

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

        matching_entries.push(entry);
    }

    matching_entries
}

fn rename_behavior_for_entry(
    session: &kernc_utils::Session,
    match_entry: &AnalysisSemanticEntry,
    matching_entries: &[&AnalysisSemanticEntry],
) -> Option<RenameBehavior> {
    if !matches!(
        match_entry.kind,
        AnalysisSemanticKind::Variable | AnalysisSemanticKind::Parameter
    ) {
        return Some(RenameBehavior::Standard);
    }

    let colliding_reference = matching_entries.iter().copied().find(|entry| {
        entry.role == AnalysisSemanticRole::Reference
            && entry.span == match_entry.span
            && entry.definition_span != match_entry.definition_span
    });
    let Some(colliding_reference) = colliding_reference else {
        return Some(RenameBehavior::Standard);
    };

    let field_name = span_text(session, colliding_reference.definition_span)?;
    Some(RenameBehavior::ExpandPatternPun { field_name })
}

fn best_definition_entry_with_hover<'a>(
    session: &kernc_utils::Session,
    hovers: &[AnalysisHover],
    matching_entries: &[&'a AnalysisSemanticEntry],
) -> Option<&'a AnalysisSemanticEntry> {
    let mut best_match = None;

    for entry in matching_entries {
        if entry.role != AnalysisSemanticRole::Definition {
            continue;
        }
        if hover_for_definition_span(session, hovers, entry.span).is_none() {
            continue;
        }

        let replace = best_match
            .map(|current: &AnalysisSemanticEntry| {
                let current_len = current.span.end.saturating_sub(current.span.start);
                let next_len = entry.span.end.saturating_sub(entry.span.start);
                next_len < current_len
            })
            .unwrap_or(true);
        if replace {
            best_match = Some(*entry);
        }
    }

    best_match
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernc_driver::{
        AnalysisDefinitionLink, AnalysisHover, AnalysisSemanticEntry, AnalysisSemanticKind,
        AnalysisSemanticRole, AnalysisSymbolKind,
    };
    use kernc_utils::{FileId, Session, Span};

    fn span(start: usize, end: usize) -> Span {
        Span {
            file: FileId(0),
            start,
            end,
        }
    }

    fn session_with_source(source: &str) -> Session {
        let mut session = Session::new();
        session
            .source_manager
            .add_file("navigation_cancel.kn".to_string(), source.to_string());
        session
    }

    #[test]
    fn workspace_symbol_recursion_observes_cancellation() {
        let session = session_with_source("fn root() void {}\n");
        let child = AnalysisSymbol {
            name: "child".to_string(),
            kind: AnalysisSymbolKind::Function,
            span: span(0, 4),
            selection_span: span(0, 4),
            detail: None,
            children: Vec::new(),
        };
        let symbol = AnalysisSymbol {
            name: "root".to_string(),
            kind: AnalysisSymbolKind::Function,
            span: span(0, 4),
            selection_span: span(0, 4),
            detail: None,
            children: vec![child; 12],
        };
        let cancellation = CancellationToken::with_check_budget_for_testing(4);
        let mut out = Vec::new();

        let result = analysis_symbol_to_workspace_symbols_cancelable(
            &session,
            &symbol,
            None,
            &BTreeMap::new(),
            &mut out,
            &cancellation,
        );

        assert_eq!(result.unwrap_err(), "request was canceled");
        assert!(cancellation.is_canceled());
    }

    #[test]
    fn reference_location_scan_observes_cancellation() {
        let session = session_with_source("target target target target target\n");
        let definition_span = span(0, 6);
        let hovers = vec![AnalysisHover {
            span: definition_span,
            contents: "```kern\nfn target() void\n```".to_string(),
        }];
        let semantic_entries = (0..12)
            .map(|index| AnalysisSemanticEntry {
                span: span(index, index + 1),
                definition_span,
                kind: AnalysisSemanticKind::Function,
                role: if index == 0 {
                    AnalysisSemanticRole::Definition
                } else {
                    AnalysisSemanticRole::Reference
                },
                is_mut: false,
                is_pub: false,
            })
            .collect::<Vec<_>>();
        let target_path = session.source_manager.get_file_path(FileId(0)).unwrap();
        let position = IdePosition {
            line: 0,
            character: 1,
        };
        let cancellation = CancellationToken::with_check_budget_for_testing(6);

        let result = find_reference_locations_cancelable(
            ReferenceLocationQuery {
                session: &session,
                hovers: &hovers,
                definition_links: &[],
                semantic_entries: &semantic_entries,
                target_path,
                position: &position,
                include_declaration: true,
                uri_by_path: &BTreeMap::new(),
            },
            &cancellation,
        );

        assert_eq!(result.unwrap_err(), "request was canceled");
        assert!(cancellation.is_canceled());
    }

    #[test]
    fn known_reference_location_scan_observes_cancellation() {
        let session = session_with_source("target target target target target\n");
        let definition_span = span(0, 6);
        let semantic_entries = (0..12)
            .map(|index| AnalysisSemanticEntry {
                span: span(index, index + 1),
                definition_span,
                kind: AnalysisSemanticKind::Function,
                role: AnalysisSemanticRole::Reference,
                is_mut: false,
                is_pub: false,
            })
            .collect::<Vec<_>>();
        let cancellation = CancellationToken::with_check_budget_for_testing(4);

        let result = find_reference_locations_for_definition_cancelable(
            KnownReferenceLocationQuery {
                session: &session,
                definition_links: &[AnalysisDefinitionLink {
                    definition_span,
                    linked_definition_span: definition_span,
                }],
                semantic_entries: &semantic_entries,
                definition_span,
                include_declaration: true,
                uri_by_path: &BTreeMap::new(),
            },
            &cancellation,
        );

        assert_eq!(result.unwrap_err(), "request was canceled");
        assert!(cancellation.is_canceled());
    }
}
