use crate::analysis_context;
use crate::build_plan;
use crate::discover;
use crate::doc;
use crate::elaborate;
use crate::error::{Error, Result};
use crate::execute;
use crate::graph;
use crate::local_state;
use crate::lockfile;
use crate::manifest::Manifest;
use crate::operation_lock::WorkspaceOperationLock;
use crate::plan::TargetKind;
use crate::source;
use crate::workspace;
use std::fs;
use std::path::{Path, PathBuf};

use super::policy::{
    publish_summary, summarize_check_sources, summarize_source_security,
    validate_check_source_policy, validate_publish_lock_status, validate_publish_metadata,
};
use super::render::{
    Renderer, Tone, format_unit_label, format_yes_no, print_compile_actions,
    print_compile_actions_for_unit, print_fetched_package, print_generated_files,
    print_generated_files_for_unit, print_link_actions, print_link_actions_for_unit,
    render_execution_timings,
};
use super::{Command, InstallSelection, RunSelection};

pub(super) fn run_command(command: Command) -> Result<()> {
    match command {
        Command::Help => {
            print!("{}", super::usage());
            Ok(())
        }
        Command::Version => {
            println!("{}", super::version_text());
            Ok(())
        }
        Command::Init { path, ui } => run_init(path, ui),
        Command::Check {
            path,
            feature_selection,
            ui,
        } => run_check(path, feature_selection, ui),
        Command::Lock {
            path,
            feature_selection,
            ui,
        } => run_lock(path, feature_selection, ui),
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
        } => run_target(path, feature_selection, ui, selection),
        Command::Test {
            path,
            feature_selection,
            ui,
        } => run_tests(path, feature_selection, ui),
    }
}

fn run_init(path: Option<PathBuf>, ui: super::UiOptions) -> Result<()> {
    let render = Renderer::new(ui);
    let root = resolve_init_root(path.as_deref())?;
    let _workspace_lock = WorkspaceOperationLock::acquire(&root, "init")?;
    let init = plan_init(&root)?;
    let created = apply_init_plan(&root, &init)?;
    let manifest_path = root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path)?;
    let feature_selection = elaborate::FeatureSelection::default();

    render.header_with_path("init", &manifest, &manifest_path, &feature_selection);
    render.summary("root", root.display());
    render.summary("targets", init.target_summary());
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

fn run_check(
    path: Option<PathBuf>,
    feature_selection: elaborate::FeatureSelection,
    ui: super::UiOptions,
) -> Result<()> {
    let render = Renderer::new(ui);
    let (loaded, _workspace_lock) = load_locked_package_graph(
        path.as_deref(),
        crate::script::ScriptCommand::Check,
        &feature_selection,
        "check",
    )?;
    let lock_status = lockfile::lock_status(&loaded.manifest_path, &loaded.elaboration)?;
    let build_plan = build_plan::derive(&loaded.elaboration, crate::script::ScriptCommand::Check)?;
    let build_plan = filter_selected_package(build_plan, loaded.selected_package_id.as_ref());
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    execute::materialize_analysis_inputs(&build_plan, &action_plan)?;
    let _ = analysis_context::sync_analysis_context(
        &loaded.manifest_path,
        &loaded.elaboration,
        &build_plan,
        &feature_selection,
    );

    render.header_with_path(
        "check",
        &loaded.manifest,
        &loaded.manifest_path,
        &feature_selection,
    );
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
            loaded.manifest.test.len(),
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
            format!(
                "workspace craft {}, package craft {}, build.rn {}",
                format_yes_no(loaded.elaboration.workspace_script.is_some()),
                loaded.elaboration.package_script_count(),
                build_plan.build_script_count()
            ),
        );
    }
    render.summary(
        "lockfile",
        match lock_status {
            lockfile::LockStatus::Missing => "missing",
            lockfile::LockStatus::Current => "current",
            lockfile::LockStatus::Stale => "stale",
        },
    );
    if render.is_verbose() && build_plan.generated_file_count() > 0 {
        render.section("generated");
    }
    print_generated_files(&render, &build_plan);
    let execution = execute::check(&build_plan, &action_plan)?;
    render_execution_timings(&render, &execution);
    render.ok("check completed");

    Ok(())
}

fn run_lock(
    path: Option<PathBuf>,
    feature_selection: elaborate::FeatureSelection,
    ui: super::UiOptions,
) -> Result<()> {
    let render = Renderer::new(ui);
    let (loaded, _workspace_lock) = load_locked_package_graph(
        path.as_deref(),
        crate::script::ScriptCommand::Lock,
        &feature_selection,
        "lock",
    )?;
    let (lock_path, lock_result) =
        lockfile::sync_lockfile(&loaded.manifest_path, &loaded.elaboration)?;
    let edge_count = loaded
        .elaboration
        .package_graph
        .packages
        .iter()
        .map(|pkg| pkg.dependencies.len())
        .sum::<usize>();

    render.header_with_path(
        "lock",
        &loaded.manifest,
        &loaded.manifest_path,
        &feature_selection,
    );
    let status = match lock_result {
        lockfile::LockWriteResult::Created => "created",
        lockfile::LockWriteResult::Updated => "updated",
        lockfile::LockWriteResult::Unchanged => "unchanged",
    };
    render.summary("lockfile", format!("{status} at {}", lock_path.display()));
    render.summary(
        "graph",
        format!(
            "{} package(s), {} edge(s), {} external package(s)",
            loaded.elaboration.package_graph.packages.len(),
            edge_count,
            loaded.elaboration.resolved_graph.external_packages.len()
        ),
    );
    render.ok(match lock_result {
        lockfile::LockWriteResult::Created => "lockfile created",
        lockfile::LockWriteResult::Updated => "lockfile updated",
        lockfile::LockWriteResult::Unchanged => "lockfile already current",
    });

    Ok(())
}

fn run_fetch(
    path: Option<PathBuf>,
    feature_selection: elaborate::FeatureSelection,
    ui: super::UiOptions,
) -> Result<()> {
    let render = Renderer::new(ui);
    let (loaded, _workspace_lock) = load_locked_package_graph(
        path.as_deref(),
        crate::script::ScriptCommand::Fetch,
        &feature_selection,
        "fetch",
    )?;
    let fetched = source::fetch_external_packages(&loaded.elaboration.resolved_graph)?;
    let summary = source::summarize_fetch(&fetched);

    render.header_with_path(
        "fetch",
        &loaded.manifest,
        &loaded.manifest_path,
        &feature_selection,
    );
    render.summary(
        "packages",
        format!(
            "{} external package(s): created {}, updated {}, unchanged {}",
            fetched.len(),
            summary.created,
            summary.updated,
            summary.unchanged
        ),
    );
    if render.is_verbose() && !fetched.is_empty() {
        render.section("actions");
    }
    for package in &fetched {
        print_fetched_package(&render, package);
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
    let (loaded, _workspace_lock) = load_locked_package_graph(
        path.as_deref(),
        crate::script::ScriptCommand::Lock,
        &feature_selection,
        "publish",
    )?;
    let lock_status = lockfile::lock_status(&loaded.manifest_path, &loaded.elaboration)?;
    validate_publish_lock_status(&loaded.manifest_path, lock_status)?;
    let summary = publish_summary(
        &loaded.manifest_path,
        &loaded.manifest,
        &loaded.workspace_members,
    )?;
    validate_publish_metadata(&summary)?;

    render.header_with_path(
        "publish",
        &loaded.manifest,
        &loaded.manifest_path,
        &feature_selection,
    );
    render.summary(
        "packages",
        format!(
            "{} publishable package(s), {} blocked package(s)",
            summary.ready.len(),
            summary.blocked.len()
        ),
    );
    render.summary("lockfile", "current (release)");
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
    let (loaded, _workspace_lock) = load_locked_package_graph(
        path.as_deref(),
        crate::script::ScriptCommand::Build,
        &feature_selection,
        "build",
    )?;
    let build_plan = build_plan::derive_with_options(
        &loaded.elaboration,
        crate::script::ScriptCommand::Build,
        build_plan::DeriveOptions { include_examples },
    )?;
    let build_plan = filter_selected_package(build_plan, loaded.selected_package_id.as_ref());
    let _ = analysis_context::sync_analysis_context(
        &loaded.manifest_path,
        &loaded.elaboration,
        &build_plan,
        &feature_selection,
    );
    let build_plan = build_plan.filtered_target_kinds(&default_build_target_kinds());
    let target = crate::script::host_target();
    let action_plan = build_plan.derive_actions(&target);

    render.header_with_path(
        "build",
        &loaded.manifest,
        &loaded.manifest_path,
        &feature_selection,
    );
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
    let execution = execute::build(&build_plan, &action_plan)?;
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
    let (loaded, _workspace_lock) = load_locked_package_graph(
        path.as_deref(),
        crate::script::ScriptCommand::Build,
        &feature_selection,
        "install",
    )?;
    let target_package_id = selected_target_package_id(&loaded, "install")?;
    let build_plan = build_plan::derive(&loaded.elaboration, crate::script::ScriptCommand::Build)?;
    let build_plan = build_plan
        .filtered_package_closure(&[(graph::BuildDomain::Target, target_package_id.clone())]);
    let _ = analysis_context::sync_analysis_context(
        &loaded.manifest_path,
        &loaded.elaboration,
        &build_plan,
        &feature_selection,
    );
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let install_units = select_install_units(&build_plan, &target_package_id, &selection)?;
    let install_root = resolve_install_root(root.as_deref())?;
    let install_bin_dir = install_root.join("bin");

    render.header_with_path(
        "install",
        &loaded.manifest,
        &loaded.manifest_path,
        &feature_selection,
    );
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

    let execution = execute::build(&build_plan, &action_plan)?;
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
    let (loaded, _workspace_lock) = load_locked_package_graph(
        path.as_deref(),
        crate::script::ScriptCommand::Build,
        &feature_selection,
        "doc",
    )?;
    let build_plan = build_plan::derive(&loaded.elaboration, crate::script::ScriptCommand::Build)?;
    let build_plan = filter_selected_package(build_plan, loaded.selected_package_id.as_ref());
    let _ = analysis_context::sync_analysis_context(
        &loaded.manifest_path,
        &loaded.elaboration,
        &build_plan,
        &feature_selection,
    );
    let build_plan = build_plan.filtered_target_kinds(&default_build_target_kinds());
    let action_plan = build_plan.derive_actions(&crate::script::host_target());

    render.header_with_path(
        "doc",
        &loaded.manifest,
        &loaded.manifest_path,
        &feature_selection,
    );
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
    let execution = execute::build(&build_plan, &action_plan)?;
    let docs = doc::sync_workspace_docs(&build_plan, &action_plan)?;
    render.summary(
        "docs",
        format!(
            "{} package(s), {} documented item(s), output {}",
            docs.len(),
            docs.iter().map(|doc| doc.item_count).sum::<usize>(),
            build_plan.workspace_root.join(".craft/docs").display()
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

fn run_uninstall(
    path: Option<PathBuf>,
    ui: super::UiOptions,
    selection: InstallSelection,
    root: Option<PathBuf>,
) -> Result<()> {
    let render = Renderer::new(ui);
    let feature_selection = elaborate::FeatureSelection::default();
    let (loaded, _workspace_lock) = load_locked_package_graph(
        path.as_deref(),
        crate::script::ScriptCommand::Build,
        &feature_selection,
        "uninstall",
    )?;
    let target_package_id = selected_target_package_id(&loaded, "uninstall")?;
    let build_plan = build_plan::derive(&loaded.elaboration, crate::script::ScriptCommand::Build)?;
    let build_plan = build_plan
        .filtered_package_closure(&[(graph::BuildDomain::Target, target_package_id.clone())]);
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let uninstall_units = select_install_units(&build_plan, &target_package_id, &selection)?;
    let install_root = resolve_install_root(root.as_deref())?;
    let install_bin_dir = install_root.join("bin");
    let mut removed = 0usize;
    let mut missing = 0usize;

    render.header_with_path(
        "uninstall",
        &loaded.manifest,
        &loaded.manifest_path,
        &feature_selection,
    );
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
) -> Result<()> {
    let render = Renderer::new(ui);
    let (loaded, _workspace_lock) = load_locked_package_graph(
        path.as_deref(),
        crate::script::ScriptCommand::Run,
        &feature_selection,
        "run",
    )?;
    let build_plan = build_plan::derive_with_options(
        &loaded.elaboration,
        crate::script::ScriptCommand::Run,
        build_plan::DeriveOptions {
            include_examples: matches!(selection, RunSelection::Example(_)),
        },
    )?;
    let build_plan = filter_selected_package(build_plan, loaded.selected_package_id.as_ref());
    let _ = analysis_context::sync_analysis_context(
        &loaded.manifest_path,
        &loaded.elaboration,
        &build_plan,
        &feature_selection,
    );
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let run_unit = select_run_unit(&build_plan, &selection)?;

    render.header_with_path(
        "run",
        &loaded.manifest,
        &loaded.manifest_path,
        &feature_selection,
    );
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
    let execution = execute::run(&build_plan, &action_plan, run_unit)?;
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
) -> Result<()> {
    let render = Renderer::new(ui);
    let (loaded, _workspace_lock) = load_locked_package_graph(
        path.as_deref(),
        crate::script::ScriptCommand::Test,
        &feature_selection,
        "test",
    )?;
    let build_plan = build_plan::derive(&loaded.elaboration, crate::script::ScriptCommand::Test)?;
    let build_plan = filter_selected_package(build_plan, loaded.selected_package_id.as_ref());
    let _ = analysis_context::sync_analysis_context(
        &loaded.manifest_path,
        &loaded.elaboration,
        &build_plan,
        &feature_selection,
    );
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let tests = units_of_kind(&build_plan, TargetKind::Test);

    render.header_with_path(
        "test",
        &loaded.manifest,
        &loaded.manifest_path,
        &feature_selection,
    );
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
    let execution = execute::test(&build_plan, &action_plan, &tests)?;
    render_execution_timings(&render, &execution.build);
    render.ok(format!(
        "test run completed ({} executed)",
        execution.executed
    ));

    Ok(())
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
        .join("src/lib.rn")
        .is_file()
        .then(|| "src/lib.rn".to_string());
    let mut bin_root = root
        .join("src/main.rn")
        .is_file()
        .then(|| "src/main.rn".to_string());
    let create_main_stub = lib_root.is_none() && bin_root.is_none();
    if create_main_stub {
        bin_root = Some("src/main.rn".to_string());
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
            &root.join("src/main.rn"),
            "fn main() i32 {\n    return 0;\n}\n",
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
        env!("CARGO_PKG_VERSION")
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
        if path.extension().and_then(|ext| ext.to_str()) != Some("rn") {
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
    if cfg!(windows) {
        if let Some(user_profile) = std::env::var_os("USERPROFILE") {
            return Ok(PathBuf::from(user_profile).join(".kern"));
        }
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
}

fn load_locked_package_graph(
    path: Option<&Path>,
    command: crate::script::ScriptCommand,
    feature_selection: &elaborate::FeatureSelection,
    operation: &str,
) -> Result<(LoadedPackageGraph, WorkspaceOperationLock)> {
    let selected_manifest_path = path
        .map(|path| discover::resolve_manifest_path(Some(path)))
        .transpose()?;
    let manifest_path = discover::resolve_project_manifest_path(path)?;
    let manifest = Manifest::load(&manifest_path)?;
    manifest.validate(&manifest_path)?;
    let workspace_members = workspace::load_members(&manifest_path, &manifest)?;
    let workspace_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let workspace_lock = WorkspaceOperationLock::acquire(workspace_root, operation)?;
    let elaboration = elaborate::plan(
        &manifest_path,
        &manifest,
        &workspace_members,
        manifest.workspace.is_some(),
        command,
        feature_selection,
    )?;
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
        },
        workspace_lock,
    ))
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
