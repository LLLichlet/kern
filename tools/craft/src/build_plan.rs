//! Workspace build-plan construction.
//!
//! Build plans expand the resolved package graph into compile/link/staging
//! actions, generated-source bindings, build-script applications, and runtime
//! support units.

mod actions;
mod derive_support;
mod paths;
#[cfg(test)]
mod tests;

use crate::graph::{BuildDomain, PackageId};
use crate::plan::{PlanValue, TargetKind};
use crate::resolver::ExternalPackageId;
use crate::script;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub use self::derive_support::derive;
pub use self::derive_support::derive_with_options;
use self::derive_support::resolve_compile_source_input;
use self::paths::{
    artifact_kind, artifact_name, artifact_path, artifact_root_path, generated_root_path,
    metadata_path, object_path, relative_display, resolve_staged_action, test_metadata_path,
    workspace_build_root,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub workspace_root: PathBuf,
    pub build_nodes: Vec<StagedAction>,
    pub packages: Vec<PackageBuildPlan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeriveOptions {
    pub include_examples: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageBuildPlan {
    pub domain: BuildDomain,
    pub package_id: PackageId,
    pub manifest_path: PathBuf,
    pub build_script: Option<BuildScriptInput>,
    pub build_local_dependencies: Vec<LocalDependencyBinding>,
    pub build_external_dependencies: Vec<ExternalDependencyBinding>,
    pub units: Vec<BuildUnit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildUnit {
    pub domain: BuildDomain,
    pub package_id: PackageId,
    pub package_root_path: PathBuf,
    pub target_kind: TargetKind,
    pub target_name: Option<String>,
    pub source_root: SourceRootBinding,
    pub artifact_kind: ArtifactKind,
    pub artifact_name: String,
    pub local_dependencies: Vec<LocalDependencyBinding>,
    pub external_dependencies: Vec<ExternalDependencyBinding>,
    pub profile: script::ScriptProfile,
    pub cfg: BTreeMap<String, PlanValue>,
    pub define: BTreeMap<String, PlanValue>,
    pub generated_files: Vec<GeneratedFile>,
    pub build: BuildNodeBindings,
    pub link: LinkPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    Library,
    Executable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildScriptInput {
    pub path: PathBuf,
    pub relative_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFile {
    pub path: String,
    pub origin: GeneratedFileOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuildNodeBindings {
    pub compile_inputs: Vec<usize>,
    pub artifact_outputs: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceRootBinding {
    PackagePath(String),
    AbsolutePath(String),
    BuildOutput { id: usize, path: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeneratedFileOrigin {
    Emitted,
    Copied { source: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedAction {
    pub id: usize,
    pub phase: StagedActionPhase,
    pub output: String,
    pub depends_on: Vec<usize>,
    pub kind: StagedActionKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StagedActionKind {
    WriteFile {
        contents: String,
    },
    CcCompile {
        source: String,
        include_dirs: Vec<String>,
        defines: Vec<String>,
        args: Vec<String>,
        opt: u8,
        debug: bool,
    },
    RunTool {
        tool: Box<script::BuildScriptTool>,
        args: Vec<String>,
    },
    CopyFile {
        source: String,
    },
    CopyDirectory {
        source: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagedActionPhase {
    PreCompile,
    PostLink,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LinkPlan {
    pub system_libs: Vec<String>,
    pub frameworks: Vec<String>,
    pub search_paths: Vec<String>,
    pub args: Vec<String>,
    pub input_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionPlan {
    pub build_nodes: Vec<StagedAction>,
    pub compile_actions: Vec<CompileAction>,
    pub link_actions: Vec<LinkAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileAction {
    pub domain: BuildDomain,
    pub package_id: PackageId,
    pub manifest_path: PathBuf,
    pub target_kind: TargetKind,
    pub target_name: Option<String>,
    pub artifact_name: String,
    pub generated_root_path: PathBuf,
    pub source_input: CompileSourceInput,
    pub metadata_path: Option<PathBuf>,
    pub test_metadata_path: Option<PathBuf>,
    pub object_path: PathBuf,
    pub artifact_path: PathBuf,
    pub profile: script::ScriptProfile,
    pub cfg: BTreeMap<String, PlanValue>,
    pub define: BTreeMap<String, PlanValue>,
    pub compile_inputs: Vec<usize>,
    pub local_dependencies: Vec<LocalDependencyBinding>,
    pub external_dependencies: Vec<ExternalDependencyBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileSourceInput {
    PackagePath(PathBuf),
    AbsolutePath(PathBuf),
    BuildOutput { id: usize, path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkAction {
    pub domain: BuildDomain,
    pub package_id: PackageId,
    pub manifest_path: PathBuf,
    pub package_root_path: PathBuf,
    pub artifact_root_path: PathBuf,
    pub target_kind: TargetKind,
    pub target_name: Option<String>,
    pub artifact_name: String,
    pub artifact_path: PathBuf,
    pub primary_object: PathBuf,
    pub local_library_objects: Vec<PathBuf>,
    pub artifact_outputs: Vec<usize>,
    pub external_dependencies: Vec<ExternalDependencyBinding>,
    pub link: LinkPlan,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalDependencyBinding {
    pub domain: BuildDomain,
    pub dependency_name: String,
    pub package_id: PackageId,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExternalDependencyBinding {
    pub domain: BuildDomain,
    pub dependency_name: String,
    pub package_id: ExternalPackageId,
}
