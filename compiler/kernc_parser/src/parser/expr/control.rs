use super::super::{ParseResult, Parser};
use super::Precedence;
use kernc_ast::*;
use kernc_lexer::{Token, TokenType};
use kernc_utils::Span;

impl<'a> Parser<'a> {
    pub fn parse_block_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        let mut stmts = Vec::new();
        let mut result = None;

        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let attributes = self.parse_attributes(false).unwrap_or_default();
            if self.check(TokenType::Defer) {
                self.parse_defer_stmt(&mut stmts, attributes)?;
                continue;
            }

            let expr = match self.parse_expression(Precedence::Lowest) {
                Ok(e) => e,
                Err(_) => {
                    self.synchronize();
                    continue;
                }
            };

            if self.match_token(&[TokenType::Semicolon]) {
                self.push_expr_stmt(&mut stmts, attributes, expr);
            } else if self.check(TokenType::RBrace) {
                if !attributes.is_empty() {
                    self.add_error(
                        attributes[0].span,
                        "Attributes are not allowed on the trailing return expression of a block. Consider adding a semicolon to make it a statement.".to_string(),
                    );
                }
                result = Some(Box::new(expr));
            } else if expr.is_block_like() {
                self.push_expr_stmt(&mut stmts, attributes, expr);
            } else {
                self.error_at_current("Expected semicolon".to_string());
                self.push_expr_stmt(&mut stmts, attributes, expr);
            }
        }

        let rb = self.expect(TokenType::RBrace)?;
        Ok(Expr {
            id: self.new_id(),
            span: start_span.to(rb.span),
            kind: ExprKind::Block { stmts, result },
        })
    }

    fn parse_defer_stmt(
        &mut self,
        stmts: &mut Vec<Stmt>,
        attributes: Vec<Attribute>,
    ) -> ParseResult<()> {
        let defer_t = self.advance();
        let expr = self.parse_expression(Precedence::Lowest)?;
        self.expect(TokenType::Semicolon)?;
        let defer_expr = Expr {
            id: self.new_id(),
            span: defer_t.span.to(self.stream.prev_span()),
            kind: ExprKind::Defer {
                expr: Box::new(expr),
            },
        };
        stmts.push(Stmt {
            id: self.new_id(),
            span: defer_expr.span,
            attributes,
            kind: StmtKind::ExprStmt(defer_expr),
        });
        Ok(())
    }

    fn push_expr_stmt(&mut self, stmts: &mut Vec<Stmt>, attributes: Vec<Attribute>, expr: Expr) {
        stmts.push(Stmt {
            id: self.new_id(),
            span: expr.span,
            attributes,
            kind: StmtKind::ExprStmt(expr),
        });
    }

    pub(super) fn parse_if_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        self.expect(TokenType::LParen)?;
        let cond = self.parse_expression(Precedence::Lowest)?;
        self.expect(TokenType::RParen)?;
        let then_branch = self.parse_expression(Precedence::Lowest)?;
        let mut else_branch = None;
        if self.match_token(&[TokenType::Else]) {
            else_branch = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
        }
        let end = if let Some(ref e) = else_branch {
            e.span
        } else {
            then_branch.span
        };
        Ok(Expr {
            id: self.new_id(),
            span: start_span.to(end),
            kind: ExprKind::If {
                cond: Box::new(cond),
                then_branch: Box::new(then_branch),
                else_branch,
            },
        })
    }

    pub(super) fn parse_for_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        self.expect(TokenType::LParen)?;
        let init = self.parse_optional_for_clause(TokenType::Semicolon)?;
        self.expect(TokenType::Semicolon)?;
        let cond = self.parse_optional_for_clause(TokenType::Semicolon)?;
        self.expect(TokenType::Semicolon)?;
        let post = self.parse_optional_for_clause(TokenType::RParen)?;
        self.expect(TokenType::RParen)?;

        let body = self.parse_expression(Precedence::Lowest)?;
        Ok(Expr {
            id: self.new_id(),
            span: start_span.to(body.span),
            kind: ExprKind::For {
                init,
                cond,
                post,
                body: Box::new(body),
            },
        })
    }

    fn parse_optional_for_clause(
        &mut self,
        terminator: TokenType,
    ) -> ParseResult<Option<Box<Expr>>> {
        if self.check(terminator) {
            Ok(None)
        } else {
            Ok(Some(Box::new(self.parse_expression(Precedence::Lowest)?)))
        }
    }

    fn parse_match_body(&mut self) -> ParseResult<Expr> {
        if self.check(TokenType::LBrace) {
            let t = self.advance();
            self.parse_block_expr(t.span)
        } else {
            self.parse_expression(Precedence::Lowest)
        }
    }

    pub(super) fn parse_match_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        self.expect(TokenType::LParen)?;
        let target = self.parse_expression(Precedence::Lowest)?;
        self.expect(TokenType::RParen)?;
        self.expect(TokenType::LBrace)?;

        let mut arms = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            arms.push(self.parse_match_arm()?);
        }

        let rb = self.expect(TokenType::RBrace)?;
        Ok(Expr {
            id: self.new_id(),
            span: start_span.to(rb.span),
            kind: ExprKind::Match {
                target: Box::new(target),
                arms,
            },
        })
    }

    fn parse_match_arm(&mut self) -> ParseResult<MatchArm> {
        let arm_start = self.peek().span;

        if self.match_token(&[TokenType::Underscore]) {
            self.expect(TokenType::Arrow)?;
            let body = self.parse_match_body()?;
            self.match_token(&[TokenType::Comma]);
            return Ok(MatchArm {
                patterns: vec![MatchPattern {
                    kind: MatchPatternKind::CatchAll,
                    span: arm_start.to(self.stream.prev_span()),
                }],
                body,
                span: arm_start.to(self.stream.prev_span()),
            });
        }

        let patterns = self.parse_match_patterns()?;
        self.expect(TokenType::Arrow)?;
        let body = self.parse_match_body()?;
        self.match_token(&[TokenType::Comma]);

        Ok(MatchArm {
            patterns,
            span: arm_start.to(body.span),
            body,
        })
    }

    fn parse_match_patterns(&mut self) -> ParseResult<Vec<MatchPattern>> {
        let mut patterns = Vec::new();
        loop {
            patterns.push(self.parse_single_match_pattern()?);
            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }
        Ok(patterns)
    }

    fn parse_single_match_pattern(&mut self) -> ParseResult<MatchPattern> {
        let pat_start = self.peek().span;

        if self.match_token(&[TokenType::Dot]) {
            return self.parse_shorthand_variant_pattern(pat_start);
        }

        let expr = self.parse_expression(Precedence::Lowest)?;
        if self.match_token(&[TokenType::DotDot]) {
            let end_expr = self.parse_expression(Precedence::Lowest)?;
            return Ok(MatchPattern {
                kind: MatchPatternKind::Range {
                    start: Box::new(expr),
                    end: Box::new(end_expr),
                    inclusive: false,
                },
                span: pat_start.to(self.stream.prev_span()),
            });
        }
        if self.match_token(&[TokenType::DotDotEqual]) {
            let end_expr = self.parse_expression(Precedence::Lowest)?;
            return Ok(MatchPattern {
                kind: MatchPatternKind::Range {
                    start: Box::new(expr),
                    end: Box::new(end_expr),
                    inclusive: true,
                },
                span: pat_start.to(self.stream.prev_span()),
            });
        }
        if self.match_token(&[TokenType::Colon]) {
            let binding = Some(self.parse_binding_pattern()?);
            let ty = self.expr_to_type(expr)?;
            let (target_type, variant_name, variant_span) = self.extract_variant_from_type(ty)?;
            return Ok(MatchPattern {
                kind: MatchPatternKind::Variant(VariantPattern {
                    target_type: Some(Box::new(target_type)),
                    variant_name,
                    variant_span,
                    binding,
                }),
                span: pat_start.to(self.stream.prev_span()),
            });
        }

        Ok(MatchPattern {
            kind: MatchPatternKind::Value(Box::new(expr.clone())),
            span: expr.span,
        })
    }

    fn parse_shorthand_variant_pattern(&mut self, pat_start: Span) -> ParseResult<MatchPattern> {
        let variant = self.parse_variant_pattern_after_dot()?;

        Ok(MatchPattern {
            kind: MatchPatternKind::Variant(variant),
            span: pat_start.to(self.stream.prev_span()),
        })
    }

    fn parse_variant_pattern_after_dot(&mut self) -> ParseResult<VariantPattern> {
        let v_tok = self.expect(TokenType::Identifier)?;
        let variant_name = self.intern_token(v_tok);

        let mut binding = None;
        if self.match_token(&[TokenType::Colon]) {
            binding = Some(self.parse_binding_pattern()?);
        }

        Ok(VariantPattern {
            target_type: None,
            variant_name,
            variant_span: v_tok.span,
            binding,
        })
    }

    fn parse_typed_let_variant_pattern(&mut self, start_span: Span) -> ParseResult<LetPattern> {
        let target_type = self.parse_type()?;
        self.expect(TokenType::Dot)?;
        let variant_tok = self.expect(TokenType::Identifier)?;
        let variant_name = self.intern_token(variant_tok);

        let mut binding = None;
        if self.match_token(&[TokenType::Colon]) {
            binding = Some(self.parse_binding_pattern()?);
        }

        Ok(LetPattern {
            kind: LetPatternKind::Variant(VariantPattern {
                target_type: Some(Box::new(target_type)),
                variant_name,
                variant_span: variant_tok.span,
                binding,
            }),
            span: start_span.to(self.stream.prev_span()),
        })
    }

    fn parse_let_pattern(&mut self) -> ParseResult<LetPattern> {
        let start_span = self.peek().span;

        if self.match_token(&[TokenType::Dot]) {
            let variant = self.parse_variant_pattern_after_dot()?;
            return Ok(LetPattern {
                kind: LetPatternKind::Variant(variant),
                span: start_span.to(self.stream.prev_span()),
            });
        }

        if self.check(TokenType::Identifier) {
            let next = self.stream.peek_nth(1).tag;
            if next == TokenType::Dot || next == TokenType::LBracket {
                return self.parse_typed_let_variant_pattern(start_span);
            }
        }

        let binding = self.parse_binding_pattern()?;
        Ok(LetPattern {
            span: binding.span,
            kind: LetPatternKind::Binding(binding),
        })
    }

    pub(super) fn parse_decl_expr(&mut self, start_token: Token) -> ParseResult<Expr> {
        let tag = start_token.tag;
        let static_pattern = if tag == TokenType::Static {
            let pattern = self.parse_binding_pattern()?;

            if self.match_token(&[TokenType::Colon]) {
                let err_span = self.stream.prev_span();
                let _ = self.parse_type();

                self.session
                    .struct_error(
                        err_span,
                        "type annotations on the left side of declarations are strictly forbidden in Kern",
                    )
                    .with_hint("Kern uses explicit constructor syntax on the right side")
                    .with_hint("try rewriting this as: `let [mut] name = Type.{ ... };`")
                    .emit();
                self.panic_mode = true;
            }

            Some(pattern)
        } else {
            None
        };
        let let_pattern = if tag == TokenType::Static {
            None
        } else {
            Some(self.parse_let_pattern()?)
        };

        self.expect(TokenType::Assign)?;
        let init = self.parse_expression(Precedence::Lowest)?;
        let mut span = start_token.span.to(init.span);
        let mut else_branch = None;

        if tag != TokenType::Static && self.match_token(&[TokenType::Else]) {
            let else_expr = self.parse_expression(Precedence::Lowest)?;
            span = start_token.span.to(else_expr.span);
            else_branch = Some(Box::new(else_expr));
        }

        match tag {
            TokenType::Static => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::Static {
                    pattern: static_pattern.unwrap(),
                    init: Box::new(init),
                },
            }),
            TokenType::Let | TokenType::Const => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::Let {
                    pattern: let_pattern.unwrap(),
                    init: Box::new(init),
                    else_branch,
                },
            }),
            _ => unreachable!(),
        }
    }
}
