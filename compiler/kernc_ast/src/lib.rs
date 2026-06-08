//! Parser-owned abstract syntax tree for Kern source.
//!
//! The AST keeps source-level structure and spans before semantic resolution.
//! Names are interned as `SymbolId`, every major node carries a `NodeId`, and
//! semantic crates attach meaning through side tables instead of mutating these
//! syntax records in place.

mod attr;
mod decl;
mod doc;
mod expr;
mod module;
mod op;
mod pat;
mod stmt;
mod ty;

pub use attr::*;
pub use decl::*;
pub use doc::*;
pub use expr::*;
pub use module::*;
pub use op::*;
pub use pat::*;
pub use stmt::*;
pub use ty::*;
