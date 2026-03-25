use super::Expr;
use kernc_utils::{NodeId, Span, SymbolId};

#[derive(Debug, Clone, PartialEq)]
pub struct TypeNode {
    pub id: NodeId,
    pub span: Span,
    pub kind: TypeKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeKind {
    /// 路径类型引用: `i32`, `std.io.File`, `Map[K, V]`
    Path {
        segments: Vec<SymbolId>,
        generics: Vec<TypeNode>,
    },

    /// 指针: `*T`, `*mut T`
    Pointer {
        is_mut: bool,
        elem: Box<TypeNode>,
    },

    /// 易失指针: `^T`, `^mut T`
    VolatilePtr {
        is_mut: bool,
        elem: Box<TypeNode>,
    },

    /// 数组: `[N]T`
    Array {
        is_mut: bool,
        elem: Box<TypeNode>,
        len: Box<Expr>, // 必须是常量表达式
    },

    // 长度推导数组: `[_]T` (用于 .{ 1, 2, 3 } 赋值时的类型推导)
    ArrayInfer {
        is_mut: bool,
        elem: Box<TypeNode>,
    },

    /// 切片: `[]T`
    Slice {
        is_mut: bool,
        elem: Box<TypeNode>,
    },

    /// 函数指针类型: `fn(i32) bool`
    Function {
        params: Vec<TypeNode>,
        ret: Option<Box<TypeNode>>,
        is_variadic: bool,
    },

    // === 匿名/内联类型定义 (Structural Types) ===
    /// 结构体定义
    Struct {
        fields: Vec<StructFieldDef>,
    },

    /// 联合体定义
    Union {
        fields: Vec<StructFieldDef>,
    },

    /// 代数数据类型
    /// type Result[T]: u8 = enum { Ok: T, Err, None = 0xFF }
    Enum {
        backing_type: Option<Box<TypeNode>>,
        variants: Vec<EnumVariant>,
    },

    /// 特征定义 (Trait)
    Trait {
        fields: Vec<StructFieldDef>,
    },

    /// 推导占位符 `_`
    Infer,

    /// 代表当前 impl 块的目标类型
    SelfType,

    /// Never 类型 `!`，代表永远不会返回的控制流
    Never,

    /// 编译期类型求值 `@typeOf(expr)`
    TypeOf(Box<Expr>),

    /// 闭包动态胖指针接口: `Fn(i32) bool`
    ClosureInterface {
        params: Vec<TypeNode>,
        ret: Option<Box<TypeNode>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructFieldDef {
    pub name: SymbolId,
    pub type_node: TypeNode,
    pub default_value: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariant {
    pub name: SymbolId,
    /// 负载类型。例如 `Ok: i32`
    pub payload_type: Option<Box<TypeNode>>,
    /// 显式赋值鉴别器。例如 `Red = 0`
    pub value: Option<Box<Expr>>,
    pub span: Span,
}
