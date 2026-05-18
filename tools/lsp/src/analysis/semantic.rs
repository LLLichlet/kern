use super::ide::IdeSemanticTokens;
use super::{IdePosition, IdeRange};
use kernc_driver::{
    AnalysisHover, AnalysisReference, AnalysisSemanticEntry, AnalysisSemanticKind,
    AnalysisSemanticRole, AnalysisSymbol, AnalysisSymbolKind,
};
use kernc_lexer::{Token, TokenType, Tokenizer};
use kernc_utils::{CancellationToken, FastHashMap, FileId};
use std::path::Path;

type SpanKey = (usize, usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SemanticClass {
    token_type: u32,
    modifiers: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SemanticTokenEntry {
    line: u32,
    start_char: u32,
    length: u32,
    token_type: u32,
    modifiers: u32,
}

pub(super) struct SemanticTokenTypes;

impl SemanticTokenTypes {
    pub(super) const NAMESPACE: u32 = 0;
    pub(super) const TYPE: u32 = 1;
    pub(super) const STRUCT: u32 = 2;
    pub(super) const ENUM: u32 = 3;
    pub(super) const INTERFACE: u32 = 4;
    pub(super) const TYPE_PARAMETER: u32 = 5;
    pub(super) const PARAMETER: u32 = 6;
    pub(super) const VARIABLE: u32 = 7;
    pub(super) const PROPERTY: u32 = 8;
    pub(super) const FUNCTION: u32 = 9;
    pub(super) const METHOD: u32 = 10;
    pub(super) const ENUM_MEMBER: u32 = 11;
}

pub(super) struct SemanticModifiers;

impl SemanticModifiers {
    pub(super) const DECLARATION: u32 = 1 << 0;
    pub(super) const READONLY: u32 = 1 << 1;
    pub(super) const STATIC: u32 = 1 << 2;
}

pub(super) struct SemanticArtifactView<'a> {
    pub session: &'a kernc_utils::Session,
    pub symbols: &'a [AnalysisSymbol],
    pub references: &'a [AnalysisReference],
    pub hovers: &'a [AnalysisHover],
    pub semantic_entries: &'a [AnalysisSemanticEntry],
}

pub(super) fn semantic_tokens_cancelable(
    artifact: SemanticArtifactView<'_>,
    file: &kernc_utils::SourceFile,
    target_path: &Path,
    cancellation: &CancellationToken,
) -> Result<IdeSemanticTokens, String> {
    let span_classes = build_semantic_span_classes_cancelable(artifact, target_path, cancellation)?;
    let entries = collect_semantic_token_entries_cancelable(file, &span_classes, cancellation)?;

    Ok(IdeSemanticTokens {
        data: encode_semantic_tokens_cancelable(&entries, cancellation)?,
    })
}

#[cfg(test)]
fn lexical_semantic_tokens_cancelable(
    file: &kernc_utils::SourceFile,
    cancellation: &CancellationToken,
) -> Result<IdeSemanticTokens, String> {
    let entries =
        collect_semantic_token_entries_cancelable(file, &FastHashMap::default(), cancellation)?;

    Ok(IdeSemanticTokens {
        data: encode_semantic_tokens_cancelable(&entries, cancellation)?,
    })
}

pub(super) fn filter_semantic_tokens_to_range_cancelable(
    tokens: &IdeSemanticTokens,
    range: &IdeRange,
    cancellation: &CancellationToken,
) -> Result<IdeSemanticTokens, String> {
    let entries = decode_semantic_token_entries_cancelable(&tokens.data, cancellation)?
        .into_iter()
        .filter(|entry| semantic_token_intersects_range(entry, range))
        .collect::<Vec<_>>();

    Ok(IdeSemanticTokens {
        data: encode_semantic_tokens_cancelable(&entries, cancellation)?,
    })
}

fn build_semantic_span_classes_cancelable(
    artifact: SemanticArtifactView<'_>,
    target_path: &Path,
    cancellation: &CancellationToken,
) -> Result<FastHashMap<SpanKey, SemanticClass>, String> {
    let mut definition_classes = FastHashMap::default();
    for entry in artifact.semantic_entries {
        cancellation
            .check()
            .map_err(|_| "request was canceled".to_string())?;
        if entry.role != AnalysisSemanticRole::Definition {
            continue;
        }
        definition_classes
            .entry(entry.definition_span)
            .or_insert_with(|| semantic_class_from_entry(entry));
    }
    for module_symbol in artifact.symbols {
        collect_semantic_definition_classes_cancelable(
            module_symbol,
            &mut definition_classes,
            cancellation,
        )?;
    }
    for hover in artifact.hovers {
        cancellation
            .check()
            .map_err(|_| "request was canceled".to_string())?;
        if let Some(class) = semantic_class_from_hover(&hover.contents) {
            definition_classes.entry(hover.span).or_insert(class);
        }
    }

    let mut document_classes = FastHashMap::default();
    for (span, class) in &definition_classes {
        cancellation
            .check()
            .map_err(|_| "request was canceled".to_string())?;
        let span = *span;
        if super::span_in_path(artifact.session, span, target_path) {
            document_classes.insert(span_key(span), *class);
        }
    }

    for entry in artifact.semantic_entries {
        cancellation
            .check()
            .map_err(|_| "request was canceled".to_string())?;
        if entry.role != AnalysisSemanticRole::Reference {
            continue;
        }
        if !super::span_in_path(artifact.session, entry.span, target_path) {
            continue;
        }
        document_classes.insert(
            span_key(entry.span),
            semantic_reference_class(semantic_class_from_entry(entry)),
        );
    }

    for reference in artifact.references {
        cancellation
            .check()
            .map_err(|_| "request was canceled".to_string())?;
        if !super::span_in_path(artifact.session, reference.reference_span, target_path) {
            continue;
        }

        let Some(definition_class) = definition_classes.get(&reference.definition_span) else {
            continue;
        };
        document_classes.insert(
            span_key(reference.reference_span),
            semantic_reference_class(*definition_class),
        );
    }

    Ok(document_classes)
}

fn semantic_class_from_entry(entry: &AnalysisSemanticEntry) -> SemanticClass {
    let token_type = match entry.kind {
        AnalysisSemanticKind::Module | AnalysisSemanticKind::Namespace => {
            SemanticTokenTypes::NAMESPACE
        }
        AnalysisSemanticKind::Struct => SemanticTokenTypes::STRUCT,
        AnalysisSemanticKind::Enum => SemanticTokenTypes::ENUM,
        AnalysisSemanticKind::EnumMember => SemanticTokenTypes::ENUM_MEMBER,
        AnalysisSemanticKind::Interface => SemanticTokenTypes::INTERFACE,
        AnalysisSemanticKind::Type => SemanticTokenTypes::TYPE,
        AnalysisSemanticKind::TypeParameter => SemanticTokenTypes::TYPE_PARAMETER,
        AnalysisSemanticKind::Property => SemanticTokenTypes::PROPERTY,
        AnalysisSemanticKind::Variable => SemanticTokenTypes::VARIABLE,
        AnalysisSemanticKind::Parameter => SemanticTokenTypes::PARAMETER,
        AnalysisSemanticKind::Function => SemanticTokenTypes::FUNCTION,
        AnalysisSemanticKind::Method => SemanticTokenTypes::METHOD,
        AnalysisSemanticKind::Constant | AnalysisSemanticKind::Static => {
            SemanticTokenTypes::VARIABLE
        }
    };

    let mut modifiers = match entry.role {
        AnalysisSemanticRole::Definition => SemanticModifiers::DECLARATION,
        AnalysisSemanticRole::Reference => 0,
    };
    if matches!(entry.kind, AnalysisSemanticKind::Constant)
        || matches!(
            entry.kind,
            AnalysisSemanticKind::Variable | AnalysisSemanticKind::Parameter
        ) && !entry.is_mut
        || matches!(entry.kind, AnalysisSemanticKind::Static) && !entry.is_mut
    {
        modifiers |= SemanticModifiers::READONLY;
    }
    if matches!(entry.kind, AnalysisSemanticKind::Static) {
        modifiers |= SemanticModifiers::STATIC;
    }

    SemanticClass {
        token_type,
        modifiers,
    }
}

fn collect_semantic_definition_classes_cancelable(
    symbol: &AnalysisSymbol,
    classes: &mut FastHashMap<kernc_utils::Span, SemanticClass>,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    cancellation
        .check()
        .map_err(|_| "request was canceled".to_string())?;
    classes.insert(
        symbol.selection_span,
        semantic_class_from_symbol_kind(symbol.kind),
    );
    for child in &symbol.children {
        collect_semantic_definition_classes_cancelable(child, classes, cancellation)?;
    }
    Ok(())
}

fn semantic_class_from_symbol_kind(kind: AnalysisSymbolKind) -> SemanticClass {
    let token_type = match kind {
        AnalysisSymbolKind::Module | AnalysisSymbolKind::Namespace => SemanticTokenTypes::NAMESPACE,
        AnalysisSymbolKind::Struct | AnalysisSymbolKind::Union => SemanticTokenTypes::STRUCT,
        AnalysisSymbolKind::Enum => SemanticTokenTypes::ENUM,
        AnalysisSymbolKind::Trait => SemanticTokenTypes::INTERFACE,
        AnalysisSymbolKind::Method => SemanticTokenTypes::METHOD,
        AnalysisSymbolKind::Function => SemanticTokenTypes::FUNCTION,
        AnalysisSymbolKind::TypeAlias => SemanticTokenTypes::TYPE,
        AnalysisSymbolKind::Constant | AnalysisSymbolKind::Static => SemanticTokenTypes::VARIABLE,
    };

    let mut modifiers = SemanticModifiers::DECLARATION;
    if matches!(kind, AnalysisSymbolKind::Constant) {
        modifiers |= SemanticModifiers::READONLY;
    }
    if matches!(kind, AnalysisSymbolKind::Static) {
        modifiers |= SemanticModifiers::STATIC;
    }

    SemanticClass {
        token_type,
        modifiers,
    }
}

fn semantic_class_from_hover(contents: &str) -> Option<SemanticClass> {
    let code = hover_code(contents)?;

    if code.starts_with("fn ") {
        return Some(SemanticClass {
            token_type: SemanticTokenTypes::FUNCTION,
            modifiers: SemanticModifiers::DECLARATION,
        });
    }
    if code.starts_with("const ") {
        return Some(SemanticClass {
            token_type: SemanticTokenTypes::VARIABLE,
            modifiers: SemanticModifiers::DECLARATION | SemanticModifiers::READONLY,
        });
    }
    if code.starts_with("static ") {
        return Some(SemanticClass {
            token_type: SemanticTokenTypes::VARIABLE,
            modifiers: SemanticModifiers::DECLARATION
                | SemanticModifiers::STATIC
                | if code.contains(" mut ") {
                    0
                } else {
                    SemanticModifiers::READONLY
                },
        });
    }
    if code.starts_with("var ") {
        return Some(SemanticClass {
            token_type: SemanticTokenTypes::VARIABLE,
            modifiers: SemanticModifiers::DECLARATION
                | if code.contains(" mut ") {
                    0
                } else {
                    SemanticModifiers::READONLY
                },
        });
    }
    if code.starts_with("struct ") || code.starts_with("union ") {
        return Some(SemanticClass {
            token_type: SemanticTokenTypes::STRUCT,
            modifiers: SemanticModifiers::DECLARATION,
        });
    }
    if code.starts_with("enum ") {
        return Some(SemanticClass {
            token_type: SemanticTokenTypes::ENUM,
            modifiers: SemanticModifiers::DECLARATION,
        });
    }
    if code.starts_with("trait ") {
        return Some(SemanticClass {
            token_type: SemanticTokenTypes::INTERFACE,
            modifiers: SemanticModifiers::DECLARATION,
        });
    }
    if code.starts_with("module ") {
        return Some(SemanticClass {
            token_type: SemanticTokenTypes::NAMESPACE,
            modifiers: SemanticModifiers::DECLARATION,
        });
    }
    if code.starts_with("type ") {
        return Some(SemanticClass {
            token_type: if code.contains(" = ") {
                SemanticTokenTypes::TYPE
            } else {
                SemanticTokenTypes::TYPE_PARAMETER
            },
            modifiers: SemanticModifiers::DECLARATION,
        });
    }

    None
}

fn hover_code(contents: &str) -> Option<&str> {
    let rest = contents.strip_prefix("```kern\n")?;
    let end = rest.find("\n```")?;
    Some(&rest[..end])
}

fn semantic_reference_class(class: SemanticClass) -> SemanticClass {
    SemanticClass {
        token_type: class.token_type,
        modifiers: class.modifiers & !SemanticModifiers::DECLARATION,
    }
}

fn collect_semantic_token_entries_cancelable(
    file: &kernc_utils::SourceFile,
    span_classes: &FastHashMap<SpanKey, SemanticClass>,
    cancellation: &CancellationToken,
) -> Result<Vec<SemanticTokenEntry>, String> {
    let mut tokenizer = Tokenizer::new(&file.src, FileId(0));
    let mut tokens = Vec::new();

    loop {
        cancellation
            .check()
            .map_err(|_| "request was canceled".to_string())?;
        let token = tokenizer.next_token();
        if token.tag == TokenType::Eof {
            break;
        }
        tokens.push(token);
    }

    let mut entries = Vec::new();
    for (index, token) in tokens.iter().copied().enumerate() {
        cancellation
            .check()
            .map_err(|_| "request was canceled".to_string())?;
        let class = match token.tag {
            TokenType::Identifier => parameter_declaration_class(&tokens, index)
                .or_else(|| span_classes.get(&span_key(token.span)).copied())
                .or_else(|| heuristic_identifier_class(&tokens, index)),
            _ => None,
        };

        let Some(class) = class else {
            continue;
        };
        push_semantic_token_entries(&mut entries, file, token.span.start, token.span.end, class);
    }

    Ok(entries)
}

fn heuristic_identifier_class(tokens: &[Token], index: usize) -> Option<SemanticClass> {
    if is_type_context_identifier(tokens, index) {
        return Some(SemanticClass {
            token_type: SemanticTokenTypes::TYPE,
            modifiers: 0,
        });
    }

    let previous = previous_significant_token(tokens, index)?;
    if previous.tag == TokenType::Dot {
        if next_significant_token(tokens, index).map(|token| token.tag) == Some(TokenType::LParen) {
            return Some(SemanticClass {
                token_type: SemanticTokenTypes::METHOD,
                modifiers: 0,
            });
        }
        return Some(SemanticClass {
            token_type: SemanticTokenTypes::PROPERTY,
            modifiers: 0,
        });
    }

    None
}

fn parameter_declaration_class(tokens: &[Token], index: usize) -> Option<SemanticClass> {
    is_parameter_declaration(tokens, index).then_some(SemanticClass {
        token_type: SemanticTokenTypes::PARAMETER,
        modifiers: SemanticModifiers::DECLARATION,
    })
}

fn is_parameter_declaration(tokens: &[Token], index: usize) -> bool {
    let Some(token) = tokens.get(index) else {
        return false;
    };
    if token.tag != TokenType::Identifier {
        return false;
    }
    if next_significant_token(tokens, index).map(|token| token.tag) != Some(TokenType::Colon) {
        return false;
    }

    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut idx = index;
    while idx > 0 {
        idx -= 1;
        match tokens[idx].tag {
            TokenType::RParen => paren_depth += 1,
            TokenType::LParen => {
                if paren_depth == 0 {
                    let Some(previous) = previous_significant_token(tokens, idx) else {
                        return false;
                    };
                    if previous.tag == TokenType::Identifier {
                        let Some(previous_index) = previous_significant_token_index(tokens, idx)
                        else {
                            return false;
                        };
                        return previous_significant_token(tokens, previous_index)
                            .map(|token| token.tag == TokenType::Fn)
                            .unwrap_or(false);
                    }
                    return previous.tag == TokenType::Fn || previous.tag == TokenType::CapitalFn;
                }
                paren_depth -= 1;
            }
            TokenType::RBracket => bracket_depth += 1,
            TokenType::LBracket => bracket_depth = bracket_depth.saturating_sub(1),
            _ => {}
        }
    }

    false
}

fn is_type_context_identifier(tokens: &[Token], index: usize) -> bool {
    let Some(token) = tokens.get(index) else {
        return false;
    };
    if token.tag != TokenType::Identifier {
        return false;
    }

    let Some(previous) = previous_significant_token(tokens, index) else {
        return false;
    };

    match previous.tag {
        TokenType::Colon
        | TokenType::Arrow
        | TokenType::As
        | TokenType::Question
        | TokenType::Impl => return true,
        TokenType::Dot => {
            let Some(dot_index) = previous_significant_token_index(tokens, index) else {
                return false;
            };
            let Some(base_index) = previous_significant_token_index(tokens, dot_index) else {
                return false;
            };
            return is_type_context_identifier(tokens, base_index);
        }
        TokenType::Mut => {
            let Some(mut_index) = previous_significant_token_index(tokens, index) else {
                return false;
            };
            if is_mut_type_qualifier(tokens, mut_index) {
                return true;
            }
        }
        TokenType::Star
        | TokenType::Caret
        | TokenType::Ampersand
        | TokenType::Bang
        | TokenType::LParen
        | TokenType::RParen
        | TokenType::RBracket
        | TokenType::DotAmpersand
        | TokenType::DotDotAmpersand
        | TokenType::LBracket
        | TokenType::Comma
            if is_nested_in_type_context(tokens, index) =>
        {
            return true;
        }
        _ => {}
    }

    false
}

fn is_mut_type_qualifier(tokens: &[Token], mut_index: usize) -> bool {
    let Some(previous_index) = previous_significant_token_index(tokens, mut_index) else {
        return false;
    };
    match tokens[previous_index].tag {
        TokenType::Ampersand | TokenType::Caret => {
            is_nested_in_type_context(tokens, previous_index)
        }
        TokenType::RBracket => {
            is_slice_type_close_bracket(tokens, previous_index)
                && is_nested_in_type_context(tokens, previous_index)
        }
        _ => false,
    }
}

fn is_slice_type_close_bracket(tokens: &[Token], rbracket_index: usize) -> bool {
    if tokens.get(rbracket_index).map(|token| token.tag) != Some(TokenType::RBracket) {
        return false;
    }
    previous_significant_token(tokens, rbracket_index)
        .map(|token| token.tag == TokenType::LBracket)
        .unwrap_or(false)
}

fn is_nested_in_type_context(tokens: &[Token], index: usize) -> bool {
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut idx = index;
    while idx > 0 {
        idx -= 1;
        match tokens[idx].tag {
            TokenType::RParen if paren_depth == 0 && bracket_depth == 0 => {
                if is_function_return_type_context_rparen(tokens, idx) {
                    return true;
                }
                paren_depth += 1;
            }
            TokenType::RParen => paren_depth += 1,
            TokenType::LParen => paren_depth = paren_depth.saturating_sub(1),
            TokenType::RBracket => bracket_depth += 1,
            TokenType::LBracket => bracket_depth = bracket_depth.saturating_sub(1),
            TokenType::Colon | TokenType::Arrow | TokenType::As | TokenType::Impl
                if paren_depth == 0 && bracket_depth == 0 =>
            {
                return true;
            }
            _ => {}
        }
    }

    false
}

fn is_function_return_type_context_rparen(tokens: &[Token], rparen_index: usize) -> bool {
    let Some(lparen_index) =
        matching_open_token_index(tokens, rparen_index, TokenType::LParen, TokenType::RParen)
    else {
        return false;
    };

    let Some(owner_index) = function_like_owner_before_lparen(tokens, lparen_index) else {
        return false;
    };

    match tokens[owner_index].tag {
        TokenType::Fn | TokenType::CapitalFn => true,
        TokenType::Identifier => previous_significant_token(tokens, owner_index)
            .map(|token| token.tag == TokenType::Fn)
            .unwrap_or(false),
        _ => false,
    }
}

fn function_like_owner_before_lparen(tokens: &[Token], lparen_index: usize) -> Option<usize> {
    let previous_index = previous_significant_token_index(tokens, lparen_index)?;
    if tokens[previous_index].tag != TokenType::RBracket {
        return Some(previous_index);
    }

    let generic_lbracket_index = matching_open_token_index(
        tokens,
        previous_index,
        TokenType::LBracket,
        TokenType::RBracket,
    )?;
    previous_significant_token_index(tokens, generic_lbracket_index)
}

fn matching_open_token_index(
    tokens: &[Token],
    close_index: usize,
    open: TokenType,
    close: TokenType,
) -> Option<usize> {
    if tokens.get(close_index).map(|token| token.tag) != Some(close) {
        return None;
    }

    let mut depth = 0usize;
    let mut idx = close_index;
    while idx > 0 {
        idx -= 1;
        if tokens[idx].tag == close {
            depth += 1;
        } else if tokens[idx].tag == open {
            if depth == 0 {
                return Some(idx);
            }
            depth -= 1;
        }
    }

    None
}

fn previous_significant_token(tokens: &[Token], index: usize) -> Option<Token> {
    tokens.get(..index)?.iter().rev().copied().next()
}

fn previous_significant_token_index(tokens: &[Token], index: usize) -> Option<usize> {
    tokens.get(..index)?;
    index.checked_sub(1)
}

fn next_significant_token(tokens: &[Token], index: usize) -> Option<Token> {
    tokens.get(index + 1..)?.iter().copied().next()
}

fn push_semantic_token_entries(
    entries: &mut Vec<SemanticTokenEntry>,
    file: &kernc_utils::SourceFile,
    start: usize,
    end: usize,
    class: SemanticClass,
) {
    if start >= end {
        return;
    }

    let mut segment_start = start;
    while segment_start < end {
        let line_index = file.lookup_line(segment_start).saturating_sub(1);
        let line_start = file.line_starts[line_index];
        let next_line_start = file
            .line_starts
            .get(line_index + 1)
            .copied()
            .unwrap_or(file.src.len());
        let visible_line_end = super::trim_line_ending(&file.src, line_start, next_line_start);
        let segment_end = end.min(visible_line_end);

        if segment_start < segment_end {
            let start_position = super::byte_offset_to_position(file, segment_start);
            let end_position = super::byte_offset_to_position(file, segment_end);
            let length = end_position
                .character
                .saturating_sub(start_position.character);
            if length > 0 {
                entries.push(SemanticTokenEntry {
                    line: start_position.line,
                    start_char: start_position.character,
                    length,
                    token_type: class.token_type,
                    modifiers: class.modifiers,
                });
            }
        }

        if next_line_start <= segment_start {
            break;
        }
        segment_start = next_line_start;
    }
}

fn encode_semantic_tokens_cancelable(
    entries: &[SemanticTokenEntry],
    cancellation: &CancellationToken,
) -> Result<Vec<u32>, String> {
    let mut sorted = entries.to_vec();
    cancellation
        .check()
        .map_err(|_| "request was canceled".to_string())?;
    sorted.sort();

    let mut data = Vec::with_capacity(sorted.len() * 5);
    let mut previous_line = 0;
    let mut previous_start = 0;

    for (index, entry) in sorted.iter().enumerate() {
        cancellation
            .check()
            .map_err(|_| "request was canceled".to_string())?;
        let delta_line = if index == 0 {
            entry.line
        } else {
            entry.line - previous_line
        };
        let delta_start = if index == 0 || delta_line > 0 {
            entry.start_char
        } else {
            entry.start_char - previous_start
        };

        data.extend_from_slice(&[
            delta_line,
            delta_start,
            entry.length,
            entry.token_type,
            entry.modifiers,
        ]);

        previous_line = entry.line;
        previous_start = entry.start_char;
    }

    Ok(data)
}

fn decode_semantic_token_entries_cancelable(
    data: &[u32],
    cancellation: &CancellationToken,
) -> Result<Vec<SemanticTokenEntry>, String> {
    let mut entries = Vec::new();
    let mut line = 0;
    let mut start_char = 0;

    for chunk in data.chunks_exact(5) {
        cancellation
            .check()
            .map_err(|_| "request was canceled".to_string())?;
        line += chunk[0];
        if chunk[0] == 0 {
            start_char += chunk[1];
        } else {
            start_char = chunk[1];
        }

        entries.push(SemanticTokenEntry {
            line,
            start_char,
            length: chunk[2],
            token_type: chunk[3],
            modifiers: chunk[4],
        });
    }

    Ok(entries)
}

fn semantic_token_intersects_range(entry: &SemanticTokenEntry, range: &IdeRange) -> bool {
    let start = IdePosition {
        line: entry.line,
        character: entry.start_char,
    };
    let end = IdePosition {
        line: entry.line,
        character: entry.start_char + entry.length,
    };

    position_less_than(&start, &range.end) && position_less_than(&range.start, &end)
}

fn position_less_than(left: &IdePosition, right: &IdePosition) -> bool {
    left.line < right.line || left.line == right.line && left.character < right.character
}

fn span_key(span: kernc_utils::Span) -> SpanKey {
    (span.start, span.end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernc_driver::{AnalysisSemanticEntry, AnalysisSemanticKind, AnalysisSemanticRole};
    use kernc_utils::{FileId, Session, Span};
    use std::path::PathBuf;

    fn span(start: usize, end: usize) -> Span {
        Span {
            file: FileId(0),
            start,
            end,
        }
    }

    #[test]
    fn semantic_tokenization_loop_observes_cancellation() {
        let file = kernc_utils::SourceFile::new(
            PathBuf::from("semantic_cancel.kn"),
            "fn main() void {\n    let value = 1;\n    let other = value;\n}\n",
        );
        let cancellation = CancellationToken::with_check_budget_for_testing(3);

        let result = lexical_semantic_tokens_cancelable(&file, &cancellation);

        assert_eq!(result.unwrap_err(), "request was canceled");
        assert!(cancellation.is_canceled());
    }

    #[test]
    fn semantic_reference_merge_loop_observes_cancellation() {
        let mut session = Session::new();
        let file_id = session.source_manager.add_file(
            "semantic_reference_cancel.kn".to_string(),
            "fn target() void {}\n",
        );
        let definition_span = Span {
            file: file_id,
            start: 3,
            end: 9,
        };
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
        let view = SemanticArtifactView {
            session: &session,
            symbols: &[],
            references: &[],
            hovers: &[],
            semantic_entries: &semantic_entries,
        };
        let file = session.source_manager.get_file(file_id).unwrap();
        let cancellation = CancellationToken::with_check_budget_for_testing(4);

        let result = semantic_tokens_cancelable(
            view,
            file,
            session.source_manager.get_file_path(file_id).unwrap(),
            &cancellation,
        );

        assert_eq!(result.unwrap_err(), "request was canceled");
        assert!(cancellation.is_canceled());
    }

    #[test]
    fn semantic_token_range_decode_loop_observes_cancellation() {
        let tokens = IdeSemanticTokens {
            data: (0..20).flat_map(|_| [0, 1, 1, 1, 0]).collect(),
        };
        let cancellation = CancellationToken::with_check_budget_for_testing(3);

        let result = filter_semantic_tokens_to_range_cancelable(
            &tokens,
            &IdeRange {
                start: IdePosition {
                    line: 0,
                    character: 0,
                },
                end: IdePosition {
                    line: 1,
                    character: 0,
                },
            },
            &cancellation,
        );

        assert_eq!(result.unwrap_err(), "request was canceled");
        assert!(cancellation.is_canceled());
    }
}
