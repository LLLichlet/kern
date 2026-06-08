//! Byte-oriented tokenizer for Kern source text.
//!
//! The tokenizer owns the first recovery boundary in the compiler: it must
//! always make forward progress, attach file-relative byte spans, preserve doc
//! comments as real tokens, and classify normal whitespace/comments as trivia
//! for tooling.  Parsing of literal values is intentionally left to later
//! stages; this layer only recognizes the lexical shape.

use super::token::{Lexeme, LexemeType, Token, TokenType};
use kernc_utils::{FileId, Span};

#[derive(Clone)]
pub struct Tokenizer<'a> {
    source: &'a [u8],
    file_id: FileId,
    /// Start byte offset of the lexeme currently being scanned.
    start: usize,
    /// One-past-the-last consumed byte in `source`.
    current: usize,
}

impl<'a> Tokenizer<'a> {
    pub fn new(source: &'a str, file_id: FileId) -> Self {
        Self {
            source: source.as_bytes(),
            file_id,
            start: 0,
            current: 0,
        }
    }

    fn emit_lex_error(&mut self, msg: &'static str) -> Token {
        Token {
            tag: TokenType::LexError(msg),
            span: Span {
                file: self.file_id,
                start: self.start,
                end: self.current,
            },
        }
    }

    /// Produce the next token from the input stream.
    pub fn next_token(&mut self) -> Token {
        loop {
            let lexeme = self.next_lexeme();
            match lexeme.tag {
                // The parser sees doc comments because they attach to AST
                // nodes; ordinary comments and whitespace are trivia.
                LexemeType::Whitespace | LexemeType::LineComment | LexemeType::BlockComment => {}
                LexemeType::Token(tag) => {
                    return Token {
                        tag,
                        span: lexeme.span,
                    };
                }
            }
        }
    }

    /// Produce the next lexeme, including comments and whitespace.
    ///
    /// Parser-facing tokenization should keep using `next_token`; this API is
    /// for tools that need a faithful lexical view of source text.
    pub fn next_lexeme(&mut self) -> Lexeme {
        self.start = self.current;

        let c = match self.advance() {
            Some(c) => c,
            None => return self.make_lexeme(LexemeType::Token(TokenType::Eof)),
        };

        match c {
            b' ' | b'\t' | b'\r' | b'\n' => self.scan_whitespace(),
            b'/' if self.peek() == b'/' => self.scan_line_comment_or_doc_comment(),
            b'/' if self.peek() == b'*' => self.scan_block_comment(),
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                // Detect the byte-character prefix `b'`.
                if c == b'b' && self.peek() == b'\'' {
                    self.advance(); // Consume the quote after `b`.
                    return token_lexeme(self.scan_char(TokenType::ByteCharLiteral));
                }
                token_lexeme(self.scan_identifier())
            }
            b'0'..=b'9' => token_lexeme(self.scan_number()),
            b'"' => token_lexeme(self.scan_string()),
            b'\\' => {
                if self.match_char(b'\\') {
                    token_lexeme(self.scan_multiline_string())
                } else {
                    self.make_token_lexeme(TokenType::Illegal)
                }
            }
            b'\'' => token_lexeme(self.scan_char(TokenType::CharLiteral)),

            b'(' => self.make_token_lexeme(TokenType::LParen),
            b')' => self.make_token_lexeme(TokenType::RParen),
            b'{' => self.make_token_lexeme(TokenType::LBrace),
            b'}' => self.make_token_lexeme(TokenType::RBrace),
            b'[' => self.make_token_lexeme(TokenType::LBracket),
            b']' => self.make_token_lexeme(TokenType::RBracket),
            b',' => self.make_token_lexeme(TokenType::Comma),
            b';' => self.make_token_lexeme(TokenType::Semicolon),
            b':' => self.make_token_lexeme(TokenType::Colon),
            b'#' => self.make_token_lexeme(TokenType::Hash),
            b'@' => self.make_token_lexeme(TokenType::At),
            b'?' => self.make_token_lexeme(TokenType::Question),

            b'.' => {
                if self.match_char(b'.') {
                    if self.match_char(b'.') {
                        self.make_token_lexeme(TokenType::Ellipsis)
                    } else if self.match_char(b'=') {
                        self.make_token_lexeme(TokenType::DotDotEqual)
                    } else if self.match_char(b'&') {
                        // Parse `..&` as mutable address-of.
                        self.make_token_lexeme(TokenType::DotDotAmpersand)
                    } else {
                        self.make_token_lexeme(TokenType::DotDot)
                    }
                } else if self.match_char(b'*') {
                    self.make_token_lexeme(TokenType::DotStar)
                } else if self.match_char(b'&') {
                    self.make_token_lexeme(TokenType::DotAmpersand)
                } else if self.match_char(b'?') {
                    self.make_token_lexeme(TokenType::DotQuestion)
                } else if self.match_char(b'[') {
                    self.make_token_lexeme(TokenType::DotLBracket)
                } else if self.match_char(b'{') {
                    self.make_token_lexeme(TokenType::DotLBrace)
                } else {
                    self.make_token_lexeme(TokenType::Dot)
                }
            }

            b'+' => token_lexeme(self.match_assign(TokenType::Plus, TokenType::PlusAssign)),
            b'-' => token_lexeme(self.match_assign(TokenType::Minus, TokenType::MinusAssign)),
            b'*' => token_lexeme(self.match_assign(TokenType::Star, TokenType::StarAssign)),
            b'%' => token_lexeme(self.match_assign(TokenType::Percent, TokenType::PercentAssign)),

            b'/' => {
                if self.match_char(b'=') {
                    self.make_token_lexeme(TokenType::SlashAssign)
                } else {
                    self.make_token_lexeme(TokenType::Slash)
                }
            }

            b'=' => {
                if self.match_char(b'=') {
                    self.make_token_lexeme(TokenType::EqualEqual)
                } else if self.match_char(b'>') {
                    self.make_token_lexeme(TokenType::Arrow)
                } else {
                    self.make_token_lexeme(TokenType::Assign)
                }
            }
            b'!' => {
                if self.match_char(b'=') {
                    self.make_token_lexeme(TokenType::NotEqual)
                } else {
                    self.make_token_lexeme(TokenType::Bang)
                }
            }
            b'<' => {
                if self.match_char(b'<') {
                    if self.match_char(b'=') {
                        return self.make_token_lexeme(TokenType::LShiftAssign);
                    }
                    return self.make_token_lexeme(TokenType::LShift);
                }
                if self.match_char(b'=') {
                    return self.make_token_lexeme(TokenType::LessEqual);
                }
                self.make_token_lexeme(TokenType::LessThan)
            }
            b'>' => {
                if self.match_char(b'>') {
                    if self.match_char(b'=') {
                        return self.make_token_lexeme(TokenType::RShiftAssign);
                    }
                    return self.make_token_lexeme(TokenType::RShift);
                }
                if self.match_char(b'=') {
                    return self.make_token_lexeme(TokenType::GreaterEqual);
                }
                self.make_token_lexeme(TokenType::GreaterThan)
            }

            b'&' => {
                token_lexeme(self.match_assign(TokenType::Ampersand, TokenType::AmpersandAssign))
            }
            b'|' => token_lexeme(self.match_assign(TokenType::Pipe, TokenType::PipeAssign)),
            b'^' => token_lexeme(self.match_assign(TokenType::Caret, TokenType::CaretAssign)),
            b'~' => self.make_token_lexeme(TokenType::Tilde),

            _ => self.make_token_lexeme(TokenType::Illegal),
        }
    }

    // === Core scanning logic ===

    fn scan_whitespace(&mut self) -> Lexeme {
        while matches!(self.peek(), b' ' | b'\t' | b'\r' | b'\n') {
            self.advance();
        }
        self.make_lexeme(LexemeType::Whitespace)
    }

    fn scan_line_comment_or_doc_comment(&mut self) -> Lexeme {
        self.advance(); // Consume the second `/`.
        let doc_kind = match self.peek() {
            // `///` is an outer doc comment, but `////` is just a normal line
            // comment so decorative separator comments do not become docs.
            b'/' if self.peek_next() != b'/' => Some(TokenType::DocCommentOuter),
            b'!' => Some(TokenType::DocCommentInner),
            _ => None,
        };
        if doc_kind.is_some() {
            self.advance();
        }
        while !self.is_eof() && !is_line_break(self.peek()) {
            self.advance();
        }

        if let Some(tag) = doc_kind {
            self.make_token_lexeme(tag)
        } else {
            self.make_lexeme(LexemeType::LineComment)
        }
    }

    fn scan_block_comment(&mut self) -> Lexeme {
        self.advance(); // Consume `*`.
        if self.skip_comment_block() {
            self.make_lexeme(LexemeType::BlockComment)
        } else {
            self.make_token_lexeme(TokenType::LexError("Unterminated multi-line comment"))
        }
    }

    fn scan_identifier(&mut self) -> Token {
        while is_alpha_numeric(self.peek()) {
            self.advance();
        }

        let text = &self.source[self.start..self.current];
        // Resolve identifiers against the keyword table.
        let tag = resolve_keyword(text);
        self.make_token(tag)
    }

    fn scan_number(&mut self) -> Token {
        // 1. Handle radix prefixes such as `0x`, `0b`, and `0o`.
        if self.source[self.start] == b'0' {
            let next_char = self.peek();
            match next_char {
                b'x' | b'X' => {
                    self.advance(); // Consume `x`.
                    self.consume_digits(16);
                    self.consume_numeric_suffix();
                    return self.make_token(TokenType::IntLiteral);
                }
                b'b' | b'B' => {
                    self.advance(); // Consume `b`.
                    self.consume_digits(2);
                    self.consume_numeric_suffix();
                    return self.make_token(TokenType::IntLiteral);
                }
                b'o' | b'O' => {
                    self.advance(); // Consume `o`.
                    self.consume_digits(8);
                    self.consume_numeric_suffix();
                    return self.make_token(TokenType::IntLiteral);
                }
                _ => {
                    // Fall through for plain `0`, decimals like `0.1`, or forms like `0123`.
                }
            }
        }

        // 2. Scan the decimal integer part.
        self.consume_digits(10);

        // 3. Parse the fractional part only when `.` is followed by a digit.
        if self.peek() == b'.' && is_digit(self.peek_next()) {
            self.advance(); // Consume `.`.
            self.consume_digits(10); // Scan the fractional digits.

            // Floats may still carry an exponent suffix such as `1.2e10`.
            self.try_scan_exponent();
            self.consume_numeric_suffix();
            return self.make_token(TokenType::FloatLiteral);
        }

        // 4. Parse exponent-only floats such as `1e10`.
        if self.try_scan_exponent() {
            self.consume_numeric_suffix();
            return self.make_token(TokenType::FloatLiteral);
        }

        // Otherwise this is a plain integer literal.
        self.consume_numeric_suffix();
        self.make_token(TokenType::IntLiteral)
    }

    fn try_scan_exponent(&mut self) -> bool {
        let c = self.peek();
        if c == b'e' || c == b'E' {
            self.advance(); // Consume `e`.

            // Exponents may carry an explicit sign.
            let next_c = self.peek();
            if next_c == b'+' || next_c == b'-' {
                self.advance();
            }

            // Lexing accepts `1e+` as a float-shaped token; literal validation
            // emits the user-facing numeric error with the complete span later.
            self.consume_digits(10);
            return true;
        }
        false
    }

    fn consume_digits(&mut self, radix: u32) {
        loop {
            let c = self.peek();
            if c == b'_' {
                self.advance();
                continue;
            }

            let is_valid = match radix {
                2 => is_bin_digit(c),
                8 => is_oct_digit(c),
                10 => is_digit(c),
                16 => is_hex_digit(c),
                _ => false,
            };

            if is_valid {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn consume_numeric_suffix(&mut self) {
        if is_alpha(self.peek()) {
            // Accept the suffix as part of the literal token even when it is
            // not a known type suffix; semantic parsing reports that error.
            self.advance();
            while is_alpha_numeric(self.peek()) {
                self.advance();
            }
        }
    }

    fn scan_string(&mut self) -> Token {
        let mut has_error = false;

        loop {
            if self.is_eof() {
                // EOF before the closing quote means the string is unterminated.
                return self.emit_lex_error("Unterminated string literal");
            }

            let char = self.peek();
            match char {
                b'\n' | b'\r' => {
                    return self.emit_lex_error("Unterminated string literal before end of line");
                }
                b'"' => {
                    self.advance(); // Consume the closing quote.
                    break;
                }
                b'\\' => {
                    self.advance(); // Consume the backslash.
                    if self.is_eof() {
                        return self
                            .emit_lex_error("Unterminated string literal at escape sequence");
                    }

                    let escaped = self.peek();
                    match escaped {
                        // Simple one-character escape.
                        b'n' | b'r' | b't' | b'\\' | b'\'' | b'\"' | b'0' => {
                            self.advance();
                        }
                        // Hex escape `\xNN`.
                        b'x' => {
                            self.advance();
                            if !self.consume_hex_digits(2) {
                                has_error = true;
                                self.emit_lex_error(
                                    "Invalid hex escape in string: expected 2 hex digits",
                                );
                            }
                        }
                        // Unicode escape `\u{...}`.
                        b'u' => {
                            self.advance();
                            if self.peek() != b'{' {
                                has_error = true;
                                self.emit_lex_error(
                                    "Invalid Unicode escape: expected '{' after '\\u'",
                                );
                                continue;
                            }
                            self.advance(); // Consume `{`.

                            let mut length = 0;
                            while is_hex_digit(self.peek()) {
                                self.advance();
                                length += 1;
                            }

                            if length == 0 || length > 6 {
                                has_error = true;
                                self.emit_lex_error(
                                    "Invalid Unicode escape: expected 1 to 6 hex digits",
                                );
                            }

                            if self.peek() != b'}' {
                                has_error = true;
                                self.emit_lex_error("Invalid Unicode escape: expected '}'");
                            } else {
                                self.advance(); // Consume `}`.
                            }
                        }
                        // Unknown escape sequence.
                        _ => {
                            has_error = true;
                            self.emit_lex_error("Unknown escape sequence in string literal");
                            self.advance(); // Skip the invalid escape and continue scanning.
                        }
                    }
                }
                _ => {
                    self.advance();
                }
            }
        }

        if has_error {
            // Preserve the consumed span but prevent later stages from using it as a value.
            Token {
                tag: TokenType::Illegal,
                span: Span {
                    file: self.file_id,
                    start: self.start,
                    end: self.current,
                },
            }
        } else {
            self.make_token(TokenType::StringLiteral)
        }
    }

    fn scan_multiline_string(&mut self) -> Token {
        loop {
            while !self.is_eof() && !is_line_break(self.peek()) {
                self.advance();
            }

            let line_break_start = self.current;
            if !self.consume_line_break() {
                break;
            }

            while is_horizontal_space(self.peek()) {
                self.advance();
            }

            if self.peek() == b'\\' && self.peek_next() == b'\\' {
                // Kern follows Zig-style multiline strings: every continued
                // source line must start with `\\` after indentation.
                self.advance();
                self.advance();
                continue;
            }

            self.current = line_break_start;
            break;
        }

        self.make_token(TokenType::StringLiteral)
    }

    fn scan_char(&mut self, tag: TokenType) -> Token {
        // The opening quote has already been consumed.
        let c = self.peek();
        let mut has_error = false;

        // 1. Handle escape sequences.
        if c == b'\\' {
            self.advance(); // Consume `\`.

            if self.is_eof() {
                return self.emit_lex_error("Unterminated character literal");
            }

            let escaped = self.peek();
            match escaped {
                // Simple one-character escape.
                b'n' | b'r' | b't' | b'\\' | b'\'' | b'\"' | b'0' => {
                    self.advance();
                }
                // Hex escape: `\xNN`.
                b'x' => {
                    self.advance(); // Consume `x`.
                    if !self.consume_hex_digits(2) {
                        has_error = true;
                        self.emit_lex_error("Invalid hex escape in char: expected 2 hex digits");
                    }
                }
                // Unicode escape: `\u{...}`.
                b'u' => {
                    self.advance(); // Consume `u`.
                    if self.peek() != b'{' {
                        has_error = true;
                        self.emit_lex_error("Invalid Unicode escape: expected '{'");
                    } else {
                        self.advance(); // Consume `{`.

                        let mut length = 0;
                        while is_hex_digit(self.peek()) {
                            self.advance();
                            length += 1;
                        }

                        if length == 0 || length > 6 {
                            has_error = true;
                            self.emit_lex_error(
                                "Invalid Unicode escape: expected 1 to 6 hex digits",
                            );
                        }

                        if self.peek() != b'}' {
                            has_error = true;
                            self.emit_lex_error("Invalid Unicode escape: expected '}'");
                        } else {
                            self.advance(); // Consume `}`.
                        }
                    }
                }
                _ => {
                    has_error = true;
                    self.emit_lex_error("Unknown escape sequence in character literal");
                    self.advance();
                }
            }
        }
        // 2. Handle a normal codepoint, including multibyte UTF-8.
        else if c != b'\'' && c != 0 {
            let len = utf8_byte_sequence_length(c);
            if len == 0 {
                has_error = true;
                self.emit_lex_error("Invalid UTF-8 sequence in character literal");
                self.advance(); // Make forward progress to avoid an infinite loop.
            } else {
                for _ in 0..len {
                    self.advance();
                }
            }
        }
        // 3. Reject empty character literals and unexpected EOF.
        else {
            has_error = true;
            if c == b'\'' {
                self.emit_lex_error("Empty character literal");
            } else {
                return self.emit_lex_error("Unterminated character literal");
            }
        }

        // 4. Character literals must end with a closing quote.
        if self.match_char(b'\'') {
            if has_error {
                // Preserve the full span for diagnostics after recovery.
                Token {
                    tag: TokenType::Illegal,
                    span: Span {
                        file: self.file_id,
                        start: self.start,
                        end: self.current,
                    },
                }
            } else {
                self.make_token(tag)
            }
        } else {
            // Recover from multi-codepoint literals by skipping until a likely boundary.
            while !self.is_eof()
                && self.peek() != b'\''
                && self.peek() != b' '
                && self.peek() != b'\n'
            {
                self.advance();
            }
            // Consume a trailing quote if one is present.
            self.match_char(b'\'');

            self.emit_lex_error(
                "Character literal may only contain one codepoint (or one valid escape)",
            )
        }
    }

    fn consume_hex_digits(&mut self, count: usize) -> bool {
        for _ in 0..count {
            if is_hex_digit(self.peek()) {
                self.advance();
            } else {
                return false;
            }
        }
        true
    }

    // === Helpers ===

    fn advance(&mut self) -> Option<u8> {
        if self.current >= self.source.len() {
            return None;
        }
        let c = self.source[self.current];
        self.current += 1;
        Some(c)
    }

    fn peek(&self) -> u8 {
        if self.current >= self.source.len() {
            // Sentinel byte keeps scanner branches compact.  Source bytes are
            // UTF-8 text, so NUL only matters as ordinary input before EOF.
            return 0;
        }
        self.source[self.current]
    }

    fn peek_next(&self) -> u8 {
        if self.current + 1 >= self.source.len() {
            return 0;
        }
        self.source[self.current + 1]
    }

    fn match_char(&mut self, expected: u8) -> bool {
        if self.current >= self.source.len() {
            return false;
        }
        if self.source[self.current] != expected {
            return false;
        }
        self.current += 1;
        true
    }

    fn consume_line_break(&mut self) -> bool {
        if self.match_char(b'\r') {
            let _ = self.match_char(b'\n');
            return true;
        }
        self.match_char(b'\n')
    }

    fn match_assign(&mut self, single: TokenType, double: TokenType) -> Token {
        if self.match_char(b'=') {
            self.make_token(double)
        } else {
            self.make_token(single)
        }
    }

    fn make_token(&self, tag: TokenType) -> Token {
        Token {
            tag,
            span: Span {
                file: self.file_id,
                start: self.start,
                end: self.current,
            },
        }
    }

    fn make_lexeme(&self, tag: LexemeType) -> Lexeme {
        Lexeme {
            tag,
            span: Span {
                file: self.file_id,
                start: self.start,
                end: self.current,
            },
        }
    }

    fn make_token_lexeme(&self, tag: TokenType) -> Lexeme {
        self.make_lexeme(LexemeType::Token(tag))
    }

    fn skip_comment_block(&mut self) -> bool {
        let mut depth = 1;

        while depth > 0 {
            if self.is_eof() {
                return false;
            }
            let c = self.peek();

            if c == b'/' && self.peek_next() == b'*' {
                self.advance();
                self.advance();
                // Nested block comments are accepted so commented-out regions
                // can contain block comments without breaking recovery.
                depth += 1;
                continue;
            }

            if c == b'*' && self.peek_next() == b'/' {
                self.advance();
                self.advance();
                depth -= 1;
                continue;
            }

            self.advance();
        }
        true
    }

    #[inline]
    fn is_eof(&self) -> bool {
        self.current >= self.source.len()
    }
}

fn token_lexeme(token: Token) -> Lexeme {
    Lexeme {
        tag: LexemeType::Token(token.tag),
        span: token.span,
    }
}

// === Character classification helpers ===

fn is_alpha_numeric(c: u8) -> bool {
    is_alpha(c) || is_digit(c)
}

fn is_line_break(c: u8) -> bool {
    c == b'\n' || c == b'\r'
}

fn is_horizontal_space(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

fn is_alpha(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_digit(c: u8) -> bool {
    c.is_ascii_digit()
}

fn is_hex_digit(c: u8) -> bool {
    is_digit(c) || (b'a'..=b'f').contains(&c) || (b'A'..=b'F').contains(&c)
}

fn is_bin_digit(c: u8) -> bool {
    c == b'0' || c == b'1'
}

fn is_oct_digit(c: u8) -> bool {
    (b'0'..=b'7').contains(&c)
}

// Determine the UTF-8 sequence length from the leading byte.
fn utf8_byte_sequence_length(c: u8) -> usize {
    if c & 0x80 == 0 {
        1 // 0xxxxxxx
    } else if c & 0xE0 == 0xC0 {
        2 // 110xxxxx
    } else if c & 0xF0 == 0xE0 {
        3 // 1110xxxx
    } else if c & 0xF8 == 0xF0 {
        4 // 11110xxx
    } else {
        0 // Invalid
    }
}

// Keyword lookup table.
fn resolve_keyword(text: &[u8]) -> TokenType {
    match text {
        b"fn" => TokenType::Fn,
        b"let" => TokenType::Let,
        b"mut" => TokenType::Mut,
        b"const" => TokenType::Const,
        b"static" => TokenType::Static,
        b"type" => TokenType::Type,
        b"struct" => TokenType::Struct,
        b"union" => TokenType::Union,
        b"enum" => TokenType::Enum,
        b"trait" => TokenType::Trait,
        b"if" => TokenType::If,
        b"else" => TokenType::Else,
        b"for" => TokenType::For,
        b"in" => TokenType::In,
        b"while" => TokenType::While,
        b"break" => TokenType::Break,
        b"continue" => TokenType::Continue,
        b"return" => TokenType::Return,
        b"defer" => TokenType::Defer,
        b"pub" => TokenType::Pub,
        b"extern" => TokenType::Extern,
        b"use" => TokenType::Use,
        b"impl" => TokenType::Impl,
        b"true" => TokenType::True,
        b"false" => TokenType::False,
        b"undef" => TokenType::Undef,
        b"as" => TokenType::As,
        b"and" => TokenType::And,
        b"or" => TokenType::Or,
        b"_" => TokenType::Underscore,
        b"Self" => TokenType::SelfType,
        b"self" => TokenType::SelfValue,
        b"match" => TokenType::Match,
        b"mod" => TokenType::Mod,
        b"where" => TokenType::Where,
        b"void" => TokenType::Void,
        b"Fn" => TokenType::CapitalFn,
        _ => TokenType::Identifier,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lexeme_tags(source: &str) -> Vec<LexemeType> {
        let mut tokenizer = Tokenizer::new(source, FileId(0));
        let mut tags = Vec::new();
        loop {
            let lexeme = tokenizer.next_lexeme();
            tags.push(lexeme.tag);
            if matches!(lexeme.tag, LexemeType::Token(TokenType::Eof)) {
                break;
            }
        }
        tags
    }

    fn token_tags(source: &str) -> Vec<TokenType> {
        let mut tokenizer = Tokenizer::new(source, FileId(0));
        let mut tags = Vec::new();
        loop {
            let token = tokenizer.next_token();
            tags.push(token.tag);
            if token.tag == TokenType::Eof {
                break;
            }
        }
        tags
    }

    #[test]
    fn next_lexeme_keeps_normal_comments_as_trivia() {
        assert_eq!(
            lexeme_tags("value // line\n/* block */ next"),
            vec![
                LexemeType::Token(TokenType::Identifier),
                LexemeType::Whitespace,
                LexemeType::LineComment,
                LexemeType::Whitespace,
                LexemeType::BlockComment,
                LexemeType::Whitespace,
                LexemeType::Token(TokenType::Identifier),
                LexemeType::Token(TokenType::Eof),
            ]
        );
    }

    #[test]
    fn next_token_still_skips_normal_comments_but_keeps_doc_comments() {
        assert_eq!(
            token_tags("value // line\n/// doc\n//! inner\n/* block */ next"),
            vec![
                TokenType::Identifier,
                TokenType::DocCommentOuter,
                TokenType::DocCommentInner,
                TokenType::Identifier,
                TokenType::Eof,
            ]
        );
    }

    #[test]
    fn next_lexeme_reports_unterminated_block_comment_as_lex_error() {
        let tags = lexeme_tags("value /* unterminated");
        assert!(matches!(
            tags.as_slice(),
            [
                LexemeType::Token(TokenType::Identifier),
                LexemeType::Whitespace,
                LexemeType::Token(TokenType::LexError("Unterminated multi-line comment")),
                LexemeType::Token(TokenType::Eof),
            ]
        ));
    }

    #[test]
    fn deterministic_lexer_fuzz_smoke_preserves_progress() {
        for seed in 0..512u64 {
            let source = fuzz_source(seed);
            let lexeme_result = std::panic::catch_unwind(|| assert_lexeme_progress(&source));
            assert!(
                lexeme_result.is_ok(),
                "lexer lexeme fuzz seed {seed} failed with source:\n{source}"
            );

            let token_result = std::panic::catch_unwind(|| assert_token_progress(&source));
            assert!(
                token_result.is_ok(),
                "lexer token fuzz seed {seed} failed with source:\n{source}"
            );
        }
    }

    fn assert_lexeme_progress(source: &str) {
        let mut tokenizer = Tokenizer::new(source, FileId(0));
        let mut previous_end = 0;

        for _ in 0..=source.len() + 1 {
            let lexeme = tokenizer.next_lexeme();
            assert!(lexeme.span.start <= lexeme.span.end, "{lexeme:?}");
            assert!(lexeme.span.end <= source.len(), "{lexeme:?}");
            assert!(
                lexeme.span.start >= previous_end,
                "lexeme span moved backwards: {lexeme:?}, previous end {previous_end}"
            );

            match lexeme.tag {
                LexemeType::Token(TokenType::Eof) => {
                    assert_eq!(lexeme.span.start, source.len(), "{lexeme:?}");
                    assert_eq!(lexeme.span.end, source.len(), "{lexeme:?}");
                    return;
                }
                _ => assert!(
                    lexeme.span.end > lexeme.span.start,
                    "non-eof lexeme made no progress: {lexeme:?}"
                ),
            }

            previous_end = lexeme.span.end;
        }

        panic!("lexer did not reach eof");
    }

    fn assert_token_progress(source: &str) {
        let mut tokenizer = Tokenizer::new(source, FileId(0));
        let mut previous_end = 0;

        for _ in 0..=source.len() + 1 {
            let token = tokenizer.next_token();
            assert!(token.span.start <= token.span.end, "{token:?}");
            assert!(token.span.end <= source.len(), "{token:?}");
            assert!(
                token.span.start >= previous_end,
                "token span moved backwards: {token:?}, previous end {previous_end}"
            );

            if token.tag == TokenType::Eof {
                assert_eq!(token.span.start, source.len(), "{token:?}");
                assert_eq!(token.span.end, source.len(), "{token:?}");
                return;
            }

            assert!(
                token.span.end > token.span.start,
                "non-eof token made no progress: {token:?}"
            );
            previous_end = token.span.end;
        }

        panic!("lexer did not reach eof");
    }

    fn fuzz_source(seed: u64) -> String {
        const FRAGMENTS: &[&str] = &[
            "",
            " ",
            "\t",
            "\n",
            "\r\n",
            "fn",
            "ident",
            "_",
            "Self",
            "self",
            "0",
            "123",
            "0x",
            "0xff",
            "0b102",
            "1e+",
            "1.2e-",
            "123_",
            "\"",
            "\"ok\"",
            "\"\\x",
            "\"\\u{110000}\"",
            "\"unterminated\n",
            "'",
            "''",
            "'x'",
            "'xy'",
            "'\\x'",
            "'\\u{}'",
            "b'",
            "b'\\n'",
            "b'xy'",
            "// comment",
            "/// doc",
            "//// normal",
            "//! inner",
            "/* block */",
            "/* /* nested */ */",
            "/* unterminated",
            "(",
            ")",
            "{",
            "}",
            "[",
            "]",
            ".",
            "..",
            "...",
            "..=",
            "..&",
            ".&",
            ".*",
            ".?",
            ".[",
            ".{",
            "+=",
            "-=",
            "*=",
            "/=",
            "%=",
            "&=",
            "|=",
            "^=",
            "<<=",
            ">>=",
            "@",
            "#",
            "=>",
            "=",
            "==",
            "!",
            "!=",
            "<",
            "<=",
            "<<",
            ">",
            ">=",
            ">>",
            "\\",
            "\\\\multi",
            "\\\\multi\n    \\\\line",
            "\u{e9}",
            "\u{4e2d}",
            "\u{0}",
            "\u{7f}",
        ];

        let mut rng = FuzzRng::new(seed ^ 0xa24b_aed4_963e_e407);
        let mut source = String::new();
        let target_len = 16 + rng.range(160) as usize;

        for index in 0..target_len {
            if index % 23 == 0 {
                source.push('\n');
            } else if rng.range(5) == 0 {
                source.push(' ');
            }

            if rng.range(12) == 0 {
                source.push(char::from(rng.range(128) as u8));
            } else {
                source.push_str(FRAGMENTS[rng.range(FRAGMENTS.len() as u64) as usize]);
            }
        }

        source
    }

    struct FuzzRng {
        state: u64,
    }

    impl FuzzRng {
        fn new(seed: u64) -> Self {
            Self { state: seed | 1 }
        }

        fn next(&mut self) -> u64 {
            let mut x = self.state;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.state = x;
            x
        }

        fn range(&mut self, upper: u64) -> u64 {
            self.next() % upper
        }
    }
}
