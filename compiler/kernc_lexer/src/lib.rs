//! # kernc_lexer
//! 
//! kernc 的 lexer 模块

/// Token 的存在形式和类型
mod token;

/// 从字符串到 Token
mod tokenizer;

pub use token::{Token, TokenType};
pub use tokenizer::Tokenizer;
