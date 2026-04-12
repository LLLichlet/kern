use crate::build_plan::{
    ActionPlan, BuildPlan, BuildUnit, CompileAction, LinkAction, StagedAction, StagedActionKind,
};
use crate::build_state;
use crate::error::{Error, Result};
use crate::graph::{BuildDomain, PackageId};
use crate::operation_lock::OutputOperationLock;
use crate::resolver::ExternalPackageId;
use kernc_driver::{
    CodegenPlanReport, CompileCacheStats, CompileReport, CompilerDriver, IncrementalDriverKey,
    KMETA_MANIFEST_FILE, PhaseTiming, load_kmeta_manifest,
};
use kernc_utils::config::{CompileOptions, LibraryBundle, RuntimeEntry};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

mod external;
mod fingerprint;
mod options;
mod parallel;
mod runtime_packages;

use self::external::{
    build_external_package, compile_actions_index, ensure_external_tool_built,
    link_actions_by_artifact_path, local_library_actions, requested_external_dependencies,
};
use self::fingerprint::{
    base_compile_action_label, build_fingerprint, compile_action_fingerprint, compile_action_label,
    link_action_fingerprint, link_action_label, rt_compile_action_label,
    rt_entry_compile_action_label, std_compile_action_label, sys_compile_action_label,
    write_compile_action_state,
};
use self::options::{compile_action_options, link_action_options};
use self::parallel::{
    build_parallel_target_compile_jobs, build_parallel_target_link_jobs,
    compile_action_for_link_action, parallel_target_compile_jobs, parallel_target_link_jobs,
};
use self::runtime_packages::ensure_std_packages_for_actions;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExecutionSummary {
    pub compile_actions: usize,
    pub link_actions: usize,
    pub phase_timings: Vec<PhaseTiming>,
    pub cache_stats: CompileCacheStats,
    pub action_cache_stats: ActionCacheStats,
    pub action_timings: Vec<ActionTiming>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ActionCacheStats {
    pub compile_hits: usize,
    pub compile_misses: usize,
    pub link_hits: usize,
    pub link_misses: usize,
    pub staged_hits: usize,
    pub staged_misses: usize,
}

impl ActionCacheStats {
    pub fn is_empty(self) -> bool {
        self == Self::default()
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
    pub cache_stats: CompileCacheStats,
    pub codegen_plan: Option<CodegenPlanReport>,
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
        self.cache_stats.absorb(other.cache_stats);
        self.action_cache_stats.compile_hits += other.action_cache_stats.compile_hits;
        self.action_cache_stats.compile_misses += other.action_cache_stats.compile_misses;
        self.action_cache_stats.link_hits += other.action_cache_stats.link_hits;
        self.action_cache_stats.link_misses += other.action_cache_stats.link_misses;
        self.action_cache_stats.staged_hits += other.action_cache_stats.staged_hits;
        self.action_cache_stats.staged_misses += other.action_cache_stats.staged_misses;
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
        cache_stats: CompileCacheStats,
        codegen_plan: Option<CodegenPlanReport>,
    ) {
        if phase_timings.is_empty() && cache_stats.is_empty() && codegen_plan.is_none() {
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
            cache_stats,
            codegen_plan,
        });
    }

    fn record_compile_cache_hit(&mut self) {
        self.action_cache_stats.compile_hits += 1;
    }

    fn record_compile_cache_miss(&mut self) {
        self.action_cache_stats.compile_misses += 1;
    }

    fn record_link_cache_hit(&mut self) {
        self.action_cache_stats.link_hits += 1;
    }

    fn record_link_cache_miss(&mut self) {
        self.action_cache_stats.link_misses += 1;
    }

    fn record_staged_cache_hit(&mut self) {
        self.action_cache_stats.staged_hits += 1;
    }

    fn record_staged_cache_miss(&mut self) {
        self.action_cache_stats.staged_misses += 1;
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
    let mut manifest_runtime_options = BTreeMap::new();
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
            manifest_runtime_options: &mut manifest_runtime_options,
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
    common_link_objects: Vec<PathBuf>,
    hosted_entry_object_path: PathBuf,
    freestanding_entry_object_path: PathBuf,
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ManifestRuntimeOptions {
    entry: Option<RuntimeEntry>,
    libc: Option<bool>,
    bundle: Option<LibraryBundle>,
}

impl ManifestRuntimeOptions {
    fn apply_for_target(&self, target_kind: crate::plan::TargetKind, options: &mut CompileOptions) {
        if target_kind != crate::plan::TargetKind::Lib {
            if let Some(entry) = self.entry {
                options.runtime_entry = entry;
            }
            if let Some(libc) = self.libc {
                options.runtime_libc = libc;
            }
        }
        if let Some(bundle) = self.bundle {
            options.library_bundle = bundle;
        }
    }
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
    manifest_runtime_options: &'a mut BTreeMap<PathBuf, ManifestRuntimeOptions>,
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

pub(super) fn runtime_profile_key(profile: &crate::script::ScriptProfile) -> String {
    format!(
        "{}-opt{}-debug{}-cgu{}",
        profile.name, profile.opt, profile.debug, profile.codegen_units
    )
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
    let mut manifest_runtime_options = BTreeMap::new();
    let config = ExecutionConfig {
        source_config: &source_config,
        dependency_workspace_root: &build_plan.workspace_root,
        command,
        profile_selection,
        std_workspace_root: &build_plan.workspace_root,
    };
    {
        let mut external = ExternalArtifacts {
            built_std_packages: &mut built_std_packages,
            built_external_packages: &mut built_external_packages,
            built_external_tools: &mut built_external_tools,
            external_build_stack: &mut external_build_stack,
            manifest_runtime_options: &mut manifest_runtime_options,
            driver_families: &mut driver_families,
        };
        for dep in requested_external_dependencies(action_plan) {
            build_external_package(&dep, config, &mut external, &mut external_summary)?;
        }
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
    {
        let mut session = ExecutionSession {
            indexes,
            config,
            external: ExternalArtifacts {
                built_std_packages: &mut built_std_packages,
                built_external_packages: &mut built_external_packages,
                built_external_tools: &mut built_external_tools,
                external_build_stack: &mut external_build_stack,
                manifest_runtime_options: &mut manifest_runtime_options,
                driver_families: &mut driver_families,
            },
            state: ExecutionState {
                compiled: &mut compiled,
                linked: &mut linked,
                staged_outputs: &mut staged_outputs,
                execution_summary: &mut local_summary,
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
    }

    loop {
        let jobs = parallel_target_compile_jobs(action_plan, &local_library_actions, &compiled);
        if jobs.len() < 2 {
            break;
        }
        for result in build_parallel_target_compile_jobs(
            &jobs,
            &local_library_actions,
            &built_std_packages,
            &built_external_packages,
        )? {
            compiled.insert(result.compile_object_path);
            local_summary.absorb(result.summary);
        }
    }

    {
        let mut session = ExecutionSession {
            indexes,
            config,
            external: ExternalArtifacts {
                built_std_packages: &mut built_std_packages,
                built_external_packages: &mut built_external_packages,
                built_external_tools: &mut built_external_tools,
                external_build_stack: &mut external_build_stack,
                manifest_runtime_options: &mut manifest_runtime_options,
                driver_families: &mut driver_families,
            },
            state: ExecutionState {
                compiled: &mut compiled,
                linked: &mut linked,
                staged_outputs: &mut staged_outputs,
                execution_summary: &mut local_summary,
            },
        };

        for action in &action_plan.compile_actions {
            if action.domain != BuildDomain::Target
                || action.target_kind != crate::plan::TargetKind::Lib
            {
                continue;
            }
            ensure_compile_action_built(action, &mut session)?;
        }
    }

    let parallel_jobs = parallel_target_link_jobs(action_plan, &compile_action_index, &linked)?;
    for result in build_parallel_target_link_jobs(
        &parallel_jobs,
        &local_library_actions,
        &built_std_packages,
        &built_external_packages,
    )? {
        compiled.insert(result.compile_object_path);
        linked.insert(result.artifact_path);
        local_summary.absorb(result.summary);
    }

    {
        let mut session = ExecutionSession {
            indexes,
            config,
            external: ExternalArtifacts {
                built_std_packages: &mut built_std_packages,
                built_external_packages: &mut built_external_packages,
                built_external_tools: &mut built_external_tools,
                external_build_stack: &mut external_build_stack,
                manifest_runtime_options: &mut manifest_runtime_options,
                driver_families: &mut driver_families,
            },
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

fn resolve_invocation_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    Ok(std::env::current_dir()
        .map_err(Error::from_io_plain)?
        .join(path))
}

fn load_source_config(build_plan: &BuildPlan) -> Result<SourceConfigContext> {
    let _ = build_plan;
    Ok(SourceConfigContext { _private: () })
}

impl SourceConfigContext {
    fn with_child(&self) -> Self {
        Self { _private: () }
    }
}

fn build_compile_action_if_needed(
    action: &CompileAction,
    options: CompileOptions,
    driver_families: &mut BTreeMap<IncrementalDriverKey, CompilerDriver>,
    execution_summary: &mut ExecutionSummary,
) -> Result<bool> {
    let compile_lock_target = action.metadata_path.as_ref().unwrap_or(&action.object_path);
    let _compile_lock = OutputOperationLock::acquire(compile_lock_target, "compile-action")?;

    let toolchain_digest = build_state::current_process_digest()?;
    let fingerprint = compile_action_fingerprint(action, &options, &toolchain_digest);

    if build_state::action_state_is_current(&action.object_path, &fingerprint)? {
        execution_summary.record_compile_cache_hit();
        return Ok(false);
    }

    ensure_parent_dir(&action.object_path)?;
    prepare_output_path(&multi_object_output_dir(&action.object_path), true)?;
    if let Some(metadata_path) = &action.metadata_path {
        ensure_parent_dir(&metadata_path.join(KMETA_MANIFEST_FILE))?;
    }

    let emit_multi_object_dir = options.emit_multi_object_dir;
    let Some(report) = compile_with_shared_driver(driver_families, options) else {
        return Err(Error::Execution(format!(
            "compile failed for `{}`",
            action.source_path().display()
        )));
    };

    write_compile_action_state(action, emit_multi_object_dir, &report, fingerprint)?;

    execution_summary.record_compile_cache_miss();
    execution_summary.compile_actions += 1;
    execution_summary.record_action(
        ActionTimingKind::Compile,
        compile_action_label(action),
        report.phase_timings,
        report.cache_stats,
        report.codegen_plan,
    );
    Ok(true)
}

fn build_link_action_if_needed(
    action: &LinkAction,
    options: CompileOptions,
    linker_inputs: &[PathBuf],
    execution_summary: &mut ExecutionSummary,
) -> Result<bool> {
    let _link_lock = OutputOperationLock::acquire(&action.artifact_path, "link-action")?;
    let toolchain_digest = build_state::current_process_digest()?;
    let fingerprint = link_action_fingerprint(action, &options, linker_inputs, &toolchain_digest);
    if build_state::action_state_is_current(&action.artifact_path, &fingerprint)? {
        execution_summary.record_link_cache_hit();
        return Ok(false);
    }

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
        linker_inputs,
        std::slice::from_ref(&action.artifact_path),
    )?;
    execution_summary.record_link_cache_miss();
    execution_summary.link_actions += 1;
    execution_summary.record_action(
        ActionTimingKind::Link,
        link_action_label(action),
        report.phase_timings,
        report.cache_stats,
        report.codegen_plan,
    );
    Ok(true)
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

    let options = compile_action_options(
        action,
        session.indexes.local_library_actions,
        session.external.built_std_packages,
        session.external.built_external_packages,
        session.external.manifest_runtime_options,
    )?;
    let built = build_compile_action_if_needed(
        action,
        options,
        session.external.driver_families,
        session.state.execution_summary,
    )?;
    session.state.compiled.insert(action.object_path.clone());
    Ok(built)
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
    let _staged_lock = OutputOperationLock::acquire(&output_path, "staged-action")?;
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
        session.state.execution_summary.record_staged_cache_hit();
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
    session.state.execution_summary.record_staged_cache_miss();
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
    let compile_action =
        compile_action_for_link_action(action, session.indexes.compile_action_index)?;
    ensure_compile_action_built(compile_action, session)?;

    ensure_parent_dir(&action.artifact_path)?;
    let (options, linker_inputs) = link_action_options(
        action,
        compile_action,
        session.indexes.local_library_actions,
        session.external.built_std_packages,
        session.external.built_external_packages,
        session.external.manifest_runtime_options,
    )?;
    let linked_now = build_link_action_if_needed(
        action,
        options,
        &linker_inputs,
        session.state.execution_summary,
    )?;

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

pub(super) fn multi_object_output_dir(primary_output: &Path) -> PathBuf {
    let mut path = primary_output.as_os_str().to_os_string();
    path.push(".d");
    PathBuf::from(path)
}

pub(super) fn linker_input_paths_for_primary_output(primary_output: &Path) -> Result<Vec<PathBuf>> {
    let multi_object_dir = multi_object_output_dir(primary_output);
    if !multi_object_dir.is_dir() {
        return Ok(vec![primary_output.to_path_buf()]);
    }

    let mut paths = fs::read_dir(&multi_object_dir)
        .map_err(|err| Error::from_io(&multi_object_dir, err))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("o"))
        .collect::<Vec<_>>();
    paths.sort();

    if paths.is_empty() {
        return Err(Error::Execution(format!(
            "multi-object directory `{}` is empty",
            multi_object_dir.display()
        )));
    }

    Ok(paths)
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

#[cfg(test)]
mod tests;
