mod control;
mod data;
mod literal;

use super::{ParseError, ParseResult, Parser};
use kernc_ast::*;
use kernc_lexer::{Token, TokenType};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Precedence {
    Lowest,
    Assignment,
    Range,
    LogicalOr,
    LogicalAnd,
    Equality,
    Comparison,
    Term,
    Factor,
    Cast,
    Unary,
    Call,
}

impl Precedence {
    fn from_token(t: TokenType) -> Self {
        match t {
            TokenType::Dot
            | TokenType::DotLBracket
            | TokenType::DotLBrace
            | TokenType::DotStar
            | TokenType::LParen
            | TokenType::LBracket
            | TokenType::DotAmpersand
            | TokenType::DotDotAmpersand
            | TokenType::Bang
            | TokenType::DotQuestion => Self::Call,
            TokenType::As => Self::Cast,
            TokenType::Ellipsis | TokenType::DotDotEqual => Self::Range,
            TokenType::Star | TokenType::Slash | TokenType::Percent => Self::Factor,
            TokenType::Plus
            | TokenType::Minus
            | TokenType::Pipe
            | TokenType::Caret
            | TokenType::Ampersand
            | TokenType::LShift
            | TokenType::RShift => Self::Term,
            TokenType::LessThan
            | TokenType::LessEqual
            | TokenType::GreaterThan
            | TokenType::GreaterEqual => Self::Comparison,
            TokenType::EqualEqual | TokenType::NotEqual => Self::Equality,
            TokenType::And => Self::LogicalAnd,
            TokenType::Or => Self::LogicalOr,
            TokenType::Assign
            | TokenType::PlusAssign
            | TokenType::MinusAssign
            | TokenType::StarAssign
            | TokenType::SlashAssign
            | TokenType::PercentAssign
            | TokenType::AmpersandAssign
            | TokenType::PipeAssign
            | TokenType::CaretAssign
            | TokenType::LShiftAssign
            | TokenType::RShiftAssign => Self::Assignment,
            _ => Self::Lowest,
        }
    }
}

fn binary_operator_from_token(token: TokenType) -> BinaryOperator {
    match token {
        TokenType::Plus => BinaryOperator::Add,
        TokenType::Minus => BinaryOperator::Subtract,
        TokenType::Star => BinaryOperator::Multiply,
        TokenType::Slash => BinaryOperator::Divide,
        TokenType::Percent => BinaryOperator::Modulo,
        TokenType::EqualEqual => BinaryOperator::Equal,
        TokenType::NotEqual => BinaryOperator::NotEqual,
        TokenType::LessThan => BinaryOperator::LessThan,
        TokenType::GreaterThan => BinaryOperator::GreaterThan,
        TokenType::LessEqual => BinaryOperator::LessOrEqual,
        TokenType::GreaterEqual => BinaryOperator::GreaterOrEqual,
        TokenType::And => BinaryOperator::LogicalAnd,
        TokenType::Or => BinaryOperator::LogicalOr,
        TokenType::Ampersand => BinaryOperator::BitwiseAnd,
        TokenType::Pipe => BinaryOperator::BitwiseOr,
        TokenType::Caret => BinaryOperator::BitwiseXor,
        TokenType::LShift => BinaryOperator::ShiftLeft,
        TokenType::RShift => BinaryOperator::ShiftRight,
        _ => unreachable!("Token {:?} is not a binary operator", token),
    }
}

fn assignment_operator_from_token(token: TokenType) -> AssignmentOperator {
    match token {
        TokenType::Assign => AssignmentOperator::Assign,
        TokenType::PlusAssign => AssignmentOperator::AddAssign,
        TokenType::MinusAssign => AssignmentOperator::SubtractAssign,
        TokenType::StarAssign => AssignmentOperator::MultiplyAssign,
        TokenType::SlashAssign => AssignmentOperator::DivideAssign,
        TokenType::PercentAssign => AssignmentOperator::ModuloAssign,
        TokenType::AmpersandAssign => AssignmentOperator::BitwiseAndAssign,
        TokenType::PipeAssign => AssignmentOperator::BitwiseOrAssign,
        TokenType::CaretAssign => AssignmentOperator::BitwiseXorAssign,
        TokenType::LShiftAssign => AssignmentOperator::ShiftLeftAssign,
        TokenType::RShiftAssign => AssignmentOperator::ShiftRightAssign,
        _ => unreachable!("Token {:?} is not an assignment operator", token),
    }
}

fn expr_can_prefix_data_init_type(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Grouped { expr: inner } => expr_can_prefix_data_init_type(inner),
        ExprKind::Identifier(_)
        | ExprKind::AnchoredPath { .. }
        | ExprKind::TypeNode(_)
        | ExprKind::FieldAccess { .. }
        | ExprKind::GenericInstantiation { .. } => true,
        _ => false,
    }
}

fn token_ends_unclosed_call_argument_list(token: TokenType) -> bool {
    matches!(
        token,
        TokenType::Semicolon | TokenType::RBrace | TokenType::Eof
    )
}

impl<'a> Parser<'a> {
    fn token_can_end_missing_expr(tag: TokenType) -> bool {
        matches!(
            tag,
            TokenType::Semicolon
                | TokenType::Comma
                | TokenType::RParen
                | TokenType::RBrace
                | TokenType::RBracket
                | TokenType::Arrow
                | TokenType::Eof
        )
    }

    pub fn parse_binding_pattern(&mut self) -> ParseResult<BindingPattern> {
        let mut is_mut = false;
        let start_span = self.peek().span;

        if self.match_token(&[TokenType::Mut]) {
            is_mut = true;
        }

        let name_token = if self.match_token(&[TokenType::Underscore]) {
            Token {
                tag: TokenType::Underscore,
                span: self.stream.prev_span(),
            }
        } else {
            self.expect(TokenType::Identifier)?
        };

        let span = if is_mut {
            start_span.to(name_token.span)
        } else {
            name_token.span
        };

        Ok(BindingPattern {
            name: self.intern_token(name_token),
            name_span: name_token.span,
            is_mut,
            span,
        })
    }

    pub fn parse_expression(&mut self, precedence: Precedence) -> ParseResult<Expr> {
        self.check_canceled()?;
        let current = self.peek();
        if Self::token_can_end_missing_expr(current.tag) {
            return Ok(self.error_expr(current.span, "Expected expression"));
        }

        let prefix_token = self.advance();
        let mut left = self.parse_prefix(prefix_token)?;

        while precedence < Precedence::from_token(self.peek().tag) {
            self.check_canceled()?;
            let next_tag = self.peek().tag;

            if next_tag == TokenType::DotLBrace && !expr_can_prefix_data_init_type(&left) {
                break;
            }

            let is_manifestly_void = matches!(
                &left.kind,
                ExprKind::While { .. }
                    | ExprKind::If {
                        else_branch: None,
                        ..
                    }
                    | ExprKind::Block { result: None, .. }
            );

            if is_manifestly_void {
                break;
            }

            let op_token = self.advance();
            left = self.parse_infix(left, op_token)?;
        }
        Ok(left)
    }

    fn parse_prefix(&mut self, token: Token) -> ParseResult<Expr> {
        let span = token.span;
        match token.tag {
            TokenType::IntLiteral
            | TokenType::FloatLiteral
            | TokenType::StringLiteral
            | TokenType::ByteCharLiteral
            | TokenType::CharLiteral => self.parse_literal_expr(token),
            TokenType::True => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::Bool(true),
            }),
            TokenType::False => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::Bool(false),
            }),
            TokenType::Identifier | TokenType::Void => {
                let name = self.intern_token(token);
                Ok(Expr {
                    id: self.new_id(),
                    span,
                    kind: ExprKind::Identifier(name),
                })
            }
            TokenType::DotDot => self.parse_anchored_path_expr(PathAnchor::Parent, token.span),
            TokenType::Ellipsis => self.parse_prefix_range_expr(token),
            TokenType::DotDotEqual => self.parse_prefix_range_expr(token),
            TokenType::Slash => self.parse_anchored_path_expr(PathAnchor::Package, token.span),
            TokenType::DotLBrace => self.parse_data_init(None, span),
            TokenType::Dot => self.parse_enum_literal_expr(span),
            TokenType::Minus | TokenType::Bang | TokenType::Tilde => {
                self.parse_unary_prefix_expr(token)
            }
            TokenType::LParen => self.parse_grouped_expr(span),
            TokenType::If => self.parse_if_expr(span),
            TokenType::Match => self.parse_match_expr(span),
            TokenType::LBrace => self.parse_block_expr(span),
            TokenType::For => self.parse_for_expr(span),
            TokenType::While => self.parse_while_expr(span),
            TokenType::Let | TokenType::Const | TokenType::Static => self.parse_decl_expr(token),
            TokenType::Break => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::Break,
            }),
            TokenType::Continue => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::Continue,
            }),
            TokenType::Return => self.parse_return_expr(span),
            TokenType::Undef => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::Undef,
            }),
            TokenType::SelfValue => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::SelfValue,
            }),
            TokenType::At => self.parse_intrinsic_expr(token),
            TokenType::LBracket => self.parse_lbracket_prefix_expr(token),
            TokenType::Ampersand
            | TokenType::Caret
            | TokenType::Question
            | TokenType::Fn
            | TokenType::CapitalFn
            | TokenType::Struct
            | TokenType::Union
            | TokenType::Enum
            | TokenType::Extern => self.parse_type_namespace_expr(token),
            TokenType::LexError(msg) => {
                self.add_error(span, msg.to_string());
                Ok(Expr {
                    id: self.new_id(),
                    span,
                    kind: ExprKind::Error,
                })
            }
            TokenType::Illegal => {
                let text = self.source_slice(span).to_string();
                let message = if text.is_empty() {
                    "invalid token".to_string()
                } else {
                    format!("invalid token `{text}`")
                };
                self.add_error(span, message);
                Ok(Expr {
                    id: self.new_id(),
                    span,
                    kind: ExprKind::Error,
                })
            }
            TokenType::Underscore => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::Infer,
            }),
            _ => {
                let text = self.source_slice(span).to_string();
                Ok(self.error_expr(span, format!("Expected expression, found '{}'", text)))
            }
        }
    }

    fn parse_infix(&mut self, left: Expr, token: Token) -> ParseResult<Expr> {
        match token.tag {
            TokenType::Plus
            | TokenType::Minus
            | TokenType::Star
            | TokenType::Slash
            | TokenType::EqualEqual
            | TokenType::NotEqual
            | TokenType::Percent
            | TokenType::LessThan
            | TokenType::LessEqual
            | TokenType::GreaterThan
            | TokenType::GreaterEqual
            | TokenType::And
            | TokenType::Or
            | TokenType::Pipe
            | TokenType::Ampersand
            | TokenType::Caret
            | TokenType::LShift
            | TokenType::RShift => self.parse_binary_expr(left, token),
            TokenType::Ellipsis | TokenType::DotDotEqual => {
                self.parse_infix_range_expr(left, token)
            }
            TokenType::Dot => self.parse_field_access_expr(left),
            TokenType::DotQuestion => Ok(Expr {
                id: self.new_id(),
                span: left.span.to(token.span),
                kind: ExprKind::Propagate {
                    operand: Box::new(left),
                },
            }),
            TokenType::Bang => {
                let ok_type = self.expr_to_type(left)?;
                let err_type = self.parse_type()?;
                Ok(Expr {
                    id: self.new_id(),
                    span: ok_type.span.to(err_type.span),
                    kind: ExprKind::TypeNode(Box::new(TypeNode {
                        id: self.new_id(),
                        span: ok_type.span.to(err_type.span),
                        kind: TypeKind::Result {
                            ok: Box::new(ok_type),
                            err: Box::new(err_type),
                        },
                    })),
                })
            }
            TokenType::LParen => self.parse_call_expr(left, token.span),
            TokenType::DotStar => Ok(Expr {
                id: self.new_id(),
                span: left.span.to(token.span),
                kind: ExprKind::Unary {
                    op: UnaryOperator::PointerDeRef,
                    operand: Box::new(left),
                },
            }),
            TokenType::DotAmpersand => {
                if self.check(TokenType::LBracket) {
                    self.advance();
                    self.parse_slice_expr(left, false)
                } else {
                    Ok(Expr {
                        id: self.new_id(),
                        span: left.span.to(token.span),
                        kind: ExprKind::Unary {
                            op: UnaryOperator::AddressOf,
                            operand: Box::new(left),
                        },
                    })
                }
            }
            TokenType::DotDotAmpersand => {
                if self.check(TokenType::LBracket) {
                    self.advance();
                    self.parse_slice_expr(left, true)
                } else {
                    Ok(Expr {
                        id: self.new_id(),
                        span: left.span.to(token.span),
                        kind: ExprKind::Unary {
                            op: UnaryOperator::MutAddressOf,
                            operand: Box::new(left),
                        },
                    })
                }
            }
            TokenType::Assign
            | TokenType::PlusAssign
            | TokenType::MinusAssign
            | TokenType::StarAssign
            | TokenType::SlashAssign
            | TokenType::PercentAssign
            | TokenType::AmpersandAssign
            | TokenType::PipeAssign
            | TokenType::CaretAssign
            | TokenType::LShiftAssign
            | TokenType::RShiftAssign => self.parse_assignment_expr(left, token),
            TokenType::As => self.parse_as_cast_expr(left),
            TokenType::DotLBracket => self.parse_index_expr(left),
            TokenType::LBracket => self.parse_generic_instantiation_expr(left),
            TokenType::DotLBrace => {
                let type_node = self.expr_to_type(left)?;
                let span = type_node.span;
                self.parse_data_init(Some(Box::new(type_node)), span)
            }
            _ => {
                self.add_error(
                    token.span,
                    format!("Unexpected infix token {:?}", token.tag),
                );
                Err(ParseError)
            }
        }
    }

    fn parse_binary_expr(&mut self, left: Expr, token: Token) -> ParseResult<Expr> {
        let op = binary_operator_from_token(token.tag);
        let precedence = Precedence::from_token(token.tag);
        let right = self.parse_expression(precedence)?;
        Ok(Expr {
            id: self.new_id(),
            span: left.span.to(right.span),
            kind: ExprKind::Binary {
                lhs: Box::new(left),
                op,
                rhs: Box::new(right),
            },
        })
    }

    fn token_can_end_range_expr(tag: TokenType) -> bool {
        matches!(
            tag,
            TokenType::Semicolon
                | TokenType::Comma
                | TokenType::RParen
                | TokenType::RBrace
                | TokenType::RBracket
                | TokenType::Arrow
                | TokenType::Eof
        )
    }

    fn parse_prefix_range_expr(&mut self, token: Token) -> ParseResult<Expr> {
        let is_inclusive = token.tag == TokenType::DotDotEqual;
        let end = if Self::token_can_end_range_expr(self.peek().tag) {
            if is_inclusive {
                self.add_error(
                    token.span,
                    "inclusive range expressions require an end bound".to_string(),
                );
                return Err(ParseError);
            }
            None
        } else {
            Some(Box::new(self.parse_expression(Precedence::Range)?))
        };
        let span = end
            .as_deref()
            .map_or(token.span, |end| token.span.to(end.span));
        Ok(Expr {
            id: self.new_id(),
            span,
            kind: ExprKind::Range {
                start: None,
                end,
                is_inclusive,
            },
        })
    }

    fn parse_infix_range_expr(&mut self, left: Expr, token: Token) -> ParseResult<Expr> {
        let is_inclusive = token.tag == TokenType::DotDotEqual;
        let end = if Self::token_can_end_range_expr(self.peek().tag) {
            if is_inclusive {
                self.add_error(
                    token.span,
                    "inclusive range expressions require an end bound".to_string(),
                );
                return Err(ParseError);
            }
            None
        } else {
            Some(Box::new(self.parse_expression(Precedence::Range)?))
        };
        let span = end
            .as_deref()
            .map_or(left.span.to(token.span), |end| left.span.to(end.span));
        Ok(Expr {
            id: self.new_id(),
            span,
            kind: ExprKind::Range {
                start: Some(Box::new(left)),
                end,
                is_inclusive,
            },
        })
    }

    fn parse_anchored_path_expr(
        &mut self,
        anchor: PathAnchor,
        anchor_span: kernc_utils::Span,
    ) -> ParseResult<Expr> {
        let name_token = self.expect(TokenType::Identifier)?;
        Ok(Expr {
            id: self.new_id(),
            span: anchor_span.to(name_token.span),
            kind: ExprKind::AnchoredPath {
                anchor,
                name: self.intern_token(name_token),
                name_span: name_token.span,
            },
        })
    }

    fn parse_field_access_expr(&mut self, left: Expr) -> ParseResult<Expr> {
        let at_span = if self.check(TokenType::At) {
            Some(self.advance().span)
        } else {
            None
        };

        if !self.check(TokenType::Identifier) {
            let span = self.peek().span;
            return Ok(self.error_expr(span, "Expected field name after `.`"));
        }
        let field_token = self.advance();
        let field_id = if at_span.is_some() {
            let name = format!("@{}", self.source_slice(field_token.span));
            self.session.intern(&name)
        } else {
            self.intern_token(field_token)
        };
        let field_span = at_span
            .map(|span| span.to(field_token.span))
            .unwrap_or(field_token.span);
        Ok(Expr {
            id: self.new_id(),
            span: left.span.to(field_span),
            kind: ExprKind::FieldAccess {
                lhs: Box::new(left),
                field: field_id,
                field_span,
            },
        })
    }

    fn parse_call_expr(&mut self, left: Expr, open_span: kernc_utils::Span) -> ParseResult<Expr> {
        let mut args = Vec::new();
        if !self.check(TokenType::RParen) {
            loop {
                self.check_canceled()?;
                if token_ends_unclosed_call_argument_list(self.peek().tag) {
                    break;
                }
                args.push(self.parse_expression(Precedence::Lowest)?);
                if token_ends_unclosed_call_argument_list(self.peek().tag) {
                    break;
                }
                if !self.continue_after_comma(&[TokenType::RParen]) {
                    break;
                }
            }
        }
        let end = self.recover_missing_closing_delimiter(
            TokenType::RParen,
            open_span,
            self.stream.prev_span(),
        );
        Ok(Expr {
            id: self.new_id(),
            span: left.span.to(end),
            kind: ExprKind::Call {
                callee: Box::new(left),
                args,
            },
        })
    }

    fn parse_assignment_expr(&mut self, left: Expr, token: Token) -> ParseResult<Expr> {
        let op = assignment_operator_from_token(token.tag);
        let right = self.parse_expression(Precedence::Lowest)?;
        Ok(Expr {
            id: self.new_id(),
            span: left.span.to(right.span),
            kind: ExprKind::Assign {
                lhs: Box::new(left),
                op,
                rhs: Box::new(right),
            },
        })
    }

    fn parse_as_cast_expr(&mut self, left: Expr) -> ParseResult<Expr> {
        let target = self.parse_type()?;
        Ok(Expr {
            id: self.new_id(),
            span: left.span.to(target.span),
            kind: ExprKind::As {
                lhs: Box::new(left),
                target: Box::new(target),
            },
        })
    }
}
