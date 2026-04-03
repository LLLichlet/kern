mod attr;
mod decl;
mod expr;
mod ty;

use super::TokenStream;
use kernc_lexer::{Token, TokenType, Tokenizer};
use kernc_utils::{DiagnosticLevel, FileId, NodeId, Session, Span, SymbolId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError;

pub type ParseResult<T> = Result<T, ParseError>;

pub struct Parser<'a> {
    stream: TokenStream<'a>,
    session: &'a mut Session,
    // Parser-level error recovery state.
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

    /// Consume one token and report a synchronized parse error on mismatch.
    fn expect(&mut self, tag: TokenType) -> ParseResult<Token> {
        if self.check(tag) {
            Ok(self.advance())
        } else {
            let current = self.peek();
            let found_text = self.describe_token(current);

            let mut diag = self.session.struct_error(
                current.span,
                format!("expected `{:?}`, found `{}`", tag, found_text),
            );

            // Attach targeted recovery hints for common delimiter mistakes.
            match tag {
                TokenType::Semicolon => diag = diag.with_hint("consider adding a `;` here"),
                TokenType::RBrace => diag = diag.with_hint("unclosed block"),
                TokenType::RParen => diag = diag.with_hint("unclosed parenthesis"),
                TokenType::RBracket => diag = diag.with_hint("unclosed bracket"),
                _ => {}
            }

            diag.emit();
            self.panic_mode = true;
            Err(ParseError)
        }
    }

    fn intern_token(&mut self, token: Token) -> SymbolId {
        let text = self.session.source_manager.slice_source(token.span);
        self.session.interner.intern(text)
    }

    fn describe_token(&self, token: Token) -> String {
        match token.tag {
            TokenType::Eof => "end of file".to_string(),
            TokenType::Illegal => "illegal token".to_string(),
            TokenType::LexError(msg) => format!("lexical error ({})", msg),
            _ => {
                let text = self
                    .session
                    .source_manager
                    .slice_source(token.span)
                    .to_string();
                if text.is_empty() {
                    format!("{:?}", token.tag)
                } else {
                    text
                }
            }
        }
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
            // A semicolon usually marks the end of the previous statement.
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
