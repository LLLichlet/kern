use super::token::{Token, TokenType};
use kernc_utils::{FileId, Span};

#[derive(Clone)]
pub struct Tokenizer<'a> {
    source: &'a [u8],
    file_id: FileId,
    start: usize,   // Start byte offset of the current token.
    current: usize, // Current scan cursor.
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
        if let Some(err_token) = self.skip_whitespace_and_comments() {
            return err_token;
        }

        self.start = self.current;

        let c = match self.advance() {
            Some(c) => c,
            None => return self.make_token(TokenType::Eof),
        };

        match c {
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                // Detect the byte-character prefix `b'`.
                if c == b'b' && self.peek() == b'\'' {
                    self.advance(); // Consume the quote after `b`.
                    return self.scan_char(TokenType::ByteCharLiteral);
                }
                self.scan_identifier()
            }
            b'0'..=b'9' => self.scan_number(),
            b'"' => self.scan_string(),
            b'\\' => {
                if self.match_char(b'\\') {
                    self.scan_multiline_string()
                } else {
                    self.make_token(TokenType::Illegal)
                }
            }
            b'\'' => self.scan_char(TokenType::CharLiteral),

            b'(' => self.make_token(TokenType::LParen),
            b')' => self.make_token(TokenType::RParen),
            b'{' => self.make_token(TokenType::LBrace),
            b'}' => self.make_token(TokenType::RBrace),
            b'[' => self.make_token(TokenType::LBracket),
            b']' => self.make_token(TokenType::RBracket),
            b',' => self.make_token(TokenType::Comma),
            b';' => self.make_token(TokenType::Semicolon),
            b':' => self.make_token(TokenType::Colon),
            b'#' => self.make_token(TokenType::Hash),
            b'@' => self.make_token(TokenType::At),
            b'?' => self.make_token(TokenType::Question),

            b'.' => {
                if self.match_char(b'.') {
                    if self.match_char(b'.') {
                        self.make_token(TokenType::Ellipsis)
                    } else if self.match_char(b'=') {
                        self.make_token(TokenType::DotDotEqual)
                    } else if self.match_char(b'&') {
                        // Parse `..&` as mutable address-of.
                        self.make_token(TokenType::DotDotAmpersand)
                    } else {
                        self.make_token(TokenType::DotDot)
                    }
                } else if self.match_char(b'*') {
                    self.make_token(TokenType::DotStar)
                } else if self.match_char(b'&') {
                    self.make_token(TokenType::DotAmpersand)
                } else if self.match_char(b'?') {
                    self.make_token(TokenType::DotQuestion)
                } else if self.match_char(b'[') {
                    self.make_token(TokenType::DotLBracket)
                } else if self.match_char(b'{') {
                    self.make_token(TokenType::DotLBrace)
                } else {
                    self.make_token(TokenType::Dot)
                }
            }

            b'+' => self.match_assign(TokenType::Plus, TokenType::PlusAssign),
            b'-' => self.match_assign(TokenType::Minus, TokenType::MinusAssign),
            b'*' => self.match_assign(TokenType::Star, TokenType::StarAssign),
            b'%' => self.match_assign(TokenType::Percent, TokenType::PercentAssign),

            b'/' => {
                if self.match_char(b'=') {
                    self.make_token(TokenType::SlashAssign)
                } else {
                    self.make_token(TokenType::Slash)
                }
            }

            b'=' => {
                if self.match_char(b'=') {
                    self.make_token(TokenType::EqualEqual)
                } else if self.match_char(b'>') {
                    self.make_token(TokenType::Arrow)
                } else {
                    self.make_token(TokenType::Assign)
                }
            }
            b'!' => {
                if self.match_char(b'=') {
                    self.make_token(TokenType::NotEqual)
                } else {
                    self.make_token(TokenType::Bang)
                }
            }
            b'<' => {
                if self.match_char(b'<') {
                    if self.match_char(b'=') {
                        return self.make_token(TokenType::LShiftAssign);
                    }
                    return self.make_token(TokenType::LShift);
                }
                if self.match_char(b'=') {
                    return self.make_token(TokenType::LessEqual);
                }
                self.make_token(TokenType::LessThan)
            }
            b'>' => {
                if self.match_char(b'>') {
                    if self.match_char(b'=') {
                        return self.make_token(TokenType::RShiftAssign);
                    }
                    return self.make_token(TokenType::RShift);
                }
                if self.match_char(b'=') {
                    return self.make_token(TokenType::GreaterEqual);
                }
                self.make_token(TokenType::GreaterThan)
            }

            b'&' => self.match_assign(TokenType::Ampersand, TokenType::AmpersandAssign),
            b'|' => self.match_assign(TokenType::Pipe, TokenType::PipeAssign),
            b'^' => self.match_assign(TokenType::Caret, TokenType::CaretAssign),
            b'~' => self.make_token(TokenType::Tilde),

            _ => self.make_token(TokenType::Illegal),
        }
    }

    // === Core scanning logic ===

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

    fn peek_nth(&self, offset: usize) -> u8 {
        self.source.get(self.current + offset).copied().unwrap_or(0)
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

    fn skip_whitespace_and_comments(&mut self) -> Option<Token> {
        loop {
            if self.is_eof() {
                break;
            }
            let c = self.peek();
            match c {
                b' ' | b'\t' | b'\r' | b'\n' => {
                    self.advance();
                }
                b'/' => {
                    if self.peek_next() == b'/' {
                        let doc_kind = match self.peek_nth(2) {
                            b'/' if self.peek_nth(3) != b'/' => Some(TokenType::DocCommentOuter),
                            b'!' => Some(TokenType::DocCommentInner),
                            _ => None,
                        };
                        if let Some(tag) = doc_kind {
                            self.start = self.current;
                            self.advance();
                            self.advance();
                            self.advance();
                            while !self.is_eof() && !is_line_break(self.peek()) {
                                self.advance();
                            }
                            return Some(self.make_token(tag));
                        }

                        // Skip a line comment.
                        while !self.is_eof() && self.peek() != b'\n' {
                            self.advance();
                        }
                    } else if self.peek_next() == b'*' {
                        let start_pos = self.current; // Remember the start of `/*`.
                        self.advance(); // Consume `/`.
                        self.advance(); // Consume `*`.

                        if !self.skip_comment_block() {
                            // Surface unterminated block comments as lexer errors.
                            return Some(Token {
                                tag: TokenType::LexError("Unterminated multi-line comment"),
                                span: Span {
                                    file: self.file_id,
                                    start: start_pos,
                                    end: self.current,
                                },
                            });
                        }
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        None // Completed normally with no lexer error.
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
