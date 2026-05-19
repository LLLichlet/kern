#![doc = include_str!("../README.md")]

//! Shared identifiers and metadata for monomorphized compiler items.
//!
//! Lowering assigns compact `MonoId`s to concrete generic instantiations and
//! backend-visible aggregate forms.  Later MIR and codegen stages use this crate
//! to agree on those ids without depending on lowering internals.

use kernc_ty::{DefId, GenericArg, TypeId};
use std::collections::HashMap;

/// Monomorphized item identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MonoId(pub u32);

/// Shared monomorphization metadata carried across lowering, MIR, and backend phases.
#[derive(Debug, Clone, Default)]
pub struct MonoModuleMetadata {
    /// Maps a generic definition plus concrete arguments to the emitted item.
    pub def_mono_map: HashMap<(DefId, Vec<GenericArg>), MonoId>,
    /// Maps pure enum instantiations to the monomorphized tag type.
    pub pure_enum_tag_map: HashMap<(DefId, Vec<GenericArg>), TypeId>,
    /// Maps an enum wrapper struct to its generated payload union.
    pub adt_union_map: HashMap<MonoId, MonoId>,
    pub range_map: HashMap<TypeId, MonoId>,
    pub anon_struct_map: HashMap<TypeId, MonoId>,
    pub anon_union_map: HashMap<TypeId, MonoId>,
    pub anon_enum_map: HashMap<TypeId, MonoId>,
}
