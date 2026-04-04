use super::{AnalysisOutcome, TextDocumentContentChangeEvent};
use crate::protocol::{Position, Range};
use kernc_lexer::{Token, TokenType, Tokenizer};
use kernc_utils::FileId;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompletionContext {
    Value,
    Type,
}

const VALUE_KEYWORD_COMPLETIONS: &[&str] = &[
    "let", "mut", "const", "static", "type", "return", "if", "else", "for", "break", "continue",
    "defer", "match", "pub", "extern", "use", "impl", "mod", "true", "false", "undef", "as", "and",
    "or", "self", "Self",
];

const TYPE_KEYWORD_COMPLETIONS: &[&str] = &[
    "Self", "void", "fn", "Fn", "struct", "union", "enum", "trait",
];

pub(super) fn apply_content_change(
    path: &Path,
    text: &mut String,
    change: &TextDocumentContentChangeEvent,
) -> Result<(), String> {
    let Some(range) = &change.range else {
        text.clear();
        text.push_str(&change.text);
        return Ok(());
    };

    let file = kernc_utils::SourceFile::new(path.to_path_buf(), text.clone());
    let Some(start) = position_to_byte_offset(&file, &range.start) else {
        return Err(format!(
            "received incremental change with invalid start position {}:{}",
            range.start.line, range.start.character
        ));
    };
    let Some(end) = position_to_byte_offset(&file, &range.end) else {
        return Err(format!(
            "received incremental change with invalid end position {}:{}",
            range.end.line, range.end.character
        ));
    };
    if start > end {
        return Err("received incremental change with a reversed range".to_string());
    }

    text.replace_range(start..end, &change.text);
    Ok(())
}

pub(super) fn match_position_in_file(
    file: &kernc_utils::SourceFile,
    target_path: &Path,
    position: &Position,
) -> Option<usize> {
    if normalize_path(&file.path) != target_path {
        return None;
    }

    position_to_byte_offset(file, position)
}

pub(super) fn span_to_range(session: &kernc_utils::Session, span: kernc_utils::Span) -> Range {
    let Some(file) = session.source_manager.get_file(span.file) else {
        return empty_range();
    };

    Range {
        start: byte_offset_to_position(file, span.start),
        end: byte_offset_to_position(file, span.end),
    }
}

pub(super) fn byte_offset_to_position(file: &kernc_utils::SourceFile, offset: usize) -> Position {
    let clamped = offset.min(file.src.len());
    let line = file.lookup_line(clamped);
    let line_start = file.line_starts[line.saturating_sub(1)];
    let character = file.src[line_start..clamped].encode_utf16().count() as u32;

    Position {
        line: line.saturating_sub(1) as u32,
        character,
    }
}

pub(super) fn position_to_byte_offset(
    file: &kernc_utils::SourceFile,
    position: &Position,
) -> Option<usize> {
    let line_index = usize::try_from(position.line).ok()?;
    let line_start = *file.line_starts.get(line_index)?;
    let next_line_start = file
        .line_starts
        .get(line_index + 1)
        .copied()
        .unwrap_or(file.src.len());
    let line_end = trim_line_ending(&file.src, line_start, next_line_start);
    let line = &file.src[line_start..line_end];
    let target_units = position.character;

    let mut utf16_units = 0;
    for (byte_idx, ch) in line.char_indices() {
        if utf16_units == target_units {
            return Some(line_start + byte_idx);
        }

        utf16_units += ch.len_utf16() as u32;
        if utf16_units > target_units {
            return None;
        }
    }

    if utf16_units == target_units {
        Some(line_start + line.len())
    } else {
        None
    }
}

pub(super) fn trim_line_ending(source: &str, start: usize, end: usize) -> usize {
    let mut trimmed_end = end;

    if trimmed_end > start && source.as_bytes()[trimmed_end - 1] == b'\n' {
        trimmed_end -= 1;
    }
    if trimmed_end > start && source.as_bytes()[trimmed_end - 1] == b'\r' {
        trimmed_end -= 1;
    }

    trimmed_end
}

pub(super) fn span_contains_offset(span: kernc_utils::Span, offset: usize) -> bool {
    let end = if span.end > span.start {
        span.end
    } else {
        span.start.saturating_add(1)
    };
    offset >= span.start && offset < end
}

pub(super) fn empty_range() -> Range {
    Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: 0,
            character: 0,
        },
    }
}

pub(super) fn single_server_diagnostic(uri: String, message: impl Into<String>) -> AnalysisOutcome {
    AnalysisOutcome {
        bundles: vec![super::DiagnosticBundle {
            uri,
            diagnostics: vec![crate::protocol::Diagnostic {
                range: empty_range(),
                severity: 2,
                source: "kern-lsp",
                message: message.into(),
                code: None,
                tags: None,
                related_information: None,
            }],
        }],
    }
}

pub(super) fn uri_to_file_path(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file://")?;
    let decoded = percent_decode(raw).ok()?;

    #[cfg(windows)]
    {
        let trimmed = decoded.strip_prefix('/').unwrap_or(&decoded);
        let with_separators = trimmed.replace('/', "\\");
        Some(PathBuf::from(with_separators))
    }

    #[cfg(not(windows))]
    {
        Some(PathBuf::from(decoded))
    }
}

pub(super) fn file_path_to_uri(path: &Path) -> io::Result<String> {
    let normalized =
        normalize_platform_path(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
    let raw = normalized.to_string_lossy();

    #[cfg(windows)]
    {
        let slash_path = raw.replace('\\', "/");
        Ok(format!("file:///{}", percent_encode(&slash_path)))
    }

    #[cfg(not(windows))]
    {
        Ok(format!("file://{}", percent_encode(&raw)))
    }
}

pub(super) fn normalize_path(path: &Path) -> PathBuf {
    normalize_platform_path(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
}

fn normalize_platform_path(path: PathBuf) -> PathBuf {
    let path = strip_windows_verbatim_prefix(path);
    strip_macos_private_var_prefix(path)
}

#[cfg(windows)]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix("\\\\?\\UNC\\") {
        return PathBuf::from(format!("\\\\{stripped}"));
    }
    if let Some(stripped) = raw.strip_prefix("\\\\?\\") {
        return PathBuf::from(stripped);
    }
    path
}

#[cfg(not(windows))]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    path
}

#[cfg(target_os = "macos")]
fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix("/private/var/") {
        return PathBuf::from(format!("/var/{stripped}"));
    }
    if raw == "/private/var" {
        return PathBuf::from("/var");
    }
    path
}

#[cfg(not(target_os = "macos"))]
fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
    path
}

fn percent_decode(input: &str) -> Result<String, ()> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;

    while idx < bytes.len() {
        match bytes[idx] {
            b'%' => {
                if idx + 2 >= bytes.len() {
                    return Err(());
                }
                let hi = hex_value(bytes[idx + 1]).ok_or(())?;
                let lo = hex_value(bytes[idx + 2]).ok_or(())?;
                out.push((hi << 4) | lo);
                idx += 3;
            }
            b => {
                out.push(b);
                idx += 1;
            }
        }
    }

    String::from_utf8(out).map_err(|_| ())
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());

    for byte in input.bytes() {
        if is_unreserved_uri_byte(byte) || byte == b'/' || byte == b':' {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(hex_digit(byte >> 4));
            out.push(hex_digit(byte & 0x0f));
        }
    }

    out
}

fn is_unreserved_uri_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn hex_digit(value: u8) -> char {
    match value & 0x0f {
        0..=9 => (b'0' + (value & 0x0f)) as char,
        10..=15 => (b'A' + ((value & 0x0f) - 10)) as char,
        _ => unreachable!(),
    }
}

pub(super) fn is_valid_identifier(name: &str) -> bool {
    if name.is_empty() || name == "_" || is_keyword(name) {
        return false;
    }

    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !is_identifier_start(first) {
        return false;
    }

    bytes.all(is_identifier_continue)
}

pub(super) fn completion_prefix(text: &str, offset: usize) -> &str {
    let clamped = offset.min(text.len());
    let bytes = text.as_bytes();

    if bytes.get(clamped) == Some(&b'.') {
        return "";
    }

    let mut start = clamped;

    while start > 0 && is_identifier_continue(bytes[start - 1]) {
        start -= 1;
    }

    &text[start..clamped]
}

pub(super) fn has_following_call_paren(text: &str, offset: usize) -> bool {
    let bytes = text.as_bytes();
    let mut index = offset.min(bytes.len());

    while let Some(byte) = bytes.get(index).copied() {
        match byte {
            b' ' | b'\t' => index += 1,
            b'(' => return true,
            b'\r' | b'\n' => return false,
            _ => return false,
        }
    }

    false
}

pub(super) fn completion_context(text: &str, offset: usize) -> CompletionContext {
    let prefix_start = completion_prefix_start(text, offset);
    let mut tokenizer = Tokenizer::new(&text[..prefix_start], FileId(0));
    let mut tokens = Vec::new();

    loop {
        let token = tokenizer.next_token();
        if token.tag == TokenType::Eof {
            break;
        }
        tokens.push(token);
    }

    classify_completion_context(&tokens)
}

pub(super) fn completion_is_member_access(text: &str, offset: usize) -> bool {
    let start = completion_prefix_start(text, offset);
    if start == 0 {
        return false;
    }
    text.as_bytes().get(start - 1) == Some(&b'.')
}

pub(super) fn keyword_completion_labels(
    prefix: &str,
    context: CompletionContext,
    member_access: bool,
) -> Vec<&'static str> {
    if prefix.is_empty() || member_access {
        return Vec::new();
    }

    let keywords = match context {
        CompletionContext::Value => VALUE_KEYWORD_COMPLETIONS,
        CompletionContext::Type => TYPE_KEYWORD_COMPLETIONS,
    };

    keywords
        .iter()
        .copied()
        .filter(|keyword| keyword.starts_with(prefix))
        .collect()
}

fn classify_completion_context(tokens: &[Token]) -> CompletionContext {
    let Some(last) = tokens.last() else {
        return CompletionContext::Value;
    };

    match last.tag {
        TokenType::As => CompletionContext::Type,
        TokenType::Colon if colon_prefers_type_context(tokens) => CompletionContext::Type,
        TokenType::Assign if assign_prefers_type_context(tokens) => CompletionContext::Type,
        _ => CompletionContext::Value,
    }
}

fn colon_prefers_type_context(tokens: &[Token]) -> bool {
    for token in tokens.iter().rev().skip(1) {
        match token.tag {
            TokenType::DotLBrace => return false,
            TokenType::Where
            | TokenType::Let
            | TokenType::Const
            | TokenType::Static
            | TokenType::Fn
            | TokenType::CapitalFn
            | TokenType::Struct
            | TokenType::Union
            | TokenType::Trait => return true,
            TokenType::Semicolon | TokenType::Assign | TokenType::Return => return false,
            _ => {}
        }
    }

    false
}

fn assign_prefers_type_context(tokens: &[Token]) -> bool {
    for token in tokens.iter().rev().skip(1) {
        match token.tag {
            TokenType::Type => return true,
            TokenType::Semicolon
            | TokenType::LBrace
            | TokenType::RBrace
            | TokenType::Return
            | TokenType::Let
            | TokenType::Const
            | TokenType::Static
            | TokenType::Fn
            | TokenType::CapitalFn => return false,
            _ => {}
        }
    }

    false
}

fn completion_prefix_start(text: &str, offset: usize) -> usize {
    let clamped = offset.min(text.len());
    let bytes = text.as_bytes();
    let mut start = clamped;

    while start > 0 && is_identifier_continue(bytes[start - 1]) {
        start -= 1;
    }

    start
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_identifier_continue(byte: u8) -> bool {
    is_identifier_start(byte) || byte.is_ascii_digit()
}

fn is_keyword(name: &str) -> bool {
    matches!(
        name,
        "fn" | "let"
            | "mut"
            | "const"
            | "static"
            | "type"
            | "struct"
            | "union"
            | "enum"
            | "trait"
            | "if"
            | "else"
            | "for"
            | "break"
            | "continue"
            | "return"
            | "defer"
            | "pub"
            | "extern"
            | "use"
            | "impl"
            | "true"
            | "false"
            | "undef"
            | "as"
            | "and"
            | "or"
            | "Self"
            | "self"
            | "match"
            | "mod"
            | "where"
            | "void"
            | "Fn"
    )
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    #[test]
    fn normalize_platform_path_strips_private_var_prefix() {
        assert_eq!(
            super::normalize_platform_path(std::path::PathBuf::from(
                "/private/var/folders/example",
            )),
            std::path::PathBuf::from("/var/folders/example")
        );
    }
}
