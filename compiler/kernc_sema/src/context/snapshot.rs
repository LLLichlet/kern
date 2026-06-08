//! Structural semantic snapshot used by cached analysis and rollback.
//!
//! The snapshot owns graph-shaped state that can be restored exactly.  It does
//! not include transient query caches, timing counters, active recursion stacks,
//! or pending escape checks; `SemaContext::restore_structure` rebuilds those.

use super::semantic_index::SemanticIndexState;
use super::*;

#[derive(Clone)]
pub struct SemaStructureSnapshot {
    pub type_registry: TypeRegistry,
    pub facts: NodeFacts,
    pub defs: DefTable,
    pub scopes: SymbolTable,
    pub resolution: SemaResolutionState,
    pub impl_index: SemaImplIndexState,
    pub(crate) semantic_index: SemanticIndexState,
    pub(crate) recursive_reports: RecursiveReportState,
}
