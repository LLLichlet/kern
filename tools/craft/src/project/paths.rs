//! Source-root path matching for analysis project resolution.
//!
//! Path helpers map workspace files, generated copies, template sources, and
//! declared targets back to the build unit the language server should analyze.

use crate::build_plan::{BuildUnit, GeneratedFileOrigin, SourceRootBinding};
use crate::plan::{PlanValue, TargetKind};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(super) fn compile_time_defines(
    cfg: &BTreeMap<String, PlanValue>,
    define: &BTreeMap<String, PlanValue>,
) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for (name, value) in cfg {
        values.insert(name.clone(), plan_value_string(value));
    }
    for (name, value) in define {
        values.insert(name.clone(), plan_value_string(value));
    }
    values
}

pub(super) fn build_unit_source_aliases(
    workspace_root: &Path,
    unit: &BuildUnit,
) -> BTreeMap<PathBuf, PathBuf> {
    unit.generated_files
        .iter()
        .filter_map(|generated| {
            let GeneratedFileOrigin::Copied { source } = &generated.origin else {
                return None;
            };
            Some((
                resolve_context_path(workspace_root, source),
                resolve_context_path(workspace_root, &generated.path),
            ))
        })
        .collect()
}

fn plan_value_string(value: &PlanValue) -> String {
    match value {
        PlanValue::Bool(value) => value.to_string(),
        PlanValue::String(value) => value.clone(),
    }
}

pub(super) fn unit_source_root_path(
    manifest_path: &Path,
    source_root: &SourceRootBinding,
) -> Option<PathBuf> {
    let package_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    match source_root {
        SourceRootBinding::PackagePath(path) => Some(package_root.join(path)),
        SourceRootBinding::AbsolutePath(path) => Some(PathBuf::from(path)),
        SourceRootBinding::BuildOutput { .. } => None,
    }
}

pub(super) fn resolve_unit_source_root_path(
    workspace_root: &Path,
    manifest_path: &Path,
    source_root: &SourceRootBinding,
) -> Option<PathBuf> {
    let package_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    match source_root {
        SourceRootBinding::PackagePath(path) => Some(package_root.join(path)),
        SourceRootBinding::AbsolutePath(path) => Some(PathBuf::from(path)),
        SourceRootBinding::BuildOutput { path, .. } => {
            let path = Path::new(path);
            Some(if path.is_absolute() {
                path.to_path_buf()
            } else {
                workspace_root.join(path)
            })
        }
    }
}

fn resolve_context_path(workspace_root: &Path, stored_path: &str) -> PathBuf {
    let path = Path::new(stored_path);
    if path.is_absolute() {
        normalize_platform_path(path.to_path_buf())
    } else {
        normalize_platform_path(workspace_root.join(path))
    }
}

pub(super) fn normalize_platform_path(path: PathBuf) -> PathBuf {
    let path = strip_windows_verbatim_prefix(path);
    strip_macos_private_var_prefix(path)
}

#[cfg(windows)]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix("\\\\?\\UNC\\") {
        return PathBuf::from(format!("\\\\{stripped}"));
    }
    if let Some(stripped) = raw.strip_prefix("\\\\?\\") {
        return PathBuf::from(stripped);
    }
    path
}

#[cfg(not(windows))]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    path
}

#[cfg(target_os = "macos")]
fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix("/private/var/") {
        return PathBuf::from(format!("/var/{stripped}"));
    }
    if raw == "/private/var" {
        return PathBuf::from("/var");
    }
    path
}

#[cfg(not(target_os = "macos"))]
fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
    path
}

pub(super) fn target_kind_from_str(raw: &str) -> Option<TargetKind> {
    match raw {
        "lib" => Some(TargetKind::Lib),
        "bin" => Some(TargetKind::Bin),
        "test" => Some(TargetKind::Test),
        "example" => Some(TargetKind::Example),
        _ => None,
    }
}
