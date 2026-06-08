//! Craft package manager and build-tool library.
//!
//! The library exposes manifest parsing, workspace graphing, build planning,
//! execution, formatting, publishing, and analysis-project support used by both
//! the `craft` binary and editor integrations.

pub mod analysis_context;
pub mod cli;
pub mod discover;
pub mod doc;
pub mod error;
pub mod fmt;
pub mod graph;
pub mod manifest;
pub mod plan;
pub mod project;
pub mod style;
pub mod workspace;

mod build_plan;
mod build_state;
mod elaborate;
mod execute;
mod local_state;
mod lockfile;
mod operation_lock;
mod publish;
mod resolver;
mod script;
mod sdk;
mod source;
mod target_defaults;

#[cfg(test)]
mod test_support;
