//! # kernc_lexer
//!
//! Lexer support for the Kern compiler frontend.

/// Token data structures and token kinds.
mod token;

/// Source-to-token conversion.
mod tokenizer;

pub use token::{Lexeme, LexemeType, Token, TokenType};
pub use tokenizer::Tokenizer;
