#![allow(unused)]

use crate::ast;
use crate::sema::{ty::{DefId, TypeId}, scope::ScopeId};
use crate::utils::{Span, SymbolId};

/// 定义的可见性
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Private,
}

impl From<bool> for Visibility {
    fn from(is_pub: bool) -> Self {
        if is_pub {
            Visibility::Public
        } else {
            Visibility::Private
        }
    }
}

/// 全局顶层定义的聚合枚举 (HIR - High-level Intermediate Representation)
/// 
/// 语义分析的 Collect 阶段会将 AST 转换为这些定义对象。
/// 注意：此时类型可能还未完全解析 (Type Resolution 未完成)，
/// 因此我们在结构中保留了部分 AST 节点（如 `ast::TypeNode`），留给下一阶段处理。
#[derive(Debug, Clone)]
pub enum Def {
    Module(ModuleDef),
    Function(FunctionDef),
    Struct(StructDef),
    Union(UnionDef),
    Enum(EnumDef),
    Trait(TraitDef),
    Impl(ImplDef),
    Global(GlobalDef),
    TypeAlias(TypeAliasDef),
}

impl Def {
    pub fn name(&self) -> Option<SymbolId> {
        match self {
            Def::Module(d) => Some(d.name),
            Def::Function(d) => Some(d.name),
            Def::Struct(d) => Some(d.name),
            Def::Union(d) => Some(d.name),
            Def::Enum(d) => Some(d.name),
            Def::Trait(d) => Some(d.name),
            Def::Global(d) => Some(d.name),
            Def::TypeAlias(d) => Some(d.name),
            Def::Impl(_) => None, // Impl 块没有直接的名字
        }
    }
}

// ==========================================
//               Definitions
// ==========================================

#[derive(Debug, Clone)]
pub struct ModuleDef {
    pub id: DefId,
    pub name: SymbolId,
    pub parent: Option<DefId>, // 记录父模块 (例如 std.io 的父模块是 std)
    pub scope_id: ScopeId,
    pub items: Vec<DefId>,     // 模块内定义的成员
    pub imports: Vec<ImportDef>, // 记录所有的 use 声明，留给下一阶段解析
}

#[derive(Debug, Clone)]
pub struct ImportDef {
    pub path_kind: ast::UsePathKind,
    pub path: Vec<SymbolId>,
    pub target: ast::UseTarget,
    pub is_reexport: bool, // pub use ...
    pub span: crate::utils::Span,
}

#[derive(Debug, Clone)]
pub struct FunctionDef {
    pub id: DefId,
    pub name: SymbolId,
    pub vis: Visibility,
    pub parent: Option<DefId>, // 所属的 Module 或 Impl 块
    pub generics: Vec<ast::GenericParam>,
    pub params: Vec<ast::FuncParam>,
    pub ret_type: ast::TypeNode, // AST 类型，等待 Resolve Pass 转换为 TypeId
    pub body: Option<Box<ast::Expr>>,
    pub is_extern: bool,
    pub is_variadic: bool,
    pub is_intrinsic: bool,
    pub span: Span,
    pub resolved_sig: Option<TypeId>,
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub id: DefId,
    pub name: SymbolId,
    pub vis: Visibility,
    pub generics: Vec<ast::GenericParam>,
    pub fields: Vec<ast::StructFieldDef>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct UnionDef {
    pub id: DefId,
    pub name: SymbolId,
    pub vis: Visibility,
    pub generics: Vec<ast::GenericParam>,
    pub fields: Vec<ast::StructFieldDef>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub id: DefId,
    pub name: SymbolId,
    pub vis: Visibility,
    pub generics: Vec<ast::GenericParam>,
    pub backing_type: Option<Box<ast::TypeNode>>,
    pub variants: Vec<ast::EnumVariant>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TraitDef {
    pub id: DefId,
    pub name: SymbolId,
    pub vis: Visibility,
    pub supertraits: Vec<ast::TypeNode>,
    // 特征中定义的方法契约
    pub methods: Vec<ast::StructFieldDef>, 
    pub span: Span,
    pub is_builtin: bool,
}

#[derive(Debug, Clone)]
pub struct TypeAliasDef {
    pub id: DefId,
    pub name: SymbolId,
    pub vis: Visibility,
    pub generics: Vec<ast::GenericParam>,
    pub target: ast::TypeNode,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ImplDef {
    pub id: DefId,
    pub parent_module: Option<DefId>,
    pub generics: Vec<ast::GenericParam>,
    pub target_type: ast::TypeNode,
    pub trait_type: Option<ast::TypeNode>,
    // 收集属于该 impl 块的所有方法的 DefId
    pub methods: Vec<DefId>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct GlobalDef {
    pub id: DefId,
    pub name: SymbolId,
    pub vis: Visibility,
    pub type_node: Option<ast::TypeNode>,
    pub value: ast::Expr,
    pub is_static: bool,
    pub is_extern: bool,
    pub span: Span,
}