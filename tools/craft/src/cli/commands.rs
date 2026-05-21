//! Implementations for parsed Craft CLI commands.
//!
//! Command handlers load projects, acquire workspace locks, synchronize
//! lockfiles/analysis context, and delegate to build, fetch, format, publish,
//! install, or inspection subsystems.

use crate::analysis_context;
use crate::build_plan;
use crate::discover;
use crate::doc;
use crate::elaborate;
use crate::error::{Error, Result};
use crate::execute;
use crate::fmt;
use crate::graph;
use crate::local_state;
use crate::lockfile;
use crate::manifest::Manifest;
use crate::operation_lock::WorkspaceOperationLock;
use crate::plan::TargetKind;
use crate::source::{self, FetchProgress, FetchProgressKind, FetchProgressPhase};
use crate::style;
use crate::workspace;
use std::fs;
use std::path::{Path, PathBuf};

use super::policy::{
    publish_summary, summarize_check_sources, summarize_source_security,
    validate_check_source_policy, validate_publish_metadata, validate_publish_vcs,
};
use super::render::{
    PipelineProgressDisplay, Renderer, Tone, format_unit_label, format_yes_no,
    print_compile_actions, print_compile_actions_for_unit, print_fetched_package,
    print_fetched_resource, print_generated_files, print_generated_files_for_unit,
    print_link_actions, print_link_actions_for_unit, render_execution_timings,
};
use super::{Command, InstallSelection, RunSelection};

pub(super) fn run_command(command: Command) -> Result<()> {
    #[cfg(test)]
    let _test_slot = command_resource_slot(&command);

    match command {
        Command::Help { topic, color } => {
            print!("{}", super::help_text(&topic, color)?);
            Ok(())
        }
        Command::Version => {
            println!("{}", super::version_text());
            Ok(())
        }
        Command::Init { path, ui } => run_init(path, ui),
        Command::Clean { path, ui } => run_clean(path, ui),
        Command::Check {
            path,
            feature_selection,
            ui,
        } => run_check(path, feature_selection, ui),
        Command::Fetch {
            path,
            feature_selection,
            ui,
        } => run_fetch(path, feature_selection, ui),
        Command::Publish {
            path,
            feature_selection,
            ui,
        } => run_publish(path, feature_selection, ui),
        Command::Doc {
            path,
            feature_selection,
            ui,
        } => run_doc(path, feature_selection, ui),
        Command::Fmt { path, ui, check } => run_fmt(path, ui, check),
        Command::Style { path, ui } => run_style(path, ui),
        Command::Build {
            path,
            feature_selection,
            ui,
            include_examples,
        } => run_build(path, feature_selection, ui, include_examples),
        Command::Install {
            path,
            feature_selection,
            ui,
            selection,
            root,
        } => run_install(path, feature_selection, ui, selection, root),
        Command::Uninstall {
            path,
            ui,
            selection,
            root,
        } => run_uninstall(path, ui, selection, root),
        Command::Run {
            path,
            feature_selection,
            ui,
            selection,
            runtime_args,
        } => run_target(path, feature_selection, ui, selection, runtime_args),
        Command::Test {
            path,
            feature_selection,
            ui,
            test_name,
            runtime_args,
        } => run_tests(path, feature_selection, ui, test_name, runtime_args),
    }
}

#[cfg(test)]
fn command_resource_slot(command: &Command) -> Option<crate::test_support::TestCommandSlot> {
    match command {
        Command::Build { .. }
        | Command::Check { .. }
        | Command::Run { .. }
        | Command::Test { .. }
        | Command::Install { .. }
        | Command::Doc { .. } => Some(crate::test_support::acquire_command_slot()),
        _ => None,
    }
}

fn run_fmt(path: Option<PathBuf>, ui: super::UiOptions, check: bool) -> Result<()> {
    let render = Renderer::new(ui);
    let manifest_path = discover::resolve_project_manifest_path(path.as_deref())?;
    let selected_manifest_path = path
        .as_deref()
        .map(|path| discover::resolve_manifest_path(Some(path)))
        .transpose()?;
    let manifest = Manifest::load(&manifest_path)?;
    manifest.validate(&manifest_path)?;
    let workspace_members = workspace::load_members(&manifest_path, &manifest)?;
    let selected_manifest = if let Some(selected_manifest_path) = selected_manifest_path
        && selected_manifest_path != manifest_path
    {
        let selected_manifest = Manifest::load(&selected_manifest_path)?;
        selected_manifest.validate(&selected_manifest_path)?;
        Some((selected_manifest_path, selected_manifest))
    } else {
        None
    };
    let mode = if check {
        fmt::FormatMode::Check
    } else {
        fmt::FormatMode::Write
    };
    let summaries = if let Some((selected_manifest_path, selected_manifest)) = &selected_manifest {
        fmt::format_workspace_sources(selected_manifest_path, selected_manifest, &[], mode)?
    } else {
        fmt::format_workspace_sources(&manifest_path, &manifest, &workspace_members, mode)?
    };
    let mut total = fmt::FormatSummary::default();
    for summary in &summaries {
        total.merge(&summary.summary);
    }
    let feature_selection = elaborate::FeatureSelection::default();
    let (header_manifest_path, header_manifest) = selected_manifest
        .as_ref()
        .map(|(path, manifest)| (path, manifest))
        .unwrap_or((&manifest_path, &manifest));

    render.header_with_path(
        "fmt",
        header_manifest,
        header_manifest_path,
        &feature_selection,
    );
    render.summary(
        "sources",
        format!(
            "{} package(s), {} file(s), {} changed, {} diagnostic(s)",
            total.packages, total.files, total.changed_files, total.diagnostics
        ),
    );
    if render.is_verbose() && (total.changed_files > 0 || total.diagnostics > 0) {
        render.section("changes");
        for summary in &summaries {
            for path in &summary.changed_paths {
                render.action(
                    Tone::Generate,
                    if check { "check" } else { "format" },
                    &summary.label,
                    path.display(),
                );
            }
            for diagnostic in &summary.diagnostics {
                render.action(
                    Tone::Muted,
                    "diagnostic",
                    &summary.label,
                    format!(
                        "{}:{} ({} > {}): {}",
                        diagnostic.path.display(),
                        diagnostic.line,
                        diagnostic.width,
                        diagnostic.limit,
                        diagnostic.message
                    ),
                );
            }
        }
    }
    if check && (total.changed_files > 0 || total.diagnostics > 0) {
        return Err(Error::Usage(format!(
            "{} file(s) need formatting and {} diagnostic(s) remain; run `craft fmt{}` and split unresolved long lines",
            total.changed_files,
            total.diagnostics,
            path.as_ref()
                .map(|path| format!(" --project-path {}", path.display()))
                .unwrap_or_default()
        )));
    }
    render.ok(if check {
        "format check completed"
    } else {
        "format completed"
    });

    Ok(())
}

fn run_init(path: Option<PathBuf>, ui: super::UiOptions) -> Result<()> {
    let render = Renderer::new(ui);
    let root = resolve_init_root(path.as_deref())?;
    let _workspace_lock = WorkspaceOperationLock::acquire(&root, "init")?;
    let init = plan_init(&root)?;
    let created = apply_init_plan(&root, &init)?;
    let manifest_path = root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path)?;
    manifest.validate(&manifest_path)?;
    let feature_selection = elaborate::FeatureSelection::default();
    let workspace_members = workspace::load_members(&manifest_path, &manifest)?;
    let elaboration = elaborate::plan(
        &manifest_path,
        &manifest,
        &workspace_members,
        manifest.workspace.is_some(),
        crate::script::ScriptCommand::Check,
        &feature_selection,
    )?;
    let (_, lockfile_write_result) = lockfile::sync_lockfile(&manifest_path, &elaboration)?;

    render.header_with_path("init", &manifest, &manifest_path, &feature_selection);
    render.summary("root", root.display());
    render.summary("targets", init.target_summary());
    render.summary(
        "lockfile",
        match lockfile_write_result {
            lockfile::LockWriteResult::Created => "created",
            lockfile::LockWriteResult::Updated => "updated",
            lockfile::LockWriteResult::Unchanged => "current",
        },
    );
    render.summary("created", created.len());
    for path in &created {
        render.action(
            Tone::Generate,
            "create",
            display_path_from_root(&root, path),
            "",
        );
    }
    render.ok("package initialized");

    Ok(())
}

fn run_clean(path: Option<PathBuf>, ui: super::UiOptions) -> Result<()> {
    let render = Renderer::new(ui);
    let manifest_path = discover::resolve_project_manifest_path(path.as_deref())?;
    let manifest = Manifest::load(&manifest_path)?;
    manifest.validate(&manifest_path)?;
    let workspace_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let craft_dir = workspace_root.join(".craft");
    let feature_selection = elaborate::FeatureSelection::default();

    render.header_with_path("clean", &manifest, &manifest_path, &feature_selection);
    if !craft_dir.exists() {
        render.summary("removed", "0 entry(s)");
        render.ok("nothing to clean");
        return Ok(());
    }
    if !craft_dir.is_dir() {
        return Err(Error::Execution(format!(
            "cannot clean `{}` because it is not a directory",
            craft_dir.display()
        )));
    }

    let _workspace_lock = WorkspaceOperationLock::acquire(workspace_root, "clean")?;
    let removed = clean_craft_dir(&craft_dir)?;
    render.summary("root", craft_dir.display());
    render.summary("removed", format!("{removed} entry(s)"));
    render.ok("clean completed");

    Ok(())
}

fn run_check(
    path: Option<PathBuf>,
    feature_selection: elaborate::FeatureSelection,
    ui: super::UiOptions,
) -> Result<()> {
    let render = Renderer::new(ui);
    let (loaded, _workspace_lock) = load_package_graph(
        path.as_deref(),
        crate::script::ScriptCommand::Check,
        &feature_selection,
        "check",
        None,
    )?;
    render.header_with_path(
        "check",
        &loaded.manifest,
        &loaded.manifest_path,
        &feature_selection,
    );
    let mut pipeline = render.pipeline_progress("check", 10);
    pipeline_step(&pipeline, "plan", "derive check build graph");
    let build_plan = derive_build_plan_with_progress(
        &pipeline,
        &loaded.elaboration,
        crate::script::ScriptCommand::Check,
        build_plan::DeriveOptions::default(),
    )?;
    let build_plan = filter_selected_package(build_plan, loaded.selected_package_id.as_ref());
    pipeline_step(&pipeline, "plan", "derive execution actions");
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    finish_pipeline(&mut pipeline);

    let edge_count = loaded
        .elaboration
        .package_graph
        .packages
        .iter()
        .map(|pkg| pkg.dependencies.len())
        .sum::<usize>();
    let source_summary = summarize_check_sources(&loaded.elaboration.resolved_graph);
    let dependency_summary = format!(
        "normal {}, dev {}, build {}",
        loaded
            .elaboration
            .packages
            .iter()
            .map(|pkg| pkg.plan.dependency_count(graph::DependencyKind::Normal))
            .sum::<usize>(),
        loaded
            .elaboration
            .packages
            .iter()
            .map(|pkg| pkg.plan.dependency_count(graph::DependencyKind::Dev))
            .sum::<usize>(),
        loaded
            .elaboration
            .packages
            .iter()
            .map(|pkg| pkg.plan.dependency_count(graph::DependencyKind::Build))
            .sum::<usize>()
    );
    render.summary(
        "workspace",
        if let Some(workspace) = &loaded.manifest.workspace {
            format!(
                "{} declared member(s), {} validated",
                workspace.members.len(),
                loaded.workspace_members.len()
            )
        } else {
            "single package".to_string()
        },
    );
    render.summary(
        "graph",
        format!(
            "{} local package(s), {} external package(s), {} edge(s)",
            loaded.elaboration.resolved_graph.packages.len(),
            loaded.elaboration.resolved_graph.external_packages.len(),
            edge_count
        ),
    );
    render.summary(
        "targets",
        format!(
            "lib {}, bin {}, test {}, example {}, normalized {}",
            format_yes_no(loaded.manifest.lib.is_some()),
            loaded.manifest.bin.len(),
            loaded
                .elaboration
                .packages
                .iter()
                .flat_map(|package| &package.plan.targets)
                .filter(|target| target.kind == TargetKind::Test)
                .count(),
            loaded.manifest.example.len(),
            loaded.elaboration.package_target_count()
        ),
    );
    if render.is_verbose() {
        render.meta("deps", dependency_summary);
    }
    let security_summary = summarize_source_security(&loaded.manifest);
    validate_check_source_policy(&loaded.manifest_path, &feature_selection, &security_summary)?;
    render.summary(
        "sources",
        format!(
            "{} git package(s), {} path package(s)",
            source_summary.git_packages, source_summary.path_packages,
        ),
    );
    if render.is_verbose()
        || !security_summary.warnings.is_empty()
        || !security_summary.suppressed.is_empty()
    {
        render.meta(
            "source-policy",
            format!(
                "mode {}, warnings {}, suppressed {}, floating git {}, insecure transport {}",
                security_summary.policy_mode.as_str(),
                security_summary.warning_count(),
                security_summary.suppressed_count(),
                security_summary.floating_git_sources,
                security_summary.insecure_transport_sources
            ),
        );
    }
    if render.is_verbose() {
        render.meta(
            "scripts",
            format!("build.kn {}", build_plan.build_script_count()),
        );
    }
    render.summary(
        "lockfile",
        match loaded.lockfile_write_result {
            lockfile::LockWriteResult::Created => "created",
            lockfile::LockWriteResult::Updated => "updated",
            lockfile::LockWriteResult::Unchanged => "current",
        },
    );
    if render.is_verbose() && build_plan.generated_file_count() > 0 {
        render.section("generated");
    }
    print_generated_files(&render, &build_plan);
    let mut pipeline = render.pipeline_progress("check", 2);
    pipeline_step(
        &pipeline,
        "prepare",
        "materialize generated analysis inputs",
    );
    let mut prepare_progress =
        render.progress("check", staged_execution_progress_plan(&action_plan));
    let prepare = execute::materialize_analysis_inputs_with_progress(
        &build_plan,
        &action_plan,
        prepare_progress
            .as_ref()
            .map(|progress| progress.reporter()),
    );
    if let Some(progress) = prepare_progress.as_mut() {
        progress.finish();
    }
    prepare?;
    pipeline_step(&pipeline, "analysis", "sync analysis context");
    let _ = analysis_context::sync_analysis_context(
        &loaded.manifest_path,
        &loaded.elaboration,
        &build_plan,
        &feature_selection,
    );
    finish_pipeline(&mut pipeline);
    #[cfg(test)]
    crate::test_support::hit(crate::test_support::FAILPOINT_AFTER_ANALYSIS_CONTEXT_SYNC);
    let mut progress = render.progress("check", compile_execution_progress_plan(&action_plan));
    let execution = execute::check_with_progress_and_timings(
        &build_plan,
        &action_plan,
        progress.as_ref().map(|progress| progress.reporter()),
        ui.timings,
    );
    if let Some(progress) = progress.as_mut() {
        progress.finish();
    }
    let execution = execution?;
    render_execution_timings(&render, &execution);
    render.ok("check completed");

    Ok(())
}

fn run_fetch(
    path: Option<PathBuf>,
    feature_selection: elaborate::FeatureSelection,
    ui: super::UiOptions,
) -> Result<()> {
    let render = Renderer::new(ui);
    let (loaded, _workspace_lock) = load_package_graph(
        path.as_deref(),
        crate::script::ScriptCommand::Fetch,
        &feature_selection,
        "fetch",
        None,
    )?;
    render.header_with_path(
        "fetch",
        &loaded.manifest,
        &loaded.manifest_path,
        &feature_selection,
    );
    let mut pipeline = render.pipeline_progress("fetch", 10);
    pipeline_step(&pipeline, "fetch", "external packages");
    let fetched = source::fetch_external_packages_with_progress(
        &loaded.elaboration.resolved_graph,
        |event| pipeline_fetch_progress(&pipeline, event),
    )?;
    pipeline_step(&pipeline, "fetch", "package resources");
    let fetched_resources =
        source::fetch_package_resources_with_progress(&loaded.elaboration, |event| {
            pipeline_fetch_progress(&pipeline, event)
        })?;
    finish_pipeline(&mut pipeline);
    let summary = source::summarize_fetch(&fetched);
    let resource_summary = source::summarize_fetch_resources(&fetched_resources);

    render.summary(
        "packages",
        format!(
            "{} external package(s): created {}, updated {}, unchanged {}; {} resource(s): created {}, updated {}, unchanged {}",
            fetched.len(),
            summary.created,
            summary.updated,
            summary.unchanged,
            fetched_resources.len(),
            resource_summary.created,
            resource_summary.updated,
            resource_summary.unchanged
        ),
    );
    if render.is_verbose() && (!fetched.is_empty() || !fetched_resources.is_empty()) {
        render.section("actions");
    }
    for package in &fetched {
        print_fetched_package(&render, package);
    }
    for resource in &fetched_resources {
        print_fetched_resource(&render, resource);
    }
    render.ok("fetch completed");

    Ok(())
}

fn run_publish(
    path: Option<PathBuf>,
    feature_selection: elaborate::FeatureSelection,
    ui: super::UiOptions,
) -> Result<()> {
    let render = Renderer::new(ui);
    let mut pipeline = render.pipeline_progress("publish", 11);
    pipeline_step(&pipeline, "manifest", "resolve project manifest");
    let manifest_path = discover::resolve_project_manifest_path(path.as_deref())?;
    pipeline_step(&pipeline, "manifest", "load project manifest");
    let manifest = Manifest::load(&manifest_path)?;
    manifest.validate(&manifest_path)?;
    pipeline_step(&pipeline, "workspace", "load workspace members");
    let workspace_members = workspace::load_members(&manifest_path, &manifest)?;
    let workspace_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    pipeline_step(&pipeline, "lock", "wait workspace lock");
    let _workspace_lock = WorkspaceOperationLock::acquire(workspace_root, "publish")?;
    #[cfg(test)]
    crate::test_support::hit(crate::test_support::FAILPOINT_AFTER_WORKSPACE_LOCK);
    pipeline_step(&pipeline, "publish", "collect publish metadata");
    let summary = publish_summary(&manifest_path, &manifest, &workspace_members)?;
    validate_publish_metadata(&summary)?;
    pipeline_step(&pipeline, "vcs", "check repository state");
    let preflight_vcs_summary = validate_publish_vcs(
        &manifest_path,
        &manifest,
        &workspace_members,
        &summary,
        None,
    )?;
    let security_summary = summarize_source_security(&manifest);
    validate_check_source_policy(&manifest_path, &feature_selection, &security_summary)?;
    let lock_path = workspace_root.join("Craft.lock");
    if !lock_path.is_file() {
        return Err(Error::Validation {
            path: lock_path,
            message: "publish lockfile check failed: Craft.lock is missing; run `craft check` and commit Craft.lock before publishing"
                .to_string(),
        });
    }
    pipeline_step(&pipeline, "graph", "resolve package graph");
    let elaboration = elaborate::plan(
        &manifest_path,
        &manifest,
        &workspace_members,
        manifest.workspace.is_some(),
        crate::script::ScriptCommand::Check,
        &feature_selection,
    )?;
    pipeline_step(&pipeline, "lockfile", "verify lockfile");
    let (_, lockfile_write_result) =
        lockfile::check_lockfile_current(&manifest_path, &elaboration)?;
    if lockfile_write_result != lockfile::LockWriteResult::Unchanged {
        return Err(Error::Validation {
            path: lock_path,
            message: "publish lockfile check failed: Craft.lock is not current; run `craft check` and commit Craft.lock before publishing"
                .to_string(),
        });
    }
    pipeline_step(&pipeline, "vcs", "recheck repository state");
    validate_publish_vcs(
        &manifest_path,
        &manifest,
        &workspace_members,
        &summary,
        None,
    )?;
    pipeline_step(&pipeline, "format", "check source formatting");
    let format_summaries = fmt::format_workspace_sources(
        &manifest_path,
        &manifest,
        &workspace_members,
        fmt::FormatMode::Check,
    )?;
    let mut format_total = fmt::FormatSummary::default();
    for summary in &format_summaries {
        format_total.merge(&summary.summary);
    }
    if format_total.changed_files > 0 {
        return Err(Error::Validation {
            path: manifest_path.clone(),
            message: format!(
                "publish format check failed: {} file(s) need formatting; run `craft fmt --check`",
                format_total.changed_files
            ),
        });
    }
    if format_total.diagnostics > 0 {
        return Err(Error::Validation {
            path: manifest_path.clone(),
            message: format!(
                "publish format check failed: {} unresolved format diagnostic(s); run `craft fmt --check --verbose` and split unresolved long lines",
                format_total.diagnostics
            ),
        });
    }
    pipeline_step(&pipeline, "style", "collect source metrics");
    let style_summaries =
        style::collect_workspace_style_metrics(&manifest_path, &manifest, &workspace_members)?;
    let mut style_total = style::StyleSummary::default();
    let style_suggestion_count: usize = style_summaries
        .iter()
        .map(|summary| summary.suggestions.len())
        .sum();
    for summary in &style_summaries {
        style_total.merge(&summary.metrics);
    }
    let vcs_summary = preflight_vcs_summary;
    finish_pipeline(&mut pipeline);

    render.header_with_path("publish", &manifest, &manifest_path, &feature_selection);
    render.summary(
        "packages",
        format!(
            "{} publishable package(s), {} blocked package(s)",
            summary.ready.len(),
            summary.blocked.len()
        ),
    );
    render.summary(
        "source-policy",
        format!(
            "mode {}, warnings {}, suppressed {}, floating git {}, insecure transport {}",
            security_summary.policy_mode.as_str(),
            security_summary.warning_count(),
            security_summary.suppressed_count(),
            security_summary.floating_git_sources,
            security_summary.insecure_transport_sources
        ),
    );
    render.summary(
        "vcs",
        format!(
            "git {}, remotes {}, head {}",
            vcs_summary.repo_root.display(),
            vcs_summary.remote_count,
            vcs_summary.head.chars().take(12).collect::<String>()
        ),
    );
    render.summary(
        "format",
        format!(
            "{} package(s), {} file(s), {} changed",
            format_total.packages, format_total.files, format_total.changed_files
        ),
    );
    render.summary(
        "public-docs",
        format!(
            "{} documented, {} missing, coverage {:.1}%",
            style_total.documented_public_items,
            style_total.undocumented_public_items,
            style_total.public_doc_coverage()
        ),
    );
    render.summary(
        "style",
        format!("{style_suggestion_count} advisory source style suggestion(s)"),
    );
    render.summary(
        "lockfile",
        match lockfile_write_result {
            lockfile::LockWriteResult::Created => "created (release)",
            lockfile::LockWriteResult::Updated => "updated (release)",
            lockfile::LockWriteResult::Unchanged => "current (release)",
        },
    );
    if render.is_verbose() {
        for package in &summary.ready {
            render.meta(
                "package",
                format!(
                    "{} {} ({})",
                    package.name,
                    package.version,
                    package.manifest_path.display()
                ),
            );
        }
        if !format_summaries.is_empty() {
            render.section("format");
            for summary in &format_summaries {
                render.action(
                    Tone::Muted,
                    "check",
                    &summary.label,
                    format!(
                        "{} file(s), {} changed, {} diagnostic(s)",
                        summary.summary.files,
                        summary.summary.changed_files,
                        summary.summary.diagnostics
                    ),
                );
            }
        }
        if !style_summaries.is_empty() {
            render.section("style");
            for summary in &style_summaries {
                render.action(
                    Tone::Muted,
                    "metric",
                    &summary.label,
                    format!(
                        "{} file(s), public-docs {:.1}%, suggestions {}",
                        summary.metrics.files,
                        summary.metrics.public_doc_coverage(),
                        summary.suggestions.len()
                    ),
                );
            }
        }
    }
    render.ok("publish check completed");

    Ok(())
}

fn run_build(
    path: Option<PathBuf>,
    feature_selection: elaborate::FeatureSelection,
    ui: super::UiOptions,
    include_examples: bool,
) -> Result<()> {
    let render = Renderer::new(ui);
    let (loaded, _workspace_lock) = load_package_graph(
        path.as_deref(),
        crate::script::ScriptCommand::Build,
        &feature_selection,
        "build",
        None,
    )?;
    render.header_with_path(
        "build",
        &loaded.manifest,
        &loaded.manifest_path,
        &feature_selection,
    );
    let mut pipeline = render.pipeline_progress("build", 11);
    pipeline_step(&pipeline, "plan", "derive build graph");
    let build_plan = derive_build_plan_with_progress(
        &pipeline,
        &loaded.elaboration,
        crate::script::ScriptCommand::Build,
        build_plan::DeriveOptions { include_examples },
    )?;
    let build_plan = filter_selected_package(build_plan, loaded.selected_package_id.as_ref());
    pipeline_step(&pipeline, "analysis", "sync analysis context");
    let _ = analysis_context::sync_analysis_context(
        &loaded.manifest_path,
        &loaded.elaboration,
        &build_plan,
        &feature_selection,
    );
    pipeline_step(&pipeline, "plan", "derive execution actions");
    let build_plan = build_plan.filtered_target_kinds(&default_build_target_kinds());
    let target = crate::script::host_target();
    let action_plan = build_plan.derive_actions(&target);
    finish_pipeline(&mut pipeline);

    render.summary(
        "plan",
        format!(
            "{} unit(s), {} compile action(s), {} link action(s), {} generated file(s)",
            build_plan.unit_count(),
            action_plan.compile_count(),
            action_plan.link_count(),
            build_plan.generated_file_count()
        ),
    );
    if render.is_verbose() {
        render.meta(
            "deps",
            format!(
                "target local {}, target external {}, build local {}, build external {}",
                build_plan.local_dependency_edge_count(),
                build_plan.external_dependency_edge_count(),
                build_plan.build_local_dependency_edge_count(),
                build_plan.build_external_dependency_edge_count()
            ),
        );
    }
    if render.is_verbose()
        && (build_plan.generated_file_count() > 0
            || action_plan.compile_count() > 0
            || action_plan.link_count() > 0)
    {
        render.section("actions");
    }
    print_generated_files(&render, &build_plan);
    print_compile_actions(&render, &action_plan);
    print_link_actions(&render, &action_plan);
    let mut progress = render.progress("build", full_execution_progress_plan(&action_plan));
    let execution = execute::build_with_progress_and_timings(
        &build_plan,
        &action_plan,
        progress.as_ref().map(|progress| progress.reporter()),
        ui.timings,
    );
    if let Some(progress) = progress.as_mut() {
        progress.finish();
    }
    let execution = execution?;
    render_execution_timings(&render, &execution);
    render.ok(format!(
        "build completed (compile {}, link {})",
        execution.compile_actions, execution.link_actions
    ));

    Ok(())
}

fn run_install(
    path: Option<PathBuf>,
    feature_selection: elaborate::FeatureSelection,
    ui: super::UiOptions,
    selection: InstallSelection,
    root: Option<PathBuf>,
) -> Result<()> {
    let render = Renderer::new(ui);
    let (loaded, _workspace_lock) = load_package_graph(
        path.as_deref(),
        crate::script::ScriptCommand::Build,
        &feature_selection,
        "install",
        None,
    )?;
    render.header_with_path(
        "install",
        &loaded.manifest,
        &loaded.manifest_path,
        &feature_selection,
    );
    let mut pipeline = render.pipeline_progress("install", 12);
    let target_package_id = selected_target_package_id(&loaded, "install")?;
    pipeline_step(&pipeline, "plan", "derive install build graph");
    let build_plan = derive_build_plan_with_progress(
        &pipeline,
        &loaded.elaboration,
        crate::script::ScriptCommand::Build,
        build_plan::DeriveOptions::default(),
    )?;
    let build_plan = build_plan
        .filtered_package_closure(&[(graph::BuildDomain::Target, target_package_id.clone())]);
    pipeline_step(&pipeline, "analysis", "sync analysis context");
    let _ = analysis_context::sync_analysis_context(
        &loaded.manifest_path,
        &loaded.elaboration,
        &build_plan,
        &feature_selection,
    );
    pipeline_step(&pipeline, "plan", "derive execution actions");
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    pipeline_step(&pipeline, "plan", "select install targets");
    let install_units = select_install_units(&build_plan, &target_package_id, &selection)?;
    let install_root = resolve_install_root(root.as_deref())?;
    let install_bin_dir = install_root.join("bin");
    finish_pipeline(&mut pipeline);

    render.summary("root", install_root.display());
    render.summary(
        "targets",
        install_units
            .iter()
            .map(|unit| format_unit_label(unit))
            .collect::<Vec<_>>()
            .join(", "),
    );
    if render.is_verbose() {
        render.section("actions");
    }
    for unit in &install_units {
        print_generated_files_for_unit(&render, unit);
    }
    for unit in &install_units {
        print_compile_actions_for_unit(&render, &action_plan, unit);
    }
    for unit in &install_units {
        print_link_actions_for_unit(&render, &action_plan, unit);
    }

    let mut progress = render.progress("install", full_execution_progress_plan(&action_plan));
    let execution = execute::build_with_progress_and_timings(
        &build_plan,
        &action_plan,
        progress.as_ref().map(|progress| progress.reporter()),
        ui.timings,
    );
    if let Some(progress) = progress.as_mut() {
        progress.finish();
    }
    let execution = execution?;
    for unit in &install_units {
        let link_action = link_action_for_unit(&action_plan, unit)?;
        let installed_path = install_bin_dir.join(installed_file_name(link_action));
        crate::local_state::copy_file_atomic(&link_action.artifact_path, &installed_path)?;
        render.action(
            Tone::Generate,
            "install",
            format_unit_label(unit),
            format!("-> {}", installed_path.display()),
        );
    }
    render_execution_timings(&render, &execution);
    render.ok(format!(
        "installed {} binary target(s) into {}",
        install_units.len(),
        install_bin_dir.display()
    ));

    Ok(())
}

fn run_doc(
    path: Option<PathBuf>,
    feature_selection: elaborate::FeatureSelection,
    ui: super::UiOptions,
) -> Result<()> {
    let render = Renderer::new(ui);
    let (loaded, _workspace_lock) = load_package_graph(
        path.as_deref(),
        crate::script::ScriptCommand::Build,
        &feature_selection,
        "doc",
        None,
    )?;
    render.header_with_path(
        "doc",
        &loaded.manifest,
        &loaded.manifest_path,
        &feature_selection,
    );
    let mut pipeline = render.pipeline_progress("doc", 11);
    pipeline_step(&pipeline, "plan", "derive documentation build graph");
    let build_plan = derive_build_plan_with_progress(
        &pipeline,
        &loaded.elaboration,
        crate::script::ScriptCommand::Build,
        build_plan::DeriveOptions::default(),
    )?;
    let build_plan = filter_selected_package(build_plan, loaded.selected_package_id.as_ref());
    pipeline_step(&pipeline, "analysis", "sync analysis context");
    let _ = analysis_context::sync_analysis_context(
        &loaded.manifest_path,
        &loaded.elaboration,
        &build_plan,
        &feature_selection,
    );
    pipeline_step(&pipeline, "plan", "derive execution actions");
    let build_plan = build_plan.filtered_target_kinds(&default_build_target_kinds());
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    finish_pipeline(&mut pipeline);

    render.summary(
        "plan",
        format!(
            "{} unit(s), {} compile action(s), {} link action(s)",
            build_plan.unit_count(),
            action_plan.compile_count(),
            action_plan.link_count()
        ),
    );
    if render.is_verbose()
        && (build_plan.generated_file_count() > 0
            || action_plan.compile_count() > 0
            || action_plan.link_count() > 0)
    {
        render.section("actions");
    }
    print_generated_files(&render, &build_plan);
    print_compile_actions(&render, &action_plan);
    print_link_actions(&render, &action_plan);
    let mut progress = render.progress("doc", full_execution_progress_plan(&action_plan));
    let execution = execute::build_with_progress_and_timings(
        &build_plan,
        &action_plan,
        progress.as_ref().map(|progress| progress.reporter()),
        ui.timings,
    );
    if let Some(progress) = progress.as_mut() {
        progress.finish();
    }
    let execution = execution?;
    let docs = doc::sync_workspace_docs(&build_plan, &action_plan)?;
    let mut doc_quality = doc::DocQualitySummary::default();
    for doc in &docs {
        doc_quality.merge(&doc.quality);
    }
    render.summary(
        "docs",
        format!(
            "{} package(s), {} metadata item(s), output {}",
            docs.len(),
            docs.iter().map(|doc| doc.item_count).sum::<usize>(),
            build_plan.workspace_root.join(".craft/docs").display()
        ),
    );
    render.summary(
        "doc-quality",
        format!(
            "{} public item(s), {} documented, {} missing, coverage {:.1}%, warnings {}",
            doc_quality.public_items,
            doc_quality.documented_public_items,
            doc_quality.undocumented_public_items,
            doc_quality.coverage(),
            doc_quality.warning_count
        ),
    );
    if render.is_verbose() {
        for doc in &docs {
            render.action(
                Tone::Generate,
                "doc",
                &doc.package_label,
                format!("-> {}", doc.markdown_path.display()),
            );
        }
    }
    render_execution_timings(&render, &execution);
    render.ok(format!(
        "doc generation completed (compile {}, link {})",
        execution.compile_actions, execution.link_actions
    ));

    Ok(())
}

fn run_style(path: Option<PathBuf>, ui: super::UiOptions) -> Result<()> {
    let render = Renderer::new(ui);
    let mut pipeline = render.pipeline_progress("style", 6);
    pipeline_step(&pipeline, "manifest", "resolve project manifest");
    let manifest_path = discover::resolve_project_manifest_path(path.as_deref())?;
    pipeline_step(&pipeline, "manifest", "resolve selected manifest");
    let selected_manifest_path = path
        .as_deref()
        .map(|path| discover::resolve_manifest_path(Some(path)))
        .transpose()?;
    pipeline_step(&pipeline, "manifest", "load project manifest");
    let manifest = Manifest::load(&manifest_path)?;
    manifest.validate(&manifest_path)?;
    pipeline_step(&pipeline, "workspace", "load workspace members");
    let workspace_members = workspace::load_members(&manifest_path, &manifest)?;
    pipeline_step(&pipeline, "manifest", "load selected manifest");
    let selected_manifest = if let Some(selected_manifest_path) = selected_manifest_path
        && selected_manifest_path != manifest_path
    {
        let selected_manifest = Manifest::load(&selected_manifest_path)?;
        selected_manifest.validate(&selected_manifest_path)?;
        Some((selected_manifest_path, selected_manifest))
    } else {
        None
    };
    pipeline_step(&pipeline, "style", "collect source metrics");
    let summaries = if let Some((selected_manifest_path, selected_manifest)) = &selected_manifest {
        style::collect_workspace_style_metrics(selected_manifest_path, selected_manifest, &[])?
    } else {
        style::collect_workspace_style_metrics(&manifest_path, &manifest, &workspace_members)?
    };
    finish_pipeline(&mut pipeline);
    let mut total = style::StyleSummary::default();
    let suggestion_count: usize = summaries
        .iter()
        .map(|summary| summary.suggestions.len())
        .sum();
    for summary in &summaries {
        total.merge(&summary.metrics);
    }
    let feature_selection = elaborate::FeatureSelection::default();

    let (header_manifest_path, header_manifest) = selected_manifest
        .as_ref()
        .map(|(path, manifest)| (path, manifest))
        .unwrap_or((&manifest_path, &manifest));
    render.header_with_path(
        "style",
        header_manifest,
        header_manifest_path,
        &feature_selection,
    );
    render.summary(
        "sources",
        format!(
            "{} package(s), {} file(s), {} total line(s)",
            total.packages, total.files, total.total_lines
        ),
    );
    render.summary(
        "lines",
        format!(
            "code {}, blank {}, comments {}",
            total.code_lines,
            total.blank_lines,
            total.comment_lines()
        ),
    );
    render.summary(
        "comments",
        format!(
            "inline {}, block {}, doc {}, ratio {:.1}%, doc {:.1}%",
            total.inline_comment_lines,
            total.block_comment_lines,
            total.doc_comment_lines,
            total.comment_ratio(),
            total.doc_ratio()
        ),
    );
    render.summary(
        "public-docs",
        format!(
            "{} documented, {} missing, coverage {:.1}%",
            total.documented_public_items,
            total.undocumented_public_items,
            total.public_doc_coverage()
        ),
    );
    render.summary(
        "suggestions",
        format!("{suggestion_count} advisory source style suggestion(s)"),
    );
    if render.is_verbose() {
        render.section("packages");
        for summary in &summaries {
            render.action(
                Tone::Muted,
                "metric",
                &summary.label,
                format!(
                    "{} file(s), code {}, comments {}, public-docs {:.1}%",
                    summary.metrics.files,
                    summary.metrics.code_lines,
                    summary.metrics.comment_lines(),
                    summary.metrics.public_doc_coverage()
                ),
            );
        }
        if suggestion_count > 0 {
            render.section("suggestions");
            let mut explained_rules = Vec::new();
            for summary in &summaries {
                for suggestion in &summary.suggestions {
                    let subject = format!(
                        "{}:{}:{}",
                        summary.label,
                        suggestion.path.display(),
                        suggestion.line
                    );
                    let detail = format!(
                        "{} {}: {}",
                        suggestion.severity.label(),
                        suggestion.rule.code(),
                        suggestion.message
                    );
                    render.action(Tone::Muted, "style", &subject, detail);
                    if !explained_rules.contains(&suggestion.rule) {
                        explained_rules.push(suggestion.rule);
                    }
                }
            }
            render.section("rules");
            for rule in explained_rules {
                render.action(Tone::Muted, "rule", rule.code(), rule.intent());
                render.action(Tone::Muted, "fix", rule.code(), rule.handling());
            }
        }
    }
    render.ok("style metrics completed");

    Ok(())
}

fn run_uninstall(
    path: Option<PathBuf>,
    ui: super::UiOptions,
    selection: InstallSelection,
    root: Option<PathBuf>,
) -> Result<()> {
    let render = Renderer::new(ui);
    let feature_selection = elaborate::FeatureSelection::default();
    let (loaded, _workspace_lock) = load_package_graph(
        path.as_deref(),
        crate::script::ScriptCommand::Build,
        &feature_selection,
        "uninstall",
        None,
    )?;
    render.header_with_path(
        "uninstall",
        &loaded.manifest,
        &loaded.manifest_path,
        &feature_selection,
    );
    let mut pipeline = render.pipeline_progress("uninstall", 11);
    let target_package_id = selected_target_package_id(&loaded, "uninstall")?;
    pipeline_step(&pipeline, "plan", "derive uninstall build graph");
    let build_plan = derive_build_plan_with_progress(
        &pipeline,
        &loaded.elaboration,
        crate::script::ScriptCommand::Build,
        build_plan::DeriveOptions::default(),
    )?;
    let build_plan = build_plan
        .filtered_package_closure(&[(graph::BuildDomain::Target, target_package_id.clone())]);
    pipeline_step(&pipeline, "plan", "derive execution actions");
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    pipeline_step(&pipeline, "plan", "select uninstall targets");
    let uninstall_units = select_install_units(&build_plan, &target_package_id, &selection)?;
    let install_root = resolve_install_root(root.as_deref())?;
    let install_bin_dir = install_root.join("bin");
    let mut removed = 0usize;
    let mut missing = 0usize;
    finish_pipeline(&mut pipeline);

    render.summary("root", install_root.display());
    render.summary(
        "targets",
        uninstall_units
            .iter()
            .map(|unit| format_unit_label(unit))
            .collect::<Vec<_>>()
            .join(", "),
    );

    for unit in &uninstall_units {
        let link_action = link_action_for_unit(&action_plan, unit)?;
        let installed_path = install_bin_dir.join(installed_file_name(link_action));
        if installed_path.exists() {
            fs::remove_file(&installed_path).map_err(|err| Error::from_io(&installed_path, err))?;
            removed += 1;
            render.action(
                Tone::Generate,
                "remove",
                format_unit_label(unit),
                format!("-> {}", installed_path.display()),
            );
        } else {
            missing += 1;
            render.action(
                Tone::Muted,
                "skip",
                format_unit_label(unit),
                format!("missing {}", installed_path.display()),
            );
        }
    }

    render.summary("removed", removed);
    if missing > 0 {
        render.summary("missing", missing);
    }
    render.ok(format!(
        "uninstall completed ({removed} removed, {missing} missing)"
    ));

    Ok(())
}

fn run_target(
    path: Option<PathBuf>,
    feature_selection: elaborate::FeatureSelection,
    ui: super::UiOptions,
    selection: RunSelection,
    runtime_args: Vec<String>,
) -> Result<()> {
    let render = Renderer::new(ui);
    let (loaded, _workspace_lock) = load_package_graph(
        path.as_deref(),
        crate::script::ScriptCommand::Run,
        &feature_selection,
        "run",
        None,
    )?;
    render.header_with_path(
        "run",
        &loaded.manifest,
        &loaded.manifest_path,
        &feature_selection,
    );
    let mut pipeline = render.pipeline_progress("run", 12);
    pipeline_step(&pipeline, "plan", "derive run build graph");
    let build_plan = derive_build_plan_with_progress(
        &pipeline,
        &loaded.elaboration,
        crate::script::ScriptCommand::Run,
        build_plan::DeriveOptions {
            include_examples: matches!(selection, RunSelection::Example(_)),
        },
    )?;
    let build_plan = filter_selected_package(build_plan, loaded.selected_package_id.as_ref());
    pipeline_step(&pipeline, "analysis", "sync analysis context");
    let _ = analysis_context::sync_analysis_context(
        &loaded.manifest_path,
        &loaded.elaboration,
        &build_plan,
        &feature_selection,
    );
    pipeline_step(&pipeline, "plan", "derive execution actions");
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    pipeline_step(&pipeline, "plan", "select run target");
    let run_unit = select_run_unit(&build_plan, &selection)?;
    finish_pipeline(&mut pipeline);

    render.summary("target", format_unit_label(run_unit));
    render.summary(
        "plan",
        format!(
            "{} unit(s), {} compile action(s), {} link action(s), {} generated file(s)",
            build_plan.unit_count(),
            action_plan.compile_count(),
            action_plan.link_count(),
            build_plan.generated_file_count()
        ),
    );
    if render.is_verbose()
        && (!run_unit.generated_files.is_empty()
            || action_plan.compile_count() > 0
            || action_plan.link_count() > 0)
    {
        render.section("actions");
    }
    print_generated_files_for_unit(&render, run_unit);
    print_compile_actions_for_unit(&render, &action_plan, run_unit);
    print_link_actions_for_unit(&render, &action_plan, run_unit);
    let mut progress = render.progress("run", full_execution_progress_plan(&action_plan));
    let build = execute::build_with_progress_and_timings(
        &build_plan,
        &action_plan,
        progress.as_ref().map(|progress| progress.reporter()),
        ui.timings,
    );
    if let Some(progress) = progress.as_mut() {
        progress.finish();
    }
    let execution = execute::run_built(&build_plan, &action_plan, run_unit, build?, &runtime_args)?;
    render_execution_timings(&render, &execution.build);
    render.ok(format!(
        "run completed ({})",
        execution.executable.display()
    ));

    Ok(())
}

fn run_tests(
    path: Option<PathBuf>,
    feature_selection: elaborate::FeatureSelection,
    ui: super::UiOptions,
    test_name: Option<String>,
    runtime_args: Vec<String>,
) -> Result<()> {
    let render = Renderer::new(ui);
    let (loaded, _workspace_lock) = load_package_graph(
        path.as_deref(),
        crate::script::ScriptCommand::Test,
        &feature_selection,
        "test",
        None,
    )?;
    render.header_with_path(
        "test",
        &loaded.manifest,
        &loaded.manifest_path,
        &feature_selection,
    );
    let mut pipeline = render.pipeline_progress("test", 12);
    pipeline_step(&pipeline, "plan", "derive test build graph");
    let build_plan = derive_build_plan_with_progress(
        &pipeline,
        &loaded.elaboration,
        crate::script::ScriptCommand::Test,
        build_plan::DeriveOptions::default(),
    )?;
    let build_plan = filter_selected_package(build_plan, loaded.selected_package_id.as_ref());
    pipeline_step(&pipeline, "plan", "filter selected tests");
    let build_plan = filter_selected_tests(build_plan, test_name.as_deref())?;
    pipeline_step(&pipeline, "analysis", "sync analysis context");
    let _ = analysis_context::sync_analysis_context(
        &loaded.manifest_path,
        &loaded.elaboration,
        &build_plan,
        &feature_selection,
    );
    pipeline_step(&pipeline, "plan", "derive execution actions");
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let tests = units_of_kind(&build_plan, TargetKind::Test);
    finish_pipeline(&mut pipeline);

    render.summary(
        "tests",
        format!(
            "{} target(s), {} compile action(s), {} link action(s)",
            tests.len(),
            action_plan.compile_count(),
            action_plan.link_count()
        ),
    );
    if render.is_verbose() && !tests.is_empty() {
        render.meta(
            "targets",
            tests
                .iter()
                .map(|unit| format_unit_label(unit))
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if render.is_verbose()
        && (!tests.is_empty() || action_plan.compile_count() > 0 || action_plan.link_count() > 0)
    {
        render.section("actions");
    }
    for unit in &tests {
        print_generated_files_for_unit(&render, unit);
    }
    for unit in &tests {
        print_compile_actions_for_unit(&render, &action_plan, unit);
    }
    for unit in &tests {
        print_link_actions_for_unit(&render, &action_plan, unit);
    }
    let mut progress = render.progress("test", full_execution_progress_plan(&action_plan));
    let build = execute::build_with_progress_and_timings(
        &build_plan,
        &action_plan,
        progress.as_ref().map(|progress| progress.reporter()),
        ui.timings,
    );
    if let Some(progress) = progress.as_mut() {
        progress.finish();
    }
    let execution = execute::test_built(&build_plan, &action_plan, &tests, build?, &runtime_args)?;
    render_execution_timings(&render, &execution.build);
    if !execution.failures.is_empty() {
        let mut message = format!(
            "{} of {} test target(s) failed",
            execution.failures.len(),
            execution.executed
        );
        for failure in &execution.failures {
            message.push_str("\n  ");
            message.push_str(&failure.label);
            message.push_str(" exited with status ");
            message.push_str(&failure.status.to_string());
        }
        return Err(Error::Execution(message));
    }
    render.ok(format!(
        "test run completed ({} executed)",
        execution.executed
    ));

    Ok(())
}

fn filter_selected_tests(
    build_plan: build_plan::BuildPlan,
    name: Option<&str>,
) -> Result<build_plan::BuildPlan> {
    let Some(name) = name else {
        return Ok(build_plan);
    };

    let matches = build_plan
        .packages
        .iter()
        .flat_map(|package| &package.units)
        .filter(|unit| unit.target_kind == TargetKind::Test)
        .filter(|unit| unit.target_name.as_deref() == Some(name))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(Error::Usage(format!(
            "`craft test` could not find test target `{name}`"
        ))),
        [_] => {
            let mut build_plan = build_plan;
            for package in &mut build_plan.packages {
                package.units.retain(|unit| {
                    unit.target_kind != TargetKind::Test
                        || unit.target_name.as_deref() == Some(name)
                });
            }
            Ok(build_plan)
        }
        units => {
            let candidates = units
                .iter()
                .map(|unit| format_unit_label(unit))
                .collect::<Vec<_>>()
                .join(", ");
            Err(Error::Usage(format!(
                "`craft test` found multiple test targets named `{name}`: {candidates}"
            )))
        }
    }
}

fn staged_execution_progress_plan(
    action_plan: &build_plan::ActionPlan,
) -> execute::ExecutionProgressPlan {
    execute::ExecutionProgressPlan {
        staged_actions: action_plan.build_nodes.len(),
        compile_actions: 0,
        link_actions: 0,
    }
}

fn compile_execution_progress_plan(
    action_plan: &build_plan::ActionPlan,
) -> execute::ExecutionProgressPlan {
    execute::ExecutionProgressPlan {
        staged_actions: 0,
        compile_actions: action_plan.compile_count(),
        link_actions: 0,
    }
}

fn full_execution_progress_plan(
    action_plan: &build_plan::ActionPlan,
) -> execute::ExecutionProgressPlan {
    execute::ExecutionProgressPlan {
        staged_actions: action_plan.build_nodes.len(),
        compile_actions: action_plan.compile_count(),
        link_actions: action_plan.link_count(),
    }
}

fn pipeline_step(
    progress: &Option<PipelineProgressDisplay>,
    label: impl Into<String>,
    detail: impl Into<String>,
) {
    if let Some(progress) = progress {
        progress.step(label, detail);
    }
}

fn pipeline_fetch_progress(progress: &Option<PipelineProgressDisplay>, event: FetchProgress) {
    let kind = match event.kind {
        FetchProgressKind::Package => "package",
        FetchProgressKind::Resource => "resource",
    };
    let phase = match event.phase {
        FetchProgressPhase::Resolve => "resolve",
        FetchProgressPhase::Git => "git",
        FetchProgressPhase::Materialize => "sync",
    };
    pipeline_step(
        progress,
        "fetch",
        format!("{phase} {kind} {} ({})", event.name, event.source),
    );
}

fn derive_build_plan_with_progress(
    progress: &Option<PipelineProgressDisplay>,
    elaboration: &elaborate::ElaborationPlan,
    command: crate::script::ScriptCommand,
    options: build_plan::DeriveOptions,
) -> Result<build_plan::BuildPlan> {
    build_plan::derive_with_options_and_progress(elaboration, command, options, |event| {
        pipeline_fetch_progress(progress, event);
    })
}

fn finish_pipeline(progress: &mut Option<PipelineProgressDisplay>) {
    if let Some(progress) = progress.as_mut() {
        progress.finish();
    }
}

#[derive(Debug, Clone)]
struct InitPlan {
    package_name: String,
    lib_root: Option<String>,
    bin_root: Option<String>,
    test_roots: Vec<String>,
    example_roots: Vec<String>,
    create_main_stub: bool,
}

impl InitPlan {
    fn target_summary(&self) -> String {
        format!(
            "lib {}, bin {}, test {}, example {}",
            format_yes_no(self.lib_root.is_some()),
            format_yes_no(self.bin_root.is_some()),
            self.test_roots.len(),
            self.example_roots.len()
        )
    }
}

fn clean_craft_dir(craft_dir: &Path) -> Result<usize> {
    let mut removed = 0;
    for entry in fs::read_dir(craft_dir).map_err(|err| Error::from_io(craft_dir, err))? {
        let entry = entry.map_err(Error::from_io_plain)?;
        let path = entry.path();
        if entry.file_name() == "lock" {
            continue;
        }

        let file_type = entry.file_type().map_err(Error::from_io_plain)?;
        if file_type.is_dir() {
            fs::remove_dir_all(&path).map_err(|err| Error::from_io(&path, err))?;
        } else {
            fs::remove_file(&path).map_err(|err| Error::from_io(&path, err))?;
        }
        removed += 1;
    }
    Ok(removed)
}

fn resolve_init_root(path: Option<&Path>) -> Result<PathBuf> {
    let root = match path {
        Some(path) if path.file_name().and_then(|name| name.to_str()) == Some("Craft.toml") => path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
        Some(path) if path.is_dir() => path.to_path_buf(),
        Some(path) if path.exists() => {
            return Err(Error::Usage(format!(
                "`craft init` requires a directory or `Craft.toml` path, found `{}`",
                path.display()
            )));
        }
        Some(path) => {
            return Err(Error::Usage(format!(
                "path `{}` does not exist; create the directory first, then run `craft init`",
                path.display()
            )));
        }
        None => std::env::current_dir().map_err(Error::from_io_plain)?,
    };

    fs::canonicalize(&root).map_err(|err| Error::from_io(&root, err))
}

fn plan_init(root: &Path) -> Result<InitPlan> {
    let manifest_path = root.join("Craft.toml");
    if manifest_path.exists() {
        match Manifest::load(&manifest_path).and_then(|manifest| manifest.validate(&manifest_path))
        {
            Ok(()) => {
                return Err(Error::Usage(format!(
                    "directory `{}` already contains `Craft.toml`",
                    root.display()
                )));
            }
            Err(err) => {
                return Err(Error::Usage(format!(
                    "directory `{}` already contains a broken `Craft.toml`; repair or remove it before `craft init` ({err})",
                    root.display()
                )));
            }
        }
    }

    let lib_root = root
        .join("src/lib.kn")
        .is_file()
        .then(|| "src/lib.kn".to_string());
    let mut bin_root = root
        .join("src/main.kn")
        .is_file()
        .then(|| "src/main.kn".to_string());
    let create_main_stub = lib_root.is_none() && bin_root.is_none();
    if create_main_stub {
        bin_root = Some("src/main.kn".to_string());
    }

    Ok(InitPlan {
        package_name: infer_package_name(root),
        lib_root,
        bin_root,
        test_roots: collect_kern_roots(root, "tests")?,
        example_roots: collect_kern_roots(root, "examples")?,
        create_main_stub,
    })
}

fn apply_init_plan(root: &Path, init: &InitPlan) -> Result<Vec<PathBuf>> {
    let mut created = Vec::new();
    let manifest_path = root.join("Craft.toml");
    write_if_missing(&manifest_path, render_init_manifest(init), &mut created)?;

    if init.create_main_stub {
        write_if_missing(
            &root.join("src/main.kn"),
            "use std.io;\n\nfn main() i32 {\n    \"Hello, Kern!\".println();\n    return 0;\n}\n",
            &mut created,
        )?;
    }

    if local_state::ensure_workspace_gitignore_entry(root)? {
        created.push(root.join(".gitignore"));
    }

    Ok(created)
}

fn write_if_missing(
    path: &Path,
    contents: impl AsRef<[u8]>,
    created: &mut Vec<PathBuf>,
) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| Error::from_io(parent, err))?;
    }
    fs::write(path, contents).map_err(|err| Error::from_io(path, err))?;
    created.push(path.to_path_buf());
    Ok(())
}

fn render_init_manifest(init: &InitPlan) -> String {
    let mut out = format!(
        "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nkern = \"{}\"\n",
        init.package_name,
        crate::manifest::default_kern_compat_version()
    );

    if let Some(root) = &init.lib_root {
        out.push_str(&format!("\n[lib]\nroot = \"{root}\"\n"));
    }
    if let Some(root) = &init.bin_root {
        out.push_str(&format!(
            "\n[[bin]]\nname = \"{}\"\nroot = \"{root}\"\n",
            init.package_name
        ));
    }
    if !init.example_roots.is_empty() {
        out.push_str("\n[example]\nroots = [\n");
        for root in &init.example_roots {
            out.push_str(&format!("    \"{root}\",\n"));
        }
        out.push_str("]\n");
    }
    if !init.test_roots.is_empty() {
        out.push_str("\n[test]\nroots = [\n");
        for root in &init.test_roots {
            out.push_str(&format!("    \"{root}\",\n"));
        }
        out.push_str("]\n");
    }

    out
}

fn infer_package_name(root: &Path) -> String {
    let raw = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("app");
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "app".to_string()
    } else {
        trimmed.to_string()
    }
}

fn collect_kern_roots(root: &Path, dir_name: &str) -> Result<Vec<String>> {
    let dir = root.join(dir_name);
    let mut found = Vec::new();
    collect_kern_roots_recursive(root, &dir, &mut found)?;
    found.sort();
    Ok(found)
}

fn collect_kern_roots_recursive(root: &Path, dir: &Path, found: &mut Vec<String>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    if !dir.is_dir() {
        return Err(Error::Usage(format!(
            "`{}` must be a directory when present",
            dir.display()
        )));
    }

    let mut entries = fs::read_dir(dir)
        .map_err(|err| Error::from_io(dir, err))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Error::from_io_plain)?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_kern_roots_recursive(root, &path, found)?;
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("kn") {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| {
                Error::Execution(format!(
                    "failed to compute relative init path for `{}`",
                    path.display()
                ))
            })?
            .to_string_lossy()
            .replace('\\', "/");
        found.push(relative);
    }

    Ok(())
}

fn select_run_unit<'a>(
    build_plan: &'a build_plan::BuildPlan,
    selection: &RunSelection,
) -> Result<&'a build_plan::BuildUnit> {
    match selection {
        RunSelection::DefaultBin => select_unique_run_unit(build_plan, TargetKind::Bin, None),
        RunSelection::Bin(name) => select_unique_run_unit(build_plan, TargetKind::Bin, Some(name)),
        RunSelection::Example(name) => {
            select_unique_run_unit(build_plan, TargetKind::Example, Some(name))
        }
    }
}

fn display_path_from_root(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.display().to_string())
}

fn select_unique_run_unit<'a>(
    build_plan: &'a build_plan::BuildPlan,
    kind: TargetKind,
    name: Option<&str>,
) -> Result<&'a build_plan::BuildUnit> {
    let runnable = build_plan
        .packages
        .iter()
        .flat_map(|package| &package.units)
        .filter(|unit| unit.target_kind == kind)
        .filter(|unit| name.is_none_or(|name| unit.target_name.as_deref() == Some(name)))
        .collect::<Vec<_>>();
    let kind_label = kind.as_str();

    match runnable.as_slice() {
        [] => {
            if let Some(name) = name {
                Err(Error::Usage(format!(
                    "`craft run` could not find {kind_label} target `{name}`"
                )))
            } else {
                Err(Error::Usage(format!(
                    "`craft run` requires exactly one runnable `{kind_label}` target, but none were found"
                )))
            }
        }
        [unit] => Ok(*unit),
        units => {
            let candidates = units
                .iter()
                .map(|unit| format_unit_label(unit))
                .collect::<Vec<_>>()
                .join(", ");
            if let Some(name) = name {
                Err(Error::Usage(format!(
                    "`craft run` found multiple runnable `{kind_label}` targets named `{name}`: {candidates}"
                )))
            } else {
                Err(Error::Usage(format!(
                    "`craft run` requires exactly one runnable `{kind_label}` target, but found {}: {}",
                    units.len(),
                    candidates
                )))
            }
        }
    }
}

fn select_install_units<'a>(
    build_plan: &'a build_plan::BuildPlan,
    package_id: &graph::PackageId,
    selection: &InstallSelection,
) -> Result<Vec<&'a build_plan::BuildUnit>> {
    let bins = build_plan
        .packages
        .iter()
        .flat_map(|package| &package.units)
        .filter(|unit| unit.package_id == *package_id && unit.target_kind == TargetKind::Bin)
        .collect::<Vec<_>>();

    match selection {
        InstallSelection::AllBins => {
            if bins.is_empty() {
                return Err(Error::Usage(
                    "the selected package does not declare any `bin` targets".to_string(),
                ));
            }
            Ok(bins)
        }
        InstallSelection::Bin(name) => bins
            .into_iter()
            .find(|unit| unit.target_name.as_deref() == Some(name.as_str()))
            .map(|unit| vec![unit])
            .ok_or_else(|| {
                Error::Usage(format!(
                    "could not find `bin` target `{name}` in the selected package"
                ))
            }),
    }
}

fn selected_target_package_id(
    loaded: &LoadedPackageGraph,
    command: &str,
) -> Result<graph::PackageId> {
    if let Some(package_id) = &loaded.selected_package_id {
        return Ok(package_id.clone());
    }
    if loaded.manifest.package.is_none() {
        return Err(Error::Usage(format!(
            "`craft {command}` requires a package selection; pass `--project-path` to a workspace member"
        )));
    }
    loaded
        .elaboration
        .packages
        .iter()
        .find(|package| package.plan.manifest_path == loaded.manifest_path)
        .map(|package| package.package_id.clone())
        .ok_or_else(|| {
            Error::Execution(format!(
                "failed to resolve selected package for `{}`",
                loaded.manifest_path.display()
            ))
        })
}

fn link_action_for_unit<'a>(
    action_plan: &'a build_plan::ActionPlan,
    unit: &build_plan::BuildUnit,
) -> Result<&'a build_plan::LinkAction> {
    action_plan
        .link_actions
        .iter()
        .find(|action| {
            action.domain == unit.domain
                && action.package_id == unit.package_id
                && action.target_kind == unit.target_kind
                && action.target_name == unit.target_name
                && action.artifact_name == unit.artifact_name
        })
        .ok_or_else(|| {
            Error::Execution(format!(
                "missing link action for target `{}`",
                format_unit_label(unit)
            ))
        })
}

fn resolve_install_root(explicit_root: Option<&Path>) -> Result<PathBuf> {
    let root = match explicit_root {
        Some(root) => root.to_path_buf(),
        None => default_install_root()?,
    };
    let canonical = if root.exists() {
        fs::canonicalize(&root).map_err(|err| Error::from_io(&root, err))?
    } else {
        root
    };
    Ok(canonical)
}

fn default_install_root() -> Result<PathBuf> {
    if let Some(kern_home) = std::env::var_os("KERN_HOME") {
        return Ok(PathBuf::from(kern_home));
    }
    if cfg!(windows)
        && let Some(user_profile) = std::env::var_os("USERPROFILE")
    {
        return Ok(PathBuf::from(user_profile).join(".kern"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home).join(".kern"));
    }
    Err(Error::Execution(
        "could not determine install root; pass `--root <PATH>` or set `KERN_HOME`".to_string(),
    ))
}

fn installed_file_name(action: &build_plan::LinkAction) -> &std::ffi::OsStr {
    action
        .artifact_path
        .file_name()
        .expect("link actions always produce a file name")
}

struct LoadedPackageGraph {
    manifest_path: PathBuf,
    manifest: Manifest,
    workspace_members: Vec<workspace::WorkspaceMember>,
    elaboration: elaborate::ElaborationPlan,
    selected_package_id: Option<graph::PackageId>,
    lockfile_write_result: lockfile::LockWriteResult,
}

fn load_package_graph(
    path: Option<&Path>,
    command: crate::script::ScriptCommand,
    feature_selection: &elaborate::FeatureSelection,
    operation: &str,
    progress: Option<&PipelineProgressDisplay>,
) -> Result<(LoadedPackageGraph, WorkspaceOperationLock)> {
    pipeline_step_ref(progress, "manifest", "resolve selected manifest");
    let selected_manifest_path = path
        .map(|path| discover::resolve_manifest_path(Some(path)))
        .transpose()?;
    pipeline_step_ref(progress, "manifest", "resolve project manifest");
    let manifest_path = discover::resolve_project_manifest_path(path)?;
    pipeline_detail_ref(progress, format!("load {}", manifest_path.display()));
    let manifest = Manifest::load(&manifest_path)?;
    manifest.validate(&manifest_path)?;
    pipeline_step_ref(progress, "workspace", "load workspace members");
    let workspace_members = workspace::load_members(&manifest_path, &manifest)?;
    let workspace_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    pipeline_step_ref(progress, "lock", "wait workspace lock");
    let workspace_lock = WorkspaceOperationLock::acquire(workspace_root, operation)?;
    #[cfg(test)]
    crate::test_support::hit(crate::test_support::FAILPOINT_AFTER_WORKSPACE_LOCK);
    pipeline_step_ref(progress, "graph", "resolve package graph");
    let elaboration = elaborate::plan(
        &manifest_path,
        &manifest,
        &workspace_members,
        manifest.workspace.is_some(),
        command,
        feature_selection,
    )?;
    pipeline_step_ref(progress, "lockfile", "sync lockfile");
    let (_, lockfile_write_result) = lockfile::sync_lockfile(&manifest_path, &elaboration)?;
    pipeline_step_ref(progress, "package", "select requested package");
    let selected_package_id = selected_manifest_path
        .as_ref()
        .filter(|selected| **selected != manifest_path)
        .and_then(|selected| {
            elaboration
                .packages
                .iter()
                .find(|package| package.plan.manifest_path == *selected)
                .map(|package| package.package_id.clone())
        });

    Ok((
        LoadedPackageGraph {
            manifest_path,
            manifest,
            workspace_members,
            elaboration,
            selected_package_id,
            lockfile_write_result,
        },
        workspace_lock,
    ))
}

fn pipeline_step_ref(
    progress: Option<&PipelineProgressDisplay>,
    label: impl Into<String>,
    detail: impl Into<String>,
) {
    if let Some(progress) = progress {
        progress.step(label, detail);
    }
}

fn pipeline_detail_ref(progress: Option<&PipelineProgressDisplay>, detail: impl Into<String>) {
    if let Some(progress) = progress {
        progress.detail(detail);
    }
}

fn filter_selected_package(
    build_plan: build_plan::BuildPlan,
    selected_package_id: Option<&graph::PackageId>,
) -> build_plan::BuildPlan {
    let Some(selected_package_id) = selected_package_id else {
        return build_plan;
    };
    build_plan
        .filtered_package_closure(&[(graph::BuildDomain::Target, selected_package_id.clone())])
}

fn units_of_kind(plan: &build_plan::BuildPlan, kind: TargetKind) -> Vec<&build_plan::BuildUnit> {
    plan.packages
        .iter()
        .flat_map(|package| &package.units)
        .filter(|unit| unit.target_kind == kind)
        .collect()
}

fn default_build_target_kinds() -> [TargetKind; 3] {
    [TargetKind::Lib, TargetKind::Bin, TargetKind::Example]
}
