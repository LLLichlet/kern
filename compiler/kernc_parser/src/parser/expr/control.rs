use super::super::{ParseResult, Parser};
use super::Precedence;
use kernc_ast::*;
use kernc_lexer::{Token, TokenType};
use kernc_utils::Span;

#[derive(Clone, Copy)]
enum PatternLead {
    Ignore,
    Destructure,
    Variant,
    Typed,
    Binding,
}

impl<'a> Parser<'a> {
    pub fn parse_block_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        let mut stmts = Vec::new();
        let mut result = None;

        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let before = self.peek().span;
            let attributes = self.parse_attributes(false).unwrap_or_default();
            if self.check(TokenType::Defer) {
                if self.parse_defer_stmt(&mut stmts, attributes).is_err() {
                    self.synchronize();
                }
                if self.peek().span == before && !self.check(TokenType::Eof) {
                    self.advance();
                }
                continue;
            }
            if self.check(TokenType::Use) {
                if self.parse_use_stmt(&mut stmts, attributes).is_err() {
                    self.synchronize();
                }
                if self.peek().span == before && !self.check(TokenType::Eof) {
                    self.advance();
                }
                continue;
            }

            let expr = match self.parse_expression(Precedence::Lowest) {
                Ok(e) => e,
                Err(_) => {
                    self.synchronize();
                    if self.peek().span == before && !self.check(TokenType::Eof) {
                        self.advance();
                    }
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
                let span = self.peek().span;
                self.report_expected_semicolon(span);
                self.push_expr_stmt(&mut stmts, attributes, expr);
            }
            if self.peek().span == before && !self.check(TokenType::Eof) {
                self.advance();
            }
        }

        let end_span = self.recover_missing_closing_delimiter(
            TokenType::RBrace,
            start_span,
            self.stream.prev_span(),
        );
        Ok(Expr {
            id: self.new_id(),
            span: start_span.to(end_span),
            kind: ExprKind::Block { stmts, result },
        })
    }

    fn parse_use_stmt(
        &mut self,
        stmts: &mut Vec<Stmt>,
        attributes: Vec<Attribute>,
    ) -> ParseResult<()> {
        let start = self.peek().span;
        let (kind, path, target, binding_span) = self.parse_use_clause(start)?;
        if !self.match_token(&[TokenType::Semicolon]) {
            let span = self.peek().span;
            self.report_expected_semicolon(span);
        }
        let end = self.stream.prev_span();
        stmts.push(Stmt {
            id: self.new_id(),
            span: start.to(end),
            attributes,
            kind: StmtKind::Use(UseStmt {
                kind,
                path,
                target,
                binding_span,
            }),
        });
        Ok(())
    }

    fn parse_defer_stmt(
        &mut self,
        stmts: &mut Vec<Stmt>,
        attributes: Vec<Attribute>,
    ) -> ParseResult<()> {
        let defer_t = self.advance();
        let expr = self.parse_expression(Precedence::Lowest)?;
        if !self.match_token(&[TokenType::Semicolon]) {
            let span = self.peek().span;
            self.report_expected_semicolon(span);
        }
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
        let pattern = self.parse_let_pattern()?;
        self.expect(TokenType::Colon)?;
        let iter = self.parse_expression(Precedence::Lowest)?;
        self.expect(TokenType::RParen)?;

        let body = self.parse_expression(Precedence::Lowest)?;
        Ok(self.desugar_for_expr(start_span, pattern, iter, body))
    }

    pub(super) fn parse_while_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        self.expect(TokenType::LParen)?;
        let cond = self.parse_expression(Precedence::Lowest)?;
        self.expect(TokenType::RParen)?;
        let body = self.parse_expression(Precedence::Lowest)?;

        Ok(Expr {
            id: self.new_id(),
            span: start_span.to(body.span),
            kind: ExprKind::While {
                cond: Box::new(cond),
                body: Box::new(body),
            },
        })
    }

    fn desugar_for_expr(
        &mut self,
        start_span: Span,
        pattern: LetPattern,
        iter: Expr,
        body: Expr,
    ) -> Expr {
        let block_id = self.new_id();
        let iter_sym = self
            .session
            .intern(&format!("\0kern_for_iter_{}", block_id.0));
        let some_sym = self.session.intern("Some");
        let next_sym = self.session.intern("next");
        let iter_span = iter.span;
        let body_span = body.span;

        let iter_let = Expr {
            id: self.new_id(),
            span: iter_span,
            kind: ExprKind::Let {
                pattern: LetPattern {
                    span: iter_span,
                    pattern: Pattern {
                        span: iter_span,
                        kind: PatternKind::Binding(BindingPattern {
                            name: iter_sym,
                            name_span: iter_span,
                            is_mut: true,
                            span: iter_span,
                        }),
                    },
                },
                type_node: None,
                init: Box::new(iter),
                else_clause: None,
            },
        };

        let iter_ident = Expr {
            id: self.new_id(),
            span: iter_span,
            kind: ExprKind::Identifier(iter_sym),
        };
        let iter_ref = Expr {
            id: self.new_id(),
            span: iter_span,
            kind: ExprKind::Unary {
                op: UnaryOperator::MutAddressOf,
                operand: Box::new(iter_ident),
            },
        };
        let next_member = Expr {
            id: self.new_id(),
            span: iter_span,
            kind: ExprKind::FieldAccess {
                lhs: Box::new(iter_ref),
                field: next_sym,
                field_span: iter_span,
            },
        };
        let next_call = Expr {
            id: self.new_id(),
            span: iter_span,
            kind: ExprKind::Call {
                callee: Box::new(next_member),
                args: Vec::new(),
            },
        };

        let item_pattern = Pattern {
            span: pattern.span,
            kind: PatternKind::Destructure(DestructurePattern {
                target_type: None,
                fields: vec![DestructurePatternField {
                    name: some_sym,
                    name_span: pattern.span,
                    pattern: Box::new(pattern.pattern),
                    span: pattern.span,
                }],
            }),
        };
        let next_let = Expr {
            id: self.new_id(),
            span: start_span.to(iter_span),
            kind: ExprKind::Let {
                pattern: LetPattern {
                    span: item_pattern.span,
                    pattern: item_pattern,
                },
                type_node: None,
                init: Box::new(next_call),
                else_clause: Some(LetElseClause::Expr(Box::new(Expr {
                    id: self.new_id(),
                    span: start_span,
                    kind: ExprKind::Break,
                }))),
            },
        };

        let loop_body = Expr {
            id: self.new_id(),
            span: iter_span.to(body_span),
            kind: ExprKind::Block {
                stmts: vec![
                    Stmt {
                        id: self.new_id(),
                        span: next_let.span,
                        attributes: Vec::new(),
                        kind: StmtKind::ExprStmt(next_let),
                    },
                    Stmt {
                        id: self.new_id(),
                        span: body.span,
                        attributes: Vec::new(),
                        kind: StmtKind::ExprStmt(body),
                    },
                ],
                result: None,
            },
        };
        let while_expr = Expr {
            id: self.new_id(),
            span: start_span.to(loop_body.span),
            kind: ExprKind::While {
                cond: Box::new(Expr {
                    id: self.new_id(),
                    span: start_span,
                    kind: ExprKind::Bool(true),
                }),
                body: Box::new(loop_body),
            },
        };

        Expr {
            id: block_id,
            span: start_span.to(while_expr.span),
            kind: ExprKind::Block {
                stmts: vec![
                    Stmt {
                        id: self.new_id(),
                        span: iter_let.span,
                        attributes: Vec::new(),
                        kind: StmtKind::ExprStmt(iter_let),
                    },
                    Stmt {
                        id: self.new_id(),
                        span: while_expr.span,
                        attributes: Vec::new(),
                        kind: StmtKind::ExprStmt(while_expr),
                    },
                ],
                result: None,
            },
        }
    }

    fn parse_match_body(&mut self) -> ParseResult<Expr> {
        if self.check(TokenType::LBrace) {
            let t = self.advance();
            self.parse_block_expr(t.span)
        } else {
            let expr = self.parse_expression(Precedence::Lowest)?;
            self.match_token(&[TokenType::Semicolon]);
            Ok(expr)
        }
    }

    pub(super) fn parse_match_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        self.expect(TokenType::LParen)?;
        let target = self.parse_expression(Precedence::Lowest)?;
        self.expect(TokenType::RParen)?;
        self.expect(TokenType::LBrace)?;

        let mut arms = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let before = self.peek().span;
            match self.parse_match_arm() {
                Ok(arm) => arms.push(arm),
                Err(_) => {
                    self.synchronize_match_arm();
                    if self.peek().span == before && !self.check(TokenType::Eof) {
                        self.advance();
                    }
                }
            }
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

    fn synchronize_match_arm(&mut self) {
        self.panic_mode = false;
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            if self.match_token(&[TokenType::Comma]) {
                return;
            }
            self.advance();
        }
    }

    fn parse_match_arm(&mut self) -> ParseResult<MatchArm> {
        let arm_start = self.peek().span;

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

    fn parse_let_else_arm(&mut self) -> ParseResult<LetElseArm> {
        let arm_start = self.peek().span;
        let pattern = self.parse_pattern()?;
        self.expect(TokenType::Arrow)?;
        let body = self.parse_match_body()?;
        self.match_token(&[TokenType::Comma]);

        Ok(LetElseArm {
            span: arm_start.to(body.span),
            pattern,
            body,
        })
    }

    fn parse_let_else_arms(&mut self) -> ParseResult<LetElseClause> {
        self.expect(TokenType::LBrace)?;
        let mut arms = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            arms.push(self.parse_let_else_arm()?);
        }
        self.expect(TokenType::RBrace)?;
        Ok(LetElseClause::Arms(arms))
    }

    fn parse_match_patterns(&mut self) -> ParseResult<Vec<MatchPattern>> {
        let mut patterns = Vec::new();
        loop {
            let before = self.peek().span;
            patterns.push(self.parse_single_match_pattern()?);
            if !self.continue_after_comma(&[TokenType::Arrow]) {
                break;
            }
            if self.peek().span == before && !self.check(TokenType::Eof) {
                self.advance();
                break;
            }
        }
        Ok(patterns)
    }

    fn parse_single_match_pattern(&mut self) -> ParseResult<MatchPattern> {
        let pat_start = self.peek().span;

        if self.looks_like_call_shaped_payload_pattern() {
            let expr = self.parse_dotted_value_pattern_expr()?;
            let span = self.peek().span;
            self.session
                .struct_error(
                    span,
                    "enum payload patterns must use braced destructuring syntax",
                )
                .with_hint("write this as `.{ Variant: value }` or `Type.{ Variant: value }`")
                .emit();
            self.skip_balanced_group(TokenType::LParen, TokenType::RParen);
            return Ok(MatchPattern {
                span: pat_start.to(expr.span),
                kind: MatchPatternKind::Value(Box::new(expr)),
            });
        }

        if let Some(lead) = self.classify_match_pattern_lead() {
            let pattern = self.parse_pattern_from_lead(pat_start, lead)?;
            return Ok(MatchPattern {
                span: pattern.span,
                kind: MatchPatternKind::Pattern(pattern),
            });
        }

        let expr = self.parse_match_value_pattern_expr()?;
        if self.match_token(&[TokenType::Colon]) {
            let _ = self.parse_pattern()?;
            self.session
                .struct_error(
                    expr.span,
                    "enum payload patterns must use braced destructuring syntax",
                )
                .with_hint("write this as `.{ Variant: value }` or `Type.{ Variant: value }`")
                .emit();
            return Err(crate::ParseError);
        }

        Ok(MatchPattern {
            kind: MatchPatternKind::Value(Box::new(expr.clone())),
            span: expr.span,
        })
    }

    fn parse_match_value_pattern_expr(&mut self) -> ParseResult<Expr> {
        let expr = self.parse_expression(Precedence::Lowest)?;
        if self.check(TokenType::LParen) {
            let span = self.peek().span;
            self.session
                .struct_error(
                    span,
                    "enum payload patterns must use braced destructuring syntax",
                )
                .with_hint("write this as `.{ Variant: value }` or `Type.{ Variant: value }`")
                .emit();
            self.skip_balanced_group(TokenType::LParen, TokenType::RParen);
        }
        Ok(expr)
    }

    fn parse_dotted_value_pattern_expr(&mut self) -> ParseResult<Expr> {
        let first = self.expect(TokenType::Identifier)?;
        let mut expr = Expr {
            id: self.new_id(),
            span: first.span,
            kind: ExprKind::Identifier(self.intern_token(first)),
        };

        while self.match_token(&[TokenType::Dot]) {
            let field = self.expect(TokenType::Identifier)?;
            let field_id = self.intern_token(field);
            expr = Expr {
                id: self.new_id(),
                span: expr.span.to(field.span),
                kind: ExprKind::FieldAccess {
                    lhs: Box::new(expr),
                    field: field_id,
                    field_span: field.span,
                },
            };
        }

        Ok(expr)
    }

    fn classify_match_pattern_lead(&mut self) -> Option<PatternLead> {
        match self.stream.peek_tag_nth(0) {
            TokenType::Identifier if self.looks_like_typed_destructure_pattern() => {
                Some(PatternLead::Typed)
            }
            TokenType::Identifier => None,
            _ => self.classify_pattern_lead(false),
        }
    }

    fn parse_unit_variant_pattern_after_dot(
        &mut self,
        target_type: Option<Box<TypeNode>>,
    ) -> ParseResult<VariantPattern> {
        let v_tok = self.expect(TokenType::Identifier)?;
        let variant_name = self.intern_token(v_tok);

        if self.match_token(&[TokenType::Colon]) {
            let _ = self.parse_binding_pattern()?;
            self.session
                .struct_error(
                    v_tok.span,
                    "enum payload patterns must use braced destructuring syntax",
                )
                .with_hint("write this as `.{ Variant: value }` or `Type.{ Variant: value }`")
                .emit();
            return Err(crate::ParseError);
        }

        Ok(VariantPattern {
            target_type,
            variant_name,
            variant_span: v_tok.span,
        })
    }

    fn parse_braced_destructure_pattern(
        &mut self,
        target_type: Option<Box<TypeNode>>,
        start_span: Span,
    ) -> ParseResult<Pattern> {
        let mut fields = Vec::new();

        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let field_tok = self.expect(TokenType::Identifier)?;
            let field_name = self.intern_token(field_tok);
            let pattern = if self.match_token(&[TokenType::Colon]) {
                Box::new(self.parse_pattern()?)
            } else {
                Box::new(Pattern {
                    span: field_tok.span,
                    kind: PatternKind::Binding(BindingPattern {
                        name: field_name,
                        name_span: field_tok.span,
                        is_mut: false,
                        span: field_tok.span,
                    }),
                })
            };
            let field_span = field_tok.span.to(pattern.span);
            fields.push(DestructurePatternField {
                name: field_name,
                name_span: field_tok.span,
                pattern,
                span: field_span,
            });

            if !self.continue_after_comma(&[TokenType::RBrace]) {
                break;
            }
        }

        self.expect(TokenType::RBrace)?;

        let end_span = self.stream.prev_span();
        Ok(Pattern {
            span: start_span.to(end_span),
            kind: PatternKind::Destructure(DestructurePattern {
                target_type,
                fields,
            }),
        })
    }

    fn parse_typed_pattern(&mut self, start_span: Span) -> ParseResult<Pattern> {
        let target_type = self.parse_type()?;
        if self.match_token(&[TokenType::DotLBrace]) {
            self.parse_braced_destructure_pattern(Some(Box::new(target_type)), start_span)
        } else if let Some((target_type, variant)) =
            self.split_trailing_variant(target_type.clone())
        {
            Ok(Pattern {
                span: start_span.to(variant.name_span),
                kind: PatternKind::Variant(VariantPattern {
                    target_type: Some(Box::new(target_type)),
                    variant_name: variant.name,
                    variant_span: variant.name_span,
                }),
            })
        } else {
            self.expect(TokenType::Dot)?;
            let variant = self.parse_unit_variant_pattern_after_dot(Some(Box::new(target_type)))?;
            Ok(Pattern {
                span: start_span.to(self.stream.prev_span()),
                kind: PatternKind::Variant(variant),
            })
        }
    }

    fn parse_pattern(&mut self) -> ParseResult<Pattern> {
        let start_span = self.peek().span;
        let lead = self
            .classify_pattern_lead(true)
            .unwrap_or(PatternLead::Binding);
        self.parse_pattern_from_lead(start_span, lead)
    }

    fn parse_let_pattern(&mut self) -> ParseResult<LetPattern> {
        let pattern = self.parse_pattern()?;
        Ok(LetPattern {
            span: pattern.span,
            pattern,
        })
    }

    fn classify_pattern_lead(&mut self, allow_binding: bool) -> Option<PatternLead> {
        match self.stream.peek_tag_nth(0) {
            TokenType::Underscore => Some(PatternLead::Ignore),
            TokenType::DotLBrace => Some(PatternLead::Destructure),
            TokenType::Dot => Some(PatternLead::Variant),
            TokenType::Identifier => {
                if self.looks_like_typed_pattern() {
                    Some(PatternLead::Typed)
                } else if allow_binding {
                    Some(PatternLead::Binding)
                } else {
                    None
                }
            }
            TokenType::Mut => {
                if allow_binding {
                    Some(PatternLead::Binding)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn parse_pattern_from_lead(
        &mut self,
        start_span: Span,
        lead: PatternLead,
    ) -> ParseResult<Pattern> {
        match lead {
            PatternLead::Ignore => {
                self.expect(TokenType::Underscore)?;
                Ok(Pattern {
                    span: start_span,
                    kind: PatternKind::Ignore,
                })
            }
            PatternLead::Destructure => {
                self.expect(TokenType::DotLBrace)?;
                self.parse_braced_destructure_pattern(None, start_span)
            }
            PatternLead::Variant => {
                self.expect(TokenType::Dot)?;
                let variant = self.parse_unit_variant_pattern_after_dot(None)?;
                Ok(Pattern {
                    span: start_span.to(self.stream.prev_span()),
                    kind: PatternKind::Variant(variant),
                })
            }
            PatternLead::Typed => self.parse_typed_pattern(start_span),
            PatternLead::Binding => {
                let binding = self.parse_binding_pattern()?;
                Ok(Pattern {
                    span: binding.span,
                    kind: PatternKind::Binding(binding),
                })
            }
        }
    }

    fn split_trailing_variant(
        &mut self,
        type_node: TypeNode,
    ) -> Option<(TypeNode, TypePathSegment)> {
        let TypeKind::Path { anchor, segments } = type_node.kind else {
            return None;
        };
        if segments.len() < 2 {
            return None;
        }

        let mut target_segments = segments;
        let variant = target_segments.pop()?;
        if !variant.args.is_empty() {
            return None;
        }
        let target_span = target_segments
            .iter()
            .flat_map(|segment| {
                std::iter::once(segment.name_span).chain(segment.args.iter().map(|arg| match arg {
                    GenericArg::Type(ty) => ty.span,
                    GenericArg::ConstExpr(expr) => expr.span,
                    GenericArg::AssocBinding { value, .. } => value.span,
                }))
            })
            .reduce(|span, next| span.to(next))
            .unwrap_or(type_node.span);
        let target_type = TypeNode {
            id: self.new_id(),
            span: target_span,
            kind: TypeKind::Path {
                anchor,
                segments: target_segments,
            },
        };
        Some((target_type, variant))
    }

    fn lookahead_type_path_segment_end(&mut self, start: usize) -> Option<usize> {
        if self.stream.peek_tag_nth(start) != TokenType::Identifier {
            return None;
        }

        let mut index = start + 1;

        if self.stream.peek_tag_nth(index) == TokenType::LBracket {
            let mut depth = 1;
            index += 1;
            while depth > 0 {
                match self.stream.peek_tag_nth(index) {
                    TokenType::LBracket => depth += 1,
                    TokenType::RBracket => depth -= 1,
                    TokenType::Eof => return None,
                    _ => {}
                }
                index += 1;
            }
        }

        Some(index)
    }

    fn lookahead_type_path_end(&mut self, start: usize) -> Option<(usize, usize)> {
        let mut index = self.lookahead_type_path_segment_end(start)?;
        let mut segments = 1;
        while self.stream.peek_tag_nth(index) == TokenType::Dot
            && self.stream.peek_tag_nth(index + 1) == TokenType::Identifier
        {
            index = self.lookahead_type_path_segment_end(index + 1)?;
            segments += 1;
        }

        Some((index, segments))
    }

    fn lookahead_destructure_pattern_end(&mut self, start: usize) -> Option<usize> {
        let mut index = start;

        loop {
            if self.stream.peek_tag_nth(index) == TokenType::RBrace {
                return Some(index + 1);
            }

            if self.stream.peek_tag_nth(index) != TokenType::Identifier {
                return None;
            }
            index += 1;

            if self.stream.peek_tag_nth(index) == TokenType::Colon {
                index += 1;
                index = self.lookahead_pattern_end(index)?;
            }

            match self.stream.peek_tag_nth(index) {
                TokenType::Comma => index += 1,
                TokenType::RBrace => return Some(index + 1),
                _ => return None,
            }
        }
    }

    fn lookahead_pattern_end(&mut self, start: usize) -> Option<usize> {
        match self.stream.peek_tag_nth(start) {
            TokenType::Underscore => Some(start + 1),
            TokenType::DotLBrace => self.lookahead_destructure_pattern_end(start + 1),
            TokenType::Dot => {
                if self.stream.peek_tag_nth(start + 1) == TokenType::Identifier {
                    Some(start + 2)
                } else {
                    None
                }
            }
            TokenType::Identifier => {
                if let Some((index, segments)) = self.lookahead_type_path_end(start) {
                    match self.stream.peek_tag_nth(index) {
                        TokenType::Dot => {
                            if self.stream.peek_tag_nth(index + 1) == TokenType::Identifier {
                                return Some(index + 2);
                            }
                        }
                        TokenType::DotLBrace => {
                            return self.lookahead_destructure_pattern_end(index + 1);
                        }
                        _ => {}
                    }
                    if segments >= 2 {
                        return Some(index);
                    }
                }

                Some(start + 1)
            }
            TokenType::Mut => {
                if self.stream.peek_tag_nth(start + 1) == TokenType::Identifier {
                    Some(start + 2)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn has_let_else_arm_start(&mut self, start: usize) -> bool {
        let Some(end) = self.lookahead_pattern_end(start) else {
            return false;
        };
        self.stream.peek_tag_nth(end) == TokenType::Arrow
    }

    fn looks_like_let_else_arm_block(&mut self) -> bool {
        self.stream.peek_tag_nth(0) == TokenType::LBrace && self.has_let_else_arm_start(1)
    }

    fn looks_like_typed_pattern(&mut self) -> bool {
        let Some((index, segments)) = self.lookahead_type_path_end(0) else {
            return false;
        };

        match self.stream.peek_tag_nth(index) {
            TokenType::Dot => self.stream.peek_tag_nth(index + 1) == TokenType::Identifier,
            TokenType::DotLBrace => self.lookahead_destructure_pattern_end(index + 1).is_some(),
            _ => segments >= 2,
        }
    }

    fn looks_like_typed_destructure_pattern(&mut self) -> bool {
        let Some((index, _)) = self.lookahead_type_path_end(0) else {
            return false;
        };

        self.stream.peek_tag_nth(index) == TokenType::DotLBrace
            && self.lookahead_destructure_pattern_end(index + 1).is_some()
    }

    fn looks_like_call_shaped_payload_pattern(&mut self) -> bool {
        let Some((index, segments)) = self.lookahead_type_path_end(0) else {
            return false;
        };

        segments >= 2 && self.stream.peek_tag_nth(index) == TokenType::LParen
    }

    pub(super) fn parse_decl_expr(&mut self, start_token: Token) -> ParseResult<Expr> {
        let tag = start_token.tag;
        let static_pattern = if tag == TokenType::Static {
            let pattern = self.parse_binding_pattern()?;
            Some(pattern)
        } else {
            None
        };
        let let_pattern = if tag == TokenType::Static {
            None
        } else {
            Some(self.parse_let_pattern()?)
        };

        let type_node = if self.match_token(&[TokenType::Colon]) {
            Some(Box::new(self.parse_type()?))
        } else {
            None
        };

        self.expect(TokenType::Assign)?;
        let init = self.parse_expression(Precedence::Lowest)?;
        let mut span = start_token.span.to(init.span);
        let mut else_clause = None;

        if tag != TokenType::Static && self.match_token(&[TokenType::Else]) {
            let clause = if self.looks_like_let_else_arm_block() {
                self.parse_let_else_arms()?
            } else {
                LetElseClause::Expr(Box::new(self.parse_expression(Precedence::Lowest)?))
            };
            span = start_token.span.to(clause.span());
            else_clause = Some(clause);
        }

        match tag {
            TokenType::Static => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::Static {
                    pattern: static_pattern.unwrap(),
                    type_node,
                    init: Some(Box::new(init)),
                },
            }),
            TokenType::Let | TokenType::Const => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::Let {
                    pattern: let_pattern.unwrap(),
                    type_node,
                    init: Box::new(init),
                    else_clause,
                },
            }),
            _ => unreachable!(),
        }
    }
}
