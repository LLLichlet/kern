//! Token stream with current-token caching and bounded lookahead.
//!
//! Parser code frequently needs one or two tokens of lookahead, but a few
//! grammar decisions need more.  `TokenStream` keeps the current token separate
//! from the lookahead buffer and compacts consumed buffer entries so long
//! speculative parses do not grow memory indefinitely.

use kernc_lexer::{Token, TokenType, Tokenizer};
use kernc_utils::Span;

#[derive(Clone)]
pub struct TokenStream<'a> {
    lexer: Tokenizer<'a>,
    /// The current token is cached separately so normal peek/bump traffic
    /// avoids pushing every token through the general lookahead buffer.
    current: Option<Token>,
    /// Buffered lookahead tokens beyond `current`.
    buffer: Vec<Token>,
    buffer_start: usize,
    /// Span of the most recently consumed token, used for diagnostics.
    last_span: Span,
}

impl<'a> TokenStream<'a> {
    pub fn new(lexer: Tokenizer<'a>) -> Self {
        Self {
            lexer,
            current: None,
            buffer: Vec::new(),
            buffer_start: 0,
            last_span: Span::default(),
        }
    }

    fn ensure_current(&mut self) -> Token {
        if let Some(token) = self.current {
            return token;
        }

        let token = if self.buffer_start < self.buffer.len() {
            // Promote buffered lookahead into the current slot when previous
            // parser code peeked ahead before consuming.
            let token = self.buffer[self.buffer_start];
            self.buffer_start += 1;
            if self.buffer_start >= 64 && self.buffer_start * 2 >= self.buffer.len() {
                self.compact_buffer();
            }
            token
        } else {
            self.buffer.clear();
            self.buffer_start = 0;
            self.lexer.next_token()
        };

        self.current = Some(token);
        token
    }

    /// Fill the lookahead buffer until it contains at least `n` items after
    /// `current`, or EOF is reached.
    fn fill_buffer(&mut self, n: usize) {
        if self.ensure_current().tag == TokenType::Eof {
            return;
        }

        while self.buffer.len().saturating_sub(self.buffer_start) < n {
            let token = self.lexer.next_token();
            let is_eof = token.tag == TokenType::Eof;
            self.buffer.push(token);

            // Once EOF is buffered, all later lookups reuse the same sentinel token.
            if is_eof {
                break;
            }
        }
    }

    /// Peek the `n`th token without consuming it.
    /// `n = 0` is the current token, `n = 1` is the next token, and so on.
    pub fn peek_nth(&mut self, n: usize) -> Token {
        let current = self.ensure_current();
        if n == 0 {
            return current;
        }

        self.fill_buffer(n);

        // If the requested index is past the buffered tail, return EOF.
        let buffered_len = self.buffer.len().saturating_sub(self.buffer_start);
        if n > buffered_len {
            return self.buffer.last().copied().unwrap_or(current);
        }

        self.buffer[self.buffer_start + (n - 1)]
    }

    /// Peek only the tag of the `n`th token without copying the full token payload.
    pub fn peek_tag_nth(&mut self, n: usize) -> TokenType {
        let current = self.ensure_current();
        if n == 0 {
            return current.tag;
        }

        self.fill_buffer(n);

        let buffered_len = self.buffer.len().saturating_sub(self.buffer_start);
        if n > buffered_len {
            return self
                .buffer
                .last()
                .map(|token| token.tag)
                .unwrap_or(current.tag);
        }

        self.buffer[self.buffer_start + (n - 1)].tag
    }

    /// Peek the current token.
    pub fn peek(&mut self) -> Token {
        self.peek_nth(0)
    }

    /// Peek the next token.
    pub fn peek_next(&mut self) -> Token {
        self.peek_nth(1)
    }

    /// Consume and return the current token.
    pub fn bump(&mut self) -> Token {
        let token = self.ensure_current();

        if token.tag == TokenType::Eof {
            // EOF is sticky: consuming it repeatedly should keep diagnostics at
            // the end-of-file span instead of pulling more lexer tokens.
            self.last_span = token.span;
            return token;
        }

        if self.buffer_start < self.buffer.len() {
            self.current = Some(self.buffer[self.buffer_start]);
            self.buffer_start += 1;
            if self.buffer_start >= 64 && self.buffer_start * 2 >= self.buffer.len() {
                self.compact_buffer();
            }
        } else {
            self.current = None;
            self.buffer.clear();
            self.buffer_start = 0;
        }

        self.last_span = token.span;
        token
    }

    /// Check whether the current token has the requested type.
    pub fn check(&mut self, tag: TokenType) -> bool {
        self.peek_tag_nth(0) == tag
    }

    /// Consume the current token if it matches `tag`.
    pub fn match_token(&mut self, tag: TokenType) -> bool {
        if self.check(tag) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Try to consume one token of the requested kind.
    pub fn eat(&mut self, tag: TokenType) -> Option<Token> {
        if self.check(tag) {
            Some(self.bump())
        } else {
            None
        }
    }

    /// Return the span of the most recently consumed token.
    pub fn prev_span(&self) -> Span {
        self.last_span
    }

    /// Return whether the stream has reached EOF.
    pub fn is_eof(&mut self) -> bool {
        self.peek_tag_nth(0) == TokenType::Eof
    }

    fn compact_buffer(&mut self) {
        let remaining = self.buffer.len().saturating_sub(self.buffer_start);
        if remaining == 0 {
            self.buffer.clear();
            self.buffer_start = 0;
            return;
        }

        // `Vec::drain` would move and drop every consumed token.  `copy_within`
        // is cheaper for this Copy token buffer and keeps allocation capacity.
        self.buffer.copy_within(self.buffer_start.., 0);
        self.buffer.truncate(remaining);
        self.buffer_start = 0;
    }
}
