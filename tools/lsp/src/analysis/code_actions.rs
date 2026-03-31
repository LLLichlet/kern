use crate::protocol::{CodeAction, Diagnostic, Position, Range, TextEdit, WorkspaceEdit};
use kernc_driver::AnalysisArtifact;
use kernc_lexer::{TokenType, Tokenizer};
use kernc_utils::FileId;
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
    if diagnostic.message == "match expression is not exhaustive"
        || diagnostic.message == "match expression must be exhaustive"
        || diagnostic
            .hints
            .iter()
            .any(|hint| hint.contains("catch-all branch") || hint.starts_with("missing variants:"))
    {
        return add_match_catch_all_code_action(artifact, diagnostic, lsp_diagnostic);
    }
    if diagnostic.message == "irrefutable `let` bindings cannot use `else`"
        || diagnostic.hints.iter().any(|hint| {
            hint == "remove the `else` block or use a refutable variant pattern like `.Ok: value`"
        })
    {
        return remove_irrefutable_let_else_code_action(artifact, diagnostic, lsp_diagnostic);
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
    single_edit_code_action(
        uri,
        title,
        TextEdit {
            range,
            new_text: text.to_string(),
        },
        diagnostic,
        true,
    )
}

fn single_edit_code_action(
    uri: &str,
    title: &str,
    edit: TextEdit,
    diagnostic: Diagnostic,
    is_preferred: bool,
) -> CodeAction {
    let mut changes = BTreeMap::new();
    changes.insert(uri.to_string(), vec![edit]);

    CodeAction {
        title: title.to_string(),
        kind: Some("quickfix"),
        diagnostics: Some(vec![diagnostic]),
        edit: Some(WorkspaceEdit { changes }),
        is_preferred: Some(is_preferred),
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

fn add_match_catch_all_code_action(
    artifact: &AnalysisArtifact,
    diagnostic: &kernc_utils::Diagnostic,
    lsp_diagnostic: Diagnostic,
) -> Option<CodeAction> {
    let file = artifact
        .session
        .source_manager
        .get_file(diagnostic.primary_span.file)?;
    let brace_offset =
        top_level_token_offset(file, diagnostic.primary_span, TokenType::RBrace, true)?;
    let insertion_range = empty_range_at(file, brace_offset);
    let line_start = file.src[..brace_offset]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let brace_indent = leading_whitespace(&file.src[line_start..brace_offset]);
    let insertion_text = if file.src[line_start..brace_offset].trim().is_empty() {
        format!("{}    _ => @unreachable(),\n", brace_indent)
    } else {
        format!("\n{}    _ => @unreachable(),", brace_indent)
    };

    Some(single_edit_code_action(
        &file_uri(artifact, diagnostic.primary_span.file)?,
        "Add `_ => @unreachable()` arm",
        TextEdit {
            range: insertion_range,
            new_text: insertion_text,
        },
        lsp_diagnostic,
        false,
    ))
}

fn remove_irrefutable_let_else_code_action(
    artifact: &AnalysisArtifact,
    diagnostic: &kernc_utils::Diagnostic,
    lsp_diagnostic: Diagnostic,
) -> Option<CodeAction> {
    let file = artifact
        .session
        .source_manager
        .get_file(diagnostic.primary_span.file)?;
    let else_offset =
        top_level_token_offset(file, diagnostic.primary_span, TokenType::Else, false)?;
    let delete_range = Range {
        start: super::byte_offset_to_position(file, else_offset),
        end: super::byte_offset_to_position(file, diagnostic.primary_span.end),
    };

    Some(single_edit_code_action(
        &file_uri(artifact, diagnostic.primary_span.file)?,
        "Remove invalid `else` branch",
        TextEdit {
            range: delete_range,
            new_text: String::new(),
        },
        lsp_diagnostic,
        true,
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

fn file_uri(artifact: &AnalysisArtifact, file_id: kernc_utils::FileId) -> Option<String> {
    artifact
        .session
        .source_manager
        .get_file_path(file_id)
        .and_then(|path| super::file_path_to_uri(path).ok())
}

fn top_level_token_offset(
    file: &kernc_utils::SourceFile,
    span: kernc_utils::Span,
    target: TokenType,
    first: bool,
) -> Option<usize> {
    let slice = file.src.get(span.start..span.end)?;
    let mut tokenizer = Tokenizer::new(slice, FileId(0));
    let mut brace_depth = 0usize;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut found = None;

    loop {
        let token = tokenizer.next_token();
        if token.tag == TokenType::Eof {
            break;
        }

        match token.tag {
            TokenType::LBrace => brace_depth += 1,
            TokenType::RBrace => {
                if brace_depth == 1 && paren_depth == 0 && bracket_depth == 0 && token.tag == target
                {
                    let offset = span.start + token.span.start;
                    if first {
                        return Some(offset);
                    }
                    found = Some(offset);
                }
                brace_depth = brace_depth.saturating_sub(1);
            }
            TokenType::LParen => paren_depth += 1,
            TokenType::RParen => paren_depth = paren_depth.saturating_sub(1),
            TokenType::LBracket => bracket_depth += 1,
            TokenType::RBracket => bracket_depth = bracket_depth.saturating_sub(1),
            _ => {
                if brace_depth == 0 && paren_depth == 0 && bracket_depth == 0 && token.tag == target
                {
                    let offset = span.start + token.span.start;
                    if first {
                        return Some(offset);
                    }
                    found = Some(offset);
                }
            }
        }
    }

    found
}

fn leading_whitespace(text: &str) -> &str {
    let width = text
        .char_indices()
        .find_map(|(index, ch)| (!ch.is_whitespace()).then_some(index))
        .unwrap_or(text.len());
    &text[..width]
}
