use kernc_lexer::{Token, TokenType, Tokenizer};
use kernc_utils::Span;

pub struct TokenStream<'a> {
    lexer: Tokenizer<'a>,
    /// Buffered tokens that support arbitrary lookahead.
    buffer: Vec<Token>,
    buffer_start: usize,
    /// Span of the most recently consumed token, used for diagnostics.
    last_span: Span,
}

impl<'a> TokenStream<'a> {
    pub fn new(lexer: Tokenizer<'a>) -> Self {
        Self {
            lexer,
            buffer: Vec::new(),
            buffer_start: 0,
            last_span: Span::default(),
        }
    }

    /// Fill the buffer until it contains at least `n + 1` items or EOF is reached.
    fn fill_buffer(&mut self, n: usize) {
        while self.buffer.len().saturating_sub(self.buffer_start) <= n {
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
        self.fill_buffer(n);

        // If the requested index is past the buffered tail, return EOF.
        let buffered_len = self.buffer.len().saturating_sub(self.buffer_start);
        if n >= buffered_len {
            return self.buffer.last().copied().unwrap_or({
                // This should be unreachable unless buffer management regressed.
                Token {
                    tag: TokenType::Eof,
                    span: self.last_span,
                }
            });
        }

        self.buffer[self.buffer_start + n]
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
        // Fast path for the common case with no buffered lookahead.
        if self.buffer_start >= self.buffer.len() {
            self.buffer.clear();
            self.buffer_start = 0;
            let t = self.lexer.next_token();
            self.last_span = t.span;
            return t;
        }

        // Pop the front of the lookahead buffer.
        let token = self
            .buffer
            .get(self.buffer_start)
            .copied()
            .unwrap_or(Token {
                tag: TokenType::Eof,
                span: self.last_span,
            });
        self.buffer_start += 1;
        if self.buffer_start >= 64 && self.buffer_start * 2 >= self.buffer.len() {
            self.buffer.drain(..self.buffer_start);
            self.buffer_start = 0;
        }
        self.last_span = token.span;
        token
    }

    /// Check whether the current token has the requested type.
    pub fn check(&mut self, tag: TokenType) -> bool {
        self.peek().tag == tag
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
        self.peek().tag == TokenType::Eof
    }
}
