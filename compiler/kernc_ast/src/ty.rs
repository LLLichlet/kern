use super::{DocBlock, Expr};
use kernc_utils::{NodeId, Span, SymbolId};

#[derive(Debug, Clone, PartialEq)]
pub struct TypeNode {
    pub id: NodeId,
    pub span: Span,
    pub kind: TypeKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeKind {
    /// Segmented type path or projection chain such as `i32`, `std.io.File`,
    /// `Map[K, V]`, or `T.Add[U].Out`.
    Path {
        segments: Vec<TypePathSegment>,
    },

    /// Builtin optional type `?T`.
    Optional { inner: Box<TypeNode> },

    /// Builtin result type `T!E`.
    Result {
        ok: Box<TypeNode>,
        err: Box<TypeNode>,
    },

    /// Pointer type: `*T` or `*mut T`.
    Pointer { is_mut: bool, elem: Box<TypeNode> },

    /// Volatile pointer type: `^T` or `^mut T`.
    VolatilePtr { is_mut: bool, elem: Box<TypeNode> },

    /// Array type: `[N]T`.
    Array {
        is_mut: bool,
        elem: Box<TypeNode>,
        /// Must evaluate successfully in a constant context.
        len: Box<Expr>,
    },

    /// Array with inferred length, `[_]T`.
    ArrayInfer { is_mut: bool, elem: Box<TypeNode> },

    /// Slice type: `[]T`.
    Slice { is_mut: bool, elem: Box<TypeNode> },

    /// Function pointer type: `fn(i32) bool`.
    Function {
        params: Vec<TypeNode>,
        ret: Option<Box<TypeNode>>,
        is_variadic: bool,
    },

    // === Anonymous / inline structural types ===
    /// Struct type definition.
    Struct {
        is_extern: bool,
        fields: Vec<StructFieldDef>,
    },

    /// Union type definition.
    Union {
        is_extern: bool,
        fields: Vec<StructFieldDef>,
    },

    /// Algebraic enum type.
    /// Example: `type Result[T]: u8 = enum { Ok: T, Err, None = 0xFF }`.
    Enum {
        backing_type: Option<Box<TypeNode>>,
        variants: Vec<EnumVariant>,
    },

    /// Trait definition.
    Trait {
        assoc_types: Vec<AssociatedTypeDecl>,
        methods: Vec<StructFieldDef>,
    },

    /// Inference placeholder `_`.
    Infer,

    /// The target type of the current impl block.
    SelfType,

    /// Never type `!`.
    Never,

    /// Void type, a zero-sized type with no payload.
    Void,

    /// Compile-time type query `@typeOf(expr)`.
    TypeOf(Box<Expr>),

    /// Dynamic closure fat-pointer interface such as `Fn(i32) bool`.
    ClosureInterface {
        params: Vec<TypeNode>,
        ret: Option<Box<TypeNode>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypePathSegment {
    pub name: SymbolId,
    pub name_span: Span,
    pub args: Vec<TypeArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeArg {
    Positional(TypeNode),
    AssocBinding {
        name: SymbolId,
        name_span: Span,
        value: TypeNode,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructFieldDef {
    pub name: SymbolId,
    pub name_span: Span,
    pub is_pub: bool,
    pub docs: Option<DocBlock>,
    pub type_node: TypeNode,
    pub default_value: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssociatedTypeDecl {
    pub name: SymbolId,
    pub name_span: Span,
    pub docs: Option<DocBlock>,
    pub generics: Vec<super::GenericParam>,
    pub bounds: Vec<TypeNode>,
    pub where_clauses: Vec<super::WhereClause>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariant {
    pub name: SymbolId,
    pub name_span: Span,
    pub docs: Option<DocBlock>,
    /// Optional payload type, for example `Ok: i32`.
    pub payload_type: Option<Box<TypeNode>>,
    /// Explicit discriminant value, for example `Red = 0`.
    pub value: Option<Box<Expr>>,
    pub span: Span,
}
