use super::expr::Precedence;
use super::{ParseError, ParseResult, Parser};
use kernc_ast::*;
use kernc_lexer::TokenType;
use kernc_utils::{Span, SymbolId};

impl<'a> Parser<'a> {
    pub(super) fn parse_use_clause(
        &mut self,
        start: Span,
    ) -> ParseResult<(UsePathKind, Vec<SymbolId>, UseTarget, Span)> {
        self.advance(); // Consume `use`.

        // 1. Parse the import root marker, if any.
        let mut kind = UsePathKind::External;

        if self.match_token(&[TokenType::DotLBrace]) {
            let target = UseTarget::Tree(self.parse_use_tree_items()?);
            return Ok((UsePathKind::Current, Vec::new(), target, start));
        } else if self.match_token(&[TokenType::Dot]) {
            kind = UsePathKind::Current;
        } else if self.match_token(&[TokenType::DotDot]) {
            kind = UsePathKind::Parent;
        } else if self.match_token(&[TokenType::Slash]) {
            kind = UsePathKind::Package;
        }

        let mut path = Vec::new();
        let target: UseTarget;
        let mut binding_span = start;

        // 2. Consume path segments until the target form is known.
        loop {
            if self.match_token(&[TokenType::LBrace]) {
                target = UseTarget::Tree(self.parse_use_tree_items()?);
                break;
            }
            if self.match_token(&[TokenType::DotLBrace]) {
                target = UseTarget::Tree(self.parse_use_tree_items()?);
                break;
            }

            let id = self.expect(TokenType::Identifier)?;
            binding_span = id.span;
            path.push(self.intern_token(id));

            if self.match_token(&[TokenType::DotLBrace]) {
                target = UseTarget::Tree(self.parse_use_tree_items()?);
                break;
            } else if self.match_token(&[TokenType::Dot]) {
                continue;
            } else {
                let mut alias = None;
                if self.match_token(&[TokenType::As]) {
                    let a = self.expect(TokenType::Identifier)?;
                    binding_span = a.span;
                    alias = Some(self.intern_token(a));
                }
                target = UseTarget::Module(alias);
                break;
            }
        }

        Ok((kind, path, target, binding_span))
    }

    fn parse_visibility(&mut self) -> (Visibility, Span) {
        if !self.match_token(&[TokenType::Pub]) {
            let span = self.peek().span;
            return (Visibility::Private, span);
        }

        let mut span = self.stream.prev_span();
        let vis = if self.match_token(&[TokenType::DotDot]) {
            span = span.to(self.stream.prev_span());
            Visibility::Super
        } else if self.match_token(&[TokenType::Slash]) {
            span = span.to(self.stream.prev_span());
            Visibility::Package
        } else {
            Visibility::Public
        };
        (vis, span)
    }

    pub(super) fn parse_generic_params(&mut self) -> ParseResult<Vec<GenericParam>> {
        if self.check(TokenType::LBracket) && self.stream.peek_tag_nth(1) == TokenType::RBracket {
            return Ok(Vec::new());
        }
        if !self.match_token(&[TokenType::LBracket]) {
            return Ok(Vec::new());
        }
        let mut params = Vec::new();
        while !self.check(TokenType::RBracket) && !self.check(TokenType::Eof) {
            let name = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name);
            let mut span = name.span;
            let kind = if self.match_token(&[TokenType::Colon]) {
                let ty = self.parse_type()?;
                span = span.to(ty.span);
                GenericParamKind::Const { ty }
            } else {
                GenericParamKind::Type
            };

            params.push(GenericParam {
                name: name_id,
                span,
                kind,
            });

            if !self.continue_after_comma(&[TokenType::RBracket]) {
                break;
            }
        }
        self.expect(TokenType::RBracket)?;
        Ok(params)
    }
    pub(super) fn parse_where_clauses(&mut self) -> ParseResult<Vec<WhereClause>> {
        if !self.match_token(&[TokenType::Where]) {
            return Ok(Vec::new());
        }

        let mut clauses = Vec::new();

        // Parse clauses such as `where &T: TraitA + TraitB, U: TraitC`.
        loop {
            let start_span = self.peek().span;

            // 1. Left-hand side: constrained target type, for example `&mut T`.
            let target_ty = self.parse_type()?;

            // 2. Constraint separator.
            self.expect(TokenType::Colon)?;

            // 3. Right-hand side: one or more trait bounds.
            let mut bounds = Vec::new();
            loop {
                bounds.push(self.parse_type()?);
                if !self.match_token(&[TokenType::Plus]) {
                    break;
                }
            }

            let end_span = self.stream.prev_span();
            clauses.push(WhereClause {
                span: start_span.to(end_span),
                target_ty,
                bounds,
            });

            // Without a comma, the where-clause list is complete.
            if !self.continue_after_comma(&[TokenType::LBrace, TokenType::Semicolon]) {
                break;
            }
        }

        Ok(clauses)
    }

    pub fn parse_func_params(&mut self) -> ParseResult<(Vec<FuncParam>, bool)> {
        self.expect(TokenType::LParen)?;
        let mut params = Vec::new();
        let mut is_variadic = false;

        while !self.check(TokenType::RParen) && !self.check(TokenType::Eof) {
            if self.match_token(&[TokenType::Ellipsis]) {
                is_variadic = true;
                break;
            }

            let pattern = self.parse_binding_pattern()?;
            self.expect(TokenType::Colon)?;
            let type_node = self.parse_type()?;
            let span = pattern.span.to(type_node.span);

            params.push(FuncParam {
                pattern,
                span,
                type_node,
            });

            if !self.continue_after_comma(&[TokenType::RParen]) {
                break;
            }
        }
        self.expect(TokenType::RParen)?;
        Ok((params, is_variadic))
    }

    // Top Level
    pub fn parse_module(&mut self) -> ParseResult<Module> {
        let (docs, attributes) = self.parse_module_leading_meta();

        let mut decls = Vec::new();
        while !self.check(TokenType::Eof) {
            let before = self.peek().span;
            match self.parse_decl() {
                Ok(Some(decl)) => decls.push(decl),
                Ok(None) => {} // Skipped
                Err(_) => {
                    self.synchronize();
                }
            }
            if self.peek().span == before && !self.check(TokenType::Eof) {
                self.advance();
            }
        }
        Ok(Module {
            path: "test.rn".to_string(),
            docs,
            attributes,
            decls,
        })
    }

    fn parse_decl(&mut self) -> ParseResult<Option<Decl>> {
        let (docs, attributes) = self.parse_item_leading_meta("item");

        if self.check(TokenType::Eof) {
            if let Some(docs) = &docs {
                self.emit_dangling_doc_error(docs, "item");
            }
            if !attributes.is_empty() {
                self.add_error(
                    attributes[0].span,
                    "Attributes cannot be placed at the end of the file".to_string(),
                );
            }
            return Ok(None);
        }
        let (vis, start_span) = self.parse_visibility();
        let is_extern = self.match_token(&[TokenType::Extern]);

        if is_extern && (self.check(TokenType::LBrace) || self.check(TokenType::StringLiteral)) {
            if vis != Visibility::Private {
                self.add_error(start_span, "Extern blocks cannot be pub".to_string());
            }
            let mut decl = self.parse_extern_block(start_span)?;
            decl.docs = docs;
            decl.attributes = attributes;
            return Ok(Some(decl));
        }

        let token = self.peek();
        let decl_res = match token.tag {
            TokenType::Mod => Ok(Some(self.parse_mod_decl(start_span, vis)?)),
            TokenType::Fn => Ok(Some(self.parse_fn_decl(start_span, vis, is_extern)?)),
            TokenType::Const if self.stream.peek_tag_nth(1) == TokenType::Fn => {
                Ok(Some(self.parse_fn_decl(start_span, vis, is_extern)?))
            }
            TokenType::Type => Ok(Some(self.parse_type_alias_decl(start_span, vis)?)),
            TokenType::Struct => Ok(Some(self.parse_struct_decl(start_span, vis, is_extern)?)),
            TokenType::Union => Ok(Some(self.parse_union_decl(start_span, vis, is_extern)?)),
            TokenType::Enum => Ok(Some(self.parse_enum_decl(start_span, vis, is_extern)?)),
            TokenType::Trait => Ok(Some(self.parse_trait_decl(start_span, vis, is_extern)?)),
            TokenType::Const | TokenType::Static => Ok(Some(
                self.parse_global_var_decl(start_span, vis, is_extern)?,
            )),
            TokenType::Use => Ok(Some(self.parse_use_decl(start_span, vis)?)),
            TokenType::Impl => {
                if vis != Visibility::Private {
                    self.add_error(start_span, "impl blocks cannot be pub".to_string());
                }
                Ok(Some(self.parse_impl_decl(start_span)?))
            }
            TokenType::Semicolon => {
                self.advance();
                Ok(None)
            }
            TokenType::Eof => Ok(None),
            _ => {
                let txt = self.source_slice(token.span).to_string();
                self.add_error(token.span, format!("Expected declaration, found '{}'", txt));
                Err(ParseError)
            }
        };
        match decl_res {
            Ok(Some(mut decl)) => {
                if is_extern {
                    match &decl.kind {
                        DeclKind::Function { body: None, .. } => {
                            self.session
                                .struct_error(
                                    decl.span,
                                    "external imports must be declared inside `extern { ... }` blocks",
                                )
                                .with_hint(
                                    "use top-level `extern fn name(...) { ... }` only for exported ABI definitions",
                                )
                                .emit();
                            return Err(ParseError);
                        }
                        DeclKind::Var { .. } => {
                            self.session
                                .struct_error(
                                    decl.span,
                                    "external statics must be declared inside `extern { ... }` blocks",
                                )
                                .with_hint(
                                    "wrap imported statics in `extern { ... }` and use `= Type.{undef};`",
                                )
                                .emit();
                            return Err(ParseError);
                        }
                        _ => {}
                    }
                }
                decl.docs = docs;
                decl.attributes = attributes;
                Ok(Some(decl))
            }
            other => other,
        }
    }

    fn parse_mod_decl(&mut self, start: Span, vis: Visibility) -> ParseResult<Decl> {
        self.advance(); // Consume `mod`.

        let name_token = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name_token);

        self.expect(TokenType::Semicolon)?;
        let end = self.stream.prev_span();

        Ok(Decl {
            id: self.new_id(),
            span: start.to(end),
            name_span: name_token.span,
            name: name_id,
            vis,
            docs: None,
            attributes: vec![],
            kind: DeclKind::ModDecl,
        })
    }

    fn parse_fn_decl(
        &mut self,
        start: Span,
        vis: Visibility,
        is_extern: bool,
    ) -> ParseResult<Decl> {
        let is_const = self.match_token(&[TokenType::Const]);
        self.expect(TokenType::Fn)?;
        let name = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name);

        let generics = self.parse_generic_params()?;
        let (params, is_variadic) = self.parse_func_params()?;

        if is_variadic && !is_extern {
            self.add_error(start, "Variadic args only allowed in extern".to_string());
        }

        let ret_type = self.parse_type()?;
        let where_clauses = self.parse_where_clauses()?;

        let body = if self.check(TokenType::LBrace) {
            let brace = self.expect(TokenType::LBrace)?;
            Some(Box::new(self.parse_block_expr(brace.span)?))
        } else {
            self.expect(TokenType::Semicolon)?;
            None
        };

        let end = if let Some(ref b) = body {
            b.span
        } else {
            self.stream.prev_span()
        };

        Ok(Decl {
            id: self.new_id(),
            span: start.to(end),
            name_span: name.span,
            name: name_id,
            vis,
            docs: None,
            attributes: vec![],
            kind: DeclKind::Function {
                generics,
                where_clauses,
                params,
                ret_type,
                body,
                is_const,
                is_extern,
                is_variadic,
            },
        })
    }

    fn parse_extern_block(&mut self, start: Span) -> ParseResult<Decl> {
        let mut abi = None;
        if self.check(TokenType::StringLiteral) {
            let t = self.advance();
            let sid = self.parse_string_literal(t)?;
            abi = Some(self.session.resolve(sid).to_string());
        }
        self.expect(TokenType::LBrace)?;

        let mut decls = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let (docs, attributes) = self.parse_item_leading_meta("extern item");
            if self.check(TokenType::RBrace) || self.check(TokenType::Eof) {
                if let Some(docs) = &docs {
                    self.emit_dangling_doc_error(docs, "extern item");
                }
                if !attributes.is_empty() {
                    self.add_error(
                        attributes[0].span,
                        "Attributes inside `extern` blocks must apply to a following item"
                            .to_string(),
                    );
                }
                break;
            }
            let (vis, d_start) = self.parse_visibility();

            if self.check(TokenType::Fn)
                || (self.check(TokenType::Const) && self.stream.peek_tag_nth(1) == TokenType::Fn)
            {
                let mut d = self.parse_fn_decl(d_start, vis, true)?;
                d.docs = docs;
                d.attributes = attributes;
                decls.push(d);
            } else if self.check(TokenType::Static) {
                let mut d = self.parse_global_var_decl(d_start, vis, true)?;
                d.docs = docs;
                d.attributes = attributes;
                decls.push(d);
            } else {
                self.error_at_current("Only fn and static allowed in extern".to_string());
                self.synchronize();
            }
        }
        let end = self.expect(TokenType::RBrace)?;
        let name = self.session.intern("extern_block");
        Ok(Decl {
            id: self.new_id(),
            span: start.to(end.span),
            name_span: start,
            name,
            vis: Visibility::Private,
            docs: None,
            attributes: vec![],
            kind: DeclKind::ExternBlock { abi, decls },
        })
    }

    fn parse_impl_decl(&mut self, start: Span) -> ParseResult<Decl> {
        self.advance(); // impl
        let generics = self.parse_generic_params()?;
        let target_type = self.parse_type()?;
        let mut trait_type = None;
        if self.match_token(&[TokenType::Colon]) {
            trait_type = Some(self.parse_type()?);
        }

        let where_clauses = self.parse_where_clauses()?;

        self.expect(TokenType::LBrace)?;

        let mut decls = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let (docs, attributes) = self.parse_item_leading_meta("impl item");
            if self.check(TokenType::RBrace) || self.check(TokenType::Eof) {
                if let Some(docs) = &docs {
                    self.emit_dangling_doc_error(docs, "impl item");
                }
                if !attributes.is_empty() {
                    self.add_error(
                        attributes[0].span,
                        "Attributes inside `impl` blocks must apply to a following item"
                            .to_string(),
                    );
                }
                break;
            }
            let (vis, d_start) = self.parse_visibility();
            if self.check(TokenType::Fn)
                || (self.check(TokenType::Const) && self.stream.peek_tag_nth(1) == TokenType::Fn)
            {
                let mut d = self.parse_fn_decl(d_start, vis, false)?;
                d.docs = docs;
                d.attributes = attributes;
                decls.push(d);
            } else if self.check(TokenType::Type) {
                if vis != Visibility::Private {
                    self.add_error(
                        d_start,
                        "Associated type definitions inside `impl` blocks cannot be `pub`"
                            .to_string(),
                    );
                }
                let mut d = self.parse_impl_assoc_type_decl(d_start)?;
                d.docs = docs;
                d.attributes = attributes;
                decls.push(d);
            } else {
                self.error_at_current("Only fn and type allowed in impl".to_string());
                self.synchronize();
            }
        }
        let end = self.expect(TokenType::RBrace)?;
        let name = self.session.intern("impl");
        Ok(Decl {
            id: self.new_id(),
            span: start.to(end.span),
            name_span: start,
            name,
            vis: Visibility::Private,
            docs: None,
            attributes: vec![],
            kind: DeclKind::Impl {
                generics,
                where_clauses,
                target_type,
                trait_type,
                decls,
            },
        })
    }

    fn parse_impl_assoc_type_decl(&mut self, start: Span) -> ParseResult<Decl> {
        self.advance(); // Consume `type`.
        let name = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name);
        let generics = self.parse_generic_params()?;
        let where_clauses = self.parse_where_clauses()?;

        if self.match_token(&[TokenType::Colon]) {
            let assoc_name = self.session.resolve(name_id).to_string();
            self.session
                .struct_error(
                    self.stream.prev_span(),
                    format!(
                        "associated type `{}` in an impl cannot declare trait bounds",
                        assoc_name
                    ),
                )
                .with_hint(format!(
                    "write `type {} = ConcreteType;` in the impl",
                    assoc_name
                ))
                .with_hint("declare the contract on the trait instead")
                .emit();
            self.synchronize();
            return Err(ParseError);
        }

        self.expect(TokenType::Assign)?;
        let target = self.parse_type()?;
        self.expect(TokenType::Semicolon)?;
        let end = self.stream.prev_span();

        Ok(Decl {
            id: self.new_id(),
            span: start.to(end),
            name_span: name.span,
            name: name_id,
            vis: Visibility::Private,
            docs: None,
            attributes: vec![],
            kind: DeclKind::TypeAlias {
                generics,
                where_clauses,
                target,
            },
        })
    }

    fn parse_global_var_decl(
        &mut self,
        start: Span,
        vis: Visibility,
        is_extern: bool,
    ) -> ParseResult<Decl> {
        let kw = self.advance();
        let is_static = kw.tag == TokenType::Static;

        // Only `static` items may carry a mutability marker.
        let mut is_mut = false;
        if self.match_token(&[TokenType::Mut]) {
            if !is_static {
                self.session
                    .struct_error(
                        self.stream.prev_span(),
                        "`const` variables cannot be mutable",
                    )
                    .emit();
            }
            is_mut = true;
        }

        let name = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name);

        if self.match_token(&[TokenType::Colon]) {
            let err_span = self.stream.prev_span();
            self.add_error(err_span, "Global variables must express their type through the initializer. Use `static X = Type.{ value };`.".to_string());
            let _ = self.parse_type();
        }

        // Init
        let value = if self.match_token(&[TokenType::Assign]) {
            self.parse_expression(Precedence::Lowest)?
        } else {
            // All globals, including extern imports, require an initializer form.
            self.add_error(
                start,
                "Global/extern vars must be initialized (use `= Type.{undef};` for externs)"
                    .to_string(),
            );
            return Err(ParseError);
        };
        self.expect(TokenType::Semicolon)?;
        let end = self.stream.prev_span();

        Ok(Decl {
            id: self.new_id(),
            span: start.to(end),
            name_span: name.span,
            name: name_id,
            vis,
            docs: None,
            attributes: vec![],
            kind: DeclKind::Var {
                value,
                is_static,
                is_extern,
                is_mut,
            },
        })
    }

    fn parse_type_alias_decl(&mut self, start: Span, vis: Visibility) -> ParseResult<Decl> {
        self.advance(); // Consume `type`.
        let name = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name);

        // 1. Parse generic parameters such as `[T]`.
        let generics = self.parse_generic_params()?;

        // 2. Parse an optional `where` clause.
        let where_clauses = self.parse_where_clauses()?;

        // 3. Parse the aliased target type after `=`.
        self.expect(TokenType::Assign)?;
        let target = self.parse_type()?;
        self.expect(TokenType::Semicolon)?;

        let end = self.stream.prev_span();

        Ok(Decl {
            id: self.new_id(),
            span: start.to(end),
            name_span: name.span,
            name: name_id,
            vis,
            docs: None,
            attributes: vec![],
            kind: DeclKind::TypeAlias {
                generics,
                where_clauses,
                target,
            },
        })
    }

    fn parse_struct_decl(
        &mut self,
        start: Span,
        vis: Visibility,
        is_extern: bool,
    ) -> ParseResult<Decl> {
        self.advance();
        let name = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name);
        let generics = self.parse_generic_params()?;
        let where_clauses = self.parse_where_clauses()?;
        let fields = self.parse_struct_fields("field")?;
        Ok(Decl {
            id: self.new_id(),
            span: start.to(self.stream.prev_span()),
            name_span: name.span,
            name: name_id,
            vis,
            docs: None,
            attributes: vec![],
            kind: DeclKind::Struct {
                generics,
                where_clauses,
                fields,
                is_extern,
            },
        })
    }

    fn parse_union_decl(
        &mut self,
        start: Span,
        vis: Visibility,
        is_extern: bool,
    ) -> ParseResult<Decl> {
        self.advance();
        let name = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name);
        let generics = self.parse_generic_params()?;
        let where_clauses = self.parse_where_clauses()?;
        let fields = self.parse_struct_fields("field")?;
        Ok(Decl {
            id: self.new_id(),
            span: start.to(self.stream.prev_span()),
            name_span: name.span,
            name: name_id,
            vis,
            docs: None,
            attributes: vec![],
            kind: DeclKind::Union {
                generics,
                where_clauses,
                fields,
                is_extern,
            },
        })
    }

    fn parse_enum_decl(
        &mut self,
        start: Span,
        vis: Visibility,
        is_extern: bool,
    ) -> ParseResult<Decl> {
        let enum_token = self.advance();
        if is_extern {
            self.add_error(start, "enum declarations cannot be extern".to_string());
        }
        let name = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name);
        let generics = self.parse_generic_params()?;
        let mut backing_type = None;
        if self.match_token(&[TokenType::Colon]) {
            backing_type = Some(Box::new(self.parse_type()?));
        }
        let where_clauses = self.parse_where_clauses()?;
        let enum_ty = self.parse_enum_type_body_from_consumed(enum_token.span, backing_type)?;
        let TypeKind::Enum {
            backing_type,
            variants,
        } = enum_ty.kind
        else {
            unreachable!()
        };
        Ok(Decl {
            id: self.new_id(),
            span: start.to(enum_ty.span),
            name_span: name.span,
            name: name_id,
            vis,
            docs: None,
            attributes: vec![],
            kind: DeclKind::Enum {
                generics,
                where_clauses,
                backing_type,
                variants,
            },
        })
    }

    fn parse_trait_decl(
        &mut self,
        start: Span,
        vis: Visibility,
        is_extern: bool,
    ) -> ParseResult<Decl> {
        let trait_token = self.advance();
        if is_extern {
            self.add_error(start, "trait declarations cannot be extern".to_string());
        }
        let name = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name);
        let generics = self.parse_generic_params()?;
        let mut supertraits = Vec::new();
        if self.match_token(&[TokenType::Colon]) {
            loop {
                supertraits.push(self.parse_type()?);
                if !self.match_token(&[TokenType::Plus]) {
                    break;
                }
            }
        }
        let where_clauses = self.parse_where_clauses()?;
        let trait_ty = self.parse_trait_type_body_from_consumed(trait_token.span)?;
        let TypeKind::Trait {
            assoc_types,
            methods,
        } = trait_ty.kind
        else {
            unreachable!()
        };
        Ok(Decl {
            id: self.new_id(),
            span: start.to(trait_ty.span),
            name_span: name.span,
            name: name_id,
            vis,
            docs: None,
            attributes: vec![],
            kind: DeclKind::Trait {
                generics,
                where_clauses,
                supertraits,
                assoc_types,
                methods,
            },
        })
    }

    fn parse_use_decl(&mut self, start: Span, vis: Visibility) -> ParseResult<Decl> {
        let (kind, path, target, binding_span) = self.parse_use_clause(start)?;
        self.expect(TokenType::Semicolon)?;

        let name = if let Some(&last) = path.last() {
            last
        } else {
            self.session.intern("root")
        };

        Ok(Decl {
            id: self.new_id(),
            span: start.to(self.stream.prev_span()),
            name_span: binding_span,
            name,
            vis,
            docs: None,
            attributes: vec![],
            kind: DeclKind::Use { kind, path, target },
        })
    }

    // Helper for brace member imports such as `{ ., env.Args, io.{Printable as P} }`.
    fn parse_use_tree_items(&mut self) -> ParseResult<Vec<UseTree>> {
        let mut items = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            items.push(self.parse_use_tree_item()?);
            if !self.continue_after_comma(&[TokenType::RBrace]) {
                break;
            }
        }
        self.expect(TokenType::RBrace)?;
        Ok(items)
    }

    fn parse_use_tree_item(&mut self) -> ParseResult<UseTree> {
        let start_span = self.peek().span;

        if self.match_token(&[TokenType::Dot]) {
            let mut binding_span = self.stream.prev_span();
            let mut alias = None;
            if self.match_token(&[TokenType::As]) {
                let a_tok = self.expect(TokenType::Identifier)?;
                binding_span = a_tok.span;
                alias = Some(self.intern_token(a_tok));
            }
            let end_span = self.stream.prev_span();
            return Ok(UseTree::SelfModule {
                alias,
                span: start_span.to(end_span),
                binding_span,
            });
        }

        let first_tok = self.expect(TokenType::Identifier)?;
        let mut path = vec![self.intern_token(first_tok)];
        let mut binding_span = first_tok.span;
        let mut nested = None;

        while self.match_token(&[TokenType::Dot]) {
            if self.check(TokenType::LBrace) {
                break;
            }
            let m_tok = self.expect(TokenType::Identifier)?;
            binding_span = m_tok.span;
            path.push(self.intern_token(m_tok));
        }

        if self.match_token(&[TokenType::DotLBrace]) || self.match_token(&[TokenType::LBrace]) {
            nested = Some(self.parse_use_tree_items()?);
            binding_span = self.stream.prev_span();
        }

        let mut alias = None;
        if self.match_token(&[TokenType::As]) {
            let a_tok = self.expect(TokenType::Identifier)?;
            binding_span = a_tok.span;
            alias = Some(self.intern_token(a_tok));
        }

        let end_span = self.stream.prev_span();
        Ok(UseTree::Path {
            path,
            alias,
            nested,
            span: start_span.to(end_span),
            binding_span,
        })
    }

    /// Convert a parsed path expression into a type node.
    /// This is used for the left-hand side of constructs such as `Type.{...}`.
    pub fn expr_to_type(&mut self, expr: Expr) -> ParseResult<TypeNode> {
        match expr.kind {
            ExprKind::Grouped { expr: inner } => self.expr_to_type(*inner),
            ExprKind::TypeNode(type_node) => Ok(*type_node),
            ExprKind::Identifier(id) => Ok(TypeNode {
                id: self.new_id(),
                span: expr.span,
                kind: TypeKind::Path {
                    anchor: None,
                    segments: vec![TypePathSegment {
                        name: id,
                        name_span: expr.span,
                        args: Vec::new(),
                    }],
                },
            }),
            ExprKind::AnchoredPath {
                anchor,
                name,
                name_span,
            } => Ok(TypeNode {
                id: self.new_id(),
                span: expr.span,
                kind: TypeKind::Path {
                    anchor: Some(anchor),
                    segments: vec![TypePathSegment {
                        name,
                        name_span,
                        args: Vec::new(),
                    }],
                },
            }),
            ExprKind::FieldAccess {
                lhs,
                field,
                field_span,
            } => {
                let mut base = self.expr_to_type(*lhs)?;
                if let TypeKind::Path {
                    ref mut segments, ..
                } = base.kind
                {
                    segments.push(TypePathSegment {
                        name: field,
                        name_span: field_span,
                        args: Vec::new(),
                    });
                    base.span = base.span.to(expr.span);
                    Ok(base)
                } else {
                    self.add_error(expr.span, "Invalid path used as type".to_string());
                    Err(ParseError)
                }
            }
            ExprKind::GenericInstantiation { target, args } => {
                let mut base = self.expr_to_type(*target)?;
                if let TypeKind::Path {
                    ref mut segments, ..
                } = base.kind
                {
                    let Some(last) = segments.last_mut() else {
                        self.add_error(expr.span, "Invalid generic type target".to_string());
                        return Err(ParseError);
                    };
                    last.args = args;
                    base.span = base.span.to(expr.span);
                    Ok(base)
                } else {
                    self.add_error(expr.span, "Invalid generic type target".to_string());
                    Err(ParseError)
                }
            }
            _ => {
                self.add_error(
                    expr.span,
                    "Invalid expression used as a type prefix".to_string(),
                );
                Err(ParseError)
            }
        }
    }
}
