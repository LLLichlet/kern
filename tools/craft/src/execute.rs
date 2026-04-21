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
use std::process::{Command, ExitStatus};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

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
    base_compile_action_label, build_fingerprint, compile_action_detail_tags,
    compile_action_fingerprint, compile_action_label, link_action_detail_tags,
    link_action_fingerprint, link_action_label, rt_compile_action_label,
    rt_entry_compile_action_label, runtime_compile_detail_tags, std_compile_action_label,
    sys_compile_action_label, write_compile_action_state,
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
    pub detail_tags: Vec<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExecutionProgressPlan {
    pub staged_actions: usize,
    pub compile_actions: usize,
    pub link_actions: usize,
}

impl ExecutionProgressPlan {
    pub fn total_steps(self) -> usize {
        self.staged_actions + self.compile_actions + self.link_actions
    }

    pub fn is_empty(self) -> bool {
        self.total_steps() == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExecutionPhase {
    #[default]
    Bootstrap,
    Stage,
    Compile,
    Link,
}

impl ExecutionPhase {
    fn encode(self) -> u8 {
        match self {
            Self::Bootstrap => 0,
            Self::Stage => 1,
            Self::Compile => 2,
            Self::Link => 3,
        }
    }

    fn decode(value: u8) -> Self {
        match value {
            1 => Self::Stage,
            2 => Self::Compile,
            3 => Self::Link,
            _ => Self::Bootstrap,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionProgressSnapshot {
    pub phase: ExecutionPhase,
    pub plan: ExecutionProgressPlan,
    pub staged_done: usize,
    pub compile_done: usize,
    pub link_done: usize,
    pub elapsed: Duration,
    pub detail: String,
}

impl ExecutionProgressSnapshot {
    pub fn completed_steps(&self) -> usize {
        self.staged_done.min(self.plan.staged_actions)
            + self.compile_done.min(self.plan.compile_actions)
            + self.link_done.min(self.plan.link_actions)
    }

    pub fn total_steps(&self) -> usize {
        self.plan.total_steps()
    }
}

#[derive(Debug, Clone)]
pub struct ProgressReporter {
    state: Arc<ProgressState>,
}

#[derive(Debug)]
struct ProgressState {
    plan: ExecutionProgressPlan,
    phase: AtomicU8,
    staged_done: AtomicUsize,
    compile_done: AtomicUsize,
    link_done: AtomicUsize,
    started_at: Instant,
    detail: Mutex<String>,
}

impl ProgressReporter {
    pub fn new(plan: ExecutionProgressPlan) -> Self {
        Self {
            state: Arc::new(ProgressState {
                plan,
                phase: AtomicU8::new(ExecutionPhase::Bootstrap.encode()),
                staged_done: AtomicUsize::new(0),
                compile_done: AtomicUsize::new(0),
                link_done: AtomicUsize::new(0),
                started_at: Instant::now(),
                detail: Mutex::new(String::new()),
            }),
        }
    }

    pub fn snapshot(&self) -> ExecutionProgressSnapshot {
        ExecutionProgressSnapshot {
            phase: ExecutionPhase::decode(self.state.phase.load(Ordering::Relaxed)),
            plan: self.state.plan,
            staged_done: self.state.staged_done.load(Ordering::Relaxed),
            compile_done: self.state.compile_done.load(Ordering::Relaxed),
            link_done: self.state.link_done.load(Ordering::Relaxed),
            elapsed: self.state.started_at.elapsed(),
            detail: self.state.detail.lock().unwrap().clone(),
        }
    }

    pub(crate) fn set_phase(&self, phase: ExecutionPhase) {
        self.state.phase.store(phase.encode(), Ordering::Relaxed);
    }

    pub(crate) fn record_staged_action(&self) {
        self.state.staged_done.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_compile_action(&self) {
        self.state.compile_done.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_link_action(&self) {
        self.state.link_done.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn set_detail(&self, detail: impl Into<String>) {
        *self.state.detail.lock().unwrap() = detail.into();
    }
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
        detail_tags: Vec<String>,
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
            detail_tags,
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

#[cfg_attr(not(test), allow(dead_code))]
pub fn build(build_plan: &BuildPlan, action_plan: &ActionPlan) -> Result<ExecutionSummary> {
    build_with_command(
        build_plan,
        action_plan,
        crate::script::ScriptCommand::Build,
        None,
    )
}

pub fn build_with_progress(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    progress: Option<ProgressReporter>,
) -> Result<ExecutionSummary> {
    build_with_command(
        build_plan,
        action_plan,
        crate::script::ScriptCommand::Build,
        progress,
    )
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn check(build_plan: &BuildPlan, action_plan: &ActionPlan) -> Result<ExecutionSummary> {
    build_with_command(
        build_plan,
        action_plan,
        crate::script::ScriptCommand::Check,
        None,
    )
}

pub fn check_with_progress(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    progress: Option<ProgressReporter>,
) -> Result<ExecutionSummary> {
    build_with_command(
        build_plan,
        action_plan,
        crate::script::ScriptCommand::Check,
        progress,
    )
}

pub(crate) fn materialize_analysis_inputs(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
) -> Result<()> {
    materialize_analysis_inputs_with_progress(build_plan, action_plan, None)
}

pub(crate) fn materialize_analysis_inputs_with_progress(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    progress: Option<ProgressReporter>,
) -> Result<()> {
    let source_config = load_source_config(build_plan)?;
    let profile_selection = profile_selection_for_action_plan(action_plan);
    let mut built_std_packages = BTreeMap::new();
    let mut driver_families = BTreeMap::new();
    let mut summary = ExecutionSummary::default();
    if let Some(progress) = &progress {
        progress.set_phase(ExecutionPhase::Bootstrap);
        progress.set_detail("prepare semantic inputs");
    }
    ensure_std_packages_for_actions(
        &build_plan.workspace_root,
        &action_plan.compile_actions,
        crate::script::ScriptCommand::Build,
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
            progress: progress.clone(),
        },
    };

    if let Some(progress) = &progress {
        progress.set_phase(ExecutionPhase::Stage);
        progress.set_detail("materialize generated inputs");
    }
    for action in &action_plan.compile_actions {
        if action.domain != BuildDomain::Target {
            continue;
        }
        cleanup_stale_compile_inputs(action, action_plan.build_nodes.as_slice())?;
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
    progress: Option<ProgressReporter>,
}

struct ExecutionSession<'a> {
    indexes: ActionIndexes<'a>,
    config: ExecutionConfig<'a>,
    external: ExternalArtifacts<'a>,
    state: ExecutionState<'a>,
}

pub(super) fn runtime_profile_key(profile: &crate::script::ScriptProfile) -> String {
    format!(
        "{}-opt{}-debug{}-cgu{}-lto{}",
        profile.name,
        profile.opt,
        profile.debug,
        profile.codegen_units,
        profile.lto_mode.as_str()
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
    progress: Option<ProgressReporter>,
) -> Result<ExecutionSummary> {
    let source_config = load_source_config(build_plan)?;
    let profile_selection = profile_selection_for_action_plan(action_plan);
    let mut built_std_packages = BTreeMap::new();
    let mut driver_families = BTreeMap::new();
    let mut external_summary = ExecutionSummary::default();
    if let Some(progress) = &progress {
        progress.set_phase(ExecutionPhase::Bootstrap);
        progress.set_detail("prepare workspace and runtime packages");
    }
    ensure_std_packages_for_actions(
        &build_plan.workspace_root,
        &action_plan.compile_actions,
        command,
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
                progress: progress.clone(),
            },
        };

        if let Some(progress) = &progress {
            progress.set_phase(ExecutionPhase::Stage);
            progress.set_detail("materialize generated inputs");
        }
        for action in &action_plan.compile_actions {
            if action.domain != BuildDomain::Target {
                continue;
            }
            cleanup_stale_compile_inputs(action, action_plan.build_nodes.as_slice())?;
            execute_staged_actions(
                action.compile_inputs.as_slice(),
                action_plan.build_nodes.as_slice(),
                action.required_source_path(),
                &mut session,
            )?;
        }
    }

    if let Some(progress) = &progress {
        progress.set_phase(ExecutionPhase::Compile);
        progress.set_detail("compile target units");
    }
    loop {
        let jobs = parallel_target_compile_jobs(action_plan, &local_library_actions, &compiled);
        if jobs.len() < 2 {
            break;
        }
        if let Some(progress) = &progress {
            progress.set_detail(format!("compile parallel batch ({} jobs)", jobs.len()));
        }
        for result in build_parallel_target_compile_jobs(
            command,
            &jobs,
            &local_library_actions,
            &built_std_packages,
            &built_external_packages,
        )? {
            compiled.insert(result.compile_object_path);
            local_summary.absorb(result.summary);
            if let Some(progress) = &progress {
                progress.record_compile_action();
            }
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
                progress: progress.clone(),
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

        if command == crate::script::ScriptCommand::Check {
            for action in &action_plan.compile_actions {
                if action.domain != BuildDomain::Target {
                    continue;
                }
                ensure_compile_action_built(action, &mut session)?;
            }
        }
    }

    if command == crate::script::ScriptCommand::Check {
        external_summary.absorb(local_summary);
        return Ok(external_summary);
    }

    if let Some(progress) = &progress {
        progress.set_phase(ExecutionPhase::Link);
        progress.set_detail("link target artifacts");
    }
    let parallel_jobs = parallel_target_link_jobs(action_plan, &compile_action_index, &linked)?;
    if let Some(progress) = &progress
        && !parallel_jobs.is_empty()
    {
        progress.set_detail(format!(
            "link parallel batch ({} jobs)",
            parallel_jobs.len()
        ));
    }
    for result in build_parallel_target_link_jobs(
        command,
        &parallel_jobs,
        &local_library_actions,
        &built_std_packages,
        &built_external_packages,
    )? {
        compiled.insert(result.compile_object_path);
        linked.insert(result.artifact_path);
        local_summary.absorb(result.summary);
        if let Some(progress) = &progress {
            progress.record_compile_action();
            progress.record_link_action();
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
                progress: progress.clone(),
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

#[cfg_attr(not(test), allow(dead_code))]
pub fn run(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    unit: &BuildUnit,
) -> Result<RunSummary> {
    let build = build_with_command(
        build_plan,
        action_plan,
        crate::script::ScriptCommand::Run,
        None,
    )?;
    run_built(build_plan, action_plan, unit, build)
}

pub fn run_built(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    unit: &BuildUnit,
    build: ExecutionSummary,
) -> Result<RunSummary> {
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

#[cfg_attr(not(test), allow(dead_code))]
pub fn test(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    units: &[&BuildUnit],
) -> Result<TestSummary> {
    let build = build_with_command(
        build_plan,
        action_plan,
        crate::script::ScriptCommand::Test,
        None,
    )?;
    test_built(build_plan, action_plan, units, build)
}

pub fn test_built(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    units: &[&BuildUnit],
    build: ExecutionSummary,
) -> Result<TestSummary> {
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
    prepare_output_path(&multi_linker_input_dir(&action.object_path), true)?;
    if let Some(metadata_path) = &action.metadata_path {
        ensure_parent_dir(&metadata_path.join(KMETA_MANIFEST_FILE))?;
    }

    let emit_multi_linker_input_dir = options.emit_multi_linker_input_dir;
    let emits_linker_input = options.driver_mode.emits_linker_input();
    let compile_label = compile_action_label(action, &options);
    let compile_tags = compile_action_detail_tags(&options);
    let Some(report) = compile_with_shared_driver(driver_families, options) else {
        return Err(Error::Execution(format!(
            "compile failed for `{}`",
            action.source_path().display()
        )));
    };

    write_compile_action_state(
        action,
        emits_linker_input,
        emit_multi_linker_input_dir,
        &report,
        fingerprint,
    )?;

    execution_summary.record_compile_cache_miss();
    execution_summary.compile_actions += 1;
    execution_summary.record_action(
        ActionTimingKind::Compile,
        compile_label,
        compile_tags,
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
    let mut link_input_paths = action
        .link
        .input_paths
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    for path in local_link_search_input_paths(action, &options) {
        if !link_input_paths.contains(&path) {
            link_input_paths.push(path);
        }
    }
    let fingerprint = link_action_fingerprint(
        action,
        &options,
        linker_inputs,
        &link_input_paths,
        &toolchain_digest,
    );
    if build_state::action_state_is_current(&action.artifact_path, &fingerprint)? {
        execution_summary.record_link_cache_hit();
        return Ok(false);
    }

    let link_label = link_action_label(action, &options);
    let link_tags = link_action_detail_tags(action, &options, linker_inputs);
    let driver = CompilerDriver::new(options);
    let Some(report) = driver.compile_with_report() else {
        return Err(Error::Execution(format!(
            "link failed for `{}`",
            action.artifact_path.display()
        )));
    };
    let mut state_inputs = linker_inputs.to_vec();
    state_inputs.extend(link_input_paths);
    build_state::record_action_state(
        &action.artifact_path,
        fingerprint,
        &state_inputs,
        std::slice::from_ref(&action.artifact_path),
    )?;
    execution_summary.record_link_cache_miss();
    execution_summary.link_actions += 1;
    execution_summary.record_action(
        ActionTimingKind::Link,
        link_label,
        link_tags,
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

    cleanup_stale_compile_inputs(action, session.indexes.action_plan.build_nodes.as_slice())?;
    execute_staged_actions(
        action.compile_inputs.as_slice(),
        session.indexes.action_plan.build_nodes.as_slice(),
        action.required_source_path(),
        session,
    )?;
    ensure_parent_dir(&action.object_path)?;
    ensure_parent_dir(&action.artifact_path)?;

    let options = compile_action_options(
        session.config.command,
        action,
        session.indexes.local_library_actions,
        session.external.built_std_packages,
        session.external.built_external_packages,
        session.external.manifest_runtime_options,
    )?;
    if let Some(progress) = &session.state.progress {
        progress.set_phase(ExecutionPhase::Compile);
        progress.set_detail(compile_action_label(action, &options));
    }
    let built = build_compile_action_if_needed(
        action,
        options,
        session.external.driver_families,
        session.state.execution_summary,
    )?;
    if let Some(progress) = &session.state.progress {
        progress.record_compile_action();
    }
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

fn format_captured_child_stream(label: &str, bytes: &[u8]) -> Option<String> {
    const MAX_LEN: usize = 8192;

    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let rendered = if trimmed.len() > MAX_LEN {
        format!("{}\n...<truncated>...", &trimmed[..MAX_LEN])
    } else {
        trimmed.to_string()
    };

    Some(format!("{label}:\n{rendered}"))
}

fn format_run_tool_failure(
    tool_path: &Path,
    status: ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
) -> String {
    let mut message = format!(
        "tool `{}` exited with status {}",
        tool_path.display(),
        status
    );
    if let Some(stderr_text) = format_captured_child_stream("stderr", stderr) {
        message.push('\n');
        message.push_str(&stderr_text);
    }
    if let Some(stdout_text) = format_captured_child_stream("stdout", stdout) {
        message.push('\n');
        message.push_str(&stdout_text);
    }
    message
}

fn stage_action_label(action: &StagedAction, output_path: &Path) -> String {
    let output = output_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&action.output);
    match &action.kind {
        StagedActionKind::WriteFile { .. } => format!("write {output}"),
        StagedActionKind::RunTool { tool, .. } => {
            let tool_name = Path::new(&tool.executable_path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&tool.executable_path);
            format!("run-tool {tool_name} -> {output}")
        }
        StagedActionKind::CopyFile { source } => {
            let input = Path::new(source)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(source);
            format!("copy {input} -> {output}")
        }
        StagedActionKind::CopyDirectory { source } => {
            let input = Path::new(source)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(source);
            format!("copy-dir {input} -> {output}")
        }
    }
}

fn execute_staged_action(
    action: &StagedAction,
    action_index: &BTreeMap<usize, &StagedAction>,
    active: &mut BTreeSet<usize>,
    session: &mut ExecutionSession<'_>,
) -> Result<bool> {
    let output_path = PathBuf::from(&action.output);
    if let Some(progress) = &session.state.progress {
        progress.set_phase(ExecutionPhase::Stage);
        progress.set_detail(stage_action_label(action, &output_path));
    }
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
    let mut input_paths = action
        .depends_on
        .iter()
        .map(|dependency_id| {
            action_index
                .get(dependency_id)
                .map(|dependency| PathBuf::from(&dependency.output))
                .ok_or_else(|| {
                    Error::Execution(format!(
                        "missing build node `{dependency_id}` while hashing `{}`",
                        action.output
                    ))
                })
        })
        .collect::<Result<Vec<_>>>()?;
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
            lines.extend(
                input_paths
                    .iter()
                    .map(|path| format!("dep={}", path.display())),
            );
            lines.extend(args.iter().map(|arg| format!("arg={arg}")));
            build_fingerprint(&lines)
        }
        StagedActionKind::CopyFile { source } => {
            let input_path = PathBuf::from(source);
            input_paths.push(input_path.clone());
            let mut lines = vec![
                "kind=copy-file".to_string(),
                format!("input={}", input_path.display()),
                format!("output={}", output_path.display()),
            ];
            lines.extend(
                action
                    .depends_on
                    .iter()
                    .map(|dependency_id| format!("dep={dependency_id}")),
            );
            build_fingerprint(&lines)
        }
        StagedActionKind::CopyDirectory { source } => {
            let input_path = PathBuf::from(source);
            input_paths.push(input_path.clone());
            let mut lines = vec![
                "kind=copy-dir".to_string(),
                format!("input={}", input_path.display()),
                format!("output={}", output_path.display()),
            ];
            lines.extend(
                action
                    .depends_on
                    .iter()
                    .map(|dependency_id| format!("dep={dependency_id}")),
            );
            build_fingerprint(&lines)
        }
    };

    if build_state::action_state_is_current(&output_path, &fingerprint)? {
        session.state.execution_summary.record_staged_cache_hit();
        session.state.staged_outputs.insert(output_path);
        if let Some(progress) = &session.state.progress {
            progress.record_staged_action();
        }
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
                return Err(Error::Execution(format_run_tool_failure(
                    &tool_path,
                    output.status,
                    &output.stdout,
                    &output.stderr,
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
    if let Some(progress) = &session.state.progress {
        progress.record_staged_action();
    }
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
    if let Some(progress) = &session.state.progress {
        progress.set_phase(ExecutionPhase::Link);
        progress.set_detail(link_action_label(action, &options));
    }
    let linked_now = build_link_action_if_needed(
        action,
        options,
        &linker_inputs,
        session.state.execution_summary,
    )?;
    if let Some(progress) = &session.state.progress {
        progress.record_link_action();
    }

    session.state.linked.insert(action.artifact_path.clone());

    cleanup_stale_artifact_outputs(action, session.indexes.action_plan.build_nodes.as_slice())?;

    execute_staged_actions(
        action.artifact_outputs.as_slice(),
        session.indexes.action_plan.build_nodes.as_slice(),
        None,
        session,
    )?;
    Ok(linked_now)
}

fn local_link_search_input_paths(action: &LinkAction, options: &CompileOptions) -> Vec<PathBuf> {
    options
        .linker_search_paths
        .iter()
        .map(PathBuf::from)
        .filter(|path| path.is_dir() && path.starts_with(&action.package_root_path))
        .collect()
}

fn cleanup_stale_artifact_outputs(action: &LinkAction, build_nodes: &[StagedAction]) -> Result<()> {
    cleanup_stale_staged_root(
        &action.artifact_root_path,
        action.artifact_outputs.as_slice(),
        build_nodes,
        "artifact",
    )
}

fn cleanup_stale_compile_inputs(
    action: &CompileAction,
    build_nodes: &[StagedAction],
) -> Result<()> {
    cleanup_stale_staged_root(
        &action.generated_root_path,
        action.compile_inputs.as_slice(),
        build_nodes,
        "generated",
    )
}

fn cleanup_stale_staged_root(
    root: &Path,
    root_ids: &[usize],
    build_nodes: &[StagedAction],
    label: &str,
) -> Result<()> {
    if !root.is_dir() {
        return Ok(());
    }

    let action_index = build_nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<BTreeMap<_, _>>();
    let mut keep_files = BTreeSet::new();
    let mut keep_dirs = BTreeSet::new();
    let mut keep_subtrees = BTreeSet::new();
    keep_dirs.insert(root.to_path_buf());

    for root_id in root_ids {
        let node = action_index
            .get(root_id)
            .ok_or_else(|| Error::Execution(format!("missing build node `{root_id}`")))?;
        let output_path = PathBuf::from(&node.output);
        if !output_path.starts_with(root) {
            return Err(Error::Execution(format!(
                "{label} output `{}` escapes owned root `{}`",
                output_path.display(),
                root.display()
            )));
        }
        keep_files.insert(output_path.clone());
        keep_files.insert(build_state::action_state_path(&output_path));
        if matches!(node.kind, StagedActionKind::CopyDirectory { .. }) {
            keep_subtrees.insert(output_path.clone());
        }
        let mut current = output_path.parent();
        while let Some(path) = current {
            if !path.starts_with(root) {
                break;
            }
            keep_dirs.insert(path.to_path_buf());
            if path == root {
                break;
            }
            current = path.parent();
        }
    }

    cleanup_stale_artifact_tree(root, root, &keep_files, &keep_dirs, &keep_subtrees)
}

fn cleanup_stale_artifact_tree(
    root: &Path,
    dir: &Path,
    keep_files: &BTreeSet<PathBuf>,
    keep_dirs: &BTreeSet<PathBuf>,
    keep_subtrees: &BTreeSet<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(dir).map_err(|err| Error::from_io(dir, err))? {
        let entry = entry.map_err(|err| Error::from_io(dir, err))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|err| Error::from_io(&path, err))?;
        if file_type.is_dir() {
            if keep_subtrees.contains(&path) {
                continue;
            }
            cleanup_stale_artifact_tree(root, &path, keep_files, keep_dirs, keep_subtrees)?;
            if path != root && !keep_dirs.contains(&path) && path.exists() {
                fs::remove_dir_all(&path).map_err(|err| Error::from_io(&path, err))?;
            }
            continue;
        }

        if !keep_files.contains(&path) {
            fs::remove_file(&path).map_err(|err| Error::from_io(&path, err))?;
        }
    }

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
        } else {
            return Err(format!(
                "unsupported filesystem entry `{}` while copying directory tree",
                source_path.display()
            ));
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

pub(super) fn multi_linker_input_dir(primary_output: &Path) -> PathBuf {
    let mut path = primary_output.as_os_str().to_os_string();
    path.push(".d");
    PathBuf::from(path)
}

pub(super) fn linker_input_paths_for_primary_output(primary_output: &Path) -> Result<Vec<PathBuf>> {
    let multi_linker_input_dir = multi_linker_input_dir(primary_output);
    if !multi_linker_input_dir.is_dir() {
        return Ok(vec![primary_output.to_path_buf()]);
    }

    let manifest =
        fs::read_to_string(primary_output).map_err(|err| Error::from_io(primary_output, err))?;
    let mut paths = Vec::new();
    for line in manifest.lines() {
        if line == "version=1" || line.is_empty() {
            continue;
        }
        if let Some(path) = line.strip_prefix("linker_input=") {
            let path = PathBuf::from(path);
            if !path.exists() {
                return Err(Error::Execution(format!(
                    "linker-input manifest `{}` references missing path `{}`",
                    primary_output.display(),
                    path.display()
                )));
            }
            paths.push(path);
            continue;
        }
        return Err(Error::Execution(format!(
            "linker-input manifest `{}` contains an unsupported entry `{}`",
            primary_output.display(),
            line
        )));
    }

    if paths.is_empty() {
        return Err(Error::Execution(format!(
            "linker-input manifest `{}` does not list any preserved linker inputs",
            primary_output.display()
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
