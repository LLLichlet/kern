//! Build-plan unit tests.
//!
//! Submodules cover dependency closure, target layout, and build-script-derived
//! actions without running the full executor.

use super::{
    ArtifactKind, DeriveOptions, GeneratedFileOrigin, SourceRootBinding, StagedActionKind,
    StagedActionPhase, artifact_path, artifact_root_path, derive, derive_with_options,
    generated_root_path, metadata_path, object_path, package_layout_key, workspace_build_root,
};
use crate::elaborate::plan;
use crate::graph::BuildDomain;
use crate::graph::PackageId;
use crate::manifest::Manifest;
use crate::plan::TargetKind;
use crate::script::ScriptOs;
use crate::workspace::load_members;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

mod deps;
mod layout;
mod scripts;

fn temp_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn os_variant_name(os: ScriptOs) -> &'static str {
    match os {
        ScriptOs::Unknown => "unknown",
        ScriptOs::Linux => "linux",
        ScriptOs::Windows => "windows",
        ScriptOs::Darwin => "darwin",
    }
}
