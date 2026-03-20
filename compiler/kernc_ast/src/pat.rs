use super::{Expr, TypeNode};
use kernc_utils::{Span, SymbolId};

/// 局部绑定模式，处理类似 `mut a` 或 `a` 的逻辑
#[derive(Debug, Clone, PartialEq)]
pub struct BindingPattern {
    pub name: SymbolId,
    pub is_mut: bool,
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
    Variant {
        target_type: Option<Box<TypeNode>>,
        variant_name: SymbolId,
        binding: Option<BindingPattern>,
    },
    CatchAll,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchPattern {
    pub kind: MatchPatternKind,
    pub span: Span,
}