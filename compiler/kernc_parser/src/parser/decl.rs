use super::expr::Precedence;
use super::{ParseResult, Parser};
use kernc_ast::*;
use kernc_lexer::TokenType;
use kernc_utils::Span;

impl<'a> Parser<'a> {
    fn parse_generic_params(&mut self) -> ParseResult<Vec<GenericParam>> {
        if !self.match_token(&[TokenType::LBracket]) {
            return Ok(Vec::new());
        }
        let mut params = Vec::new();
        while !self.check(TokenType::RBracket) && !self.check(TokenType::Eof) {
            let name = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name);
            let mut span = name.span;
            let mut constraints = Vec::new();

            if self.match_token(&[TokenType::Colon]) {
                loop {
                    let con = self.parse_type()?;
                    span = span.to(con.span);
                    constraints.push(con);
                    if !self.match_token(&[TokenType::Plus]) {
                        break;
                    }
                }
            }
            params.push(GenericParam {
                name: name_id,
                constraints,
                span,
            });
            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }
        self.expect(TokenType::RBracket)?;
        Ok(params)
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
        // 先解析文件最顶部的 #![...]
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
            path: "test.kr".to_string(),
            attributes,
            decls,
        })
    }

    fn parse_decl(&mut self) -> ParseResult<Option<Decl>> {
        // 拦截 attributes
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

        if is_extern {
            if self.check(TokenType::LBrace) || self.check(TokenType::StringLiteral) {
                if is_pub {
                    self.add_error(start_span, "Extern blocks cannot be pub".to_string());
                }
                return Ok(Some(self.parse_extern_block(start_span)?));
            }
        }

        let token = self.peek();
        let decl_res = match token.tag {
            TokenType::Mod => Ok(Some(self.parse_mod_decl(start_span, is_pub)?)),
            TokenType::Fn => Ok(Some(self.parse_fn_decl(start_span, is_pub, is_extern)?)),
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
                Err(())
            }
        };
        match decl_res {
            Ok(Some(mut decl)) => {
                decl.attributes = attributes;
                Ok(Some(decl))
            }
            other => other,
        }
    }

    fn parse_mod_decl(&mut self, start: Span, is_pub: bool) -> ParseResult<Decl> {
        self.advance(); // 消费 `mod`

        let name_token = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name_token);

        self.expect(TokenType::Semicolon)?;
        let end = self.stream.prev_span();

        Ok(Decl {
            id: self.new_id(),
            span: start.to(end),
            name: name_id,
            is_pub,
            attributes: vec![],
            kind: DeclKind::ModDecl { is_pub },
        })
    }

    fn parse_fn_decl(&mut self, start: Span, is_pub: bool, is_extern: bool) -> ParseResult<Decl> {
        self.advance(); // fn
        let name = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name);

        let generics = self.parse_generic_params()?;
        let (params, is_variadic) = self.parse_func_params()?;

        if is_variadic && !is_extern {
            self.add_error(start, "Variadic args only allowed in extern".to_string());
        }

        let ret_type = self.parse_type()?;

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
            name: name_id,
            is_pub,
            attributes: vec![],
            kind: DeclKind::Function {
                generics,
                params,
                ret_type,
                body,
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
            let attributes = self.parse_attributes(false).unwrap_or_default(); // 拦截
            let is_pub = self.match_token(&[TokenType::Pub]);
            let d_start = if is_pub {
                self.stream.prev_span()
            } else {
                self.peek().span
            };

            if self.check(TokenType::Fn) {
                let mut d = self.parse_fn_decl(d_start, is_pub, true)?;
                d.attributes = attributes; // 注入
                decls.push(d);
            } else if self.check(TokenType::Static) {
                let mut d = self.parse_global_var_decl(d_start, is_pub, true)?;
                d.attributes = attributes; // 注入
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
        self.expect(TokenType::LBrace)?;

        let mut decls = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let attributes = self.parse_attributes(false).unwrap_or_default(); // 拦截
            let is_pub = self.match_token(&[TokenType::Pub]);
            let d_start = if is_pub {
                self.stream.prev_span()
            } else {
                self.peek().span
            };
            if self.check(TokenType::Fn) {
                let mut d = self.parse_fn_decl(d_start, is_pub, false)?;
                d.attributes = attributes; // 注入
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
            name,
            is_pub: false,
            attributes: vec![],
            kind: DeclKind::Impl {
                generics,
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

        // 嗅探是否带有 mut (仅对 static 有效，const 不能 mut)
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

        // 全局变量同样拦截左侧冒号
        if self.match_token(&[TokenType::Colon]) {
            let err_span = self.stream.prev_span();
            self.add_error(err_span, "Global variables no longer support left-side type annotations. Use explicit constructors: `static X = Type.{ value };`".to_string());
            let _ = self.parse_type();
        }

        // Init
        let value;
        if self.match_token(&[TokenType::Assign]) {
            value = self.parse_expression(Precedence::Lowest)?;
        } else {
            // 无论是 extern 还是普通全局变量，都必须带 =
            self.add_error(
                start,
                "Global/extern vars must be initialized (use `= Type.{undef};` for externs)"
                    .to_string(),
            );
            return Err(());
        }
        self.expect(TokenType::Semicolon)?;
        let end = self.stream.prev_span();

        Ok(Decl {
            id: self.new_id(),
            span: start.to(end),
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
        self.advance(); // 消费 `type`
        let name = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name);

        // 解析泛型参数 [T]
        let generics = self.parse_generic_params()?;

        // 解析约束界限 `: Reader + Writer`
        let mut bounds = Vec::new();
        if self.match_token(&[TokenType::Colon]) {
            loop {
                bounds.push(self.parse_type()?);
                if !self.match_token(&[TokenType::Plus]) {
                    break;
                }
            }
        }

        self.expect(TokenType::Assign)?;
        let target = self.parse_type()?;
        self.expect(TokenType::Semicolon)?;

        let end = self.stream.prev_span();

        Ok(Decl {
            id: self.new_id(),
            span: start.to(end),
            name: name_id,
            is_pub,
            attributes: vec![],
            kind: DeclKind::TypeAlias {
                generics,
                bounds,
                target,
                is_extern,
            },
        })
    }

    fn parse_use_decl(&mut self, start: Span, is_pub: bool) -> ParseResult<Decl> {
        self.advance(); // 消费 `use`

        // 1. 精确且极简的起始路径解析
        let mut kind = UsePathKind::Root;

        if self.match_token(&[TokenType::Dot]) {
            kind = UsePathKind::Current;
        } else if self.match_token(&[TokenType::DotDot]) {
            kind = UsePathKind::Parent;
        }

        let mut path = Vec::new();
        let target: UseTarget;

        // 2. 循环读取路径段和目标
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

    // 辅助方法：专门解析大括号里的重导出成员 `{ Point, env.Args, new_point as np }`
    fn parse_use_members(&mut self) -> ParseResult<UseTarget> {
        let mut members = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let start_span = self.peek().span;
            let mut member_path = Vec::new();

            // 1. 内部循环：解析像 `env.Args` 这样的多段路径
            loop {
                let m_tok = self.expect(TokenType::Identifier)?;
                member_path.push(self.intern_token(m_tok));

                if self.match_token(&[TokenType::Dot]) {
                    continue; // 遇到点号，继续解析下一个 Identifier
                } else {
                    break;    // 不是点号，当前路径解析结束
                }
            }

            // 2. 解析别名 
            let mut alias = None;
            if self.match_token(&[TokenType::As]) {
                let a_tok = self.expect(TokenType::Identifier)?;
                alias = Some(self.intern_token(a_tok));
            }

            let end_span = self.stream.prev_span();

            // 3. 压入解析结果
            members.push(UseMember {
                path: member_path,
                alias,
                span: start_span.to(end_span),
            });

            // 4. 逗号分隔逻辑
            if !self.match_token(&[TokenType::Comma]) {
                break;
            }
        }
        self.expect(TokenType::RBrace)?;
        Ok(UseTarget::Members(members))
    }

    /// 将一个路径表达式强制转换为 TypeNode（用于处理 Type.{...} 的左侧）
    pub fn expr_to_type(&mut self, expr: Expr) -> ParseResult<TypeNode> {
        match expr.kind {
            ExprKind::Identifier(id) => Ok(TypeNode {
                id: self.new_id(),
                span: expr.span,
                kind: TypeKind::Path {
                    segments: vec![id],
                    generics: Vec::new(),
                },
            }),
            ExprKind::FieldAccess { lhs, field } => {
                let mut base = self.expr_to_type(*lhs)?;
                if let TypeKind::Path {
                    ref mut segments, ..
                } = base.kind
                {
                    segments.push(field);
                    base.span = base.span.to(expr.span);
                    Ok(base)
                } else {
                    self.add_error(expr.span, "Invalid path used as type".to_string());
                    Err(())
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
                    Err(())
                }
            }
            _ => {
                self.add_error(
                    expr.span,
                    "Invalid expression used as a type prefix".to_string(),
                );
                Err(())
            }
        }
    }
}
