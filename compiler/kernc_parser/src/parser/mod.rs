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

    fn parse_doc_block(&mut self, expect_inner: bool) -> Option<kernc_ast::DocBlock> {
        let expected = if expect_inner {
            TokenType::DocCommentInner
        } else {
            TokenType::DocCommentOuter
        };

        if !self.check(expected) {
            return None;
        }

        let mut lines = Vec::new();
        let mut span = Span::default();
        while self.check(expected) {
            let token = self.advance();
            let text = self.doc_text_for_token(token, expect_inner);
            span = if lines.is_empty() {
                token.span
            } else {
                span.to(token.span)
            };
            lines.push(kernc_ast::DocLine {
                span: token.span,
                text,
            });
        }

        Some(kernc_ast::DocBlock { span, lines })
    }

    fn append_doc_block(docs: &mut Option<kernc_ast::DocBlock>, mut block: kernc_ast::DocBlock) {
        if let Some(existing) = docs {
            existing.span = existing.span.to(block.span);
            existing.lines.append(&mut block.lines);
        } else {
            *docs = Some(block);
        }
    }

    fn parse_module_leading_meta(
        &mut self,
    ) -> (Option<kernc_ast::DocBlock>, Vec<kernc_ast::Attribute>) {
        let mut docs = None;
        let mut attributes = Vec::new();

        loop {
            if self.check(TokenType::DocCommentInner) {
                if let Some(block) = self.parse_doc_block(true) {
                    Self::append_doc_block(&mut docs, block);
                }
                continue;
            }

            if self.is_at_attribute_with_level(true) {
                attributes.extend(self.parse_attributes(true).unwrap_or_default());
                continue;
            }

            break;
        }

        (docs, attributes)
    }

    fn parse_item_leading_meta(
        &mut self,
        item_kind: &str,
    ) -> (Option<kernc_ast::DocBlock>, Vec<kernc_ast::Attribute>) {
        let mut docs = None;
        let mut attributes = Vec::new();

        loop {
            if self.check(TokenType::DocCommentOuter) {
                if let Some(block) = self.parse_doc_block(false) {
                    Self::append_doc_block(&mut docs, block);
                }
                continue;
            }

            if self.is_at_attribute_with_level(false) {
                attributes.extend(self.parse_attributes(false).unwrap_or_default());
                continue;
            }

            if self.check(TokenType::DocCommentInner) {
                let span = self.peek().span;
                self.session
                    .struct_error(
                        span,
                        "inner doc comments (`//!`) are only allowed at module scope",
                    )
                    .with_hint(format!("use `///` to document this {item_kind}"))
                    .emit();
                let _ = self.parse_doc_block(true);
                continue;
            }

            break;
        }

        (docs, attributes)
    }

    fn parse_item_doc_block(&mut self, item_kind: &str) -> Option<kernc_ast::DocBlock> {
        let mut docs = None;

        loop {
            if self.check(TokenType::DocCommentOuter) {
                if let Some(block) = self.parse_doc_block(false) {
                    Self::append_doc_block(&mut docs, block);
                }
                continue;
            }

            if self.check(TokenType::DocCommentInner) {
                let span = self.peek().span;
                self.session
                    .struct_error(
                        span,
                        "inner doc comments (`//!`) are only allowed at module scope",
                    )
                    .with_hint(format!("use `///` to document this {item_kind}"))
                    .emit();
                let _ = self.parse_doc_block(true);
                continue;
            }

            break;
        }

        docs
    }

    fn emit_dangling_doc_error(&mut self, docs: &kernc_ast::DocBlock, expected_item: &str) {
        self.session
            .struct_error(
                docs.span,
                format!("doc comments must document a following {expected_item}"),
            )
            .with_hint(format!(
                "place the doc block directly above the {expected_item} it describes"
            ))
            .emit();
    }

    fn doc_text_for_token(&self, token: Token, is_inner: bool) -> String {
        let source = self.session.source_manager.slice_source(token.span);
        let prefix = if is_inner { "//!" } else { "///" };
        let mut text = source.strip_prefix(prefix).unwrap_or(source);
        if let Some(rest) = text.strip_prefix(' ') {
            text = rest;
        }
        text.to_string()
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
