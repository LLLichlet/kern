mod attr;
mod decl;
mod expr;
mod ty;

use super::TokenStream;
use kernc_lexer::{Token, TokenType, Tokenizer};
use kernc_utils::{DiagnosticLevel, FileId, NodeId, Session, Span, SymbolId};

pub type ParseResult<T> = Result<T, ()>;

pub struct Parser<'a> {
    stream: TokenStream<'a>,
    session: &'a mut Session,
    // 状态标记
    panic_mode: bool,
}

impl<'a> Parser<'a> {
    pub fn new(source: &'a str, file_id: FileId, session: &'a mut Session) -> Self {
        let tokenizer = Tokenizer::new(source, file_id);
        let stream = TokenStream::new(tokenizer);
        Self {
            stream,
            session,
            panic_mode: false,
        }
    }

    // ==========================================
    // Core Tools: AST Node Creation
    // ==========================================

    fn new_id(&mut self) -> NodeId {
        self.session.next_node_id()
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
            let found_text = self
                .session
                .source_manager
                .slice_source(current.span)
                .to_string();

            let mut diag = self.session.struct_error(
                current.span,
                format!("expected `{:?}`, found `{}`", tag, found_text),
            );

            // 针对特定的缺失提供智能提示
            match tag {
                TokenType::Semicolon => diag = diag.with_hint("consider adding a `;` here"),
                TokenType::RBrace => diag = diag.with_hint("unclosed block"),
                TokenType::RParen => diag = diag.with_hint("unclosed parenthesis"),
                _ => {}
            }

            diag.emit();
            self.panic_mode = true;
            Err(())
        }
    }

    fn intern_token(&mut self, token: Token) -> SymbolId {
        let text = self.session.source_manager.slice_source(token.span);
        self.session.interner.intern(text)
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
        self.session.report(span, DiagnosticLevel::Error, msg);
    }

    pub fn synchronize(&mut self) {
        self.panic_mode = false;
        if !self.check(TokenType::Eof) {
            self.advance();
        }

        while !self.check(TokenType::Eof) {
            // 如果碰到了分号，很可能上一个语句结束了
            if self.stream.peek_nth(0).tag == TokenType::Semicolon {
                self.advance();
                return;
            }

            match self.peek().tag {
                TokenType::Fn
                | TokenType::Let
                | TokenType::Const
                | TokenType::Static
                | TokenType::Type
                | TokenType::Pub
                | TokenType::Struct
                | TokenType::Enum
                | TokenType::If
                | TokenType::Match
                | TokenType::For
                | TokenType::Return => return,
                _ => {
                    self.advance();
                }
            }
        }
    }
}
