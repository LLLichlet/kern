//! Semantic analysis for Kern source.
//!
//! This crate resolves names, builds semantic definition tables, checks types
//! and expressions, injects language builtins, evaluates trait/impl lookup, and
//! records per-node facts consumed by MIR/lowering and editor tooling.

mod builtin;
pub mod checker;
mod context;
pub mod def;
pub mod passes;
pub mod query;
pub mod scope;
pub mod semantic;
pub mod ty;

pub use builtin::BuiltinInjector;
pub use context::{SemaContext, SemaStructureSnapshot};
pub use query::{MemberCandidate, MemberQuery, MemberQueryEnv};
pub use semantic::{SemanticDefinition, SemanticSymbolKind};
pub use ty::LayoutEngine;
