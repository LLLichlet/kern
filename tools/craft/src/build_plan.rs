use crate::elaborate::{ElaborationPlan, PackageElaboration};
use crate::error::Result;
use crate::graph::{BuildDomain, PackageId};
use crate::manifest::Manifest;
use crate::plan::PackagePlan;
use crate::plan::{PlanValue, TargetKind};
use crate::resolver::{
    ExternalPackageId, ResolvedDependencyTarget, ResolvedExternalPackage, ResolvedGraph,
    ResolvedPackageNode,
};
use crate::script;
use crate::source;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub workspace_root: PathBuf,
    pub build_nodes: Vec<StagedAction>,
    pub packages: Vec<PackageBuildPlan>,
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

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StagedActionKind {
    WriteFile {
        contents: String,
    },
    RunTool {
        tool: script::BuildScriptTool,
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
    pub target_kind: TargetKind,
    pub target_name: Option<String>,
    pub artifact_name: String,
    pub source_input: CompileSourceInput,
    pub metadata_path: Option<PathBuf>,
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

impl ActionPlan {
    pub fn compile_count(&self) -> usize {
        self.compile_actions.len()
    }

    pub fn link_count(&self) -> usize {
        self.link_actions.len()
    }

    #[cfg(test)]
    pub fn artifact_output_nodes_for_link_action<'a>(
        &'a self,
        action: &LinkAction,
    ) -> Vec<&'a StagedAction> {
        collect_build_nodes(
            self.build_nodes.as_slice(),
            action.artifact_outputs.as_slice(),
        )
    }
}

impl SourceRootBinding {
    pub fn display_path(&self) -> &str {
        match self {
            Self::PackagePath(path) | Self::AbsolutePath(path) => path.as_str(),
            Self::BuildOutput { path, .. } => path.as_str(),
        }
    }
}

impl CompileSourceInput {
    pub fn path(&self) -> &Path {
        match self {
            Self::PackagePath(path) | Self::AbsolutePath(path) => path.as_path(),
            Self::BuildOutput { path, .. } => path.as_path(),
        }
    }
}

impl CompileAction {
    pub fn source_path(&self) -> &Path {
        self.source_input.path()
    }

    pub fn required_source_path(&self) -> Option<&Path> {
        match &self.source_input {
            CompileSourceInput::BuildOutput { path, .. } => Some(path.as_path()),
            CompileSourceInput::PackagePath(_) | CompileSourceInput::AbsolutePath(_) => None,
        }
    }
}

impl BuildPlan {
    pub fn unit_count(&self) -> usize {
        self.packages
            .iter()
            .map(|package| package.units.len())
            .sum()
    }

    pub fn local_dependency_edge_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.local_dependencies.len())
            .sum()
    }

    pub fn external_dependency_edge_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.external_dependencies.len())
            .sum()
    }

    pub fn build_local_dependency_edge_count(&self) -> usize {
        self.packages
            .iter()
            .map(|package| package.build_local_dependencies.len())
            .sum()
    }

    pub fn build_external_dependency_edge_count(&self) -> usize {
        self.packages
            .iter()
            .map(|package| package.build_external_dependencies.len())
            .sum()
    }

    pub fn build_script_count(&self) -> usize {
        self.packages
            .iter()
            .filter(|package| package.build_script.is_some())
            .count()
    }

    pub fn generated_file_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.generated_files.len())
            .sum()
    }

    pub fn staged_action_count(&self) -> usize {
        self.build_nodes.len()
    }

    pub fn compile_input_nodes_for_unit<'a>(&'a self, unit: &BuildUnit) -> Vec<&'a StagedAction> {
        collect_build_nodes(
            self.build_nodes.as_slice(),
            unit.build.compile_inputs.as_slice(),
        )
    }

    pub fn artifact_output_nodes_for_unit<'a>(&'a self, unit: &BuildUnit) -> Vec<&'a StagedAction> {
        collect_build_nodes(
            self.build_nodes.as_slice(),
            unit.build.artifact_outputs.as_slice(),
        )
    }

    pub fn link_system_lib_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.link.system_libs.len())
            .sum()
    }

    pub fn link_framework_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.link.frameworks.len())
            .sum()
    }

    pub fn link_search_path_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.link.search_paths.len())
            .sum()
    }

    pub fn link_arg_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.link.args.len())
            .sum()
    }

    pub fn derive_actions(&self, target: &script::ScriptTarget) -> ActionPlan {
        let host = script::host_target();
        self.derive_actions_with_targets(&host, target)
    }

    pub fn derive_actions_with_targets(
        &self,
        host: &script::ScriptTarget,
        target: &script::ScriptTarget,
    ) -> ActionPlan {
        let build_nodes = self
            .build_nodes
            .iter()
            .map(|action| resolve_staged_action(&self.workspace_root, action))
            .collect();
        let mut compile_actions = Vec::new();

        for package in &self.packages {
            let package_root = package
                .manifest_path
                .parent()
                .unwrap_or_else(|| Path::new("."));
            for unit in &package.units {
                compile_actions.push(CompileAction {
                    domain: unit.domain,
                    package_id: unit.package_id.clone(),
                    target_kind: unit.target_kind,
                    target_name: unit.target_name.clone(),
                    artifact_name: unit.artifact_name.clone(),
                    source_input: resolve_compile_source_input(package_root, &unit.source_root),
                    metadata_path: (unit.target_kind == TargetKind::Lib).then(|| {
                        metadata_path(
                            &self.workspace_root,
                            unit.domain,
                            &unit.package_id,
                            &unit.profile.name,
                        )
                    }),
                    object_path: object_path(
                        &self.workspace_root,
                        unit.domain,
                        &unit.package_id,
                        &unit.profile.name,
                        unit.target_kind,
                        &unit.artifact_name,
                    ),
                    artifact_path: artifact_path(
                        &self.workspace_root,
                        unit.domain.select_target(host, target),
                        unit.domain,
                        &unit.package_id,
                        &unit.profile.name,
                        unit.target_kind,
                        &unit.artifact_name,
                    ),
                    profile: unit.profile.clone(),
                    cfg: unit.cfg.clone(),
                    define: unit.define.clone(),
                    compile_inputs: unit.build.compile_inputs.clone(),
                    local_dependencies: unit.local_dependencies.clone(),
                    external_dependencies: unit.external_dependencies.clone(),
                });
            }
        }

        let mut package_lib_objects = BTreeMap::new();
        for action in &compile_actions {
            if action.target_kind == TargetKind::Lib {
                package_lib_objects.insert(
                    (action.domain, action.package_id.clone()),
                    action.object_path.clone(),
                );
            }
        }

        let mut link_actions = Vec::new();
        for package in &self.packages {
            for unit in &package.units {
                if unit.artifact_kind != ArtifactKind::Executable {
                    continue;
                }

                link_actions.push(LinkAction {
                    domain: unit.domain,
                    package_id: unit.package_id.clone(),
                    target_kind: unit.target_kind,
                    target_name: unit.target_name.clone(),
                    artifact_name: unit.artifact_name.clone(),
                    artifact_path: artifact_path(
                        &self.workspace_root,
                        unit.domain.select_target(host, target),
                        unit.domain,
                        &unit.package_id,
                        &unit.profile.name,
                        unit.target_kind,
                        &unit.artifact_name,
                    ),
                    primary_object: object_path(
                        &self.workspace_root,
                        unit.domain,
                        &unit.package_id,
                        &unit.profile.name,
                        unit.target_kind,
                        &unit.artifact_name,
                    ),
                    local_library_objects: unit
                        .local_dependencies
                        .iter()
                        .filter_map(|binding| {
                            package_lib_objects
                                .get(&(binding.domain, binding.package_id.clone()))
                                .cloned()
                        })
                        .collect(),
                    artifact_outputs: unit.build.artifact_outputs.clone(),
                    external_dependencies: unit.external_dependencies.clone(),
                    link: unit.link.clone(),
                });
            }
        }

        ActionPlan {
            build_nodes,
            compile_actions,
            link_actions,
        }
    }
}

fn collect_build_nodes<'a>(
    build_nodes: &'a [StagedAction],
    ids: &[usize],
) -> Vec<&'a StagedAction> {
    ids.iter()
        .filter_map(|id| build_nodes.iter().find(|action| action.id == *id))
        .collect()
}

pub fn derive(
    elaboration: &ElaborationPlan,
    command: crate::script::ScriptCommand,
) -> Result<BuildPlan> {
    let host_target = script::host_target();
    let target_target = host_target.clone();
    let mut build_nodes = Vec::new();
    let mut sources = BTreeMap::new();
    for package in &elaboration.resolved_graph.packages {
        let package_elaboration = elaboration
            .packages
            .iter()
            .find(|entry| entry.package_id == package.id)
            .expect("elaboration must contain package plan");
        let package_root = package_elaboration
            .plan
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let build_script =
            discover_build_script(&elaboration.resolved_graph.workspace_root, package_root)?;
        sources.insert(
            package.id.clone(),
            PackageDeriveSource {
                resolved: package,
                elaboration: package_elaboration,
                build_script,
            },
        );
    }

    let mut host_packages = BTreeSet::new();
    for source in sources.values() {
        for dependency in &source.resolved.dependencies {
            if dependency.kind != crate::graph::DependencyKind::Build {
                continue;
            }
            if let ResolvedDependencyTarget::Local(target) = &dependency.target {
                collect_host_local_packages(target, &sources, &mut host_packages);
            }
        }
    }

    let mut packages = Vec::new();
    for source in sources.values() {
        packages.push(build_package_for_domain(
            BuildDomain::Target,
            source,
            &elaboration.resolved_graph.workspace_root,
        ));
    }
    for package_id in &host_packages {
        let source = sources
            .get(package_id)
            .expect("host closure package must exist in source map");
        packages.push(build_package_for_domain(
            BuildDomain::Host,
            source,
            &elaboration.resolved_graph.workspace_root,
        ));
    }

    packages.sort_by(|lhs, rhs| {
        lhs.domain
            .cmp(&rhs.domain)
            .then_with(|| lhs.package_id.cmp(&rhs.package_id))
    });

    let tool_index = build_tool_index(
        &packages,
        &elaboration.resolved_graph.workspace_root,
        &host_target,
        &target_target,
    );
    let workspace_manifest_path = elaboration.resolved_graph.workspace_root.join("Craft.toml");
    let workspace_manifest = Manifest::load(&workspace_manifest_path)?;
    workspace_manifest.validate(&workspace_manifest_path)?;
    let external_tool_index = build_external_tool_index(
        &packages,
        &elaboration.resolved_graph.workspace_root,
        &host_target,
        &workspace_manifest_path,
        &workspace_manifest,
        elaboration.profile_selection,
    )?;
    for package in &mut packages {
        let package_root = package
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let package_tools = build_tools_for_package(package, &tool_index, &external_tool_index);
        if let Some(build_script) = &package.build_script {
            let source = sources
                .get(&package.package_id)
                .expect("package source must exist for build script application");
            for unit in &mut package.units {
                let build_context = script::BuildScriptContext {
                    script: script_context_for_instance(
                        package.domain,
                        source.elaboration,
                        &elaboration.resolved_graph.workspace_root,
                        elaboration.has_workspace,
                        command,
                        &host_target,
                        &target_target,
                    ),
                    unit: script::BuildScriptUnit {
                        domain: unit.domain,
                        target_kind: unit.target_kind,
                        target_name: unit.target_name.clone(),
                        source_root: unit.source_root.display_path().to_string(),
                        artifact_name: unit.artifact_name.clone(),
                    },
                    paths: script::BuildScriptPaths {
                        build_root: workspace_build_root(
                            &elaboration.resolved_graph.workspace_root,
                            &unit.profile.name,
                            unit.domain,
                        )
                        .to_string_lossy()
                        .to_string(),
                        generated_root: generated_root_path(
                            &elaboration.resolved_graph.workspace_root,
                            unit.domain,
                            &unit.package_id,
                            &unit.profile.name,
                            unit.target_kind,
                            &unit.artifact_name,
                        )
                        .to_string_lossy()
                        .to_string(),
                        artifact_root: artifact_root_path(
                            &elaboration.resolved_graph.workspace_root,
                            unit.domain,
                            &unit.package_id,
                            &unit.profile.name,
                            unit.target_kind,
                            &unit.artifact_name,
                        )
                        .to_string_lossy()
                        .to_string(),
                        object_path: object_path(
                            &elaboration.resolved_graph.workspace_root,
                            unit.domain,
                            &unit.package_id,
                            &unit.profile.name,
                            unit.target_kind,
                            &unit.artifact_name,
                        )
                        .to_string_lossy()
                        .to_string(),
                        artifact_path: artifact_path(
                            &elaboration.resolved_graph.workspace_root,
                            unit.domain.select_target(&host_target, &target_target),
                            unit.domain,
                            &unit.package_id,
                            &unit.profile.name,
                            unit.target_kind,
                            &unit.artifact_name,
                        )
                        .to_string_lossy()
                        .to_string(),
                        metadata_path: (unit.target_kind == TargetKind::Lib).then(|| {
                            metadata_path(
                                &elaboration.resolved_graph.workspace_root,
                                unit.domain,
                                &unit.package_id,
                                &unit.profile.name,
                            )
                            .to_string_lossy()
                            .to_string()
                        }),
                    },
                    tools: package_tools.clone(),
                    package_root_path: package_root.clone(),
                    workspace_root_path: elaboration.resolved_graph.workspace_root.clone(),
                };
                script::apply_build_script(
                    &build_script.path,
                    &mut build_nodes,
                    unit,
                    &build_context,
                )?;
            }
        }
    }

    Ok(BuildPlan {
        workspace_root: elaboration.resolved_graph.workspace_root.clone(),
        build_nodes,
        packages,
    })
}

struct PackageDeriveSource<'a> {
    resolved: &'a ResolvedPackageNode,
    elaboration: &'a PackageElaboration,
    build_script: Option<BuildScriptInput>,
}

fn collect_host_local_packages(
    package_id: &PackageId,
    sources: &BTreeMap<PackageId, PackageDeriveSource<'_>>,
    into: &mut BTreeSet<PackageId>,
) {
    if !into.insert(package_id.clone()) {
        return;
    }
    let Some(source) = sources.get(package_id) else {
        return;
    };
    for dependency in &source.resolved.dependencies {
        if let ResolvedDependencyTarget::Local(target) = &dependency.target {
            collect_host_local_packages(target, sources, into);
        }
    }
}

fn build_package_for_domain(
    domain: BuildDomain,
    source: &PackageDeriveSource<'_>,
    workspace_root: &Path,
) -> PackageBuildPlan {
    let (
        unit_local_dependencies,
        unit_external_dependencies,
        build_local_dependencies,
        build_external_dependencies,
    ) = dependency_bindings_for_domain(domain, &source.resolved.dependencies);
    let self_lib_dependency = source
        .elaboration
        .plan
        .targets
        .iter()
        .any(|target| target.kind == TargetKind::Lib)
        .then(|| LocalDependencyBinding {
            domain,
            dependency_name: source.resolved.id.name.clone(),
            package_id: source.resolved.id.clone(),
        });
    let units = source
        .elaboration
        .plan
        .targets
        .iter()
        .filter(|target| include_target_in_domain(domain, target.kind))
        .map(|target| {
            let mut local_dependencies = unit_local_dependencies.clone();
            if target.kind != TargetKind::Lib
                && let Some(self_lib_dependency) = &self_lib_dependency
            {
                local_dependencies.push(self_lib_dependency.clone());
            }

            BuildUnit {
                domain,
                package_id: source.resolved.id.clone(),
                target_kind: target.kind,
                target_name: target.name.clone(),
                source_root: SourceRootBinding::PackagePath(target.root.clone()),
                artifact_kind: artifact_kind(target.kind),
                artifact_name: artifact_name(
                    &source.resolved.id,
                    target.kind,
                    target.name.as_deref(),
                ),
                local_dependencies,
                external_dependencies: unit_external_dependencies.clone(),
                profile: source.elaboration.profile.clone(),
                cfg: source.elaboration.plan.cfg.clone(),
                define: source.elaboration.plan.define.clone(),
                generated_files: Vec::new(),
                build: BuildNodeBindings::default(),
                link: LinkPlan::default(),
            }
        })
        .collect();

    let _ = workspace_root;
    PackageBuildPlan {
        domain,
        package_id: source.resolved.id.clone(),
        manifest_path: source.elaboration.plan.manifest_path.clone(),
        build_script: source.build_script.clone(),
        build_local_dependencies,
        build_external_dependencies,
        units,
    }
}

fn resolve_compile_source_input(
    package_root: &Path,
    source_root: &SourceRootBinding,
) -> CompileSourceInput {
    match source_root {
        SourceRootBinding::PackagePath(path) => {
            CompileSourceInput::PackagePath(package_root.join(path))
        }
        SourceRootBinding::AbsolutePath(path) => {
            CompileSourceInput::AbsolutePath(PathBuf::from(path))
        }
        SourceRootBinding::BuildOutput { id, path } => CompileSourceInput::BuildOutput {
            id: *id,
            path: PathBuf::from(path),
        },
    }
}

fn dependency_bindings_for_domain(
    domain: BuildDomain,
    dependencies: &[crate::resolver::ResolvedDependencyEdge],
) -> (
    Vec<LocalDependencyBinding>,
    Vec<ExternalDependencyBinding>,
    Vec<LocalDependencyBinding>,
    Vec<ExternalDependencyBinding>,
) {
    let mut unit_local_dependencies = Vec::new();
    let mut unit_external_dependencies = Vec::new();
    let mut build_local_dependencies = Vec::new();
    let mut build_external_dependencies = Vec::new();

    for dependency in dependencies {
        match &dependency.target {
            ResolvedDependencyTarget::Local(target) => {
                let binding = LocalDependencyBinding {
                    domain: if dependency.kind == crate::graph::DependencyKind::Build {
                        BuildDomain::Host
                    } else {
                        domain
                    },
                    dependency_name: dependency.dependency_name.clone(),
                    package_id: target.clone(),
                };
                if dependency.kind == crate::graph::DependencyKind::Build {
                    if !build_local_dependencies.contains(&binding) {
                        build_local_dependencies.push(binding);
                    }
                } else if !unit_local_dependencies.contains(&binding) {
                    unit_local_dependencies.push(binding);
                }
            }
            ResolvedDependencyTarget::External(target) => {
                let binding = ExternalDependencyBinding {
                    domain: if dependency.kind == crate::graph::DependencyKind::Build {
                        BuildDomain::Host
                    } else {
                        domain
                    },
                    dependency_name: dependency.dependency_name.clone(),
                    package_id: target.clone(),
                };
                if dependency.kind == crate::graph::DependencyKind::Build {
                    if !build_external_dependencies.contains(&binding) {
                        build_external_dependencies.push(binding);
                    }
                } else if !unit_external_dependencies.contains(&binding) {
                    unit_external_dependencies.push(binding);
                }
            }
        }
    }

    (
        unit_local_dependencies,
        unit_external_dependencies,
        build_local_dependencies,
        build_external_dependencies,
    )
}

fn include_target_in_domain(domain: BuildDomain, kind: TargetKind) -> bool {
    match domain {
        BuildDomain::Target => true,
        BuildDomain::Host => matches!(kind, TargetKind::Lib | TargetKind::Bin),
    }
}

fn script_context_for_instance(
    domain: BuildDomain,
    package_elaboration: &PackageElaboration,
    workspace_root: &Path,
    has_workspace: bool,
    command: crate::script::ScriptCommand,
    host: &script::ScriptTarget,
    target: &script::ScriptTarget,
) -> script::ScriptContext {
    let package_root = package_elaboration
        .plan
        .manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    script::ScriptContext {
        package: script::ScriptPackage {
            name: package_elaboration.package_id.name.clone(),
            version: package_elaboration.package_id.version.clone(),
            root: relative_display(workspace_root, package_root),
            is_root: package_elaboration.package_id.source == crate::graph::SourceId::Root,
        },
        workspace: script::ScriptWorkspace {
            root: relative_display(workspace_root, workspace_root),
            has_workspace,
        },
        host: host.clone(),
        target: domain.select_target(host, target).clone(),
        profile: package_elaboration.profile.clone(),
        command,
        features: package_elaboration.selected_features.clone(),
        env: BTreeMap::new(),
    }
}

fn build_tool_index(
    packages: &[PackageBuildPlan],
    workspace_root: &Path,
    host: &script::ScriptTarget,
    target: &script::ScriptTarget,
) -> BTreeMap<PackageId, Vec<script::BuildScriptTool>> {
    let mut index = BTreeMap::new();
    for package in packages {
        if package.domain != BuildDomain::Host {
            continue;
        }
        let mut tools = Vec::new();
        for unit in &package.units {
            if unit.target_kind != TargetKind::Bin {
                continue;
            }
            tools.push(script::BuildScriptTool {
                target_name: unit
                    .target_name
                    .clone()
                    .expect("bin target must provide a name"),
                executable_path: artifact_path(
                    workspace_root,
                    unit.domain.select_target(host, target),
                    unit.domain,
                    &unit.package_id,
                    &unit.profile.name,
                    unit.target_kind,
                    &unit.artifact_name,
                )
                .to_string_lossy()
                .to_string(),
                origin: script::BuildScriptToolOrigin::LocalPackage {
                    package_id: package.package_id.clone(),
                },
            });
        }
        if !tools.is_empty() {
            index.insert(package.package_id.clone(), tools);
        }
    }
    index
}

fn build_external_tool_index(
    packages: &[PackageBuildPlan],
    workspace_root: &Path,
    host: &script::ScriptTarget,
    manifest_path: &Path,
    manifest: &Manifest,
    profile_selection: script::ProfileSelection,
) -> Result<BTreeMap<ExternalPackageId, Vec<script::BuildScriptTool>>> {
    let external_packages = packages
        .iter()
        .filter(|package| package.build_script.is_some())
        .flat_map(|package| package.build_external_dependencies.iter())
        .map(|binding| binding.package_id.clone())
        .collect::<BTreeSet<_>>();
    if external_packages.is_empty() {
        return Ok(BTreeMap::new());
    }

    let resolved = ResolvedGraph {
        workspace_root: workspace_root.to_path_buf(),
        packages: Vec::new(),
        external_packages: external_packages
            .into_iter()
            .map(|id| ResolvedExternalPackage { id })
            .collect(),
    };

    let mut index = BTreeMap::new();
    for fetched in source::fetch_external_packages(manifest_path, manifest, &resolved)? {
        let external_manifest_path = fetched.cache_path.join("Craft.toml");
        let external_manifest = Manifest::load(&external_manifest_path)?;
        external_manifest.validate(&external_manifest_path)?;
        let Some(package) = &external_manifest.package else {
            continue;
        };
        let package_id = PackageId {
            name: package.name.clone(),
            version: package.version.clone(),
            source: fetched.id.source.clone(),
        };
        let plan =
            PackagePlan::from_manifest(&external_manifest_path, &package_id, &external_manifest)?;
        let profile = script::manifest_profile(&external_manifest, profile_selection);
        let mut tools = Vec::new();
        for target_plan in plan
            .targets
            .iter()
            .filter(|target| target.kind == TargetKind::Bin)
        {
            let target_name = target_plan
                .name
                .clone()
                .expect("bin targets must provide a name");
            tools.push(script::BuildScriptTool {
                executable_path: artifact_path(
                    &fetched.cache_path,
                    host,
                    BuildDomain::Target,
                    &package_id,
                    &profile.name,
                    TargetKind::Bin,
                    &target_name,
                )
                .to_string_lossy()
                .to_string(),
                origin: script::BuildScriptToolOrigin::ExternalPackage {
                    dependency_id: fetched.id.clone(),
                    package_id: package_id.clone(),
                },
                target_name,
            });
        }
        if !tools.is_empty() {
            index.insert(fetched.id, tools);
        }
    }

    Ok(index)
}

fn build_tools_for_package(
    package: &PackageBuildPlan,
    tool_index: &BTreeMap<PackageId, Vec<script::BuildScriptTool>>,
    external_tool_index: &BTreeMap<ExternalPackageId, Vec<script::BuildScriptTool>>,
) -> BTreeMap<String, Vec<script::BuildScriptTool>> {
    let mut tools = BTreeMap::new();
    for binding in &package.build_local_dependencies {
        if let Some(entries) = tool_index.get(&binding.package_id) {
            tools.insert(binding.dependency_name.clone(), entries.clone());
        }
    }
    for binding in &package.build_external_dependencies {
        if let Some(entries) = external_tool_index.get(&binding.package_id) {
            tools.insert(binding.dependency_name.clone(), entries.clone());
        }
    }
    tools
}

fn discover_build_script(
    workspace_root: &Path,
    package_root: &Path,
) -> Result<Option<BuildScriptInput>> {
    let path = package_root.join("build.rn");
    if !path.is_file() {
        return Ok(None);
    }

    script::validate_build_script(&path)?;

    Ok(Some(BuildScriptInput {
        relative_path: relative_display(workspace_root, &path),
        path,
    }))
}

fn artifact_kind(kind: TargetKind) -> ArtifactKind {
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

    fn select_target<'a>(
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

fn object_path(
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

fn workspace_build_root(workspace_root: &Path, profile: &str, domain: BuildDomain) -> PathBuf {
    workspace_root
        .join(".craft")
        .join("build")
        .join(profile)
        .join(domain.as_str())
}

fn generated_root_path(
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

fn artifact_root_path(
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

fn artifact_path(
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

fn metadata_path(
    workspace_root: &Path,
    domain: BuildDomain,
    package_id: &PackageId,
    profile: &str,
) -> PathBuf {
    workspace_build_root(workspace_root, profile, domain)
        .join("meta")
        .join(package_dir_name(package_id))
}

fn package_dir_name(package_id: &PackageId) -> String {
    format!("{}-{}", package_id.name, package_id.version)
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

fn resolve_staged_action(workspace_root: &Path, action: &StagedAction) -> StagedAction {
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
            StagedActionKind::RunTool { tool, args } => StagedActionKind::RunTool {
                tool: script::BuildScriptTool {
                    target_name: tool.target_name.clone(),
                    executable_path: workspace_root
                        .join(&tool.executable_path)
                        .to_string_lossy()
                        .to_string(),
                    origin: tool.origin.clone(),
                },
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

fn artifact_name(package_id: &PackageId, kind: TargetKind, target_name: Option<&str>) -> String {
    match kind {
        TargetKind::Lib => package_id.name.clone(),
        TargetKind::Bin | TargetKind::Test | TargetKind::Example => target_name
            .expect("named targets must provide a name")
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ArtifactKind, GeneratedFileOrigin, SourceRootBinding, StagedActionKind, StagedActionPhase,
        artifact_path, derive,
    };
    use crate::elaborate::plan;
    use crate::graph::PackageId;
    use crate::manifest::Manifest;
    use crate::plan::TargetKind;
    use crate::script::ScriptOs;
    use crate::workspace::load_members;
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

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

    #[test]
    fn derives_workspace_build_units_from_package_targets() {
        let root = temp_dir("craft-build-plan-targets");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
members = ["app"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[lib]
root = "src/lib.rn"

[[bin]]
name = "app"
root = "src/main.rn"

[[test]]
name = "smoke"
root = "tests/smoke.rn"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Check,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let build_plan = derive(&elaboration, crate::script::ScriptCommand::Check).unwrap();
        let actions = build_plan.derive_actions(&crate::script::host_target());

        assert_eq!(build_plan.unit_count(), 3);
        assert_eq!(actions.compile_count(), 3);
        assert_eq!(actions.link_count(), 2);
        let app_package = build_plan
            .packages
            .iter()
            .find(|package| package.package_id.name == "app")
            .unwrap();
        assert!(app_package.units.iter().any(|unit| {
            unit.target_kind == TargetKind::Lib
                && unit.artifact_kind == ArtifactKind::Library
                && unit.artifact_name == "app"
        }));
        assert!(app_package.units.iter().any(|unit| {
            unit.target_kind == TargetKind::Bin
                && unit.artifact_kind == ArtifactKind::Executable
                && unit.artifact_name == "app"
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn carries_local_and_external_dependencies_into_build_units() {
        let root = temp_dir("craft-build-plan-deps");
        let app_dir = root.join("app");
        let util_dir = root.join("util");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&util_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
members = ["app", "util"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "app"
root = "src/main.rn"

[dependencies]
util = { path = "../util" }
log = "1"
"#,
        )
        .unwrap();
        fs::write(
            util_dir.join("Craft.toml"),
            r#"
[package]
name = "util"
version = "0.1.0"
kern = "0.7"

[lib]
root = "src/lib.rn"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Check,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let build_plan = derive(&elaboration, crate::script::ScriptCommand::Check).unwrap();
        let actions = build_plan.derive_actions(&crate::script::host_target());

        let app_unit = build_plan
            .packages
            .iter()
            .find(|package| package.package_id.name == "app")
            .unwrap()
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Bin)
            .unwrap();
        assert_eq!(app_unit.local_dependencies.len(), 1);
        assert_eq!(app_unit.local_dependencies[0].dependency_name, "util");
        assert_eq!(app_unit.local_dependencies[0].package_id.name, "util");
        assert_eq!(app_unit.external_dependencies.len(), 1);
        assert_eq!(app_unit.external_dependencies[0].dependency_name, "log");
        assert_eq!(
            app_unit.external_dependencies[0].package_id.package_name,
            "log"
        );
        assert_eq!(build_plan.local_dependency_edge_count(), 1);
        assert_eq!(build_plan.external_dependency_edge_count(), 1);
        let link = actions
            .link_actions
            .iter()
            .find(|action| action.package_id.name == "app" && action.target_kind == TargetKind::Bin)
            .unwrap();
        assert_eq!(link.local_library_objects.len(), 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn isolates_build_dependencies_from_target_units() {
        let root = temp_dir("craft-build-plan-build-deps");
        let app_dir = root.join("app");
        let util_dir = root.join("util");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&util_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
members = ["app", "util"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "app"
root = "src/main.rn"

[dependencies]
util = { path = "../util" }
log = "1"

[build-dependencies]
util_build = { path = "../util", package = "util" }
cc = "1"
"#,
        )
        .unwrap();
        fs::write(
            util_dir.join("Craft.toml"),
            r#"
[package]
name = "util"
version = "0.1.0"
kern = "0.7"

[lib]
root = "src/lib.rn"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Check,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let build_plan = derive(&elaboration, crate::script::ScriptCommand::Check).unwrap();
        let app_package = build_plan
            .packages
            .iter()
            .find(|package| package.package_id.name == "app")
            .unwrap();
        let app_unit = app_package
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Bin)
            .unwrap();

        assert_eq!(app_unit.domain, crate::graph::BuildDomain::Target);
        assert_eq!(app_unit.local_dependencies.len(), 1);
        assert_eq!(app_unit.local_dependencies[0].dependency_name, "util");
        assert_eq!(app_unit.external_dependencies.len(), 1);
        assert_eq!(app_unit.external_dependencies[0].dependency_name, "log");

        assert_eq!(app_package.build_local_dependencies.len(), 1);
        assert_eq!(
            app_package.build_local_dependencies[0].dependency_name,
            "util_build"
        );
        assert_eq!(app_package.build_external_dependencies.len(), 1);
        assert_eq!(
            app_package.build_external_dependencies[0].dependency_name,
            "cc"
        );
        assert_eq!(build_plan.local_dependency_edge_count(), 1);
        assert_eq!(build_plan.external_dependency_edge_count(), 1);
        assert_eq!(build_plan.build_local_dependency_edge_count(), 1);
        assert_eq!(build_plan.build_external_dependency_edge_count(), 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_dependencies_create_host_tool_instances() {
        let root = temp_dir("craft-build-plan-host-tools");
        let app_dir = root.join("app");
        let tool_dir = root.join("tool");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&tool_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
members = ["app", "tool"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "app"
root = "src/main.rn"

[build-dependencies]
codegen = { path = "../tool", package = "tool" }
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("build.rn"),
            r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    b.define_string("tool_path", b.tool_path("codegen", "codegen"));
}
"#,
        )
        .unwrap();
        fs::write(
            tool_dir.join("Craft.toml"),
            r#"
[package]
name = "tool"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "codegen"
root = "src/main.rn"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Build,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let host_tool_package = build_plan
            .packages
            .iter()
            .find(|package| {
                package.domain == crate::graph::BuildDomain::Host
                    && package.package_id.name == "tool"
            })
            .unwrap();
        assert!(host_tool_package.units.iter().any(|unit| {
            unit.domain == crate::graph::BuildDomain::Host
                && unit.target_kind == TargetKind::Bin
                && unit.target_name.as_deref() == Some("codegen")
        }));

        let app_package = build_plan
            .packages
            .iter()
            .find(|package| {
                package.domain == crate::graph::BuildDomain::Target
                    && package.package_id.name == "app"
            })
            .unwrap();
        let app_unit = app_package
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Bin)
            .unwrap();
        let expected_tool_path = artifact_path(
            &build_plan.workspace_root,
            &crate::script::host_target(),
            crate::graph::BuildDomain::Host,
            &host_tool_package.package_id,
            &host_tool_package.units[0].profile.name,
            TargetKind::Bin,
            "codegen",
        )
        .to_string_lossy()
        .to_string();
        assert_eq!(
            app_unit.define.get("tool_path"),
            Some(&crate::plan::PlanValue::String(expected_tool_path))
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_tool_lookup_supports_explicit_named_tools() {
        let root = temp_dir("craft-build-plan-named-tools");
        let app_dir = root.join("app");
        let tool_dir = root.join("tool");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&tool_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
members = ["app", "tool"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "app"
root = "src/main.rn"

[build-dependencies]
tools = { path = "../tool", package = "tool" }
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("build.rn"),
            r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    b.define_string("selected_tool", b.tool_path("tools", "beta"));
}
"#,
        )
        .unwrap();
        fs::write(
            tool_dir.join("Craft.toml"),
            r#"
[package]
name = "tool"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "alpha"
root = "src/alpha.rn"

[[bin]]
name = "beta"
root = "src/beta.rn"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Build,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let host_tool_package = build_plan
            .packages
            .iter()
            .find(|package| {
                package.domain == crate::graph::BuildDomain::Host
                    && package.package_id.name == "tool"
            })
            .unwrap();
        let expected_tool_path = artifact_path(
            &build_plan.workspace_root,
            &crate::script::host_target(),
            crate::graph::BuildDomain::Host,
            &host_tool_package.package_id,
            &host_tool_package.units[0].profile.name,
            TargetKind::Bin,
            "beta",
        )
        .to_string_lossy()
        .to_string();
        let app_unit = build_plan
            .packages
            .iter()
            .find(|package| {
                package.domain == crate::graph::BuildDomain::Target
                    && package.package_id.name == "app"
            })
            .unwrap()
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Bin)
            .unwrap();

        assert_eq!(
            app_unit.define.get("selected_tool"),
            Some(&crate::plan::PlanValue::String(expected_tool_path))
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_tool_lookup_supports_external_build_dependency_tools() {
        let root = temp_dir("craft-build-plan-external-tools");
        let registry_root = root.join("vendor-registry");
        let tool_root = registry_root.join("codegen").join("1");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(&tool_root).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "app"
root = "src/main.rn"

[build-dependencies]
codegen = "1"

[source.default]
directory = "vendor-registry"
"#,
        )
        .unwrap();
        fs::write(
            root.join("build.rn"),
            r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    b.define_string("selected_tool", b.tool_path("codegen", "codegen"));
}
"#,
        )
        .unwrap();
        fs::write(
            tool_root.join("Craft.toml"),
            r#"
[package]
name = "codegen"
version = "1"
kern = "0.7"

[[bin]]
name = "codegen"
root = "src/main.rn"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Build,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let unit = build_plan.packages[0]
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Bin)
            .unwrap();
        let expected_tool_path = artifact_path(
            &root
                .join(".craft")
                .join("sources")
                .join("registry")
                .join("default")
                .join("codegen")
                .join("1"),
            &crate::script::host_target(),
            crate::graph::BuildDomain::Target,
            &PackageId {
                name: "codegen".to_string(),
                version: "1".to_string(),
                source: crate::graph::SourceId::Registry { name: None },
            },
            "dev",
            TargetKind::Bin,
            "codegen",
        )
        .to_string_lossy()
        .to_string();

        assert_eq!(
            unit.define.get("selected_tool"),
            Some(&crate::plan::PlanValue::String(expected_tool_path))
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn preserves_dependency_aliases_in_build_units() {
        let root = temp_dir("craft-build-plan-alias");
        let app_dir = root.join("app");
        let util_dir = root.join("util");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&util_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
members = ["app", "util"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "app"
root = "src/main.rn"

[dependencies]
foo = { path = "../util", package = "util" }
"#,
        )
        .unwrap();
        fs::write(
            util_dir.join("Craft.toml"),
            r#"
[package]
name = "util"
version = "0.1.0"
kern = "0.7"

[lib]
root = "src/lib.rn"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Check,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let build_plan = derive(&elaboration, crate::script::ScriptCommand::Check).unwrap();

        let app_unit = build_plan
            .packages
            .iter()
            .find(|package| package.package_id.name == "app")
            .unwrap()
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Bin)
            .unwrap();

        assert_eq!(app_unit.local_dependencies.len(), 1);
        assert_eq!(app_unit.local_dependencies[0].dependency_name, "foo");
        assert_eq!(app_unit.local_dependencies[0].package_id.name, "util");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn applies_build_script_link_directives_per_unit() {
        let root = temp_dir("craft-build-plan-script");
        let os_variant = os_variant_name(crate::script::host_target().os);
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"

[features]
default = ["simd"]
simd = []

[[bin]]
name = "demo"
root = "src/main.rn"

[[test]]
name = "smoke"
root = "tests/smoke.rn"
"#,
        )
        .unwrap();
        fs::write(
            root.join("build.rn"),
            format!(
                r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {{
    if (b.feature_enabled("simd")) {{
        b.link_arg("-flto");
    }}

    if (b.target.os == .{os_variant}) {{
        b.link_arg("-Dtarget-os-match");
    }}

    if (b.unit.kind == .bin) {{
        b.link_framework("Security");
    }}

    if (b.unit.kind == .test) {{
        b.link_search("native/test");
    }}
}}
"#
            ),
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Build,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let actions = build_plan.derive_actions(&crate::script::host_target());
        let package = build_plan
            .packages
            .iter()
            .find(|package| package.package_id.name == "demo")
            .unwrap();
        assert_eq!(
            package
                .build_script
                .as_ref()
                .map(|script| script.relative_path.as_str()),
            Some("build.rn")
        );

        let bin = package
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Bin)
            .unwrap();
        assert!(bin.link.args.iter().any(|arg| arg == "-flto"));
        assert!(bin.link.args.iter().any(|arg| arg == "-Dtarget-os-match"));
        assert!(bin.link.frameworks.iter().any(|name| name == "Security"));

        let test = package
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Test)
            .unwrap();
        assert!(test.link.args.iter().any(|arg| arg == "-flto"));
        assert!(
            test.link
                .search_paths
                .iter()
                .any(|path| path == "native/test")
        );
        let bin_action = actions
            .link_actions
            .iter()
            .find(|action| {
                action.package_id.name == "demo" && action.target_kind == TargetKind::Bin
            })
            .unwrap();
        assert!(
            bin_action
                .link
                .frameworks
                .iter()
                .any(|name| name == "Security")
        );
        assert!(bin_action.link.args.iter().any(|arg| arg == "-flto"));
        let test_action = actions
            .link_actions
            .iter()
            .find(|action| {
                action.package_id.name == "demo" && action.target_kind == TargetKind::Test
            })
            .unwrap();
        assert!(
            test_action
                .link
                .search_paths
                .iter()
                .any(|path| path == "native/test")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_script_can_generate_sources_and_mutate_unit_cfg_define() {
        let root = temp_dir("craft-build-plan-generated");
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "demo"
root = "src/placeholder.rn"
"#,
        )
        .unwrap();
        fs::write(
            root.join("build.rn"),
            r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    let root = b.emit_generated(
        "src/main.rn",
        "extern fn main(args: [][]u8) i32 { let _ = args; return 0; }\n"
    );
    b.set_source_root(root);
    b.cfg_bool("generated", true);
    b.define_string("entry", "generated");
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Build,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let unit = build_plan.packages[0]
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Bin)
            .unwrap();
        let unit_nodes = build_plan.compile_input_nodes_for_unit(unit);

        let SourceRootBinding::AbsolutePath(source_root) = &unit.source_root else {
            panic!("expected generated source root to be an absolute path binding");
        };
        assert!(
            Path::new(source_root).is_absolute(),
            "expected generated source root to be absolute: {}",
            source_root
        );
        assert!(!Path::new(source_root).exists());
        assert_eq!(
            unit.cfg.get("generated"),
            Some(&crate::plan::PlanValue::Bool(true))
        );
        assert_eq!(
            unit.define.get("entry"),
            Some(&crate::plan::PlanValue::String("generated".to_string()))
        );
        assert_eq!(unit.generated_files.len(), 1);
        assert_eq!(unit.generated_files[0].origin, GeneratedFileOrigin::Emitted);
        assert_eq!(unit_nodes.len(), 1);
        assert!(matches!(
            &unit_nodes[0].kind,
            StagedActionKind::WriteFile { .. }
        ));
        assert_eq!(unit_nodes[0].phase, StagedActionPhase::PreCompile);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_script_can_copy_package_files_into_generated_root() {
        let root = temp_dir("craft-build-plan-copy");
        fs::create_dir_all(root.join("templates")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "demo"
root = "src/placeholder.rn"
"#,
        )
        .unwrap();
        fs::write(
            root.join("templates").join("main.rn"),
            "extern fn main(args: [][]u8) i32 { let _ = args; return 0; }\n",
        )
        .unwrap();
        fs::write(
            root.join("build.rn"),
            r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    let root = b.copy_package_file("templates/main.rn", "src/main.rn");
    b.set_source_root(root);
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Build,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let unit = build_plan.packages[0]
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Bin)
            .unwrap();
        let unit_nodes = build_plan.compile_input_nodes_for_unit(unit);

        assert_eq!(unit.generated_files.len(), 1);
        assert_eq!(
            unit.generated_files[0].origin,
            GeneratedFileOrigin::Copied {
                source: "templates/main.rn".to_string()
            }
        );
        assert_eq!(unit_nodes.len(), 1);
        assert!(matches!(
            &unit_nodes[0].kind,
            StagedActionKind::CopyFile { source } if source == "templates/main.rn"
        ));
        assert_eq!(unit_nodes[0].phase, StagedActionPhase::PreCompile);
        let SourceRootBinding::AbsolutePath(source_root) = &unit.source_root else {
            panic!("expected copied generated source root to be an absolute path binding");
        };
        assert!(!Path::new(source_root).exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_script_can_model_explicit_staged_dependencies() {
        let root = temp_dir("craft-build-plan-staged-deps");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "demo"
root = "src/placeholder.rn"
"#,
        )
        .unwrap();
        fs::write(
            root.join("build.rn"),
            r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    let helper = b.stage_generated("tmp/value.txt", "41\n");
    let source = b.stage_generated("src/main.rn", "extern fn main(args: [][]u8) i32 { let _ = args; return 0; }\n");
    b.depend(source, helper);
    b.set_source_root_from(source);
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Build,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let unit = build_plan.packages[0]
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Bin)
            .unwrap();
        let unit_nodes = build_plan.compile_input_nodes_for_unit(unit);

        assert_eq!(unit_nodes.len(), 2);
        let helper = unit_nodes
            .iter()
            .find(|action| action.output.ends_with("tmp/value.txt"))
            .unwrap();
        let source = unit_nodes
            .iter()
            .find(|action| action.output.ends_with("src/main.rn"))
            .unwrap();
        assert_eq!(source.depends_on, vec![helper.id]);
        assert!(matches!(
            &unit.source_root,
            SourceRootBinding::BuildOutput { id, path }
                if *id == source.id && path.replace('\\', "/").ends_with("src/main.rn")
        ));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_script_can_stage_post_link_artifact_outputs() {
        let root = temp_dir("craft-build-plan-post-link");
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
        )
        .unwrap();
        fs::write(
            root.join("assets").join("config.json"),
            "{ \"mode\": \"demo\" }\n",
        )
        .unwrap();
        fs::write(
            root.join("build.rn"),
            r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    let _ = b.copy_package_file_to_artifact("assets/config.json", "config/config.json");
    let _ = b.emit_artifact_file("notes/build.txt", "built by craft\n");
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Build,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let unit = build_plan.packages[0]
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Bin)
            .unwrap();
        let link_action = action_plan
            .link_actions
            .iter()
            .find(|action| {
                action.package_id.name == "demo" && action.target_kind == TargetKind::Bin
            })
            .unwrap();
        let unit_nodes = build_plan.artifact_output_nodes_for_unit(unit);
        let link_nodes = action_plan.artifact_output_nodes_for_link_action(link_action);

        assert_eq!(unit_nodes.len(), 2);
        assert!(
            unit_nodes
                .iter()
                .all(|action| action.phase == StagedActionPhase::PostLink)
        );
        assert_eq!(link_nodes.len(), 2);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_script_can_stage_post_link_directory_copies() {
        let root = temp_dir("craft-build-plan-post-link-dir");
        fs::create_dir_all(root.join("assets").join("images")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
        )
        .unwrap();
        fs::write(
            root.join("assets").join("config.json"),
            "{ \"mode\": \"demo\" }\n",
        )
        .unwrap();
        fs::write(
            root.join("assets").join("images").join("logo.txt"),
            "logo\n",
        )
        .unwrap();
        fs::write(
            root.join("build.rn"),
            r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    let _ = b.copy_package_dir_to_artifact("assets", "bundle/assets");
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Build,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let unit = build_plan.packages[0]
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Bin)
            .unwrap();
        let link_action = action_plan
            .link_actions
            .iter()
            .find(|action| {
                action.package_id.name == "demo" && action.target_kind == TargetKind::Bin
            })
            .unwrap();
        let unit_nodes = build_plan.artifact_output_nodes_for_unit(unit);
        let link_nodes = action_plan.artifact_output_nodes_for_link_action(link_action);

        assert_eq!(unit_nodes.len(), 1);
        assert!(matches!(
            &unit_nodes[0].kind,
            StagedActionKind::CopyDirectory { source } if source == "assets"
        ));
        assert_eq!(unit_nodes[0].phase, StagedActionPhase::PostLink);
        assert_eq!(link_nodes.len(), 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_script_receives_host_target_and_domain_context() {
        let root = temp_dir("craft-build-plan-domain-context");
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
        )
        .unwrap();
        fs::write(
            root.join("build.rn"),
            r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    b.define_string("host_arch", b.host.arch);
    b.define_string("target_arch", b.target.arch);

    match (b.unit.domain) {
        .host => b.link_arg("-host-unit"),
        .target => b.link_arg("-target-unit"),
    }
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Build,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let unit = build_plan.packages[0]
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Bin)
            .unwrap();

        assert_eq!(unit.domain, crate::graph::BuildDomain::Target);
        assert_eq!(
            unit.define.get("host_arch"),
            Some(&crate::plan::PlanValue::String(
                crate::script::host_target().arch.to_string()
            ))
        );
        assert_eq!(
            unit.define.get("target_arch"),
            Some(&crate::plan::PlanValue::String(
                crate::script::host_target().arch.to_string()
            ))
        );
        assert!(unit.link.args.iter().any(|arg| arg == "-target-unit"));

        let _ = fs::remove_dir_all(root);
    }
}
