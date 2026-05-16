use super::ide::{
    IdeCallHierarchyIncomingCall, IdeCallHierarchyItem, IdeCallHierarchyOutgoingCall,
    IdeCompletionItem, IdeCompletionKind, IdeDocumentHighlight, IdeDocumentHighlightKind,
    IdeDocumentSymbol, IdeHover, IdeInlayHint, IdeInlayHintKind, IdeLocation,
    IdeParameterInformation, IdeSignatureHelp, IdeSignatureInformation, IdeSymbolKind, IdeTextEdit,
    IdeWorkspaceSymbol,
};
use super::{RenameBehavior, RenameTarget};
use crate::protocol::{Position, Range};
use kernc_driver::{
    AnalysisCall, AnalysisCallKind, AnalysisCompletionItem, AnalysisCompletionKind,
    AnalysisDefinitionLink, AnalysisHover, AnalysisSemanticEntry, AnalysisSemanticKind,
    AnalysisSemanticRole, AnalysisSignatureHelp, AnalysisSymbol, AnalysisSymbolKind,
    AnalysisTypeHint, AnalysisTypeHintKind,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub(super) struct ReferenceLocationQuery<'a> {
    pub session: &'a kernc_utils::Session,
    pub hovers: &'a [AnalysisHover],
    pub definition_links: &'a [AnalysisDefinitionLink],
    pub semantic_entries: &'a [AnalysisSemanticEntry],
    pub target_path: &'a Path,
    pub position: &'a Position,
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
        range: super::span_to_range(session, symbol.span),
        selection_range: super::span_to_range(session, symbol.selection_span),
        children: symbol
            .children
            .iter()
            .map(|child| analysis_symbol_to_document_symbol(session, child))
            .collect(),
    }
}

pub(super) fn analysis_symbol_to_workspace_symbols(
    session: &kernc_utils::Session,
    symbol: &AnalysisSymbol,
    container_name: Option<&str>,
    uri_by_path: &BTreeMap<PathBuf, String>,
    out: &mut Vec<IdeWorkspaceSymbol>,
) {
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
        analysis_symbol_to_workspace_symbols(
            session,
            child,
            Some(symbol.name.as_str()),
            uri_by_path,
            out,
        );
    }
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
        position,
        label: hint.label.clone(),
        kind: Some(ide_inlay_hint_kind(hint.kind)),
        padding_left: Some(false),
        padding_right,
    }
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
    position: &Position,
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
    position: &Position,
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
    position: &Position,
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
    position: &Position,
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
    target_path: &Path,
    position: &Position,
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
    call_hierarchy_item_for_definition(session, semantic_entries, definition_span, uri_by_path)
}

pub(super) fn find_call_hierarchy_incoming_calls(
    session: &kernc_utils::Session,
    semantic_entries: &[AnalysisSemanticEntry],
    calls: &[AnalysisCall],
    target_uri: &str,
    target_range: &Range,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> Vec<IdeCallHierarchyIncomingCall> {
    let Some(target_span) = span_for_lsp_range(session, target_uri, target_range, uri_by_path)
    else {
        return Vec::new();
    };
    let Some(target_entry) = call_hierarchy_definition_entry(semantic_entries, target_span) else {
        return Vec::new();
    };

    let mut grouped = BTreeMap::<kernc_utils::Span, (IdeCallHierarchyItem, Vec<Range>)>::new();
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
        AnalysisCallKind::Indirect => false,
    }) {
        let Some(from) = call_hierarchy_item_for_definition(
            session,
            semantic_entries,
            call.caller_definition_span,
            uri_by_path,
        ) else {
            continue;
        };
        grouped
            .entry(call.caller_definition_span)
            .or_insert((from, Vec::new()))
            .1
            .push(super::span_to_range(session, call.callee_span));
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
    target_range: &Range,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> Vec<IdeCallHierarchyOutgoingCall> {
    let Some(target_span) = span_for_lsp_range(session, target_uri, target_range, uri_by_path)
    else {
        return Vec::new();
    };
    let Some(target_entry) = call_hierarchy_definition_entry(semantic_entries, target_span) else {
        return Vec::new();
    };

    let mut grouped = BTreeMap::<kernc_utils::Span, (IdeCallHierarchyItem, Vec<Range>)>::new();
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
                    );
                }
            }
            AnalysisCallKind::Indirect => {}
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
    grouped: &mut BTreeMap<kernc_utils::Span, (IdeCallHierarchyItem, Vec<Range>)>,
    callee_definition_span: kernc_utils::Span,
    callee_span: kernc_utils::Span,
) {
    let Some(to) = call_hierarchy_item_for_definition(
        session,
        semantic_entries,
        callee_definition_span,
        uri_by_path,
    ) else {
        return;
    };
    grouped
        .entry(callee_definition_span)
        .or_insert((to, Vec::new()))
        .1
        .push(super::span_to_range(session, callee_span));
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

pub(super) fn find_reference_locations(query: ReferenceLocationQuery<'_>) -> Vec<IdeLocation> {
    let Some(definition_span) = find_target_definition_span(
        query.session,
        query.hovers,
        query.semantic_entries,
        query.target_path,
        query.position,
    ) else {
        return Vec::new();
    };
    let definition_spans = rename_definition_span_group(definition_span, query.definition_links);

    let mut locations = Vec::new();
    if query.include_declaration {
        for definition_span in &definition_spans {
            if let Some(location) =
                location_from_span(query.session, *definition_span, query.uri_by_path)
            {
                locations.push(location);
            }
        }
    }

    for entry in query.semantic_entries {
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
    locations
}

pub(super) fn find_reference_locations_for_definition(
    query: KnownReferenceLocationQuery<'_>,
) -> Vec<IdeLocation> {
    let definition_spans =
        rename_definition_span_group(query.definition_span, query.definition_links);

    let mut locations = Vec::new();
    if query.include_declaration {
        for definition_span in &definition_spans {
            if let Some(location) =
                location_from_span(query.session, *definition_span, query.uri_by_path)
            {
                locations.push(location);
            }
        }
    }

    for entry in query.semantic_entries {
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
    locations
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
    position: &Position,
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
            range: super::span_to_range(session, *definition_span),
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
            range: super::span_to_range(session, entry.span),
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
    position: &Position,
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
    position: &Position,
) -> Option<kernc_utils::Span> {
    find_target_definition_span(session, hovers, semantic_entries, target_path, position)
}

pub(super) fn find_hover(
    session: &kernc_utils::Session,
    hovers: &[AnalysisHover],
    semantic_entries: &[AnalysisSemanticEntry],
    target_path: &Path,
    position: &Position,
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
    position: &Position,
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
        range: Some(super::span_to_range(session, range_span)),
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
            range: super::span_to_range(session, span),
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

fn call_hierarchy_item_for_definition(
    session: &kernc_utils::Session,
    semantic_entries: &[AnalysisSemanticEntry],
    definition_span: kernc_utils::Span,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> Option<IdeCallHierarchyItem> {
    let entry = call_hierarchy_definition_entry(semantic_entries, definition_span)?;
    if !matches!(
        entry.kind,
        AnalysisSemanticKind::Function | AnalysisSemanticKind::Method
    ) {
        return None;
    }
    let path = session.source_manager.get_file_path(entry.span.file)?;
    let uri = uri_for_path(path, uri_by_path)?;
    Some(IdeCallHierarchyItem {
        name: span_text(session, entry.span)?,
        kind: ide_symbol_kind(match entry.kind {
            AnalysisSemanticKind::Method => AnalysisSymbolKind::Method,
            _ => AnalysisSymbolKind::Function,
        }),
        uri,
        range: super::span_to_range(session, entry.span),
        selection_range: super::span_to_range(session, entry.span),
    })
}

fn call_hierarchy_definition_entry(
    semantic_entries: &[AnalysisSemanticEntry],
    definition_span: kernc_utils::Span,
) -> Option<&AnalysisSemanticEntry> {
    semantic_entries.iter().find(|entry| {
        entry.role == AnalysisSemanticRole::Definition && entry.definition_span == definition_span
    })
}

fn span_for_lsp_range(
    session: &kernc_utils::Session,
    uri: &str,
    range: &Range,
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

fn range_order_key(range: &Range) -> (u32, u32, u32, u32) {
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
    position: &Position,
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
