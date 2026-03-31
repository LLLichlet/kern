use crate::protocol::SemanticTokens;
use kernc_driver::{AnalysisArtifact, AnalysisSymbol, AnalysisSymbolKind};
use kernc_lexer::{Token, TokenType, Tokenizer};
use kernc_utils::FileId;
use std::collections::BTreeMap;
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

const TOKEN_NAMESPACE: u32 = 0;
pub(super) const TOKEN_TYPE: u32 = 1;
pub(super) const TOKEN_STRUCT: u32 = 2;
const TOKEN_ENUM: u32 = 3;
const TOKEN_INTERFACE: u32 = 4;
const TOKEN_TYPE_PARAMETER: u32 = 5;
pub(super) const TOKEN_PARAMETER: u32 = 6;
pub(super) const TOKEN_PROPERTY: u32 = 8;
const TOKEN_VARIABLE: u32 = 7;
pub(super) const TOKEN_FUNCTION: u32 = 9;
const TOKEN_METHOD: u32 = 10;
pub(super) const TOKEN_KEYWORD: u32 = 11;
const TOKEN_STRING: u32 = 12;
const TOKEN_NUMBER: u32 = 13;
const TOKEN_OPERATOR: u32 = 14;

const MOD_DECLARATION: u32 = 1 << 0;
const MOD_READONLY: u32 = 1 << 1;
const MOD_STATIC: u32 = 1 << 2;

pub(super) fn semantic_tokens(
    artifact: &AnalysisArtifact,
    file: &kernc_utils::SourceFile,
    target_path: &Path,
) -> SemanticTokens {
    let span_classes = build_semantic_span_classes(artifact, target_path);
    let entries = collect_semantic_token_entries(file, &span_classes);

    SemanticTokens {
        data: encode_semantic_tokens(&entries),
    }
}

fn build_semantic_span_classes(
    artifact: &AnalysisArtifact,
    target_path: &Path,
) -> BTreeMap<SpanKey, SemanticClass> {
    let mut definition_classes = BTreeMap::new();
    for module_symbol in &artifact.symbols {
        collect_semantic_definition_classes(module_symbol, &mut definition_classes);
    }
    for hover in &artifact.hovers {
        if let Some(class) = semantic_class_from_hover(&hover.contents) {
            definition_classes.entry(hover.span).or_insert(class);
        }
    }

    let mut document_classes = BTreeMap::new();
    for (span, class) in &definition_classes {
        let span = *span;
        if super::span_in_path(&artifact.session, span, target_path) {
            document_classes.insert(span_key(span), *class);
        }
    }

    for reference in &artifact.references {
        if !super::span_in_path(&artifact.session, reference.reference_span, target_path) {
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

    document_classes
}

fn collect_semantic_definition_classes(
    symbol: &AnalysisSymbol,
    classes: &mut BTreeMap<kernc_utils::Span, SemanticClass>,
) {
    classes.insert(
        symbol.selection_span,
        semantic_class_from_symbol_kind(symbol.kind),
    );
    for child in &symbol.children {
        collect_semantic_definition_classes(child, classes);
    }
}

fn semantic_class_from_symbol_kind(kind: AnalysisSymbolKind) -> SemanticClass {
    let token_type = match kind {
        AnalysisSymbolKind::Module | AnalysisSymbolKind::Namespace => TOKEN_NAMESPACE,
        AnalysisSymbolKind::Struct | AnalysisSymbolKind::Union => TOKEN_STRUCT,
        AnalysisSymbolKind::Enum => TOKEN_ENUM,
        AnalysisSymbolKind::Trait => TOKEN_INTERFACE,
        AnalysisSymbolKind::Method => TOKEN_METHOD,
        AnalysisSymbolKind::Function => TOKEN_FUNCTION,
        AnalysisSymbolKind::TypeAlias => TOKEN_TYPE,
        AnalysisSymbolKind::Constant | AnalysisSymbolKind::Static => TOKEN_VARIABLE,
    };

    let mut modifiers = MOD_DECLARATION;
    if matches!(kind, AnalysisSymbolKind::Constant) {
        modifiers |= MOD_READONLY;
    }
    if matches!(kind, AnalysisSymbolKind::Static) {
        modifiers |= MOD_STATIC;
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
            token_type: TOKEN_FUNCTION,
            modifiers: MOD_DECLARATION,
        });
    }
    if code.starts_with("const ") {
        return Some(SemanticClass {
            token_type: TOKEN_VARIABLE,
            modifiers: MOD_DECLARATION | MOD_READONLY,
        });
    }
    if code.starts_with("static ") {
        return Some(SemanticClass {
            token_type: TOKEN_VARIABLE,
            modifiers: MOD_DECLARATION
                | MOD_STATIC
                | if code.contains(" mut ") {
                    0
                } else {
                    MOD_READONLY
                },
        });
    }
    if code.starts_with("var ") {
        return Some(SemanticClass {
            token_type: TOKEN_VARIABLE,
            modifiers: MOD_DECLARATION
                | if code.contains(" mut ") {
                    0
                } else {
                    MOD_READONLY
                },
        });
    }
    if code.starts_with("struct ") || code.starts_with("union ") {
        return Some(SemanticClass {
            token_type: TOKEN_STRUCT,
            modifiers: MOD_DECLARATION,
        });
    }
    if code.starts_with("enum ") {
        return Some(SemanticClass {
            token_type: TOKEN_ENUM,
            modifiers: MOD_DECLARATION,
        });
    }
    if code.starts_with("trait ") {
        return Some(SemanticClass {
            token_type: TOKEN_INTERFACE,
            modifiers: MOD_DECLARATION,
        });
    }
    if code.starts_with("module ") {
        return Some(SemanticClass {
            token_type: TOKEN_NAMESPACE,
            modifiers: MOD_DECLARATION,
        });
    }
    if code.starts_with("type ") {
        return Some(SemanticClass {
            token_type: if code.contains(" = ") {
                TOKEN_TYPE
            } else {
                TOKEN_TYPE_PARAMETER
            },
            modifiers: MOD_DECLARATION,
        });
    }

    None
}

fn hover_code(contents: &str) -> Option<&str> {
    contents
        .strip_prefix("```kern\n")
        .and_then(|code| code.strip_suffix("\n```"))
}

fn semantic_reference_class(class: SemanticClass) -> SemanticClass {
    SemanticClass {
        token_type: class.token_type,
        modifiers: class.modifiers & !MOD_DECLARATION,
    }
}

fn collect_semantic_token_entries(
    file: &kernc_utils::SourceFile,
    span_classes: &BTreeMap<SpanKey, SemanticClass>,
) -> Vec<SemanticTokenEntry> {
    let mut tokenizer = Tokenizer::new(&file.src, FileId(0));
    let mut tokens = Vec::new();

    loop {
        let token = tokenizer.next_token();
        if token.tag == TokenType::Eof {
            break;
        }
        tokens.push(token);
    }

    let mut entries = Vec::new();
    for (index, token) in tokens.iter().copied().enumerate() {
        let class = match token.tag {
            TokenType::Identifier => heuristic_identifier_class(&tokens, index)
                .or_else(|| span_classes.get(&span_key(token.span)).copied()),
            TokenType::Fn
            | TokenType::Let
            | TokenType::Mut
            | TokenType::Const
            | TokenType::Static
            | TokenType::Type
            | TokenType::Struct
            | TokenType::Union
            | TokenType::Enum
            | TokenType::Trait
            | TokenType::If
            | TokenType::Else
            | TokenType::For
            | TokenType::Break
            | TokenType::Continue
            | TokenType::Return
            | TokenType::Defer
            | TokenType::Pub
            | TokenType::Extern
            | TokenType::Use
            | TokenType::Impl
            | TokenType::True
            | TokenType::False
            | TokenType::Undef
            | TokenType::As
            | TokenType::And
            | TokenType::Or
            | TokenType::Underscore
            | TokenType::SelfType
            | TokenType::SelfValue
            | TokenType::Match
            | TokenType::Mod
            | TokenType::Where
            | TokenType::CapitalFn
            | TokenType::Void => Some(SemanticClass {
                token_type: TOKEN_KEYWORD,
                modifiers: 0,
            }),
            TokenType::StringLiteral | TokenType::CharLiteral | TokenType::ByteCharLiteral => {
                Some(SemanticClass {
                    token_type: TOKEN_STRING,
                    modifiers: 0,
                })
            }
            TokenType::IntLiteral | TokenType::FloatLiteral => Some(SemanticClass {
                token_type: TOKEN_NUMBER,
                modifiers: 0,
            }),
            TokenType::Plus
            | TokenType::Minus
            | TokenType::Star
            | TokenType::Slash
            | TokenType::Percent
            | TokenType::Hash
            | TokenType::At
            | TokenType::Caret
            | TokenType::Bang
            | TokenType::Ampersand
            | TokenType::Pipe
            | TokenType::Tilde
            | TokenType::EqualEqual
            | TokenType::NotEqual
            | TokenType::LessThan
            | TokenType::LessEqual
            | TokenType::GreaterThan
            | TokenType::GreaterEqual
            | TokenType::LShift
            | TokenType::RShift
            | TokenType::Assign
            | TokenType::PlusAssign
            | TokenType::MinusAssign
            | TokenType::StarAssign
            | TokenType::SlashAssign
            | TokenType::PercentAssign
            | TokenType::AmpersandAssign
            | TokenType::PipeAssign
            | TokenType::CaretAssign
            | TokenType::LShiftAssign
            | TokenType::RShiftAssign
            | TokenType::Dot
            | TokenType::DotDot
            | TokenType::DotDotEqual
            | TokenType::DotAmpersand
            | TokenType::DotStar
            | TokenType::DotLBracket
            | TokenType::DotLBrace
            | TokenType::DotDotAmpersand
            | TokenType::DotDotLBracket
            | TokenType::Ellipsis
            | TokenType::Arrow => Some(SemanticClass {
                token_type: TOKEN_OPERATOR,
                modifiers: 0,
            }),
            _ => None,
        };

        let Some(class) = class else {
            continue;
        };
        push_semantic_token_entries(&mut entries, file, token.span.start, token.span.end, class);
    }

    entries
}

fn heuristic_identifier_class(tokens: &[Token], index: usize) -> Option<SemanticClass> {
    if is_parameter_declaration(tokens, index) {
        return Some(SemanticClass {
            token_type: TOKEN_PARAMETER,
            modifiers: MOD_DECLARATION,
        });
    }

    if is_type_context_identifier(tokens, index) {
        return Some(SemanticClass {
            token_type: TOKEN_TYPE,
            modifiers: 0,
        });
    }

    let previous = previous_significant_token(tokens, index)?;
    if previous.tag == TokenType::Dot {
        return Some(SemanticClass {
            token_type: TOKEN_PROPERTY,
            modifiers: 0,
        });
    }

    None
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
                        return previous_significant_token(tokens, token_index(tokens, previous))
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
        TokenType::Colon | TokenType::Arrow | TokenType::As => return true,
        TokenType::Dot => {
            let Some(base_index) = token_index_by_end(tokens, previous.span.start) else {
                return false;
            };
            return is_type_context_identifier(tokens, base_index);
        }
        TokenType::Star
        | TokenType::Ampersand
        | TokenType::DotAmpersand
        | TokenType::DotDotAmpersand
        | TokenType::LBracket
        | TokenType::Comma => {
            if is_nested_in_type_context(tokens, index) {
                return true;
            }
        }
        _ => {}
    }

    false
}

fn is_nested_in_type_context(tokens: &[Token], index: usize) -> bool {
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut idx = index;
    while idx > 0 {
        idx -= 1;
        match tokens[idx].tag {
            TokenType::RParen => paren_depth += 1,
            TokenType::LParen => paren_depth = paren_depth.saturating_sub(1),
            TokenType::RBracket => bracket_depth += 1,
            TokenType::LBracket => bracket_depth = bracket_depth.saturating_sub(1),
            TokenType::Colon | TokenType::Arrow | TokenType::As
                if paren_depth == 0 && bracket_depth == 0 =>
            {
                return true;
            }
            _ => {}
        }
    }

    false
}

fn previous_significant_token(tokens: &[Token], index: usize) -> Option<Token> {
    tokens.get(..index)?.iter().rev().copied().next()
}

fn next_significant_token(tokens: &[Token], index: usize) -> Option<Token> {
    tokens.get(index + 1..)?.iter().copied().next()
}

fn token_index(tokens: &[Token], target: Token) -> usize {
    tokens
        .iter()
        .position(|token| *token == target)
        .expect("token must exist in stream")
}

fn token_index_by_end(tokens: &[Token], end: usize) -> Option<usize> {
    tokens.iter().position(|token| token.span.end == end)
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

fn encode_semantic_tokens(entries: &[SemanticTokenEntry]) -> Vec<u32> {
    let mut sorted = entries.to_vec();
    sorted.sort();

    let mut data = Vec::with_capacity(sorted.len() * 5);
    let mut previous_line = 0;
    let mut previous_start = 0;

    for (index, entry) in sorted.iter().enumerate() {
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

    data
}

fn span_key(span: kernc_utils::Span) -> SpanKey {
    (span.start, span.end)
}
