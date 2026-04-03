use super::expr::Precedence;
use super::{ParseError, ParseResult, Parser};
use kernc_ast::*;
use kernc_lexer::TokenType;

impl<'a> Parser<'a> {
    pub fn parse_type(&mut self) -> ParseResult<TypeNode> {
        let start_token = self.peek();

        match start_token.tag {
            TokenType::Star => self.parse_pointer_type(),
            TokenType::Caret => self.parse_volatile_pointer_type(),
            TokenType::LBracket => self.parse_array_or_slice_type(),
            TokenType::Fn => self.parse_fn_type(),
            TokenType::CapitalFn => self.parse_closure_interface_type(),
            TokenType::Identifier => self.parse_path_type(),
            TokenType::At => self.parse_intrinsic_type(),
            TokenType::Void => {
                self.advance();
                Ok(TypeNode {
                    id: self.new_id(),
                    span: start_token.span,
                    kind: TypeKind::Void,
                })
            }
            TokenType::Bang => {
                self.advance();
                Ok(TypeNode {
                    id: self.new_id(),
                    span: start_token.span,
                    kind: TypeKind::Never,
                })
            }
            TokenType::Underscore => {
                self.advance();
                Ok(TypeNode {
                    id: self.new_id(),
                    span: start_token.span,
                    kind: TypeKind::Infer,
                })
            }
            TokenType::SelfType => {
                self.advance();
                Ok(TypeNode {
                    id: self.new_id(),
                    span: start_token.span,
                    kind: TypeKind::SelfType,
                })
            }

            TokenType::Extern => {
                let ext_span = self.advance().span; // Consume `extern`.
                if self.check(TokenType::Struct) {
                    // Parse an inline extern struct type.
                    let mut struct_ty = self.parse_struct_type(false, true)?;
                    struct_ty.span = ext_span.to(struct_ty.span);
                    Ok(struct_ty)
                } else if self.check(TokenType::Union) {
                    let mut union_ty = self.parse_struct_type(true, true)?;
                    union_ty.span = ext_span.to(union_ty.span);
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

            TokenType::Struct => self.parse_struct_type(false, false),
            TokenType::Union => self.parse_struct_type(true, false),
            TokenType::Enum => self.parse_enum_type(),
            TokenType::Trait => self.parse_trait_type(),

            _ => {
                let token = self.peek();
                let found_text = self
                    .session
                    .source_manager
                    .slice_source(token.span)
                    .to_string();
                self.add_error(
                    token.span,
                    format!("Expected type definition, found '{}'", found_text),
                );
                Err(ParseError)
            }
        }
    }

    // --- Type Parsing Sub-Routines ---

    fn parse_pointer_type(&mut self) -> ParseResult<TypeNode> {
        let start_span = self.advance().span; // Consume `*`.
        let is_mut = self.match_token(&[TokenType::Mut]);
        let elem = self.parse_type()?;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_span.to(elem.span),
            kind: TypeKind::Pointer {
                is_mut,
                elem: Box::new(elem),
            },
        })
    }

    fn parse_volatile_pointer_type(&mut self) -> ParseResult<TypeNode> {
        let start_span = self.advance().span; // Consume `^`.
        let is_mut = self.match_token(&[TokenType::Mut]);
        let elem = self.parse_type()?;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_span.to(elem.span),
            kind: TypeKind::VolatilePtr {
                is_mut,
                elem: Box::new(elem),
            },
        })
    }

    fn parse_array_or_slice_type(&mut self) -> ParseResult<TypeNode> {
        let start_span = self.advance().span; // Consume `[`.

        // Form A: slice types, `[]T` or `[]mut T`.
        if self.match_token(&[TokenType::RBracket]) {
            let is_mut = self.match_token(&[TokenType::Mut]);
            let elem = self.parse_type()?;
            Ok(TypeNode {
                id: self.new_id(),
                span: start_span.to(elem.span),
                kind: TypeKind::Slice {
                    is_mut,
                    elem: Box::new(elem),
                },
            })
        }
        // Form B: length-inferred arrays, `[_]T`.
        else if self.match_token(&[TokenType::Underscore]) {
            self.expect(TokenType::RBracket)?;
            let is_mut = self.match_token(&[TokenType::Mut]);
            let elem = self.parse_type()?;
            Ok(TypeNode {
                id: self.new_id(),
                span: start_span.to(elem.span),
                kind: TypeKind::ArrayInfer {
                    is_mut,
                    elem: Box::new(elem),
                },
            })
        }
        // Form C: fixed-length arrays, `[expr]T`.
        else {
            let len_expr = self.parse_expression(Precedence::Lowest)?;
            self.expect(TokenType::RBracket)?;
            let is_mut = self.match_token(&[TokenType::Mut]);
            let elem = self.parse_type()?;

            Ok(TypeNode {
                id: self.new_id(),
                span: start_span.to(elem.span),
                kind: TypeKind::Array {
                    is_mut,
                    elem: Box::new(elem),
                    len: Box::new(len_expr),
                },
            })
        }
    }

    fn parse_fn_type(&mut self) -> ParseResult<TypeNode> {
        let start_span = self.advance().span; // Consume `fn`.
        self.expect(TokenType::LParen)?;

        let mut params = Vec::new();
        let mut is_variadic = false;

        if !self.check(TokenType::RParen) {
            loop {
                // Variadic `...` must appear in the final parameter slot.
                if self.match_token(&[TokenType::Ellipsis]) {
                    is_variadic = true;
                    break;
                }

                params.push(self.parse_type()?);

                if !self.match_token(&[TokenType::Comma]) {
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

    fn parse_path_type(&mut self) -> ParseResult<TypeNode> {
        let start_token = self.advance(); // Consume the first identifier.
        let first_id = self.intern_token(start_token);
        let mut span = start_token.span;

        let mut segments = vec![first_id];
        let mut segment_spans = vec![start_token.span];

        while self.match_token(&[TokenType::Dot]) {
            let id_token = self.expect(TokenType::Identifier)?;
            segments.push(self.intern_token(id_token));
            segment_spans.push(id_token.span);
            span = span.to(id_token.span);
        }

        // Parse optional type arguments such as `List[T]`.
        let mut generics = Vec::new();
        if self.check(TokenType::LBracket) {
            generics = self.parse_type_args()?;
            span = span.to(self.stream.prev_span());
        }

        Ok(TypeNode {
            id: self.new_id(),
            span,
            kind: TypeKind::Path {
                segments,
                segment_spans,
                generics,
            },
        })
    }

    fn parse_type_args(&mut self) -> ParseResult<Vec<TypeNode>> {
        self.expect(TokenType::LBracket)?;
        let mut args = Vec::new();
        if !self.check(TokenType::RBracket) {
            loop {
                args.push(self.parse_type()?);
                if !self.match_token(&[TokenType::Comma]) {
                    break;
                }
                if self.check(TokenType::RBracket) {
                    break;
                }
            }
        }
        self.expect(TokenType::RBracket)?;
        Ok(args)
    }

    fn parse_struct_type(&mut self, is_union: bool, is_extern: bool) -> ParseResult<TypeNode> {
        let start_token = self.advance(); // Consume `struct` or `union`.
        self.expect(TokenType::LBrace)?;

        let mut fields = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let docs = self.parse_item_doc_block("field");
            let is_pub = self.match_token(&[TokenType::Pub]);
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
                is_pub,
                docs,
                type_node: field_type,
                default_value,
                span,
            });

            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }

        let end_token = self.expect(TokenType::RBrace)?;
        let kind = if is_union {
            TypeKind::Union { is_extern, fields }
        } else {
            TypeKind::Struct { is_extern, fields }
        };

        Ok(TypeNode {
            id: self.new_id(),
            span: start_token.span.to(end_token.span),
            kind,
        })
    }

    pub(super) fn parse_enum_type(&mut self) -> ParseResult<TypeNode> {
        let start_token = self.advance(); // Consume `enum`.

        // Parse an optional explicit backing type.
        let mut backing_type = None;
        if self.match_token(&[TokenType::Colon]) {
            backing_type = Some(Box::new(self.parse_type()?));
        }

        self.expect(TokenType::LBrace)?;
        let mut variants = Vec::new();

        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
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

            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }

        let end_token = self.expect(TokenType::RBrace)?;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_token.span.to(end_token.span),
            kind: TypeKind::Enum {
                backing_type,
                variants,
            },
        })
    }

    fn parse_trait_type(&mut self) -> ParseResult<TypeNode> {
        let start_token = self.advance();
        self.expect(TokenType::LBrace)?;

        let mut fields = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let docs = self.parse_item_doc_block("trait method");
            let name_token = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name_token);
            self.expect(TokenType::Colon)?;
            // Trait members must parse as function signatures such as `fn() i32`.
            let mut method_type = self.parse_type()?;
            if let TypeKind::Function { ref mut params, .. } = method_type.kind {
                // Traits implicitly prepend `Self` to the method parameter list.
                let implicit_self = TypeNode {
                    id: self.new_id(),
                    span: name_token.span, // Reuse the method name span for the synthetic node.
                    kind: TypeKind::SelfType,
                };
                params.insert(0, implicit_self);
            } else {
                self.add_error(
                    method_type.span,
                    "Trait members must be function signatures (e.g., `fn() void`)".to_string(),
                );
            }

            if self.check(TokenType::Assign) {
                self.error_at_current(
                    "Trait methods cannot have default implementations here.".to_string(),
                );
                self.advance();
                let _ = self.parse_expression(Precedence::Lowest)?; // Consume the rejected body.
            }

            fields.push(StructFieldDef {
                name: name_id,
                name_span: name_token.span,
                is_pub: false,
                docs,
                default_value: None,
                span: name_token.span.to(method_type.span),
                type_node: method_type,
            });

            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }
        let end_token = self.expect(TokenType::RBrace)?;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_token.span.to(end_token.span),
            kind: TypeKind::Trait { fields },
        })
    }

    fn parse_intrinsic_type(&mut self) -> ParseResult<TypeNode> {
        let at_span = self.advance().span; // Consume `@`.
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

    fn parse_closure_interface_type(&mut self) -> ParseResult<TypeNode> {
        let start_span = self.advance().span; // Consume `Fn`.
        self.expect(TokenType::LParen)?;

        let mut params = Vec::new();
        if !self.check(TokenType::RParen) {
            loop {
                params.push(self.parse_type()?);
                if !self.match_token(&[TokenType::Comma]) {
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
