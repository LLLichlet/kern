//! Type parser.
//!
//! Type grammar overlaps with expressions in array lengths, generic arguments,
//! and type namespace expressions.  Where the grammar is ambiguous, this module
//! uses bounded speculative parsing and restores the token stream, diagnostics,
//! panic mode, and node-id cursor before trying the alternate interpretation.

use super::expr::Precedence;
use super::{ParseError, ParseResult, Parser};
use kernc_ast::*;
use kernc_lexer::TokenType;

impl<'a> Parser<'a> {
    fn token_can_start_array_element_type(tag: TokenType) -> bool {
        matches!(
            tag,
            TokenType::Ampersand
                | TokenType::Caret
                | TokenType::LBracket
                | TokenType::CapitalFn
                | TokenType::LParen
                | TokenType::Identifier
                | TokenType::DotDot
                | TokenType::DotDotEqual
                | TokenType::Ellipsis
                | TokenType::Slash
                | TokenType::At
                | TokenType::Void
                | TokenType::Underscore
                | TokenType::SelfType
                | TokenType::Extern
                | TokenType::Struct
                | TokenType::Union
                | TokenType::Enum
                | TokenType::Trait
                | TokenType::Question
        )
    }

    fn token_can_end_missing_type(tag: TokenType) -> bool {
        matches!(
            tag,
            TokenType::Assign
                | TokenType::Semicolon
                | TokenType::Comma
                | TokenType::LBrace
                | TokenType::RParen
                | TokenType::RBrace
                | TokenType::RBracket
                | TokenType::Arrow
                | TokenType::Eof
        )
    }

    pub fn parse_type(&mut self) -> ParseResult<TypeNode> {
        self.check_canceled()?;
        let current = self.peek();
        if Self::token_can_end_missing_type(current.tag) {
            return Ok(self.error_type(current.span, "Expected type"));
        }

        self.parse_range_type()
    }

    fn token_can_end_range_type(tag: TokenType) -> bool {
        matches!(
            tag,
            TokenType::Assign
                | TokenType::Semicolon
                | TokenType::Comma
                | TokenType::LBrace
                | TokenType::RParen
                | TokenType::RBrace
                | TokenType::RBracket
                | TokenType::Arrow
                | TokenType::Eof
        )
    }

    fn parse_range_type(&mut self) -> ParseResult<TypeNode> {
        let mut ty = self.parse_result_type()?;
        if self.match_token(&[TokenType::Ellipsis, TokenType::DotDotEqual]) {
            let op_span = self.stream.prev_span();
            let is_inclusive = self.source_slice(op_span) == "..=";
            let end = if Self::token_can_end_range_type(self.peek().tag) {
                if is_inclusive {
                    self.add_error(
                        op_span,
                        "inclusive range types require an end bound".to_string(),
                    );
                    return Err(ParseError);
                }
                None
            } else {
                Some(Box::new(self.parse_result_type()?))
            };
            let span = end
                .as_deref()
                .map_or(ty.span.to(op_span), |end| ty.span.to(end.span));
            ty = TypeNode {
                id: self.new_id(),
                span,
                kind: TypeKind::Range {
                    start: Some(Box::new(ty)),
                    end,
                    is_inclusive,
                },
            };
        }

        Ok(ty)
    }

    fn parse_result_type(&mut self) -> ParseResult<TypeNode> {
        let mut ty = self.parse_prefixed_type()?;
        if self.match_token(&[TokenType::Bang]) {
            let err = self.parse_result_type()?;
            ty = TypeNode {
                id: self.new_id(),
                span: ty.span.to(err.span),
                kind: TypeKind::Result {
                    ok: Box::new(ty),
                    err: Box::new(err),
                },
            };
        }

        Ok(ty)
    }

    fn parse_prefixed_type(&mut self) -> ParseResult<TypeNode> {
        let current = self.peek();
        if Self::token_can_end_missing_type(current.tag) {
            return Ok(self.error_type(current.span, "Expected type"));
        }

        let start_token = self.advance();
        self.parse_type_after_consumed(start_token)
    }

    pub(super) fn parse_type_after_consumed(
        &mut self,
        start_token: kernc_lexer::Token,
    ) -> ParseResult<TypeNode> {
        if start_token.tag == TokenType::Question {
            // Optional type syntax binds tightly: `?A!E` parses as `(?A)!E`
            // because this routine runs before result-type parsing resumes.
            let inner = self.parse_prefixed_type()?;
            return Ok(TypeNode {
                id: self.new_id(),
                span: start_token.span.to(inner.span),
                kind: TypeKind::Optional {
                    inner: Box::new(inner),
                },
            });
        }

        if start_token.tag == TokenType::Ellipsis || start_token.tag == TokenType::DotDotEqual {
            let is_inclusive = start_token.tag == TokenType::DotDotEqual;
            let end = if Self::token_can_end_range_type(self.peek().tag) {
                if is_inclusive {
                    self.add_error(
                        start_token.span,
                        "inclusive range types require an end bound".to_string(),
                    );
                    return Err(ParseError);
                }
                None
            } else {
                Some(Box::new(self.parse_result_type()?))
            };
            let span = end
                .as_deref()
                .map_or(start_token.span, |end| start_token.span.to(end.span));
            return Ok(TypeNode {
                id: self.new_id(),
                span,
                kind: TypeKind::Range {
                    start: None,
                    end,
                    is_inclusive,
                },
            });
        }

        self.parse_primary_type_after_consumed(start_token)
    }

    fn parse_primary_type_after_consumed(
        &mut self,
        start_token: kernc_lexer::Token,
    ) -> ParseResult<TypeNode> {
        match start_token.tag {
            TokenType::Ampersand => self.parse_reference_type_from_consumed(start_token.span),
            TokenType::Caret => self.parse_volatile_pointer_type_from_consumed(start_token.span),
            TokenType::LBracket => self.parse_array_type_from_consumed(start_token.span),
            TokenType::CapitalFn => {
                self.parse_closure_interface_type_from_consumed(start_token.span)
            }
            TokenType::LParen => {
                let mut inner = self.parse_type()?;
                let end = self.expect(TokenType::RParen)?;
                inner.span = start_token.span.to(end.span);
                Ok(inner)
            }
            TokenType::Identifier => self.parse_path_type_from_consumed(start_token),
            TokenType::DotDot => {
                self.parse_anchored_path_type_from_consumed(PathAnchor::Parent, start_token.span)
            }
            TokenType::Slash => {
                self.parse_anchored_path_type_from_consumed(PathAnchor::Package, start_token.span)
            }
            TokenType::At => self.parse_intrinsic_type_from_consumed(start_token.span),
            TokenType::Void => Ok(TypeNode {
                id: self.new_id(),
                span: start_token.span,
                kind: TypeKind::Void,
            }),
            TokenType::Bang => Ok(TypeNode {
                id: self.new_id(),
                span: start_token.span,
                kind: TypeKind::Never,
            }),
            TokenType::Underscore => Ok(TypeNode {
                id: self.new_id(),
                span: start_token.span,
                kind: TypeKind::Infer,
            }),
            TokenType::SelfType => {
                if self.check(TokenType::Dot) {
                    // In type position, `Self` is normally a standalone type.
                    // `Self.Trait.Assoc` is the explicit projection form used
                    // by trait and impl signatures; later path segments still
                    // require ordinary identifiers, so `A.Self` stays invalid.
                    self.parse_path_type_from_consumed(start_token)
                } else {
                    Ok(TypeNode {
                        id: self.new_id(),
                        span: start_token.span,
                        kind: TypeKind::SelfType,
                    })
                }
            }
            TokenType::Extern => {
                if self.check(TokenType::Struct) {
                    let struct_token = self.advance();
                    let mut struct_ty =
                        self.parse_struct_or_union_type_from_consumed(struct_token, false, true)?;
                    struct_ty.span = start_token.span.to(struct_ty.span);
                    Ok(struct_ty)
                } else if self.check(TokenType::Union) {
                    let union_token = self.advance();
                    let mut union_ty =
                        self.parse_struct_or_union_type_from_consumed(union_token, true, true)?;
                    union_ty.span = start_token.span.to(union_ty.span);
                    Ok(union_ty)
                } else {
                    let token = self.peek();
                    self.add_error(
                        token.span,
                        "Expected `struct` or `union` after `extern` in type position".to_string(),
                    );
                    Err(ParseError)
                }
            }
            TokenType::Struct => self.parse_anonymous_struct_type_from_consumed(start_token, false),
            TokenType::Union => self.parse_anonymous_union_type_from_consumed(start_token, false),
            TokenType::Enum => self.parse_enum_type_from_consumed(start_token),
            TokenType::Trait => self.parse_trait_type_from_consumed(start_token),
            _ => {
                let found_text = self.source_slice(start_token.span).to_string();
                self.add_error(
                    start_token.span,
                    format!("Expected type definition, found '{}'", found_text),
                );
                Err(ParseError)
            }
        }
    }

    fn parse_anonymous_struct_type_from_consumed(
        &mut self,
        start_token: kernc_lexer::Token,
        is_extern: bool,
    ) -> ParseResult<TypeNode> {
        self.parse_struct_or_union_type_from_consumed(start_token, false, is_extern)
    }

    fn parse_anonymous_union_type_from_consumed(
        &mut self,
        start_token: kernc_lexer::Token,
        is_extern: bool,
    ) -> ParseResult<TypeNode> {
        self.parse_struct_or_union_type_from_consumed(start_token, true, is_extern)
    }

    // --- Type Parsing Sub-Routines ---

    fn parse_reference_type_from_consumed(
        &mut self,
        start_span: kernc_utils::Span,
    ) -> ParseResult<TypeNode> {
        let is_mut = self.match_token(&[TokenType::Mut]);
        if self.match_token(&[TokenType::LBracket]) {
            let lbracket_span = self.stream.prev_span();
            let saved_stream = self.stream.clone();
            let saved_panic_mode = self.panic_mode;
            let saved_next_node_id = self.session.next_node_id;
            let saved_diagnostic_len = self.session.diagnostics.len();

            if self.match_token(&[TokenType::Underscore])
                && self.match_token(&[TokenType::RBracket])
                && Self::token_can_start_array_element_type(self.peek().tag)
            {
                // `&[_]T` is a pointer to an inferred-length array, distinct
                // from the slice type `&[T]`.
                let elem = self.parse_prefixed_type()?;
                let array = TypeNode {
                    id: self.new_id(),
                    span: lbracket_span.to(elem.span),
                    kind: TypeKind::ArrayInfer {
                        elem: Box::new(elem),
                    },
                };
                return Ok(TypeNode {
                    id: self.new_id(),
                    span: start_span.to(array.span),
                    kind: TypeKind::Pointer {
                        is_mut,
                        elem: Box::new(array),
                    },
                });
            }

            self.stream = saved_stream.clone();
            self.panic_mode = saved_panic_mode;
            self.session.next_node_id = saved_next_node_id;
            self.session.diagnostics.truncate(saved_diagnostic_len);

            if let Ok(len) = self.parse_expression(Precedence::Lowest)
                && self.match_token(&[TokenType::RBracket])
                && Self::token_can_start_array_element_type(self.peek().tag)
            {
                // `&[N]T` is a pointer to an array.  If the expression parse
                // does not form that shape, fall back to slice parsing below.
                let elem = self.parse_prefixed_type()?;
                let array = TypeNode {
                    id: self.new_id(),
                    span: lbracket_span.to(elem.span),
                    kind: TypeKind::Array {
                        elem: Box::new(elem),
                        len: Box::new(len),
                    },
                };
                return Ok(TypeNode {
                    id: self.new_id(),
                    span: start_span.to(array.span),
                    kind: TypeKind::Pointer {
                        is_mut,
                        elem: Box::new(array),
                    },
                });
            }

            self.stream = saved_stream;
            self.panic_mode = saved_panic_mode;
            self.session.next_node_id = saved_next_node_id;
            self.session.diagnostics.truncate(saved_diagnostic_len);

            let elem = self.parse_type()?;
            let end = self.expect(TokenType::RBracket)?;
            return Ok(TypeNode {
                id: self.new_id(),
                span: start_span.to(end.span),
                kind: TypeKind::Slice {
                    is_mut,
                    elem: Box::new(elem),
                },
            });
        }
        if self.check(TokenType::Fn) {
            let fn_token = self.advance();
            return self.parse_fn_pointer_type_from_consumed(start_span, fn_token.span);
        }
        let elem = self.parse_prefixed_type()?;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_span.to(elem.span),
            kind: TypeKind::Pointer {
                is_mut,
                elem: Box::new(elem),
            },
        })
    }

    fn parse_volatile_pointer_type_from_consumed(
        &mut self,
        start_span: kernc_utils::Span,
    ) -> ParseResult<TypeNode> {
        let is_mut = self.match_token(&[TokenType::Mut]);
        let elem = self.parse_prefixed_type()?;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_span.to(elem.span),
            kind: TypeKind::VolatilePtr {
                is_mut,
                elem: Box::new(elem),
            },
        })
    }

    fn parse_array_type_from_consumed(
        &mut self,
        start_span: kernc_utils::Span,
    ) -> ParseResult<TypeNode> {
        // Form A: length-inferred arrays, `[_]T`.
        if self.match_token(&[TokenType::Underscore]) {
            self.expect(TokenType::RBracket)?;
            if self.match_token(&[TokenType::Mut]) {
                self.session
                    .struct_error(
                        self.stream.prev_span(),
                        "unexpected `mut` after array length",
                    )
                    .with_hint("write `[_]T` for an array value type and use `let mut`, `&mut`, or `..&[` when you need a mutable storage path")
                    .emit();
                return Err(ParseError);
            }
            let elem = self.parse_prefixed_type()?;
            Ok(TypeNode {
                id: self.new_id(),
                span: start_span.to(elem.span),
                kind: TypeKind::ArrayInfer {
                    elem: Box::new(elem),
                },
            })
        }
        // Form B: fixed-length arrays, `[expr]T`.
        else {
            let len_expr = self.parse_expression(Precedence::Lowest)?;
            self.expect(TokenType::RBracket)?;
            if self.match_token(&[TokenType::Mut]) {
                self.session
                    .struct_error(
                        self.stream.prev_span(),
                        "unexpected `mut` after array length",
                    )
                    .with_hint("write `[N]T` for an array value type and use `let mut`, `&mut`, or `..&[` when you need a mutable storage path")
                    .emit();
                return Err(ParseError);
            }
            let elem = self.parse_prefixed_type()?;

            Ok(TypeNode {
                id: self.new_id(),
                span: start_span.to(elem.span),
                kind: TypeKind::Array {
                    elem: Box::new(elem),
                    len: Box::new(len_expr),
                },
            })
        }
    }

    fn parse_fn_pointer_type_from_consumed(
        &mut self,
        start_span: kernc_utils::Span,
        fn_span: kernc_utils::Span,
    ) -> ParseResult<TypeNode> {
        let _ = fn_span;
        self.expect(TokenType::LParen)?;

        let mut params = Vec::new();
        let mut is_variadic = false;

        if !self.check(TokenType::RParen) {
            loop {
                self.check_canceled()?;
                // Variadic `...` must appear in the final parameter slot.
                if self.match_token(&[TokenType::Ellipsis]) {
                    is_variadic = true;
                    break;
                }

                params.push(self.parse_type()?);

                if !self.continue_after_comma(&[TokenType::RParen]) {
                    break;
                }
            }
        }
        self.expect(TokenType::RParen)?;

        let ret_type = self.parse_type()?;
        let end = ret_type.span;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_span.to(end),
            kind: TypeKind::Function {
                params,
                ret: Some(Box::new(ret_type)),
                is_variadic,
            },
        })
    }

    fn parse_path_type_from_consumed(
        &mut self,
        start_token: kernc_lexer::Token,
    ) -> ParseResult<TypeNode> {
        self.parse_path_type_from_consumed_with_anchor(None, start_token, start_token.span)
    }

    fn parse_anchored_path_type_from_consumed(
        &mut self,
        anchor: PathAnchor,
        anchor_span: kernc_utils::Span,
    ) -> ParseResult<TypeNode> {
        let name_token = self.expect(TokenType::Identifier)?;
        self.parse_path_type_from_consumed_with_anchor(Some(anchor), name_token, anchor_span)
    }

    fn parse_path_type_from_consumed_with_anchor(
        &mut self,
        anchor: Option<PathAnchor>,
        start_token: kernc_lexer::Token,
        start_span: kernc_utils::Span,
    ) -> ParseResult<TypeNode> {
        let mut span = start_span;
        let mut segments = vec![self.parse_type_path_segment_after_name(start_token)?];
        // There is always one segment because `start_token` is already a
        // consumed identifier or anchored-path identifier.
        span = span.to(segments.last().unwrap().name_span);
        if let Some(last_arg_span) =
            segments
                .last()
                .and_then(|segment| segment.args.last())
                .map(|arg| match arg {
                    GenericArg::Type(ty) => ty.span,
                    GenericArg::ConstExpr(expr) => expr.span,
                    GenericArg::AssocBinding { value, .. } => value.span,
                })
        {
            span = span.to(last_arg_span);
        }

        while self.match_token(&[TokenType::Dot]) {
            self.check_canceled()?;
            let id_token = self.expect(TokenType::Identifier)?;
            let segment = self.parse_type_path_segment_after_name(id_token)?;
            span = span.to(segment.name_span);
            if let Some(last_arg_span) = segment.args.last().map(|arg| match arg {
                GenericArg::Type(ty) => ty.span,
                GenericArg::ConstExpr(expr) => expr.span,
                GenericArg::AssocBinding { value, .. } => value.span,
            }) {
                span = span.to(last_arg_span);
            }
            segments.push(segment);
        }

        Ok(TypeNode {
            id: self.new_id(),
            span,
            kind: TypeKind::Path { anchor, segments },
        })
    }

    fn parse_type_path_segment_after_name(
        &mut self,
        name_token: kernc_lexer::Token,
    ) -> ParseResult<TypePathSegment> {
        let name = self.intern_token(name_token);
        let args = if self.check(TokenType::LBracket) {
            self.parse_type_args()?
        } else {
            Vec::new()
        };
        Ok(TypePathSegment {
            name,
            name_span: name_token.span,
            args,
        })
    }

    pub(super) fn parse_generic_arg(
        &mut self,
        allow_assoc_bindings: bool,
    ) -> ParseResult<GenericArg> {
        if allow_assoc_bindings
            && self.check(TokenType::Identifier)
            && self.stream.peek_tag_nth(1) == TokenType::Assign
        {
            let name_token = self.advance();
            let name = self.intern_token(name_token);
            self.expect(TokenType::Assign)?;
            let value = self.parse_type()?;
            return Ok(GenericArg::AssocBinding {
                name,
                name_span: name_token.span,
                value,
            });
        }

        let saved_stream = self.stream.clone();
        let saved_panic_mode = self.panic_mode;
        let saved_next_node_id = self.session.next_node_id;
        let saved_diagnostic_len = self.session.diagnostics.len();

        if let Ok(ty) = self.parse_type()
            && matches!(self.peek().tag, TokenType::Comma | TokenType::RBracket)
        {
            // Generic arguments prefer type syntax when it parses cleanly up to
            // the delimiter.  Otherwise the same token sequence may be a const
            // expression such as `N + 1`.
            return Ok(GenericArg::Type(ty));
        }

        self.stream = saved_stream;
        self.panic_mode = saved_panic_mode;
        self.session.next_node_id = saved_next_node_id;
        self.session.diagnostics.truncate(saved_diagnostic_len);

        Ok(GenericArg::ConstExpr(
            self.parse_expression(super::expr::Precedence::Lowest)?,
        ))
    }

    fn parse_type_args(&mut self) -> ParseResult<Vec<GenericArg>> {
        self.expect(TokenType::LBracket)?;
        let mut args = Vec::new();
        if !self.check(TokenType::RBracket) {
            loop {
                self.check_canceled()?;
                args.push(self.parse_generic_arg(true)?);
                if !self.continue_after_comma(&[TokenType::RBracket]) {
                    break;
                }
            }
        }
        self.expect(TokenType::RBracket)?;
        Ok(args)
    }

    fn parse_struct_or_union_type_from_consumed(
        &mut self,
        start_token: kernc_lexer::Token,
        is_union: bool,
        is_extern: bool,
    ) -> ParseResult<TypeNode> {
        let fields = self.parse_struct_fields("field")?;
        let kind = if is_union {
            TypeKind::Union { is_extern, fields }
        } else {
            TypeKind::Struct { is_extern, fields }
        };

        Ok(TypeNode {
            id: self.new_id(),
            span: start_token.span.to(self.stream.prev_span()),
            kind,
        })
    }

    pub(super) fn parse_struct_fields(
        &mut self,
        doc_target: &str,
    ) -> ParseResult<Vec<StructFieldDef>> {
        self.expect(TokenType::LBrace)?;

        let mut fields = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            self.check_canceled()?;
            let docs = self.parse_item_doc_block(doc_target);
            let (vis, _) = self.parse_visibility();
            let name_token = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name_token);
            self.expect(TokenType::Colon)?;
            let field_type = self.parse_type()?;

            let mut default_value = None;
            if self.match_token(&[TokenType::Assign]) {
                default_value = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
            }

            let span = name_token.span.to(if let Some(ref v) = default_value {
                v.span
            } else {
                field_type.span
            });

            fields.push(StructFieldDef {
                name: name_id,
                name_span: name_token.span,
                vis,
                docs,
                type_node: field_type,
                default_value,
                span,
            });

            if !self.continue_after_comma(&[TokenType::RBrace]) {
                break;
            }
        }

        self.expect(TokenType::RBrace)?;
        Ok(fields)
    }

    fn parse_enum_type_from_consumed(
        &mut self,
        start_token: kernc_lexer::Token,
    ) -> ParseResult<TypeNode> {
        // Parse an optional explicit backing type.
        let mut backing_type = None;
        if self.match_token(&[TokenType::Colon]) {
            backing_type = Some(Box::new(self.parse_type()?));
        }

        self.parse_enum_type_body_from_consumed(start_token.span, backing_type)
    }

    pub(super) fn parse_enum_type_body_from_consumed(
        &mut self,
        start_span: kernc_utils::Span,
        backing_type: Option<Box<TypeNode>>,
    ) -> ParseResult<TypeNode> {
        self.expect(TokenType::LBrace)?;
        let mut variants = Vec::new();

        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            self.check_canceled()?;
            let docs = self.parse_item_doc_block("variant");
            let name_token = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name_token);

            let mut payload_type = None;
            let mut value = None;

            // Form 1: payload-carrying variants, `Variant: Type`.
            if self.match_token(&[TokenType::Colon]) {
                payload_type = Some(Box::new(self.parse_type()?));
            }
            // Form 2: explicitly assigned discriminants, `Variant = Expr`.
            else if self.match_token(&[TokenType::Assign]) {
                value = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
            }

            let mut span = name_token.span;
            if let Some(ref p) = payload_type {
                span = span.to(p.span);
            }
            if let Some(ref v) = value {
                span = span.to(v.span);
            }

            variants.push(EnumVariant {
                name: name_id,
                name_span: name_token.span,
                docs,
                payload_type,
                value,
                span,
            });

            if !self.continue_after_comma(&[TokenType::RBrace]) {
                break;
            }
        }

        let end_token = self.expect(TokenType::RBrace)?;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_span.to(end_token.span),
            kind: TypeKind::Enum {
                backing_type,
                variants,
            },
        })
    }

    fn parse_trait_type_from_consumed(
        &mut self,
        start_token: kernc_lexer::Token,
    ) -> ParseResult<TypeNode> {
        self.parse_trait_type_body_from_consumed(start_token.span)
    }

    pub(super) fn parse_trait_type_body_from_consumed(
        &mut self,
        start_span: kernc_utils::Span,
    ) -> ParseResult<TypeNode> {
        self.expect(TokenType::LBrace)?;

        let mut assoc_types = Vec::new();
        let mut methods = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            self.check_canceled()?;
            let docs = self.parse_item_doc_block("trait item");
            if self.match_token(&[TokenType::Type]) {
                let name_token = self.expect(TokenType::Identifier)?;
                let generics = self.parse_generic_params()?;
                let mut bounds = Vec::new();
                if self.match_token(&[TokenType::Colon]) {
                    loop {
                        self.check_canceled()?;
                        bounds.push(self.parse_type()?);
                        if !self.match_token(&[TokenType::Plus]) {
                            break;
                        }
                    }
                }
                let where_clauses = self.parse_where_clauses()?;
                let end_token = self.expect(TokenType::Semicolon)?;
                assoc_types.push(AssociatedTypeDecl {
                    name: self.intern_token(name_token),
                    name_span: name_token.span,
                    docs,
                    generics,
                    bounds,
                    where_clauses,
                    span: name_token.span.to(end_token.span),
                });
            } else if self.check(TokenType::Fn) {
                self.advance();
                let name_token = self.expect(TokenType::Identifier)?;
                let name_id = self.intern_token(name_token);
                self.expect(TokenType::LParen)?;
                let self_type = TypeNode {
                    id: self.new_id(),
                    span: name_token.span,
                    kind: TypeKind::SelfType,
                };
                let mut params = vec![self_type];
                let mut func_params = Vec::new();
                let mut is_variadic = false;
                if !self.check(TokenType::RParen) {
                    loop {
                        self.check_canceled()?;
                        if self.match_token(&[TokenType::Ellipsis]) {
                            is_variadic = true;
                            break;
                        }
                        let pattern = self.parse_binding_pattern()?;
                        self.expect(TokenType::Colon)?;
                        let type_node = self.parse_type()?;
                        let span = pattern.span.to(type_node.span);
                        params.push(type_node.clone());
                        func_params.push(FuncParam {
                            pattern,
                            type_node,
                            span,
                        });
                        if !self.continue_after_comma(&[TokenType::RParen]) {
                            break;
                        }
                    }
                }
                self.expect(TokenType::RParen)?;
                let ret = self.parse_type()?;
                let method_type = TypeNode {
                    id: self.new_id(),
                    span: name_token.span.to(ret.span),
                    kind: TypeKind::Function {
                        params,
                        ret: Some(Box::new(ret)),
                        is_variadic,
                    },
                };

                let signature = StructFieldDef {
                    name: name_id,
                    name_span: name_token.span,
                    vis: Visibility::Private,
                    docs,
                    default_value: None,
                    span: name_token.span.to(method_type.span),
                    type_node: method_type,
                };

                let body = if self.check(TokenType::LBrace) {
                    let brace = self.expect(TokenType::LBrace)?;
                    Some(Box::new(self.parse_block_expr(brace.span)?))
                } else {
                    self.expect(TokenType::Semicolon)?;
                    None
                };
                let span = body
                    .as_ref()
                    .map(|body| name_token.span.to(body.span))
                    .unwrap_or_else(|| name_token.span.to(signature.span));

                methods.push(TraitMethodDef {
                    signature,
                    params: func_params,
                    body,
                    span,
                });
            } else {
                self.error_at_current("Expected trait item".to_string());
                self.synchronize();
            }
        }
        let end_token = self.expect(TokenType::RBrace)?;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_span.to(end_token.span),
            kind: TypeKind::Trait {
                assoc_types,
                methods,
            },
        })
    }

    fn parse_intrinsic_type_from_consumed(
        &mut self,
        at_span: kernc_utils::Span,
    ) -> ParseResult<TypeNode> {
        let id_token = self.expect(TokenType::Identifier)?;
        let sym = self.intern_token(id_token);
        let name = self.session.resolve(sym);

        if name == "typeOf" {
            self.expect(TokenType::LParen)?;
            // `@typeOf(...)` wraps a full expression.
            let expr = self.parse_expression(Precedence::Lowest)?;
            let end_token = self.expect(TokenType::RParen)?;

            Ok(TypeNode {
                id: self.new_id(),
                span: at_span.to(end_token.span),
                kind: TypeKind::TypeOf(Box::new(expr)),
            })
        } else {
            self.add_error(
                id_token.span,
                format!("Unknown compile-time type intrinsic: @{}", name),
            );
            Err(ParseError)
        }
    }

    fn parse_closure_interface_type_from_consumed(
        &mut self,
        start_span: kernc_utils::Span,
    ) -> ParseResult<TypeNode> {
        self.expect(TokenType::LParen)?;

        let mut params = Vec::new();
        if !self.check(TokenType::RParen) {
            loop {
                self.check_canceled()?;
                params.push(self.parse_type()?);
                if !self.continue_after_comma(&[TokenType::RParen]) {
                    break;
                }
            }
        }
        self.expect(TokenType::RParen)?;

        let ret_type = self.parse_type()?;
        let end_span = ret_type.span;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_span.to(end_span),
            kind: TypeKind::ClosureInterface {
                params,
                ret: Some(Box::new(ret_type)),
            },
        })
    }
}
