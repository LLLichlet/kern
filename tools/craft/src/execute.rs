use crate::build_plan::{
    ActionPlan, BuildPlan, BuildUnit, CompileAction, LinkAction, StagedAction, StagedActionKind,
};
use crate::build_state;
use crate::elaborate::{self, FeatureSelection};
use crate::error::{Error, Result};
use crate::graph::{BuildDomain, PackageId};
use crate::manifest::Manifest;
use crate::resolver::{ExternalPackageId, ResolvedExternalPackage, ResolvedGraph};
use crate::source;
use crate::workspace;
use kernc_driver::{
    CompileReport, CompilerDriver, KMETA_MANIFEST_FILE, PhaseTiming, load_kmeta_manifest,
};
use kernc_utils::config::{
    CompileOptions, DriverMode, LibraryBundle, OptLevel, RuntimeEntry, RuntimeProvider,
    inject_driver_condition_defines, maybe_inject_base_alias, maybe_inject_rt_alias,
    maybe_inject_std_alias, maybe_inject_sys_alias, resolve_base_path, resolve_rt_path,
    resolve_std_path, resolve_sys_path,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

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
    maybe_inject_rt_alias(options);
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
    let mut summary = ExecutionSummary::default();
    ensure_std_packages_for_actions(
        &build_plan.workspace_root,
        &action_plan.compile_actions,
        &mut built_std_packages,
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
            profile_selection,
            &build_plan.workspace_root,
            &mut built_std_packages,
            &mut built_external_packages,
            &mut built_external_tools,
            &mut external_build_stack,
            &mut compiled,
            &mut linked,
            &mut summary,
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

fn build_with_command(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    command: crate::script::ScriptCommand,
) -> Result<ExecutionSummary> {
    let source_config = load_source_config(build_plan)?;
    let profile_selection = profile_selection_for_action_plan(action_plan);
    let mut built_std_packages = BTreeMap::new();
    let mut external_summary = ExecutionSummary::default();
    ensure_std_packages_for_actions(
        &build_plan.workspace_root,
        &action_plan.compile_actions,
        &mut built_std_packages,
        &mut external_summary,
    )?;
    let mut built_external_packages = BTreeMap::new();
    let mut built_external_tools = BTreeMap::new();
    let mut external_build_stack = BTreeSet::new();

    for dep in requested_external_dependencies(action_plan) {
        build_external_package(
            &source_config,
            &build_plan.workspace_root,
            &dep,
            command,
            profile_selection,
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
    let mut local_summary = ExecutionSummary::default();

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
            profile_selection,
            &build_plan.workspace_root,
            &mut built_std_packages,
            &mut built_external_packages,
            &mut built_external_tools,
            &mut external_build_stack,
            &mut compiled,
            &mut linked,
            &mut staged_outputs,
            &mut local_summary,
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
            profile_selection,
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
            &mut local_summary,
        )?;
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

fn apply_manifest_runtime_options(manifest_path: &Path, options: &mut CompileOptions) -> Result<()> {
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

fn build_fingerprint(lines: &[String]) -> String {
    build_state::hash_string(&lines.join("\n"))
}

fn map_fingerprint_lines(
    label: &str,
    values: &std::collections::HashMap<String, String>,
) -> Vec<String> {
    let mut entries = values.iter().collect::<Vec<_>>();
    entries.sort_by(|lhs, rhs| lhs.0.cmp(rhs.0));
    entries
        .into_iter()
        .map(|(key, value)| format!("{label}:{key}={value}"))
        .collect()
}

fn compile_action_fingerprint(
    action: &CompileAction,
    options: &CompileOptions,
    toolchain_digest: &str,
) -> String {
    let mut lines = vec![
        "kind=compile".to_string(),
        format!("toolchain={toolchain_digest}"),
        format!("target={}", options.target.triple),
        format!("source={}", action.source_path().display()),
        format!("object={}", action.object_path.display()),
        format!("profile={}", action.profile.name),
        format!("opt={}", action.profile.opt),
        format!("debug={}", action.profile.debug),
        format!(
            "root={}",
            options.root_module_name.as_deref().unwrap_or_default()
        ),
        format!("runtime_entry={}", options.runtime_entry.as_str()),
        format!("runtime_provider={}", options.runtime_provider.as_str()),
        format!("runtime_libc={}", options.runtime_libc),
        format!("library_bundle={}", options.library_bundle.as_str()),
    ];
    if let Some(metadata_output) = options.metadata_output.as_deref() {
        lines.push(format!("metadata={metadata_output}"));
    }
    lines.extend(map_fingerprint_lines("define", &options.custom_defines));
    lines.extend(map_fingerprint_lines(
        "ifalias",
        &options.module_interface_aliases,
    ));
    build_fingerprint(&lines)
}

fn link_action_fingerprint(
    action: &LinkAction,
    options: &CompileOptions,
    linker_inputs: &[PathBuf],
    toolchain_digest: &str,
) -> String {
    let mut lines = vec![
        "kind=link".to_string(),
        format!("toolchain={toolchain_digest}"),
        format!("artifact={}", action.artifact_path.display()),
        format!("linker={}", options.linker_cmd),
    ];
    lines.extend(
        linker_inputs
            .iter()
            .map(|path| format!("input={}", path.display())),
    );
    lines.extend(
        options
            .linker_search_paths
            .iter()
            .map(|path| format!("search={path}")),
    );
    lines.extend(
        options
            .linker_libraries
            .iter()
            .map(|library| format!("lib={library}")),
    );
    lines.extend(options.linker_args.iter().map(|arg| format!("arg={arg}")));
    build_fingerprint(&lines)
}

fn write_compile_action_state(
    action: &CompileAction,
    report: &CompileReport,
    fingerprint: String,
) -> Result<()> {
    let mut inputs = report.loaded_sources.clone();
    inputs.sort();
    inputs.dedup();

    let mut outputs = vec![action.object_path.clone()];
    if let Some(metadata_path) = &action.metadata_path {
        outputs.push(metadata_path.clone());
    }

    build_state::record_action_state(&action.object_path, fingerprint, &inputs, &outputs)
}

fn compile_action_label(action: &CompileAction) -> String {
    format!(
        "{}:{} -> {}",
        action.package_id.name,
        action.source_path().display(),
        action.object_path.display()
    )
}

fn link_action_label(action: &LinkAction) -> String {
    format!(
        "{}:{}",
        action.package_id.name,
        action.artifact_path.display()
    )
}

fn std_compile_action_label(profile: &str) -> String {
    format!("std ({profile})")
}

fn rt_compile_action_label(profile: &str) -> String {
    format!("rt ({profile})")
}

fn base_compile_action_label(profile: &str) -> String {
    format!("base ({profile})")
}

fn sys_compile_action_label(profile: &str) -> String {
    format!("sys ({profile})")
}

fn rt_entry_compile_action_label(profile: &str) -> String {
    format!("rt-entry ({profile})")
}

fn interface_alias_strings(aliases: &BTreeMap<String, PathBuf>) -> HashMap<String, String> {
    aliases
        .iter()
        .map(|(name, path)| (name.clone(), path.to_string_lossy().to_string()))
        .collect()
}

fn extend_interface_aliases(options: &mut CompileOptions, aliases: &BTreeMap<String, PathBuf>) {
    options
        .module_interface_aliases
        .extend(interface_alias_strings(aliases));
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

#[allow(clippy::too_many_arguments)]
fn ensure_compile_action_built(
    action: &CompileAction,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    link_action_index: &BTreeMap<PathBuf, LinkAction>,
    source_config: &SourceConfigContext,
    dependency_workspace_root: &Path,
    command: crate::script::ScriptCommand,
    profile_selection: crate::script::ProfileSelection,
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
    execution_summary: &mut ExecutionSummary,
) -> Result<bool> {
    if compiled.contains(&action.object_path) {
        return Ok(false);
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
                profile_selection,
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
                execution_summary,
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
        profile_selection,
        std_workspace_root,
        built_std_packages,
        built_external_packages,
        built_external_tools,
        external_build_stack,
        compiled,
        linked,
        execution_summary,
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
        local_library_actions,
        built_std_packages.get(&action.profile.name),
        built_external_packages,
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
        compiled.insert(action.object_path.clone());
        return Ok(false);
    }

    let driver = CompilerDriver::new(options);
    let Some(report) = driver.compile_with_report() else {
        return Err(Error::Execution(format!(
            "compile failed for `{}`",
            action.source_path().display()
        )));
    };

    write_compile_action_state(action, &report, fingerprint)?;

    compiled.insert(action.object_path.clone());
    execution_summary.compile_actions += 1;
    execution_summary.record_action(
        ActionTimingKind::Compile,
        compile_action_label(action),
        report.phase_timings,
    );
    Ok(true)
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
    profile_selection: crate::script::ProfileSelection,
    std_workspace_root: &Path,
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &mut BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    built_external_tools: &mut BTreeMap<ExternalToolKey, PathBuf>,
    external_build_stack: &mut BTreeSet<ExternalPackageId>,
    compiled: &mut BTreeSet<PathBuf>,
    linked: &mut BTreeSet<PathBuf>,
    execution_summary: &mut ExecutionSummary,
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
            profile_selection,
            std_workspace_root,
            built_std_packages,
            built_external_packages,
            built_external_tools,
            external_build_stack,
            compiled,
            linked,
            execution_summary,
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
    profile_selection: crate::script::ProfileSelection,
    std_workspace_root: &Path,
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &mut BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    built_external_tools: &mut BTreeMap<ExternalToolKey, PathBuf>,
    external_build_stack: &mut BTreeSet<ExternalPackageId>,
    compiled: &mut BTreeSet<PathBuf>,
    linked: &mut BTreeSet<PathBuf>,
    execution_summary: &mut ExecutionSummary,
) -> Result<bool> {
    let output_path = PathBuf::from(&action.output);
    if staged_outputs.contains(&output_path) {
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
            profile_selection,
            std_workspace_root,
            built_std_packages,
            built_external_packages,
            built_external_tools,
            external_build_stack,
            compiled,
            linked,
            execution_summary,
        )?;
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
                            profile_selection,
                            std_workspace_root,
                            built_std_packages,
                            built_external_packages,
                            built_external_tools,
                            external_build_stack,
                            compiled,
                            linked,
                            staged_outputs,
                            execution_summary,
                        )?;
                    }
                }
                crate::script::BuildScriptToolOrigin::ExternalPackage { .. } => {
                    ensure_external_tool_built(
                        tool,
                        source_config,
                        dependency_workspace_root,
                        command,
                        profile_selection,
                        std_workspace_root,
                        built_std_packages,
                        built_external_packages,
                        built_external_tools,
                        external_build_stack,
                        execution_summary,
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
        staged_outputs.insert(output_path);
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
    staged_outputs.insert(output_path);
    Ok(true)
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
    profile_selection: crate::script::ProfileSelection,
    std_workspace_root: &Path,
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &mut BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    built_external_tools: &mut BTreeMap<ExternalToolKey, PathBuf>,
    external_build_stack: &mut BTreeSet<ExternalPackageId>,
    compiled: &mut BTreeSet<PathBuf>,
    linked: &mut BTreeSet<PathBuf>,
    staged_outputs: &mut BTreeSet<PathBuf>,
    execution_summary: &mut ExecutionSummary,
) -> Result<bool> {
    if linked.contains(&action.artifact_path) {
        return Ok(false);
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
        profile_selection,
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
        execution_summary,
    )?;

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
        action_plan,
        local_library_actions,
        built_std_packages,
        built_external_packages,
    )?;
    options.linker_inputs = linker_inputs
        .iter()
        .cloned()
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
        execution_summary.link_actions += 1;
        execution_summary.record_action(
            ActionTimingKind::Link,
            link_action_label(action),
            report.phase_timings,
        );
        true
    };

    linked.insert(action.artifact_path.clone());

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
        profile_selection,
        std_workspace_root,
        built_std_packages,
        built_external_packages,
        built_external_tools,
        external_build_stack,
        compiled,
        linked,
        execution_summary,
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
    profile_selection: crate::script::ProfileSelection,
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

    let loaded = load_external_package_actions(
        source_config,
        dependency_workspace_root,
        dep,
        command,
        profile_selection,
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
            &loaded.source_config,
            &loaded.workspace_root,
            &child,
            command,
            profile_selection,
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
        external_summary,
    )?;

    let compile_summary = execute_compile_actions(
        &required_library_actions,
        &loaded.action_plan,
        &loaded.compile_action_index,
        &loaded.local_library_actions,
        &loaded.link_action_index,
        &loaded.source_config,
        &loaded.workspace_root,
        command,
        profile_selection,
        std_workspace_root,
        built_std_packages,
        built_external_packages,
        built_external_tools,
        external_build_stack,
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
    external_build_stack.remove(dep);
    Ok(())
}

fn fetch_external_package(
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

#[allow(clippy::too_many_arguments)]
fn ensure_external_tool_built(
    tool: &crate::script::BuildScriptTool,
    source_config: &SourceConfigContext,
    dependency_workspace_root: &Path,
    command: crate::script::ScriptCommand,
    profile_selection: crate::script::ProfileSelection,
    std_workspace_root: &Path,
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &mut BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    built_external_tools: &mut BTreeMap<ExternalToolKey, PathBuf>,
    external_build_stack: &mut BTreeSet<ExternalPackageId>,
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
    if built_external_tools.contains_key(&tool_key) {
        return Ok(());
    }

    let loaded = load_external_package_actions(
        source_config,
        dependency_workspace_root,
        dependency_id,
        command,
        profile_selection,
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
            &loaded.source_config,
            &loaded.workspace_root,
            &child,
            command,
            profile_selection,
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
        &mut external_summary,
    )?;
    let compile_summary = execute_compile_actions(
        &required_compile_actions,
        &loaded.action_plan,
        &loaded.compile_action_index,
        &loaded.local_library_actions,
        &loaded.link_action_index,
        &loaded.source_config,
        &loaded.workspace_root,
        command,
        profile_selection,
        std_workspace_root,
        built_std_packages,
        built_external_packages,
        built_external_tools,
        external_build_stack,
    )?;
    external_summary.absorb(compile_summary);

    let mut compiled = BTreeSet::new();
    let mut linked = BTreeSet::new();
    let mut staged_outputs = BTreeSet::new();
    let mut summary = ExecutionSummary::default();
    ensure_link_action_built(
        root_link_action,
        &loaded.action_plan,
        &loaded.compile_action_index,
        &loaded.local_library_actions,
        &loaded.link_action_index,
        &loaded.source_config,
        &loaded.workspace_root,
        command,
        profile_selection,
        std_workspace_root,
        built_std_packages,
        built_external_packages,
        built_external_tools,
        external_build_stack,
        &mut compiled,
        &mut linked,
        &mut staged_outputs,
        &mut summary,
    )?;
    execution_summary.absorb(external_summary);
    execution_summary.absorb(summary);
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
    profile_selection: crate::script::ProfileSelection,
    std_workspace_root: &Path,
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &mut BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    built_external_tools: &mut BTreeMap<ExternalToolKey, PathBuf>,
    external_build_stack: &mut BTreeSet<ExternalPackageId>,
) -> Result<ExecutionSummary> {
    let mut compiled = BTreeSet::new();
    let mut linked = BTreeSet::new();
    let mut staged_outputs = BTreeSet::new();
    let mut summary = ExecutionSummary::default();
    for action in actions {
        let _ = ensure_compile_action_built(
            action,
            local_library_actions,
            link_action_index,
            source_config,
            dependency_workspace_root,
            command,
            profile_selection,
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
            &mut summary,
        )?;
    }
    Ok(summary)
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
    execution_summary: &mut ExecutionSummary,
) -> Result<()> {
    let profiles = actions
        .iter()
        .map(|action| action.profile.name.clone())
        .collect::<BTreeSet<_>>();
    for profile in profiles {
        build_std_package(
            workspace_root,
            &profile,
            built_std_packages,
            execution_summary,
        )?;
    }
    Ok(())
}

fn build_std_package(
    workspace_root: &Path,
    profile: &str,
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
    execution_summary: &mut ExecutionSummary,
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
    let built_rt = build_rt_package(workspace_root, profile, execution_summary)?;
    let built_sys = build_sys_package(workspace_root, profile, execution_summary)?;
    let rt_entry_object_path = build_rt_entry_package(
        workspace_root,
        profile,
        execution_summary,
        &built_sys,
    )?;

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
        opt_level: if profile == "release" {
            OptLevel::O3
        } else {
            OptLevel::O0
        },
        library_bundle: LibraryBundle::Std,
        ..CompileOptions::default()
    };
    apply_host_linker_env(&mut options);
    options
        .module_aliases
        .insert("std".to_string(), std_root.to_string_lossy().to_string());
    extend_interface_aliases(&mut options, &built_sys.interface_aliases);
    options.module_interface_aliases.insert(
        "sys".to_string(),
        built_sys.metadata_root_path.to_string_lossy().to_string(),
    );
    inject_driver_condition_defines(&mut options);
    let toolchain_digest = build_state::current_process_digest()?;
    let std_fingerprint = build_fingerprint(&[
        "std_runtime_layout=v3".to_string(),
        "kind=compile-std".to_string(),
        format!("toolchain={toolchain_digest}"),
        format!("profile={profile}"),
        format!("source={}", source_path.display()),
        format!("object={}", object_path.display()),
        format!("metadata={}", metadata_root_path.display()),
        format!("rt_meta={}", built_rt.metadata_root_path.display()),
        format!("rt_obj={}", built_rt.object_path.display()),
        format!("sys_meta={}", built_sys.metadata_root_path.display()),
        format!("sys_obj={}", built_sys.object_path.display()),
        format!("rt_entry_obj={}", rt_entry_object_path.display()),
    ]);

    if !build_state::action_state_is_current(&object_path, &std_fingerprint)? {
        let driver = CompilerDriver::new(options);
        let Some(report) = driver.compile_with_report() else {
            return Err(Error::Execution(format!(
                "compile failed for standard library `{}`",
                source_path.display()
            )));
        };

        let mut inputs = report.loaded_sources;
        inputs.sort();
        inputs.dedup();
        build_state::record_action_state(
            &object_path,
            std_fingerprint,
            &inputs,
            &[object_path.clone(), metadata_root_path.clone()],
        )?;
        execution_summary.record_action(
            ActionTimingKind::Compile,
            std_compile_action_label(profile),
            report.phase_timings,
        );
    }

    built_std_packages.insert(
        profile.to_string(),
        BuiltStdPackage {
            metadata_root_path,
            link_objects: vec![
                object_path,
                built_rt.object_path.clone(),
                built_sys.object_path.clone(),
                workspace_root
                    .join(".craft")
                    .join("build")
                    .join(profile)
                    .join("obj")
                    .join("base")
                    .join("lib")
                    .join("base.o"),
                rt_entry_object_path,
            ],
            interface_aliases: {
                let mut aliases = built_sys.interface_aliases.clone();
                aliases.insert("rt".to_string(), built_rt.metadata_root_path);
                aliases.insert("sys".to_string(), built_sys.metadata_root_path);
                aliases
            },
        },
    );
    Ok(())
}

fn build_rt_package(
    workspace_root: &Path,
    profile: &str,
    execution_summary: &mut ExecutionSummary,
) -> Result<BuiltLibraryPackage> {
    let rt_root = resolve_rt_path();
    let source_path = rt_root.join("init.rn");
    if !source_path.is_file() {
        return Err(Error::Execution(format!(
            "rt library root `{}` is missing",
            source_path.display()
        )));
    }

    let object_path = workspace_root
        .join(".craft")
        .join("build")
        .join(profile)
        .join("obj")
        .join("rt")
        .join("lib")
        .join("rt.o");
    let metadata_root_path = workspace_root
        .join(".craft")
        .join("build")
        .join(profile)
        .join("meta")
        .join("rt");

    ensure_parent_dir(&object_path)?;
    ensure_parent_dir(&metadata_root_path.join(KMETA_MANIFEST_FILE))?;

    let mut options = CompileOptions {
        input_file: Some(source_path.to_string_lossy().to_string()),
        output_file: object_path.to_string_lossy().to_string(),
        metadata_output: Some(metadata_root_path.to_string_lossy().to_string()),
        metadata_package_name: Some("rt".to_string()),
        metadata_package_version: None,
        root_module_name: Some("rt".to_string()),
        driver_mode: DriverMode::CompileOnly,
        report_progress: false,
        opt_level: if profile == "release" {
            OptLevel::O3
        } else {
            OptLevel::O0
        },
        ..CompileOptions::default()
    };
    apply_host_linker_env(&mut options);
    options
        .module_aliases
        .insert("rt".to_string(), rt_root.to_string_lossy().to_string());
    inject_driver_condition_defines(&mut options);
    let toolchain_digest = build_state::current_process_digest()?;
    let rt_fingerprint = build_fingerprint(&[
        "rt_runtime_layout=v1".to_string(),
        "kind=compile-rt".to_string(),
        format!("toolchain={toolchain_digest}"),
        format!("profile={profile}"),
        format!("source={}", source_path.display()),
        format!("object={}", object_path.display()),
        format!("metadata={}", metadata_root_path.display()),
    ]);

    if !build_state::action_state_is_current(&object_path, &rt_fingerprint)? {
        let driver = CompilerDriver::new(options);
        let Some(report) = driver.compile_with_report() else {
            return Err(Error::Execution(format!(
                "compile failed for rt library `{}`",
                source_path.display()
            )));
        };

        let mut inputs = report.loaded_sources;
        inputs.sort();
        inputs.dedup();
        build_state::record_action_state(
            &object_path,
            rt_fingerprint,
            &inputs,
            &[object_path.clone(), metadata_root_path.clone()],
        )?;
        execution_summary.record_action(
            ActionTimingKind::Compile,
            rt_compile_action_label(profile),
            report.phase_timings,
        );
    }

    Ok(BuiltLibraryPackage {
        metadata_root_path,
        object_path,
        interface_aliases: BTreeMap::new(),
    })
}

fn build_base_package(
    workspace_root: &Path,
    profile: &str,
    execution_summary: &mut ExecutionSummary,
) -> Result<BuiltLibraryPackage> {
    let base_root = resolve_base_path();
    let source_path = base_root.join("init.rn");
    if !source_path.is_file() {
        return Err(Error::Execution(format!(
            "base library root `{}` is missing",
            source_path.display()
        )));
    }

    let object_path = workspace_root
        .join(".craft")
        .join("build")
        .join(profile)
        .join("obj")
        .join("base")
        .join("lib")
        .join("base.o");
    let metadata_root_path = workspace_root
        .join(".craft")
        .join("build")
        .join(profile)
        .join("meta")
        .join("base");

    ensure_parent_dir(&object_path)?;
    ensure_parent_dir(&metadata_root_path.join(KMETA_MANIFEST_FILE))?;

    let mut options = CompileOptions {
        input_file: Some(source_path.to_string_lossy().to_string()),
        output_file: object_path.to_string_lossy().to_string(),
        metadata_output: Some(metadata_root_path.to_string_lossy().to_string()),
        metadata_package_name: Some("base".to_string()),
        metadata_package_version: None,
        root_module_name: Some("base".to_string()),
        driver_mode: DriverMode::CompileOnly,
        report_progress: false,
        opt_level: if profile == "release" {
            OptLevel::O3
        } else {
            OptLevel::O0
        },
        library_bundle: LibraryBundle::Base,
        ..CompileOptions::default()
    };
    apply_host_linker_env(&mut options);
    options
        .module_aliases
        .insert("base".to_string(), base_root.to_string_lossy().to_string());
    inject_driver_condition_defines(&mut options);
    let toolchain_digest = build_state::current_process_digest()?;
    let base_fingerprint = build_fingerprint(&[
        "base_runtime_layout=v1".to_string(),
        "kind=compile-base".to_string(),
        format!("toolchain={toolchain_digest}"),
        format!("profile={profile}"),
        format!("source={}", source_path.display()),
        format!("object={}", object_path.display()),
        format!("metadata={}", metadata_root_path.display()),
    ]);

    if !build_state::action_state_is_current(&object_path, &base_fingerprint)? {
        let driver = CompilerDriver::new(options);
        let Some(report) = driver.compile_with_report() else {
            return Err(Error::Execution(format!(
                "compile failed for base library `{}`",
                source_path.display()
            )));
        };

        let mut inputs = report.loaded_sources;
        inputs.sort();
        inputs.dedup();
        build_state::record_action_state(
            &object_path,
            base_fingerprint,
            &inputs,
            &[object_path.clone(), metadata_root_path.clone()],
        )?;
        execution_summary.record_action(
            ActionTimingKind::Compile,
            base_compile_action_label(profile),
            report.phase_timings,
        );
    }

    Ok(BuiltLibraryPackage {
        metadata_root_path,
        object_path,
        interface_aliases: BTreeMap::new(),
    })
}

fn build_sys_package(
    workspace_root: &Path,
    profile: &str,
    execution_summary: &mut ExecutionSummary,
) -> Result<BuiltLibraryPackage> {
    let sys_root = resolve_sys_path();
    let source_path = sys_root.join("init.rn");
    if !source_path.is_file() {
        return Err(Error::Execution(format!(
            "sys library root `{}` is missing",
            source_path.display()
        )));
    }
    let built_base = build_base_package(workspace_root, profile, execution_summary)?;

    let object_path = workspace_root
        .join(".craft")
        .join("build")
        .join(profile)
        .join("obj")
        .join("sys")
        .join("lib")
        .join("sys.o");
    let metadata_root_path = workspace_root
        .join(".craft")
        .join("build")
        .join(profile)
        .join("meta")
        .join("sys");

    ensure_parent_dir(&object_path)?;
    ensure_parent_dir(&metadata_root_path.join(KMETA_MANIFEST_FILE))?;

    let mut options = CompileOptions {
        input_file: Some(source_path.to_string_lossy().to_string()),
        output_file: object_path.to_string_lossy().to_string(),
        metadata_output: Some(metadata_root_path.to_string_lossy().to_string()),
        metadata_package_name: Some("sys".to_string()),
        metadata_package_version: None,
        root_module_name: Some("sys".to_string()),
        driver_mode: DriverMode::CompileOnly,
        report_progress: false,
        opt_level: if profile == "release" {
            OptLevel::O3
        } else {
            OptLevel::O0
        },
        library_bundle: LibraryBundle::Base,
        ..CompileOptions::default()
    };
    apply_host_linker_env(&mut options);
    options
        .module_aliases
        .insert("sys".to_string(), sys_root.to_string_lossy().to_string());
    extend_interface_aliases(&mut options, &built_base.interface_aliases);
    options.module_interface_aliases.insert(
        "base".to_string(),
        built_base.metadata_root_path.to_string_lossy().to_string(),
    );
    inject_driver_condition_defines(&mut options);
    let toolchain_digest = build_state::current_process_digest()?;
    let sys_fingerprint = build_fingerprint(&[
        "sys_runtime_layout=v1".to_string(),
        "kind=compile-sys".to_string(),
        format!("toolchain={toolchain_digest}"),
        format!("profile={profile}"),
        format!("source={}", source_path.display()),
        format!("object={}", object_path.display()),
        format!("metadata={}", metadata_root_path.display()),
        format!("base_meta={}", built_base.metadata_root_path.display()),
        format!("base_obj={}", built_base.object_path.display()),
    ]);

    if !build_state::action_state_is_current(&object_path, &sys_fingerprint)? {
        let driver = CompilerDriver::new(options);
        let Some(report) = driver.compile_with_report() else {
            return Err(Error::Execution(format!(
                "compile failed for sys library `{}`",
                source_path.display()
            )));
        };

        let mut inputs = report.loaded_sources;
        inputs.sort();
        inputs.dedup();
        build_state::record_action_state(
            &object_path,
            sys_fingerprint,
            &inputs,
            &[object_path.clone(), metadata_root_path.clone()],
        )?;
        execution_summary.record_action(
            ActionTimingKind::Compile,
            sys_compile_action_label(profile),
            report.phase_timings,
        );
    }

    let mut interface_aliases = built_base.interface_aliases.clone();
    interface_aliases.insert("base".to_string(), built_base.metadata_root_path);
    Ok(BuiltLibraryPackage {
        metadata_root_path,
        object_path,
        interface_aliases,
    })
}

fn build_rt_entry_package(
    workspace_root: &Path,
    profile: &str,
    execution_summary: &mut ExecutionSummary,
    built_sys: &BuiltLibraryPackage,
) -> Result<PathBuf> {
    let source_path = resolve_rt_path().join("entry.rn");
    if !source_path.is_file() {
        return Err(Error::Execution(format!(
            "rt entry source `{}` is missing",
            source_path.display()
        )));
    }

    let object_path = workspace_root
        .join(".craft")
        .join("build")
        .join(profile)
        .join("obj")
        .join("rt")
        .join("entry")
        .join("rt_entry.o");

    ensure_parent_dir(&object_path)?;

    let mut options = CompileOptions {
        input_file: Some(source_path.to_string_lossy().to_string()),
        output_file: object_path.to_string_lossy().to_string(),
        root_module_name: Some("rt_entry".to_string()),
        driver_mode: DriverMode::CompileOnly,
        report_progress: false,
        opt_level: if profile == "release" {
            OptLevel::O3
        } else {
            OptLevel::O0
        },
        ..CompileOptions::default()
    };
    options.custom_defines.insert(
        "rt_role".to_string(),
        "entry".to_string(),
    );
    extend_interface_aliases(&mut options, &built_sys.interface_aliases);
    options.module_interface_aliases.insert(
        "sys".to_string(),
        built_sys.metadata_root_path.to_string_lossy().to_string(),
    );
    inject_driver_condition_defines(&mut options);
    let toolchain_digest = build_state::current_process_digest()?;
    let entry_fingerprint = build_fingerprint(&[
        "rt_runtime_layout=v1".to_string(),
        "kind=compile-rt-entry".to_string(),
        format!("toolchain={toolchain_digest}"),
        format!("profile={profile}"),
        format!("source={}", source_path.display()),
        format!("object={}", object_path.display()),
        format!("sys_meta={}", built_sys.metadata_root_path.display()),
    ]);

    if !build_state::action_state_is_current(&object_path, &entry_fingerprint)? {
        let driver = CompilerDriver::new(options);
        let Some(report) = driver.compile_with_report() else {
            return Err(Error::Execution(format!(
                "compile failed for rt hosted entry `{}`",
                source_path.display()
            )));
        };

        let mut inputs = report.loaded_sources;
        inputs.sort();
        inputs.dedup();
        build_state::record_action_state(
            &object_path,
            entry_fingerprint,
            &inputs,
            &[object_path.clone()],
        )?;
        execution_summary.record_action(
            ActionTimingKind::Compile,
            rt_entry_compile_action_label(profile),
            report.phase_timings,
        );
    }

    Ok(object_path)
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
mod tests {
    use super::{build, run, test, validate_package_metadata_root};
    use crate::build_plan;
    use crate::elaborate::{FeatureSelection, plan};
    use crate::manifest::Manifest;
    use crate::workspace;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
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
kern = "0.6.7"

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
fn main() i32 {
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
kern = "0.6.7"

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
kern = "0.6.7"

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
fn main() i32 {
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
kern = "0.6.7"

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
kern = "0.6.7"

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
fn main() i32 {
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
kern = "0.6.7"

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
kern = "0.6.7"

[test]
roots = ["tests/smoke.rn"]
"#,
        )
        .unwrap();
        fs::write(
            root.join("tests/smoke.rn"),
            r#"
fn main() i32 {
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
        let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(gitignore.contains(".craft/"));

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
kern = "0.6.7"

[lib]
root = "src/lib.rn"

[test]
roots = ["tests/smoke.rn"]
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

fn main() i32 {
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
kern = "0.6.7"

[test]
roots = ["tests/cwd.rn"]
"#,
        )
        .unwrap();
        fs::write(app_dir.join("fixtures/ok.txt"), "ok\n").unwrap();
        fs::write(
            app_dir.join("tests/cwd.rn"),
            r#"
use std.fs;
use base.mem.alloc.{Allocator, GPA};
use sys.mem.Page;

fn main() i32 {
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
kern = "0.6.7"

[test]
roots = ["tests/env.rn"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("tests/env.rn"),
            r#"
use std.env;
use base.mem.alloc.{Allocator, GPA};
use sys.mem.Page;

fn main() i32 {
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
kern = "0.6.7"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/main.rn"),
            r#"
fn main() i32 {
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
    fn incremental_build_skips_unchanged_actions() {
        let root = temp_dir("craft-exec-incremental-skip");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/main.rn"),
            r#"
fn main() i32 {
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

        let first = build(&build_plan, &action_plan).unwrap();
        assert_eq!(first.compile_actions, 1);
        assert_eq!(first.link_actions, 1);

        let second = build(&build_plan, &action_plan).unwrap();
        assert_eq!(second.compile_actions, 0);
        assert_eq!(second.link_actions, 0);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn incremental_build_rebuilds_only_changed_workspace_actions() {
        let root = temp_dir("craft-exec-incremental-workspace");
        let app_dir = root.join("app");
        let util_dir = root.join("util");
        fs::create_dir_all(app_dir.join("src")).unwrap();
        fs::create_dir_all(util_dir.join("src")).unwrap();

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
kern = "0.6.7"

[[bin]]
name = "app"
root = "src/main.rn"

[dependencies]
util = { path = "../util" }
"#,
        )
        .unwrap();
        fs::write(
            util_dir.join("Craft.toml"),
            r#"
[package]
name = "util"
version = "0.1.0"
kern = "0.6.7"

[lib]
root = "src/lib.rn"
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("src/main.rn"),
            r#"
fn main() i32 {
    return util.answer();
}
"#,
        )
        .unwrap();
        fs::write(
            util_dir.join("src/lib.rn"),
            r#"
pub fn answer() i32 {
    return 41;
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
            crate::script::ScriptCommand::Build,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());

        let first = build(&build_plan, &action_plan).unwrap();
        assert_eq!(first.compile_actions, 2);
        assert_eq!(first.link_actions, 1);

        fs::write(
            app_dir.join("src/main.rn"),
            r#"
fn main() i32 {
    return util.answer() + 1;
}
"#,
        )
        .unwrap();
        let app_changed = build(&build_plan, &action_plan).unwrap();
        assert_eq!(app_changed.compile_actions, 1);
        assert_eq!(app_changed.link_actions, 1);

        fs::write(
            util_dir.join("src/lib.rn"),
            r#"
pub fn answer() i32 {
    return 42;
}
"#,
        )
        .unwrap();
        let dep_changed = build(&build_plan, &action_plan).unwrap();
        assert_eq!(dep_changed.compile_actions, 2);
        assert_eq!(dep_changed.link_actions, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_package_with_direct_external_path_dependency() {
        let root = temp_dir("craft-exec-external-direct");
        let log_root = root.join("vendor").join("log");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(log_root.join("src")).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "app"
root = "src/main.rn"

[dependencies]
log = { path = "vendor/log", version = "1" }
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/main.rn"),
            r#"
fn main() i32 {
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
kern = "0.6.7"

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
    fn builds_package_with_direct_external_git_dependency_in_release_profile() {
        let root = temp_dir("craft-exec-external-git-release");
        let repo = root.join("log.git");
        fs::create_dir_all(root.join("src")).unwrap();
        init_git_package(
            &repo,
            r#"
[package]
name = "log"
version = "1"
kern = "0.6.7"

[lib]
root = "src/lib.rn"
"#,
            r#"
pub fn answer() i32 {
    return 42;
}
"#,
        );

        fs::write(
            root.join("Craft.toml"),
            format!(
                r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "app"
root = "src/main.rn"

[dependencies]
log = {{ git = "{}", branch = "main", version = "1" }}
"#,
                toml_string_literal(&repo)
            ),
        )
        .unwrap();
        fs::write(
            root.join("src/main.rn"),
            r#"
fn main() i32 {
    if (log.answer() == 42) {
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
            crate::script::ScriptCommand::Build,
            &FeatureSelection {
                profile: crate::script::ProfileSelection::Release,
                ..Default::default()
            },
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
    fn builds_and_runs_hosted_package_with_transitive_external_path_dependency() {
        let root = temp_dir("craft-exec-external-transitive");
        let log_root = root.join("vendor").join("log");
        let corelog_root = log_root.join("vendor").join("corelog");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(log_root.join("src")).unwrap();
        fs::create_dir_all(corelog_root.join("src")).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "app"
root = "src/main.rn"

[dependencies]
log = { path = "vendor/log", version = "1" }
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/main.rn"),
            r#"
fn main() i32 {
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
kern = "0.6.7"

[lib]
root = "src/lib.rn"

[dependencies]
corelog = { path = "vendor/corelog", version = "1" }
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
kern = "0.6.7"

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
    fn builds_and_runs_external_package_with_nested_path_dependency() {
        let root = temp_dir("craft-exec-external-package-local-source");
        let log_root = root.join("vendor").join("log");
        let corelog_root = log_root.join("vendor-nested").join("corelog");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(log_root.join("src")).unwrap();
        fs::create_dir_all(corelog_root.join("src")).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "app"
root = "src/main.rn"

[dependencies]
log = { path = "vendor/log", version = "1" }
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/main.rn"),
            r#"
fn main() i32 {
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
kern = "0.6.7"

[lib]
root = "src/lib.rn"

[dependencies]
corelog = { path = "vendor-nested/corelog", version = "1" }
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
kern = "0.6.7"

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
kern = "0.6.7"

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
        "fn main() i32 { return 0; }\n"
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
kern = "0.6.7"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
        )
        .unwrap();
        fs::write(
            root.join("templates").join("main.rn"),
            "fn main() i32 { return 0; }\n",
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
kern = "0.6.7"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
        )
        .unwrap();
        fs::write(
            root.join("src").join("main.rn"),
            "fn main() i32 { return 0; }\n",
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
kern = "0.6.7"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
        )
        .unwrap();
        fs::write(
            root.join("src").join("main.rn"),
            "fn main() i32 { return 0; }\n",
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
kern = "0.6.7"

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
kern = "0.6.7"

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

fn main() i32 {
    let mut out = io.stdout();
    let writer = *mut Writer.{ out..& };
    let _ = writer.write("fn main() i32 { return 0; }\n");
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
        assert!(generated.contains("fn main() i32"));

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
kern = "0.6.7"

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
    let helper = b.stage_generated("tmp/main.template.rn", "fn main() i32 { return 0; }\n");
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
        let tool_root = root.join("vendor").join("codegen");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(tool_root.join("src")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "app"
root = "src/placeholder.rn"

[build-dependencies]
codegen = { path = "vendor/codegen", version = "1" }
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
kern = "0.6.7"

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

fn main() i32 {
    let mut out = io.stdout();
    let writer = *mut Writer.{ out..& };
    let _ = writer.write("fn main() i32 { return 0; }\n");
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
        assert!(generated.contains("fn main() i32"));

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

    fn init_git_package(repo: &Path, manifest: &str, lib_source: &str) {
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("Craft.toml"), manifest).unwrap();
        fs::write(repo.join("src/lib.rn"), lib_source).unwrap();
        run_git(repo, ["init", "--initial-branch=main"]);
        run_git(repo, ["config", "user.name", "Craft Tests"]);
        run_git(
            repo,
            ["config", "user.email", "craft-tests@example.invalid"],
        );
        run_git(repo, ["add", "."]);
        run_git(repo, ["commit", "-m", "initial"]);
    }

    fn toml_string_literal(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "\\\\")
    }

    fn run_git<const N: usize>(cwd: &Path, args: [&str; N]) {
        let output = Command::new("git")
            .args(["-c", "commit.gpgsign=false"])
            .args(["-c", "tag.gpgSign=false"])
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}


