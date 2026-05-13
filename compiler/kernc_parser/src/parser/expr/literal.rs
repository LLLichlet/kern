use super::super::{ParseError, ParseResult, Parser};
use super::Precedence;
use kernc_ast::*;
use kernc_lexer::{Token, TokenType};
use kernc_utils::{Span, SymbolId};

impl<'a> Parser<'a> {
    pub(super) fn parse_literal_expr(&mut self, token: Token) -> ParseResult<Expr> {
        let span = token.span;
        match token.tag {
            TokenType::IntLiteral => {
                let text = self.source_slice(span).to_string();
                let (text_clean, suffix) = split_integer_literal_suffix(&text).map_err(|msg| {
                    self.add_error(span, msg);
                    ParseError
                })?;
                let (radix, num_str) = if let Some(stripped) = text_clean.strip_prefix("0x") {
                    (16, stripped)
                } else if let Some(stripped) = text_clean.strip_prefix("0b") {
                    (2, stripped)
                } else if let Some(stripped) = text_clean.strip_prefix("0o") {
                    (8, stripped)
                } else {
                    (10, text_clean.as_str())
                };

                let val = u128::from_str_radix(num_str, radix).map_err(|_| {
                    self.add_error(span, format!("Invalid integer literal: {}", text));
                    ParseError
                })?;
                Ok(Expr {
                    id: self.new_id(),
                    span,
                    kind: ExprKind::Integer { value: val, suffix },
                })
            }
            TokenType::FloatLiteral => {
                let text = self.source_slice(span).to_string();
                let (digits, suffix) = split_float_literal_suffix(&text).map_err(|msg| {
                    self.add_error(span, msg);
                    ParseError
                })?;
                let text_clean = digits.replace("_", "");
                let val = text_clean.parse::<f64>().map_err(|_| {
                    self.add_error(span, format!("Invalid float literal: {}", text_clean));
                    ParseError
                })?;
                Ok(Expr {
                    id: self.new_id(),
                    span,
                    kind: ExprKind::Float { value: val, suffix },
                })
            }
            TokenType::StringLiteral => {
                let sid = self.parse_string_literal(token)?;
                Ok(Expr {
                    id: self.new_id(),
                    span,
                    kind: ExprKind::String(self.session.resolve(sid).to_string()),
                })
            }
            TokenType::CharLiteral => self.parse_char_literal(token),
            TokenType::ByteCharLiteral => self.parse_byte_char_literal(token),
            _ => unreachable!(),
        }
    }

    fn parse_char_literal(&mut self, token: Token) -> ParseResult<Expr> {
        let span = token.span;
        let raw = self.source_slice(span).to_string();
        let inner = &raw[1..raw.len() - 1];

        let c = if inner.is_empty() {
            self.add_error(span, "Empty character literal".to_string());
            '\0'
        } else {
            match self.unescape_string(inner, span) {
                Ok(unescaped) => {
                    let mut chars = unescaped.chars();
                    if let Some(ch) = chars.next() {
                        if chars.next().is_some() {
                            self.add_error(
                                span,
                                "Character literal may only contain one character".to_string(),
                            );
                        }
                        ch
                    } else {
                        self.add_error(
                            span,
                            "Empty character literal after unescaping".to_string(),
                        );
                        '\0'
                    }
                }
                Err(ParseError) => '\0',
            }
        };

        Ok(Expr {
            id: self.new_id(),
            span,
            kind: ExprKind::Char(c),
        })
    }

    fn parse_byte_char_literal(&mut self, token: Token) -> ParseResult<Expr> {
        let span = token.span;
        let raw = self.source_slice(span).to_string();
        let inner = &raw[2..raw.len() - 1];

        let byte_val = if inner.is_empty() {
            self.add_error(span, "Empty byte character literal".to_string());
            0u8
        } else {
            match self.unescape_string(inner, span) {
                Ok(unescaped) => {
                    let mut chars = unescaped.chars();
                    if let Some(ch) = chars.next() {
                        if chars.next().is_some() {
                            self.add_error(
                                span,
                                "Byte character literal may only contain one byte".to_string(),
                            );
                        }
                        if ch as u32 > 255 {
                            self.add_error(
                                span,
                                "Byte character literal must be an ASCII character or a valid byte escape (<= 0xFF)"
                                    .to_string(),
                            );
                            0u8
                        } else {
                            ch as u8
                        }
                    } else {
                        self.add_error(
                            span,
                            "Empty byte character literal after unescaping".to_string(),
                        );
                        0u8
                    }
                }
                Err(ParseError) => 0u8,
            }
        };

        Ok(Expr {
            id: self.new_id(),
            span,
            kind: ExprKind::ByteChar(byte_val),
        })
    }

    pub(super) fn parse_enum_literal_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        if self.check(TokenType::Identifier) {
            let id_token = self.advance();
            let sid = self.intern_token(id_token);
            Ok(Expr {
                id: self.new_id(),
                span: start_span.to(id_token.span),
                kind: ExprKind::EnumLiteral {
                    variant: sid,
                    variant_span: id_token.span,
                },
            })
        } else {
            self.add_error(
                start_span,
                "Unexpected `.` at start of expression; expected an enum variant name".to_string(),
            );
            Err(ParseError)
        }
    }

    pub(super) fn parse_unary_prefix_expr(&mut self, token: Token) -> ParseResult<Expr> {
        let op = match token.tag {
            TokenType::Minus => UnaryOperator::Negate,
            TokenType::Bang => UnaryOperator::LogicalNot,
            TokenType::Tilde => UnaryOperator::BitwiseNot,
            TokenType::Hash => UnaryOperator::MetaOf,
            _ => unreachable!(),
        };
        let operand = self.parse_expression(Precedence::Unary)?;
        Ok(Expr {
            id: self.new_id(),
            span: token.span.to(operand.span),
            kind: ExprKind::Unary {
                op,
                operand: Box::new(operand),
            },
        })
    }

    pub(super) fn parse_grouped_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        let expr = self.parse_expression(Precedence::Lowest)?;
        let rparen = self.expect(TokenType::RParen)?;
        Ok(Expr {
            id: self.new_id(),
            span: start_span.to(rparen.span),
            kind: ExprKind::Grouped {
                expr: Box::new(expr),
            },
        })
    }

    pub(super) fn parse_return_expr(&mut self, span: Span) -> ParseResult<Expr> {
        let mut val = None;
        let mut expr_span = span;
        let is_stopper = self.check(TokenType::Semicolon)
            || self.check(TokenType::RBrace)
            || self.check(TokenType::Else)
            || self.check(TokenType::RParen)
            || self.check(TokenType::RBracket)
            || self.check(TokenType::Comma)
            || self.check(TokenType::Eof);
        if !is_stopper {
            let value = self.parse_expression(Precedence::Lowest)?;
            expr_span = span.to(value.span);
            val = Some(Box::new(value));
        }
        Ok(Expr {
            id: self.new_id(),
            span: expr_span,
            kind: ExprKind::Return(val),
        })
    }

    pub(super) fn parse_intrinsic_expr(&mut self, at_token: Token) -> ParseResult<Expr> {
        let id_token = self.expect(TokenType::Identifier)?;
        let sym = self.intern_token(id_token);
        let name_str = format!("@{}", self.session.resolve(sym));
        let sym_id = self.session.intern(&name_str);
        Ok(Expr {
            id: self.new_id(),
            span: at_token.span.to(id_token.span),
            kind: ExprKind::Identifier(sym_id),
        })
    }

    pub fn parse_string_literal(&mut self, token: Token) -> ParseResult<SymbolId> {
        let raw = self.source_slice(token.span).to_string();

        if raw.starts_with('"') {
            if raw.len() < 2 || !raw.ends_with('"') {
                self.session
                    .struct_error(token.span, "invalid or unterminated string literal")
                    .with_hint("ensure the string is properly enclosed in double quotes `\"`")
                    .emit();
                return Err(ParseError);
            }

            let inner = &raw[1..raw.len() - 1];
            let unescaped = self.unescape_string(inner, token.span)?;
            return Ok(self.session.intern(&unescaped));
        }

        if raw.starts_with("\\\\") {
            let cooked = self.parse_multiline_string_literal(&raw, token.span)?;
            return Ok(self.session.intern(&cooked));
        }

        self.session
            .struct_error(token.span, "invalid string literal")
            .with_hint("use either `\"...\"` or Zig-style multiline lines beginning with `\\\\`")
            .emit();
        Err(ParseError)
    }

    fn parse_multiline_string_literal(&mut self, raw: &str, span: Span) -> ParseResult<String> {
        let mut lines = Vec::new();
        for line in raw.lines() {
            let trimmed = line.trim_start_matches([' ', '\t']);
            let Some(content) = trimmed.strip_prefix("\\\\") else {
                self.session
                    .struct_error(span, "invalid multiline string literal continuation")
                    .with_hint("each continued line must begin with `\\\\` after indentation")
                    .emit();
                return Err(ParseError);
            };
            lines.push(content);
        }

        Ok(lines.join("\n"))
    }

    fn unescape_string(&mut self, input: &str, span: Span) -> ParseResult<String> {
        let mut result = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => result.push('\n'),
                    Some('r') => result.push('\r'),
                    Some('t') => result.push('\t'),
                    Some('\\') => result.push('\\'),
                    Some('\'') => result.push('\''),
                    Some('"') => result.push('"'),
                    Some('0') => result.push('\0'),
                    Some('x') => {
                        let hex: String = chars.by_ref().take(2).collect();
                        if hex.len() != 2 {
                            self.add_error(span, "Invalid hex escape sequence".to_string());
                            return Err(ParseError);
                        }
                        let byte = u8::from_str_radix(&hex, 16).map_err(|_| {
                            self.add_error(span, format!("Invalid hex escape: {}", hex));
                            ParseError
                        })?;
                        result.push(byte as char);
                    }
                    Some('u') => {
                        if chars.next() != Some('{') {
                            self.add_error(span, "Expected '{' after \\u".to_string());
                            return Err(ParseError);
                        }

                        let mut hex_str = String::new();
                        let mut found_brace = false;
                        for ch in chars.by_ref() {
                            if ch == '}' {
                                found_brace = true;
                                break;
                            }
                            hex_str.push(ch);
                        }

                        if !found_brace {
                            self.add_error(span, "Unterminated unicode escape".to_string());
                            return Err(ParseError);
                        }

                        let code_point = u32::from_str_radix(&hex_str, 16).map_err(|_| {
                            self.add_error(span, format!("Invalid unicode scalar: {}", hex_str));
                            ParseError
                        })?;

                        if let Some(c) = std::char::from_u32(code_point) {
                            result.push(c);
                        } else {
                            self.add_error(
                                span,
                                format!("Invalid unicode scalar value: {:x}", code_point),
                            );
                            return Err(ParseError);
                        }
                    }
                    Some(c) => {
                        self.session
                            .struct_error(span, format!("unknown escape sequence: `\\{}`", c))
                            .with_hint("if you meant to write a backslash, use `\\\\`")
                            .emit();
                        self.panic_mode = true;
                        return Err(ParseError);
                    }
                    None => {
                        self.add_error(span, "Unterminated escape sequence".to_string());
                        return Err(ParseError);
                    }
                }
            } else {
                result.push(c);
            }
        }
        Ok(result)
    }
}

fn split_integer_literal_suffix(
    text: &str,
) -> Result<(String, Option<NumericLiteralSuffix>), String> {
    for suffix_text in NUMERIC_SUFFIXES {
        if let Some(digits) = text.strip_suffix(suffix_text) {
            if digits.is_empty() {
                continue;
            }
            let digits_clean = digits.replace("_", "");
            if parse_integer_digits(&digits_clean).is_ok() {
                return Ok((
                    digits_clean,
                    parse_numeric_literal_suffix(Some(suffix_text))?,
                ));
            }
        }
    }
    let text_clean = text.replace("_", "");
    if parse_integer_digits(&text_clean).is_ok() {
        Ok((text_clean, None))
    } else if text.bytes().any(|byte| byte.is_ascii_alphabetic()) {
        Err(format!("unsupported numeric literal suffix in `{}`", text))
    } else {
        Ok((text_clean, None))
    }
}

fn split_float_literal_suffix(text: &str) -> Result<(&str, Option<NumericLiteralSuffix>), String> {
    for suffix_text in NUMERIC_SUFFIXES {
        if let Some(digits) = text.strip_suffix(suffix_text) {
            if digits.is_empty() {
                continue;
            }
            return Ok((digits, parse_numeric_literal_suffix(Some(suffix_text))?));
        }
    }
    if text.bytes().any(|byte| byte.is_ascii_alphabetic()) {
        let without_exponent = text
            .trim_start_matches(|ch: char| ch.is_ascii_digit() || ch == '_' || ch == '.')
            .trim_start_matches(['e', 'E', '+', '-']);
        if without_exponent
            .bytes()
            .any(|byte| byte.is_ascii_alphabetic())
        {
            return Err(format!("unsupported numeric literal suffix in `{}`", text));
        }
    }
    Ok((text, None))
}

fn parse_integer_digits(text_clean: &str) -> Result<u128, std::num::ParseIntError> {
    let (radix, num_str) = if let Some(stripped) = text_clean.strip_prefix("0x") {
        (16, stripped)
    } else if let Some(stripped) = text_clean.strip_prefix("0b") {
        (2, stripped)
    } else if let Some(stripped) = text_clean.strip_prefix("0o") {
        (8, stripped)
    } else {
        (10, text_clean)
    };
    u128::from_str_radix(num_str, radix)
}

const NUMERIC_SUFFIXES: &[&str] = &[
    "isize", "usize", "i128", "u128", "i64", "u64", "i32", "u32", "i16", "u16", "f32", "f64", "i8",
    "u8",
];

fn parse_numeric_literal_suffix(
    suffix: Option<&str>,
) -> Result<Option<NumericLiteralSuffix>, String> {
    let Some(suffix) = suffix else {
        return Ok(None);
    };
    let suffix = match suffix {
        "i8" => NumericLiteralSuffix::I8,
        "i16" => NumericLiteralSuffix::I16,
        "i32" => NumericLiteralSuffix::I32,
        "i64" => NumericLiteralSuffix::I64,
        "i128" => NumericLiteralSuffix::I128,
        "isize" => NumericLiteralSuffix::ISize,
        "u8" => NumericLiteralSuffix::U8,
        "u16" => NumericLiteralSuffix::U16,
        "u32" => NumericLiteralSuffix::U32,
        "u64" => NumericLiteralSuffix::U64,
        "u128" => NumericLiteralSuffix::U128,
        "usize" => NumericLiteralSuffix::USize,
        "f32" => NumericLiteralSuffix::F32,
        "f64" => NumericLiteralSuffix::F64,
        other => return Err(format!("unsupported numeric literal suffix `{}`", other)),
    };
    Ok(Some(suffix))
}
