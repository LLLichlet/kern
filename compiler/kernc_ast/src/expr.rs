use super::{
    AssignmentOperator, BinaryOperator, BindingPattern, FuncParam, LetPattern, MatchPattern,
    GenericArg, PathAnchor, Pattern, Stmt, TypeNode, UnaryOperator,
};
use kernc_utils::{NodeId, Span, SymbolId};

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub id: NodeId,
    pub span: Span,
    pub kind: ExprKind,
}

impl Expr {
    /// Returns true when the expression already ends with a block-like visual boundary.
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
    /// `let mut x = v` or `let x = v`
    Let {
        pattern: LetPattern,
        init: Box<Expr>,
        else_pattern: Option<Pattern>,
        else_branch: Option<Box<Expr>>,
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
    /// Anchored module/package path start such as `/net` or `..detail`.
    AnchoredPath {
        anchor: PathAnchor,
        name: SymbolId,
        name_span: Span,
    },

    /// Type namespace expression used by builtin type forms such as `?i32.None`.
    TypeNode(Box<TypeNode>),

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
        field_span: Span,
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
    /// Unified data-initialization syntax supporting both `Type.{ ... }` and `.{ ... }`.
    DataInit {
        /// `None` for `.{ ... }`, or the explicit type prefix for `Point.{ ... }`.
        type_node: Option<Box<TypeNode>>,
        /// The literal payload to initialize.
        literal: DataLiteralKind,
    },

    /// Contextual enum shorthand such as `.Red` or `.Ok`.
    EnumLiteral {
        variant: SymbolId,
        variant_span: Span,
    },

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

    /// Block expression: `{ stmt; stmt; expr }`
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

    /// Slice construction: `arr.[start..end]`
    SliceOp {
        lhs: Box<Expr>,
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        is_inclusive: bool,
        is_mut: bool,
    },

    /// Deferred execution: `defer expr`
    Defer {
        expr: Box<Expr>,
    },

    Break,
    Continue,
    Return(Option<Box<Expr>>),

    /// Assignment expression.
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

    /// Builtin propagation operators `.?` and `.!`.
    Propagate {
        operand: Box<Expr>,
        kind: PropagateKind,
    },

    Undef,
    Infer,

    /// Generic instantiation: `target[T, U]`
    GenericInstantiation {
        target: Box<Expr>,
        args: Vec<GenericArg>,
    },

    /// The `self` value inside methods.
    SelfValue,

    /// Closure expression.
    /// Example: `[a, ptr = b..&](x: i32) bool { return x > a; }`
    Closure {
        captures: Vec<CapturePattern>,
        params: Vec<FuncParam>,
        ret_type: Box<TypeNode>,
        /// Always a block expression.
        body: Box<Expr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropagateKind {
    Option,
    Result,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DataLiteralKind {
    /// Struct-style payload: `.{ x: 1, y: 2 }`
    Struct(Vec<StructFieldInit>),

    /// Array element list: `.{ 1, 2, 3 }`
    Array(Vec<Expr>),

    /// Array repeat literal: `.{ 0; 1024 }`
    Repeat { value: Box<Expr>, count: Box<Expr> },

    /// Scalar payload: `.{ 10 }`
    Scalar(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructFieldInit {
    pub name: SymbolId,
    pub name_span: Span,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CapturePattern {
    /// Local binding name visible inside the closure body.
    pub name: SymbolId,
    pub name_span: Span,
    /// Value expression to capture, such as `counter..&` or an implicit self-reference.
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    /// One arm may contain multiple patterns, for example `11, 12, 13 =>`.
    pub patterns: Vec<MatchPattern>,
    pub body: Expr,
    pub span: Span,
}
