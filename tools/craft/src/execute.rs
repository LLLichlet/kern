use crate::build_plan::{ActionPlan, BuildPlan, CompileAction, LinkAction};
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
use std::time::Duration;

mod external;
mod fingerprint;
mod options;
mod orchestrate;
mod parallel;
mod progress;
mod runtime;
mod runtime_packages;
mod staging;

use self::external::{
    build_external_package, compile_actions_index, link_actions_by_artifact_path,
    local_library_actions, requested_external_dependencies,
};
use self::fingerprint::{
    base_compile_action_label, build_fingerprint, compile_action_detail_tags,
    compile_action_fingerprint, compile_action_label, link_action_detail_tags,
    link_action_fingerprint, link_action_label, prov_compile_action_label, rt_compile_action_label,
    rt_entry_compile_action_label, runtime_compile_detail_tags, std_compile_action_label,
    write_compile_action_state,
};
use self::options::{compile_action_options, link_action_options};
use self::orchestrate::build_with_command;
use self::parallel::{
    build_parallel_target_compile_jobs, build_parallel_target_link_jobs,
    compile_action_for_link_action, parallel_target_compile_jobs, parallel_target_link_jobs,
};
use self::runtime_packages::ensure_std_packages_for_actions;
use self::staging::{
    cleanup_stale_artifact_outputs, cleanup_stale_compile_inputs, compile_progress_label,
    execute_staged_actions, link_progress_label,
};

pub use self::orchestrate::{build_with_progress, check_with_progress};
pub(crate) use self::orchestrate::{
    materialize_analysis_inputs, materialize_analysis_inputs_with_progress,
};
pub use self::progress::{
    ExecutionPhase, ExecutionProgressPlan, ExecutionProgressSnapshot, ProgressReporter,
};
pub use self::runtime::{run_built, test_built};

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

impl ExecutionSummary {
    pub fn total_duration(&self) -> Duration {
        self.phase_timings
            .iter()
            .map(|phase| phase.duration)
            .sum::<Duration>()
    }

    fn absorb(&mut self, other: ExecutionSummary) {
        // Aggregate coarse phase timings by phase label so higher-level callers get one merged
        // summary even when work happened across many compile/link actions.
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct BuiltExternalPackage {
    metadata_root_path: PathBuf,
    link_objects: Vec<PathBuf>,
    module_aliases: BTreeMap<String, PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BuiltStdPackage {
    metadata_root_path: PathBuf,
    base_object_path: PathBuf,
    prov_object_path: PathBuf,
    rt_object_path: Option<PathBuf>,
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
    export_package_name: String,
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
        "{}-opt{}-debug{}-cgu{}-lto{}-codemodel{}",
        profile.name,
        profile.opt,
        profile.debug,
        profile.codegen_units,
        profile.lto_mode.as_str(),
        profile.code_model.as_str()
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
    progress: Option<&ProgressReporter>,
) -> Result<bool> {
    // The output lock and fingerprint check make compile actions idempotent even when the build
    // graph reaches the same unit from multiple dependent targets.
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
    let _long_action = progress
        .map(|progress| progress.report_long_action("compiling", compile_progress_label(action)));
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
    #[cfg(test)]
    crate::test_support::hit(crate::test_support::FAILPOINT_AFTER_COMPILE_STATE_WRITE);

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
    progress: Option<&ProgressReporter>,
) -> Result<bool> {
    let _link_lock = OutputOperationLock::acquire(&action.artifact_path, "link-action")?;
    let toolchain_digest = build_state::current_process_digest()?;
    // Link fingerprints include discovered local search-path inputs so adding or replacing a
    // project-local native library invalidates the cached link even if explicit inputs are stable.
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
    let _long_action = progress
        .map(|progress| progress.report_long_action("linking", link_progress_label(action)));
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
    #[cfg(test)]
    crate::test_support::hit(crate::test_support::FAILPOINT_AFTER_LINK_STATE_WRITE);
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

    // Stage generators run before fingerprinted compilation so generated headers, copied assets,
    // or emitted sources are all present when we hash and compile the action inputs.
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
        progress.set_detail(compile_progress_label(action));
    }
    let _progress_suspend = session
        .state
        .progress
        .as_ref()
        .map(|progress| progress.suspend_terminal());
    let built = build_compile_action_if_needed(
        action,
        options,
        session.external.driver_families,
        session.state.execution_summary,
        session.state.progress.as_ref(),
    )?;
    if let Some(progress) = &session.state.progress {
        progress.record_compile_action();
    }
    session.state.compiled.insert(action.object_path.clone());
    Ok(built)
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

    // Linking is intentionally serialized after the primary compile action completes so the linker
    // always sees the current object, metadata manifest, and any staged post-compile artifacts.
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
        progress.set_detail(link_progress_label(action));
    }
    let _progress_suspend = session
        .state
        .progress
        .as_ref()
        .map(|progress| progress.suspend_terminal());
    let linked_now = build_link_action_if_needed(
        action,
        options,
        &linker_inputs,
        session.state.execution_summary,
        session.state.progress.as_ref(),
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
    let package_root = normalize_local_input_path(&action.package_root_path);
    options
        .linker_search_paths
        .iter()
        .map(PathBuf::from)
        .filter(|path| path.is_dir() && normalize_local_input_path(path).starts_with(&package_root))
        .collect()
}

fn normalize_local_input_path(path: &Path) -> PathBuf {
    strip_macos_private_var_prefix(strip_windows_verbatim_prefix(path.to_path_buf()))
}

#[cfg(windows)]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix("\\\\?\\UNC\\") {
        return PathBuf::from(format!("\\\\{stripped}"));
    }
    if let Some(stripped) = raw.strip_prefix("\\\\?\\") {
        return PathBuf::from(stripped);
    }
    path
}

#[cfg(not(windows))]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    path
}

#[cfg(target_os = "macos")]
fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix("/private/var/") {
        return PathBuf::from(format!("/var/{stripped}"));
    }
    if raw == "/private/var" {
        return PathBuf::from("/var");
    }
    path
}

#[cfg(not(target_os = "macos"))]
fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
    path
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
