use super::{Attribute, Expr, UsePathKind, UseTarget};
use kernc_utils::{NodeId, Span, SymbolId};

#[derive(Debug, Clone, PartialEq)]
pub struct UseStmt {
    pub kind: UsePathKind,
    pub path: Vec<SymbolId>,
    pub target: UseTarget,
    pub binding_span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub id: NodeId,
    pub span: Span,
    pub attributes: Vec<Attribute>,
    pub kind: StmtKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    /// Local import statement: `use path;`
    Use(UseStmt),

    /// Expression statement: `expr;`
    ExprStmt(Expr),

    /// Trailing block expression: `expr`
    ExprValue(Expr),
}
