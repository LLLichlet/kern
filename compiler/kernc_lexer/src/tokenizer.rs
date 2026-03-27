use super::token::{Token, TokenType};
use kernc_utils::{FileId, Span};

pub struct Tokenizer<'a> {
    source: &'a [u8],
    file_id: FileId,
    start: usize,   // 当前 Token 的起始位置
    current: usize, // 扫描探针的当前位置
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

    /// 获取下一个 Token
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
                // 嗅探是否是字节字符前缀: b'
                if c == b'b' && self.peek() == b'\'' {
                    self.advance(); // 吃掉单引号 '\''
                    return self.scan_char(TokenType::ByteCharLiteral);
                }
                self.scan_identifier()
            }
            b'0'..=b'9' => self.scan_number(),
            b'"' => self.scan_string(),
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

            b'.' => {
                if self.match_char(b'.') {
                    if self.match_char(b'.') {
                        self.make_token(TokenType::Ellipsis)
                    } else if self.match_char(b'=') {
                        self.make_token(TokenType::DotDotEqual)
                    } else if self.match_char(b'&') {
                        // 解析为 ..& (可变取地址)
                        self.make_token(TokenType::DotDotAmpersand)
                    } else if self.match_char(b'[') {
                        // 解析为 ..[ (可变切片)
                        self.make_token(TokenType::DotDotLBracket)
                    } else {
                        self.make_token(TokenType::DotDot)
                    }
                } else if self.match_char(b'*') {
                    self.make_token(TokenType::DotStar)
                } else if self.match_char(b'&') {
                    self.make_token(TokenType::DotAmpersand)
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

    // === 核心扫描逻辑 ===

    fn scan_identifier(&mut self) -> Token {
        while is_alpha_numeric(self.peek()) {
            self.advance();
        }

        let text = &self.source[self.start..self.current];
        // 查表
        let tag = resolve_keyword(text);
        self.make_token(tag)
    }

    fn scan_number(&mut self) -> Token {
        // 1. 处理进制前缀 (0x, 0b, 0o)
        // source[start] 一定是数字，因为进入此函数前已经判断过
        if self.source[self.start] == b'0' {
            let next_char = self.peek();
            match next_char {
                b'x' | b'X' => {
                    self.advance(); // 吃掉 'x'
                    self.consume_digits(16);
                    return self.make_token(TokenType::IntLiteral);
                }
                b'b' | b'B' => {
                    self.advance(); // 吃掉 'b'
                    self.consume_digits(2);
                    return self.make_token(TokenType::IntLiteral);
                }
                b'o' | b'O' => {
                    self.advance(); // 吃掉 'o'
                    self.consume_digits(8);
                    return self.make_token(TokenType::IntLiteral);
                }
                _ => {
                    // 只是一个普通的 0，或者 0.xxxx，或者 0123
                    // 继续往下走，进入十进制逻辑
                }
            }
        }

        // 2. 扫描整数部分 (十进制)
        self.consume_digits(10);

        // 3. 处理小数部分 (Float)
        // 如果是 '.'，必须确认 '.' 后面紧跟着数字，才算是浮点数。
        // 否则可能是 1.method() 或者是 1..10 (Range)
        if self.peek() == b'.' && is_digit(self.peek_next()) {
            self.advance(); // 吃掉 '.'
            self.consume_digits(10); // 扫描小数部分

            // 扫描完小数后，还可以继续跟指数部分，如 1.2e10
            self.try_scan_exponent();
            return self.make_token(TokenType::FloatLiteral);
        }

        // 4. 处理没有小数点的指数部分 (如 1e10)
        if self.try_scan_exponent() {
            return self.make_token(TokenType::FloatLiteral);
        }

        // 既没有小数点，也没有指数，就是普通的整数
        self.make_token(TokenType::IntLiteral)
    }

    fn try_scan_exponent(&mut self) -> bool {
        let c = self.peek();
        if c == b'e' || c == b'E' {
            self.advance(); // 吃掉 'e'

            // 指数部分可以有正负号
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

    fn scan_string(&mut self) -> Token {
        let mut has_error = false;

        loop {
            if self.is_eof() {
                // 遇到真实的 EOF，说明字符串未闭合
                return self.emit_lex_error("Unterminated string literal");
            }

            let char = self.peek();
            match char {
                b'"' => {
                    self.advance(); // 吞掉右引号
                    break;
                }
                b'\\' => {
                    self.advance(); // 跳过转义符
                    if self.is_eof() {
                        return self
                            .emit_lex_error("Unterminated string literal at escape sequence");
                    }

                    let escaped = self.peek();
                    match escaped {
                        // 合法的单字符转义
                        b'n' | b'r' | b't' | b'\\' | b'\'' | b'\"' | b'0' => {
                            self.advance();
                        }
                        // 十六进制转义 \xNN
                        b'x' => {
                            self.advance();
                            if !self.consume_hex_digits(2) {
                                has_error = true;
                                self.emit_lex_error(
                                    "Invalid hex escape in string: expected 2 hex digits",
                                );
                            }
                        }
                        // Unicode 转义 \u{...}
                        b'u' => {
                            self.advance();
                            if self.peek() != b'{' {
                                has_error = true;
                                self.emit_lex_error(
                                    "Invalid Unicode escape: expected '{' after '\\u'",
                                );
                                continue;
                            }
                            self.advance(); // 吃掉 '{'

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
                                self.advance(); // 吃掉 '}'
                            }
                        }
                        // 未知的转义字符
                        _ => {
                            has_error = true;
                            self.emit_lex_error("Unknown escape sequence in string literal");
                            self.advance(); // 跳过未知的转义字符，继续解析
                        }
                    }
                }
                _ => {
                    self.advance();
                }
            }
        }

        if has_error {
            // 虽然有错误，但已经报过错了，且游标已经推进到了正确的引号后。
            // 返回 Illegal 可以阻止后续的 AST 生成。
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

    fn scan_char(&mut self, tag: TokenType) -> Token {
        // 刚吃掉了左边的单引号 '，现在处于字符内容的第一个字节
        let c = self.peek();
        let mut has_error = false;

        // 1. 处理转义字符 (以 \ 开头)
        if c == b'\\' {
            self.advance(); // 吃掉反斜杠 '\'

            if self.is_eof() {
                return self.emit_lex_error("Unterminated character literal");
            }

            let escaped = self.peek();
            match escaped {
                // 简单单字符转义
                b'n' | b'r' | b't' | b'\\' | b'\'' | b'\"' | b'0' => {
                    self.advance();
                }
                // 十六进制转义: \xNN
                b'x' => {
                    self.advance(); // 吃掉 'x'
                    if !self.consume_hex_digits(2) {
                        has_error = true;
                        self.emit_lex_error("Invalid hex escape in char: expected 2 hex digits");
                    }
                }
                // Unicode 转义: \u{...}
                b'u' => {
                    self.advance(); // 吃掉 'u'
                    if self.peek() != b'{' {
                        has_error = true;
                        self.emit_lex_error("Invalid Unicode escape: expected '{'");
                    } else {
                        self.advance(); // 吃掉 '{'

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
                            self.advance(); // 吃掉 '}'
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
        // 2. 处理普通字符 (包括 UTF-8 多字节字符)
        else if c != b'\'' && c != 0 {
            let len = utf8_byte_sequence_length(c);
            if len == 0 {
                has_error = true;
                self.emit_lex_error("Invalid UTF-8 sequence in character literal");
                self.advance(); // 象征性前进，防止死循环
            } else {
                for _ in 0..len {
                    self.advance();
                }
            }
        }
        // 3. 空字符 '' 或者直接遇到 EOF
        else {
            has_error = true;
            if c == b'\'' {
                self.emit_lex_error("Empty character literal");
            } else {
                return self.emit_lex_error("Unterminated character literal");
            }
        }

        // 4. 必须以单引号闭合
        if self.match_char(b'\'') {
            if has_error {
                // 已经发过错误了，组装一个带 span 的 Illegal token
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
            // 如果遇到别的字符，说明这个 char 包含多个字符但没有引号闭合，或者没写引号
            // 错误恢复：吃掉直到下一个单引号或空格
            while !self.is_eof()
                && self.peek() != b'\''
                && self.peek() != b' '
                && self.peek() != b'\n'
            {
                self.advance();
            }
            // 尝试吃掉那个引号
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

    // === 辅助工具 ===

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
                        // 单行注释
                        while !self.is_eof() && self.peek() != b'\n' {
                            self.advance();
                        }
                    } else if self.peek_next() == b'*' {
                        let start_pos = self.current; // 记录 /* 的起始位置
                        self.advance(); // 吃掉 '/'
                        self.advance(); // 吃掉 '*'

                        if !self.skip_comment_block() {
                            // 返回词法错误
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
        None // 正常结束，没有发生错误
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

// === 字符判断辅助函数 ===

fn is_alpha_numeric(c: u8) -> bool {
    is_alpha(c) || is_digit(c)
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

// 根据 UTF-8 首字节判断字符长度
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

// 关键字映射
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
