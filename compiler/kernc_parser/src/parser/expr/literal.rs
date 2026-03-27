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
                let text = self.session.source_manager.slice_source(span).to_string();
                let text_clean = text.replace("_", "");
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
                    kind: ExprKind::Integer(val),
                })
            }
            TokenType::FloatLiteral => {
                let text = self
                    .session
                    .source_manager
                    .slice_source(span)
                    .replace("_", "");
                let val = text.parse::<f64>().map_err(|_| {
                    self.add_error(span, format!("Invalid float literal: {}", text));
                    ParseError
                })?;
                Ok(Expr {
                    id: self.new_id(),
                    span,
                    kind: ExprKind::Float(val),
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
        let raw = self.session.source_manager.slice_source(span).to_string();
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
        let raw = self.session.source_manager.slice_source(span).to_string();
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
                kind: ExprKind::EnumLiteral(sid),
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
        let mut expr = self.parse_expression(Precedence::Lowest)?;
        let rparen = self.expect(TokenType::RParen)?;
        expr.span = start_span.to(rparen.span);
        Ok(expr)
    }

    pub(super) fn parse_return_expr(&mut self, span: Span) -> ParseResult<Expr> {
        let mut val = None;
        let is_stopper = self.check(TokenType::Semicolon)
            || self.check(TokenType::RBrace)
            || self.check(TokenType::Else)
            || self.check(TokenType::RParen)
            || self.check(TokenType::RBracket)
            || self.check(TokenType::Comma)
            || self.check(TokenType::Eof);
        if !is_stopper {
            val = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
        }
        Ok(Expr {
            id: self.new_id(),
            span,
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
        let raw = self
            .session
            .source_manager
            .slice_source(token.span)
            .to_string();

        if raw.len() < 2 || !raw.starts_with('"') || !raw.ends_with('"') {
            self.session
                .struct_error(token.span, "invalid or unterminated string literal")
                .with_hint("ensure the string is properly enclosed in double quotes `\"`")
                .emit();
            return Err(ParseError);
        }

        let inner = &raw[1..raw.len() - 1];
        let unescaped = self.unescape_string(inner, token.span)?;
        Ok(self.session.intern(&unescaped))
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
