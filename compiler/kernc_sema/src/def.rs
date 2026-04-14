use crate::scope::ScopeId;
use crate::ty::TypeId;
use kernc_ast as ast;
pub use kernc_ast::Visibility;
use kernc_utils::{FileId, Span, SymbolId};
use std::collections::HashMap;
use std::path::PathBuf;

/// Identifier for a semantic definition collected from the AST.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DefId(pub u32);

/// Unified representation for every top-level semantic definition.
/// The collect pass lowers AST declarations into these records.
#[derive(Debug, Clone)]
pub enum Def {
    Module(ModuleDef),
    Function(FunctionDef),
    Struct(StructDef),
    Union(UnionDef),
    Enum(EnumDef),
    Trait(TraitDef),
    AssociatedType(AssociatedTypeDef),
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
            Def::AssociatedType(d) => Some(d.name),
            Def::Global(d) => Some(d.name),
            Def::TypeAlias(d) => Some(d.name),
            Def::Impl(_) => None, // Impl blocks are anonymous containers.
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
    pub parent: Option<DefId>, // Parent module, for example `std` is the parent of `std.io`.
    pub is_imported: bool,
    pub scope_id: ScopeId,
    // Physical directory used as the anchor for relative imports like `use .foo`.
    pub dir_path: PathBuf,
    pub file_id: FileId,
    // On-demand registry of filesystem-backed submodules.
    pub submodules: HashMap<SymbolId, DefId>,
    pub items: Vec<DefId>,       // Definitions owned by this module.
    pub imports: Vec<ImportDef>, // Deferred `use` declarations resolved by a later pass.
    pub is_init: bool,
    pub docs: Option<ast::DocBlock>,
}

#[derive(Debug, Clone)]
pub struct ImportDef {
    pub path_kind: ast::UsePathKind,
    pub path: Vec<SymbolId>,
    pub target: ast::UseTarget,
    pub vis: Visibility,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FunctionDef {
    pub id: DefId,
    pub name: SymbolId,
    pub name_span: Span,
    pub vis: Visibility,
    pub parent: Option<DefId>, // Enclosing module or impl block.
    pub is_imported: bool,
    pub generics: Vec<ast::GenericParam>,
    pub where_clauses: Vec<ast::WhereClause>,
    pub params: Vec<ast::FuncParam>,
    pub ret_type: ast::TypeNode, // AST return type before semantic resolution.
    pub body: Option<Box<ast::Expr>>,
    pub is_const: bool,
    pub is_extern: bool,
    pub is_variadic: bool,
    pub is_intrinsic: bool,
    pub span: Span,
    pub resolved_sig: Option<TypeId>,
    pub docs: Option<ast::DocBlock>,
    pub attributes: Vec<ast::Attribute>,
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub id: DefId,
    pub name: SymbolId,
    pub vis: Visibility,
    pub parent_module: Option<DefId>,
    pub is_imported: bool,
    pub generics: Vec<ast::GenericParam>,
    pub where_clauses: Vec<ast::WhereClause>,
    pub fields: Vec<ast::StructFieldDef>,
    pub is_extern: bool,
    pub span: Span,
    pub docs: Option<ast::DocBlock>,
    pub attributes: Vec<ast::Attribute>,
}

#[derive(Debug, Clone)]
pub struct UnionDef {
    pub id: DefId,
    pub name: SymbolId,
    pub vis: Visibility,
    pub parent_module: Option<DefId>,
    pub is_imported: bool,
    pub generics: Vec<ast::GenericParam>,
    pub where_clauses: Vec<ast::WhereClause>,
    pub fields: Vec<ast::StructFieldDef>,
    pub is_extern: bool,
    pub span: Span,
    pub docs: Option<ast::DocBlock>,
}

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub id: DefId,
    pub name: SymbolId,
    pub vis: Visibility,
    pub is_imported: bool,
    pub generics: Vec<ast::GenericParam>,
    pub where_clauses: Vec<ast::WhereClause>,
    pub backing_type: Option<Box<ast::TypeNode>>,
    pub variants: Vec<ast::EnumVariant>,
    pub span: Span,
    pub docs: Option<ast::DocBlock>,
}

#[derive(Debug, Clone)]
pub struct TraitDef {
    pub id: DefId,
    pub name: SymbolId,
    pub vis: Visibility,
    pub is_imported: bool,
    pub generics: Vec<ast::GenericParam>,
    pub where_clauses: Vec<ast::WhereClause>,
    pub supertraits: Vec<ast::TypeNode>,
    pub resolved_supertraits: Vec<TypeId>,
    pub assoc_types: Vec<DefId>,
    // Method contracts declared by the trait.
    pub methods: Vec<ast::StructFieldDef>,
    pub resolved_methods: Vec<(SymbolId, TypeId)>,
    pub span: Span,
    pub is_builtin: bool,
    pub docs: Option<ast::DocBlock>,
}

#[derive(Debug, Clone)]
pub struct AssociatedTypeDef {
    pub id: DefId,
    pub name: SymbolId,
    pub parent_trait: Option<DefId>,
    pub parent_impl: Option<DefId>,
    pub is_imported: bool,
    pub generics: Vec<ast::GenericParam>,
    pub bounds: Vec<ast::TypeNode>,
    pub where_clauses: Vec<ast::WhereClause>,
    pub target: Option<ast::TypeNode>,
    pub resolved_bounds: Vec<TypeId>,
    pub span: Span,
    pub docs: Option<ast::DocBlock>,
}

#[derive(Debug, Clone)]
pub struct TypeAliasDef {
    pub id: DefId,
    pub name: SymbolId,
    pub vis: Visibility,
    pub is_imported: bool,
    pub generics: Vec<ast::GenericParam>,
    pub where_clauses: Vec<ast::WhereClause>,
    pub target: ast::TypeNode,
    pub span: Span,
    pub docs: Option<ast::DocBlock>,
}

#[derive(Debug, Clone)]
pub struct ImplDef {
    pub id: DefId,
    pub parent_module: Option<DefId>,
    pub is_imported: bool,
    pub generics: Vec<ast::GenericParam>,
    pub where_clauses: Vec<ast::WhereClause>,
    pub target_type: ast::TypeNode,
    pub trait_type: Option<ast::TypeNode>,
    pub assoc_types: Vec<DefId>,
    // Methods collected under this impl block.
    pub methods: Vec<DefId>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct GlobalDef {
    pub id: DefId,
    pub name: SymbolId,
    pub vis: Visibility,
    pub parent: Option<DefId>,
    pub is_imported: bool,
    pub value: ast::Expr,
    pub is_static: bool,
    pub is_extern: bool,
    pub is_mut: bool,
    pub span: Span,
    pub docs: Option<ast::DocBlock>,
    pub attributes: Vec<ast::Attribute>,
}
