//! Ordered semantic passes after parsing and conditional pruning.
//!
//! The driver runs these passes to collect declarations into definitions,
//! resolve imports, resolve types/contracts, validate linkage, and prune
//! conditionally-disabled AST before deeper checking/lowering.

mod collect;
mod imports;
mod linkage;
mod prune;
mod types;

pub use collect::Collector;
pub use imports::ImportResolver;
pub use linkage::LinkageChecker;
pub use prune::Pruner;
pub use types::TypeResolver;
