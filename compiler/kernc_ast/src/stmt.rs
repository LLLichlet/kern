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
    /// 表达式语句: `expr;`
    ExprStmt(Expr),

    /// 块末尾的表达式: `expr`
    ExprValue(Expr),
}
