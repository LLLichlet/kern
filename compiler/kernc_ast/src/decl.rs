use super::{Attribute, BindingPattern, DocBlock, Expr, TypeNode};
use kernc_utils::{NodeId, Span, SymbolId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Visibility {
    Private,
    Super,
    Package,
    Public,
}

impl Visibility {
    pub fn is_public(self) -> bool {
        matches!(self, Self::Public)
    }

    pub fn is_private(self) -> bool {
        matches!(self, Self::Private)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decl {
    pub id: NodeId,
    pub span: Span,
    pub name_span: Span,
    pub name: SymbolId,
    pub vis: Visibility,
    pub docs: Option<DocBlock>,
    pub attributes: Vec<Attribute>,
    pub kind: DeclKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhereClause {
    pub span: Span,
    pub target_ty: TypeNode,
    /// Usually trait paths such as `Ord` or `Iterator[T]`.
    pub bounds: Vec<TypeNode>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeclKind {
    /// `fn name[T](...) Ret { ... }`
    Function {
        generics: Vec<GenericParam>,
        where_clauses: Vec<WhereClause>,
        params: Vec<FuncParam>,
        ret_type: TypeNode,
        /// Function body block, when present.
        body: Option<Box<Expr>>,
        is_const: bool,
        is_extern: bool,
        is_variadic: bool,
    },

    /// `const x = ...` or `static x = ...`
    Var {
        value: Expr,
        is_static: bool,
        is_extern: bool,
        is_mut: bool,
    },

    /// `type Name[T] = Target;`
    TypeAlias {
        generics: Vec<GenericParam>,
        bounds: Vec<TypeNode>,
        where_clauses: Vec<WhereClause>,
        target: TypeNode,
        is_extern: bool,
    },

    /// Module declaration: `mod name;`
    ModDecl,

    /// Import declaration.
    Use {
        kind: UsePathKind,
        path: Vec<SymbolId>,
        target: UseTarget,
    },

    /// Extern block.
    ExternBlock {
        abi: Option<String>,
        decls: Vec<Decl>,
    },

    /// Impl block.
    Impl {
        generics: Vec<GenericParam>,
        where_clauses: Vec<WhereClause>,
        target_type: TypeNode,
        trait_type: Option<TypeNode>,
        decls: Vec<Decl>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsePathKind {
    /// Absolute path starting from the root module, for example `use std.io`.
    Root,
    /// Relative path starting from the current module, for example `use .utils`.
    Current,
    /// Relative path starting from the parent module, for example `use ..common`.
    Parent,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UseTarget {
    Module(Option<SymbolId>),
    Members(Vec<UseMember>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct UseMember {
    pub path: Vec<SymbolId>,
    pub alias: Option<SymbolId>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FuncParam {
    pub pattern: BindingPattern,
    pub type_node: TypeNode,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenericParam {
    pub name: SymbolId,
    pub span: Span,
}
