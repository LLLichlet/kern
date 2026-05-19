//! Strongly typed MIR index identifiers.
//!
//! Blocks and locals are stored in dense vectors inside each MIR body.  These
//! wrapper types keep those index spaces distinct while remaining cheap to copy,
//! hash, sort, and print in diagnostics.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MirBlockId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MirLocalId(pub u32);
