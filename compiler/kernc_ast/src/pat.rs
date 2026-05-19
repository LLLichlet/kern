//! Pattern syntax used by `let`, `match`, parameters, and destructuring forms.
//!
//! Pattern nodes intentionally preserve whether the parser saw a binding,
//! ignore marker, enum-like variant, or field destructure.  Exhaustiveness and
//! irrefutability checks operate on this syntax before MIR lowering rewrites it
//! into control flow.

use super::{Expr, TypeNode};
use kernc_utils::{Span, SymbolId};

/// Local binding pattern such as `mut a` or `a`.
#[derive(Debug, Clone, PartialEq)]
pub struct BindingPattern {
    pub name: SymbolId,
    pub name_span: Span,
    pub is_mut: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VariantPattern {
    /// Optional explicit type prefix for disambiguating contextual variants.
    pub target_type: Option<Box<TypeNode>>,
    pub variant_name: SymbolId,
    pub variant_span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DestructurePatternField {
    pub name: SymbolId,
    pub name_span: Span,
    pub pattern: Box<Pattern>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DestructurePattern {
    pub target_type: Option<Box<TypeNode>>,
    pub fields: Vec<DestructurePatternField>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternKind {
    Binding(BindingPattern),
    Ignore,
    Variant(VariantPattern),
    Destructure(DestructurePattern),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LetPattern {
    pub pattern: Pattern,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchPatternKind {
    /// Expression-valued pattern such as a literal or constant path.
    Value(Box<Expr>),
    /// Structural pattern such as `_`, `x`, `.Some(x)`, or `Point.{x}`.
    Pattern(Pattern),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchPattern {
    pub kind: MatchPatternKind,
    pub span: Span,
}
