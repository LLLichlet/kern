use super::Expr;
use kernc_utils::{Span, SymbolId};

#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    pub span: Span,
    pub is_module_level: bool, // 区分 #![...] 和 #[...]
    pub kind: AttributeKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AttributeKind {
    /// 条件编译: `#[if(os == "linux" and arch == "x86")]`
    /// 括号内部直接是一个标准的 Expr 树
    If(Box<Expr>),

    /// 元数据集合: `#[cold, export_name("NtCreateFile")]`
    Meta(Vec<MetaItem>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MetaItem {
    /// 仅标记，如 `cold`, `packed`
    Marker(SymbolId),

    /// 传参标记，如 `export_name("foo")`, `align(4)`
    /// 这里统一把括号内的东西当作 Expr 处理，Sema 阶段再校验它是不是字符串或整数
    Call(SymbolId, Box<Expr>),
}
