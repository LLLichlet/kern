use super::MastExpr;
use kernc_sema::ty::TypeId;
use kernc_utils::SymbolId;

#[derive(Debug, Clone)]
pub struct MastBlock {
    pub stmts: Vec<MastStmt>,
    pub result: Option<Box<MastExpr>>, // 块的返回值
    pub defers: Vec<MastExpr>,
}

#[derive(Debug, Clone)]
pub enum MastStmt {
    /// 局部变量绑定 (注意：局部 static 不在这里，已被提升为 MastGlobal)
    Let {
        name: SymbolId,
        ty: TypeId,
        is_mut: bool,
        init: MastExpr,
    },
    /// 表达式语句
    Expr(MastExpr),
    // 在 Lowering 阶段，所有的 defer 都已经被
    // 倒序强行插入到了此 Block 的每一个返回/退出路径上。
}
