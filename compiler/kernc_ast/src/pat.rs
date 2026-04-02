use super::{Expr, TypeNode};
use kernc_utils::{Span, SymbolId};

/// 局部绑定模式，处理类似 `mut a` 或 `a` 的逻辑
#[derive(Debug, Clone, PartialEq)]
pub struct BindingPattern {
    pub name: SymbolId,
    pub name_span: Span,
    pub is_mut: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VariantPattern {
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
    Value(Box<Expr>),
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        inclusive: bool,
    },
    Pattern(Pattern),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchPattern {
    pub kind: MatchPatternKind,
    pub span: Span,
}
