use super::{Attribute, Expr};
use kernc_utils::{NodeId, Span};

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub id: NodeId,
    pub span: Span,
    pub attributes: Vec<Attribute>,
    pub kind: StmtKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    /// Expression statement: `expr;`
    ExprStmt(Expr),

    /// Trailing block expression: `expr`
    ExprValue(Expr),
}
