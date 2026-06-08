//! Expression syntax tree.
//!
//! This file is intentionally broad because Kern expressions include value
//! literals, control flow, local bindings, aggregate construction, generic
//! instantiation, closure syntax, and a few builtin operators.  The enum stores
//! parser-visible form; semantic lowering decides whether a form is legal in a
//! value, type, constant, or pattern-adjacent context.

use super::{
    AssignmentOperator, BinaryOperator, BindingPattern, FuncParam, GenericArg, LetPattern,
    MatchPattern, PathAnchor, Pattern, Stmt, TypeNode, UnaryOperator,
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
                | ExprKind::While { .. }
                | ExprKind::Block { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// Parser recovery placeholder for a syntactically missing or invalid expression.
    Error,

    /// `let mut x = v` or `let x = v`
    Let {
        pattern: LetPattern,
        type_node: Option<Box<TypeNode>>,
        init: Box<Expr>,
        else_clause: Option<LetElseClause>,
    },

    /// `static x = v` or `static x: T`
    Static {
        pattern: BindingPattern,
        type_node: Option<Box<TypeNode>>,
        init: Option<Box<Expr>>,
    },

    // --- Literals ---
    Integer {
        value: u128,
        suffix: Option<NumericLiteralSuffix>,
    },
    Float {
        value: f64,
        suffix: Option<NumericLiteralSuffix>,
    },
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
    Range {
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        is_inclusive: bool,
    },
    Unary {
        op: UnaryOperator,
        operand: Box<Expr>,
    },
    Grouped {
        expr: Box<Expr>,
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

    /// `while cond body`
    While {
        cond: Box<Expr>,
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

    /// Builtin propagation operator `.?`.
    Propagate {
        operand: Box<Expr>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NumericLiteralSuffix {
    I8,
    I16,
    I32,
    I64,
    I128,
    ISize,
    U8,
    U16,
    U32,
    U64,
    U128,
    USize,
    F32,
    F64,
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

#[derive(Debug, Clone, PartialEq)]
pub enum LetElseClause {
    Expr(Box<Expr>),
    Arms(Vec<LetElseArm>),
}

impl LetElseClause {
    pub fn span(&self) -> Span {
        match self {
            Self::Expr(expr) => expr.span,
            Self::Arms(arms) => arms
                .first()
                // Parser recovery can synthesize an empty arm list.  Keep a
                // default span here instead of forcing every diagnostic path to
                // special-case malformed `let else`.
                .map(|arm| arm.span)
                .unwrap_or_else(Span::default),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LetElseArm {
    pub pattern: Pattern,
    pub body: Expr,
    pub span: Span,
}
