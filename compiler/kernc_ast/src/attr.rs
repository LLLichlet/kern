//! Attribute syntax attached to modules, declarations, and statements.
//!
//! Attribute payloads remain expression-like at this layer.  Semantic analysis
//! decides which marker names and argument forms are valid for each attribute.

use super::Expr;
use kernc_utils::{Span, SymbolId};

#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    pub span: Span,
    /// Distinguishes `#![...]` from `#[...]`.
    pub is_module_level: bool,
    pub kind: AttributeKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AttributeKind {
    /// Conditional compilation such as `#[if(os == "linux" and arch == "x86")]`.
    /// The payload is stored as a regular expression AST.
    If(Box<Expr>),

    /// Metadata items such as `#[cold, export_name("NtCreateFile")]`.
    Meta(Vec<MetaItem>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MetaItem {
    /// Marker-only metadata such as `cold` or `packed`.
    Marker(SymbolId),

    /// Metadata with an argument such as `export_name("foo")` or `align(4)`.
    /// The parser keeps the payload as an expression and semantic analysis
    /// validates the expected literal form later.
    Call(SymbolId, Box<Expr>),
}
