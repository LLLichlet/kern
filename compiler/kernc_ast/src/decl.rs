use super::{
    AssociatedTypeDecl, Attribute, BindingPattern, DocBlock, EnumVariant, Expr, StructFieldDef,
    TraitMethodDef, TypeNode,
};
use kernc_utils::{NodeId, Span, SymbolId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Visibility {
    Private,
    Super,
    Package,
    Public,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PathAnchor {
    Parent,
    Package,
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

    /// `const x = ...`, `const x: T = ...`, `static x = ...`, or `extern static x: T;`
    Var {
        type_node: Option<Box<TypeNode>>,
        value: Option<Expr>,
        is_static: bool,
        is_extern: bool,
        is_mut: bool,
    },

    /// `type Name[T] = Target;`
    TypeAlias {
        generics: Vec<GenericParam>,
        where_clauses: Vec<WhereClause>,
        target: TypeNode,
    },

    /// `struct Name[T] { ... }`
    Struct {
        generics: Vec<GenericParam>,
        where_clauses: Vec<WhereClause>,
        fields: Vec<StructFieldDef>,
        is_extern: bool,
    },

    /// `union Name[T] { ... }`
    Union {
        generics: Vec<GenericParam>,
        where_clauses: Vec<WhereClause>,
        fields: Vec<StructFieldDef>,
        is_extern: bool,
    },

    /// `enum Name[T] { ... }`
    Enum {
        generics: Vec<GenericParam>,
        where_clauses: Vec<WhereClause>,
        backing_type: Option<Box<TypeNode>>,
        variants: Vec<EnumVariant>,
    },

    /// `trait Name[T] { ... }`
    Trait {
        generics: Vec<GenericParam>,
        where_clauses: Vec<WhereClause>,
        supertraits: Vec<TypeNode>,
        assoc_types: Vec<AssociatedTypeDecl>,
        methods: Vec<TraitMethodDef>,
    },

    /// Module declaration: `mod name;` or inline module `mod name { ... }`.
    Mod { decls: Option<Vec<Decl>> },

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
    /// External package root, for example `use std.io`.
    External,
    /// Relative path starting from the current module, for example `use .utils`.
    Current,
    /// Relative path starting from the parent module, for example `use ..common`.
    Parent,
    /// Current package root, for example `use /net.http`.
    Package,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UseTarget {
    Module(Option<SymbolId>),
    Tree(Vec<UseTree>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum UseTree {
    SelfModule {
        alias: Option<SymbolId>,
        span: Span,
        binding_span: Span,
    },
    Path {
        path: Vec<SymbolId>,
        alias: Option<SymbolId>,
        nested: Option<Vec<UseTree>>,
        span: Span,
        binding_span: Span,
    },
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
    pub kind: GenericParamKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GenericParamKind {
    Type,
    Const { ty: TypeNode },
}
