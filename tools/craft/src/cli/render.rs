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
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

use super::{ColorChoice, UiOptions};

pub(super) struct Renderer {
    verbose: bool,
    timings: bool,
    color_enabled: bool,
    terminal_output: bool,
}

pub(super) struct ProgressDisplay {
    reporter: execute::ProgressReporter,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

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
        let terminal_output = std::io::stderr().is_terminal();
        let color_enabled = match ui.color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => terminal_output && std::env::var_os("NO_COLOR").is_none(),
        };

        Self {
            verbose: ui.verbose,
            timings: ui.timings,
            color_enabled,
            terminal_output,
        }
    }

    pub(super) fn header_with_path(
        &self,
        command: &str,
        manifest: &Manifest,
        manifest_path: &std::path::Path,
        feature_selection: &elaborate::FeatureSelection,
    ) {
        let marker = self.paint(Tone::Accent, "==>");
        let command = self.paint(Tone::Accent, command);
        println!("{marker} {command} {}", format_package_label(manifest));
        if self.verbose {
            self.meta("manifest", manifest_path.display());
            self.meta("features", format_feature_inputs(feature_selection));
        }
    }

    pub(super) fn meta(&self, label: &str, value: impl Display) {
        let label = self.paint(
            Tone::Muted,
            &format!("{label:<width$}", width = Self::LABEL_WIDTH),
        );
        println!("    {label} {value}");
    }

    pub(super) fn summary(&self, label: &str, value: impl Display) {
        let label = self.paint(
            Tone::Muted,
            &format!("{label:<width$}", width = Self::LABEL_WIDTH),
        );
        println!("    {label} {value}");
    }

    pub(super) fn section(&self, name: &str) {
        if !self.verbose {
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
        if !self.verbose {
            return;
        }
        let kind = self.paint(tone, &format!("{kind:<8}"));
        println!("  {kind} {subject} {detail}");
    }

    pub(super) fn ok(&self, message: impl Display) {
        println!("{} {message}", self.paint(Tone::Ok, "[ok]"));
    }

    pub(super) fn is_verbose(&self) -> bool {
        self.verbose
    }

    pub(super) fn progress(
        &self,
        command: &'static str,
        plan: execute::ExecutionProgressPlan,
    ) -> Option<ProgressDisplay> {
        if self.verbose || !self.terminal_output || plan.is_empty() {
            return None;
        }

        Some(ProgressDisplay::spawn(command, plan))
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

impl ProgressDisplay {
    fn spawn(command: &'static str, plan: execute::ExecutionProgressPlan) -> Self {
        let reporter = execute::ProgressReporter::new(plan);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker_reporter = reporter.clone();
        let worker = thread::spawn(move || {
            let mut last_len = 0usize;
            let mut last_line = String::new();
            loop {
                let snapshot = worker_reporter.snapshot();
                let line = render_progress_line(command, snapshot);
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

fn format_package_label(manifest: &Manifest) -> String {
    manifest
        .package
        .as_ref()
        .map(|package| format!("{} {}", package.name, package.version))
        .unwrap_or_else(|| "<workspace>".to_string())
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

    if !render.verbose {
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

fn render_progress_line(command: &str, snapshot: execute::ExecutionProgressSnapshot) -> String {
    let total_steps = snapshot.total_steps();
    let completed_steps = snapshot.completed_steps().min(total_steps);
    let percent = if total_steps == 0 {
        0
    } else {
        completed_steps.saturating_mul(100) / total_steps
    };
    let bar = render_progress_bar(completed_steps, total_steps, 24);
    let mut segments = Vec::new();
    segments.push(format!("phase {}", format_progress_phase(snapshot.phase)));
    if snapshot.plan.staged_actions > 0 {
        segments.push(format!(
            "gen {}/{}",
            snapshot.staged_done.min(snapshot.plan.staged_actions),
            snapshot.plan.staged_actions
        ));
    }
    if snapshot.plan.compile_actions > 0 {
        segments.push(format!(
            "compile {}/{}",
            snapshot.compile_done.min(snapshot.plan.compile_actions),
            snapshot.plan.compile_actions
        ));
    }
    if snapshot.plan.link_actions > 0 {
        segments.push(format!(
            "link {}/{}",
            snapshot.link_done.min(snapshot.plan.link_actions),
            snapshot.plan.link_actions
        ));
    }
    segments.push(format!(
        "elapsed {}",
        format_progress_clock(snapshot.elapsed)
    ));
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
    if !snapshot.detail.is_empty() {
        segments.push(snapshot.detail);
    }

    format!("{command} {bar} {percent:>3}%  {}", segments.join("  "))
}

fn render_progress_bar(completed: usize, total: usize, width: usize) -> String {
    if total == 0 {
        return format!("[{}]", "-".repeat(width));
    }

    let filled = completed.saturating_mul(width) / total;
    format!(
        "[{}{}]",
        "#".repeat(filled),
        "-".repeat(width.saturating_sub(filled))
    )
}

fn format_progress_phase(phase: execute::ExecutionPhase) -> &'static str {
    match phase {
        execute::ExecutionPhase::Bootstrap => "bootstrap",
        execute::ExecutionPhase::Stage => "generate",
        execute::ExecutionPhase::Compile => "compile",
        execute::ExecutionPhase::Link => "link",
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
    let padding = last_len.saturating_sub(line.len());
    let _ = write!(stderr, "\r{line}{}", " ".repeat(padding));
    let _ = stderr.flush();
    *last_len = line.len();
}

fn clear_progress_line(last_len: usize) {
    let mut stderr = std::io::stderr();
    let _ = write!(stderr, "\r{}\r", " ".repeat(last_len));
    let _ = stderr.flush();
}

#[cfg(test)]
mod tests {
    use super::{format_action_timing_detail, format_thinlto_link_summary, render_progress_line};
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
        );

        assert!(line.contains("build ["));
        assert!(line.contains("phase compile"));
        assert!(line.contains("gen 2/2"));
        assert!(line.contains("compile 1/4"));
        assert!(line.contains("link 0/1"));
        assert!(line.contains("eta "));
        assert!(line.contains("demo:bed [bin,target]"));
    }
}
