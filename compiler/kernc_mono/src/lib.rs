#![doc = include_str!("../README.md")]

use kernc_sema::def::DefId;
use kernc_sema::ty::TypeId;
use std::collections::HashMap;

/// Monomorphized item identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MonoId(pub u32);

/// Shared monomorphization metadata carried across lowering, MIR, and backend phases.
#[derive(Debug, Clone, Default)]
pub struct MonoModuleMetadata {
    pub def_mono_map: HashMap<(DefId, Vec<TypeId>), MonoId>,
    pub pure_enum_tag_map: HashMap<(DefId, Vec<TypeId>), TypeId>,
    pub adt_union_map: HashMap<MonoId, MonoId>,
    pub anon_struct_map: HashMap<TypeId, MonoId>,
    pub anon_union_map: HashMap<TypeId, MonoId>,
    pub anon_enum_map: HashMap<TypeId, MonoId>,
}
