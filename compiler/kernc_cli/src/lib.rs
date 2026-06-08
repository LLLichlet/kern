//! Test-facing support exports for the `kernc` CLI crate.
//!
//! The binary lives in `main.rs`; this library exists so integration tests can
//! share process-spawning and temporary-file helpers without depending on the
//! CLI parser internals.

#[doc(hidden)]
pub mod test_support;
