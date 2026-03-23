use super::{
    AssignmentOperator, BinaryOperator, BindingPattern, FuncParam, MatchPattern, Stmt, TypeNode,
    UnaryOperator,
};
use kernc_utils::{NodeId, Span, SymbolId};

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub id: NodeId,
    pub span: Span,
    pub kind: ExprKind,
}

impl Expr {
    /// 判断该表达式在视觉上是否自带块级闭合边界 (即以 `}` 结尾)
    pub fn is_block_like(&self) -> bool {
        matches!(
            self.kind,
            ExprKind::If { .. }
                | ExprKind::Match { .. }
                | ExprKind::For { .. }
                | ExprKind::Block { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// `let mut x = v` 或 `let x = v`
    Let {
        pattern: BindingPattern,
        init: Box<Expr>,
    },

    /// `static x = v`
    Static {
        pattern: BindingPattern,
        init: Box<Expr>,
    },

    // --- Literals ---
    Integer(u128),
    Float(f64),
    Bool(bool),
    Char(char),
    ByteChar(u8),
    String(String),
    Identifier(SymbolId),

    // --- Ops ---
    Binary {
        lhs: Box<Expr>,
        op: BinaryOperator,
        rhs: Box<Expr>,
    },
    Unary {
        op: UnaryOperator,
        operand: Box<Expr>,
    },

    // --- Access ---
    FieldAccess {
        lhs: Box<Expr>,
        field: SymbolId,
    },
    IndexAccess {
        lhs: Box<Expr>,
        index: Box<Expr>,
        is_mut: bool,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    // --- Constructors ---
    /// 统一的数据字面量初始化，支持 Type.{ ... } 和 .{ ... }
    DataInit {
        /// 如果是 `.{ ... }`，此字段为 None；
        /// 如果是 `Point.{ ... }`，此字段就是 `Point` 对应的 TypeNode。
        type_node: Option<Box<TypeNode>>,
        /// 具体的数据负载
        literal: DataLiteralKind,
    },

    /// 枚举/上下文简写: .Red, .Ok
    EnumLiteral(SymbolId),

    // --- Control Flow ---
    If {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Option<Box<Expr>>,
    },
    Match {
        target: Box<Expr>,
        arms: Vec<MatchArm>,
    },

    /// 块表达式: `{ stmt; stmt; expr }`
    Block {
        stmts: Vec<Stmt>,
        result: Option<Box<Expr>>,
    },

    /// `for (init; cond; post) body`
    For {
        init: Option<Box<Expr>>,
        cond: Option<Box<Expr>>,
        post: Option<Box<Expr>>,
        body: Box<Expr>,
    },

    /// 切片构造: arr.[start..end]
    SliceOp {
        lhs: Box<Expr>,
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        is_inclusive: bool,
        is_mut: bool,
    },

    /// 延迟执行: defer expr
    Defer {
        expr: Box<Expr>,
    },

    Break,
    Continue,
    Return(Option<Box<Expr>>),

    // 赋值表达式
    Assign {
        lhs: Box<Expr>,
        op: AssignmentOperator,
        rhs: Box<Expr>,
    },

    // --- Conversion ---
    As {
        lhs: Box<Expr>,
        target: Box<TypeNode>,
    },

    Undef,
    Infer,

    /// 泛型实例化: target[T, U]
    GenericInstantiation {
        target: Box<Expr>,
        types: Vec<TypeNode>,
    },

    /// 代表 `self`
    SelfValue,

    /// 无状态匿名函数 (Lambda)
    /// 语法: `fn(a: i32, b: i32) bool { return a < b; }`
    Lambda {
        params: Vec<FuncParam>,
        ret_type: Box<TypeNode>,
        body: Box<Expr>, // 必定是一个 Block 表达式
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum DataLiteralKind {
    /// 结构体模式: `.{ x: 1, y: 2 }`
    Struct(Vec<StructFieldInit>),

    /// 数组列表模式: `.{ 1, 2, 3 }`
    Array(Vec<Expr>),

    /// 数组重复模式: `.{ 0; 1024 }`
    Repeat { value: Box<Expr>, count: Box<Expr> },

    /// 标量/单值构造模式: `.{ 10 }`
    Scalar(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructFieldInit {
    pub name: SymbolId,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    /// 一个 Arm 可以有多个 Pattern，比如: 11, 12, 13 =>
    pub patterns: Vec<MatchPattern>,
    pub body: Expr,
    pub span: Span,
}
