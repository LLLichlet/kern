use crate::scope::ScopeId;
use crate::ty::TypeId;
use kernc_ast as ast;
pub use kernc_ast::Visibility;
pub use kernc_ty::DefId;
use kernc_utils::{FileId, Span, SymbolId};
use std::collections::HashMap;
use std::ops::{Deref, Index, IndexMut};
use std::path::PathBuf;

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
    pub fn id(&self) -> DefId {
        match self {
            Def::Module(d) => d.id,
            Def::Function(d) => d.id,
            Def::Struct(d) => d.id,
            Def::Union(d) => d.id,
            Def::Enum(d) => d.id,
            Def::Trait(d) => d.id,
            Def::AssociatedType(d) => d.id,
            Def::Impl(d) => d.id,
            Def::Global(d) => d.id,
            Def::TypeAlias(d) => d.id,
        }
    }

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

#[derive(Debug, Clone, Default)]
pub struct DefTable {
    defs: Vec<Def>,
}

impl DefTable {
    pub fn new() -> Self {
        Self { defs: Vec::new() }
    }

    pub fn next_id(&self) -> DefId {
        DefId(self.defs.len() as u32)
    }

    pub fn add(&mut self, def: Def) -> DefId {
        let id = self.next_id();
        assert_eq!(
            def.id(),
            id,
            "definition table inserted a definition with a mismatched DefId",
        );
        self.defs.push(def);
        id
    }

    pub fn ids(&self) -> impl Iterator<Item = DefId> {
        (0..self.defs.len()).map(|i| DefId(i as u32))
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut Def> {
        self.defs.get_mut(index)
    }
}

impl Index<usize> for DefTable {
    type Output = Def;

    fn index(&self, index: usize) -> &Self::Output {
        &self.defs[index]
    }
}

impl IndexMut<usize> for DefTable {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.defs[index]
    }
}

impl Deref for DefTable {
    type Target = [Def];

    fn deref(&self) -> &Self::Target {
        &self.defs
    }
}

impl<'a> IntoIterator for &'a DefTable {
    type Item = &'a Def;
    type IntoIter = std::slice::Iter<'a, Def>;

    fn into_iter(self) -> Self::IntoIter {
        self.defs.iter()
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
    pub binding_span: Span,
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
    pub implemented_trait_assoc: Option<DefId>,
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
