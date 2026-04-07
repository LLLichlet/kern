use crate::build_plan::{
    ActionPlan, BuildPlan, BuildUnit, CompileAction, LinkAction, StagedAction, StagedActionKind,
};
use crate::build_state;
use crate::error::{Error, Result};
use crate::graph::{BuildDomain, PackageId};
use crate::manifest::Manifest;
use crate::resolver::ExternalPackageId;
use kernc_driver::{
    CompileReport, CompilerDriver, IncrementalDriverKey, KMETA_MANIFEST_FILE, PhaseTiming,
    load_kmeta_manifest,
};
use kernc_utils::config::{
    CompileOptions, DriverMode, LibraryBundle, OptLevel, RuntimeEntry, RuntimeProvider,
    inject_driver_condition_defines, maybe_inject_base_alias, maybe_inject_std_alias,
    maybe_inject_sys_alias,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

mod external;
mod fingerprint;
mod runtime_packages;

use self::external::{
    build_external_package, compile_actions_index, compile_module_aliases,
    ensure_external_tool_built, link_actions_by_artifact_path, link_inputs_for_action,
    local_library_actions, requested_external_dependencies,
};
use self::fingerprint::{
    base_compile_action_label, build_fingerprint, compile_action_fingerprint, compile_action_label,
    link_action_fingerprint, link_action_label, rt_compile_action_label,
    rt_entry_compile_action_label, std_compile_action_label, sys_compile_action_label,
    write_compile_action_state,
};
use self::runtime_packages::ensure_std_packages_for_actions;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExecutionSummary {
    pub compile_actions: usize,
    pub link_actions: usize,
    pub phase_timings: Vec<PhaseTiming>,
    pub action_timings: Vec<ActionTiming>,
}

fn target_runtime_entry(target_kind: crate::plan::TargetKind) -> RuntimeEntry {
    match target_kind {
        crate::plan::TargetKind::Lib => RuntimeEntry::None,
        crate::plan::TargetKind::Bin
        | crate::plan::TargetKind::Test
        | crate::plan::TargetKind::Example => RuntimeEntry::Crt,
    }
}

fn target_runtime_provider(target_kind: crate::plan::TargetKind) -> RuntimeProvider {
    match target_kind {
        crate::plan::TargetKind::Lib => RuntimeProvider::None,
        crate::plan::TargetKind::Bin
        | crate::plan::TargetKind::Test
        | crate::plan::TargetKind::Example => RuntimeProvider::Toolchain,
    }
}

fn target_library_bundle(_target_kind: crate::plan::TargetKind) -> LibraryBundle {
    LibraryBundle::Std
}

fn target_runtime_libc(target_kind: crate::plan::TargetKind) -> bool {
    !matches!(target_kind, crate::plan::TargetKind::Lib)
}

fn default_target_compile_options(target_kind: crate::plan::TargetKind) -> CompileOptions {
    CompileOptions {
        runtime_entry: target_runtime_entry(target_kind),
        runtime_provider: target_runtime_provider(target_kind),
        runtime_libc: target_runtime_libc(target_kind),
        library_bundle: target_library_bundle(target_kind),
        ..CompileOptions::default()
    }
}

fn inject_target_library_aliases(options: &mut CompileOptions) {
    if options.module_interface_aliases.contains_key("std") {
        return;
    }
    maybe_inject_base_alias(options);
    maybe_inject_sys_alias(options);
    if !options.module_interface_aliases.contains_key("std") {
        maybe_inject_std_alias(options);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionTimingKind {
    Compile,
    Link,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionTiming {
    pub kind: ActionTimingKind,
    pub label: String,
    pub phase_timings: Vec<PhaseTiming>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    pub executable: PathBuf,
    pub build: ExecutionSummary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSummary {
    pub executed: usize,
    pub build: ExecutionSummary,
}

impl ExecutionSummary {
    pub fn total_duration(&self) -> Duration {
        self.phase_timings
            .iter()
            .map(|phase| phase.duration)
            .sum::<Duration>()
    }

    fn absorb(&mut self, other: ExecutionSummary) {
        self.compile_actions += other.compile_actions;
        self.link_actions += other.link_actions;
        for phase in other.phase_timings {
            if let Some(existing) = self
                .phase_timings
                .iter_mut()
                .find(|existing| existing.name == phase.name)
            {
                existing.duration += phase.duration;
            } else {
                self.phase_timings.push(phase);
            }
        }
        self.action_timings.extend(other.action_timings);
    }

    fn record_action(
        &mut self,
        kind: ActionTimingKind,
        label: impl Into<String>,
        phase_timings: Vec<PhaseTiming>,
    ) {
        if phase_timings.is_empty() {
            return;
        }

        for phase in &phase_timings {
            if let Some(existing) = self
                .phase_timings
                .iter_mut()
                .find(|existing| existing.name == phase.name)
            {
                existing.duration += phase.duration;
            } else {
                self.phase_timings.push(*phase);
            }
        }

        self.action_timings.push(ActionTiming {
            kind,
            label: label.into(),
            phase_timings,
        });
    }
}

pub fn build(build_plan: &BuildPlan, action_plan: &ActionPlan) -> Result<ExecutionSummary> {
    build_with_command(build_plan, action_plan, crate::script::ScriptCommand::Build)
}

pub(crate) fn materialize_analysis_inputs(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
) -> Result<()> {
    let source_config = load_source_config(build_plan)?;
    let profile_selection = profile_selection_for_action_plan(action_plan);
    let mut built_std_packages = BTreeMap::new();
    let mut driver_families = BTreeMap::new();
    let mut summary = ExecutionSummary::default();
    ensure_std_packages_for_actions(
        &build_plan.workspace_root,
        &action_plan.compile_actions,
        &mut built_std_packages,
        &mut driver_families,
        &mut summary,
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
    let indexes = ActionIndexes {
        action_plan,
        compile_action_index: &compile_action_index,
        local_library_actions: &local_library_actions,
        link_action_index: &link_action_index,
    };
    let config = ExecutionConfig {
        source_config: &source_config,
        dependency_workspace_root: &build_plan.workspace_root,
        command: crate::script::ScriptCommand::Build,
        profile_selection,
        std_workspace_root: &build_plan.workspace_root,
    };
    let mut session = ExecutionSession {
        indexes,
        config,
        external: ExternalArtifacts {
            built_std_packages: &mut built_std_packages,
            built_external_packages: &mut built_external_packages,
            built_external_tools: &mut built_external_tools,
            external_build_stack: &mut external_build_stack,
            driver_families: &mut driver_families,
        },
        state: ExecutionState {
            compiled: &mut compiled,
            linked: &mut linked,
            staged_outputs: &mut staged_outputs,
            execution_summary: &mut summary,
        },
    };

    for action in &action_plan.compile_actions {
        if action.domain != BuildDomain::Target {
            continue;
        }
        execute_staged_actions(
            action.compile_inputs.as_slice(),
            action_plan.build_nodes.as_slice(),
            action.required_source_path(),
            &mut session,
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
    interface_aliases: BTreeMap<String, PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BuiltLibraryPackage {
    metadata_root_path: PathBuf,
    object_path: PathBuf,
    interface_aliases: BTreeMap<String, PathBuf>,
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
    _private: (),
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

#[derive(Debug, Clone, Copy)]
struct ExecutionConfig<'a> {
    source_config: &'a SourceConfigContext,
    dependency_workspace_root: &'a Path,
    command: crate::script::ScriptCommand,
    profile_selection: crate::script::ProfileSelection,
    std_workspace_root: &'a Path,
}

#[derive(Debug, Clone, Copy)]
struct ActionIndexes<'a> {
    action_plan: &'a ActionPlan,
    compile_action_index: &'a BTreeMap<ActionKey, CompileAction>,
    local_library_actions: &'a BTreeMap<PackageInstanceKey, CompileAction>,
    link_action_index: &'a BTreeMap<PathBuf, LinkAction>,
}

struct ExternalArtifacts<'a> {
    built_std_packages: &'a mut BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &'a mut BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    built_external_tools: &'a mut BTreeMap<ExternalToolKey, PathBuf>,
    external_build_stack: &'a mut BTreeSet<ExternalPackageId>,
    driver_families: &'a mut BTreeMap<IncrementalDriverKey, CompilerDriver>,
}

#[derive(Debug)]
struct ExecutionState<'a> {
    compiled: &'a mut BTreeSet<PathBuf>,
    linked: &'a mut BTreeSet<PathBuf>,
    staged_outputs: &'a mut BTreeSet<PathBuf>,
    execution_summary: &'a mut ExecutionSummary,
}

struct ExecutionSession<'a> {
    indexes: ActionIndexes<'a>,
    config: ExecutionConfig<'a>,
    external: ExternalArtifacts<'a>,
    state: ExecutionState<'a>,
}

pub(super) fn compile_with_shared_driver(
    driver_families: &mut BTreeMap<IncrementalDriverKey, CompilerDriver>,
    options: CompileOptions,
) -> Option<CompileReport> {
    let key = IncrementalDriverKey::from_options(&options);
    if let Some(shared) = driver_families
        .get(&key)
        .and_then(|driver| driver.share_incremental_state(options.clone()))
    {
        return shared.compile_with_report();
    }

    let driver = CompilerDriver::new(options);
    let report = driver.compile_with_report();
    driver_families.entry(key).or_insert(driver);
    report
}

fn build_with_command(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    command: crate::script::ScriptCommand,
) -> Result<ExecutionSummary> {
    let source_config = load_source_config(build_plan)?;
    let profile_selection = profile_selection_for_action_plan(action_plan);
    let mut built_std_packages = BTreeMap::new();
    let mut driver_families = BTreeMap::new();
    let mut external_summary = ExecutionSummary::default();
    ensure_std_packages_for_actions(
        &build_plan.workspace_root,
        &action_plan.compile_actions,
        &mut built_std_packages,
        &mut driver_families,
        &mut external_summary,
    )?;
    let mut built_external_packages = BTreeMap::new();
    let mut built_external_tools = BTreeMap::new();
    let mut external_build_stack = BTreeSet::new();
    let config = ExecutionConfig {
        source_config: &source_config,
        dependency_workspace_root: &build_plan.workspace_root,
        command,
        profile_selection,
        std_workspace_root: &build_plan.workspace_root,
    };
    let mut external = ExternalArtifacts {
        built_std_packages: &mut built_std_packages,
        built_external_packages: &mut built_external_packages,
        built_external_tools: &mut built_external_tools,
        external_build_stack: &mut external_build_stack,
        driver_families: &mut driver_families,
    };

    for dep in requested_external_dependencies(action_plan) {
        build_external_package(&dep, config, &mut external, &mut external_summary)?;
    }

    let compile_action_index = compile_actions_index(&action_plan.compile_actions);
    let local_library_actions = local_library_actions(&action_plan.compile_actions);
    let link_action_index = link_actions_by_artifact_path(&action_plan.link_actions);
    let mut compiled = BTreeSet::new();
    let mut linked = BTreeSet::new();
    let mut staged_outputs = BTreeSet::new();
    let mut local_summary = ExecutionSummary::default();
    let indexes = ActionIndexes {
        action_plan,
        compile_action_index: &compile_action_index,
        local_library_actions: &local_library_actions,
        link_action_index: &link_action_index,
    };
    let mut session = ExecutionSession {
        indexes,
        config,
        external,
        state: ExecutionState {
            compiled: &mut compiled,
            linked: &mut linked,
            staged_outputs: &mut staged_outputs,
            execution_summary: &mut local_summary,
        },
    };

    for action in &action_plan.link_actions {
        if action.domain != BuildDomain::Target {
            continue;
        }
        ensure_link_action_built(action, &mut session)?;
    }
    for action in &action_plan.compile_actions {
        if action.domain != BuildDomain::Target {
            continue;
        }
        ensure_compile_action_built(action, &mut session)?;
    }

    external_summary.absorb(local_summary);
    Ok(external_summary)
}

pub fn run(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    unit: &BuildUnit,
) -> Result<RunSummary> {
    let build = build_with_command(build_plan, action_plan, crate::script::ScriptCommand::Run)?;
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
        build,
    })
}

pub fn test(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    units: &[&BuildUnit],
) -> Result<TestSummary> {
    let build = build_with_command(build_plan, action_plan, crate::script::ScriptCommand::Test)?;

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

    Ok(TestSummary { executed, build })
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

fn apply_manifest_runtime_options(
    manifest_path: &Path,
    options: &mut CompileOptions,
) -> Result<()> {
    let manifest = Manifest::load(manifest_path)?;
    manifest.validate(manifest_path)?;
    manifest.apply_runtime_options(options);
    Ok(())
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

fn prepare_output_path(path: &Path, expects_directory: bool) -> Result<()> {
    if expects_directory {
        if path.is_file() {
            fs::remove_file(path).map_err(|err| Error::from_io(path, err))?;
        } else if path.is_dir() {
            fs::remove_dir_all(path).map_err(|err| Error::from_io(path, err))?;
        }
        return Ok(());
    }

    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|err| Error::from_io(path, err))?;
    }
    Ok(())
}

fn profile_opt_level(profile: &crate::script::ScriptProfile) -> OptLevel {
    match profile.opt {
        0 => OptLevel::O0,
        1 => OptLevel::O1,
        2 => OptLevel::O2,
        _ => OptLevel::O3,
    }
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

fn source_config_context(_manifest_path: PathBuf, _manifest: Manifest) -> SourceConfigContext {
    SourceConfigContext { _private: () }
}

impl SourceConfigContext {
    fn with_child(&self, _manifest_path: PathBuf, _manifest: &Manifest) -> Self {
        Self { _private: () }
    }
}

fn ensure_compile_action_built(
    action: &CompileAction,
    session: &mut ExecutionSession<'_>,
) -> Result<bool> {
    if session.state.compiled.contains(&action.object_path) {
        return Ok(false);
    }

    for dep in &action.local_dependencies {
        if let Some(dep_action) = session
            .indexes
            .local_library_actions
            .get(&PackageInstanceKey {
                domain: dep.domain,
                package_id: dep.package_id.clone(),
            })
        {
            ensure_compile_action_built(dep_action, session)?;
        }
    }

    execute_staged_actions(
        action.compile_inputs.as_slice(),
        session.indexes.action_plan.build_nodes.as_slice(),
        action.required_source_path(),
        session,
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
        opt_level: profile_opt_level(&action.profile),
        ..default_target_compile_options(action.target_kind)
    };
    apply_manifest_runtime_options(&action.manifest_path, &mut options)?;
    apply_host_linker_env(&mut options);
    options.module_interface_aliases = compile_module_aliases(
        action,
        session.indexes.local_library_actions,
        session
            .external
            .built_std_packages
            .get(&action.profile.name),
        session.external.built_external_packages,
    )?;
    inject_target_library_aliases(&mut options);
    inject_driver_condition_defines(&mut options);
    options.custom_defines.extend(compile_time_defines(
        &action.cfg,
        &action.define,
        action.source_path(),
    )?);
    let toolchain_digest = build_state::current_process_digest()?;
    let fingerprint = compile_action_fingerprint(action, &options, &toolchain_digest);

    if build_state::action_state_is_current(&action.object_path, &fingerprint)? {
        session.state.compiled.insert(action.object_path.clone());
        return Ok(false);
    }

    let Some(report) = compile_with_shared_driver(session.external.driver_families, options) else {
        return Err(Error::Execution(format!(
            "compile failed for `{}`",
            action.source_path().display()
        )));
    };

    write_compile_action_state(action, &report, fingerprint)?;

    session.state.compiled.insert(action.object_path.clone());
    session.state.execution_summary.compile_actions += 1;
    session.state.execution_summary.record_action(
        ActionTimingKind::Compile,
        compile_action_label(action),
        report.phase_timings,
    );
    Ok(true)
}

fn execute_staged_actions(
    root_ids: &[usize],
    build_nodes: &[StagedAction],
    required_path: Option<&Path>,
    session: &mut ExecutionSession<'_>,
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
        execute_staged_action(action, &action_index, &mut active, session)?;
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

fn execute_staged_action(
    action: &StagedAction,
    action_index: &BTreeMap<usize, &StagedAction>,
    active: &mut BTreeSet<usize>,
    session: &mut ExecutionSession<'_>,
) -> Result<bool> {
    let output_path = PathBuf::from(&action.output);
    if session.state.staged_outputs.contains(&output_path) {
        return Ok(false);
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
        execute_staged_action(dependency, action_index, active, session)?;
    }
    active.remove(&action.id);
    let toolchain_digest = build_state::current_process_digest()?;
    let mut input_paths = Vec::new();
    let fingerprint = match &action.kind {
        StagedActionKind::WriteFile { contents } => build_fingerprint(&[
            "kind=write".to_string(),
            format!("output={}", output_path.display()),
            format!("contents={}", build_state::hash_string(contents)),
        ]),
        StagedActionKind::RunTool { tool, args } => {
            let tool_path = PathBuf::from(&tool.executable_path);
            match &tool.origin {
                crate::script::BuildScriptToolOrigin::LocalPackage { .. } => {
                    if let Some(link_action) = session.indexes.link_action_index.get(&tool_path) {
                        ensure_link_action_built(link_action, session)?;
                    }
                }
                crate::script::BuildScriptToolOrigin::ExternalPackage { .. } => {
                    ensure_external_tool_built(
                        tool,
                        session.config,
                        &mut session.external,
                        session.state.execution_summary,
                    )?;
                }
            }
            input_paths.push(tool_path.clone());
            let mut lines = vec![
                "kind=run-tool".to_string(),
                format!("toolchain={toolchain_digest}"),
                format!("tool={}", tool_path.display()),
                format!("output={}", output_path.display()),
            ];
            lines.extend(args.iter().map(|arg| format!("arg={arg}")));
            build_fingerprint(&lines)
        }
        StagedActionKind::CopyFile { source } => {
            let input_path = PathBuf::from(source);
            input_paths.push(input_path.clone());
            build_fingerprint(&[
                "kind=copy-file".to_string(),
                format!("input={}", input_path.display()),
                format!("output={}", output_path.display()),
            ])
        }
        StagedActionKind::CopyDirectory { source } => {
            let input_path = PathBuf::from(source);
            input_paths.push(input_path.clone());
            build_fingerprint(&[
                "kind=copy-dir".to_string(),
                format!("input={}", input_path.display()),
                format!("output={}", output_path.display()),
            ])
        }
    };

    if build_state::action_state_is_current(&output_path, &fingerprint)? {
        session.state.staged_outputs.insert(output_path);
        return Ok(false);
    }

    ensure_parent_dir(&output_path)?;

    match &action.kind {
        StagedActionKind::WriteFile { contents } => {
            prepare_output_path(&output_path, false)?;
            fs::write(&output_path, contents).map_err(|err| Error::from_io(&output_path, err))?;
        }
        StagedActionKind::RunTool { tool, args } => {
            prepare_output_path(&output_path, false)?;
            let tool_path = PathBuf::from(&tool.executable_path);
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
            prepare_output_path(&output_path, false)?;
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
            prepare_output_path(&output_path, true)?;
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

    build_state::record_action_state(
        &output_path,
        fingerprint,
        &input_paths,
        std::slice::from_ref(&output_path),
    )?;
    session.state.staged_outputs.insert(output_path);
    Ok(true)
}

fn ensure_link_action_built(
    action: &LinkAction,
    session: &mut ExecutionSession<'_>,
) -> Result<bool> {
    if session.state.linked.contains(&action.artifact_path) {
        return Ok(false);
    }
    let compile_action = session
        .indexes
        .compile_action_index
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
    ensure_compile_action_built(compile_action, session)?;

    ensure_parent_dir(&action.artifact_path)?;

    let mut options = CompileOptions {
        output_file: action.artifact_path.to_string_lossy().to_string(),
        driver_mode: DriverMode::LinkOnly,
        report_progress: false,
        ..default_target_compile_options(action.target_kind)
    };
    apply_manifest_runtime_options(&action.manifest_path, &mut options)?;
    apply_host_linker_env(&mut options);
    let linker_inputs = link_inputs_for_action(
        action,
        session.indexes.action_plan,
        session.indexes.local_library_actions,
        session.external.built_std_packages,
        session.external.built_external_packages,
    )?;
    options.linker_inputs = linker_inputs
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();
    options.linker_libraries = action.link.system_libs.clone();
    options.linker_search_paths = action.link.search_paths.clone();
    options.linker_args = action.link.args.clone();
    for framework in &action.link.frameworks {
        options.linker_args.push("-framework".to_string());
        options.linker_args.push(framework.clone());
    }
    let toolchain_digest = build_state::current_process_digest()?;
    let fingerprint = link_action_fingerprint(action, &options, &linker_inputs, &toolchain_digest);
    let linked_now = if build_state::action_state_is_current(&action.artifact_path, &fingerprint)? {
        false
    } else {
        let driver = CompilerDriver::new(options);
        let Some(report) = driver.compile_with_report() else {
            return Err(Error::Execution(format!(
                "link failed for `{}`",
                action.artifact_path.display()
            )));
        };
        build_state::record_action_state(
            &action.artifact_path,
            fingerprint,
            &linker_inputs,
            std::slice::from_ref(&action.artifact_path),
        )?;
        session.state.execution_summary.link_actions += 1;
        session.state.execution_summary.record_action(
            ActionTimingKind::Link,
            link_action_label(action),
            report.phase_timings,
        );
        true
    };

    session.state.linked.insert(action.artifact_path.clone());

    execute_staged_actions(
        action.artifact_outputs.as_slice(),
        session.indexes.action_plan.build_nodes.as_slice(),
        None,
        session,
    )?;
    Ok(linked_now)
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

fn profile_selection_for_action_plan(action_plan: &ActionPlan) -> crate::script::ProfileSelection {
    match action_plan
        .compile_actions
        .first()
        .map(|action| action.profile.name.as_str())
    {
        Some("release") => crate::script::ProfileSelection::Release,
        _ => crate::script::ProfileSelection::Dev,
    }
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
mod tests;
