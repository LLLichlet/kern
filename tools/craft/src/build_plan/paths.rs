//! Path derivation for generated build artifacts.
//!
//! Centralized path helpers keep compile outputs, link artifacts, staged files,
//! generated roots, and runtime package locations stable across build modes.

use super::{ArtifactKind, StagedAction, StagedActionKind};
use crate::graph::{BuildDomain, PackageId, SourceId};
use crate::plan::TargetKind;
use crate::script;
use std::path::{Path, PathBuf};

pub(super) fn artifact_kind(kind: TargetKind) -> ArtifactKind {
    match kind {
        TargetKind::Lib => ArtifactKind::Library,
        TargetKind::Bin | TargetKind::Test | TargetKind::Example => ArtifactKind::Executable,
    }
}

impl BuildDomain {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Target => "target",
        }
    }

    pub(super) fn select_target<'a>(
        self,
        host: &'a script::ScriptTarget,
        target: &'a script::ScriptTarget,
    ) -> &'a script::ScriptTarget {
        match self {
            Self::Host => host,
            Self::Target => target,
        }
    }
}

pub(super) fn package_layout_key(package_id: &PackageId) -> String {
    let base = sanitize_layout_segment(&package_id.name);
    match package_id.source {
        SourceId::Root | SourceId::WorkspaceMember { .. } => base,
        SourceId::PathDependency { .. } | SourceId::GitDependency { .. } => {
            format!("{base}~{:08x}", package_identity_hash(package_id))
        }
    }
}

pub(super) fn workspace_build_root(
    workspace_root: &Path,
    profile: &str,
    domain: BuildDomain,
    target: &script::ScriptTarget,
) -> PathBuf {
    workspace_root
        .join(".craft")
        .join("build")
        .join(profile)
        .join(build_domain_layout_segment(domain, target))
}

fn build_domain_layout_segment(domain: BuildDomain, target: &script::ScriptTarget) -> String {
    if *target == script::host_target() {
        domain.as_str().to_string()
    } else {
        format!("{}-{}", domain.as_str(), target.layout_key())
    }
}

pub(super) fn object_path(
    workspace_root: &Path,
    domain: BuildDomain,
    target: &script::ScriptTarget,
    package_layout_key: &str,
    profile: &str,
    kind: TargetKind,
    artifact_name: &str,
) -> PathBuf {
    workspace_build_root(workspace_root, profile, domain, target)
        .join("obj")
        .join(package_layout_key)
        .join(kind.as_str())
        .join(format!("{artifact_name}.o"))
}

pub(super) fn generated_root_path(
    workspace_root: &Path,
    domain: BuildDomain,
    target: &script::ScriptTarget,
    package_layout_key: &str,
    profile: &str,
    kind: TargetKind,
    artifact_name: &str,
) -> PathBuf {
    workspace_build_root(workspace_root, profile, domain, target)
        .join("gen")
        .join(package_layout_key)
        .join(kind.as_str())
        .join(artifact_name)
}

pub(super) fn artifact_root_path(
    workspace_root: &Path,
    domain: BuildDomain,
    target: &script::ScriptTarget,
    package_layout_key: &str,
    profile: &str,
    kind: TargetKind,
    artifact_name: &str,
) -> PathBuf {
    workspace_build_root(workspace_root, profile, domain, target)
        .join("stage")
        .join(package_layout_key)
        .join(kind.as_str())
        .join(artifact_name)
}

pub(super) fn artifact_path(
    workspace_root: &Path,
    target: &script::ScriptTarget,
    domain: BuildDomain,
    package_layout_key: &str,
    profile: &str,
    kind: TargetKind,
    artifact_name: &str,
) -> PathBuf {
    let file_name = match kind {
        TargetKind::Lib => format!("lib{artifact_name}.o"),
        TargetKind::Bin | TargetKind::Test | TargetKind::Example => {
            if target.os == script::ScriptOs::Windows {
                format!("{artifact_name}.exe")
            } else {
                artifact_name.to_string()
            }
        }
    };

    workspace_build_root(workspace_root, profile, domain, target)
        .join("out")
        .join(package_layout_key)
        .join(kind.as_str())
        .join(file_name)
}

pub(super) fn metadata_path(
    workspace_root: &Path,
    domain: BuildDomain,
    target: &script::ScriptTarget,
    package_layout_key: &str,
    profile: &str,
) -> PathBuf {
    workspace_build_root(workspace_root, profile, domain, target)
        .join("meta")
        .join(package_layout_key)
}

pub(super) fn test_metadata_path(
    workspace_root: &Path,
    domain: BuildDomain,
    target: &script::ScriptTarget,
    package_layout_key: &str,
    profile: &str,
    artifact_name: &str,
) -> PathBuf {
    workspace_build_root(workspace_root, profile, domain, target)
        .join("test")
        .join(package_layout_key)
        .join(format!("{artifact_name}.cases"))
}

fn sanitize_layout_segment(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "package".to_string()
    } else {
        trimmed.to_string()
    }
}

fn package_identity_hash(package_id: &PackageId) -> u32 {
    let mut hash = 0x811c9dc5u32;
    fn update(hash: &mut u32, text: &str) {
        for byte in text.as_bytes() {
            *hash ^= u32::from(*byte);
            *hash = hash.wrapping_mul(0x01000193);
        }
        *hash ^= 0xff;
        *hash = hash.wrapping_mul(0x01000193);
    }

    update(&mut hash, &package_id.name);
    update(&mut hash, &package_id.version);
    match &package_id.source {
        SourceId::Root => update(&mut hash, "root"),
        SourceId::WorkspaceMember { path } => {
            update(&mut hash, "workspace");
            update(&mut hash, path);
        }
        SourceId::PathDependency { path } => {
            update(&mut hash, "path");
            update(&mut hash, path);
        }
        SourceId::GitDependency {
            git,
            rev,
            branch,
            tag,
        } => {
            update(&mut hash, "git");
            update(&mut hash, git);
            update(&mut hash, rev.as_deref().unwrap_or(""));
            update(&mut hash, branch.as_deref().unwrap_or(""));
            update(&mut hash, tag.as_deref().unwrap_or(""));
        }
    }
    hash
}

pub(super) fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

pub(super) fn resolve_staged_action(workspace_root: &Path, action: &StagedAction) -> StagedAction {
    StagedAction {
        id: action.id,
        phase: action.phase,
        output: workspace_root
            .join(&action.output)
            .to_string_lossy()
            .to_string(),
        depends_on: action.depends_on.clone(),
        kind: match &action.kind {
            StagedActionKind::WriteFile { contents } => StagedActionKind::WriteFile {
                contents: contents.clone(),
            },
            StagedActionKind::CcCompile {
                source,
                include_dirs,
                defines,
                args,
                opt,
                debug,
            } => StagedActionKind::CcCompile {
                source: workspace_root.join(source).to_string_lossy().to_string(),
                include_dirs: include_dirs
                    .iter()
                    .map(|path| workspace_root.join(path).to_string_lossy().to_string())
                    .collect(),
                defines: defines.clone(),
                args: args.clone(),
                opt: *opt,
                debug: *debug,
            },
            StagedActionKind::RunTool { tool, args } => StagedActionKind::RunTool {
                tool: Box::new(script::BuildScriptTool {
                    target_name: tool.target_name.clone(),
                    executable_path: workspace_root
                        .join(&tool.executable_path)
                        .to_string_lossy()
                        .to_string(),
                    origin: tool.origin.clone(),
                }),
                args: args.clone(),
            },
            StagedActionKind::CopyFile { source } => StagedActionKind::CopyFile {
                source: workspace_root.join(source).to_string_lossy().to_string(),
            },
            StagedActionKind::CopyDirectory { source } => StagedActionKind::CopyDirectory {
                source: workspace_root.join(source).to_string_lossy().to_string(),
            },
        },
    }
}

pub(super) fn artifact_name(
    package_id: &PackageId,
    kind: TargetKind,
    target_name: Option<&str>,
) -> String {
    match kind {
        TargetKind::Lib => package_id.name.clone(),
        TargetKind::Bin | TargetKind::Test | TargetKind::Example => target_name
            .expect("named targets must provide a name")
            .to_string(),
    }
}
