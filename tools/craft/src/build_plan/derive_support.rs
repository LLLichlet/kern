use super::{
    BuildNodeBindings, BuildPlan, BuildScriptInput, BuildUnit, CompileSourceInput, DeriveOptions,
    ExternalDependencyBinding, LinkPlan, LocalDependencyBinding, PackageBuildPlan,
    SourceRootBinding, artifact_kind, artifact_name, artifact_path, artifact_root_path,
    generated_root_path, metadata_path, object_path, relative_display, workspace_build_root,
};
use crate::elaborate::{ElaborationPlan, PackageElaboration};
use crate::error::Result;
use crate::graph::{BuildDomain, PackageId};
use crate::manifest::Manifest;
use crate::plan::{PackagePlan, TargetKind};
use crate::resolver::{
    ExternalPackageId, ResolvedDependencyTarget, ResolvedExternalPackage, ResolvedGraph,
    ResolvedPackageNode,
};
use crate::script;
use crate::source;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn normalized_path_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub fn derive(
    elaboration: &ElaborationPlan,
    command: crate::script::ScriptCommand,
) -> Result<BuildPlan> {
    derive_with_options(elaboration, command, DeriveOptions::default())
}

pub fn derive_with_options(
    elaboration: &ElaborationPlan,
    command: crate::script::ScriptCommand,
    options: DeriveOptions,
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
            command,
            BuildDomain::Target,
            source,
            &elaboration.resolved_graph.workspace_root,
            options,
        ));
    }
    for package_id in &host_packages {
        let source = sources
            .get(package_id)
            .expect("host closure package must exist in source map");
        packages.push(build_package_for_domain(
            command,
            BuildDomain::Host,
            source,
            &elaboration.resolved_graph.workspace_root,
            options,
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
    let external_tool_index = build_external_tool_index(
        &packages,
        &elaboration.resolved_graph.workspace_root,
        &host_target,
        &elaboration.resolved_graph.workspace_root.join("Craft.toml"),
        &elaboration.manifest,
        elaboration.profile_selection,
    )?;
    let resource_index = build_resource_index(elaboration)?;
    for package in &mut packages {
        let package_root = package
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let package_tools = build_tools_for_package(package, &tool_index, &external_tool_index);
        let package_resources = resource_index
            .get(&package.package_id)
            .cloned()
            .unwrap_or_default();
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
                        build_root: normalized_path_string(&workspace_build_root(
                            &elaboration.resolved_graph.workspace_root,
                            &unit.profile.name,
                            unit.domain,
                        )),
                        generated_root: normalized_path_string(&generated_root_path(
                            &elaboration.resolved_graph.workspace_root,
                            unit.domain,
                            &unit.package_id,
                            &unit.profile.name,
                            unit.target_kind,
                            &unit.artifact_name,
                        )),
                        artifact_root: normalized_path_string(&artifact_root_path(
                            &elaboration.resolved_graph.workspace_root,
                            unit.domain,
                            &unit.package_id,
                            &unit.profile.name,
                            unit.target_kind,
                            &unit.artifact_name,
                        )),
                        object_path: normalized_path_string(&object_path(
                            &elaboration.resolved_graph.workspace_root,
                            unit.domain,
                            &unit.package_id,
                            &unit.profile.name,
                            unit.target_kind,
                            &unit.artifact_name,
                        )),
                        artifact_path: normalized_path_string(&artifact_path(
                            &elaboration.resolved_graph.workspace_root,
                            unit.domain.select_target(&host_target, &target_target),
                            unit.domain,
                            &unit.package_id,
                            &unit.profile.name,
                            unit.target_kind,
                            &unit.artifact_name,
                        )),
                        metadata_path: (unit.target_kind == TargetKind::Lib).then(|| {
                            normalized_path_string(&metadata_path(
                                &elaboration.resolved_graph.workspace_root,
                                unit.domain,
                                &unit.package_id,
                                &unit.profile.name,
                            ))
                        }),
                    },
                    tools: package_tools.clone(),
                    resources: package_resources.clone(),
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
    command: crate::script::ScriptCommand,
    domain: BuildDomain,
    source: &PackageDeriveSource<'_>,
    workspace_root: &Path,
    options: DeriveOptions,
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
        .filter(|target| include_target_in_domain(command, domain, target.kind, options))
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
                package_root_path: source
                    .elaboration
                    .plan
                    .manifest_path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf(),
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

pub(super) fn resolve_compile_source_input(
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

fn include_target_in_domain(
    command: crate::script::ScriptCommand,
    domain: BuildDomain,
    kind: TargetKind,
    options: DeriveOptions,
) -> bool {
    match domain {
        BuildDomain::Target => match command {
            crate::script::ScriptCommand::Build => {
                matches!(kind, TargetKind::Lib | TargetKind::Bin)
                    || (options.include_examples && kind == TargetKind::Example)
            }
            crate::script::ScriptCommand::Run => {
                if options.include_examples {
                    matches!(kind, TargetKind::Lib | TargetKind::Example)
                } else {
                    matches!(kind, TargetKind::Lib | TargetKind::Bin)
                }
            }
            crate::script::ScriptCommand::Test => {
                matches!(kind, TargetKind::Lib | TargetKind::Test)
            }
            _ => true,
        },
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
            root: absolute_script_path(package_root),
            is_root: package_elaboration.package_id.source == crate::graph::SourceId::Root,
        },
        workspace: script::ScriptWorkspace {
            root: absolute_script_path(workspace_root),
            has_workspace,
        },
        host: host.clone(),
        target: domain.select_target(host, target).clone(),
        profile: package_elaboration.profile.clone(),
        command,
        features: package_elaboration.selected_features.clone(),
    }
}

fn absolute_script_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
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
    let _ = manifest_path;
    let _ = manifest;
    for fetched in source::fetch_external_packages(&resolved)? {
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

fn build_resource_index(
    elaboration: &ElaborationPlan,
) -> Result<BTreeMap<PackageId, BTreeMap<String, script::BuildScriptResource>>> {
    let mut index = BTreeMap::new();
    for fetched in source::fetch_package_resources(elaboration)? {
        index
            .entry(fetched.id.package_id)
            .or_insert_with(BTreeMap::new)
            .insert(
                fetched.id.name,
                script::BuildScriptResource {
                    root_path: normalized_path_string(&fetched.cache_path),
                },
            );
    }
    Ok(index)
}

fn discover_build_script(
    workspace_root: &Path,
    package_root: &Path,
) -> Result<Option<BuildScriptInput>> {
    let path = package_root.join("build.kn");
    if !path.is_file() {
        return Ok(None);
    }

    script::validate_build_script(&path)?;

    Ok(Some(BuildScriptInput {
        relative_path: relative_display(workspace_root, &path),
        path,
    }))
}
