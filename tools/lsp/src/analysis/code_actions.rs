use crate::protocol::{CodeAction, Diagnostic, Position, Range, TextEdit, WorkspaceEdit};
use kernc_driver::AnalysisArtifact;
use std::collections::BTreeMap;

pub(super) fn quick_fix_for_diagnostic(
    uri: &str,
    artifact: &AnalysisArtifact,
    diagnostic: &kernc_utils::Diagnostic,
    lsp_diagnostic: Diagnostic,
) -> Option<CodeAction> {
    let session = &artifact.session;
    let file = session
        .source_manager
        .get_file(diagnostic.primary_span.file)?;
    let insertion_range = empty_range_at(file, diagnostic.primary_span.start);

    if diagnostic.message == "Expected semicolon"
        || diagnostic
            .hints
            .iter()
            .any(|hint| hint == "consider adding a `;` here")
    {
        return Some(insert_text_code_action(
            uri,
            "Insert `;`",
            ";",
            insertion_range,
            lsp_diagnostic,
        ));
    }
    if diagnostic
        .hints
        .iter()
        .any(|hint| hint == "unclosed parenthesis")
    {
        return Some(insert_text_code_action(
            uri,
            "Insert `)`",
            ")",
            insertion_range,
            lsp_diagnostic,
        ));
    }
    if diagnostic
        .hints
        .iter()
        .any(|hint| hint == "unclosed bracket")
    {
        return Some(insert_text_code_action(
            uri,
            "Insert `]`",
            "]",
            insertion_range,
            lsp_diagnostic,
        ));
    }
    if diagnostic.hints.iter().any(|hint| hint == "unclosed block") {
        return Some(insert_text_code_action(
            uri,
            "Insert `}`",
            "}",
            insertion_range,
            lsp_diagnostic,
        ));
    }
    if diagnostic.message == "ignored non-void return value"
        || diagnostic
            .hints
            .iter()
            .any(|hint| hint == "in Kern, use `let _ = ...;` to explicitly discard the value")
    {
        return Some(insert_text_code_action(
            uri,
            "Discard value with `let _ =`",
            "let _ = ",
            insertion_range,
            lsp_diagnostic,
        ));
    }
    if diagnostic.hints.iter().any(suggests_let_mut_fix) {
        return let_mut_code_action(artifact, diagnostic, lsp_diagnostic);
    }

    None
}

fn insert_text_code_action(
    uri: &str,
    title: &str,
    text: &str,
    range: Range,
    diagnostic: Diagnostic,
) -> CodeAction {
    let mut changes = BTreeMap::new();
    changes.insert(
        uri.to_string(),
        vec![TextEdit {
            range,
            new_text: text.to_string(),
        }],
    );

    CodeAction {
        title: title.to_string(),
        kind: Some("quickfix"),
        diagnostics: Some(vec![diagnostic]),
        edit: Some(WorkspaceEdit { changes }),
        is_preferred: Some(true),
    }
}

fn let_mut_code_action(
    artifact: &AnalysisArtifact,
    diagnostic: &kernc_utils::Diagnostic,
    lsp_diagnostic: Diagnostic,
) -> Option<CodeAction> {
    let definition_span = mutable_binding_definition(artifact, diagnostic.primary_span)?;
    let file = artifact
        .session
        .source_manager
        .get_file(definition_span.file)?;
    let insertion_offset = let_mut_insertion_offset(file, definition_span.start)?;
    let insertion_range = empty_range_at(file, insertion_offset);
    let target_uri = artifact
        .session
        .source_manager
        .get_file_path(definition_span.file)
        .and_then(|path| super::file_path_to_uri(path).ok())?;

    Some(insert_text_code_action(
        &target_uri,
        "Change to `let mut`",
        "mut ",
        insertion_range,
        lsp_diagnostic,
    ))
}

pub(super) fn workspace_edit_key(edit: &WorkspaceEdit) -> String {
    let mut key = String::new();
    for (uri, edits) in &edit.changes {
        key.push_str(uri);
        for edit in edits {
            key.push_str(&format!(
                "|{}:{}:{}:{}:{}:{}",
                edit.range.start.line,
                edit.range.start.character,
                edit.range.end.line,
                edit.range.end.character,
                edit.new_text.len(),
                edit.new_text
            ));
        }
    }
    key
}

fn empty_range_at(file: &kernc_utils::SourceFile, offset: usize) -> Range {
    let position = super::byte_offset_to_position(file, offset);
    Range {
        start: position.clone(),
        end: position,
    }
}

pub(super) fn ranges_overlap(left: &Range, right: &Range) -> bool {
    position_leq(&left.start, &right.end) && position_leq(&right.start, &left.end)
}

fn position_leq(left: &Position, right: &Position) -> bool {
    left.line < right.line || (left.line == right.line && left.character <= right.character)
}

fn suggests_let_mut_fix(hint: &String) -> bool {
    hint == "consider declaring the variable as `let mut`"
        || hint == "consider declaring the closure variable as `let mut`"
        || hint == "if this is a variable, declare it with `let mut`"
        || hint == "ensure the target is bound with `let mut` or is a mutable pointer"
}

fn mutable_binding_definition(
    artifact: &AnalysisArtifact,
    primary_span: kernc_utils::Span,
) -> Option<kernc_utils::Span> {
    artifact
        .references
        .iter()
        .filter(|reference| spans_overlap(reference.reference_span, primary_span))
        .min_by_key(|reference| span_len(reference.reference_span))
        .map(|reference| reference.definition_span)
        .or(Some(primary_span))
}

fn let_mut_insertion_offset(
    file: &kernc_utils::SourceFile,
    identifier_start: usize,
) -> Option<usize> {
    if identifier_start > file.src.len() {
        return None;
    }

    let line_start = file.src[..identifier_start]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let prefix = file.src[line_start..identifier_start].trim();

    if prefix == "let" {
        Some(identifier_start)
    } else {
        None
    }
}

fn spans_overlap(left: kernc_utils::Span, right: kernc_utils::Span) -> bool {
    left.start < right.end && right.start < left.end
}

fn span_len(span: kernc_utils::Span) -> usize {
    span.end.saturating_sub(span.start)
}
