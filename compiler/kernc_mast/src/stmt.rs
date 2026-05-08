use super::MastExpr;
use kernc_ty::TypeId;
use kernc_utils::SymbolId;

#[derive(Debug, Clone)]
pub struct MastBlock {
    pub stmts: Vec<MastStmt>,
    /// Optional trailing value produced by the block.
    pub result: Option<Box<MastExpr>>,
    pub defers: Vec<MastExpr>,
}

#[derive(Debug, Clone)]
pub enum MastStmt {
    /// Local variable binding. Lowered local statics live in `MastGlobal`.
    Let {
        name: SymbolId,
        ty: TypeId,
        is_mut: bool,
        init: MastExpr,
    },
    /// Expression statement.
    Expr(MastExpr),
    // During lowering, defers are expanded in reverse order onto every exit path.
}
