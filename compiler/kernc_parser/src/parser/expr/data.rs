use super::super::{ParseError, ParseResult, Parser};
use super::Precedence;
use kernc_ast::*;
use kernc_lexer::{Token, TokenType};
use kernc_utils::Span;

impl<'a> Parser<'a> {
    pub(super) fn parse_type_namespace_expr(&mut self, start_token: Token) -> ParseResult<Expr> {
        let type_node = self.parse_type_after_consumed(start_token)?;
        Ok(Expr {
            id: self.new_id(),
            span: type_node.span,
            kind: ExprKind::TypeNode(Box::new(type_node)),
        })
    }

    pub(super) fn parse_closure_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        let mut captures = Vec::new();
        if !self.check(TokenType::RBracket) {
            loop {
                let name_tok = self.expect(TokenType::Identifier)?;
                let name = self.intern_token(name_tok);

                let value = if self.match_token(&[TokenType::Assign]) {
                    self.parse_expression(Precedence::Lowest)?
                } else {
                    Expr {
                        id: self.new_id(),
                        span: name_tok.span,
                        kind: ExprKind::Identifier(name),
                    }
                };

                captures.push(CapturePattern {
                    name,
                    name_span: name_tok.span,
                    value,
                    span: name_tok.span.to(self.stream.prev_span()),
                });

                if !self.continue_after_comma(&[TokenType::RBracket]) {
                    break;
                }
            }
        }
        self.expect(TokenType::RBracket)?;

        let (params, is_variadic) = self.parse_func_params()?;
        if is_variadic {
            self.add_error(
                start_span,
                "Closures cannot use C-style variadic arguments".to_string(),
            );
        }

        let ret_type = self.parse_type()?;
        let brace_token = self.expect(TokenType::LBrace)?;
        let body = self.parse_block_expr(brace_token.span)?;
        let end_span = body.span;

        Ok(Expr {
            id: self.new_id(),
            span: start_span.to(end_span),
            kind: ExprKind::Closure {
                captures,
                params,
                ret_type: Box::new(ret_type),
                body: Box::new(body),
            },
        })
    }

    pub(super) fn parse_slice_or_index_expr(
        &mut self,
        left: Expr,
        is_mut: bool,
    ) -> ParseResult<Expr> {
        let mut start = None;
        let mut end = None;
        let mut is_range = false;
        let mut is_inclusive = false;

        if self.match_token(&[TokenType::DotDot]) {
            is_range = true;
            if !self.check(TokenType::RBracket) {
                end = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
            }
        } else {
            start = Some(Box::new(self.parse_expression(Precedence::Lowest)?));

            if self.match_token(&[TokenType::DotDot]) {
                is_range = true;
                if !self.check(TokenType::RBracket) {
                    end = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
                }
            } else if self.match_token(&[TokenType::DotDotEqual]) {
                is_range = true;
                is_inclusive = true;
                end = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
            }
        }

        let rbracket = self.expect(TokenType::RBracket)?;
        let span = left.span.to(rbracket.span);

        if is_range {
            Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::SliceOp {
                    lhs: Box::new(left),
                    start,
                    end,
                    is_inclusive,
                    is_mut,
                },
            })
        } else {
            let Some(index) = start else {
                self.add_error(
                    span,
                    "Expected an index expression before `]`; ranges must use `..` syntax"
                        .to_string(),
                );
                return Err(ParseError);
            };

            Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::IndexAccess {
                    lhs: Box::new(left),
                    index,
                    is_mut,
                },
            })
        }
    }

    pub(super) fn parse_generic_instantiation_expr(&mut self, left: Expr) -> ParseResult<Expr> {
        let mut types = Vec::new();
        if !self.check(TokenType::RBracket) {
            loop {
                types.push(self.parse_type()?);
                if !self.continue_after_comma(&[TokenType::RBracket]) {
                    break;
                }
            }
        }
        let rb = self.expect(TokenType::RBracket)?;
        Ok(Expr {
            id: self.new_id(),
            span: left.span.to(rb.span),
            kind: ExprKind::GenericInstantiation {
                target: Box::new(left),
                types,
            },
        })
    }

    pub(super) fn parse_data_init(
        &mut self,
        type_node: Option<Box<TypeNode>>,
        start_span: Span,
    ) -> ParseResult<Expr> {
        if self.check(TokenType::RBrace) {
            let rb = self.advance();
            return Ok(Expr {
                id: self.new_id(),
                span: start_span.to(rb.span),
                kind: ExprKind::DataInit {
                    type_node,
                    literal: DataLiteralKind::Array(vec![]),
                },
            });
        }

        let is_struct_mode =
            self.check(TokenType::Identifier) && self.stream.peek_nth(1).tag == TokenType::Colon;

        if is_struct_mode {
            return self.parse_struct_data_init(type_node, start_span);
        }

        let first = self.parse_expression(Precedence::Lowest)?;
        if self.match_token(&[TokenType::Semicolon]) {
            let count = self.parse_expression(Precedence::Lowest)?;
            let rb = self.expect(TokenType::RBrace)?;
            Ok(Expr {
                id: self.new_id(),
                span: start_span.to(rb.span),
                kind: ExprKind::DataInit {
                    type_node,
                    literal: DataLiteralKind::Repeat {
                        value: Box::new(first),
                        count: Box::new(count),
                    },
                },
            })
        } else if self.match_token(&[TokenType::Comma]) {
            let mut elems = vec![first];
            while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
                elems.push(self.parse_expression(Precedence::Lowest)?);
                if !self.continue_after_comma(&[TokenType::RBrace]) {
                    break;
                }
            }
            let rb = self.expect(TokenType::RBrace)?;
            Ok(Expr {
                id: self.new_id(),
                span: start_span.to(rb.span),
                kind: ExprKind::DataInit {
                    type_node,
                    literal: DataLiteralKind::Array(elems),
                },
            })
        } else {
            let rb = self.expect(TokenType::RBrace)?;
            Ok(Expr {
                id: self.new_id(),
                span: start_span.to(rb.span),
                kind: ExprKind::DataInit {
                    type_node,
                    literal: DataLiteralKind::Scalar(Box::new(first)),
                },
            })
        }
    }

    fn parse_struct_data_init(
        &mut self,
        type_node: Option<Box<TypeNode>>,
        start_span: Span,
    ) -> ParseResult<Expr> {
        let mut fields = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let name = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name);

            if self.expect(TokenType::Colon).is_err() {
                let name_str = self.session.resolve(name_id).to_string();
                self.session
                    .struct_error(
                        name.span,
                        "explicit field names are required in struct/union initialization",
                    )
                    .with_hint(format!(
                        "Kern does not support elided fields. Write `{name_str}: {name_str}` instead."
                    ))
                    .emit();
                return Err(ParseError);
            }

            let val = self.parse_expression(Precedence::Lowest)?;
            let field_span = name.span.to(val.span);
            fields.push(StructFieldInit {
                name: name_id,
                name_span: name.span,
                value: val,
                span: field_span,
            });

            if !self.continue_after_comma(&[TokenType::RBrace]) {
                break;
            }
        }

        let rb = self.expect(TokenType::RBrace)?;
        Ok(Expr {
            id: self.new_id(),
            span: start_span.to(rb.span),
            kind: ExprKind::DataInit {
                type_node,
                literal: DataLiteralKind::Struct(fields),
            },
        })
    }
}
