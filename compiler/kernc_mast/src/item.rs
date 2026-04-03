use crate::{MastBlock, MastExpr, MonoId};
use kernc_ast::MetaItem;
use kernc_sema::def::DefId;
use kernc_sema::ty::TypeId;
use kernc_utils::SymbolId;
use std::collections::HashMap;

/// Final flattened compilation unit produced by lowering.
/// At this stage there are no nested modules, impl blocks, or unresolved generics.
#[derive(Debug, Clone)]
pub struct MastModule {
    pub name: String,
    pub structs: Vec<MastStruct>,
    /// All statics, including lowered local statics.
    pub globals: Vec<MastGlobal>,
    pub functions: Vec<MastFunction>,
    /// Maps frontend abstract entities to concrete backend monomorphizations.
    pub def_mono_map: HashMap<(DefId, Vec<TypeId>), MonoId>,
    pub pure_enum_tag_map: HashMap<(DefId, Vec<TypeId>), TypeId>,
    pub adt_union_map: HashMap<MonoId, MonoId>,
    pub anon_struct_map: HashMap<TypeId, MonoId>,
    pub anon_union_map: HashMap<TypeId, MonoId>,
    pub anon_enum_map: HashMap<TypeId, MonoId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MastLinkage {
    External,
    Internal,
}

#[derive(Debug, Clone)]
pub struct MastStruct {
    pub id: MonoId,
    /// Flattened fully qualified name such as `std_collections_ArrayList_i32`.
    pub name: String,
    pub fields: Vec<MastField>,
    /// Preserves source layout for ABI-facing structs.
    pub is_extern: bool,
    pub is_union: bool,
    pub largest_field_idx: usize,
    pub union_size: usize,
    pub union_align: usize,
    pub attributes: Vec<MetaItem>,
}

#[derive(Debug, Clone)]
pub struct MastField {
    pub name: SymbolId,
    /// Always fully concrete and never contains `Param`.
    pub ty: TypeId,
}

#[derive(Debug, Clone)]
pub struct MastGlobal {
    pub id: MonoId,
    /// Flattened global symbol name.
    pub name: String,
    pub linkage: MastLinkage,
    pub ty: TypeId,
    /// Mirrors `static mut`.
    pub is_mut: bool,
    /// `None` for extern declarations. Initializers must be constant expressions.
    pub init: Option<MastExpr>,
    pub is_extern: bool,
    pub attributes: Vec<MetaItem>,
}

#[derive(Debug, Clone)]
pub struct MastFunction {
    pub id: MonoId,
    /// Flattened symbol name, for example `Point_i32_move_by`.
    pub name: String,
    pub linkage: MastLinkage,
    pub params: Vec<MastParam>,
    pub ret_ty: TypeId,
    /// `None` for extern declarations.
    pub body: Option<MastBlock>,
    pub is_extern: bool,
    pub is_variadic: bool,
    pub attributes: Vec<MetaItem>,
}

#[derive(Debug, Clone)]
pub struct MastParam {
    pub name: SymbolId,
    pub ty: TypeId,
    pub is_mut: bool,
}
