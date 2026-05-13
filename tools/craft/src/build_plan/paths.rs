use super::{ArtifactKind, StagedAction, StagedActionKind};
use crate::graph::{BuildDomain, PackageId};
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

pub(super) fn object_path(
    workspace_root: &Path,
    domain: BuildDomain,
    package_id: &PackageId,
    profile: &str,
    kind: TargetKind,
    artifact_name: &str,
) -> PathBuf {
    workspace_build_root(workspace_root, profile, domain)
        .join("obj")
        .join(package_dir_name(package_id))
        .join(kind.as_str())
        .join(format!("{artifact_name}.o"))
}

pub(super) fn workspace_build_root(
    workspace_root: &Path,
    profile: &str,
    domain: BuildDomain,
) -> PathBuf {
    workspace_root
        .join(".craft")
        .join("build")
        .join(profile)
        .join(domain.as_str())
}

pub(super) fn generated_root_path(
    workspace_root: &Path,
    domain: BuildDomain,
    package_id: &PackageId,
    profile: &str,
    kind: TargetKind,
    artifact_name: &str,
) -> PathBuf {
    workspace_build_root(workspace_root, profile, domain)
        .join("gen")
        .join(package_dir_name(package_id))
        .join(kind.as_str())
        .join(artifact_name)
}

pub(super) fn artifact_root_path(
    workspace_root: &Path,
    domain: BuildDomain,
    package_id: &PackageId,
    profile: &str,
    kind: TargetKind,
    artifact_name: &str,
) -> PathBuf {
    workspace_build_root(workspace_root, profile, domain)
        .join("stage")
        .join(package_dir_name(package_id))
        .join(kind.as_str())
        .join(artifact_name)
}

pub(super) fn artifact_path(
    workspace_root: &Path,
    target: &script::ScriptTarget,
    domain: BuildDomain,
    package_id: &PackageId,
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

    workspace_build_root(workspace_root, profile, domain)
        .join("out")
        .join(package_dir_name(package_id))
        .join(kind.as_str())
        .join(file_name)
}

pub(super) fn metadata_path(
    workspace_root: &Path,
    domain: BuildDomain,
    package_id: &PackageId,
    profile: &str,
) -> PathBuf {
    workspace_build_root(workspace_root, profile, domain)
        .join("meta")
        .join(package_dir_name(package_id))
}

pub(super) fn test_metadata_path(
    workspace_root: &Path,
    domain: BuildDomain,
    package_id: &PackageId,
    profile: &str,
    artifact_name: &str,
) -> PathBuf {
    workspace_build_root(workspace_root, profile, domain)
        .join("test")
        .join(package_dir_name(package_id))
        .join(format!("{artifact_name}.cases"))
}

fn package_dir_name(package_id: &PackageId) -> String {
    format!("{}-{}", package_id.name, package_id.version)
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
