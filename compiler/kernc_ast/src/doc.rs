//! Normalized documentation comments collected by the parser.
//!
//! The lexer exposes doc comments as tokens; parser attachment strips the
//! comment marker and preserves per-line spans so generated docs and editor
//! hovers can still point back to the original source.

use kernc_utils::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocLine {
    pub span: Span,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocBlock {
    pub span: Span,
    pub lines: Vec<DocLine>,
}
