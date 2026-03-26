use super::expr::Precedence;
use super::{ParseResult, Parser};
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

            TokenType::Struct => self.parse_struct_type(false),
            TokenType::Union => self.parse_struct_type(true),
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
                Err(())
            }
        }
    }

    // --- Type Parsing Sub-Routines ---

    fn parse_pointer_type(&mut self) -> ParseResult<TypeNode> {
        let start_span = self.advance().span; // 消费 '*'
        let is_mut = self.match_token(&[TokenType::Mut]); // 核心：拦截 mut
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
        let start_span = self.advance().span; // 消费 '^'
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
        let start_span = self.advance().span; // 消费 '['

        // A. 切片 []T
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
        // B. 数组推导 [_]T
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
        // C. 数组 [expr]T
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
        let start_span = self.advance().span; // 消费 'fn'
        self.expect(TokenType::LParen)?;

        let mut params = Vec::new();
        let mut is_variadic = false;

        if !self.check(TokenType::RParen) {
            loop {
                // 拦截可变参数 ...
                if self.match_token(&[TokenType::Ellipsis]) {
                    is_variadic = true;
                    break; // ... 必须是最后一个参数
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
        let start_token = self.advance(); // 消费第一个 ident
        let first_id = self.intern_token(start_token);
        let mut span = start_token.span;

        let mut segments = vec![first_id];

        while self.match_token(&[TokenType::Dot]) {
            let id_token = self.expect(TokenType::Identifier)?;
            segments.push(self.intern_token(id_token));
            span = span.to(id_token.span);
        }

        // 泛型参数 List[T]
        let mut generics = Vec::new();
        if self.check(TokenType::LBracket) {
            generics = self.parse_type_args()?;
            span = span.to(self.stream.prev_span());
        }

        Ok(TypeNode {
            id: self.new_id(),
            span,
            kind: TypeKind::Path { segments, generics },
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

    fn parse_struct_type(&mut self, is_union: bool) -> ParseResult<TypeNode> {
        let start_token = self.advance(); // struct / union
        self.expect(TokenType::LBrace)?;

        let mut fields = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
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
            TypeKind::Union { fields }
        } else {
            TypeKind::Struct { fields }
        };

        Ok(TypeNode {
            id: self.new_id(),
            span: start_token.span.to(end_token.span),
            kind,
        })
    }

    fn parse_enum_type(&mut self) -> ParseResult<TypeNode> {
        let start_token = self.advance(); // 消费 'data'

        // 解析可选的底层存储类型
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

            // 1. 嗅探数据负载: `Variant: Type`
            if self.match_token(&[TokenType::Colon]) {
                payload_type = Some(Box::new(self.parse_type()?));
            }
            // 2. 嗅探显式赋值: `Variant = Expr`
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
            let name_token = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name_token);
            self.expect(TokenType::Colon)?;
            // 1. 解析后面的签名，比如 fn() i32
            let mut method_type = self.parse_type()?;
            if let TypeKind::Function { ref mut params, .. } = method_type.kind {
                // 构造一个隐式的 Self 类型节点
                let implicit_self = TypeNode {
                    id: self.new_id(),
                    span: name_token.span, // 使用方法名的位置作为 span
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
                let _ = self.parse_expression(Precedence::Lowest)?; // consume expr
            }

            fields.push(StructFieldDef {
                name: name_id,
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
        let at_span = self.advance().span; // 消费 '@'
        let id_token = self.expect(TokenType::Identifier)?;
        let sym = self.intern_token(id_token);
        let name = self.session.resolve(sym);

        if name == "typeOf" {
            self.expect(TokenType::LParen)?;
            // @typeOf 内部包含的是一个完整的表达式
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
                format!("Unknown compile-time type intrinsic: @{}", name)
            );
            Err(())
        }
    }

    fn parse_closure_interface_type(&mut self) -> ParseResult<TypeNode> {
        let start_span = self.advance().span; // 消费 'Fn' 
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
