use super::{
    ActionIndexes, ActionKey, BuiltExternalPackage, BuiltStdPackage, ExecutionConfig,
    ExecutionSession, ExecutionState, ExecutionSummary, ExternalArtifacts, ExternalToolKey,
    LoadedExternalPackage, PackageInstanceKey, Result, SourceConfigContext,
    ensure_compile_action_built, ensure_link_action_built, ensure_std_packages_for_actions,
    linker_input_paths_for_primary_output, push_link_object, runtime_profile_key,
    validate_package_metadata_root,
};
use crate::build_plan::{ActionPlan, CompileAction, LinkAction};
use crate::elaborate::{self, FeatureSelection};
use crate::error::Error;
use crate::graph::BuildDomain;
use crate::manifest::Manifest;
use crate::resolver::{ExternalPackageId, ResolvedExternalPackage, ResolvedGraph};
use crate::source;
use crate::target_defaults::apply_target_runtime_defaults;
use crate::workspace;
use kernc_utils::config::{CompileOptions, LibraryBundle, RuntimeEntry};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use super::options::apply_manifest_runtime_options;

fn push_linker_inputs_for_primary_output(
    objects: &mut Vec<PathBuf>,
    seen: &mut BTreeSet<PathBuf>,
    primary_output: &Path,
) -> Result<()> {
    for object in linker_input_paths_for_primary_output(primary_output)? {
        push_link_object(objects, seen, &object);
    }
    Ok(())
}

pub(super) fn compile_module_aliases(
    action: &CompileAction,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    std_package: Option<&BuiltStdPackage>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
) -> Result<HashMap<String, String>> {
    let aliases = module_alias_paths(
        action,
        local_library_actions,
        std_package,
        built_external_packages,
    )?;
    Ok(aliases
        .into_iter()
        .map(|(name, path)| (name, path.to_string_lossy().to_string()))
        .collect())
}

pub(super) fn requested_external_dependencies(action_plan: &ActionPlan) -> Vec<ExternalPackageId> {
    let mut requested = BTreeSet::new();
    for action in &action_plan.compile_actions {
        requested.extend(
            action
                .external_dependencies
                .iter()
                .map(|binding| binding.package_id.clone()),
        );
    }
    for action in &action_plan.link_actions {
        requested.extend(
            action
                .external_dependencies
                .iter()
                .map(|binding| binding.package_id.clone()),
        );
    }
    requested.into_iter().collect()
}

pub(super) fn load_external_package_actions(
    source_config: &SourceConfigContext,
    dependency_workspace_root: &Path,
    dep: &ExternalPackageId,
    command: crate::script::ScriptCommand,
    profile_selection: crate::script::ProfileSelection,
) -> Result<LoadedExternalPackage> {
    let fetched = fetch_external_package(source_config, dependency_workspace_root, dep)?;
    let manifest_path = fetched.cache_path.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path)?;
    manifest.validate(&manifest_path)?;
    let workspace_members = workspace::load_members(&manifest_path, &manifest)?;
    let elaboration = elaborate::plan(
        &manifest_path,
        &manifest,
        &workspace_members,
        manifest.workspace.is_some(),
        command,
        &FeatureSelection {
            profile: profile_selection,
            ..Default::default()
        },
    )?;
    let build_plan = crate::build_plan::derive(&elaboration, command)?;
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let compile_action_index = compile_actions_index(&action_plan.compile_actions);
    let local_library_actions = local_library_actions(&action_plan.compile_actions);
    let link_action_index = link_actions_by_artifact_path(&action_plan.link_actions);

    Ok(LoadedExternalPackage {
        workspace_root: fetched.cache_path,
        source_config: source_config.with_child(),
        action_plan,
        compile_action_index,
        local_library_actions,
        link_action_index,
    })
}

pub(super) fn build_external_package(
    dep: &ExternalPackageId,
    config: ExecutionConfig<'_>,
    external: &mut ExternalArtifacts<'_>,
    external_summary: &mut ExecutionSummary,
) -> Result<()> {
    if external.built_external_packages.contains_key(dep) {
        return Ok(());
    }
    if !external.external_build_stack.insert(dep.clone()) {
        return Err(Error::Execution(format!(
            "cyclic external package build detected for `{}`",
            dep.package_name
        )));
    }

    let loaded = load_external_package_actions(
        config.source_config,
        config.dependency_workspace_root,
        dep,
        config.command,
        config.profile_selection,
    )?;
    let root_library_action = root_external_library_action(dep, &loaded.local_library_actions)?;
    let required_library_actions = compile_actions_for_root(
        root_library_action,
        &loaded.action_plan.compile_actions,
        &loaded.local_library_actions,
    );
    let required_external_dependencies =
        required_external_dependencies(root_library_action, &loaded.local_library_actions);
    for child in required_external_dependencies {
        build_external_package(
            &child,
            ExecutionConfig {
                source_config: &loaded.source_config,
                dependency_workspace_root: &loaded.workspace_root,
                command: config.command,
                profile_selection: config.profile_selection,
                std_workspace_root: config.std_workspace_root,
            },
            external,
            external_summary,
        )?;
    }

    ensure_std_packages_for_actions(
        config.std_workspace_root,
        &required_library_actions,
        config.command,
        external.built_std_packages,
        external.driver_families,
        external_summary,
    )?;

    let compile_summary = execute_compile_actions(
        &required_library_actions,
        ActionIndexes {
            action_plan: &loaded.action_plan,
            compile_action_index: &loaded.compile_action_index,
            local_library_actions: &loaded.local_library_actions,
            link_action_index: &loaded.link_action_index,
        },
        ExecutionConfig {
            source_config: &loaded.source_config,
            dependency_workspace_root: &loaded.workspace_root,
            command: config.command,
            profile_selection: config.profile_selection,
            std_workspace_root: config.std_workspace_root,
        },
        external,
    )?;
    external_summary.absorb(compile_summary);

    let root_library_action = root_external_library_action(dep, &loaded.local_library_actions)?;
    let metadata_root_path = root_library_action.metadata_path.clone().ok_or_else(|| {
        Error::Execution(format!(
            "library `{}` is missing kmeta output path",
            dep.package_name
        ))
    })?;
    validate_package_metadata_root(
        &metadata_root_path,
        &dep.package_name,
        dep.version.as_deref(),
    )?;
    let mut root_options = CompileOptions::default();
    apply_target_runtime_defaults(&mut root_options, root_library_action.target_kind);
    apply_manifest_runtime_options(
        &root_library_action.manifest_path,
        external.manifest_runtime_options,
        root_library_action.target_kind,
        &mut root_options,
    )?;
    let std_package = matches!(root_options.library_bundle, LibraryBundle::Std)
        .then(|| {
            external
                .built_std_packages
                .get(&runtime_profile_key(&root_library_action.profile))
        })
        .flatten();
    let module_aliases = module_alias_paths(
        root_library_action,
        &loaded.local_library_actions,
        std_package,
        external.built_external_packages,
    )?;
    let link_objects = if config.command == crate::script::ScriptCommand::Check {
        Vec::new()
    } else {
        link_objects_for_compile_action(
            root_library_action,
            &{
                let mut options = CompileOptions::default();
                apply_target_runtime_defaults(&mut options, root_library_action.target_kind);
                options
            },
            &loaded.local_library_actions,
            external.built_std_packages,
            external.built_external_packages,
        )?
    };
    external.built_external_packages.insert(
        dep.clone(),
        BuiltExternalPackage {
            metadata_root_path,
            link_objects,
            module_aliases,
        },
    );
    external.external_build_stack.remove(dep);
    Ok(())
}

pub(super) fn fetch_external_package(
    source_config: &SourceConfigContext,
    dependency_workspace_root: &Path,
    dep: &ExternalPackageId,
) -> Result<source::FetchedPackage> {
    let _ = source_config;
    let resolved = ResolvedGraph {
        workspace_root: dependency_workspace_root.to_path_buf(),
        packages: Vec::new(),
        external_packages: vec![ResolvedExternalPackage { id: dep.clone() }],
    };
    let mut fetched = source::fetch_external_packages(&resolved)?;
    fetched.pop().ok_or_else(|| {
        Error::Execution(format!(
            "failed to fetch external package `{}`",
            dep.package_name
        ))
    })
}

pub(super) fn ensure_external_tool_built(
    tool: &crate::script::BuildScriptTool,
    config: ExecutionConfig<'_>,
    external: &mut ExternalArtifacts<'_>,
    execution_summary: &mut ExecutionSummary,
) -> Result<()> {
    let crate::script::BuildScriptToolOrigin::ExternalPackage { dependency_id, .. } = &tool.origin
    else {
        return Ok(());
    };

    let tool_key = ExternalToolKey {
        package_id: dependency_id.clone(),
        target_name: tool.target_name.clone(),
    };
    if external.built_external_tools.contains_key(&tool_key) {
        return Ok(());
    }

    let loaded = load_external_package_actions(
        config.source_config,
        config.dependency_workspace_root,
        dependency_id,
        config.command,
        config.profile_selection,
    )?;
    let root_link_action = root_external_bin_action(
        dependency_id,
        &tool.target_name,
        &loaded.action_plan.link_actions,
    )?;
    let root_compile_action = loaded
        .compile_action_index
        .get(&ActionKey {
            domain: root_link_action.domain,
            package_id: root_link_action.package_id.clone(),
            target_kind: root_link_action.target_kind,
            target_name: root_link_action.target_name.clone(),
        })
        .ok_or_else(|| {
            Error::Execution(format!(
                "missing compile action for external tool `{}` from `{}`",
                tool.target_name, dependency_id.package_name
            ))
        })?;
    let required_compile_actions = compile_actions_for_root(
        root_compile_action,
        &loaded.action_plan.compile_actions,
        &loaded.local_library_actions,
    );
    let required_external_dependencies =
        required_external_dependencies(root_compile_action, &loaded.local_library_actions);
    let mut external_summary = ExecutionSummary::default();
    for child in required_external_dependencies {
        build_external_package(
            &child,
            ExecutionConfig {
                source_config: &loaded.source_config,
                dependency_workspace_root: &loaded.workspace_root,
                command: config.command,
                profile_selection: config.profile_selection,
                std_workspace_root: config.std_workspace_root,
            },
            external,
            &mut external_summary,
        )?;
    }
    ensure_std_packages_for_actions(
        config.std_workspace_root,
        &required_compile_actions,
        config.command,
        external.built_std_packages,
        external.driver_families,
        &mut external_summary,
    )?;
    let compile_summary = execute_compile_actions(
        &required_compile_actions,
        ActionIndexes {
            action_plan: &loaded.action_plan,
            compile_action_index: &loaded.compile_action_index,
            local_library_actions: &loaded.local_library_actions,
            link_action_index: &loaded.link_action_index,
        },
        ExecutionConfig {
            source_config: &loaded.source_config,
            dependency_workspace_root: &loaded.workspace_root,
            command: config.command,
            profile_selection: config.profile_selection,
            std_workspace_root: config.std_workspace_root,
        },
        external,
    )?;
    external_summary.absorb(compile_summary);

    let mut compiled = BTreeSet::new();
    let mut linked = BTreeSet::new();
    let mut staged_outputs = BTreeSet::new();
    let mut summary = ExecutionSummary::default();
    {
        let mut session = ExecutionSession {
            indexes: ActionIndexes {
                action_plan: &loaded.action_plan,
                compile_action_index: &loaded.compile_action_index,
                local_library_actions: &loaded.local_library_actions,
                link_action_index: &loaded.link_action_index,
            },
            config: ExecutionConfig {
                source_config: &loaded.source_config,
                dependency_workspace_root: &loaded.workspace_root,
                command: config.command,
                profile_selection: config.profile_selection,
                std_workspace_root: config.std_workspace_root,
            },
            external: ExternalArtifacts {
                built_std_packages: &mut *external.built_std_packages,
                built_external_packages: &mut *external.built_external_packages,
                built_external_tools: &mut *external.built_external_tools,
                external_build_stack: &mut *external.external_build_stack,
                manifest_runtime_options: &mut *external.manifest_runtime_options,
                driver_families: &mut *external.driver_families,
            },
            state: ExecutionState {
                compiled: &mut compiled,
                linked: &mut linked,
                staged_outputs: &mut staged_outputs,
                execution_summary: &mut summary,
                progress: None,
            },
        };
        ensure_link_action_built(root_link_action, &mut session)?;
    }
    execution_summary.absorb(external_summary);
    execution_summary.absorb(summary);
    external
        .built_external_tools
        .insert(tool_key, PathBuf::from(&tool.executable_path));
    Ok(())
}

pub(super) fn root_external_library_action<'a>(
    dep: &ExternalPackageId,
    local_library_actions: &'a BTreeMap<PackageInstanceKey, CompileAction>,
) -> Result<&'a CompileAction> {
    local_library_actions
        .values()
        .find(|action| {
            action.domain == BuildDomain::Target
                && action.package_id.name == dep.package_name
                && action.target_kind == crate::plan::TargetKind::Lib
                && match &dep.version {
                    Some(version) => action.package_id.version == *version,
                    None => true,
                }
        })
        .ok_or_else(|| {
            Error::Execution(format!(
                "external package `{}` does not expose a buildable lib target",
                dep.package_name
            ))
        })
}

pub(super) fn root_external_bin_action<'a>(
    dep: &ExternalPackageId,
    tool_name: &str,
    link_actions: &'a [LinkAction],
) -> Result<&'a LinkAction> {
    link_actions
        .iter()
        .find(|action| {
            action.package_id.name == dep.package_name
                && action.target_kind == crate::plan::TargetKind::Bin
                && action.target_name.as_deref() == Some(tool_name)
                && match &dep.version {
                    Some(version) => action.package_id.version == *version,
                    None => true,
                }
        })
        .ok_or_else(|| {
            Error::Execution(format!(
                "external package `{}` does not expose buildable tool `{tool_name}`",
                dep.package_name
            ))
        })
}

pub(super) fn compile_actions_for_root(
    root_action: &CompileAction,
    actions: &[CompileAction],
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
) -> Vec<CompileAction> {
    let required_local_packages = required_local_packages(root_action, local_library_actions);
    actions
        .iter()
        .filter(|action| {
            (action.domain == root_action.domain
                && action.package_id == root_action.package_id
                && action.target_kind == root_action.target_kind
                && action.target_name == root_action.target_name)
                || (action.target_kind == crate::plan::TargetKind::Lib
                    && required_local_packages.contains(&PackageInstanceKey {
                        domain: action.domain,
                        package_id: action.package_id.clone(),
                    }))
        })
        .cloned()
        .collect()
}

pub(super) fn local_library_actions(
    actions: &[CompileAction],
) -> BTreeMap<PackageInstanceKey, CompileAction> {
    actions
        .iter()
        .filter(|action| action.target_kind == crate::plan::TargetKind::Lib)
        .map(|action| {
            (
                PackageInstanceKey {
                    domain: action.domain,
                    package_id: action.package_id.clone(),
                },
                action.clone(),
            )
        })
        .collect()
}

pub(super) fn compile_actions_index(
    actions: &[CompileAction],
) -> BTreeMap<ActionKey, CompileAction> {
    actions
        .iter()
        .map(|action| {
            (
                ActionKey {
                    domain: action.domain,
                    package_id: action.package_id.clone(),
                    target_kind: action.target_kind,
                    target_name: action.target_name.clone(),
                },
                action.clone(),
            )
        })
        .collect()
}

pub(super) fn link_actions_by_artifact_path(
    actions: &[LinkAction],
) -> BTreeMap<PathBuf, LinkAction> {
    actions
        .iter()
        .map(|action| (action.artifact_path.clone(), action.clone()))
        .collect()
}

pub(super) fn required_local_packages(
    root_action: &CompileAction,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
) -> BTreeSet<PackageInstanceKey> {
    let mut required = BTreeSet::new();
    collect_local_packages(root_action, local_library_actions, &mut required);
    required
}

pub(super) fn collect_local_packages(
    action: &CompileAction,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    required: &mut BTreeSet<PackageInstanceKey>,
) {
    if !required.insert(PackageInstanceKey {
        domain: action.domain,
        package_id: action.package_id.clone(),
    }) {
        return;
    }
    for dep in &action.local_dependencies {
        if let Some(dep_action) = local_library_actions.get(&PackageInstanceKey {
            domain: dep.domain,
            package_id: dep.package_id.clone(),
        }) {
            collect_local_packages(dep_action, local_library_actions, required);
        }
    }
}

pub(super) fn required_external_dependencies(
    root_action: &CompileAction,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
) -> BTreeSet<ExternalPackageId> {
    let mut required = BTreeSet::new();
    collect_external_dependencies(root_action, local_library_actions, &mut required);
    required
}

pub(super) fn module_alias_paths(
    root_action: &CompileAction,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    std_package: Option<&BuiltStdPackage>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
) -> Result<BTreeMap<String, PathBuf>> {
    let mut aliases = BTreeMap::new();
    if let Some(std_package) = std_package {
        if root_action.package_id.name != "std" {
            aliases.insert("std".to_string(), std_package.metadata_root_path.clone());
        }
        aliases.extend(std_package.interface_aliases.clone());
    }
    let mut visited_local = BTreeSet::new();
    let mut visited_external = BTreeSet::new();
    collect_module_alias_paths(
        root_action,
        local_library_actions,
        built_external_packages,
        &mut visited_local,
        &mut visited_external,
        &mut aliases,
    )?;
    Ok(aliases)
}

pub(super) fn collect_module_alias_paths(
    action: &CompileAction,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    visited_local: &mut BTreeSet<PackageInstanceKey>,
    visited_external: &mut BTreeSet<ExternalPackageId>,
    aliases: &mut BTreeMap<String, PathBuf>,
) -> Result<()> {
    for dep in &action.local_dependencies {
        let Some(dep_action) = local_library_actions.get(&PackageInstanceKey {
            domain: dep.domain,
            package_id: dep.package_id.clone(),
        }) else {
            continue;
        };
        if visited_local.insert(PackageInstanceKey {
            domain: dep.domain,
            package_id: dep.package_id.clone(),
        }) {
            let metadata_path = dep_action.metadata_path.clone().ok_or_else(|| {
                Error::Execution(format!(
                    "library `{}` is missing kmeta output path",
                    dep.package_id.name
                ))
            })?;
            validate_package_metadata_root(
                &metadata_path,
                &dep.package_id.name,
                Some(dep.package_id.version.as_str()),
            )?;
            aliases.insert(dep.dependency_name.clone(), metadata_path);
            collect_module_alias_paths(
                dep_action,
                local_library_actions,
                built_external_packages,
                visited_local,
                visited_external,
                aliases,
            )?;
        }
    }

    for dep in &action.external_dependencies {
        if !visited_external.insert(dep.package_id.clone()) {
            continue;
        }
        let package = built_external_packages
            .get(&dep.package_id)
            .ok_or_else(|| {
                Error::Execution(format!(
                    "missing built external package `{}`",
                    dep.package_id.package_name
                ))
            })?;
        aliases.insert(
            dep.dependency_name.clone(),
            package.metadata_root_path.clone(),
        );
        aliases.extend(
            package
                .module_aliases
                .iter()
                .map(|(name, path)| (name.clone(), path.clone())),
        );
    }

    Ok(())
}

pub(super) fn execute_compile_actions(
    actions: &[CompileAction],
    indexes: ActionIndexes<'_>,
    config: ExecutionConfig<'_>,
    external: &mut ExternalArtifacts<'_>,
) -> Result<ExecutionSummary> {
    let mut compiled = BTreeSet::new();
    let mut linked = BTreeSet::new();
    let mut staged_outputs = BTreeSet::new();
    let mut summary = ExecutionSummary::default();
    {
        let mut session = ExecutionSession {
            indexes,
            config,
            external: ExternalArtifacts {
                built_std_packages: &mut *external.built_std_packages,
                built_external_packages: &mut *external.built_external_packages,
                built_external_tools: &mut *external.built_external_tools,
                external_build_stack: &mut *external.external_build_stack,
                manifest_runtime_options: &mut *external.manifest_runtime_options,
                driver_families: &mut *external.driver_families,
            },
            state: ExecutionState {
                compiled: &mut compiled,
                linked: &mut linked,
                staged_outputs: &mut staged_outputs,
                execution_summary: &mut summary,
                progress: None,
            },
        };
        for action in actions {
            let _ = ensure_compile_action_built(action, &mut session)?;
        }
    }
    Ok(summary)
}

pub(super) fn collect_external_dependencies(
    action: &CompileAction,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    required: &mut BTreeSet<ExternalPackageId>,
) {
    for dep in &action.external_dependencies {
        required.insert(dep.package_id.clone());
    }
    for dep in &action.local_dependencies {
        if let Some(dep_action) = local_library_actions.get(&PackageInstanceKey {
            domain: dep.domain,
            package_id: dep.package_id.clone(),
        }) {
            collect_external_dependencies(dep_action, local_library_actions, required);
        }
    }
}

pub(super) fn link_objects_for_compile_action(
    root_action: &CompileAction,
    link_options: &CompileOptions,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    built_std_packages: &BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
) -> Result<Vec<PathBuf>> {
    let mut objects = Vec::new();
    let mut seen = BTreeSet::new();
    push_linker_inputs_for_primary_output(&mut objects, &mut seen, &root_action.object_path)?;

    for package_id in required_local_packages(root_action, local_library_actions) {
        if let Some(action) = local_library_actions.get(&package_id) {
            push_linker_inputs_for_primary_output(&mut objects, &mut seen, &action.object_path)?;
        }
    }
    for dep in required_external_dependencies(root_action, local_library_actions) {
        let package = built_external_packages.get(&dep).ok_or_else(|| {
            Error::Execution(format!(
                "missing built external package `{}`",
                dep.package_name
            ))
        })?;
        for object in &package.link_objects {
            push_link_object(&mut objects, &mut seen, object);
        }
    }
    if let Some(std_package) = built_std_packages.get(&runtime_profile_key(&root_action.profile)) {
        for object in &std_package.common_link_objects {
            push_linker_inputs_for_primary_output(&mut objects, &mut seen, object)?;
        }
        match link_options.runtime_entry {
            RuntimeEntry::None => {}
            RuntimeEntry::Crt => push_linker_inputs_for_primary_output(
                &mut objects,
                &mut seen,
                &std_package.hosted_entry_object_path,
            )?,
            RuntimeEntry::Rt => push_linker_inputs_for_primary_output(
                &mut objects,
                &mut seen,
                &std_package.freestanding_entry_object_path,
            )?,
        }
    }

    Ok(objects)
}
