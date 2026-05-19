//! Shared compiler infrastructure used by every kernc stage.
//!
//! This crate deliberately stays small and dependency-light.  It owns the
//! cross-cutting data structures that should not belong to any one frontend,
//! semantic-analysis, MIR, or codegen crate: spans, source files, diagnostics,
//! symbol interning, cancellation, target/configuration data, and a few small
//! performance helpers.

pub mod atomic;
mod cancel;
pub mod config;
mod diagnostic;
mod fast_hash;
mod interner;
pub mod llvm_bitcode;
mod node;
mod panic;
mod session;
mod source;
mod span;

pub use atomic::{AtomicOrdering, AtomicRmwOp};
pub use cancel::{Canceled, CancellationToken};
pub use diagnostic::{
    Diagnostic, DiagnosticBuilder, DiagnosticCode, DiagnosticLevel, DiagnosticTag,
};
pub use fast_hash::{FastHashMap, FastHashSet};
pub use interner::{Interner, SymbolId};
pub use node::NodeId;
pub use panic::install_compiler_panic_hook;
pub use session::Session;
pub use source::{FileId, Location, SourceFile, SourceManager};
pub use span::Span;
