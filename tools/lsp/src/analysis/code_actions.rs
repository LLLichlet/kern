use crate::protocol::{CodeAction, Diagnostic, Position, Range, TextEdit, WorkspaceEdit};
use kernc_driver::{AnalysisArtifact, AnalysisDeadStoreKind};
use kernc_lexer::{TokenType, Tokenizer};
use kernc_utils::{DiagnosticCode, FileId};
use std::collections::BTreeMap;

pub(super) fn quick_fix_for_diagnostic(
    uri: &str,
    artifact: &AnalysisArtifact,
    diagnostic: &kernc_utils::Diagnostic,
    lsp_diagnostic: Diagnostic,
) -> Option<CodeAction> {
    if diagnostic.level == kernc_utils::DiagnosticLevel::Warning
        && let Some(action) =
            fact_driven_quick_fix(uri, artifact, diagnostic, lsp_diagnostic.clone())
    {
        return Some(action);
    }
    if let Some(action) = structured_quick_fix(uri, artifact, diagnostic, lsp_diagnostic.clone()) {
        return Some(action);
    }
    if diagnostic.code.is_some() {
        return None;
    }

    fallback_text_quick_fix(uri, artifact, diagnostic, lsp_diagnostic)
}

fn structured_quick_fix(
    uri: &str,
    artifact: &AnalysisArtifact,
    diagnostic: &kernc_utils::Diagnostic,
    lsp_diagnostic: Diagnostic,
) -> Option<CodeAction> {
    match diagnostic.code {
        Some(DiagnosticCode::ExpectedSemicolon) => Some(insert_text_code_action(
            uri,
            "Insert `;`",
            ";",
            Range {
                start: lsp_diagnostic.range.start.clone(),
                end: lsp_diagnostic.range.start.clone(),
            },
            lsp_diagnostic,
        )),
        Some(DiagnosticCode::UnclosedParen) => Some(insert_text_code_action(
            uri,
            "Insert `)`",
            ")",
            Range {
                start: lsp_diagnostic.range.start.clone(),
                end: lsp_diagnostic.range.start.clone(),
            },
            lsp_diagnostic,
        )),
        Some(DiagnosticCode::UnclosedBracket) => Some(insert_text_code_action(
            uri,
            "Insert `]`",
            "]",
            Range {
                start: lsp_diagnostic.range.start.clone(),
                end: lsp_diagnostic.range.start.clone(),
            },
            lsp_diagnostic,
        )),
        Some(DiagnosticCode::UnclosedBlock) => Some(insert_text_code_action(
            uri,
            "Insert `}`",
            "}",
            Range {
                start: lsp_diagnostic.range.start.clone(),
                end: lsp_diagnostic.range.start.clone(),
            },
            lsp_diagnostic,
        )),
        Some(DiagnosticCode::IgnoredNonvoidValue) => Some(insert_text_code_action(
            uri,
            "Discard value with `let _ =`",
            "let _ = ",
            Range {
                start: lsp_diagnostic.range.start.clone(),
                end: lsp_diagnostic.range.start.clone(),
            },
            lsp_diagnostic,
        )),
        Some(DiagnosticCode::RequiresLetMut) => {
            let_mut_code_action(artifact, diagnostic, lsp_diagnostic)
        }
        Some(DiagnosticCode::NonexhaustiveMatch) => {
            add_match_catch_all_code_action(artifact, diagnostic, lsp_diagnostic)
        }
        Some(DiagnosticCode::IrrefutableLetElse) => {
            remove_irrefutable_let_else_code_action(artifact, diagnostic, lsp_diagnostic)
        }
        _ => None,
    }
}

fn fallback_text_quick_fix(
    uri: &str,
    artifact: &AnalysisArtifact,
    diagnostic: &kernc_utils::Diagnostic,
    lsp_diagnostic: Diagnostic,
) -> Option<CodeAction> {
    let insertion_range = Range {
        start: lsp_diagnostic.range.start.clone(),
        end: lsp_diagnostic.range.start.clone(),
    };

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
        || diagnostic.message == "irrefutable `let` patterns cannot use `else`"
        || diagnostic
            .hints
            .iter()
            .any(|hint| hint.contains("remove the `else` block") && hint.contains("refutable"))
    {
        return remove_irrefutable_let_else_code_action(artifact, diagnostic, lsp_diagnostic);
    }

    None
}

fn fact_driven_quick_fix(
    uri: &str,
    artifact: &AnalysisArtifact,
    diagnostic: &kernc_utils::Diagnostic,
    lsp_diagnostic: Diagnostic,
) -> Option<CodeAction> {
    match diagnostic.code {
        Some(DiagnosticCode::UnusedBinding) => {
            unused_binding_code_action(uri, artifact, diagnostic, lsp_diagnostic)
        }
        Some(DiagnosticCode::DeadStore) => {
            dead_store_code_action(uri, artifact, diagnostic, lsp_diagnostic)
        }
        Some(DiagnosticCode::UnusedPrivateItem) => {
            unused_private_item_code_action(uri, artifact, diagnostic, lsp_diagnostic)
        }
        _ => None,
    }
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

fn unused_binding_code_action(
    uri: &str,
    artifact: &AnalysisArtifact,
    diagnostic: &kernc_utils::Diagnostic,
    lsp_diagnostic: Diagnostic,
) -> Option<CodeAction> {
    artifact
        .unused_bindings()
        .into_iter()
        .find(|binding| binding.definition_span == diagnostic.primary_span)
        .map(|_| {
            single_edit_code_action(
                uri,
                "Rename binding to `_`",
                TextEdit {
                    range: super::span_to_range(&artifact.session, diagnostic.primary_span),
                    new_text: "_".to_string(),
                },
                lsp_diagnostic,
                true,
            )
        })
}

fn dead_store_code_action(
    uri: &str,
    artifact: &AnalysisArtifact,
    diagnostic: &kernc_utils::Diagnostic,
    lsp_diagnostic: Diagnostic,
) -> Option<CodeAction> {
    let store = artifact
        .dead_stores()
        .into_iter()
        .find(|store| store.span == diagnostic.primary_span)?;
    if store.kind != AnalysisDeadStoreKind::Assignment {
        return None;
    }

    let file = artifact.session.source_manager.get_file(store.span.file)?;
    let delete_range = standalone_statement_delete_range(file, store.span)?;

    Some(single_edit_code_action(
        uri,
        "Remove dead assignment",
        TextEdit {
            range: delete_range,
            new_text: String::new(),
        },
        lsp_diagnostic,
        true,
    ))
}

fn unused_private_item_code_action(
    uri: &str,
    artifact: &AnalysisArtifact,
    diagnostic: &kernc_utils::Diagnostic,
    lsp_diagnostic: Diagnostic,
) -> Option<CodeAction> {
    let item = artifact
        .unused_private_items()
        .into_iter()
        .find(|item| item.definition_span == diagnostic.primary_span)?;
    let file = artifact
        .session
        .source_manager
        .get_file(item.definition_span.file)?;
    let insertion_offset = pub_insertion_offset(file, item.definition_span.start)?;

    Some(single_edit_code_action(
        uri,
        "Make item public",
        TextEdit {
            range: empty_range_at(file, insertion_offset),
            new_text: "pub ".to_string(),
        },
        lsp_diagnostic,
        false,
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

fn standalone_statement_delete_range(
    file: &kernc_utils::SourceFile,
    span: kernc_utils::Span,
) -> Option<Range> {
    if span.end > file.src.len() || span.start >= span.end {
        return None;
    }

    let (line_start, line_end) = line_bounds(&file.src, span.start)?;
    if !file.src[line_start..span.start].trim().is_empty() {
        return None;
    }

    let statement_end = skip_inline_whitespace(&file.src, span.end);
    if file.src.get(statement_end..=statement_end)? != ";" {
        return None;
    }
    let delete_end = statement_end + 1;

    if !file.src[delete_end..line_end].trim().is_empty() {
        return None;
    }

    let range_end = if file.src.as_bytes().get(line_end) == Some(&b'\n') {
        line_end + 1
    } else {
        line_end
    };

    Some(Range {
        start: super::byte_offset_to_position(file, line_start),
        end: super::byte_offset_to_position(file, range_end),
    })
}

fn line_bounds(source: &str, offset: usize) -> Option<(usize, usize)> {
    if offset > source.len() {
        return None;
    }

    let line_start = source[..offset]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let line_end = source[offset..]
        .find('\n')
        .map(|index| offset + index)
        .unwrap_or(source.len());
    Some((line_start, line_end))
}

fn pub_insertion_offset(file: &kernc_utils::SourceFile, offset: usize) -> Option<usize> {
    let (line_start, line_end) = line_bounds(&file.src, offset)?;
    let line = &file.src[line_start..line_end];
    let indent = leading_whitespace(line).len();
    let trimmed = &line[indent..];
    if trimmed.starts_with("fn ")
        || trimmed.starts_with("const ")
        || trimmed.starts_with("static ")
        || trimmed.starts_with("extern fn ")
    {
        return Some(line_start + indent);
    }

    None
}

fn skip_inline_whitespace(source: &str, mut offset: usize) -> usize {
    let bytes = source.as_bytes();
    while let Some(byte) = bytes.get(offset) {
        if *byte == b' ' || *byte == b'\t' {
            offset += 1;
        } else {
            break;
        }
    }
    offset
}

fn leading_whitespace(text: &str) -> &str {
    let width = text
        .char_indices()
        .find_map(|(index, ch)| (!ch.is_whitespace()).then_some(index))
        .unwrap_or(text.len());
    &text[..width]
}
