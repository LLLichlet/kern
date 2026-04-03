use crate::protocol::SemanticTokens;
use kernc_driver::{
    AnalysisArtifact, AnalysisSemanticEntry, AnalysisSemanticKind, AnalysisSemanticRole,
    AnalysisSymbol, AnalysisSymbolKind,
};
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
    pub(super) const KEYWORD: u32 = 11;
    pub(super) const STRING: u32 = 12;
    pub(super) const NUMBER: u32 = 13;
    pub(super) const OPERATOR: u32 = 14;
}

pub(super) struct SemanticModifiers;

impl SemanticModifiers {
    pub(super) const DECLARATION: u32 = 1 << 0;
    pub(super) const READONLY: u32 = 1 << 1;
    pub(super) const STATIC: u32 = 1 << 2;
}

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
    for entry in &artifact.semantic_entries {
        if entry.role != AnalysisSemanticRole::Definition {
            continue;
        }
        definition_classes
            .entry(entry.definition_span)
            .or_insert_with(|| semantic_class_from_entry(entry));
    }
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

    for entry in &artifact.semantic_entries {
        if entry.role != AnalysisSemanticRole::Reference {
            continue;
        }
        if !super::span_in_path(&artifact.session, entry.span, target_path) {
            continue;
        }
        document_classes.insert(
            span_key(entry.span),
            semantic_reference_class(semantic_class_from_entry(entry)),
        );
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

fn semantic_class_from_entry(entry: &AnalysisSemanticEntry) -> SemanticClass {
    let token_type = match entry.kind {
        AnalysisSemanticKind::Module | AnalysisSemanticKind::Namespace => {
            SemanticTokenTypes::NAMESPACE
        }
        AnalysisSemanticKind::Struct => SemanticTokenTypes::STRUCT,
        AnalysisSemanticKind::Enum => SemanticTokenTypes::ENUM,
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
            TokenType::Identifier => parameter_declaration_class(&tokens, index)
                .or_else(|| span_classes.get(&span_key(token.span)).copied())
                .or_else(|| heuristic_identifier_class(&tokens, index)),
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
                token_type: SemanticTokenTypes::KEYWORD,
                modifiers: 0,
            }),
            TokenType::StringLiteral | TokenType::CharLiteral | TokenType::ByteCharLiteral => {
                Some(SemanticClass {
                    token_type: SemanticTokenTypes::STRING,
                    modifiers: 0,
                })
            }
            TokenType::DocCommentOuter | TokenType::DocCommentInner => None,
            TokenType::IntLiteral | TokenType::FloatLiteral => Some(SemanticClass {
                token_type: SemanticTokenTypes::NUMBER,
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
                token_type: SemanticTokenTypes::OPERATOR,
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
