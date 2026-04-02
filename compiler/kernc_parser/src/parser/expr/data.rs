use super::super::{ParseError, ParseResult, Parser};
use super::Precedence;
use kernc_ast::*;
use kernc_lexer::{Token, TokenType};
use kernc_utils::Span;

impl<'a> Parser<'a> {
    pub(super) fn parse_typed_data_init_prefix(&mut self, start_token: Token) -> ParseResult<Expr> {
        let span = start_token.span;

        let type_node = match start_token.tag {
            TokenType::LBracket => {
                if self.match_token(&[TokenType::RBracket]) {
                    let is_mut = self.match_token(&[TokenType::Mut]);
                    let elem = self.parse_type()?;
                    TypeNode {
                        id: self.new_id(),
                        span: span.to(elem.span),
                        kind: TypeKind::Slice {
                            is_mut,
                            elem: Box::new(elem),
                        },
                    }
                } else if self.match_token(&[TokenType::Underscore]) {
                    self.expect(TokenType::RBracket)?;
                    let is_mut = self.match_token(&[TokenType::Mut]);
                    let elem = self.parse_type()?;
                    TypeNode {
                        id: self.new_id(),
                        span: span.to(elem.span),
                        kind: TypeKind::ArrayInfer {
                            is_mut,
                            elem: Box::new(elem),
                        },
                    }
                } else {
                    let len_expr = self.parse_expression(Precedence::Lowest)?;
                    self.expect(TokenType::RBracket)?;
                    let is_mut = self.match_token(&[TokenType::Mut]);
                    let elem = self.parse_type()?;
                    TypeNode {
                        id: self.new_id(),
                        span: span.to(elem.span),
                        kind: TypeKind::Array {
                            is_mut,
                            elem: Box::new(elem),
                            len: Box::new(len_expr),
                        },
                    }
                }
            }
            TokenType::Star => {
                let is_mut = self.match_token(&[TokenType::Mut]);
                let elem = self.parse_type()?;
                TypeNode {
                    id: self.new_id(),
                    span: span.to(elem.span),
                    kind: TypeKind::Pointer {
                        is_mut,
                        elem: Box::new(elem),
                    },
                }
            }
            TokenType::Caret => {
                let is_mut = self.match_token(&[TokenType::Mut]);
                let elem = self.parse_type()?;
                TypeNode {
                    id: self.new_id(),
                    span: span.to(elem.span),
                    kind: TypeKind::VolatilePtr {
                        is_mut,
                        elem: Box::new(elem),
                    },
                }
            }
            TokenType::Struct => self.parse_struct_literal_fields(span, false)?,
            TokenType::Union => self.parse_union_literal_fields(span, false)?,
            TokenType::Enum => self.parse_inline_enum_type(span)?,
            TokenType::Extern => {
                if self.match_token(&[TokenType::Struct]) {
                    self.parse_struct_literal_fields(span, true)?
                } else if self.match_token(&[TokenType::Union]) {
                    self.parse_union_literal_fields(span, true)?
                } else {
                    let err_span = self.peek().span;
                    self.add_error(
                        err_span,
                        "Expected `struct` or `union` after `extern` in typed initialization"
                            .to_string(),
                    );
                    return Err(ParseError);
                }
            }
            _ => unreachable!(),
        };

        self.expect(TokenType::DotLBrace)?;
        self.parse_data_init(Some(Box::new(type_node)), span)
    }

    fn parse_inline_enum_type(&mut self, span: Span) -> ParseResult<TypeNode> {
        let mut backing_type = None;
        if self.match_token(&[TokenType::Colon]) {
            backing_type = Some(Box::new(self.parse_type()?));
        }

        self.expect(TokenType::LBrace)?;
        let mut variants = Vec::new();

        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let name_token = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name_token);

            let mut payload_type = None;
            let mut value = None;

            if self.match_token(&[TokenType::Colon]) {
                payload_type = Some(Box::new(self.parse_type()?));
            } else if self.match_token(&[TokenType::Assign]) {
                value = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
            }

            let mut variant_span = name_token.span;
            if let Some(ref p) = payload_type {
                variant_span = variant_span.to(p.span);
            }
            if let Some(ref v) = value {
                variant_span = variant_span.to(v.span);
            }

            variants.push(EnumVariant {
                name: name_id,
                name_span: name_token.span,
                payload_type,
                value,
                span: variant_span,
            });

            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }

        let end_token = self.expect(TokenType::RBrace)?;
        Ok(TypeNode {
            id: self.new_id(),
            span: span.to(end_token.span),
            kind: TypeKind::Enum {
                backing_type,
                variants,
            },
        })
    }

    fn parse_struct_literal_fields(
        &mut self,
        start_span: Span,
        is_extern: bool,
    ) -> ParseResult<TypeNode> {
        self.expect(TokenType::LBrace)?;

        let mut fields = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let is_pub = self.match_token(&[TokenType::Pub]);
            let name_token = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name_token);
            self.expect(TokenType::Colon)?;
            let field_type = self.parse_type()?;

            let mut default_value = None;
            if self.match_token(&[TokenType::Assign]) {
                default_value = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
            }

            let field_span = name_token.span.to(if let Some(ref v) = default_value {
                v.span
            } else {
                field_type.span
            });

            fields.push(StructFieldDef {
                name: name_id,
                name_span: name_token.span,
                is_pub,
                type_node: field_type,
                default_value,
                span: field_span,
            });

            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }

        let end_token = self.expect(TokenType::RBrace)?;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_span.to(end_token.span),
            kind: TypeKind::Struct { is_extern, fields },
        })
    }

    fn parse_union_literal_fields(
        &mut self,
        start_span: Span,
        is_extern: bool,
    ) -> ParseResult<TypeNode> {
        self.expect(TokenType::LBrace)?;

        let mut fields = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let is_pub = self.match_token(&[TokenType::Pub]);
            let name_token = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name_token);
            self.expect(TokenType::Colon)?;
            let field_type = self.parse_type()?;

            let mut default_value = None;
            if self.match_token(&[TokenType::Assign]) {
                default_value = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
            }

            let field_span = name_token.span.to(if let Some(ref v) = default_value {
                v.span
            } else {
                field_type.span
            });

            fields.push(StructFieldDef {
                name: name_id,
                name_span: name_token.span,
                is_pub,
                type_node: field_type,
                default_value,
                span: field_span,
            });

            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }

        let end_token = self.expect(TokenType::RBrace)?;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_span.to(end_token.span),
            kind: TypeKind::Union { is_extern, fields },
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

                if !self.match_token(&[TokenType::Comma]) {
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
                if !self.match_token(&[TokenType::Comma]) {
                    break;
                }
                if self.check(TokenType::RBracket) {
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
                if !self.match_token(&[TokenType::Comma]) {
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

            if !self.match_token(&[TokenType::Comma]) {
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
