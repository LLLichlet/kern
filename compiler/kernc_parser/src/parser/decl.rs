use super::expr::Precedence;
use super::{ParseError, ParseResult, Parser};
use kernc_ast::*;
use kernc_lexer::TokenType;
use kernc_utils::Span;

impl<'a> Parser<'a> {
    fn parse_generic_params(&mut self) -> ParseResult<Vec<GenericParam>> {
        if self.check(TokenType::LBracket) && self.stream.peek_nth(1).tag == TokenType::RBracket {
            return Ok(Vec::new());
        }
        if !self.match_token(&[TokenType::LBracket]) {
            return Ok(Vec::new());
        }
        let mut params = Vec::new();
        while !self.check(TokenType::RBracket) && !self.check(TokenType::Eof) {
            let name = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name);
            let span = name.span;

            params.push(GenericParam {
                name: name_id,
                span,
            });

            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }
        self.expect(TokenType::RBracket)?;
        Ok(params)
    }
    fn parse_where_clauses(&mut self) -> ParseResult<Vec<WhereClause>> {
        if !self.match_token(&[TokenType::Where]) {
            return Ok(Vec::new());
        }

        let mut clauses = Vec::new();

        // Parse clauses such as `where *T: TraitA + TraitB, U: TraitC`.
        loop {
            let start_span = self.peek().span;

            // 1. Left-hand side: constrained target type, for example `*mut T`.
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
            if !self.match_token(&[TokenType::Comma]) {
                break;
            }

            // Accept a trailing comma before `{` or `;`.
            if self.check(TokenType::LBrace) || self.check(TokenType::Semicolon) {
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

            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }
        self.expect(TokenType::RParen)?;
        Ok((params, is_variadic))
    }

    // Top Level
    pub fn parse_module(&mut self) -> ParseResult<Module> {
        // Parse file-level `#![...]` attributes before any declarations.
        let attributes = self.parse_attributes(true).unwrap_or_default();

        let mut decls = Vec::new();
        while !self.check(TokenType::Eof) {
            match self.parse_decl() {
                Ok(Some(decl)) => decls.push(decl),
                Ok(None) => {} // Skipped
                Err(_) => {
                    self.synchronize();
                }
            }
        }
        Ok(Module {
            path: "test.rn".to_string(),
            attributes,
            decls,
        })
    }

    fn parse_decl(&mut self) -> ParseResult<Option<Decl>> {
        // Attributes syntactically precede every declaration.
        let attributes = self.parse_attributes(false).unwrap_or_default();

        if self.check(TokenType::Eof) {
            if !attributes.is_empty() {
                self.add_error(
                    attributes[0].span,
                    "Attributes cannot be placed at the end of the file".to_string(),
                );
            }
            return Ok(None);
        }
        let is_pub = self.match_token(&[TokenType::Pub]);
        let start_span = if is_pub {
            self.stream.prev_span()
        } else {
            self.peek().span
        };
        let is_extern = self.match_token(&[TokenType::Extern]);

        if is_extern && (self.check(TokenType::LBrace) || self.check(TokenType::StringLiteral)) {
            if is_pub {
                self.add_error(start_span, "Extern blocks cannot be pub".to_string());
            }
            let mut decl = self.parse_extern_block(start_span)?;
            decl.attributes = attributes;
            return Ok(Some(decl));
        }

        let token = self.peek();
        let decl_res = match token.tag {
            TokenType::Mod => Ok(Some(self.parse_mod_decl(start_span, is_pub)?)),
            TokenType::Fn => Ok(Some(self.parse_fn_decl(start_span, is_pub, is_extern)?)),
            TokenType::Const if self.stream.peek_nth(1).tag == TokenType::Fn => {
                Ok(Some(self.parse_fn_decl(start_span, is_pub, is_extern)?))
            }
            TokenType::Type => Ok(Some(
                self.parse_type_alias_decl(start_span, is_pub, is_extern)?,
            )),
            TokenType::Const | TokenType::Static => Ok(Some(
                self.parse_global_var_decl(start_span, is_pub, is_extern)?,
            )),
            TokenType::Use => Ok(Some(self.parse_use_decl(start_span, is_pub)?)),
            TokenType::Impl => {
                if is_pub {
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
                let txt = self
                    .session
                    .source_manager
                    .slice_source(token.span)
                    .to_string();
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
                decl.attributes = attributes;
                Ok(Some(decl))
            }
            other => other,
        }
    }

    fn parse_mod_decl(&mut self, start: Span, is_pub: bool) -> ParseResult<Decl> {
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
            is_pub,
            attributes: vec![],
            kind: DeclKind::ModDecl { is_pub },
        })
    }

    fn parse_fn_decl(&mut self, start: Span, is_pub: bool, is_extern: bool) -> ParseResult<Decl> {
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
            is_pub,
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
            let attributes = self.parse_attributes(false).unwrap_or_default();
            let is_pub = self.match_token(&[TokenType::Pub]);
            let d_start = if is_pub {
                self.stream.prev_span()
            } else {
                self.peek().span
            };

            if self.check(TokenType::Fn)
                || (self.check(TokenType::Const) && self.stream.peek_nth(1).tag == TokenType::Fn)
            {
                let mut d = self.parse_fn_decl(d_start, is_pub, true)?;
                d.attributes = attributes;
                decls.push(d);
            } else if self.check(TokenType::Static) {
                let mut d = self.parse_global_var_decl(d_start, is_pub, true)?;
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
            is_pub: false,
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
            let attributes = self.parse_attributes(false).unwrap_or_default();
            let is_pub = self.match_token(&[TokenType::Pub]);
            let d_start = if is_pub {
                self.stream.prev_span()
            } else {
                self.peek().span
            };
            if self.check(TokenType::Fn)
                || (self.check(TokenType::Const) && self.stream.peek_nth(1).tag == TokenType::Fn)
            {
                let mut d = self.parse_fn_decl(d_start, is_pub, false)?;
                d.attributes = attributes;
                decls.push(d);
            } else {
                self.error_at_current("Only fn allowed in impl".to_string());
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
            is_pub: false,
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

    fn parse_global_var_decl(
        &mut self,
        start: Span,
        is_pub: bool,
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

        // Left-side global type annotations are intentionally rejected.
        if self.match_token(&[TokenType::Colon]) {
            let err_span = self.stream.prev_span();
            self.add_error(err_span, "Global variables no longer support left-side type annotations. Use explicit constructors: `static X = Type.{ value };`".to_string());
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
            is_pub,
            attributes: vec![],
            kind: DeclKind::Var {
                value,
                is_static,
                is_extern,
                is_mut,
            },
        })
    }

    fn parse_type_alias_decl(
        &mut self,
        start: Span,
        is_pub: bool,
        is_extern: bool,
    ) -> ParseResult<Decl> {
        self.advance(); // Consume `type`.
        let name = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name);

        // 1. Parse generic parameters such as `[T]`.
        let generics = self.parse_generic_params()?;

        // 2. Parse optional bounds or an explicit backing type after `:`.
        let mut bounds = Vec::new();
        if self.match_token(&[TokenType::Colon]) {
            loop {
                bounds.push(self.parse_type()?);
                if !self.match_token(&[TokenType::Plus]) {
                    break;
                }
            }
        }

        // 3. Parse an optional `where` clause.
        let where_clauses = self.parse_where_clauses()?;

        // 4. Parse the aliased target type after `=`.
        self.expect(TokenType::Assign)?;
        let target = self.parse_type()?;
        match &target.kind {
            TypeKind::Struct {
                is_extern: true, ..
            }
            | TypeKind::Union {
                is_extern: true, ..
            } => {
                let kind_name = match &target.kind {
                    TypeKind::Struct { .. } => "struct",
                    TypeKind::Union { .. } => "union",
                    _ => unreachable!(),
                };
                let name = self.session.resolve(name_id).to_string();
                let message = if is_extern {
                    format!(
                        "named {} declarations must place `extern` before `type`, not on the right-hand side",
                        kind_name
                    )
                } else {
                    format!(
                        "named {} declarations must use `extern type Name = {} {{ ... }}`",
                        kind_name, kind_name
                    )
                };
                self.session
                    .struct_error(target.span, message)
                    .with_hint(format!(
                        "write `extern type {} = {} {{ ... }};` for named C-ABI declarations",
                        name, kind_name
                    ))
                    .emit();
                return Err(ParseError);
            }
            _ => {}
        }
        self.expect(TokenType::Semicolon)?;

        let end = self.stream.prev_span();

        Ok(Decl {
            id: self.new_id(),
            span: start.to(end),
            name_span: name.span,
            name: name_id,
            is_pub,
            attributes: vec![],
            kind: DeclKind::TypeAlias {
                generics,
                bounds,
                where_clauses,
                target,
                is_extern,
            },
        })
    }

    fn parse_use_decl(&mut self, start: Span, is_pub: bool) -> ParseResult<Decl> {
        self.advance(); // Consume `use`.

        // 1. Parse the import root marker, if any.
        let mut kind = UsePathKind::Root;

        if self.match_token(&[TokenType::Dot]) {
            kind = UsePathKind::Current;
        } else if self.match_token(&[TokenType::DotDot]) {
            kind = UsePathKind::Parent;
        }

        let mut path = Vec::new();
        let target: UseTarget;

        // 2. Consume path segments until the target form is known.
        loop {
            if self.match_token(&[TokenType::LBrace]) {
                target = self.parse_use_members()?;
                break;
            }
            if self.match_token(&[TokenType::DotLBrace]) {
                target = self.parse_use_members()?;
                break;
            }

            let id = self.expect(TokenType::Identifier)?;
            path.push(self.intern_token(id));

            if self.match_token(&[TokenType::DotLBrace]) {
                target = self.parse_use_members()?;
                break;
            } else if self.match_token(&[TokenType::Dot]) {
                continue;
            } else {
                let mut alias = None;
                if self.match_token(&[TokenType::As]) {
                    let a = self.expect(TokenType::Identifier)?;
                    alias = Some(self.intern_token(a));
                }
                target = UseTarget::Module(alias);
                break;
            }
        }

        self.expect(TokenType::Semicolon)?;

        let name = if let Some(&last) = path.last() {
            last
        } else {
            self.session.intern("root")
        };

        Ok(Decl {
            id: self.new_id(),
            span: start.to(self.stream.prev_span()),
            name_span: path
                .last()
                .map(|_| self.stream.prev_span())
                .unwrap_or(start),
            name,
            is_pub,
            attributes: vec![],
            kind: DeclKind::Use {
                kind,
                path,
                target,
                is_reexport: is_pub,
            },
        })
    }

    // Helper for brace member imports such as `{ Point, env.Args, new_point as np }`.
    fn parse_use_members(&mut self) -> ParseResult<UseTarget> {
        let mut members = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let start_span = self.peek().span;
            let mut member_path = Vec::new();

            // 1. Parse a dotted member path such as `env.Args`.
            loop {
                let m_tok = self.expect(TokenType::Identifier)?;
                member_path.push(self.intern_token(m_tok));

                if self.match_token(&[TokenType::Dot]) {
                    continue;
                } else {
                    break;
                }
            }

            // 2. Parse an optional alias.
            let mut alias = None;
            if self.match_token(&[TokenType::As]) {
                let a_tok = self.expect(TokenType::Identifier)?;
                alias = Some(self.intern_token(a_tok));
            }

            let end_span = self.stream.prev_span();

            // 3. Record the parsed member.
            members.push(UseMember {
                path: member_path,
                alias,
                span: start_span.to(end_span),
            });

            // 4. Consume the member separator.
            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }
        self.expect(TokenType::RBrace)?;
        Ok(UseTarget::Members(members))
    }

    /// Convert a parsed path expression into a type node.
    /// This is used for the left-hand side of constructs such as `Type.{...}`.
    pub fn expr_to_type(&mut self, expr: Expr) -> ParseResult<TypeNode> {
        match expr.kind {
            ExprKind::Identifier(id) => Ok(TypeNode {
                id: self.new_id(),
                span: expr.span,
                kind: TypeKind::Path {
                    segments: vec![id],
                    segment_spans: vec![expr.span],
                    generics: Vec::new(),
                },
            }),
            ExprKind::FieldAccess {
                lhs,
                field,
                field_span,
            } => {
                let mut base = self.expr_to_type(*lhs)?;
                if let TypeKind::Path {
                    ref mut segments,
                    ref mut segment_spans,
                    ..
                } = base.kind
                {
                    segments.push(field);
                    segment_spans.push(field_span);
                    base.span = base.span.to(expr.span);
                    Ok(base)
                } else {
                    self.add_error(expr.span, "Invalid path used as type".to_string());
                    Err(ParseError)
                }
            }
            ExprKind::GenericInstantiation { target, types } => {
                let mut base = self.expr_to_type(*target)?;
                if let TypeKind::Path {
                    ref mut generics, ..
                } = base.kind
                {
                    *generics = types;
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
