use super::{Attribute, BindingPattern, Expr, TypeNode};
use kernc_utils::{NodeId, Span, SymbolId};

#[derive(Debug, Clone, PartialEq)]
pub struct Decl {
    pub id: NodeId,
    pub span: Span,
    pub name: SymbolId,
    pub is_pub: bool,
    pub attributes: Vec<Attribute>,
    pub kind: DeclKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhereClause {
    pub span: Span,
    pub target_ty: TypeNode,
    pub bounds: Vec<TypeNode>, // 通常是 TypeKind::Path (Trait的路径)
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeclKind {
    /// `fn name[T](...) Ret { ... }`
    Function {
        generics: Vec<GenericParam>,
        where_clauses: Vec<WhereClause>,
        params: Vec<FuncParam>,
        ret_type: TypeNode,
        body: Option<Box<Expr>>, // Block
        is_const: bool,
        is_extern: bool,
        is_variadic: bool,
    },

    /// `const x = ...` 或 `static x = ...`
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

    /// 模块声明：`mod name;`
    ModDecl { is_pub: bool },

    /// 模块引入
    Use {
        kind: UsePathKind,
        path: Vec<SymbolId>,
        target: UseTarget,
        is_reexport: bool,
    },

    /// Extern 块
    ExternBlock {
        abi: Option<String>,
        decls: Vec<Decl>,
    },

    /// Impl 块
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
    /// 绝对路径，从根模块 (Root/Crate) 开始找：`use std.io`
    Root,
    /// 相对路径，从当前模块开始找：`use .utils`
    Current,
    /// 相对路径，仅支持从父级模块开始找：`use ..common`
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
