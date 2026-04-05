use crate::build_plan::{
    ActionPlan, BuildPlan, BuildUnit, CompileAction, LinkAction, StagedAction, StagedActionKind,
};
use crate::elaborate::{self, FeatureSelection};
use crate::error::{Error, Result};
use crate::graph::{BuildDomain, PackageId};
use crate::manifest::Manifest;
use crate::resolver::{ExternalPackageId, ResolvedExternalPackage, ResolvedGraph};
use crate::source;
use crate::workspace;
use kernc_driver::{CompilerDriver, KMETA_MANIFEST_FILE, load_kmeta_manifest};
use kernc_utils::config::{
    CompileOptions, DriverMode, LinkProfile, inject_driver_condition_defines, resolve_std_path,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionSummary {
    pub compile_actions: usize,
    pub link_actions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    pub executable: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSummary {
    pub executed: usize,
}

pub fn build(build_plan: &BuildPlan, action_plan: &ActionPlan) -> Result<ExecutionSummary> {
    build_with_command(build_plan, action_plan, crate::script::ScriptCommand::Build)
}

pub(crate) fn materialize_analysis_inputs(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
) -> Result<()> {
    let source_config = load_source_config(build_plan)?;
    let mut built_std_packages = BTreeMap::new();
    ensure_std_packages_for_actions(
        &build_plan.workspace_root,
        &action_plan.compile_actions,
        &mut built_std_packages,
    )?;
    let mut built_external_packages = BTreeMap::new();
    let mut built_external_tools = BTreeMap::new();
    let mut external_build_stack = BTreeSet::new();
    let compile_action_index = compile_actions_index(&action_plan.compile_actions);
    let local_library_actions = local_library_actions(&action_plan.compile_actions);
    let link_action_index = link_actions_by_artifact_path(&action_plan.link_actions);
    let mut compiled = BTreeSet::new();
    let mut linked = BTreeSet::new();
    let mut staged_outputs = BTreeSet::new();

    for action in &action_plan.compile_actions {
        if action.domain != BuildDomain::Target {
            continue;
        }
        execute_staged_actions(
            action.compile_inputs.as_slice(),
            action_plan.build_nodes.as_slice(),
            &mut staged_outputs,
            action.required_source_path(),
            action_plan,
            &compile_action_index,
            &local_library_actions,
            &link_action_index,
            &source_config,
            &build_plan.workspace_root,
            crate::script::ScriptCommand::Build,
            &build_plan.workspace_root,
            &mut built_std_packages,
            &mut built_external_packages,
            &mut built_external_tools,
            &mut external_build_stack,
            &mut compiled,
            &mut linked,
        )?;
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BuiltExternalPackage {
    metadata_root_path: PathBuf,
    link_objects: Vec<PathBuf>,
    module_aliases: BTreeMap<String, PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BuiltStdPackage {
    metadata_root_path: PathBuf,
    link_objects: Vec<PathBuf>,
}

#[derive(Debug)]
struct LoadedExternalPackage {
    workspace_root: PathBuf,
    source_config: SourceConfigContext,
    action_plan: ActionPlan,
    compile_action_index: BTreeMap<ActionKey, CompileAction>,
    local_library_actions: BTreeMap<PackageInstanceKey, CompileAction>,
    link_action_index: BTreeMap<PathBuf, LinkAction>,
}

#[derive(Debug)]
struct SourceConfigContext {
    scopes: Vec<SourceConfigScope>,
}

#[derive(Debug, Clone)]
struct SourceConfigScope {
    manifest_path: PathBuf,
    sources: BTreeMap<String, crate::manifest::SourceConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PackageInstanceKey {
    domain: BuildDomain,
    package_id: PackageId,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ActionKey {
    domain: BuildDomain,
    package_id: PackageId,
    target_kind: crate::plan::TargetKind,
    target_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ExternalToolKey {
    package_id: ExternalPackageId,
    target_name: String,
}

fn build_with_command(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    command: crate::script::ScriptCommand,
) -> Result<ExecutionSummary> {
    let source_config = load_source_config(build_plan)?;
    let mut built_std_packages = BTreeMap::new();
    ensure_std_packages_for_actions(
        &build_plan.workspace_root,
        &action_plan.compile_actions,
        &mut built_std_packages,
    )?;
    let mut built_external_packages = BTreeMap::new();
    let mut built_external_tools = BTreeMap::new();
    let mut external_build_stack = BTreeSet::new();
    let mut external_summary = ExecutionSummary {
        compile_actions: 0,
        link_actions: 0,
    };

    for dep in requested_external_dependencies(action_plan) {
        build_external_package(
            &source_config,
            &build_plan.workspace_root,
            &dep,
            command,
            &build_plan.workspace_root,
            &mut built_std_packages,
            &mut built_external_packages,
            &mut built_external_tools,
            &mut external_build_stack,
            &mut external_summary,
        )?;
    }

    let compile_action_index = compile_actions_index(&action_plan.compile_actions);
    let local_library_actions = local_library_actions(&action_plan.compile_actions);
    let link_action_index = link_actions_by_artifact_path(&action_plan.link_actions);
    let mut compiled = BTreeSet::new();
    let mut linked = BTreeSet::new();
    let mut staged_outputs = BTreeSet::new();

    for action in &action_plan.link_actions {
        if action.domain != BuildDomain::Target {
            continue;
        }
        ensure_link_action_built(
            action,
            action_plan,
            &compile_action_index,
            &local_library_actions,
            &link_action_index,
            &source_config,
            &build_plan.workspace_root,
            command,
            &build_plan.workspace_root,
            &mut built_std_packages,
            &mut built_external_packages,
            &mut built_external_tools,
            &mut external_build_stack,
            &mut compiled,
            &mut linked,
            &mut staged_outputs,
        )?;
    }
    for action in &action_plan.compile_actions {
        if action.domain != BuildDomain::Target {
            continue;
        }
        ensure_compile_action_built(
            action,
            &local_library_actions,
            &link_action_index,
            &source_config,
            &build_plan.workspace_root,
            command,
            &build_plan.workspace_root,
            &mut built_std_packages,
            &mut built_external_packages,
            &mut built_external_tools,
            &mut external_build_stack,
            &mut compiled,
            &mut linked,
            &mut staged_outputs,
            action_plan,
            &compile_action_index,
        )?;
    }

    Ok(ExecutionSummary {
        compile_actions: external_summary.compile_actions + action_plan.compile_count(),
        link_actions: external_summary.link_actions + action_plan.link_count(),
    })
}

pub fn run(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    unit: &BuildUnit,
) -> Result<RunSummary> {
    build_with_command(build_plan, action_plan, crate::script::ScriptCommand::Run)?;
    let action = find_link_action(action_plan, unit)?;
    let executable_path = resolve_invocation_path(&action.artifact_path)?;
    let status = runtime_command(&executable_path, action, &build_plan.workspace_root)
        .status()
        .map_err(Error::from_io_plain)?;
    if !status.success() {
        return Err(Error::Execution(format!(
            "`{}` exited with status {}",
            action.artifact_path.display(),
            status
        )));
    }

    Ok(RunSummary {
        executable: action.artifact_path.clone(),
    })
}

pub fn test(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    units: &[&BuildUnit],
) -> Result<TestSummary> {
    build_with_command(build_plan, action_plan, crate::script::ScriptCommand::Test)?;

    let mut executed = 0;
    for unit in units {
        let action = find_link_action(action_plan, unit)?;
        let executable_path = resolve_invocation_path(&action.artifact_path)?;
        let status = runtime_command(&executable_path, action, &build_plan.workspace_root)
            .status()
            .map_err(Error::from_io_plain)?;
        if !status.success() {
            return Err(Error::Execution(format!(
                "test `{}` exited with status {}",
                action.artifact_path.display(),
                status
            )));
        }
        executed += 1;
    }

    Ok(TestSummary { executed })
}

fn runtime_command(executable_path: &Path, action: &LinkAction, workspace_root: &Path) -> Command {
    let mut command = Command::new(executable_path);
    command.current_dir(&action.package_root_path);
    command.env("CRAFT_WORKSPACE_ROOT", workspace_root);
    command.env("CRAFT_PACKAGE_ROOT", &action.package_root_path);
    command
}

fn find_link_action<'a>(action_plan: &'a ActionPlan, unit: &BuildUnit) -> Result<&'a LinkAction> {
    action_plan
        .link_actions
        .iter()
        .find(|action| {
            action.domain == unit.domain
                && action.package_id == unit.package_id
                && action.target_kind == unit.target_kind
                && action.target_name == unit.target_name
        })
        .ok_or_else(|| {
            Error::Execution(format!(
                "missing link action for `{}` target `{}`",
                unit.package_id.name, unit.artifact_name
            ))
        })
}

fn compile_time_defines(
    cfg: &std::collections::BTreeMap<String, crate::plan::PlanValue>,
    define: &std::collections::BTreeMap<String, crate::plan::PlanValue>,
    source_path: &Path,
) -> Result<HashMap<String, String>> {
    let mut values = HashMap::new();

    for (name, value) in cfg {
        values.insert(name.clone(), plan_value_string(value));
    }
    for (name, value) in define {
        let value = plan_value_string(value);
        if let Some(existing) = values.get(name)
            && existing != &value
        {
            return Err(Error::Execution(format!(
                "compile-time key `{name}` has conflicting cfg/define values for `{}`",
                source_path.display()
            )));
        }
        values.insert(name.clone(), value);
    }

    Ok(values)
}

fn plan_value_string(value: &crate::plan::PlanValue) -> String {
    match value {
        crate::plan::PlanValue::Bool(value) => value.to_string(),
        crate::plan::PlanValue::String(value) => value.clone(),
    }
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    crate::local_state::ensure_parent_dir(path)
}

fn resolve_invocation_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    Ok(std::env::current_dir()
        .map_err(Error::from_io_plain)?
        .join(path))
}

fn load_source_config(build_plan: &BuildPlan) -> Result<SourceConfigContext> {
    let manifest_path = build_plan.workspace_root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path)?;
    manifest.validate(&manifest_path)?;
    Ok(source_config_context(manifest_path, manifest))
}

fn source_config_context(manifest_path: PathBuf, manifest: Manifest) -> SourceConfigContext {
    SourceConfigContext {
        scopes: vec![SourceConfigScope {
            manifest_path,
            sources: manifest.sources.clone(),
        }],
    }
}

impl SourceConfigContext {
    fn with_child(&self, manifest_path: PathBuf, manifest: &Manifest) -> Self {
        let mut scopes = Vec::with_capacity(self.scopes.len() + 1);
        scopes.push(SourceConfigScope {
            manifest_path,
            sources: manifest.sources.clone(),
        });
        scopes.extend(self.scopes.iter().cloned());
        Self { scopes }
    }

    fn lookup_chain(&self) -> Vec<source::SourceLookup<'_>> {
        self.scopes
            .iter()
            .map(|scope| source::SourceLookup {
                manifest_path: &scope.manifest_path,
                sources: &scope.sources,
            })
            .collect()
    }
}

#[allow(clippy::too_many_arguments)]
fn ensure_compile_action_built(
    action: &CompileAction,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    link_action_index: &BTreeMap<PathBuf, LinkAction>,
    source_config: &SourceConfigContext,
    dependency_workspace_root: &Path,
    command: crate::script::ScriptCommand,
    std_workspace_root: &Path,
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &mut BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    built_external_tools: &mut BTreeMap<ExternalToolKey, PathBuf>,
    external_build_stack: &mut BTreeSet<ExternalPackageId>,
    compiled: &mut BTreeSet<PathBuf>,
    linked: &mut BTreeSet<PathBuf>,
    staged_outputs: &mut BTreeSet<PathBuf>,
    action_plan: &ActionPlan,
    compile_action_index: &BTreeMap<ActionKey, CompileAction>,
) -> Result<()> {
    if compiled.contains(&action.object_path) {
        return Ok(());
    }

    for dep in &action.local_dependencies {
        if let Some(dep_action) = local_library_actions.get(&PackageInstanceKey {
            domain: dep.domain,
            package_id: dep.package_id.clone(),
        }) {
            ensure_compile_action_built(
                dep_action,
                local_library_actions,
                link_action_index,
                source_config,
                dependency_workspace_root,
                command,
                std_workspace_root,
                built_std_packages,
                built_external_packages,
                built_external_tools,
                external_build_stack,
                compiled,
                linked,
                staged_outputs,
                action_plan,
                compile_action_index,
            )?;
        }
    }

    execute_staged_actions(
        action.compile_inputs.as_slice(),
        action_plan.build_nodes.as_slice(),
        staged_outputs,
        action.required_source_path(),
        action_plan,
        compile_action_index,
        local_library_actions,
        link_action_index,
        source_config,
        dependency_workspace_root,
        command,
        std_workspace_root,
        built_std_packages,
        built_external_packages,
        built_external_tools,
        external_build_stack,
        compiled,
        linked,
    )?;
    ensure_parent_dir(&action.object_path)?;
    ensure_parent_dir(&action.artifact_path)?;

    let mut options = CompileOptions {
        input_file: Some(action.source_path().to_string_lossy().to_string()),
        output_file: action.object_path.to_string_lossy().to_string(),
        metadata_output: action
            .metadata_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        metadata_package_name: (action.target_kind == crate::plan::TargetKind::Lib)
            .then(|| action.package_id.name.clone()),
        metadata_package_version: (action.target_kind == crate::plan::TargetKind::Lib)
            .then(|| action.package_id.version.clone()),
        root_module_name: (action.target_kind == crate::plan::TargetKind::Lib)
            .then(|| action.package_id.name.clone()),
        driver_mode: DriverMode::CompileOnly,
        report_progress: false,
        use_std: true,
        link_profile: LinkProfile::Hosted,
        ..CompileOptions::default()
    };
    apply_host_linker_env(&mut options);
    inject_driver_condition_defines(&mut options);
    options.module_interface_aliases = compile_module_aliases(
        action,
        local_library_actions,
        built_std_packages.get(&action.profile.name),
        built_external_packages,
    )?;
    options.custom_defines.extend(compile_time_defines(
        &action.cfg,
        &action.define,
        action.source_path(),
    )?);

    let driver = CompilerDriver::new(options);
    if !driver.compile() {
        return Err(Error::Execution(format!(
            "compile failed for `{}`",
            action.source_path().display()
        )));
    }

    compiled.insert(action.object_path.clone());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn execute_staged_actions(
    root_ids: &[usize],
    build_nodes: &[StagedAction],
    staged_outputs: &mut BTreeSet<PathBuf>,
    required_path: Option<&Path>,
    action_plan: &ActionPlan,
    compile_action_index: &BTreeMap<ActionKey, CompileAction>,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    link_action_index: &BTreeMap<PathBuf, LinkAction>,
    source_config: &SourceConfigContext,
    dependency_workspace_root: &Path,
    command: crate::script::ScriptCommand,
    std_workspace_root: &Path,
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &mut BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    built_external_tools: &mut BTreeMap<ExternalToolKey, PathBuf>,
    external_build_stack: &mut BTreeSet<ExternalPackageId>,
    compiled: &mut BTreeSet<PathBuf>,
    linked: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    let action_index = build_nodes
        .iter()
        .map(|action| (action.id, action))
        .collect::<BTreeMap<_, _>>();
    let mut active = BTreeSet::new();
    for root_id in root_ids {
        let action = action_index
            .get(root_id)
            .ok_or_else(|| Error::Execution(format!("missing build node `{root_id}`")))?;
        execute_staged_action(
            action,
            &action_index,
            &mut active,
            staged_outputs,
            action_plan,
            compile_action_index,
            local_library_actions,
            link_action_index,
            source_config,
            dependency_workspace_root,
            command,
            std_workspace_root,
            built_std_packages,
            built_external_packages,
            built_external_tools,
            external_build_stack,
            compiled,
            linked,
        )?;
    }
    if let Some(required_path) = required_path
        && !root_ids.is_empty()
        && !required_path.is_file()
    {
        return Err(Error::Execution(format!(
            "staged actions did not materialize source `{}`",
            required_path.display()
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn execute_staged_action(
    action: &StagedAction,
    action_index: &BTreeMap<usize, &StagedAction>,
    active: &mut BTreeSet<usize>,
    staged_outputs: &mut BTreeSet<PathBuf>,
    action_plan: &ActionPlan,
    compile_action_index: &BTreeMap<ActionKey, CompileAction>,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    link_action_index: &BTreeMap<PathBuf, LinkAction>,
    source_config: &SourceConfigContext,
    dependency_workspace_root: &Path,
    command: crate::script::ScriptCommand,
    std_workspace_root: &Path,
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &mut BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    built_external_tools: &mut BTreeMap<ExternalToolKey, PathBuf>,
    external_build_stack: &mut BTreeSet<ExternalPackageId>,
    compiled: &mut BTreeSet<PathBuf>,
    linked: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    let output_path = PathBuf::from(&action.output);
    if staged_outputs.contains(&output_path) {
        return Ok(());
    }
    if !active.insert(action.id) {
        return Err(Error::Execution(format!(
            "cyclic staged action dependency detected at `{}`",
            action.output
        )));
    }
    for dependency_id in &action.depends_on {
        let dependency = action_index.get(dependency_id).ok_or_else(|| {
            Error::Execution(format!(
                "missing staged action dependency `{dependency_id}` for `{}`",
                action.output
            ))
        })?;
        execute_staged_action(
            dependency,
            action_index,
            active,
            staged_outputs,
            action_plan,
            compile_action_index,
            local_library_actions,
            link_action_index,
            source_config,
            dependency_workspace_root,
            command,
            std_workspace_root,
            built_std_packages,
            built_external_packages,
            built_external_tools,
            external_build_stack,
            compiled,
            linked,
        )?;
    }
    active.remove(&action.id);
    if !staged_outputs.insert(output_path.clone()) {
        return Ok(());
    }

    ensure_parent_dir(&output_path)?;

    match &action.kind {
        StagedActionKind::WriteFile { contents } => {
            fs::write(&output_path, contents).map_err(|err| Error::from_io(&output_path, err))?;
        }
        StagedActionKind::RunTool { tool, args } => {
            let tool_path = PathBuf::from(&tool.executable_path);
            match &tool.origin {
                crate::script::BuildScriptToolOrigin::LocalPackage { .. } => {
                    if let Some(link_action) = link_action_index.get(&tool_path) {
                        ensure_link_action_built(
                            link_action,
                            action_plan,
                            compile_action_index,
                            local_library_actions,
                            link_action_index,
                            source_config,
                            dependency_workspace_root,
                            command,
                            std_workspace_root,
                            built_std_packages,
                            built_external_packages,
                            built_external_tools,
                            external_build_stack,
                            compiled,
                            linked,
                            staged_outputs,
                        )?;
                    }
                }
                crate::script::BuildScriptToolOrigin::ExternalPackage { .. } => {
                    ensure_external_tool_built(
                        tool,
                        source_config,
                        dependency_workspace_root,
                        command,
                        std_workspace_root,
                        built_std_packages,
                        built_external_packages,
                        built_external_tools,
                        external_build_stack,
                    )?;
                }
            }
            let output = Command::new(&tool_path)
                .args(args)
                .output()
                .map_err(Error::from_io_plain)?;
            if !output.status.success() {
                return Err(Error::Execution(format!(
                    "tool `{}` exited with status {}",
                    tool_path.display(),
                    output.status
                )));
            }
            fs::write(&output_path, output.stdout)
                .map_err(|err| Error::from_io(&output_path, err))?;
        }
        StagedActionKind::CopyFile { source } => {
            let input_path = PathBuf::from(source);
            fs::copy(&input_path, &output_path).map_err(|err| {
                Error::Execution(format!(
                    "failed to copy staged input `{}` to `{}`: {err}",
                    input_path.display(),
                    output_path.display(),
                ))
            })?;
        }
        StagedActionKind::CopyDirectory { source } => {
            let input_path = PathBuf::from(source);
            copy_dir_all(&input_path, &output_path).map_err(|err| {
                Error::Execution(format!(
                    "failed to copy staged directory `{}` to `{}`: {err}",
                    input_path.display(),
                    output_path.display(),
                ))
            })?;
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn ensure_link_action_built(
    action: &LinkAction,
    action_plan: &ActionPlan,
    compile_action_index: &BTreeMap<ActionKey, CompileAction>,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    link_action_index: &BTreeMap<PathBuf, LinkAction>,
    source_config: &SourceConfigContext,
    dependency_workspace_root: &Path,
    command: crate::script::ScriptCommand,
    std_workspace_root: &Path,
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &mut BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    built_external_tools: &mut BTreeMap<ExternalToolKey, PathBuf>,
    external_build_stack: &mut BTreeSet<ExternalPackageId>,
    compiled: &mut BTreeSet<PathBuf>,
    linked: &mut BTreeSet<PathBuf>,
    staged_outputs: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    if linked.contains(&action.artifact_path) {
        return Ok(());
    }
    let compile_action = compile_action_index
        .get(&ActionKey {
            domain: action.domain,
            package_id: action.package_id.clone(),
            target_kind: action.target_kind,
            target_name: action.target_name.clone(),
        })
        .ok_or_else(|| {
            Error::Execution(format!(
                "missing compile action for `{}` target `{}`",
                action.package_id.name, action.artifact_name
            ))
        })?;
    ensure_compile_action_built(
        compile_action,
        local_library_actions,
        link_action_index,
        source_config,
        dependency_workspace_root,
        command,
        std_workspace_root,
        built_std_packages,
        built_external_packages,
        built_external_tools,
        external_build_stack,
        compiled,
        linked,
        staged_outputs,
        action_plan,
        compile_action_index,
    )?;

    ensure_parent_dir(&action.artifact_path)?;

    let mut options = CompileOptions {
        output_file: action.artifact_path.to_string_lossy().to_string(),
        driver_mode: DriverMode::LinkOnly,
        report_progress: false,
        link_profile: LinkProfile::Hosted,
        use_std: true,
        ..CompileOptions::default()
    };
    apply_host_linker_env(&mut options);
    options.linker_inputs = link_inputs_for_action(
        action,
        action_plan,
        local_library_actions,
        built_std_packages,
        built_external_packages,
    )?
    .into_iter()
    .map(|path| path.to_string_lossy().to_string())
    .collect();
    options.linker_libraries = action.link.system_libs.clone();
    options.linker_search_paths = action.link.search_paths.clone();
    options.linker_args = action.link.args.clone();
    for framework in &action.link.frameworks {
        options.linker_args.push("-framework".to_string());
        options.linker_args.push(framework.clone());
    }

    let driver = CompilerDriver::new(options);
    if !driver.compile() {
        return Err(Error::Execution(format!(
            "link failed for `{}`",
            action.artifact_path.display()
        )));
    }

    execute_staged_actions(
        action.artifact_outputs.as_slice(),
        action_plan.build_nodes.as_slice(),
        staged_outputs,
        None,
        action_plan,
        compile_action_index,
        local_library_actions,
        link_action_index,
        source_config,
        dependency_workspace_root,
        command,
        std_workspace_root,
        built_std_packages,
        built_external_packages,
        built_external_tools,
        external_build_stack,
        compiled,
        linked,
    )?;
    linked.insert(action.artifact_path.clone());
    Ok(())
}

fn copy_dir_all(source: &Path, dest: &Path) -> std::result::Result<(), String> {
    fs::create_dir_all(dest).map_err(|err| err.to_string())?;
    for entry in fs::read_dir(source).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let file_type = entry.file_type().map_err(|err| err.to_string())?;
        let source_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&source_path, &dest_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &dest_path).map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

fn compile_module_aliases(
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

fn requested_external_dependencies(action_plan: &ActionPlan) -> Vec<ExternalPackageId> {
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

fn load_external_package_actions(
    source_config: &SourceConfigContext,
    dependency_workspace_root: &Path,
    dep: &ExternalPackageId,
    command: crate::script::ScriptCommand,
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
        &FeatureSelection::default(),
    )?;
    let build_plan = crate::build_plan::derive(&elaboration, command)?;
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let compile_action_index = compile_actions_index(&action_plan.compile_actions);
    let local_library_actions = local_library_actions(&action_plan.compile_actions);
    let link_action_index = link_actions_by_artifact_path(&action_plan.link_actions);

    Ok(LoadedExternalPackage {
        workspace_root: fetched.cache_path,
        source_config: source_config.with_child(manifest_path, &manifest),
        action_plan,
        compile_action_index,
        local_library_actions,
        link_action_index,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_external_package(
    source_config: &SourceConfigContext,
    dependency_workspace_root: &Path,
    dep: &ExternalPackageId,
    command: crate::script::ScriptCommand,
    std_workspace_root: &Path,
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &mut BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    built_external_tools: &mut BTreeMap<ExternalToolKey, PathBuf>,
    external_build_stack: &mut BTreeSet<ExternalPackageId>,
    external_summary: &mut ExecutionSummary,
) -> Result<()> {
    if built_external_packages.contains_key(dep) {
        return Ok(());
    }
    if !external_build_stack.insert(dep.clone()) {
        return Err(Error::Execution(format!(
            "cyclic external package build detected for `{}`",
            dep.package_name
        )));
    }

    let loaded =
        load_external_package_actions(source_config, dependency_workspace_root, dep, command)?;
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
            &loaded.source_config,
            &loaded.workspace_root,
            &child,
            command,
            std_workspace_root,
            built_std_packages,
            built_external_packages,
            built_external_tools,
            external_build_stack,
            external_summary,
        )?;
    }

    ensure_std_packages_for_actions(
        std_workspace_root,
        &required_library_actions,
        built_std_packages,
    )?;

    execute_compile_actions(
        &required_library_actions,
        &loaded.action_plan,
        &loaded.compile_action_index,
        &loaded.local_library_actions,
        &loaded.link_action_index,
        &loaded.source_config,
        &loaded.workspace_root,
        command,
        std_workspace_root,
        built_std_packages,
        built_external_packages,
        built_external_tools,
        external_build_stack,
    )?;

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
    let module_aliases = module_alias_paths(
        root_library_action,
        &loaded.local_library_actions,
        built_std_packages.get(&root_library_action.profile.name),
        built_external_packages,
    )?;
    let link_objects = link_objects_for_compile_action(
        root_library_action,
        &loaded.local_library_actions,
        built_std_packages,
        built_external_packages,
    )?;
    built_external_packages.insert(
        dep.clone(),
        BuiltExternalPackage {
            metadata_root_path,
            link_objects,
            module_aliases,
        },
    );
    external_summary.compile_actions += required_library_actions.len();
    external_build_stack.remove(dep);
    Ok(())
}

fn fetch_external_package(
    source_config: &SourceConfigContext,
    dependency_workspace_root: &Path,
    dep: &ExternalPackageId,
) -> Result<source::FetchedPackage> {
    let resolved = ResolvedGraph {
        workspace_root: dependency_workspace_root.to_path_buf(),
        packages: Vec::new(),
        external_packages: vec![ResolvedExternalPackage { id: dep.clone() }],
    };
    let lookup_chain = source_config.lookup_chain();
    let mut fetched = source::fetch_external_packages_with_lookup(&lookup_chain, &resolved)?;
    fetched.pop().ok_or_else(|| {
        Error::Execution(format!(
            "failed to fetch external package `{}`",
            dep.package_name
        ))
    })
}

#[allow(clippy::too_many_arguments)]
fn ensure_external_tool_built(
    tool: &crate::script::BuildScriptTool,
    source_config: &SourceConfigContext,
    dependency_workspace_root: &Path,
    command: crate::script::ScriptCommand,
    std_workspace_root: &Path,
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &mut BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    built_external_tools: &mut BTreeMap<ExternalToolKey, PathBuf>,
    external_build_stack: &mut BTreeSet<ExternalPackageId>,
) -> Result<()> {
    let crate::script::BuildScriptToolOrigin::ExternalPackage { dependency_id, .. } = &tool.origin
    else {
        return Ok(());
    };

    let tool_key = ExternalToolKey {
        package_id: dependency_id.clone(),
        target_name: tool.target_name.clone(),
    };
    if built_external_tools.contains_key(&tool_key) {
        return Ok(());
    }

    let loaded = load_external_package_actions(
        source_config,
        dependency_workspace_root,
        dependency_id,
        command,
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
    let mut external_summary = ExecutionSummary {
        compile_actions: 0,
        link_actions: 0,
    };
    for child in required_external_dependencies {
        build_external_package(
            &loaded.source_config,
            &loaded.workspace_root,
            &child,
            command,
            std_workspace_root,
            built_std_packages,
            built_external_packages,
            built_external_tools,
            external_build_stack,
            &mut external_summary,
        )?;
    }
    ensure_std_packages_for_actions(
        std_workspace_root,
        &required_compile_actions,
        built_std_packages,
    )?;
    execute_compile_actions(
        &required_compile_actions,
        &loaded.action_plan,
        &loaded.compile_action_index,
        &loaded.local_library_actions,
        &loaded.link_action_index,
        &loaded.source_config,
        &loaded.workspace_root,
        command,
        std_workspace_root,
        built_std_packages,
        built_external_packages,
        built_external_tools,
        external_build_stack,
    )?;

    let mut compiled = BTreeSet::new();
    let mut linked = BTreeSet::new();
    let mut staged_outputs = BTreeSet::new();
    ensure_link_action_built(
        root_link_action,
        &loaded.action_plan,
        &loaded.compile_action_index,
        &loaded.local_library_actions,
        &loaded.link_action_index,
        &loaded.source_config,
        &loaded.workspace_root,
        command,
        std_workspace_root,
        built_std_packages,
        built_external_packages,
        built_external_tools,
        external_build_stack,
        &mut compiled,
        &mut linked,
        &mut staged_outputs,
    )?;
    built_external_tools.insert(tool_key, PathBuf::from(&tool.executable_path));
    Ok(())
}

fn root_external_library_action<'a>(
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

fn root_external_bin_action<'a>(
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

fn compile_actions_for_root(
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

fn local_library_actions(actions: &[CompileAction]) -> BTreeMap<PackageInstanceKey, CompileAction> {
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

fn compile_actions_index(actions: &[CompileAction]) -> BTreeMap<ActionKey, CompileAction> {
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

fn link_actions_by_artifact_path(actions: &[LinkAction]) -> BTreeMap<PathBuf, LinkAction> {
    actions
        .iter()
        .map(|action| (action.artifact_path.clone(), action.clone()))
        .collect()
}

fn required_local_packages(
    root_action: &CompileAction,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
) -> BTreeSet<PackageInstanceKey> {
    let mut required = BTreeSet::new();
    collect_local_packages(root_action, local_library_actions, &mut required);
    required
}

fn collect_local_packages(
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

fn required_external_dependencies(
    root_action: &CompileAction,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
) -> BTreeSet<ExternalPackageId> {
    let mut required = BTreeSet::new();
    collect_external_dependencies(root_action, local_library_actions, &mut required);
    required
}

fn module_alias_paths(
    root_action: &CompileAction,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    std_package: Option<&BuiltStdPackage>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
) -> Result<BTreeMap<String, PathBuf>> {
    let mut aliases = BTreeMap::new();
    if let Some(std_package) = std_package {
        aliases.insert("std".to_string(), std_package.metadata_root_path.clone());
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

fn collect_module_alias_paths(
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

#[allow(clippy::too_many_arguments)]
fn execute_compile_actions(
    actions: &[CompileAction],
    action_plan: &ActionPlan,
    compile_action_index: &BTreeMap<ActionKey, CompileAction>,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    link_action_index: &BTreeMap<PathBuf, LinkAction>,
    source_config: &SourceConfigContext,
    dependency_workspace_root: &Path,
    command: crate::script::ScriptCommand,
    std_workspace_root: &Path,
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &mut BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    built_external_tools: &mut BTreeMap<ExternalToolKey, PathBuf>,
    external_build_stack: &mut BTreeSet<ExternalPackageId>,
) -> Result<()> {
    let mut compiled = BTreeSet::new();
    let mut linked = BTreeSet::new();
    let mut staged_outputs = BTreeSet::new();
    for action in actions {
        ensure_compile_action_built(
            action,
            local_library_actions,
            link_action_index,
            source_config,
            dependency_workspace_root,
            command,
            std_workspace_root,
            built_std_packages,
            built_external_packages,
            built_external_tools,
            external_build_stack,
            &mut compiled,
            &mut linked,
            &mut staged_outputs,
            action_plan,
            compile_action_index,
        )?;
    }
    Ok(())
}

fn collect_external_dependencies(
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

fn link_inputs_for_action(
    link_action: &LinkAction,
    action_plan: &ActionPlan,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    built_std_packages: &BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
) -> Result<Vec<PathBuf>> {
    let compile_action = action_plan
        .compile_actions
        .iter()
        .find(|action| {
            action.domain == link_action.domain
                && action.package_id == link_action.package_id
                && action.target_kind == link_action.target_kind
                && action.target_name == link_action.target_name
        })
        .ok_or_else(|| {
            Error::Execution(format!(
                "missing compile action for `{}` target `{}`",
                link_action.package_id.name, link_action.artifact_name
            ))
        })?;
    link_objects_for_compile_action(
        compile_action,
        local_library_actions,
        built_std_packages,
        built_external_packages,
    )
}

fn link_objects_for_compile_action(
    root_action: &CompileAction,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    built_std_packages: &BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
) -> Result<Vec<PathBuf>> {
    let mut objects = Vec::new();
    let mut seen = BTreeSet::new();
    push_link_object(&mut objects, &mut seen, &root_action.object_path);

    for package_id in required_local_packages(root_action, local_library_actions) {
        if let Some(action) = local_library_actions.get(&package_id) {
            push_link_object(&mut objects, &mut seen, &action.object_path);
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
    if let Some(std_package) = built_std_packages.get(&root_action.profile.name) {
        for object in &std_package.link_objects {
            push_link_object(&mut objects, &mut seen, object);
        }
    }

    Ok(objects)
}

fn ensure_std_packages_for_actions(
    workspace_root: &Path,
    actions: &[CompileAction],
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
) -> Result<()> {
    let profiles = actions
        .iter()
        .map(|action| action.profile.name.clone())
        .collect::<BTreeSet<_>>();
    for profile in profiles {
        build_std_package(workspace_root, &profile, built_std_packages)?;
    }
    Ok(())
}

fn build_std_package(
    workspace_root: &Path,
    profile: &str,
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
) -> Result<()> {
    if built_std_packages.contains_key(profile) {
        return Ok(());
    }

    let std_root = resolve_std_path();
    let source_path = std_root.join("init.rn");
    if !source_path.is_file() {
        return Err(Error::Execution(format!(
            "standard library root `{}` is missing",
            source_path.display()
        )));
    }

    let object_path = workspace_root
        .join(".craft")
        .join("build")
        .join(profile)
        .join("obj")
        .join("std")
        .join("lib")
        .join("std.o");
    let metadata_root_path = workspace_root
        .join(".craft")
        .join("build")
        .join(profile)
        .join("meta")
        .join("std");

    ensure_parent_dir(&object_path)?;
    ensure_parent_dir(&metadata_root_path.join(KMETA_MANIFEST_FILE))?;

    let mut options = CompileOptions {
        input_file: Some(source_path.to_string_lossy().to_string()),
        output_file: object_path.to_string_lossy().to_string(),
        metadata_output: Some(metadata_root_path.to_string_lossy().to_string()),
        metadata_package_name: Some("std".to_string()),
        metadata_package_version: None,
        root_module_name: Some("std".to_string()),
        driver_mode: DriverMode::CompileOnly,
        report_progress: false,
        use_std: true,
        link_profile: LinkProfile::Hosted,
        ..CompileOptions::default()
    };
    apply_host_linker_env(&mut options);
    inject_driver_condition_defines(&mut options);
    options
        .module_aliases
        .insert("std".to_string(), std_root.to_string_lossy().to_string());

    let driver = CompilerDriver::new(options);
    if !driver.compile() {
        return Err(Error::Execution(format!(
            "compile failed for standard library `{}`",
            source_path.display()
        )));
    }

    built_std_packages.insert(
        profile.to_string(),
        BuiltStdPackage {
            metadata_root_path,
            link_objects: vec![object_path],
        },
    );
    Ok(())
}

fn push_link_object(objects: &mut Vec<PathBuf>, seen: &mut BTreeSet<PathBuf>, path: &Path) {
    if seen.insert(path.to_path_buf()) {
        objects.push(path.to_path_buf());
    }
}

fn validate_package_metadata_root(
    metadata_root: &Path,
    expected_package_name: &str,
    expected_version: Option<&str>,
) -> Result<()> {
    let manifest = load_kmeta_manifest(metadata_root)
        .map_err(|err| {
            Error::Execution(format!(
                "failed to read kmeta manifest from `{}`: {err}",
                metadata_root.display()
            ))
        })?
        .ok_or_else(|| {
            Error::Execution(format!(
                "kmeta package root `{}` is missing `{}`",
                metadata_root.display(),
                KMETA_MANIFEST_FILE
            ))
        })?;

    if manifest.package_name != expected_package_name {
        return Err(Error::Execution(format!(
            "kmeta package at `{}` declares package `{}` but `{}` was required",
            metadata_root.display(),
            manifest.package_name,
            expected_package_name
        )));
    }

    if let Some(expected_version) = expected_version
        && manifest.package_version.as_deref() != Some(expected_version)
    {
        let actual = manifest.package_version.as_deref().unwrap_or("<none>");
        return Err(Error::Execution(format!(
            "kmeta package `{}` at `{}` declares version `{}` but `{}` was required",
            expected_package_name,
            metadata_root.display(),
            actual,
            expected_version
        )));
    }

    Ok(())
}

fn apply_host_linker_env(options: &mut CompileOptions) {
    if let Ok(cc_env) = std::env::var("CC") {
        options.linker_cmd = cc_env;
    }
}

#[cfg(test)]
mod tests {
    use super::{build, run, test, validate_package_metadata_root};
    use crate::build_plan;
    use crate::elaborate::{FeatureSelection, plan};
    use crate::manifest::Manifest;
    use crate::workspace;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn builds_and_runs_hosted_package_with_local_library_dependency() {
        let root = temp_dir("craft-exec-run");
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
"#,
        )
        .unwrap();
        fs::create_dir_all(app_dir.join("src")).unwrap();
        fs::write(
            app_dir.join("src/main.rn"),
            r#"
extern fn main(args: [][]u8) i32 {
    let _ = args;
    if (util.answer() == 42) {
        return 0;
    }
    return 1;
}
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
        fs::create_dir_all(util_dir.join("src")).unwrap();
        fs::write(
            util_dir.join("src/lib.rn"),
            r#"
pub fn answer() i32 {
    return 42;
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = crate::workspace::load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Run,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let unit = build_plan
            .packages
            .iter()
            .find(|package| package.package_id.name == "app")
            .unwrap()
            .units
            .iter()
            .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
            .unwrap();

        let summary = run(&build_plan, &action_plan, unit).unwrap();
        assert!(summary.executable.is_file());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_and_runs_hosted_package_with_renamed_local_library_dependency() {
        let root = temp_dir("craft-exec-run-alias");
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
        fs::create_dir_all(app_dir.join("src")).unwrap();
        fs::write(
            app_dir.join("src/main.rn"),
            r#"
extern fn main(args: [][]u8) i32 {
    let _ = args;
    if (foo.answer() == 42) {
        return 0;
    }
    return 1;
}
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
        fs::create_dir_all(util_dir.join("src")).unwrap();
        fs::write(
            util_dir.join("src/lib.rn"),
            r#"
pub fn answer() i32 {
    return 42;
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = crate::workspace::load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Run,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let unit = build_plan
            .packages
            .iter()
            .find(|package| package.package_id.name == "app")
            .unwrap()
            .units
            .iter()
            .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
            .unwrap();

        let summary = run(&build_plan, &action_plan, unit).unwrap();
        assert!(summary.executable.is_file());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_hosted_package_when_dependency_emits_generic_std_instantiations() {
        let root = temp_dir("craft-exec-generic-std-linkage");
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
"#,
        )
        .unwrap();
        fs::create_dir_all(app_dir.join("src")).unwrap();
        fs::write(
            app_dir.join("src/main.rn"),
            r#"
extern fn main(args: [][]u8) i32 {
    let _ = args;
    if (util.is_truthy("true")) {
        return 0;
    }
    return 1;
}
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
        fs::create_dir_all(util_dir.join("src")).unwrap();
        fs::write(
            util_dir.join("src/lib.rn"),
            r#"
pub fn is_truthy(value: []u8) bool {
    return value.eq("true");
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = crate::workspace::load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Build,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());

        let summary = build(&build_plan, &action_plan).unwrap();
        assert_eq!(summary.compile_actions, 2);
        assert_eq!(summary.link_actions, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_and_executes_test_units() {
        let root = temp_dir("craft-exec-test");
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"

[[test]]
name = "smoke"
root = "tests/smoke.rn"
"#,
        )
        .unwrap();
        fs::write(
            root.join("tests/smoke.rn"),
            r#"
extern fn main(args: [][]u8) i32 {
    let _ = args;
    return 0;
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
            crate::script::ScriptCommand::Test,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Test).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let test_units = build_plan.packages[0]
            .units
            .iter()
            .filter(|unit| unit.target_kind == crate::plan::TargetKind::Test)
            .collect::<Vec<_>>();

        let summary = test(&build_plan, &action_plan, &test_units).unwrap();
        assert_eq!(summary.executed, 1);
        let gitignore = fs::read_to_string(root.join(".craft").join(".gitignore")).unwrap();
        assert!(gitignore.contains("*"));
        assert!(gitignore.contains("!.gitignore"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn tests_can_import_their_own_package_library() {
        let root = temp_dir("craft-exec-test-self-lib");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"

[lib]
root = "src/lib.rn"

[[test]]
name = "smoke"
root = "tests/smoke.rn"
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/lib.rn"),
            r#"
pub fn answer() i32 {
    return 42;
}
"#,
        )
        .unwrap();
        fs::write(
            root.join("tests/smoke.rn"),
            r#"
use demo.answer;

extern fn main(args: [][]u8) i32 {
    let _ = args;
    if (answer() == 42) {
        return 0;
    }
    return 1;
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
            crate::script::ScriptCommand::Test,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Test).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let test_units = build_plan.packages[0]
            .units
            .iter()
            .filter(|unit| unit.target_kind == crate::plan::TargetKind::Test)
            .collect::<Vec<_>>();

        let summary = test(&build_plan, &action_plan, &test_units).unwrap();
        assert_eq!(summary.executed, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_member_tests_run_from_member_package_root() {
        let root = temp_dir("craft-exec-test-member-cwd");
        let app_dir = root.join("app");
        fs::create_dir_all(app_dir.join("tests")).unwrap();
        fs::create_dir_all(app_dir.join("fixtures")).unwrap();
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

[[test]]
name = "cwd"
root = "tests/cwd.rn"
"#,
        )
        .unwrap();
        fs::write(app_dir.join("fixtures/ok.txt"), "ok\n").unwrap();
        fs::write(
            app_dir.join("tests/cwd.rn"),
            r#"
use std.fs;
use std.mem.alloc.{Allocator, GPA, Page};

extern fn main(args: [][]u8) i32 {
    let _ = args;
    let mut page = Page.{};
    let mut gpa = GPA.{ backing: *mut Allocator.{ page..& } };
    defer gpa..&.deinit();
    let alloc = *mut Allocator.{ gpa..& };

    let found = match (fs.exists(alloc, "fixtures/ok.txt")) {
        .{ Ok: value } => value,
        .{ Err: _ } => false,
    };
    if (found) {
        return 0;
    }
    return 1;
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = workspace::load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Test,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Test).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let test_units = build_plan
            .packages
            .iter()
            .find(|package| package.package_id.name == "app")
            .unwrap()
            .units
            .iter()
            .filter(|unit| unit.target_kind == crate::plan::TargetKind::Test)
            .collect::<Vec<_>>();

        let summary = test(&build_plan, &action_plan, &test_units).unwrap();
        assert_eq!(summary.executed, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_member_tests_receive_package_and_workspace_root_env() {
        let root = temp_dir("craft-exec-test-member-env");
        let app_dir = root.join("app");
        fs::create_dir_all(app_dir.join("tests")).unwrap();
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

[[test]]
name = "env"
root = "tests/env.rn"
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("tests/env.rn"),
            r#"
use std.env;
use std.mem.alloc.{Allocator, GPA, Page};

extern fn main(args: [][]u8) i32 {
    let _ = args;
    let mut page = Page.{};
    let mut gpa = GPA.{ backing: *mut Allocator.{ page..& } };
    defer gpa..&.deinit();
    let alloc = *mut Allocator.{ gpa..& };

    let mut workspace_root = match (env.get(alloc, "CRAFT_WORKSPACE_ROOT")) {
        .{ Some: value } => value,
        .None => return 1,
    };
    defer workspace_root..&.deinit(alloc);

    let mut package_root = match (env.get(alloc, "CRAFT_PACKAGE_ROOT")) {
        .{ Some: value } => value,
        .None => return 2,
    };
    defer package_root..&.deinit(alloc);

    if (!package_root.&.ends_with("/app") and !package_root.&.ends_with("\\app")) {
        return 3;
    }
    if (workspace_root.&.eq(package_root.&.as_str())) {
        return 4;
    }
    return 0;
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = workspace::load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Test,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Test).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let test_units = build_plan
            .packages
            .iter()
            .find(|package| package.package_id.name == "app")
            .unwrap()
            .units
            .iter()
            .filter(|unit| unit.target_kind == crate::plan::TargetKind::Test)
            .collect::<Vec<_>>();

        let summary = test(&build_plan, &action_plan, &test_units).unwrap();
        assert_eq!(summary.executed, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_compile_and_link_actions() {
        let root = temp_dir("craft-exec-build");
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
root = "src/main.rn"
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/main.rn"),
            r#"
extern fn main(args: [][]u8) i32 {
    let _ = args;
    return 0;
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
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());

        let summary = build(&build_plan, &action_plan).unwrap();
        assert_eq!(summary.compile_actions, 1);
        assert_eq!(summary.link_actions, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_package_with_direct_external_registry_dependency() {
        let root = temp_dir("craft-exec-external-direct");
        let registry_root = root.join("vendor-registry");
        let log_root = registry_root.join("log").join("1");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(log_root.join("src")).unwrap();

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

[dependencies]
log = "1"

[source.default]
directory = "vendor-registry"
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/main.rn"),
            r#"
extern fn main(args: [][]u8) i32 {
    let _ = args;
    if (log.answer() == 42) {
        return 0;
    }
    return 1;
}
"#,
        )
        .unwrap();
        fs::write(
            log_root.join("Craft.toml"),
            r#"
[package]
name = "log"
version = "1"
kern = "0.7"

[lib]
root = "src/lib.rn"
"#,
        )
        .unwrap();
        fs::write(
            log_root.join("src/lib.rn"),
            r#"
pub fn answer() i32 {
    return 42;
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
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());

        let summary = build(&build_plan, &action_plan).unwrap();
        assert_eq!(summary.compile_actions, 2);
        assert_eq!(summary.link_actions, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_and_runs_hosted_package_with_transitive_external_registry_dependency() {
        let root = temp_dir("craft-exec-external-transitive");
        let registry_root = root.join("vendor-registry");
        let log_root = registry_root.join("log").join("1");
        let corelog_root = registry_root.join("corelog").join("1");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(log_root.join("src")).unwrap();
        fs::create_dir_all(corelog_root.join("src")).unwrap();

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

[dependencies]
log = "1"

[source.default]
directory = "vendor-registry"
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/main.rn"),
            r#"
extern fn main(args: [][]u8) i32 {
    let _ = args;
    if (log.answer() == 42) {
        return 0;
    }
    return 1;
}
"#,
        )
        .unwrap();
        fs::write(
            log_root.join("Craft.toml"),
            r#"
[package]
name = "log"
version = "1"
kern = "0.7"

[lib]
root = "src/lib.rn"

[dependencies]
corelog = "1"
"#,
        )
        .unwrap();
        fs::write(
            log_root.join("src/lib.rn"),
            r#"
pub fn answer() i32 {
    return corelog.base() + 1;
}
"#,
        )
        .unwrap();
        fs::write(
            corelog_root.join("Craft.toml"),
            r#"
[package]
name = "corelog"
version = "1"
kern = "0.7"

[lib]
root = "src/lib.rn"
"#,
        )
        .unwrap();
        fs::write(
            corelog_root.join("src/lib.rn"),
            r#"
pub fn base() i32 {
    return 41;
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
            crate::script::ScriptCommand::Run,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let unit = build_plan.packages[0]
            .units
            .iter()
            .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
            .unwrap();

        let summary = run(&build_plan, &action_plan, unit).unwrap();
        assert!(summary.executable.is_file());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_and_runs_external_package_with_package_local_registry_source() {
        let root = temp_dir("craft-exec-external-package-local-source");
        let registry_root = root.join("vendor-registry");
        let log_root = registry_root.join("log").join("1");
        let nested_registry_root = log_root.join("vendor-nested");
        let corelog_root = nested_registry_root.join("corelog").join("1");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(log_root.join("src")).unwrap();
        fs::create_dir_all(corelog_root.join("src")).unwrap();

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

[dependencies]
log = "1"

[source.default]
directory = "vendor-registry"
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/main.rn"),
            r#"
extern fn main(args: [][]u8) i32 {
    let _ = args;
    if (log.answer() == 42) {
        return 0;
    }
    return 1;
}
"#,
        )
        .unwrap();
        fs::write(
            log_root.join("Craft.toml"),
            r#"
[package]
name = "log"
version = "1"
kern = "0.7"

[lib]
root = "src/lib.rn"

[dependencies]
corelog = { version = "1", registry = "nested" }

[source.nested]
directory = "vendor-nested"
"#,
        )
        .unwrap();
        fs::write(
            log_root.join("src/lib.rn"),
            r#"
pub fn answer() i32 {
    return corelog.base() + 1;
}
"#,
        )
        .unwrap();
        fs::write(
            corelog_root.join("Craft.toml"),
            r#"
[package]
name = "corelog"
version = "1"
kern = "0.7"

[lib]
root = "src/lib.rn"
"#,
        )
        .unwrap();
        fs::write(
            corelog_root.join("src/lib.rn"),
            r#"
pub fn base() i32 {
    return 41;
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
            crate::script::ScriptCommand::Run,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let unit = build_plan.packages[0]
            .units
            .iter()
            .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
            .unwrap();

        let summary = run(&build_plan, &action_plan, unit).unwrap();
        assert!(summary.executable.is_file());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_and_runs_hosted_package_with_generated_source_from_build_script() {
        let root = temp_dir("craft-exec-generated-source");
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
root = "src/main.rn"
"#,
        )
        .unwrap();
        fs::write(
            root.join("build.rn"),
            r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    let path = b.emit_generated(
        "src/main.rn",
        "extern fn main(args: [][]u8) i32 { let _ = args; return 0; }\n"
    );
    b.set_source_root(path);
    b.define_bool("generated", true);
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
            crate::script::ScriptCommand::Run,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let unit = build_plan.packages[0]
            .units
            .iter()
            .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
            .unwrap();

        let summary = run(&build_plan, &action_plan, unit).unwrap();
        assert!(summary.executable.is_file());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_and_runs_hosted_package_with_copied_generated_source_from_build_script() {
        let root = temp_dir("craft-exec-copied-source");
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
root = "src/main.rn"
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
    let path = b.copy_package_file("templates/main.rn", "src/main.rn");
    b.set_source_root(path);
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
            crate::script::ScriptCommand::Run,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let unit = build_plan.packages[0]
            .units
            .iter()
            .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
            .unwrap();

        let summary = run(&build_plan, &action_plan, unit).unwrap();
        assert!(summary.executable.is_file());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_and_runs_hosted_package_with_post_link_artifact_stage_outputs() {
        let root = temp_dir("craft-exec-post-link-stage");
        fs::create_dir_all(root.join("src")).unwrap();
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
            root.join("src").join("main.rn"),
            "extern fn main(args: [][]u8) i32 { let _ = args; return 0; }\n",
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
            crate::script::ScriptCommand::Run,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let unit = build_plan.packages[0]
            .units
            .iter()
            .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
            .unwrap();
        let link_action = action_plan
            .link_actions
            .iter()
            .find(|action| {
                action.package_id.name == "demo"
                    && action.target_kind == crate::plan::TargetKind::Bin
            })
            .unwrap();
        let link_nodes = action_plan.artifact_output_nodes_for_link_action(link_action);

        let summary = run(&build_plan, &action_plan, unit).unwrap();
        assert!(summary.executable.is_file());
        assert!(Path::new(&link_nodes[0].output).exists());
        assert!(Path::new(&link_nodes[1].output).exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_and_runs_hosted_package_with_post_link_directory_stage_outputs() {
        let root = temp_dir("craft-exec-post-link-dir");
        fs::create_dir_all(root.join("src")).unwrap();
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
            root.join("src").join("main.rn"),
            "extern fn main(args: [][]u8) i32 { let _ = args; return 0; }\n",
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
            crate::script::ScriptCommand::Run,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let unit = build_plan.packages[0]
            .units
            .iter()
            .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
            .unwrap();
        let link_action = action_plan
            .link_actions
            .iter()
            .find(|action| {
                action.package_id.name == "demo"
                    && action.target_kind == crate::plan::TargetKind::Bin
            })
            .unwrap();
        let link_nodes = action_plan.artifact_output_nodes_for_link_action(link_action);

        let summary = run(&build_plan, &action_plan, unit).unwrap();
        assert!(summary.executable.is_file());
        assert!(
            Path::new(&link_nodes[0].output)
                .join("config.json")
                .exists()
        );
        assert!(
            Path::new(&link_nodes[0].output)
                .join("images")
                .join("logo.txt")
                .exists()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_and_runs_hosted_package_with_generated_source_from_host_tool() {
        let root = temp_dir("craft-exec-host-tool-generated");
        let app_dir = root.join("app");
        let tool_dir = root.join("tool");
        fs::create_dir_all(app_dir.join("src")).unwrap();
        fs::create_dir_all(tool_dir.join("src")).unwrap();
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
root = "src/placeholder.rn"

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
    let generated = b.emit_generated_from_tool("codegen", "codegen", "src/main.rn", .{});
    b.set_source_root(generated);
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
        fs::write(
            tool_dir.join("src").join("main.rn"),
            r#"
use std.io;
use std.io.Writer;

extern fn main(args: [][]u8) i32 {
    let _ = args;
    let mut out = io.stdout();
    let writer = *mut Writer.{ out..& };
    let _ = writer.write("extern fn main(args: [][]u8) i32 { let _ = args; return 0; }\n");
    return 0;
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = workspace::load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Run,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            crate::build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let unit = build_plan
            .packages
            .iter()
            .find(|package| {
                package.domain == crate::graph::BuildDomain::Target
                    && package.package_id.name == "app"
            })
            .unwrap()
            .units
            .iter()
            .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
            .unwrap();

        let summary = run(&build_plan, &action_plan, unit).unwrap();
        assert!(summary.executable.is_file());
        let crate::build_plan::SourceRootBinding::AbsolutePath(source_root) = &unit.source_root
        else {
            panic!("expected generated source root to be an absolute path binding");
        };
        assert!(Path::new(source_root).is_file());
        let generated = fs::read_to_string(source_root).unwrap();
        assert!(generated.contains("extern fn main"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_and_runs_hosted_package_with_explicit_staged_dependencies() {
        let root = temp_dir("craft-exec-staged-dependencies");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "app"
root = "src/placeholder.rn"
"#,
        )
        .unwrap();
        fs::write(
            root.join("build.rn"),
            r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    let helper = b.stage_generated("tmp/main.template.rn", "extern fn main(args: [][]u8) i32 { let _ = args; return 0; }\n");
    let source = b.stage_copy_output(helper, "src/main.rn");
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
            crate::script::ScriptCommand::Run,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            crate::build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let unit = build_plan.packages[0]
            .units
            .iter()
            .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
            .unwrap();

        let summary = run(&build_plan, &action_plan, unit).unwrap();
        assert!(summary.executable.is_file());
        let crate::build_plan::SourceRootBinding::BuildOutput { path, .. } = &unit.source_root
        else {
            panic!("expected staged generated source root to bind to a build output");
        };
        assert!(Path::new(path).is_file());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_and_runs_hosted_package_with_generated_source_from_external_host_tool() {
        let root = temp_dir("craft-exec-external-host-tool-generated");
        let registry_root = root.join("vendor-registry");
        let tool_root = registry_root.join("codegen").join("1");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(tool_root.join("src")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "app"
root = "src/placeholder.rn"

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
    let generated = b.emit_generated_from_tool("codegen", "codegen", "src/main.rn", .{});
    b.set_source_root(generated);
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
        fs::write(
            tool_root.join("src").join("main.rn"),
            r#"
use std.io;
use std.io.Writer;

extern fn main(args: [][]u8) i32 {
    let _ = args;
    let mut out = io.stdout();
    let writer = *mut Writer.{ out..& };
    let _ = writer.write("extern fn main(args: [][]u8) i32 { let _ = args; return 0; }\n");
    return 0;
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
            crate::script::ScriptCommand::Run,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            crate::build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let unit = build_plan.packages[0]
            .units
            .iter()
            .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
            .unwrap();

        let summary = run(&build_plan, &action_plan, unit).unwrap();
        assert!(summary.executable.is_file());
        let crate::build_plan::SourceRootBinding::AbsolutePath(source_root) = &unit.source_root
        else {
            panic!("expected generated source root to be an absolute path binding");
        };
        assert!(Path::new(source_root).is_file());
        let generated = fs::read_to_string(source_root).unwrap();
        assert!(generated.contains("extern fn main"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_kmeta_package_with_mismatched_declared_identity() {
        let root = temp_dir("craft-exec-kmeta-identity");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("Kmeta.toml"),
            r#"
format_version = 2
kind = "source_snapshot"
package_name = "other"
package_version = "2.0.0"
root_module_name = "other"
entry_module_path = "src/init.rn"
"#,
        )
        .unwrap();

        let err = validate_package_metadata_root(&root, "util", Some("1.0.0")).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("declares package `other` but `util` was required"),
            "unexpected error: {message}"
        );

        let _ = fs::remove_dir_all(root);
    }
}
