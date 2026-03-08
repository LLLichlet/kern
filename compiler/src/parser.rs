#![allow(unused)]
use crate::ast::*;
use crate::context::Context;
use crate::lexer::Lexer;
use crate::token::{Token, TokenType};
use crate::stream::TokenStream;
use crate::utils::{Span, SymbolId};
use crate::utils::FileId;
use crate::diagnostic::DiagnosticLevel;

/// 解析结果类型
/// Err(()) 表示错误已经报告给 Context，调用者应该进行恢复或向上传播
pub type ParseResult<T> = Result<T, ()>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Precedence {
    Lowest,
    Assignment, // =
    LogicalOr,  // or
    LogicalAnd, // and
    Equality,   // == !=
    Comparison, // < > <= >=
    Term,       // + -
    Factor,     // * / %
    Unary,      // ! - * &
    Cast,       // as
    Call,       // () . []
}

impl Precedence {
    fn from_token(t: TokenType) -> Self {
        match t {
            TokenType::Dot | TokenType::DotLBracket | TokenType::DotLBrace | TokenType::DotStar | 
            TokenType::LParen | TokenType::LBracket | TokenType::DotAmpersand | 
            TokenType::Bang => Self::Call,

            TokenType::As => Self::Cast,

            TokenType::Star | TokenType::Slash | TokenType::Percent => Self::Factor,
            TokenType::Plus | TokenType::Minus | TokenType::Pipe | TokenType::Caret => Self::Term,

            TokenType::LessThan | TokenType::LessEqual | TokenType::GreaterThan | TokenType::GreaterEqual => Self::Comparison,
            TokenType::EqualEqual | TokenType::NotEqual => Self::Equality,

            TokenType::And => Self::LogicalAnd,
            TokenType::Or => Self::LogicalOr,

            TokenType::Assign | TokenType::PlusAssign | TokenType::MinusAssign |
            TokenType::StarAssign | TokenType::SlashAssign | TokenType::PercentAssign |
            TokenType::AmpersandAssign | TokenType::PipeAssign | TokenType::CaretAssign |
            TokenType::LShiftAssign | TokenType::RShiftAssign => Self::Assignment,

            _ => Self::Lowest,
        }
    }
}

pub struct Parser<'a> {
    stream: TokenStream<'a>,
    context: &'a mut Context,
    file_id: FileId,
    
    // 状态标记
    panic_mode: bool,
}

impl<'a> Parser<'a> {
    pub fn new(source: &'a str, file_id: FileId, context: &'a mut Context) -> Self {
        let lexer = Lexer::new(source, file_id);
        let stream = TokenStream::new(lexer);
        Self {
            stream,
            context,
            file_id,
            panic_mode: false,
        }
    }

    // ==========================================
    // Core Tools: AST Node Creation
    // ==========================================

    fn new_id(&mut self) -> NodeId {
        self.context.next_node_id()
    }

    // ==========================================
    // Core Tools: Token Consumption
    // ==========================================

    fn peek(&mut self) -> Token {
        self.stream.peek()
    }

    fn advance(&mut self) -> Token {
        self.stream.bump()
    }

    fn check(&mut self, tag: TokenType) -> bool {
        self.stream.check(tag)
    }

    fn match_token(&mut self, tags: &[TokenType]) -> bool {
        for &tag in tags {
            if self.check(tag) {
                self.advance();
                return true;
            }
        }
        false
    }

    /// 消费一个 Token，如果类型不对则报错 (Sync 入口)
    fn expect(&mut self, tag: TokenType) -> ParseResult<Token> {
        if self.check(tag) {
            Ok(self.advance())
        } else {
            let current = self.peek();
            let found_text = self.context.source_manager.slice_source(current.span).to_string();
            
            self.add_error(
                current.span,
                format!("Expected token '{:?}', but found '{}'.", tag, found_text),
            );
            Err(())
        }
    }

    // ==========================================
    // Integration: String Interner & Unescape
    // ==========================================

    fn intern_token(&mut self, token: Token) -> SymbolId {
        let text = self.context.source_manager.slice_source(token.span);
        self.context.interner.intern(text)
    }

    fn parse_string_literal(&mut self, token: Token) -> ParseResult<SymbolId> {
        let raw = self.context.source_manager.slice_source(token.span).to_string();
        
        // 1. 去掉引号
        if raw.len() < 2 {
            return Err(());
        }
        let inner = &raw[1..raw.len() - 1];

        // 2. 转义处理
        let unescaped = self.unescape_string(inner, token.span)?;

        // 3. Intern
        Ok(self.context.intern(&unescaped))
    }

    fn unescape_string(&mut self, input: &str, span: Span) -> ParseResult<String> {
        let mut result = String::with_capacity(input.len());
        let mut chars = input.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => result.push('\n'),
                    Some('r') => result.push('\r'),
                    Some('t') => result.push('\t'),
                    Some('\\') => result.push('\\'),
                    Some('\'') => result.push('\''),
                    Some('"') => result.push('"'),
                    Some('0') => result.push('\0'),
                    
                    // Hex: \xNN
                    Some('x') => {
                        let hex: String = chars.by_ref().take(2).collect();
                        if hex.len() != 2 {
                            self.add_error(span, "Invalid hex escape sequence".to_string());
                            return Err(());
                        }
                        let byte = u8::from_str_radix(&hex, 16).map_err(|_| {
                            self.add_error(span, format!("Invalid hex escape: {}", hex));
                        })?;
                        result.push(byte as char);
                    }

                    // Unicode: \u{...}
                    Some('u') => {
                        if chars.next() != Some('{') {
                             self.add_error(span, "Expected '{' after \\u".to_string());
                             return Err(());
                        }
                        
                        let mut hex_str = String::new();
                        let mut found_brace = false;
                        for ch in chars.by_ref() {
                            if ch == '}' {
                                found_brace = true;
                                break;
                            }
                            hex_str.push(ch);
                        }

                        if !found_brace {
                            self.add_error(span, "Unterminated unicode escape".to_string());
                            return Err(());
                        }

                        let code_point = u32::from_str_radix(&hex_str, 16).map_err(|_| {
                             self.add_error(span, format!("Invalid unicode scalar: {}", hex_str));
                        })?;
                        
                        if let Some(c) = std::char::from_u32(code_point) {
                            result.push(c);
                        } else {
                            self.add_error(span, format!("Invalid unicode scalar value: {:x}", code_point));
                            return Err(());
                        }
                    }

                    Some(c) => {
                        // 未知转义，保留原样
                        result.push('\\');
                        result.push(c);
                    }
                    None => {
                        self.add_error(span, "Unterminated escape sequence".to_string());
                        return Err(());
                    }
                }
            } else {
                result.push(c);
            }
        }
        Ok(result)
    }

    // ==========================================
    // Error Handling & Synchronization
    // ==========================================

    fn error_at_current(&mut self, msg: String) {
        let span = self.peek().span;
        self.add_error(span, msg);
    }

    fn add_error(&mut self, span: Span, msg: String) {
        if self.panic_mode {
            return;
        }
        self.panic_mode = true;
        self.context.report(span, DiagnosticLevel::Error, msg);
    }

    pub fn synchronize(&mut self) {
        self.panic_mode = false;

        while !self.check(TokenType::Eof) {
            if self.stream.prev_span().end > 0 && self.stream.prev_span() != Span::default() {
                 // 简单的近似：如果 TokenStream 可以访问上一个 Token 的类型最好
                 // 但这里我们根据 current token 判断
            }
            
            // 如果碰到了分号，很可能上一个语句结束了
             if self.stream.peek_nth(0).tag == TokenType::Semicolon {
                // 如果当前是分号，消耗它，然后结束 sync
                self.advance();
                return;
            }

            match self.peek().tag {
                TokenType::Fn | TokenType::Let | TokenType::Const | TokenType::Struct | 
                TokenType::Enum | TokenType::If | TokenType::For | TokenType::Return => return,
                _ => {
                    self.advance();
                }
            }
        }
    }

    // ==========================================
    //               Type Parsing
    // ==========================================

    pub fn parse_type(&mut self) -> ParseResult<TypeNode> {
        let start_token = self.peek();
        let start_span = start_token.span;

        match start_token.tag {
            // 1. 指针: *T 或 *mut T
            TokenType::Star => {
                self.advance();
 
                let elem = self.parse_type()?;
                Ok(TypeNode {
                    id: self.new_id(),
                    span: start_span.to(elem.span),
                    kind: TypeKind::Pointer {
                        elem: Box::new(elem),
                        
                    },
                })
            }

            // 2. 易失指针: ^T
            TokenType::Caret => {
                self.advance();
 
                let elem = self.parse_type()?;
                Ok(TypeNode {
                    id: self.new_id(),
                    span: start_span.to(elem.span),
                    kind: TypeKind::VolatilePtr {
                        elem: Box::new(elem),
                        
                    },
                })
            }

            // 3. 数组或切片
            TokenType::LBracket => {
                self.advance(); // eat [

                // A. 切片 []T
                if self.match_token(&[TokenType::RBracket]) {
     
                    let elem = self.parse_type()?;
                    Ok(TypeNode {
                        id: self.new_id(),
                        span: start_span.to(elem.span),
                        kind: TypeKind::Slice {
                            elem: Box::new(elem),
                            
                        },
                    })
                } else {
                    // B. 数组 [expr]T
                    let len_expr = self.parse_expression(Precedence::Lowest)?;
                    self.expect(TokenType::RBracket)?;
     
                    let elem = self.parse_type()?;
                    
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

            // 4. 函数指针 fn(args) ret
            TokenType::Fn => {
                self.advance();
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
                    }
                })
            }

            // 5. 路径类型
            TokenType::Identifier => {
                self.advance(); // consume first ident
                let first_id = self.intern_token(start_token);
                let mut span = start_span;
                
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
                    kind: TypeKind::Path {
                        segments,
                        generics,
                    }
                })
            }

            TokenType::Mut => {
                self.advance();
                let elem = self.parse_type()?;
                // 护城河：拦截 mut *, mut ^, mut []
                if matches!(elem.kind, TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. } | TypeKind::Slice { .. }) {
                    self.add_error(start_span.to(elem.span), "Forbidden: cannot apply 'mut' directly to pointers or slices (e.g., 'mut *T'). Pointer arithmetic is disabled.".to_string());
                }
                Ok(TypeNode {
                    id: self.new_id(),
                    span: start_span.to(elem.span),
                    kind: TypeKind::Mut(Box::new(elem)),
                })
            }

            TokenType::Underscore => {
                self.advance();
                Ok(TypeNode { id: self.new_id(), span: start_span, kind: TypeKind::Infer })
            }

            TokenType::SelfType => {
                self.advance();
                Ok(TypeNode { id: self.new_id(), span: start_span, kind: TypeKind::SelfType })
            }

            TokenType::Struct => self.parse_struct_type(false),
            TokenType::Union => self.parse_struct_type(true),
            TokenType::Enum => self.parse_enum_type(),
            TokenType::Trait => self.parse_trait_type(),

            _ => {
                let token = self.peek();
                let found_text = self.context.source_manager.slice_source(token.span).to_string();
                self.add_error(token.span, format!("Expected type definition, found '{}'", found_text));
                Err(())
            }
        }
    }

    fn parse_type_args(&mut self) -> ParseResult<Vec<TypeNode>> {
        self.expect(TokenType::LBracket)?;
        let mut args = Vec::new();
        if !self.check(TokenType::RBracket) {
            loop {
                args.push(self.parse_type()?);
                if !self.match_token(&[TokenType::Comma]) { break; }
                if self.check(TokenType::RBracket) { break; }
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
            
            let span = name_token.span.to(if let Some(ref v) = default_value { v.span } else { field_type.span });

            fields.push(StructFieldDef {
                name: name_id,
                type_node: field_type,
                default_value,
                span,
            });

            if !self.match_token(&[TokenType::Comma]) { break; }
        }

        let end_token = self.expect(TokenType::RBrace)?;
        let kind = if is_union { TypeKind::Union { fields } } else { TypeKind::Struct { fields } };

        Ok(TypeNode {
            id: self.new_id(),
            span: start_token.span.to(end_token.span),
            kind,
        })
    }

    fn parse_enum_type(&mut self) -> ParseResult<TypeNode> {
        let start_token = self.advance();
        let mut backing_type = None;
        if self.match_token(&[TokenType::Colon]) {
            backing_type = Some(Box::new(self.parse_type()?));
        }

        self.expect(TokenType::LBrace)?;
        let mut variants = Vec::new();

        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let name_token = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name_token);
            
            let mut value = None;
            if self.match_token(&[TokenType::Assign]) {
                value = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
            }

            let span = name_token.span.to(if let Some(ref v) = value { v.span } else { name_token.span });

            variants.push(EnumVariant { name: name_id, value, span });
            if !self.match_token(&[TokenType::Comma]) { break; }
        }

        let end_token = self.expect(TokenType::RBrace)?;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_token.span.to(end_token.span),
            kind: TypeKind::Enum { backing_type, variants },
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
            let method_type = self.parse_type()?;

            if self.check(TokenType::Assign) {
                self.error_at_current("Trait methods cannot have default implementations here.".to_string());
                self.advance();
                self.parse_expression(Precedence::Lowest)?; // consume expr
            }

            fields.push(StructFieldDef {
                name: name_id,
                default_value: None,
                span: name_token.span.to(method_type.span),
                type_node: method_type,
            });

            if !self.match_token(&[TokenType::Comma]) { break; }
        }
        let end_token = self.expect(TokenType::RBrace)?;

        Ok(TypeNode {
            id: self.new_id(),
            span: start_token.span.to(end_token.span),
            kind: TypeKind::Trait { fields },
        })
    }

    // ==========================================
    //            Expression Parsing
    // ==========================================

    pub fn parse_expression(&mut self, precedence: Precedence) -> ParseResult<Expr> {
        let prefix_token = self.advance();
        let mut left = self.parse_prefix(prefix_token)?;

        while precedence < Precedence::from_token(self.peek().tag) {
            let op_token = self.advance();
            left = self.parse_infix(left, op_token)?;
        }
        Ok(left)
    }

    fn parse_prefix(&mut self, token: Token) -> ParseResult<Expr> {
        let span = token.span;
        match token.tag {
            TokenType::IntLiteral => {
                let text = self.context.source_manager.slice_source(span).to_string();
                // Rust 处理 0xFF, 0b10 比较方便，但如果带下划线 1_000 需要去掉
                let text_clean = text.replace("_", "");
                
                // 处理前缀
                let (radix, num_str) = if text_clean.starts_with("0x") { (16, &text_clean[2..]) }
                else if text_clean.starts_with("0b") { (2, &text_clean[2..]) }
                else if text_clean.starts_with("0o") { (8, &text_clean[2..]) }
                else { (10, text_clean.as_str()) };

                let val = u128::from_str_radix(num_str, radix).map_err(|_| {
                    self.add_error(span, format!("Invalid integer literal: {}", text));
                })?;
                Ok(Expr { id: self.new_id(), span, kind: ExprKind::Integer(val) })
            }
            TokenType::FloatLiteral => {
                let text = self.context.source_manager.slice_source(span).replace("_", "");
                let val = text.parse::<f64>().map_err(|_| {
                    self.add_error(span, format!("Invalid float literal: {}", text));
                })?;
                Ok(Expr { id: self.new_id(), span, kind: ExprKind::Float(val) })
            }
            TokenType::StringLiteral => {
                let sid = self.parse_string_literal(token)?;
                Ok(Expr { id: self.new_id(), span, kind: ExprKind::String(self.context.resolve(sid).to_string()) })
            }
            TokenType::CharLiteral => {
                // 简化的 char 处理，严谨的需要 unescape
                let raw = self.context.source_manager.slice_source(span);
                let inner = &raw[1..raw.len()-1];
                let c = inner.chars().next().unwrap_or('\0');
                Ok(Expr { id: self.new_id(), span, kind: ExprKind::Char(c) })
            }
            TokenType::Identifier => {
                let name = self.intern_token(token);
                Ok(Expr { id: self.new_id(), span, kind: ExprKind::Identifier(name) })
            }
            
            // .{ ... }
            TokenType::DotLBrace => self.parse_data_init(None, span),

            // .Enum
            TokenType::Dot => {
                if self.check(TokenType::Identifier) {
                    let id_token = self.advance();
                    let sid = self.intern_token(id_token);
                    Ok(Expr {
                        id: self.new_id(),
                        span: span.to(id_token.span),
                        kind: ExprKind::EnumLiteral(sid),
                    })
                } else {
                    self.add_error(span, "Unexpected '.' at start of expression".to_string());
                    Err(())
                }
            }

            TokenType::Minus | TokenType::Bang | TokenType::Tilde | TokenType::Hash => {
                let op = match token.tag {
                    TokenType::Minus => UnaryOperator::Negate,
                    TokenType::Bang => UnaryOperator::LogicalNot,
                    TokenType::Tilde => UnaryOperator::BitwiseNot,
                    TokenType::Hash => UnaryOperator::LengthOf,
                    _ => unreachable!(),
                };
                let operand = self.parse_expression(Precedence::Unary)?;
                Ok(Expr {
                    id: self.new_id(),
                    span: span.to(operand.span),
                    kind: ExprKind::Unary { op, operand: Box::new(operand) }
                })
            }

            TokenType::LParen => {
                let expr = self.parse_expression(Precedence::Lowest)?;
                self.expect(TokenType::RParen)?;
                Ok(expr)
            }

            TokenType::If => self.parse_if_expr(span),
            TokenType::Switch => self.parse_switch_expr(span),
            TokenType::LBrace => self.parse_block_expr(span),
            TokenType::For => self.parse_for_expr(span),
            
            TokenType::Let | TokenType::Const | TokenType::Static => self.parse_decl_expr(token),

            TokenType::Break => Ok(Expr { id: self.new_id(), span, kind: ExprKind::Break }),
            TokenType::Continue => Ok(Expr { id: self.new_id(), span, kind: ExprKind::Continue }),
            
            TokenType::Return => {
                let mut val = None;
                let is_stopper = self.check(TokenType::Semicolon) || self.check(TokenType::RBrace) || 
                                 self.check(TokenType::Else) || self.check(TokenType::RParen) || 
                                 self.check(TokenType::RBracket) || self.check(TokenType::Comma) || 
                                 self.check(TokenType::Eof);
                if !is_stopper {
                    val = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
                }
                Ok(Expr { id: self.new_id(), span, kind: ExprKind::Return(val) })
            }

            TokenType::Undef => Ok(Expr { id: self.new_id(), span, kind: ExprKind::Undef }),
            TokenType::SelfValue => Ok(Expr { id: self.new_id(), span, kind: ExprKind::SelfValue }),
            TokenType::At => {
                let at_token = self.advance(); // 消费 @
                let id_token = self.expect(TokenType::Identifier)?;
                // 拼成如 "@sizeof" 的字符串并 Intern
                let sym = self.intern_token(id_token);
                let name_str = format!("@{}", self.context.resolve(sym));
                let sym_id = self.context.intern(&name_str);
                Ok(Expr { 
                    id: self.new_id(), 
                    span: at_token.span.to(id_token.span), 
                    kind: ExprKind::Identifier(sym_id) 
                })
            }

            TokenType::Mut => {
                // 特判：这必然是 `mut Type.{ ... }` 的开头
                // 手动组装 TypeNode：
                let elem = self.parse_type()?;
                let type_span = span.to(elem.span);
                let mut_type = TypeNode { id: self.new_id(), span: type_span, kind: TypeKind::Mut(Box::new(elem)) };
                
                self.expect(TokenType::DotLBrace)?;
                self.parse_data_init(Some(Box::new(mut_type)), span)
            }

            _ => {
                let text = self.context.source_manager.slice_source(span).to_string();
                self.add_error(span, format!("Expected expression, found '{}'", text));
                Err(())
            }
        }
    }

    fn parse_infix(&mut self, left: Expr, token: Token) -> ParseResult<Expr> {
        match token.tag {
            TokenType::Plus | TokenType::Minus | TokenType::Star | TokenType::Slash | 
            TokenType::EqualEqual | TokenType::NotEqual | TokenType::Percent |
            TokenType::LessThan | TokenType::LessEqual | TokenType::GreaterThan | TokenType::GreaterEqual |
            TokenType::And | TokenType::Or | TokenType::Pipe | TokenType::Ampersand | TokenType::Caret |
            TokenType::LShift | TokenType::RShift => {
                let op = BinaryOperator::from_token(token.tag);
                let precedence = Precedence::from_token(token.tag);
                let right = self.parse_expression(precedence)?;
                Ok(Expr {
                    id: self.new_id(),
                    span: left.span.to(right.span),
                    kind: ExprKind::Binary { lhs: Box::new(left), op, rhs: Box::new(right) }
                })
            }
            
            TokenType::Dot => {
                let field_token = self.expect(TokenType::Identifier)?;
                let field_id = self.intern_token(field_token);
                Ok(Expr {
                    id: self.new_id(),
                    span: left.span.to(field_token.span),
                    kind: ExprKind::FieldAccess { lhs: Box::new(left), field: field_id }
                })
            }

            TokenType::DotStar => {
                Ok(Expr {
                    id: self.new_id(),
                    span: left.span.to(token.span),
                    kind: ExprKind::Unary { op: UnaryOperator::PointerDeRef, operand: Box::new(left) }
                })
            }

            TokenType::LParen => {
                let mut args = Vec::new();
                if !self.check(TokenType::RParen) {
                    loop {
                        args.push(self.parse_expression(Precedence::Lowest)?);
                        if !self.match_token(&[TokenType::Comma]) { break; }
                    }
                }
                let end = self.expect(TokenType::RParen)?;
                Ok(Expr {
                    id: self.new_id(),
                    span: left.span.to(end.span),
                    kind: ExprKind::Call { callee: Box::new(left), args }
                })
            }

            TokenType::Assign | TokenType::PlusAssign | TokenType::MinusAssign | TokenType::StarAssign |
            TokenType::SlashAssign | TokenType::PercentAssign | TokenType::AmpersandAssign | 
            TokenType::PipeAssign | TokenType::CaretAssign | TokenType::LShiftAssign | TokenType::RShiftAssign => {
                let op = AssignmentOperator::from_token(token.tag);
                let right = self.parse_expression(Precedence::Lowest)?;
                Ok(Expr {
                    id: self.new_id(),
                    span: left.span.to(right.span),
                    kind: ExprKind::Assign { lhs: Box::new(left), op, rhs: Box::new(right) }
                })
            }

            TokenType::As => {
                let target = self.parse_type()?;
                Ok(Expr {
                    id: self.new_id(),
                    span: left.span.to(target.span),
                    kind: ExprKind::As { lhs: Box::new(left), target: Box::new(target) }
                })
            }

            // Slice or Index: .[
            TokenType::DotLBracket => {
 
                
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
                    Ok(Expr { id: self.new_id(), span, kind: ExprKind::SliceOp { lhs: Box::new(left), start, end, is_inclusive } })
                } else {
                    Ok(Expr { id: self.new_id(), span, kind: ExprKind::IndexAccess { lhs: Box::new(left), index: start.unwrap() } })
                }
            }

            TokenType::DotAmpersand => {
                let mut op = UnaryOperator::AddressOf;
                let mut span = left.span.to(token.span);
                Ok(Expr { id: self.new_id(), span, kind: ExprKind::Unary { op, operand: Box::new(left) } })
            }

            TokenType::LBracket => {
                // Generic Instantiation: expr[T, U]
                let mut types = Vec::new();
                if !self.check(TokenType::RBracket) {
                    loop {
                        types.push(self.parse_type()?);
                        if !self.match_token(&[TokenType::Comma]) { break; }
                        if self.check(TokenType::RBracket) { break; }
                    }
                }
                let rb = self.expect(TokenType::RBracket)?;
                Ok(Expr {
                    id: self.new_id(),
                    span: left.span.to(rb.span),
                    kind: ExprKind::GenericInstantiation { target: Box::new(left), types }
                })
            }

            TokenType::DotLBrace => {
                // 当遇到 `.{` 时，说明左侧的表达式其实是一个类型！
                let type_node = self.expr_to_type(left)?;
                let span = type_node.span;
                self.parse_data_init(Some(Box::new(type_node)), span)
            }

            _ => {
                self.add_error(token.span, format!("Unexpected infix token {:?}", token.tag));
                Err(())
            }
        }
    }

    // ==========================================
    //            Specific Expressions
    // ==========================================

    fn parse_data_init(&mut self, type_node: Option<Box<TypeNode>>, start_span: Span) -> ParseResult<Expr> {
        // 空数组兜底
        if self.check(TokenType::RBrace) {
            let rb = self.advance();
            return Ok(Expr { 
                id: self.new_id(), 
                span: start_span.to(rb.span), 
                kind: ExprKind::DataInit { type_node, literal: DataLiteralKind::Array(vec![]) } 
            });
        }

        // 1. 判断是否是 Struct 模式 (包含 field: value)
        let mut is_struct_mode = false;
        if self.check(TokenType::Identifier) {
            if self.stream.peek_nth(1).tag == TokenType::Colon {
                is_struct_mode = true;
            }
        }

        if is_struct_mode {
            let mut fields = Vec::new();
            while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
                let name = self.expect(TokenType::Identifier)?;
                let name_id = self.intern_token(name);
                self.expect(TokenType::Colon)?;
                let val = self.parse_expression(Precedence::Lowest)?;
                fields.push(StructFieldInit { name: name_id, value: val, span: name.span });
                if !self.match_token(&[TokenType::Comma]) { break; }
            }
            let rb = self.expect(TokenType::RBrace)?;
            Ok(Expr { 
                id: self.new_id(), 
                span: start_span.to(rb.span), 
                kind: ExprKind::DataInit { type_node, literal: DataLiteralKind::Struct(fields) } 
            })
        } else {
            // 2. 此时大括号内是一个普通的表达式
            let first = self.parse_expression(Precedence::Lowest)?;
            
            // 模式 A: [Repeat] .{ 0; 1024 }
            if self.match_token(&[TokenType::Semicolon]) {
                let count = self.parse_expression(Precedence::Lowest)?;
                let rb = self.expect(TokenType::RBrace)?;
                Ok(Expr { 
                    id: self.new_id(), 
                    span: start_span.to(rb.span), 
                    kind: ExprKind::DataInit { type_node, literal: DataLiteralKind::Repeat { value: Box::new(first), count: Box::new(count) } } 
                })
            } 
            // 模式 B: [Array] .{ 1, 2, 3 } (只要遇到了逗号，就一定是数组)
            else if self.match_token(&[TokenType::Comma]) {
                let mut elems = vec![first];
                while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
                    elems.push(self.parse_expression(Precedence::Lowest)?);
                    if !self.match_token(&[TokenType::Comma]) { break; }
                }
                let rb = self.expect(TokenType::RBrace)?;
                Ok(Expr { 
                    id: self.new_id(), 
                    span: start_span.to(rb.span), 
                    kind: ExprKind::DataInit { type_node, literal: DataLiteralKind::Array(elems) } 
                })
            } 
            // ✅ 模式 C: [Scalar] .{ 10 } 或者 Type.{ 1 << 12 }
            else {
                // 既没有逗号，也没有分号，那就是唯一的一个单值！直接包装为 Scalar！
                let rb = self.expect(TokenType::RBrace)?;
                Ok(Expr { 
                    id: self.new_id(), 
                    span: start_span.to(rb.span), 
                    kind: ExprKind::DataInit { type_node, literal: DataLiteralKind::Scalar(Box::new(first)) } 
                })
            }
        }
    }

    fn parse_block_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        let mut stmts = Vec::new();
        let mut result = None;
        let mut end_span = start_span;

        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            if self.check(TokenType::Defer) {
                let defer_t = self.advance();
                let expr = self.parse_expression(Precedence::Lowest)?;
                self.expect(TokenType::Semicolon)?;
                
                let defer_expr = Expr {
                    id: self.new_id(),
                    span: defer_t.span.to(self.stream.prev_span()),
                    kind: ExprKind::Defer { expr: Box::new(expr) }
                };
                stmts.push(Stmt { id: self.new_id(), span: defer_expr.span, kind: StmtKind::ExprStmt(defer_expr) });
                continue;
            }

            let expr = self.parse_expression(Precedence::Lowest)?;
            
            // 判断当前表达式是否是自带大括号的“块级表达式”
            let is_block_like = matches!(
                &expr.kind,
                ExprKind::If { .. } | ExprKind::Block { .. } | ExprKind::Switch { .. } | ExprKind::For { .. }
            );

            if self.match_token(&[TokenType::Semicolon]) {
                stmts.push(Stmt { id: self.new_id(), span: expr.span, kind: StmtKind::ExprStmt(expr) });
            } else if self.check(TokenType::RBrace) {
                // 如果紧跟着是 }，说明这是整个 Block 的返回值
                result = Some(Box::new(expr));
            } else if is_block_like {
                // 如果是块级表达式，没有分号也是合法的独立语句
                stmts.push(Stmt { id: self.new_id(), span: expr.span, kind: StmtKind::ExprStmt(expr) });
            } else {
                self.error_at_current("Expected semicolon".to_string());
                stmts.push(Stmt { id: self.new_id(), span: expr.span, kind: StmtKind::ExprStmt(expr) });
            }
        }
        let rb = self.expect(TokenType::RBrace)?;
        end_span = rb.span;
        Ok(Expr { id: self.new_id(), span: start_span.to(end_span), kind: ExprKind::Block { stmts, result } })
    }

    fn parse_if_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        self.expect(TokenType::LParen)?;
        let cond = self.parse_expression(Precedence::Lowest)?;
        self.expect(TokenType::RParen)?;
        let then_branch = self.parse_expression(Precedence::Lowest)?;
        let mut else_branch = None;
        if self.match_token(&[TokenType::Else]) {
            else_branch = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
        }
        let end = if let Some(ref e) = else_branch { e.span } else { then_branch.span };
        Ok(Expr {
             id: self.new_id(), span: start_span.to(end),
             kind: ExprKind::If { cond: Box::new(cond), then_branch: Box::new(then_branch), else_branch }
        })
    }

    fn parse_for_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        self.expect(TokenType::LParen)?;
        let mut init = None;
        if !self.check(TokenType::Semicolon) { init = Some(Box::new(self.parse_expression(Precedence::Lowest)?)); }
        self.expect(TokenType::Semicolon)?;

        let mut cond = None;
        if !self.check(TokenType::Semicolon) { cond = Some(Box::new(self.parse_expression(Precedence::Lowest)?)); }
        self.expect(TokenType::Semicolon)?;

        let mut post = None;
        if !self.check(TokenType::RParen) { post = Some(Box::new(self.parse_expression(Precedence::Lowest)?)); }
        self.expect(TokenType::RParen)?;

        let body = self.parse_expression(Precedence::Lowest)?;
        Ok(Expr {
             id: self.new_id(), span: start_span.to(body.span),
             kind: ExprKind::For { init, cond, post, body: Box::new(body) }
        })
    }
    
    fn parse_switch_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        self.expect(TokenType::LParen)?;
        let target = self.parse_expression(Precedence::Lowest)?;
        self.expect(TokenType::RParen)?;
        self.expect(TokenType::LBrace)?;
        
        let mut cases = Vec::new();
        let mut default_case = None;

        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            if self.match_token(&[TokenType::Else]) {
                self.expect(TokenType::Arrow)?;
                default_case = Some(Box::new(self.parse_switch_body()?));
                self.match_token(&[TokenType::Comma]);
                continue;
            }

            let mut patterns = Vec::new();
            loop {
                let start = self.parse_expression(Precedence::Lowest)?;
                if self.match_token(&[TokenType::DotDot]) {
                    let end = self.parse_expression(Precedence::Lowest)?;
                    patterns.push(SwitchPattern::Range { start, end, inclusive: false });
                } else if self.match_token(&[TokenType::DotDotEqual]) {
                    let end = self.parse_expression(Precedence::Lowest)?;
                    patterns.push(SwitchPattern::Range { start, end, inclusive: true });
                } else {
                    patterns.push(SwitchPattern::Value(start));
                }

                if !self.match_token(&[TokenType::Comma]) { break; }
                if self.check(TokenType::Arrow) { break; }
            }
            self.expect(TokenType::Arrow)?;
            let body = self.parse_switch_body()?;
            self.match_token(&[TokenType::Comma]);
            cases.push(SwitchCase { patterns, body, span: start_span /* imprecise */ });
        }
        let rb = self.expect(TokenType::RBrace)?;
        Ok(Expr { id: self.new_id(), span: start_span.to(rb.span), kind: ExprKind::Switch { target: Box::new(target), cases, default_case } })
    }

    fn parse_switch_body(&mut self) -> ParseResult<Expr> {
        if self.check(TokenType::LBrace) {
            let t = self.advance();
            self.parse_block_expr(t.span)
        } else {
            self.parse_expression(Precedence::Lowest)
        }
    }

    fn parse_decl_expr(&mut self, start_token: Token) -> ParseResult<Expr> {
        let tag = start_token.tag;

        let name = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name);

        // 🌟 护城河：拦截旧的冒号类型标注
        if self.match_token(&[TokenType::Colon]) {
            let err_span = self.stream.prev_span();
            self.add_error(err_span, "Type annotations on the left side of declarations are no longer supported. Use explicit constructor syntax on the right side: `let x = Type.{ value };`".to_string());
            // 为了恢复，我们假装吃掉这个类型，但不存入 AST
            let _ = self.parse_type(); 
        }

        self.expect(TokenType::Assign)?;
        let init = self.parse_expression(Precedence::Lowest)?;
        let span = start_token.span.to(init.span);

        match tag {
            TokenType::Static => Ok(Expr {
                id: self.new_id(), span,
                // ✅ 初始化直接接管全部语义
                kind: ExprKind::Static { name: name_id, init: Box::new(init) } 
            }),
            TokenType::Let | TokenType::Const => {
                Ok(Expr {
                    id: self.new_id(), span,
                    // ✅ 瘦身后的 Let
                    kind: ExprKind::Let { name: name_id, init: Box::new(init) }
                })
            },
            _ => unreachable!()
        }
    }

    // ==========================================
    //               Declarations
    // ==========================================

    fn parse_generic_params(&mut self) -> ParseResult<Vec<GenericParam>> {
        if !self.match_token(&[TokenType::LBracket]) { return Ok(Vec::new()); }
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
                    if !self.match_token(&[TokenType::Plus]) { break; }
                }
            }
            params.push(GenericParam { name: name_id, constraints, span });
            if !self.match_token(&[TokenType::Comma]) { break; }
        }
        self.expect(TokenType::RBracket)?;
        Ok(params)
    }

    fn parse_func_params(&mut self) -> ParseResult<(Vec<FuncParam>, bool)> {
        self.expect(TokenType::LParen)?;
        let mut params = Vec::new();
        let mut is_variadic = false;
        
        while !self.check(TokenType::RParen) && !self.check(TokenType::Eof) {
            if self.match_token(&[TokenType::Ellipsis]) {
                is_variadic = true;
                break;
            }
            
            let name = self.expect(TokenType::Identifier)?;
            let name_id = self.intern_token(name);
            self.expect(TokenType::Colon)?;
            let type_node = self.parse_type()?;
            
            params.push(FuncParam {
                name: name_id, 
                span: name.span.to(type_node.span), 
                type_node 
            });

            if !self.match_token(&[TokenType::Comma]) { break; }
        }
        self.expect(TokenType::RParen)?;
        Ok((params, is_variadic))
    }

    // Top Level
    pub fn parse_module(&mut self) -> ParseResult<Module> {
        let mut decls = Vec::new();
        while !self.check(TokenType::Eof) {
            match self.parse_decl() {
                Ok(Some(decl)) => decls.push(decl),
                Ok(None) => {}, // Skipped
                Err(_) => {
                    // Error already reported, sync and continue
                    self.synchronize();
                }
            }
        }
        Ok(Module { path: "test.kn".to_string(), decls })
    }

    fn parse_decl(&mut self) -> ParseResult<Option<Decl>> {
        let is_pub = self.match_token(&[TokenType::Pub]);
        let start_span = if is_pub { self.stream.prev_span() } else { self.peek().span };
        let is_extern = self.match_token(&[TokenType::Extern]);

        if is_extern {
            if self.check(TokenType::LBrace) || self.check(TokenType::StringLiteral) {
                if is_pub { self.add_error(start_span, "Extern blocks cannot be pub".to_string()); }
                return Ok(Some(self.parse_extern_block(start_span)?));
            }
        }

        let token = self.peek();
        match token.tag {
            TokenType::Fn => Ok(Some(self.parse_fn_decl(start_span, is_pub, is_extern)?)),
            TokenType::Type => Ok(Some(self.parse_type_alias_decl(start_span, is_pub, is_extern)?)),
            TokenType::Const | TokenType::Static => Ok(Some(self.parse_global_var_decl(start_span, is_pub, is_extern)?)),
            TokenType::Use => Ok(Some(self.parse_use_decl(start_span, is_pub)?)),
            TokenType::Impl => {
                if is_pub { self.add_error(start_span, "impl blocks cannot be pub".to_string()); }
                Ok(Some(self.parse_impl_decl(start_span)?))
            },
            TokenType::Semicolon => { self.advance(); Ok(None) },
            TokenType::Eof => Ok(None),
            _ => {
                let txt = self.context.source_manager.slice_source(token.span).to_string();
                self.add_error(token.span, format!("Expected declaration, found '{}'", txt));
                Err(())
            }
        }
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
        
        let body = if is_extern {
            self.expect(TokenType::Semicolon)?;
            None
        } else {
            let brace = self.expect(TokenType::LBrace)?;
            Some(Box::new(self.parse_block_expr(brace.span)?))
        };
        
        let end = if let Some(ref b) = body { b.span } else { self.stream.prev_span() };

        Ok(Decl {
             id: self.new_id(), span: start.to(end), name: name_id, is_pub,
             kind: DeclKind::Function { generics, params, ret_type, body, is_extern, is_variadic }
        })
    }
    
    fn parse_extern_block(&mut self, start: Span) -> ParseResult<Decl> {
        let mut abi = None;
        if self.check(TokenType::StringLiteral) {
            let t = self.advance();
            let sid = self.parse_string_literal(t)?;
            abi = Some(self.context.resolve(sid).to_string());
        }
        self.expect(TokenType::LBrace)?;
        
        let mut decls = Vec::new();
        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let is_pub = self.match_token(&[TokenType::Pub]);
            let d_start = if is_pub { self.stream.prev_span() } else { self.peek().span };
            
            if self.check(TokenType::Fn) {
                decls.push(self.parse_fn_decl(d_start, is_pub, true)?);
            } else if self.check(TokenType::Static) {
                decls.push(self.parse_global_var_decl(d_start, is_pub, true)?);
            } else {
                self.error_at_current("Only fn and static allowed in extern".to_string());
                self.synchronize();
            }
        }
        let end = self.expect(TokenType::RBrace)?;
        let name = self.context.intern("extern_block");
        Ok(Decl { id: self.new_id(), span: start.to(end.span), name, is_pub: false, kind: DeclKind::ExternBlock { abi, decls } })
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
            let is_pub = self.match_token(&[TokenType::Pub]);
            let d_start = if is_pub { self.stream.prev_span() } else { self.peek().span };
            if self.check(TokenType::Fn) {
                decls.push(self.parse_fn_decl(d_start, is_pub, false)?);
            } else {
                self.error_at_current("Only fn allowed in impl".to_string());
                self.synchronize();
            }
        }
        let end = self.expect(TokenType::RBrace)?;
        let name = self.context.intern("impl");
        Ok(Decl { id: self.new_id(), span: start.to(end.span), name, is_pub: false, kind: DeclKind::Impl { generics, target_type, trait_type, decls } })
    }

    fn parse_global_var_decl(&mut self, start: Span, is_pub: bool, is_extern: bool) -> ParseResult<Decl> {
        let kw = self.advance();
        let is_static = kw.tag == TokenType::Static;
        
        let name = self.expect(TokenType::Identifier)?;
        let name_id = self.intern_token(name);
        
        // 🌟 护城河：全局变量同样拦截左侧冒号
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
            // 不再自动补齐，无论是 extern 还是普通全局变量，都必须带 =
            self.add_error(start, "Global/extern vars must be initialized (use `= Type.{undef};` for externs)".to_string());
            return Err(());
        }
        self.expect(TokenType::Semicolon)?;
        let end = self.stream.prev_span();

        Ok(Decl {
            id: self.new_id(), span: start.to(end), name: name_id, is_pub,
            // ✅ 瘦身后的 Var
            kind: DeclKind::Var { value, is_static, is_extern }
        })
    }

    fn parse_type_alias_decl(&mut self, start: Span, is_pub: bool, is_extern: bool) -> ParseResult<Decl> {
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
             id: self.new_id(), span: start.to(end), name: name_id, is_pub,
             kind: DeclKind::TypeAlias { generics, bounds, target, is_extern }
        })
    }

    fn parse_use_decl(&mut self, start: Span, is_pub: bool) -> ParseResult<Decl> {
        self.advance();
        let mut kind = UsePathKind::Absolute;
        if self.match_token(&[TokenType::Dot]) { kind = UsePathKind::Relative; }
        else if self.match_token(&[TokenType::DotDot]) { kind = UsePathKind::Super; }
        
        let mut path = Vec::new();
        loop {
            if self.check(TokenType::LBrace) { break; }
            let id = self.expect(TokenType::Identifier)?;
            path.push(self.intern_token(id));
            if !self.match_token(&[TokenType::Dot]) { break; }
        }

        let target;
        if self.check(TokenType::LBrace) {
            self.advance();
            let mut members = Vec::new();
            while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
                 let m_tok = self.expect(TokenType::Identifier)?;
                 let m_id = self.intern_token(m_tok);
                 let mut alias = None;
                 if self.match_token(&[TokenType::As]) {
                     let a_tok = self.expect(TokenType::Identifier)?;
                     alias = Some(self.intern_token(a_tok));
                 }
                 members.push(UseMember { name: m_id, alias, span: m_tok.span });
                 if !self.match_token(&[TokenType::Comma]) { break; }
            }
            self.expect(TokenType::RBrace)?;
            target = UseTarget::Members(members);
        } else {
            let mut alias = None;
            if self.match_token(&[TokenType::As]) {
                let a = self.expect(TokenType::Identifier)?;
                alias = Some(self.intern_token(a));
            }
            target = UseTarget::Module(alias);
        }
        self.expect(TokenType::Semicolon)?;
        let name = if let Some(&last) = path.last() { last } else { self.context.intern("root") };
        
        Ok(Decl {
             id: self.new_id(), span: start.to(self.stream.prev_span()), name, is_pub,
             kind: DeclKind::Use { kind, path, target, is_reexport: is_pub }
        })
    }

    /// 将一个路径表达式强制转换为 TypeNode（用于处理 Type.{...} 的左侧）
    fn expr_to_type(&mut self, expr: Expr) -> ParseResult<TypeNode> {
        match expr.kind {
            ExprKind::Identifier(id) => Ok(TypeNode {
                id: self.new_id(), span: expr.span,
                kind: TypeKind::Path { segments: vec![id], generics: Vec::new() }
            }),
            ExprKind::FieldAccess { lhs, field } => {
                let mut base = self.expr_to_type(*lhs)?;
                if let TypeKind::Path { ref mut segments, .. } = base.kind {
                    segments.push(field);
                    base.span = base.span.to(expr.span);
                    Ok(base)
                } else {
                    self.add_error(expr.span, "Invalid path used as type".to_string());
                    Err(())
                }
            },
            ExprKind::GenericInstantiation { target, types } => {
                let mut base = self.expr_to_type(*target)?;
                if let TypeKind::Path { ref mut generics, .. } = base.kind {
                    *generics = types;
                    base.span = base.span.to(expr.span);
                    Ok(base)
                } else {
                    self.add_error(expr.span, "Invalid generic type target".to_string());
                    Err(())
                }
            },
            _ => {
                self.add_error(expr.span, "Invalid expression used as a type prefix".to_string());
                Err(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*; // 确保引入了所有的 AST 结构

    // 测试辅助结构体
    struct TestContext {
        context: Context,
        file_id: FileId,
    }

    impl TestContext {
        fn new(source: &str) -> Self {
            let mut context = Context::new();
            let file_id = context.source_manager.add_file("test.kn".to_string(), source.to_string());
            Self { context, file_id }
        }

        fn parse(&mut self) -> Module {
            let src = self.context.source_manager.get_file(self.file_id).unwrap().src.clone();
            let mut parser = Parser::new(&src, self.file_id, &mut self.context);
            parser.parse_module().expect("Parse failed")
        }
    }

    #[test]
    fn test_basic_function_and_call() {
        let source = r#"
            fn main() void {
                let x = 10;
                print(x);
            }
        "#;
        let mut ctx = TestContext::new(source);
        let mod_ = ctx.parse();

        assert_eq!(mod_.decls.len(), 1);
        
        let func = &mod_.decls[0];
        if let DeclKind::Function { body, .. } = &func.kind {
            let func_name = ctx.context.resolve(func.name);
            assert_eq!(func_name, "main");

            let body_expr = body.as_ref().expect("Body should exist");
            if let ExprKind::Block { stmts, .. } = &body_expr.kind {
                assert_eq!(stmts.len(), 2);
            } else {
                panic!("Body is not a block");
            }
        } else {
            panic!("Expected Function declaration");
        }
        
        assert!(!ctx.context.has_errors());
    }

    #[test]
    fn test_struct_definition() {
        let source = r#"
            type Point = struct {
                x: i32,
                y: i32 = 0,
            };
        "#;
        let mut ctx = TestContext::new(source);
        let mod_ = ctx.parse();

        assert_eq!(mod_.decls.len(), 1);
        let decl = &mod_.decls[0];
        
        if let DeclKind::TypeAlias { target, .. } = &decl.kind {
            if let TypeKind::Struct { fields } = &target.kind {
                assert_eq!(fields.len(), 2);
                let f1_name = ctx.context.resolve(fields[0].name);
                assert_eq!(f1_name, "x");
            } else {
                panic!("Expected Struct type");
            }
        } else {
            panic!("Expected TypeAlias declaration");
        }
    }

    #[test]
    fn test_expr_precedence() {
        let source = r#"
            fn calc() void {
                let a = 1 + 2 * 3;
                let b = (1 + 2) * 3;
            }
        "#;
        let mut ctx = TestContext::new(source);
        let mod_ = ctx.parse();
        
        let func = &mod_.decls[0];
        let body = match &func.kind {
            DeclKind::Function { body: Some(b), .. } => b,
            _ => panic!("Expected function with body"),
        };
        
        let stmts = match &body.kind {
            ExprKind::Block { stmts, .. } => stmts,
            _ => panic!("Expected block"),
        };

        // 1. let a = 1 + 2 * 3;
        let init1 = match &stmts[0].kind {
            StmtKind::ExprStmt(e) => match &e.kind {
                // 注意：移除了旧的 is_mut
                ExprKind::Let { init, .. } => init,
                _ => panic!("Expected Let"),
            },
            _ => panic!("Expected ExprStmt"),
        };

        if let ExprKind::Binary { op, rhs, .. } = &init1.kind {
            assert_eq!(*op, BinaryOperator::Add); // Top level is +
            if let ExprKind::Binary { op: op2, .. } = &rhs.kind {
                assert_eq!(*op2, BinaryOperator::Multiply); // RHS is *
            } else { panic!("RHS should be multiply"); }
        } else { panic!("Expected Binary"); }

        // 2. let b = (1 + 2) * 3;
        let init2 = match &stmts[1].kind {
            StmtKind::ExprStmt(e) => match &e.kind {
                ExprKind::Let { init, .. } => init,
                _ => panic!("Expected Let"),
            },
            _ => panic!("Expected ExprStmt"),
        };

        if let ExprKind::Binary { lhs, op, .. } = &init2.kind {
            assert_eq!(*op, BinaryOperator::Multiply); // Top level is *
            if let ExprKind::Binary { op: op2, .. } = &lhs.kind {
                assert_eq!(*op2, BinaryOperator::Add); // LHS is +
            } else { panic!("LHS should be add"); }
        } else { panic!("Expected Binary"); }
    }

    #[test]
    fn test_control_flow() {
        let source = r#"
            fn flow(x: bool) void {
                if (x) return else return;
                for (;;) break;
            }
        "#;
        let mut ctx = TestContext::new(source);
        let mod_ = ctx.parse();

        let func = &mod_.decls[0];
        let stmts = match &func.kind {
            DeclKind::Function { body: Some(b), .. } => match &b.kind {
                ExprKind::Block { stmts, .. } => stmts,
                _ => panic!(),
            },
            _ => panic!(),
        };

        // 1. If
        let if_expr = match &stmts[0].kind {
            StmtKind::ExprStmt(e) => e,
            _ => panic!(),
        };
        if let ExprKind::If { cond, else_branch, .. } = &if_expr.kind {
            if let ExprKind::Identifier(_) = cond.kind {} else { panic!("Cond should be ident"); }
            assert!(else_branch.is_some());
        } else { panic!("Expected If"); }

        // 2. For
        let for_expr = match &stmts[1].kind {
            StmtKind::ExprStmt(e) => e,
            _ => panic!(),
        };
        if let ExprKind::For { init, cond, post, body } = &for_expr.kind {
            assert!(init.is_none());
            assert!(cond.is_none());
            assert!(post.is_none());
            if let ExprKind::Break = body.kind {} else { panic!("Body should be break"); }
        } else { panic!("Expected For"); }
    }

    #[test]
    fn test_complex_types() {
        let source = r#"
            type MyType = struct {
                ptr: *mut i32,
                arr: [10]mut u8,
                slice: []u8,
                map: Map[String, i32],
            };
        "#;
        let mut ctx = TestContext::new(source);
        let mod_ = ctx.parse();

        let fields = match &mod_.decls[0].kind {
            DeclKind::TypeAlias { target, .. } => match &target.kind {
                TypeKind::Struct { fields } => fields,
                _ => panic!(),
            },
            _ => panic!(),
        };

        // 1. *mut i32 -> Pointer(Mut(i32))
        match &fields[0].type_node.kind {
            TypeKind::Pointer { elem } => {
                match &elem.kind {
                    TypeKind::Mut(_) => {}, // 验证包含 Mut 节点
                    _ => panic!("Expected Mut inside Pointer"),
                }
            },
            _ => panic!("Expected Pointer"),
        }

        // 2. [10]mut u8 -> Array(Mut(u8), 10)
        match &fields[1].type_node.kind {
            TypeKind::Array { elem, len } => {
                match &elem.kind {
                    TypeKind::Mut(_) => {}, // 验证包含 Mut 节点
                    _ => panic!("Expected Mut inside Array"),
                }
                match &len.kind {
                    ExprKind::Integer(v) => assert_eq!(*v, 10),
                    _ => panic!("Expected integer len"),
                }
            },
            _ => panic!("Expected Array"),
        }

        // 3. []u8 -> Slice(u8) (不包含 Mut)
        match &fields[2].type_node.kind {
            TypeKind::Slice { elem } => {
                match &elem.kind {
                    TypeKind::Mut(_) => panic!("Did not expect Mut inside immutable Slice"),
                    TypeKind::Path { .. } => {},
                    _ => panic!("Expected Path inside Slice"),
                }
            },
            _ => panic!("Expected Slice"),
        }

        // 4. Map
        match &fields[3].type_node.kind {
            TypeKind::Path { .. } => {},
            _ => panic!("Expected Path"),
        }
    }

    #[test]
    fn test_global_variables() {
        let source = r#"
            static x: i32 = 10;
            const y: f32 = 3.14;
            extern static z: i32;
        "#;
        let mut ctx = TestContext::new(source);
        let mod_ = ctx.parse();

        // 1. static x
        match &mod_.decls[0].kind {
            DeclKind::Var { is_static, is_extern, value, .. } => {
                assert!(is_static);
                assert!(!is_extern);
                match value.kind { ExprKind::Undef => panic!("Should have value"), _ => {} }
            },
            _ => panic!(),
        }

        // 2. const y
        match &mod_.decls[1].kind {
            DeclKind::Var { is_static, is_extern, .. } => {
                assert!(!is_static);
                assert!(!is_extern);
            },
            _ => panic!(),
        }

        // 3. extern static z
        match &mod_.decls[2].kind {
            DeclKind::Var { is_static, is_extern, value, .. } => {
                assert!(is_static);
                assert!(is_extern);
                match value.kind { ExprKind::Undef => {}, _ => panic!("Extern should be undef") }
            },
            _ => panic!(),
        }
    }

    #[test]
    fn test_postfix_address_of() {
        let source = r#"
            fn main() void {
                let p1 = x.&;
                let p2 = instance.data.&;
                let p3 = p1.*.&;
            }
        "#;
        let mut ctx = TestContext::new(source);
        let mod_ = ctx.parse();
        
        let stmts = match &mod_.decls[0].kind {
            DeclKind::Function { body: Some(b), .. } => match &b.kind {
                 ExprKind::Block { stmts, .. } => stmts,
                 _ => panic!(),
            },
            _ => panic!(),
        };

        fn get_unary_op(stmt: &crate::ast::Stmt) -> UnaryOperator {
            match &stmt.kind {
                StmtKind::ExprStmt(e) => match &e.kind {
                    ExprKind::Let { init, .. } => match &init.kind {
                        ExprKind::Unary { op, .. } => *op,
                        _ => panic!("Expected Unary"),
                    },
                    _ => panic!("Expected Let"),
                },
                _ => panic!("Expected ExprStmt"),
            }
        }

        assert_eq!(get_unary_op(&stmts[0]), UnaryOperator::AddressOf);
        assert_eq!(get_unary_op(&stmts[1]), UnaryOperator::AddressOf);
        assert_eq!(get_unary_op(&stmts[2]), UnaryOperator::AddressOf);
    }

    #[test]
    fn test_switch_expr() {
        let source = r#"
            fn check(val: i32) i32 {
                return switch (val) {
                    1..10 => 10,
                    11, 12 => 20,
                    else => 0,
                };
            }
        "#;
        let mut ctx = TestContext::new(source);
        let mod_ = ctx.parse();

        let func = &mod_.decls[0];
        let stmts = match &func.kind {
            DeclKind::Function { body: Some(b), .. } => match &b.kind {
                ExprKind::Block { stmts, .. } => stmts,
                _ => panic!(),
            },
            _ => panic!(),
        };

        let ret_stmt = match &stmts[0].kind {
            StmtKind::ExprStmt(e) => e,
            _ => panic!(),
        };

        if let ExprKind::Return(Some(ret_val)) = &ret_stmt.kind {
            if let ExprKind::Switch { cases, default_case, .. } = &ret_val.kind {
                assert_eq!(cases.len(), 2);
                assert!(default_case.is_some());
            } else {
                panic!("Expected Switch expression");
            }
        } else {
            panic!("Expected Return statement");
        }
    }
}