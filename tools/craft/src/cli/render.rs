//! Human-readable rendering for Craft command output.
//!
//! Render helpers format progress lines, summaries, diagnostics, timing tables,
//! source-policy reports, and install/uninstall messages while honoring terminal
//! width and color settings.

use crate::build_plan;
use crate::elaborate;
use crate::execute;
use crate::manifest::Manifest;
use crate::plan::TargetKind;
use crate::source;
use kernc_driver::{CodegenPlanFallback, CodegenPlanReport, CompileCacheStats, PhaseTiming};
use std::fmt::Display;
use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{ColorChoice, UiOptions, Verbosity};

pub(super) struct Renderer {
    verbosity: Verbosity,
    timings: bool,
    color_enabled: bool,
    terminal_output: bool,
    quiet: bool,
}

pub(super) struct ProgressDisplay {
    reporter: execute::ProgressReporter,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

pub(super) struct PipelineProgressDisplay {
    reporter: PipelineProgressReporter,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone, Copy)]
struct ProgressStyle {
    color_enabled: bool,
}

#[derive(Clone)]
pub(super) struct PipelineProgressReporter {
    state: Arc<Mutex<PipelineProgressState>>,
}

#[derive(Debug, Clone)]
struct PipelineProgressState {
    total_steps: usize,
    current_step: usize,
    label: String,
    detail: String,
    started_at: Instant,
}

#[derive(Debug, Clone)]
struct PipelineProgressSnapshot {
    total_steps: usize,
    current_step: usize,
    label: String,
    detail: String,
    elapsed: Duration,
}

const PROGRESS_BAR_WIDTH: usize = 24;
const MIN_PROGRESS_BAR_WIDTH: usize = 12;
const MIN_PROGRESS_COLUMNS: usize = 48;
const MIN_PROGRESS_DETAIL_COLUMNS: usize = 12;

#[derive(Clone, Copy)]
pub(super) enum Tone {
    Accent,
    Muted,
    Ok,
    Build,
    Link,
    Generate,
    Fetch,
}

impl Renderer {
    const LABEL_WIDTH: usize = 10;

    pub(super) fn new(ui: UiOptions) -> Self {
        let quiet = test_ui_output_is_suppressed();
        let terminal_output = !quiet && std::io::stderr().is_terminal();
        let color_enabled = match ui.color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => terminal_output && std::env::var_os("NO_COLOR").is_none(),
        };

        Self {
            verbosity: ui.verbosity,
            timings: ui.timings,
            color_enabled,
            terminal_output,
            quiet,
        }
    }

    pub(super) fn header_with_path(
        &self,
        command: &str,
        manifest: &Manifest,
        manifest_path: &std::path::Path,
        feature_selection: &elaborate::FeatureSelection,
    ) {
        if self.quiet {
            return;
        }
        let marker = self.paint(Tone::Accent, "==>");
        let command = self.paint(Tone::Accent, command);
        println!("{marker} {command} {}", format_package_label(manifest));
        if self.is_verbose() {
            self.meta("manifest", manifest_path.display());
            self.meta("features", format_feature_inputs(feature_selection));
        }
    }

    pub(super) fn meta(&self, label: &str, value: impl Display) {
        if self.quiet {
            return;
        }
        let label = self.paint(
            Tone::Muted,
            &format!("{label:<width$}", width = Self::LABEL_WIDTH),
        );
        println!("    {label} {value}");
    }

    pub(super) fn summary(&self, label: &str, value: impl Display) {
        if self.quiet {
            return;
        }
        let label = self.paint(
            Tone::Muted,
            &format!("{label:<width$}", width = Self::LABEL_WIDTH),
        );
        println!("    {label} {value}");
    }

    pub(super) fn section(&self, name: &str) {
        if self.quiet || !self.is_verbose() {
            return;
        }
        let marker = self.paint(Tone::Muted, "--");
        let name = self.paint(Tone::Accent, name);
        println!("  {marker} {name}");
    }

    pub(super) fn action(
        &self,
        tone: Tone,
        kind: &str,
        subject: impl Display,
        detail: impl Display,
    ) {
        if self.quiet || !self.is_verbose() {
            return;
        }
        let kind = self.paint(tone, &format!("{kind:<8}"));
        println!("  {kind} {subject} {detail}");
    }

    pub(super) fn ok(&self, message: impl Display) {
        if self.quiet {
            return;
        }
        println!("{} {message}", self.paint(Tone::Ok, "[ok]"));
    }

    pub(super) fn is_verbose(&self) -> bool {
        self.verbosity >= Verbosity::Verbose
    }

    pub(super) fn is_debug(&self) -> bool {
        self.verbosity >= Verbosity::Debug
    }

    pub(super) fn is_trace(&self) -> bool {
        self.verbosity >= Verbosity::Trace
    }

    pub(super) fn progress(
        &self,
        command: &'static str,
        plan: execute::ExecutionProgressPlan,
    ) -> Option<ProgressDisplay> {
        if self.quiet || self.is_verbose() || !self.terminal_output || plan.is_empty() {
            return None;
        }

        Some(ProgressDisplay::spawn(
            command,
            plan,
            ProgressStyle {
                color_enabled: self.color_enabled,
            },
        ))
    }

    pub(super) fn pipeline_progress(
        &self,
        command: &'static str,
        total_steps: usize,
    ) -> Option<PipelineProgressDisplay> {
        if self.quiet || self.is_verbose() || !self.terminal_output || total_steps == 0 {
            return None;
        }

        Some(PipelineProgressDisplay::spawn(
            command,
            total_steps,
            ProgressStyle {
                color_enabled: self.color_enabled,
            },
        ))
    }

    fn paint(&self, tone: Tone, text: &str) -> String {
        if !self.color_enabled {
            return text.to_string();
        }

        let code = match tone {
            Tone::Accent => "1;36",
            Tone::Muted => "2",
            Tone::Ok => "1;32",
            Tone::Build => "1;34",
            Tone::Link => "1;35",
            Tone::Generate => "1;36",
            Tone::Fetch => "1;32",
        };
        format!("\x1b[{code}m{text}\x1b[0m")
    }
}

#[cfg(test)]
fn test_ui_output_is_suppressed() -> bool {
    std::env::var_os("CRAFT_TEST_SHOW_UI").is_none()
}

#[cfg(not(test))]
fn test_ui_output_is_suppressed() -> bool {
    false
}

impl ProgressDisplay {
    fn spawn(
        command: &'static str,
        plan: execute::ExecutionProgressPlan,
        style: ProgressStyle,
    ) -> Self {
        let reporter = execute::ProgressReporter::new(plan);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker_reporter = reporter.clone();
        let worker = thread::spawn(move || {
            let mut last_len = 0usize;
            let mut last_line = String::new();
            loop {
                if worker_reporter.terminal_suspended() {
                    if last_len != 0 {
                        clear_progress_line(last_len);
                        last_len = 0;
                        last_line.clear();
                    }
                } else {
                    let snapshot = worker_reporter.snapshot();
                    let line =
                        render_progress_line(command, snapshot, progress_line_columns(), style);
                    if line != last_line {
                        write_progress_line(&line, &mut last_len);
                        last_line = line;
                    }
                }
                if worker_stop.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(120));
            }
            clear_progress_line(last_len);
        });

        Self {
            reporter,
            stop,
            worker: Some(worker),
        }
    }

    pub(super) fn reporter(&self) -> execute::ProgressReporter {
        self.reporter.clone()
    }

    pub(super) fn finish(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for ProgressDisplay {
    fn drop(&mut self) {
        self.finish();
    }
}

impl PipelineProgressDisplay {
    fn spawn(command: &'static str, total_steps: usize, style: ProgressStyle) -> Self {
        let reporter = PipelineProgressReporter::new(total_steps);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker_reporter = reporter.clone();
        let worker = thread::spawn(move || {
            let mut last_len = 0usize;
            let mut last_line = String::new();
            loop {
                let snapshot = worker_reporter.snapshot();
                let line = render_pipeline_progress_line(
                    command,
                    snapshot,
                    progress_line_columns(),
                    style,
                );
                if line != last_line {
                    write_progress_line(&line, &mut last_len);
                    last_line = line;
                }
                if worker_stop.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(120));
            }
            clear_progress_line(last_len);
        });

        Self {
            reporter,
            stop,
            worker: Some(worker),
        }
    }

    pub(super) fn step(&self, label: impl Into<String>, detail: impl Into<String>) {
        self.reporter.step(label, detail);
    }

    pub(super) fn detail(&self, detail: impl Into<String>) {
        self.reporter.detail(detail);
    }

    pub(super) fn finish(&mut self) {
        self.reporter.complete();
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for PipelineProgressDisplay {
    fn drop(&mut self) {
        self.finish();
    }
}

impl PipelineProgressReporter {
    fn new(total_steps: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(PipelineProgressState {
                total_steps,
                current_step: 0,
                label: "prepare".to_string(),
                detail: String::new(),
                started_at: Instant::now(),
            })),
        }
    }

    fn step(&self, label: impl Into<String>, detail: impl Into<String>) {
        if let Ok(mut state) = self.state.lock() {
            state.current_step = state.current_step.saturating_add(1).min(state.total_steps);
            state.label = label.into();
            state.detail = detail.into();
        }
    }

    fn detail(&self, detail: impl Into<String>) {
        if let Ok(mut state) = self.state.lock() {
            state.detail = detail.into();
        }
    }

    fn complete(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.current_step = state.total_steps;
        }
    }

    fn snapshot(&self) -> PipelineProgressSnapshot {
        let state = self
            .state
            .lock()
            .expect("pipeline progress state should not be poisoned");
        PipelineProgressSnapshot {
            total_steps: state.total_steps,
            current_step: state.current_step,
            label: state.label.clone(),
            detail: state.detail.clone(),
            elapsed: state.started_at.elapsed(),
        }
    }
}

impl ProgressStyle {
    fn paint(self, tone: Tone, text: &str) -> String {
        if !self.color_enabled {
            return text.to_string();
        }

        let code = match tone {
            Tone::Accent => "1;36",
            Tone::Muted => "2",
            Tone::Ok => "1;32",
            Tone::Build => "1;34",
            Tone::Link => "1;35",
            Tone::Generate => "1;36",
            Tone::Fetch => "1;32",
        };
        format!("\x1b[{code}m{text}\x1b[0m")
    }
}

fn format_package_label(manifest: &Manifest) -> String {
    if let Some(package) = &manifest.package {
        return format!("{} {}", package.name, package.version);
    }
    if let Some(workspace) = &manifest.workspace {
        return format!("workspace {}", workspace.name);
    }
    "manifest".to_string()
}

pub(super) fn format_yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn format_feature_inputs(selection: &elaborate::FeatureSelection) -> String {
    format!(
        "{}, default={}, explicit={}",
        selection.profile.name(),
        if selection.enable_default {
            "on"
        } else {
            "off"
        },
        format_explicit_features(selection)
    )
}

fn format_explicit_features(selection: &elaborate::FeatureSelection) -> String {
    if selection.explicit.is_empty() {
        "<none>".to_string()
    } else {
        selection
            .explicit
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(",")
    }
}

pub(super) fn format_unit_label(unit: &build_plan::BuildUnit) -> String {
    format!(
        "{}:{} [{},{}]",
        unit.package_id.name,
        unit.artifact_name,
        unit.target_kind.as_str(),
        unit.domain.as_str()
    )
}

pub(super) fn format_external_package_label(
    package: &crate::resolver::ExternalPackageId,
) -> String {
    match &package.version {
        Some(version) => format!("{} {}", package.package_name, version),
        None => package.package_name.clone(),
    }
}

fn format_fetched_source_backend(package: &source::FetchedPackage) -> &'static str {
    package.source.backend.as_str()
}

fn format_fetched_resource_backend(resource: &source::FetchedResource) -> &'static str {
    resource.source.backend.as_str()
}

fn format_action_label(
    package_id: &crate::graph::PackageId,
    domain: crate::graph::BuildDomain,
    target_kind: TargetKind,
    artifact_name: &str,
) -> String {
    format!(
        "{}:{} [{},{}]",
        package_id.name,
        artifact_name,
        target_kind.as_str(),
        domain.as_str()
    )
}

fn format_plan_value(value: &crate::plan::PlanValue) -> String {
    match value {
        crate::plan::PlanValue::Bool(value) => value.to_string(),
        crate::plan::PlanValue::String(value) => value.clone(),
    }
}

fn format_plan_map(values: &std::collections::BTreeMap<String, crate::plan::PlanValue>) -> String {
    if values.is_empty() {
        "<none>".to_string()
    } else {
        values
            .iter()
            .map(|(key, value)| format!("{key}={}", format_plan_value(value)))
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn format_source_input(input: &build_plan::CompileSourceInput) -> String {
    match input {
        build_plan::CompileSourceInput::PackagePath(path) => {
            format!("package:{}", path.display())
        }
        build_plan::CompileSourceInput::AbsolutePath(path) => {
            format!("absolute:{}", path.display())
        }
        build_plan::CompileSourceInput::BuildOutput { id, path } => {
            format!("build_output#{id}:{}", path.display())
        }
    }
}

pub(super) fn print_compile_actions(render: &Renderer, action_plan: &build_plan::ActionPlan) {
    for action in &action_plan.compile_actions {
        print_compile_action(render, action, &action.artifact_name);
    }
}

pub(super) fn print_generated_files(render: &Renderer, build_plan: &build_plan::BuildPlan) {
    for package in &build_plan.packages {
        for unit in &package.units {
            print_generated_files_for_unit(render, unit);
        }
    }
}

pub(super) fn print_generated_files_for_unit(render: &Renderer, unit: &build_plan::BuildUnit) {
    if unit.generated_files.is_empty() {
        return;
    }

    let files = unit
        .generated_files
        .iter()
        .map(|file| match &file.origin {
            build_plan::GeneratedFileOrigin::Emitted => file.path.clone(),
            build_plan::GeneratedFileOrigin::Copied { source } => {
                format!("{}<=copy:{}", file.path, source)
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    render.action(
        Tone::Generate,
        "generate",
        format_unit_label(unit),
        format!("-> {files}"),
    );
}

pub(super) fn print_link_actions(render: &Renderer, action_plan: &build_plan::ActionPlan) {
    for action in &action_plan.link_actions {
        print_link_action(render, action, &action.artifact_name);
    }
}

pub(super) fn print_compile_actions_for_unit(
    render: &Renderer,
    action_plan: &build_plan::ActionPlan,
    unit: &build_plan::BuildUnit,
) {
    for action in action_plan.compile_actions.iter().filter(|action| {
        action.domain == unit.domain
            && action.package_id == unit.package_id
            && action.target_kind == unit.target_kind
            && action.target_name == unit.target_name
    }) {
        print_compile_action(render, action, &unit.artifact_name);
    }
}

pub(super) fn print_link_actions_for_unit(
    render: &Renderer,
    action_plan: &build_plan::ActionPlan,
    unit: &build_plan::BuildUnit,
) {
    for action in action_plan.link_actions.iter().filter(|action| {
        action.domain == unit.domain
            && action.package_id == unit.package_id
            && action.target_kind == unit.target_kind
            && action.target_name == unit.target_name
    }) {
        print_link_action(render, action, &unit.artifact_name);
    }
}

pub(super) fn print_fetched_package(render: &Renderer, package: &source::FetchedPackage) {
    render.action(
        Tone::Fetch,
        "fetch",
        format_external_package_label(&package.id),
        format!(
            "from {} [{}] -> {}",
            package.source.locator,
            format_fetched_source_backend(package),
            package.cache_path.display()
        ),
    );
}

pub(super) fn print_fetched_resource(render: &Renderer, resource: &source::FetchedResource) {
    render.action(
        Tone::Fetch,
        "fetch",
        format!("{}::{}", resource.id.package_id.name, resource.id.name),
        format!(
            "from {} [{}] -> {}",
            resource.source.locator,
            format_fetched_resource_backend(resource),
            resource.cache_path.display()
        ),
    );
}

fn print_compile_action(
    render: &Renderer,
    action: &build_plan::CompileAction,
    artifact_name: &str,
) {
    let mut detail = format!("<= {}", format_source_input(&action.source_input));
    if !action.cfg.is_empty() {
        detail.push_str(&format!(" | cfg {}", format_plan_map(&action.cfg)));
    }
    if !action.define.is_empty() {
        detail.push_str(&format!(" | define {}", format_plan_map(&action.define)));
    }
    if render.is_debug() {
        detail.push_str(&format!(
            " | profile {} | object {}",
            action.profile.name.as_str(),
            action.object_path.display()
        ));
        if let Some(metadata_path) = &action.metadata_path {
            detail.push_str(&format!(" | metadata {}", metadata_path.display()));
        }
        if !action.local_dependencies.is_empty() {
            detail.push_str(&format!(
                " | local-deps {}",
                action
                    .local_dependencies
                    .iter()
                    .map(|dep| dep.dependency_name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    if render.is_trace() {
        detail.push_str(&format!(
            " | manifest {} | generated-root {}",
            action.manifest_path.display(),
            action.generated_root_path.display()
        ));
        if !action.compile_inputs.is_empty() {
            detail.push_str(&format!(
                " | build-inputs {}",
                action
                    .compile_inputs
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    render.action(
        Tone::Build,
        "compile",
        format_action_label(
            &action.package_id,
            action.domain,
            action.target_kind,
            artifact_name,
        ),
        detail,
    );
}

fn print_link_action(render: &Renderer, action: &build_plan::LinkAction, artifact_name: &str) {
    let mut extras = Vec::new();
    if !action.external_dependencies.is_empty() {
        extras.push(format!(
            "externals {}",
            action
                .external_dependencies
                .iter()
                .map(|dep| dep.package_id.package_name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !action.link.system_libs.is_empty() {
        extras.push(format!("libs {}", action.link.system_libs.join(", ")));
    }
    if !action.link.frameworks.is_empty() {
        extras.push(format!("frameworks {}", action.link.frameworks.join(", ")));
    }
    if !action.link.search_paths.is_empty() {
        extras.push(format!("search {}", action.link.search_paths.join(", ")));
    }
    if !action.link.args.is_empty() {
        extras.push(format!("args {}", action.link.args.join(", ")));
    }
    if render.is_debug() {
        extras.push(format!("primary {}", action.primary_object.display()));
        if !action.local_library_objects.is_empty() {
            extras.push(format!(
                "local-objects {}",
                action
                    .local_library_objects
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    if render.is_trace() {
        extras.push(format!("manifest {}", action.manifest_path.display()));
        extras.push(format!(
            "package-root {}",
            action.package_root_path.display()
        ));
        extras.push(format!(
            "artifact-root {}",
            action.artifact_root_path.display()
        ));
        if !action.artifact_outputs.is_empty() {
            extras.push(format!(
                "build-outputs {}",
                action
                    .artifact_outputs
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    let detail = if extras.is_empty() {
        format!("-> {}", action.artifact_path.display())
    } else {
        format!(
            "-> {} ({})",
            action.artifact_path.display(),
            extras.join("; ")
        )
    };
    render.action(
        Tone::Link,
        "link",
        format_action_label(
            &action.package_id,
            action.domain,
            action.target_kind,
            artifact_name,
        ),
        detail,
    );
}

pub(super) fn render_execution_timings(render: &Renderer, summary: &execute::ExecutionSummary) {
    if !render.timings
        || (summary.phase_timings.is_empty()
            && summary.cache_stats.is_empty()
            && summary.action_cache_stats.is_empty())
    {
        return;
    }

    if !summary.phase_timings.is_empty() {
        render.summary("time", format_duration(summary.total_duration()));
        render.summary("phases", format_phase_timings(&summary.phase_timings));
    }
    if let Some(thinlto_summary) = format_thinlto_link_summary(summary) {
        render.summary("thinlto", thinlto_summary);
    }
    if !summary.cache_stats.is_empty() {
        render.summary("cache", format_compile_cache_stats(summary.cache_stats));
    }
    if !summary.action_cache_stats.is_empty() {
        render.summary(
            "action-cache",
            format_action_cache_stats(summary.action_cache_stats),
        );
    }

    if !render.is_verbose() {
        return;
    }

    render.section("timings");
    for action in &summary.action_timings {
        let tone = match action.kind {
            execute::ActionTimingKind::Compile => Tone::Build,
            execute::ActionTimingKind::Link => Tone::Link,
        };
        render.action(
            tone,
            "time",
            &action.label,
            format_action_timing_detail(
                &action.detail_tags,
                &action.phase_timings,
                action.cache_stats,
                action.codegen_plan.as_ref(),
            ),
        );
    }
}

fn format_phase_timings(phases: &[PhaseTiming]) -> String {
    phases
        .iter()
        .map(|phase| format!("{} {}", phase.name, format_duration(phase.duration)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_compile_cache_stats(stats: CompileCacheStats) -> String {
    [
        format!(
            "compile {}/{}",
            stats.compile_structure_hits,
            stats.compile_structure_hits + stats.compile_structure_misses
        ),
        format!(
            "typed {}/{}",
            stats.structure_hits,
            stats.structure_hits + stats.structure_misses
        ),
        format!(
            "imported {}/{}",
            stats.imported_hits,
            stats.imported_hits + stats.imported_misses
        ),
        format!(
            "collected {}/{}",
            stats.collected_hits,
            stats.collected_hits + stats.collected_misses
        ),
        format!("frontend_parse {}", stats.fresh_frontend_parses),
    ]
    .join(", ")
}

fn format_action_cache_stats(stats: execute::ActionCacheStats) -> String {
    [
        format!(
            "compile {}/{}",
            stats.compile_hits,
            stats.compile_hits + stats.compile_misses
        ),
        format!(
            "link {}/{}",
            stats.link_hits,
            stats.link_hits + stats.link_misses
        ),
        format!(
            "staged {}/{}",
            stats.staged_hits,
            stats.staged_hits + stats.staged_misses
        ),
    ]
    .join(", ")
}

fn format_action_timing_detail(
    detail_tags: &[String],
    phases: &[PhaseTiming],
    cache_stats: CompileCacheStats,
    codegen_plan: Option<&CodegenPlanReport>,
) -> String {
    let mut parts = Vec::new();
    if !detail_tags.is_empty() {
        parts.push(detail_tags.join(", "));
    }
    if !phases.is_empty() {
        parts.push(format_phase_timings(phases));
    }
    if !cache_stats.is_empty() {
        parts.push(format_compile_cache_stats(cache_stats));
    }
    if let Some(codegen_plan) = codegen_plan {
        parts.push(format_codegen_plan(codegen_plan));
    }
    parts.join("; ")
}

fn format_thinlto_link_summary(summary: &execute::ExecutionSummary) -> Option<String> {
    let mut final_link_count = 0usize;
    let mut cross_package_count = 0usize;
    let mut total_inputs = 0usize;
    let mut max_inputs = 0usize;

    for action in &summary.action_timings {
        if !has_detail_tag(&action.detail_tags, "pipeline=thinlto-final-link") {
            continue;
        }

        final_link_count += 1;
        if has_detail_tag(&action.detail_tags, "cross-package=true") {
            cross_package_count += 1;
        }
        if let Some(inputs) = parse_usize_detail_tag(&action.detail_tags, "inputs=") {
            total_inputs += inputs;
            max_inputs = max_inputs.max(inputs);
        }
    }

    if final_link_count == 0 {
        return None;
    }

    Some(format!(
        "final-links {}, cross-package {}, total-inputs {}, max-inputs {}",
        final_link_count, cross_package_count, total_inputs, max_inputs
    ))
}

fn has_detail_tag(detail_tags: &[String], needle: &str) -> bool {
    detail_tags.iter().any(|tag| tag == needle)
}

fn parse_usize_detail_tag(detail_tags: &[String], prefix: &str) -> Option<usize> {
    detail_tags
        .iter()
        .find_map(|tag| tag.strip_prefix(prefix)?.parse().ok())
}

fn format_codegen_plan(report: &CodegenPlanReport) -> String {
    let fallback = match &report.fallback_reason {
        Some(CodegenPlanFallback::RequestedSingleUnit) => "requested_single_unit".to_string(),
        Some(CodegenPlanFallback::ContainsControlFlowAsm { function_name }) => {
            format!("contains_control_flow_asm({function_name})")
        }
        Some(CodegenPlanFallback::NameCollision { item_kind, name }) => {
            format!("name_collision({item_kind}:{name})")
        }
        Some(CodegenPlanFallback::TooFewRoots) => "too_few_roots".to_string(),
        Some(CodegenPlanFallback::TooFewTargetUnits) => "too_few_target_units".to_string(),
        Some(CodegenPlanFallback::TooFewMaterializedUnits) => {
            "too_few_materialized_units".to_string()
        }
        None => "planned".to_string(),
    };

    format!(
        "cgu-plan requested={} roots={} clusters={} planned={} workload={} cluster_min={} cluster_max={} unit_min={} unit_max={} promoted_fns={} promoted_globals={} fallback={}",
        report.requested_units,
        report.root_count,
        report.cluster_count,
        report.planned_units,
        report.total_workload,
        report.min_cluster_workload,
        report.max_cluster_workload,
        report.min_unit_workload,
        report.max_unit_workload,
        report.promoted_function_count,
        report.promoted_global_count,
        fallback
    )
}

fn format_duration(duration: std::time::Duration) -> String {
    if duration.as_secs() >= 1 {
        format!("{:.3}s", duration.as_secs_f64())
    } else if duration.as_millis() >= 1 {
        format!("{:.3}ms", duration.as_secs_f64() * 1_000.0)
    } else if duration.as_micros() >= 1 {
        format!("{:.3}us", duration.as_secs_f64() * 1_000_000.0)
    } else {
        format!("{}ns", duration.as_nanos())
    }
}

fn render_progress_line(
    command: &str,
    snapshot: execute::ExecutionProgressSnapshot,
    columns: usize,
    style: ProgressStyle,
) -> String {
    let total_steps = snapshot.total_steps();
    let completed_steps = snapshot.completed_steps().min(total_steps);
    let percent = completed_steps
        .saturating_mul(100)
        .checked_div(total_steps)
        .unwrap_or(0);
    let bar = render_progress_bar(completed_steps, total_steps, progress_bar_width(columns));
    let mut segments = Vec::new();
    segments.push(format_phase_progress(&snapshot));
    segments.push(format_progress_clock(snapshot.elapsed));
    if total_steps > 0
        && completed_steps >= 2
        && completed_steps < total_steps
        && snapshot.elapsed >= Duration::from_secs(3)
    {
        let remaining_steps = total_steps - completed_steps;
        let eta = snapshot
            .elapsed
            .mul_f64(remaining_steps as f64 / completed_steps as f64);
        segments.push(format!("eta {}", format_progress_clock(eta)));
    }

    let mut line = format!(
        "{} {} {}  {}",
        style.paint(Tone::Accent, command),
        style.paint(Tone::Build, &bar),
        style.paint(Tone::Muted, &format!("{percent:>3}%")),
        segments.join("  ")
    );
    if !snapshot.detail.is_empty() {
        let detail_budget = columns.saturating_sub(display_width(&line) + 2);
        if detail_budget >= MIN_PROGRESS_DETAIL_COLUMNS {
            line.push_str("  ");
            line.push_str(&truncate_detail(&snapshot.detail, detail_budget));
        }
    }
    truncate_text(&line, columns)
}

fn render_pipeline_progress_line(
    command: &str,
    snapshot: PipelineProgressSnapshot,
    columns: usize,
    style: ProgressStyle,
) -> String {
    let total_steps = snapshot.total_steps.max(1);
    let completed_steps = snapshot.current_step;
    let visual_total = total_steps.max(completed_steps.saturating_add(1));
    let percent = completed_steps
        .saturating_mul(100)
        .checked_div(visual_total)
        .unwrap_or(0);
    let bar = render_progress_bar(completed_steps, visual_total, progress_bar_width(columns));
    let step = format_pipeline_step(&snapshot, completed_steps, total_steps);
    let mut line = format!(
        "{} {} {}  {}  {}",
        style.paint(Tone::Accent, command),
        style.paint(Tone::Build, &bar),
        style.paint(Tone::Muted, &format!("{percent:>3}%")),
        step,
        format_progress_clock(snapshot.elapsed)
    );
    if !snapshot.detail.is_empty() {
        let detail_budget = columns.saturating_sub(display_width(&line) + 2);
        if detail_budget >= MIN_PROGRESS_DETAIL_COLUMNS {
            line.push_str("  ");
            line.push_str(&truncate_detail(&snapshot.detail, detail_budget));
        }
    }
    truncate_text(&line, columns)
}

fn format_pipeline_step(
    snapshot: &PipelineProgressSnapshot,
    completed_steps: usize,
    total_steps: usize,
) -> String {
    let phase_step = match snapshot.label.as_str() {
        "manifest" => Some((completed_steps.min(2), 2)),
        "workspace" => Some((1, 1)),
        "lock" => Some((1, 1)),
        "graph" => Some((1, 1)),
        "lockfile" => Some((1, 1)),
        "package" => Some((1, 1)),
        "plan" => Some((completed_steps.saturating_sub(7).min(3), 3)),
        _ => None,
    };
    if let Some((current, total)) = phase_step {
        format!("{} {current}/{total}", snapshot.label)
    } else if completed_steps == 0 {
        format!("{} 0/{total_steps}", snapshot.label)
    } else {
        snapshot.label.clone()
    }
}

fn render_progress_bar(completed: usize, total: usize, width: usize) -> String {
    if total == 0 {
        return format!("[>{}]", "-".repeat(width.saturating_sub(1)));
    }

    if completed >= total {
        return format!("[{}]", "=".repeat(width));
    }

    let filled = completed.saturating_mul(width) / total;
    let head = filled.min(width.saturating_sub(1));
    format!(
        "[{}>{}]",
        "=".repeat(head),
        "-".repeat(width.saturating_sub(head + 1))
    )
}

fn format_progress_phase(phase: execute::ExecutionPhase) -> &'static str {
    match phase {
        execute::ExecutionPhase::Bootstrap => "prepare",
        execute::ExecutionPhase::Stage => "gen",
        execute::ExecutionPhase::Compile => "compile",
        execute::ExecutionPhase::Link => "link",
    }
}

fn format_phase_progress(snapshot: &execute::ExecutionProgressSnapshot) -> String {
    match snapshot.phase {
        execute::ExecutionPhase::Bootstrap => format_progress_phase(snapshot.phase).to_string(),
        execute::ExecutionPhase::Stage => format!(
            "{} {}/{}",
            format_progress_phase(snapshot.phase),
            snapshot.staged_done.min(snapshot.plan.staged_actions),
            snapshot.plan.staged_actions
        ),
        execute::ExecutionPhase::Compile => format!(
            "{} {}/{}",
            format_progress_phase(snapshot.phase),
            snapshot.compile_done.min(snapshot.plan.compile_actions),
            snapshot.plan.compile_actions
        ),
        execute::ExecutionPhase::Link => format!(
            "{} {}/{}",
            format_progress_phase(snapshot.phase),
            snapshot.link_done.min(snapshot.plan.link_actions),
            snapshot.plan.link_actions
        ),
    }
}

fn format_progress_clock(duration: Duration) -> String {
    if duration.as_secs() >= 60 {
        let mins = duration.as_secs() / 60;
        let secs = duration.as_secs() % 60;
        format!("{mins}m{secs:02}s")
    } else if duration.as_secs() >= 1 {
        format!("{}s", duration.as_secs())
    } else {
        "<1s".to_string()
    }
}

fn write_progress_line(line: &str, last_len: &mut usize) {
    let mut stderr = std::io::stderr();
    let _ = write!(stderr, "\r\x1b[2K{line}");
    let _ = stderr.flush();
    *last_len = display_width(line);
}

fn clear_progress_line(last_len: usize) {
    if last_len == 0 {
        return;
    }
    let mut stderr = std::io::stderr();
    let _ = write!(stderr, "\r\x1b[2K\r");
    let _ = stderr.flush();
}

fn progress_line_columns() -> usize {
    terminal_columns()
        .or_else(columns_from_env)
        .unwrap_or(100)
        .max(MIN_PROGRESS_COLUMNS)
}

fn progress_bar_width(columns: usize) -> usize {
    columns
        .saturating_sub(60)
        .clamp(MIN_PROGRESS_BAR_WIDTH, PROGRESS_BAR_WIDTH)
}

fn columns_from_env() -> Option<usize> {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
}

#[cfg(unix)]
fn terminal_columns() -> Option<usize> {
    use std::mem::MaybeUninit;
    use std::os::fd::AsRawFd;

    let fd = std::io::stderr().as_raw_fd();
    let mut winsize = MaybeUninit::<libc::winsize>::uninit();
    // SAFETY: winsize points to writable uninitialized storage for ioctl to fill. The value is
    // only assumed initialized after ioctl reports success.
    let result = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, winsize.as_mut_ptr()) };
    if result != 0 {
        return None;
    }

    // SAFETY: ioctl returned success, so the kernel initialized winsize.
    let winsize = unsafe { winsize.assume_init() };
    (winsize.ws_col > 0).then_some(winsize.ws_col as usize)
}

#[cfg(windows)]
fn terminal_columns() -> Option<usize> {
    use std::mem::MaybeUninit;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo, GetStdHandle, STD_ERROR_HANDLE,
    };

    // SAFETY: GetStdHandle does not dereference any caller-provided pointer and is queried for
    // stderr only. Invalid handles are rejected below.
    let handle = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    if handle == null_mut() || handle == INVALID_HANDLE_VALUE {
        return None;
    }

    let mut info = MaybeUninit::<CONSOLE_SCREEN_BUFFER_INFO>::uninit();
    // SAFETY: handle was validated above and info points to writable storage. The value is only
    // read after GetConsoleScreenBufferInfo reports success.
    if unsafe { GetConsoleScreenBufferInfo(handle, info.as_mut_ptr()) } == 0 {
        return None;
    }

    // SAFETY: GetConsoleScreenBufferInfo returned success, so info is initialized.
    let info = unsafe { info.assume_init() };
    let width = info.srWindow.Right - info.srWindow.Left + 1;
    (width > 0).then_some(width as usize)
}

#[cfg(not(any(unix, windows)))]
fn terminal_columns() -> Option<usize> {
    None
}

fn truncate_text(text: &str, max_columns: usize) -> String {
    if display_width(text) <= max_columns {
        return text.to_string();
    }
    if max_columns <= 3 {
        return ".".repeat(max_columns);
    }

    let mut out = String::new();
    for ch in text.chars() {
        if display_width(&out) + 1 + 3 > max_columns {
            break;
        }
        out.push(ch);
    }
    out.push_str("...");
    out
}

fn truncate_detail(text: &str, max_columns: usize) -> String {
    if display_width(text) <= max_columns {
        return text.to_string();
    }
    if max_columns <= 3 {
        return ".".repeat(max_columns);
    }

    if let Some(tag_start) = text.rfind(" [") {
        let suffix = &text[tag_start..];
        let suffix_width = display_width(suffix);
        if suffix_width + 3 < max_columns {
            let prefix = take_prefix_columns(text, max_columns - suffix_width - 3);
            return format!("{prefix}...{suffix}");
        }
    }

    let prefix_columns = (max_columns - 3) / 2 + (max_columns - 3) % 2;
    let suffix_columns = (max_columns - 3) / 2;
    let prefix = take_prefix_columns(text, prefix_columns);
    let suffix = take_suffix_columns(text, suffix_columns);
    format!("{prefix}...{suffix}")
}

fn take_prefix_columns(text: &str, max_columns: usize) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if display_width(&out) + 1 > max_columns {
            break;
        }
        out.push(ch);
    }
    out
}

fn take_suffix_columns(text: &str, max_columns: usize) -> String {
    let mut out = String::new();
    for ch in text.chars().rev() {
        if display_width(&out) + 1 > max_columns {
            break;
        }
        out.insert(0, ch);
    }
    out
}

fn display_width(text: &str) -> usize {
    let mut width = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            let _ = chars.next();
            for code_ch in chars.by_ref() {
                if code_ch.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        width += 1;
    }
    width
}

#[cfg(test)]
mod tests {
    use super::{
        ProgressStyle, format_action_timing_detail, format_thinlto_link_summary,
        render_progress_line, truncate_detail,
    };
    use crate::execute::{
        ActionTiming, ActionTimingKind, ExecutionPhase, ExecutionProgressPlan,
        ExecutionProgressSnapshot, ExecutionSummary,
    };
    use kernc_driver::{CompileCacheStats, PhaseTiming};
    use std::time::Duration;

    #[test]
    fn action_timing_detail_starts_with_tags() {
        let detail = format_action_timing_detail(
            &[
                "pipeline=thinlto-final-link".to_string(),
                "inputs=6".to_string(),
            ],
            &[PhaseTiming {
                name: "link",
                duration: Duration::from_millis(12),
            }],
            CompileCacheStats::default(),
            None,
        );

        assert!(detail.starts_with("pipeline=thinlto-final-link, inputs=6; "));
        assert!(detail.contains("link 12.000ms"));
    }

    #[test]
    fn thinlto_link_summary_aggregates_link_actions() {
        let summary = ExecutionSummary {
            action_timings: vec![
                ActionTiming {
                    kind: ActionTimingKind::Link,
                    label: "a".to_string(),
                    detail_tags: vec![
                        "pipeline=thinlto-final-link".to_string(),
                        "inputs=6".to_string(),
                        "cross-package=true".to_string(),
                    ],
                    phase_timings: Vec::new(),
                    cache_stats: CompileCacheStats::default(),
                    codegen_plan: None,
                },
                ActionTiming {
                    kind: ActionTimingKind::Link,
                    label: "b".to_string(),
                    detail_tags: vec![
                        "pipeline=thinlto-final-link".to_string(),
                        "inputs=4".to_string(),
                        "cross-package=false".to_string(),
                    ],
                    phase_timings: Vec::new(),
                    cache_stats: CompileCacheStats::default(),
                    codegen_plan: None,
                },
            ],
            ..ExecutionSummary::default()
        };

        assert_eq!(
            format_thinlto_link_summary(&summary).as_deref(),
            Some("final-links 2, cross-package 1, total-inputs 10, max-inputs 6")
        );
    }

    #[test]
    fn progress_line_includes_phase_counts_and_eta() {
        let line = render_progress_line(
            "build",
            ExecutionProgressSnapshot {
                phase: ExecutionPhase::Compile,
                plan: ExecutionProgressPlan {
                    staged_actions: 2,
                    compile_actions: 4,
                    link_actions: 1,
                },
                staged_done: 2,
                compile_done: 1,
                link_done: 0,
                elapsed: Duration::from_secs(6),
                detail: "demo:bed [bin,target]".to_string(),
            },
            160,
            ProgressStyle {
                color_enabled: false,
            },
        );

        assert!(line.contains("build ["));
        assert!(line.contains("compile"));
        assert!(line.contains("compile 1/4"));
        assert!(line.contains("eta "));
        assert!(line.contains("demo:bed"));
    }

    #[test]
    fn progress_line_truncates_detail_to_terminal_width() {
        let line = render_progress_line(
            "check",
            ExecutionProgressSnapshot {
                phase: ExecutionPhase::Compile,
                plan: ExecutionProgressPlan {
                    staged_actions: 0,
                    compile_actions: 4,
                    link_actions: 0,
                },
                staged_done: 0,
                compile_done: 2,
                link_done: 0,
                elapsed: Duration::from_secs(5),
                detail: "json:hello_compact [example,target]".to_string(),
            },
            64,
            ProgressStyle {
                color_enabled: false,
            },
        );

        assert!(line.len() <= 64);
        assert!(line.contains("..."));
    }

    #[test]
    fn progress_line_uses_compact_layout_on_narrow_terminal() {
        let line = render_progress_line(
            "check",
            ExecutionProgressSnapshot {
                phase: ExecutionPhase::Compile,
                plan: ExecutionProgressPlan {
                    staged_actions: 0,
                    compile_actions: 4,
                    link_actions: 0,
                },
                staged_done: 0,
                compile_done: 2,
                link_done: 0,
                elapsed: Duration::from_secs(5),
                detail: "json:hello_compact [example,target]".to_string(),
            },
            80,
            ProgressStyle {
                color_enabled: false,
            },
        );

        assert!(line.len() <= 80);
        assert!(line.contains('>'));
        assert!(!line.contains("elapsed"));
        assert!(line.contains("eta 5s"));
    }

    #[test]
    fn truncate_detail_preserves_tag_suffix_when_space_allows() {
        let text = truncate_detail("json:hello_compact [example]", 24);

        assert!(text.contains("..."));
        assert!(text.ends_with(" [example]"));
    }
}
