use super::{ParseResult, Parser};
use kernc_ast::*;
use kernc_lexer::{Token, TokenType};
use kernc_utils::{Span, SymbolId};

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
            TokenType::Dot
            | TokenType::DotLBracket
            | TokenType::DotDotLBracket
            | TokenType::DotLBrace
            | TokenType::DotStar
            | TokenType::LParen
            | TokenType::LBracket
            | TokenType::DotAmpersand
            | TokenType::DotDotAmpersand
            | TokenType::Bang => Self::Call,

            TokenType::As => Self::Cast,

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

impl<'a> Parser<'a> {
    //  Binding Pattern
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
            is_mut,
            span,
        })
    }

    pub fn parse_expression(&mut self, precedence: Precedence) -> ParseResult<Expr> {
        let prefix_token = self.advance();
        let mut left = self.parse_prefix(prefix_token)?;

        while precedence < Precedence::from_token(self.peek().tag) {
            let next_tag = self.peek().tag;

            // 1. 防止后缀 `.{` 贪婪吞噬下一行的前缀 `.{`
            if next_tag == TokenType::DotLBrace {
                // 只有标识符、路径访问或泛型，才有资格做 `Type.{}` 的左前缀
                let is_type_prefix = matches!(
                    left.kind,
                    ExprKind::Identifier(_)
                        | ExprKind::FieldAccess { .. }
                        | ExprKind::GenericInstantiation { .. }
                );
                if !is_type_prefix {
                    break; // 停止粘合,让下一行去作为独立的 `.{ ... }` 解析
                }
            }

            // 2. 防止无返回值的控制流块贪婪吞噬下一行
            // 在 Kern 中，for、无 else 的 if、无尾表达式的 block 必然计算为 void。
            // 它们不可能作为左操作数参与任何中缀运算（比如函数调用 (、算术 + 等）。
            // 遇到它们直接 break，从而解决大括号后未加分号时被下一行误认为函数调用的二义性
            let is_manifestly_void = match &left.kind {
                ExprKind::For { .. } => true,
                ExprKind::If {
                    else_branch: None, ..
                } => true, // 没有 else 的 if 必定为 void
                ExprKind::Block { result: None, .. } => true, // 没有 result 的 block 必定为 void
                _ => false,
            };

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
            // Literals
            TokenType::IntLiteral
            | TokenType::FloatLiteral
            | TokenType::StringLiteral
            | TokenType::ByteCharLiteral
            | TokenType::CharLiteral => self.parse_literal_expr(token),

            // 处理布尔字面量关键字
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

            TokenType::Identifier => {
                let name = self.intern_token(token);
                Ok(Expr {
                    id: self.new_id(),
                    span,
                    kind: ExprKind::Identifier(name),
                })
            }

            // Unary & Enums
            TokenType::DotLBrace => self.parse_data_init(None, span),
            TokenType::Dot => self.parse_enum_literal_expr(span),
            TokenType::Minus | TokenType::Bang | TokenType::Tilde | TokenType::Hash => {
                self.parse_unary_prefix_expr(token)
            }
            TokenType::LParen => self.parse_grouped_expr(span),

            // Control Flow & Blocks
            TokenType::If => self.parse_if_expr(span),
            TokenType::Match => self.parse_match_expr(span),
            TokenType::LBrace => self.parse_block_expr(span),
            TokenType::For => self.parse_for_expr(span),
            TokenType::Let | TokenType::Const | TokenType::Static => self.parse_decl_expr(token),

            // Jumps
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

            // Special / Intrinsics
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

            // Explicitly Typed Initializations (e.g., [N]T.{...}, *T.{...})
            TokenType::LBracket | TokenType::Star | TokenType::Caret => {
                self.parse_typed_data_init_prefix(token)
            }

            TokenType::Underscore => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::Infer,
            }),

            // Closure
            TokenType::DotLBracket => self.parse_closure_expr(span),

            _ => {
                let text = self.session.source_manager.slice_source(span).to_string();
                self.add_error(span, format!("Expected expression, found '{}'", text));
                Err(())
            }
        }
    }

    fn parse_infix(&mut self, left: Expr, token: Token) -> ParseResult<Expr> {
        match token.tag {
            // Binary Operators
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

            // Field & Method Access
            TokenType::Dot => self.parse_field_access_expr(left),
            TokenType::LParen => self.parse_call_expr(left),

            // Pointer Deref & AddressOf
            TokenType::DotStar => Ok(Expr {
                id: self.new_id(),
                span: left.span.to(token.span),
                kind: ExprKind::Unary {
                    op: UnaryOperator::PointerDeRef,
                    operand: Box::new(left),
                },
            }),
            TokenType::DotAmpersand => Ok(Expr {
                id: self.new_id(),
                span: left.span.to(token.span),
                kind: ExprKind::Unary {
                    op: UnaryOperator::AddressOf,
                    operand: Box::new(left),
                },
            }),
            TokenType::DotDotAmpersand => Ok(Expr {
                id: self.new_id(),
                span: left.span.to(token.span),
                kind: ExprKind::Unary {
                    op: UnaryOperator::MutAddressOf,
                    operand: Box::new(left),
                },
            }),

            // Assignments
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

            // Casts & Indexing/Slicing
            TokenType::As => self.parse_as_cast_expr(left),
            TokenType::DotLBracket => self.parse_slice_or_index_expr(left, false),
            TokenType::DotDotLBracket => self.parse_slice_or_index_expr(left, true),
            TokenType::LBracket => self.parse_generic_instantiation_expr(left),

            // Type-affixed Enum Init (Type.{...})
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
                Err(())
            }
        }
    }

    // --- Prefix Sub-Routines ---

    fn parse_literal_expr(&mut self, token: Token) -> ParseResult<Expr> {
        let span = token.span;
        match token.tag {
            TokenType::IntLiteral => {
                let text = self.session.source_manager.slice_source(span).to_string();
                let text_clean = text.replace("_", "");
                let (radix, num_str) = if text_clean.starts_with("0x") {
                    (16, &text_clean[2..])
                } else if text_clean.starts_with("0b") {
                    (2, &text_clean[2..])
                } else if text_clean.starts_with("0o") {
                    (8, &text_clean[2..])
                } else {
                    (10, text_clean.as_str())
                };

                let val = u128::from_str_radix(num_str, radix).map_err(|_| {
                    self.add_error(span, format!("Invalid integer literal: {}", text));
                })?;
                Ok(Expr {
                    id: self.new_id(),
                    span,
                    kind: ExprKind::Integer(val),
                })
            }
            TokenType::FloatLiteral => {
                let text = self
                    .session
                    .source_manager
                    .slice_source(span)
                    .replace("_", "");
                let val = text.parse::<f64>().map_err(|_| {
                    self.add_error(span, format!("Invalid float literal: {}", text));
                })?;
                Ok(Expr {
                    id: self.new_id(),
                    span,
                    kind: ExprKind::Float(val),
                })
            }
            TokenType::StringLiteral => {
                let sid = self.parse_string_literal(token)?;
                Ok(Expr {
                    id: self.new_id(),
                    span,
                    kind: ExprKind::String(self.session.resolve(sid).to_string()),
                })
            }
            TokenType::CharLiteral => {
                let raw = self.session.source_manager.slice_source(span).to_string();
                let inner = &raw[1..raw.len() - 1];

                let c = if inner.is_empty() {
                    self.add_error(span, "Empty character literal".to_string());
                    '\0' // 兜底恢复
                } else {
                    match self.unescape_string(inner, span) {
                        Ok(unescaped) => {
                            let mut chars = unescaped.chars();
                            if let Some(ch) = chars.next() {
                                // 确切检查转义后是否只包含一个字符 (防止出现 '\n\n')
                                if chars.next().is_some() {
                                    self.add_error(
                                        span,
                                        "Character literal may only contain one character"
                                            .to_string(),
                                    );
                                }
                                ch
                            } else {
                                self.add_error(
                                    span,
                                    "Empty character literal after unescaping".to_string(),
                                );
                                '\0'
                            }
                        }
                        Err(()) => '\0', // 错误已经在 unescape_string 中报告过了
                    }
                };

                Ok(Expr {
                    id: self.new_id(),
                    span,
                    kind: ExprKind::Char(c),
                })
            }
            TokenType::ByteCharLiteral => {
                let raw = self.session.source_manager.slice_source(span).to_string();
                // 提取 b' 和 ' 中间的内容
                let inner = &raw[2..raw.len() - 1];

                let byte_val = if inner.is_empty() {
                    self.add_error(span, "Empty byte character literal".to_string());
                    0u8
                } else {
                    match self.unescape_string(inner, span) {
                        Ok(unescaped) => {
                            let mut chars = unescaped.chars();
                            if let Some(ch) = chars.next() {
                                if chars.next().is_some() {
                                    self.add_error(
                                        span,
                                        "Byte character literal may only contain one byte"
                                            .to_string(),
                                    );
                                }
                                // 严格校验必须是合法的单字节 ASCII 或转义字节 (<= 255)
                                if ch as u32 > 255 {
                                    self.add_error(
                                        span,
                                        "Byte character literal must be an ASCII character or a valid byte escape (<= 0xFF)".to_string(),
                                    );
                                    0u8
                                } else {
                                    ch as u8
                                }
                            } else {
                                self.add_error(
                                    span,
                                    "Empty byte character literal after unescaping".to_string(),
                                );
                                0u8
                            }
                        }
                        Err(()) => 0u8, // 错误已在 unescape_string 中报告
                    }
                };

                Ok(Expr {
                    id: self.new_id(),
                    span,
                    kind: ExprKind::ByteChar(byte_val),
                })
            }
            _ => unreachable!(),
        }
    }

    fn parse_enum_literal_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        if self.check(TokenType::Identifier) {
            let id_token = self.advance();
            let sid = self.intern_token(id_token);
            Ok(Expr {
                id: self.new_id(),
                span: start_span.to(id_token.span),
                kind: ExprKind::EnumLiteral(sid),
            })
        } else {
            self.add_error(
                start_span,
                "Unexpected '.' at start of expression".to_string(),
            );
            Err(())
        }
    }

    fn parse_unary_prefix_expr(&mut self, token: Token) -> ParseResult<Expr> {
        let op = match token.tag {
            TokenType::Minus => UnaryOperator::Negate,
            TokenType::Bang => UnaryOperator::LogicalNot,
            TokenType::Tilde => UnaryOperator::BitwiseNot,
            TokenType::Hash => UnaryOperator::MetaOf,
            _ => unreachable!(),
        };
        let operand = self.parse_expression(Precedence::Unary)?;
        Ok(Expr {
            id: self.new_id(),
            span: token.span.to(operand.span),
            kind: ExprKind::Unary {
                op,
                operand: Box::new(operand),
            },
        })
    }

    fn parse_grouped_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        let mut expr = self.parse_expression(Precedence::Lowest)?;
        let rparen = self.expect(TokenType::RParen)?;
        expr.span = start_span.to(rparen.span);
        Ok(expr)
    }

    fn parse_return_expr(&mut self, span: Span) -> ParseResult<Expr> {
        let mut val = None;
        let is_stopper = self.check(TokenType::Semicolon)
            || self.check(TokenType::RBrace)
            || self.check(TokenType::Else)
            || self.check(TokenType::RParen)
            || self.check(TokenType::RBracket)
            || self.check(TokenType::Comma)
            || self.check(TokenType::Eof);
        if !is_stopper {
            val = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
        }
        Ok(Expr {
            id: self.new_id(),
            span,
            kind: ExprKind::Return(val),
        })
    }

    fn parse_intrinsic_expr(&mut self, at_token: Token) -> ParseResult<Expr> {
        let id_token = self.expect(TokenType::Identifier)?;
        let sym = self.intern_token(id_token);
        let name_str = format!("@{}", self.session.resolve(sym));
        let sym_id = self.session.intern(&name_str);
        Ok(Expr {
            id: self.new_id(),
            span: at_token.span.to(id_token.span),
            kind: ExprKind::Identifier(sym_id),
        })
    }

    fn parse_typed_data_init_prefix(&mut self, start_token: Token) -> ParseResult<Expr> {
        let span = start_token.span;

        // 这里利用了 parse_type 的递归结构，但是需要为第一步的特殊 token 手动桥接
        let type_node = match start_token.tag {
            TokenType::LBracket => {
                if self.match_token(&[TokenType::RBracket]) {
                    // 解析切片: []mut T
                    let is_mut = self.match_token(&[TokenType::Mut]);
                    let elem = self.parse_type()?;
                    TypeNode {
                        id: self.new_id(),
                        span: span.to(elem.span),
                        kind: TypeKind::Slice {
                            is_mut,
                            elem: Box::new(elem),
                        },
                    }
                } else {
                    // 解析数组: [N]mut T
                    let len_expr = self.parse_expression(Precedence::Lowest)?;
                    self.expect(TokenType::RBracket)?;
                    let is_mut = self.match_token(&[TokenType::Mut]);
                    let elem = self.parse_type()?;
                    TypeNode {
                        id: self.new_id(),
                        span: span.to(elem.span),
                        kind: TypeKind::Array {
                            is_mut,
                            elem: Box::new(elem),
                            len: Box::new(len_expr),
                        },
                    }
                }
            }
            TokenType::Star => {
                // 解析指针: *mut T
                let is_mut = self.match_token(&[TokenType::Mut]);
                let elem = self.parse_type()?;
                TypeNode {
                    id: self.new_id(),
                    span: span.to(elem.span),
                    kind: TypeKind::Pointer {
                        is_mut,
                        elem: Box::new(elem),
                    },
                }
            }
            TokenType::Caret => {
                // 解析易失指针: ^mut T
                let is_mut = self.match_token(&[TokenType::Mut]);
                let elem = self.parse_type()?;
                TypeNode {
                    id: self.new_id(),
                    span: span.to(elem.span),
                    kind: TypeKind::VolatilePtr {
                        is_mut,
                        elem: Box::new(elem),
                    },
                }
            }
            _ => unreachable!(),
        };

        self.expect(TokenType::DotLBrace)?;
        self.parse_data_init(Some(Box::new(type_node)), span)
    }

    fn parse_closure_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        // 1. 解析捕获列表 `a, ptr = counter..&`
        let mut captures = Vec::new();
        if !self.check(TokenType::RBracket) {
            loop {
                let name_tok = self.expect(TokenType::Identifier)?;
                let name = self.intern_token(name_tok);
                
                // 解析显式绑定 (=) 或省略简写
                let value = if self.match_token(&[TokenType::Assign]) {
                    self.parse_expression(Precedence::Lowest)?
                } else {
                    // Elided shorthand: 只有名字时，它就是捕获自身
                    Expr {
                        id: self.new_id(),
                        span: name_tok.span,
                        kind: ExprKind::Identifier(name),
                    }
                };
                
                captures.push(CapturePattern {
                    name,
                    value,
                    span: name_tok.span.to(self.stream.prev_span()),
                });
                
                if !self.match_token(&[TokenType::Comma]) {
                    break;
                }
            }
        }
        self.expect(TokenType::RBracket)?;

        // 2. 解析参数列表 (复用原有的 params 解析)
        let (params, is_variadic) = self.parse_func_params()?;
        if is_variadic {
            self.add_error(start_span, "Closures cannot use C-style variadic arguments".to_string());
        }

        // 3. 解析闭包返回值类型
        let ret_type = self.parse_type()?;

        // 4. 解析闭包执行体
        let brace_token = self.expect(TokenType::LBrace)?;
        let body = self.parse_block_expr(brace_token.span)?;

        let end_span = body.span;

        Ok(Expr {
            id: self.new_id(),
            span: start_span.to(end_span),
            kind: ExprKind::Closure {
                captures,
                params,
                ret_type: Box::new(ret_type),
                body: Box::new(body),
            },
        })
    }

    // --- Infix Sub-Routines ---

    fn parse_binary_expr(&mut self, left: Expr, token: Token) -> ParseResult<Expr> {
        let op = BinaryOperator::from_token(token.tag);
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

    fn parse_field_access_expr(&mut self, left: Expr) -> ParseResult<Expr> {
        let field_token = self.expect(TokenType::Identifier)?;
        let field_id = self.intern_token(field_token);
        Ok(Expr {
            id: self.new_id(),
            span: left.span.to(field_token.span),
            kind: ExprKind::FieldAccess {
                lhs: Box::new(left),
                field: field_id,
            },
        })
    }

    fn parse_call_expr(&mut self, left: Expr) -> ParseResult<Expr> {
        let mut args = Vec::new();
        if !self.check(TokenType::RParen) {
            loop {
                args.push(self.parse_expression(Precedence::Lowest)?);
                if !self.match_token(&[TokenType::Comma]) {
                    break;
                }
            }
        }
        let end = self.expect(TokenType::RParen)?;
        Ok(Expr {
            id: self.new_id(),
            span: left.span.to(end.span),
            kind: ExprKind::Call {
                callee: Box::new(left),
                args,
            },
        })
    }

    fn parse_assignment_expr(&mut self, left: Expr, token: Token) -> ParseResult<Expr> {
        let op = AssignmentOperator::from_token(token.tag);
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

    fn parse_slice_or_index_expr(&mut self, left: Expr, is_mut: bool) -> ParseResult<Expr> {
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
            Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::SliceOp {
                    lhs: Box::new(left),
                    start,
                    end,
                    is_inclusive,
                    is_mut,
                },
            })
        } else {
            Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::IndexAccess {
                    lhs: Box::new(left),
                    index: start.unwrap(),
                    is_mut,
                },
            })
        }
    }

    fn parse_generic_instantiation_expr(&mut self, left: Expr) -> ParseResult<Expr> {
        let mut types = Vec::new();
        if !self.check(TokenType::RBracket) {
            loop {
                types.push(self.parse_type()?);
                if !self.match_token(&[TokenType::Comma]) {
                    break;
                }
                if self.check(TokenType::RBracket) {
                    break;
                }
            }
        }
        let rb = self.expect(TokenType::RBracket)?;
        Ok(Expr {
            id: self.new_id(),
            span: left.span.to(rb.span),
            kind: ExprKind::GenericInstantiation {
                target: Box::new(left),
                types,
            },
        })
    }

    // ==========================================
    //            Specific Expressions
    // ==========================================

    fn parse_data_init(
        &mut self,
        type_node: Option<Box<TypeNode>>,
        start_span: Span,
    ) -> ParseResult<Expr> {
        // 空数组/空结构体兜底
        if self.check(TokenType::RBrace) {
            let rb = self.advance();
            return Ok(Expr {
                id: self.new_id(),
                span: start_span.to(rb.span),
                kind: ExprKind::DataInit {
                    type_node,
                    literal: DataLiteralKind::Array(vec![]), // 空的统一先视为 Array，Sema 会根据 Context 调整
                },
            });
        }

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
                
                if let Err(_) = self.expect(TokenType::Colon) {
                    let name_str = self.session.resolve(name_id).to_string();
                    
                    self.session.struct_error(name.span, "explicit field names are required in struct/union initialization")
                        .with_hint(format!("Kern does not support elided fields. Write `{name_str}: {name_str}` instead."))
                        .emit();
                        
                    return Err(());
                }
                
                let val = self.parse_expression(Precedence::Lowest)?;
                
                fields.push(StructFieldInit {
                    name: name_id,
                    value: val,
                    span: name.span,
                });
                
                if !self.match_token(&[TokenType::Comma]) {
                    break;
                }
            }
            let rb = self.expect(TokenType::RBrace)?;
            Ok(Expr {
                id: self.new_id(),
                span: start_span.to(rb.span),
                kind: ExprKind::DataInit {
                    type_node,
                    literal: DataLiteralKind::Struct(fields),
                },
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
                    kind: ExprKind::DataInit {
                        type_node,
                        literal: DataLiteralKind::Repeat {
                            value: Box::new(first),
                            count: Box::new(count),
                        },
                    },
                })
            }
            // 模式 B: [Array] .{ 1, 2, 3 } (只要遇到了逗号，就一定是数组)
            else if self.match_token(&[TokenType::Comma]) {
                let mut elems = vec![first];
                while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
                    elems.push(self.parse_expression(Precedence::Lowest)?);
                    if !self.match_token(&[TokenType::Comma]) {
                        break;
                    }
                }
                let rb = self.expect(TokenType::RBrace)?;
                Ok(Expr {
                    id: self.new_id(),
                    span: start_span.to(rb.span),
                    kind: ExprKind::DataInit {
                        type_node,
                        literal: DataLiteralKind::Array(elems),
                    },
                })
            }
            // 模式 C: [Scalar] .{ 10 } 或者 Type.{ 1 << 12 }
            else {
                // 既没有逗号，也没有分号，那就是唯一的一个单值！直接包装为 Scalar！
                let rb = self.expect(TokenType::RBrace)?;
                Ok(Expr {
                    id: self.new_id(),
                    span: start_span.to(rb.span),
                    kind: ExprKind::DataInit {
                        type_node,
                        literal: DataLiteralKind::Scalar(Box::new(first)),
                    },
                })
            }
        }
    }

    pub fn parse_block_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        let mut stmts = Vec::new();
        let mut result = None;

        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let attributes = self.parse_attributes(false).unwrap_or_default(); // 拦截语句级属性
            if self.check(TokenType::Defer) {
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
                continue;
            }
            let expr = match self.parse_expression(Precedence::Lowest) {
                Ok(e) => e,
                Err(_) => {
                    // 如果块内的表达式解析失败，就在块内部同步，跳到下一个分号或右大括号
                    self.synchronize();
                    continue;
                }
            };

            // 使用 AST 提供的统一方法判断
            let is_block_like = expr.is_block_like();

            if self.match_token(&[TokenType::Semicolon]) {
                stmts.push(Stmt {
                    id: self.new_id(),
                    span: expr.span,
                    attributes,
                    kind: StmtKind::ExprStmt(expr),
                });
            } else if self.check(TokenType::RBrace) {
                // 如果紧跟着是 }，说明这是整个 Block 的返回值
                // 严禁在尾随返回值表达式上附加属性
                if !attributes.is_empty() {
                    self.add_error(attributes[0].span, "Attributes are not allowed on the trailing return expression of a block. Consider adding a semicolon to make it a statement.".to_string());
                }
                result = Some(Box::new(expr));
            } else if is_block_like {
                // 如果是块级表达式，没有分号也是合法的独立语句
                stmts.push(Stmt {
                    id: self.new_id(),
                    span: expr.span,
                    attributes,
                    kind: StmtKind::ExprStmt(expr),
                });
            } else {
                // 普通表达式必须以分号结尾
                self.error_at_current("Expected semicolon".to_string());
                stmts.push(Stmt {
                    id: self.new_id(),
                    span: expr.span,
                    attributes,
                    kind: StmtKind::ExprStmt(expr),
                });
            }
        }

        let rb = self.expect(TokenType::RBrace)?;
        let end_span = rb.span;
        Ok(Expr {
            id: self.new_id(),
            span: start_span.to(end_span),
            kind: ExprKind::Block { stmts, result },
        })
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

    fn parse_for_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        self.expect(TokenType::LParen)?;
        let mut init = None;
        if !self.check(TokenType::Semicolon) {
            init = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
        }
        self.expect(TokenType::Semicolon)?;

        let mut cond = None;
        if !self.check(TokenType::Semicolon) {
            cond = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
        }
        self.expect(TokenType::Semicolon)?;

        let mut post = None;
        if !self.check(TokenType::RParen) {
            post = Some(Box::new(self.parse_expression(Precedence::Lowest)?));
        }
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

    fn parse_match_body(&mut self) -> ParseResult<Expr> {
        if self.check(TokenType::LBrace) {
            let t = self.advance();
            self.parse_block_expr(t.span)
        } else {
            self.parse_expression(Precedence::Lowest)
        }
    }

    fn parse_match_expr(&mut self, start_span: Span) -> ParseResult<Expr> {
        self.expect(TokenType::LParen)?;
        let target = self.parse_expression(Precedence::Lowest)?;
        self.expect(TokenType::RParen)?;
        self.expect(TokenType::LBrace)?;

        let mut arms = Vec::new();

        while !self.check(TokenType::RBrace) && !self.check(TokenType::Eof) {
            let arm_start = self.peek().span;

            // 1. 兜底分支 (使用通配符 _)
            if self.match_token(&[TokenType::Underscore]) {
                self.expect(TokenType::Arrow)?;
                let body = self.parse_match_body()?;
                self.match_token(&[TokenType::Comma]);
                arms.push(MatchArm {
                    patterns: vec![MatchPattern {
                        kind: MatchPatternKind::CatchAll,
                        span: arm_start.to(self.stream.prev_span()),
                    }],
                    body,
                    span: arm_start.to(self.stream.prev_span()),
                });
                continue;
            }

            // 2. 解析由逗号分隔的多个 Pattern (如: 11, 12, .Ok: val)
            let mut patterns = Vec::new();
            loop {
                let pat_start = self.peek().span;

                // 语法 A: 简写变体匹配 `.Ok` 或 `.Ok: val`
                if self.match_token(&[TokenType::Dot]) {
                    let v_tok = self.expect(TokenType::Identifier)?;
                    let variant_name = self.intern_token(v_tok);

                    let mut binding = None;
                    if self.match_token(&[TokenType::Colon]) {
                        binding = Some(self.parse_binding_pattern()?);
                    }

                    patterns.push(MatchPattern {
                        kind: MatchPatternKind::Variant {
                            target_type: None,
                            variant_name,
                            binding,
                        },
                        span: pat_start.to(self.stream.prev_span()),
                    });
                }
                // 语法 B: 范围、值、或带显式前缀的变体匹配
                else {
                    let expr = self.parse_expression(Precedence::Lowest)?;

                    if self.match_token(&[TokenType::DotDot]) {
                        // 左闭右开范围: 1..10
                        let end_expr = self.parse_expression(Precedence::Lowest)?;
                        patterns.push(MatchPattern {
                            kind: MatchPatternKind::Range {
                                start: Box::new(expr),
                                end: Box::new(end_expr),
                                inclusive: false,
                            },
                            span: pat_start.to(self.stream.prev_span()),
                        });
                    } else if self.match_token(&[TokenType::DotDotEqual]) {
                        // 包含范围: 14..=15
                        let end_expr = self.parse_expression(Precedence::Lowest)?;
                        patterns.push(MatchPattern {
                            kind: MatchPatternKind::Range {
                                start: Box::new(expr),
                                end: Box::new(end_expr),
                                inclusive: true,
                            },
                            span: pat_start.to(self.stream.prev_span()),
                        });
                    } else if self.match_token(&[TokenType::Colon]) {
                        // 带有载荷解包的完全限定变体: `Result[i32].Ok : mut val`
                        let binding = Some(self.parse_binding_pattern()?);

                        // 利用你在 decl.rs 里写好的极其牛逼的 expr_to_type 方法
                        let ty = self.expr_to_type(expr)?;
                        let (target_type, variant_name) = self.extract_variant_from_type(ty)?;

                        patterns.push(MatchPattern {
                            kind: MatchPatternKind::Variant {
                                target_type: Some(Box::new(target_type)),
                                variant_name,
                                binding,
                            },
                            span: pat_start.to(self.stream.prev_span()),
                        });
                    } else {
                        // 普通值匹配 (或者像 `Result.None` 这种无载荷的变体，统一作为普通值推给语义分析处理)
                        patterns.push(MatchPattern {
                            kind: MatchPatternKind::Value(Box::new(expr.clone())),
                            span: expr.span,
                        });
                    }
                }

                if !self.match_token(&[TokenType::Comma]) {
                    break;
                }
            }

            self.expect(TokenType::Arrow)?;
            let body = self.parse_match_body()?;
            self.match_token(&[TokenType::Comma]); // 可选的末尾逗号

            arms.push(MatchArm {
                patterns,
                span: arm_start.to(body.span),
                body,
            });
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

    fn extract_variant_from_type(&mut self, ty: TypeNode) -> ParseResult<(TypeNode, SymbolId)> {
        if let TypeKind::Path {
            mut segments,
            generics,
        } = ty.kind
        {
            if segments.len() >= 2 {
                let variant_name = segments.pop().unwrap(); // 弹出最后一段作为 Variant 名
                let remain_ty = TypeNode {
                    id: self.new_id(),
                    span: ty.span, // 作为内部生成的节点，借用原本的 span
                    kind: TypeKind::Path { segments, generics }, // 泛型约束保留在主体上
                };
                return Ok((remain_ty, variant_name));
            }
        }
        self.add_error(
            ty.span,
            "Expected a valid ADT variant path before ':'".to_string(),
        );
        Err(())
    }

    fn parse_decl_expr(&mut self, start_token: Token) -> ParseResult<Expr> {
        let tag = start_token.tag;

        // 直接解析绑定模式 (支持 `let mut a` 或 `let a`)
        let pattern = self.parse_binding_pattern()?;

        if self.match_token(&[TokenType::Colon]) {
            let err_span = self.stream.prev_span();
            let _ = self.parse_type(); // 吃掉类型防报错

            self.session.struct_error(err_span, "type annotations on the left side of declarations are strictly forbidden in Kern")
                .with_hint("Kern uses explicit constructor syntax on the right side")
                .with_hint("try rewriting this as: `let [mut] name = Type.{ ... };`")
                .emit();
            self.panic_mode = true;
        }

        self.expect(TokenType::Assign)?;
        let init = self.parse_expression(Precedence::Lowest)?;
        let span = start_token.span.to(init.span);

        match tag {
            TokenType::Static => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::Static {
                    pattern,
                    init: Box::new(init),
                },
            }),
            TokenType::Let | TokenType::Const => Ok(Expr {
                id: self.new_id(),
                span,
                kind: ExprKind::Let {
                    pattern,
                    init: Box::new(init),
                },
            }),
            _ => unreachable!(),
        }
    }
}

impl<'a> Parser<'a> {
    pub fn parse_string_literal(&mut self, token: Token) -> ParseResult<SymbolId> {
        let raw = self
            .session
            .source_manager
            .slice_source(token.span)
            .to_string();

        // 1. 检查并去掉引号
        if raw.len() < 2 || !raw.starts_with('"') || !raw.ends_with('"') {
            self.session
                .struct_error(token.span, "invalid or unterminated string literal")
                .with_hint("ensure the string is properly enclosed in double quotes `\"`")
                .emit();
            return Err(());
        }

        let inner = &raw[1..raw.len() - 1];

        // 2. 转义处理
        let unescaped = self.unescape_string(inner, token.span)?;

        // 3. Intern
        Ok(self.session.intern(&unescaped))
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
                            self.add_error(
                                span,
                                format!("Invalid unicode scalar value: {:x}", code_point),
                            );
                            return Err(());
                        }
                    }

                    Some(c) => {
                        self.session
                            .struct_error(span, format!("unknown escape sequence: `\\{}`", c))
                            .with_hint(format!("if you meant to write a backslash, use `\\\\`"))
                            .emit();
                        self.panic_mode = true;
                        return Err(());
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
}
