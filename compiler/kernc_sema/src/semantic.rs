//! Public semantic symbol summaries for tooling.
//!
//! These records are intentionally smaller than `Def` and `SymbolInfo`: LSP and
//! analysis clients need stable spans, mutability, visibility, and a broad kind
//! classification without depending on the full semantic graph.

use kernc_utils::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SemanticSymbolKind {
    Module,
    Namespace,
    Struct,
    Union,
    Enum,
    Trait,
    TypeAlias,
    TypeParameter,
    Function,
    Method,
    Constant,
    Static,
    Variable,
    Parameter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SemanticDefinition {
    pub span: Span,
    pub kind: SemanticSymbolKind,
    pub is_mut: bool,
    pub is_pub: bool,
}
