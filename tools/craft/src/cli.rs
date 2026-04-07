use crate::analysis_context;
use crate::build_plan;
use crate::discover;
use crate::doc;
use crate::elaborate;
use crate::error::{Error, Result};
use crate::execute;
use crate::graph;
use crate::lockfile;
use crate::manifest::Manifest;
use crate::operation_lock::WorkspaceOperationLock;
use crate::plan::TargetKind;
use crate::source;
use crate::workspace;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt::Display;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum Command {
    Help,
    Check {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
    },
    Lock {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
    },
    Fetch {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
    },
    Publish {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
    },
    Doc {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
    },
    Build {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
    },
    Run {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
    },
    Test {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UiOptions {
    verbose: bool,
    timings: bool,
    color: ColorChoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

struct Renderer {
    verbose: bool,
    timings: bool,
    color_enabled: bool,
}

#[derive(Clone, Copy)]
enum Tone {
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

    fn new(ui: UiOptions) -> Self {
        let color_enabled = match ui.color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => {
                std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
            }
        };

        Self {
            verbose: ui.verbose,
            timings: ui.timings,
            color_enabled,
        }
    }

    fn header_with_path(
        &self,
        command: &str,
        manifest: &Manifest,
        manifest_path: &Path,
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

    fn meta(&self, label: &str, value: impl Display) {
        let label = self.paint(
            Tone::Muted,
            &format!("{label:<width$}", width = Self::LABEL_WIDTH),
        );
        println!("    {label} {value}");
    }

    fn summary(&self, label: &str, value: impl Display) {
        let label = self.paint(
            Tone::Muted,
            &format!("{label:<width$}", width = Self::LABEL_WIDTH),
        );
        println!("    {label} {value}");
    }

    fn section(&self, name: &str) {
        if !self.verbose {
            return;
        }
        let marker = self.paint(Tone::Muted, "--");
        let name = self.paint(Tone::Accent, name);
        println!("  {marker} {name}");
    }

    fn action(&self, tone: Tone, kind: &str, subject: impl Display, detail: impl Display) {
        if !self.verbose {
            return;
        }
        let kind = self.paint(tone, &format!("{kind:<8}"));
        println!("  {kind} {subject} {detail}");
    }

    fn ok(&self, message: impl Display) {
        println!("{} {message}", self.paint(Tone::Ok, "[ok]"));
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

pub fn run() -> Result<()> {
    run_command(parse_args(env::args().skip(1))?)
}

fn run_command(command: Command) -> Result<()> {
    match command {
        Command::Help => {
            print!("{}", usage());
            Ok(())
        }
        Command::Check {
            path,
            feature_selection,
            ui,
        } => {
            let render = Renderer::new(ui);
            let (loaded, _workspace_lock) = load_locked_package_graph(
                path.as_deref(),
                crate::script::ScriptCommand::Check,
                &feature_selection,
                "check",
            )?;
            let lock_status = lockfile::lock_status(&loaded.manifest_path, &loaded.elaboration)?;
            let build_plan =
                build_plan::derive(&loaded.elaboration, crate::script::ScriptCommand::Check)?;
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
            if render.verbose {
                render.meta("deps", dependency_summary);
            }
            let security_summary = summarize_source_security(&loaded.manifest);
            validate_check_source_policy(
                &loaded.manifest_path,
                &feature_selection,
                &security_summary,
            )?;
            render.summary(
                "sources",
                format!(
                    "{} git package(s), {} path package(s)",
                    source_summary.git_packages, source_summary.path_packages,
                ),
            );
            if render.verbose
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
            if render.verbose {
                render.meta(
                    "scripts",
                    format!(
                        "workspace craft {}, package craft {}, build.rn {}, env inputs {}",
                        format_yes_no(loaded.elaboration.workspace_script.is_some()),
                        loaded.elaboration.package_script_count(),
                        build_plan.build_script_count(),
                        loaded.elaboration.workspace_env_input_count()
                            + loaded.elaboration.package_env_input_count()
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
            if render.verbose && build_plan.generated_file_count() > 0 {
                render.section("generated");
            }
            print_generated_files(&render, &build_plan);
            render.ok("check completed");

            Ok(())
        }
        Command::Lock {
            path,
            feature_selection,
            ui,
        } => {
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
                    loaded.package_graph.packages.len(),
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
        Command::Fetch {
            path,
            feature_selection,
            ui,
        } => {
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
            if render.verbose && !fetched.is_empty() {
                render.section("actions");
            }
            for package in &fetched {
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
            render.ok("fetch completed");

            Ok(())
        }
        Command::Publish {
            path,
            feature_selection,
            ui,
        } => {
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
            if render.verbose {
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
        Command::Build {
            path,
            feature_selection,
            ui,
        } => {
            let render = Renderer::new(ui);
            let (loaded, _workspace_lock) = load_locked_package_graph(
                path.as_deref(),
                crate::script::ScriptCommand::Build,
                &feature_selection,
                "build",
            )?;
            let build_plan =
                build_plan::derive(&loaded.elaboration, crate::script::ScriptCommand::Build)?;
            let _ = analysis_context::sync_analysis_context(
                &loaded.manifest_path,
                &loaded.elaboration,
                &build_plan,
                &feature_selection,
            );
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
            if render.verbose {
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
            if render.verbose
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
        Command::Doc {
            path,
            feature_selection,
            ui,
        } => {
            let render = Renderer::new(ui);
            let (loaded, _workspace_lock) = load_locked_package_graph(
                path.as_deref(),
                crate::script::ScriptCommand::Build,
                &feature_selection,
                "doc",
            )?;
            let build_plan =
                build_plan::derive(&loaded.elaboration, crate::script::ScriptCommand::Build)?;
            let _ = analysis_context::sync_analysis_context(
                &loaded.manifest_path,
                &loaded.elaboration,
                &build_plan,
                &feature_selection,
            );
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
            if render.verbose
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
            if render.verbose {
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
        Command::Run {
            path,
            feature_selection,
            ui,
        } => {
            let render = Renderer::new(ui);
            let (loaded, _workspace_lock) = load_locked_package_graph(
                path.as_deref(),
                crate::script::ScriptCommand::Run,
                &feature_selection,
                "run",
            )?;
            let build_plan =
                build_plan::derive(&loaded.elaboration, crate::script::ScriptCommand::Run)?;
            let _ = analysis_context::sync_analysis_context(
                &loaded.manifest_path,
                &loaded.elaboration,
                &build_plan,
                &feature_selection,
            );
            let action_plan = build_plan.derive_actions(&crate::script::host_target());
            let runnable = units_of_kind(&build_plan, TargetKind::Bin);

            let run_unit = match runnable.as_slice() {
                [] => {
                    return Err(Error::Usage(
                        "`craft run` requires exactly one runnable `bin` target, but none were found"
                            .to_string(),
                    ));
                }
                [unit] => *unit,
                units => {
                    let candidates = units
                        .iter()
                        .map(|unit| format_unit_label(unit))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(Error::Usage(format!(
                        "`craft run` requires exactly one runnable `bin` target, but found {}: {}",
                        units.len(),
                        candidates
                    )));
                }
            };

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
            if render.verbose
                && (!run_unit.generated_files.is_empty()
                    || action_plan.compile_count() > 0
                    || action_plan.link_count() > 0)
            {
                render.section("actions");
            }
            print_generated_files_for_unit(&render, &build_plan, run_unit);
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
        Command::Test {
            path,
            feature_selection,
            ui,
        } => {
            let render = Renderer::new(ui);
            let (loaded, _workspace_lock) = load_locked_package_graph(
                path.as_deref(),
                crate::script::ScriptCommand::Test,
                &feature_selection,
                "test",
            )?;
            let build_plan =
                build_plan::derive(&loaded.elaboration, crate::script::ScriptCommand::Test)?;
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
            if render.verbose && !tests.is_empty() {
                render.meta(
                    "targets",
                    tests
                        .iter()
                        .map(|unit| format_unit_label(unit))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            if render.verbose
                && (!tests.is_empty()
                    || action_plan.compile_count() > 0
                    || action_plan.link_count() > 0)
            {
                render.section("actions");
            }
            for unit in &tests {
                print_generated_files_for_unit(&render, &build_plan, unit);
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
    }
}

struct LoadedPackageGraph {
    manifest_path: PathBuf,
    manifest: Manifest,
    workspace_members: Vec<workspace::WorkspaceMember>,
    package_graph: graph::PackageGraph,
    elaboration: elaborate::ElaborationPlan,
}

fn load_locked_package_graph(
    path: Option<&std::path::Path>,
    command: crate::script::ScriptCommand,
    feature_selection: &elaborate::FeatureSelection,
    operation: &str,
) -> Result<(LoadedPackageGraph, WorkspaceOperationLock)> {
    let manifest_path = discover::resolve_manifest_path(path)?;
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
    let package_graph = graph::build_graph_from_plans(
        &manifest_path,
        &manifest,
        elaboration.packages.iter().map(|pkg| &pkg.plan),
    )?;

    Ok((
        LoadedPackageGraph {
            manifest_path,
            manifest,
            workspace_members,
            package_graph,
            elaboration,
        },
        workspace_lock,
    ))
}

fn format_package_label(manifest: &Manifest) -> String {
    manifest
        .package
        .as_ref()
        .map(|package| format!("{} {}", package.name, package.version))
        .unwrap_or_else(|| "<workspace>".to_string())
}

fn format_yes_no(value: bool) -> &'static str {
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

fn units_of_kind(plan: &build_plan::BuildPlan, kind: TargetKind) -> Vec<&build_plan::BuildUnit> {
    plan.packages
        .iter()
        .flat_map(|package| &package.units)
        .filter(|unit| unit.target_kind == kind)
        .collect()
}

fn format_unit_label(unit: &build_plan::BuildUnit) -> String {
    format!(
        "{}:{} [{},{}]",
        unit.package_id.name,
        unit.artifact_name,
        unit.target_kind.as_str(),
        unit.domain.as_str()
    )
}

fn format_external_package_label(package: &crate::resolver::ExternalPackageId) -> String {
    match &package.version {
        Some(version) => format!("{} {}", package.package_name, version),
        None => package.package_name.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CheckSourceSummary {
    git_sources: usize,
    git_packages: usize,
    path_packages: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceSecuritySummary {
    policy_mode: crate::manifest::ReleaseSourcePolicy,
    floating_git_sources: usize,
    insecure_transport_sources: usize,
    warnings: Vec<String>,
    suppressed: Vec<String>,
    release_blockers: Vec<String>,
}

impl SourceSecuritySummary {
    fn warning_count(&self) -> usize {
        self.warnings.len()
    }

    fn suppressed_count(&self) -> usize {
        self.suppressed.len()
    }

    fn release_blockers(&self) -> &[String] {
        self.release_blockers.as_slice()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PublishPackageSummary {
    name: String,
    version: String,
    manifest_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PublishIssue {
    package_name: String,
    manifest_path: PathBuf,
    missing_fields: Vec<&'static str>,
    missing_readme_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PublishSummary {
    ready: Vec<PublishPackageSummary>,
    blocked: Vec<PublishIssue>,
}

fn summarize_check_sources(resolved: &crate::resolver::ResolvedGraph) -> CheckSourceSummary {
    let mut git_packages = 0usize;
    let mut path_packages = 0usize;

    for package in &resolved.external_packages {
        match &package.id.source {
            crate::graph::SourceId::PathDependency { .. } => path_packages += 1,
            crate::graph::SourceId::GitDependency { .. } => git_packages += 1,
            crate::graph::SourceId::Root | crate::graph::SourceId::WorkspaceMember { .. } => {}
        }
    }

    CheckSourceSummary {
        git_sources: git_packages,
        git_packages,
        path_packages,
    }
}

fn summarize_source_security(manifest: &Manifest) -> SourceSecuritySummary {
    let policy_mode = manifest
        .craft
        .as_ref()
        .and_then(|craft| craft.release_source_policy)
        .unwrap_or(crate::manifest::ReleaseSourcePolicy::Enforce);
    let allow_floating_git = manifest
        .craft
        .as_ref()
        .map(|craft| {
            craft
                .allow_floating_git
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let allow_insecure_source = manifest
        .craft
        .as_ref()
        .map(|craft| {
            craft
                .allow_insecure_source
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let mut floating_git_sources = 0usize;
    let mut insecure_transport_sources = 0usize;
    let mut warnings = Vec::new();
    let mut suppressed = Vec::new();

    for (name, dep) in release_policy_dependencies(manifest) {
        let Some(git) = dep.git.as_deref() else {
            continue;
        };

        if is_insecure_git_source(git) {
            insecure_transport_sources += 1;
            let label = format!("{name}(insecure-transport)");
            if allow_insecure_source.contains(name.as_str()) {
                suppressed.push(label);
            } else {
                warnings.push(label);
            }
        }

        if dep.rev.is_none() && dep.tag.is_none() {
            floating_git_sources += 1;
            let label = format!("{name}(floating-git)");
            if allow_floating_git.contains(name.as_str()) {
                suppressed.push(label);
            } else {
                warnings.push(label);
            }
        }
    }

    let release_blockers = match policy_mode {
        crate::manifest::ReleaseSourcePolicy::Enforce => warnings.clone(),
        crate::manifest::ReleaseSourcePolicy::Warn | crate::manifest::ReleaseSourcePolicy::Off => {
            Vec::new()
        }
    };

    SourceSecuritySummary {
        policy_mode,
        floating_git_sources,
        insecure_transport_sources,
        warnings,
        suppressed,
        release_blockers,
    }
}

fn release_policy_dependencies<'a>(
    manifest: &'a Manifest,
) -> BTreeMap<String, &'a crate::manifest::DetailedDependency> {
    let mut dependencies = BTreeMap::new();

    if let Some(workspace) = &manifest.workspace {
        collect_release_policy_dependencies(&mut dependencies, &workspace.dependencies);
    }
    collect_release_policy_dependencies(&mut dependencies, &manifest.dependencies);
    collect_release_policy_dependencies(&mut dependencies, &manifest.dev_dependencies);
    collect_release_policy_dependencies(&mut dependencies, &manifest.build_dependencies);

    dependencies
}

fn collect_release_policy_dependencies<'a>(
    out: &mut BTreeMap<String, &'a crate::manifest::DetailedDependency>,
    section: &'a BTreeMap<String, crate::manifest::DependencySpec>,
) {
    for (name, spec) in section {
        let crate::manifest::DependencySpec::Detailed(dep) = spec else {
            continue;
        };
        if dep.git.is_some() {
            out.entry(name.clone()).or_insert(dep);
        }
    }
}

fn is_insecure_git_source(locator: &str) -> bool {
    locator.starts_with("http://")
}

fn validate_check_source_policy(
    manifest_path: &Path,
    selection: &elaborate::FeatureSelection,
    summary: &SourceSecuritySummary,
) -> Result<()> {
    if selection.profile != crate::script::ProfileSelection::Release
        || summary.release_blockers().is_empty()
    {
        return Ok(());
    }

    Err(Error::Validation {
        path: manifest_path.to_path_buf(),
        message: format!(
            "release source policy rejected: {}",
            summary.release_blockers().join(", ")
        ),
    })
}

fn validate_publish_lock_status(
    manifest_path: &Path,
    lock_status: lockfile::LockStatus,
) -> Result<()> {
    if lock_status == lockfile::LockStatus::Current {
        return Ok(());
    }

    Err(Error::Validation {
        path: manifest_path.to_path_buf(),
        message:
            "publish requires a current release `Craft.lock`; run `craft lock --profile release` first"
                .to_string(),
    })
}

fn publish_summary(
    root_manifest_path: &Path,
    root_manifest: &Manifest,
    workspace_members: &[workspace::WorkspaceMember],
) -> Result<PublishSummary> {
    let workspace_defaults = root_manifest
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.package.as_ref());
    let mut ready = Vec::new();
    let mut blocked = Vec::new();

    if let Some(package) = &root_manifest.package
        && package.publish != Some(false)
    {
        classify_publish_package(
            root_manifest_path,
            root_manifest_path,
            package,
            workspace_defaults,
            &mut ready,
            &mut blocked,
        )?;
    }

    for member in workspace_members {
        let Some(package) = &member.manifest.package else {
            continue;
        };
        if package.publish == Some(false) {
            continue;
        }
        classify_publish_package(
            root_manifest_path,
            &member.manifest_path,
            package,
            workspace_defaults,
            &mut ready,
            &mut blocked,
        )?;
    }

    if ready.is_empty() && blocked.is_empty() {
        return Err(Error::Validation {
            path: root_manifest_path.to_path_buf(),
            message: "publish found no publishable packages; set `[package].publish = true` or omit `publish = false`"
                .to_string(),
        });
    }

    Ok(PublishSummary { ready, blocked })
}

fn validate_publish_metadata(summary: &PublishSummary) -> Result<()> {
    if summary.blocked.is_empty() {
        return Ok(());
    }

    let message = summary
        .blocked
        .iter()
        .map(|issue| {
            let mut parts = Vec::new();
            if !issue.missing_fields.is_empty() {
                parts.push(format!("missing {}", issue.missing_fields.join(", ")));
            }
            if let Some(path) = &issue.missing_readme_path {
                parts.push(format!("readme not found at {}", path.display()));
            }
            format!(
                "{} ({}): {}",
                issue.package_name,
                issue.manifest_path.display(),
                parts.join("; ")
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");

    Err(Error::Validation {
        path: summary.blocked[0].manifest_path.clone(),
        message: format!("publish metadata check failed: {message}"),
    })
}

fn classify_publish_package(
    root_manifest_path: &Path,
    manifest_path: &Path,
    package: &crate::manifest::Package,
    defaults: Option<&crate::manifest::WorkspacePackage>,
    ready: &mut Vec<PublishPackageSummary>,
    blocked: &mut Vec<PublishIssue>,
) -> Result<()> {
    let mut missing_fields = Vec::new();
    if publish_description(package, defaults).is_none() {
        missing_fields.push("[package].description");
    }
    if publish_license(package, defaults).is_none() {
        missing_fields.push("[package].license");
    }
    if publish_authors(package, defaults).is_none() {
        missing_fields.push("[package].authors");
    }
    let readme = publish_readme(package, defaults);
    if readme.is_none() {
        missing_fields.push("[package].readme");
    }
    if publish_repository(package, defaults).is_none() {
        missing_fields.push("[package].repository");
    }

    let package_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let workspace_root = root_manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let missing_readme_path = readme
        .map(|(readme, inherited)| {
            if inherited {
                workspace_root.join(readme)
            } else {
                package_root.join(readme)
            }
        })
        .filter(|path| !path.is_file());

    if missing_fields.is_empty() && missing_readme_path.is_none() {
        ready.push(PublishPackageSummary {
            name: package.name.clone(),
            version: package.version.clone(),
            manifest_path: manifest_path.to_path_buf(),
        });
    } else {
        blocked.push(PublishIssue {
            package_name: package.name.clone(),
            manifest_path: manifest_path.to_path_buf(),
            missing_fields,
            missing_readme_path,
        });
    }

    Ok(())
}

fn publish_description<'a>(
    package: &'a crate::manifest::Package,
    defaults: Option<&'a crate::manifest::WorkspacePackage>,
) -> Option<&'a str> {
    package
        .description
        .as_deref()
        .or_else(|| defaults.and_then(|defaults| defaults.description.as_deref()))
}

fn publish_license<'a>(
    package: &'a crate::manifest::Package,
    defaults: Option<&'a crate::manifest::WorkspacePackage>,
) -> Option<&'a str> {
    package
        .license
        .as_deref()
        .or_else(|| defaults.and_then(|defaults| defaults.license.as_deref()))
}

fn publish_authors<'a>(
    package: &'a crate::manifest::Package,
    defaults: Option<&'a crate::manifest::WorkspacePackage>,
) -> Option<&'a [String]> {
    if !package.authors.is_empty() {
        Some(package.authors.as_slice())
    } else {
        defaults
            .filter(|defaults| !defaults.authors.is_empty())
            .map(|defaults| defaults.authors.as_slice())
    }
}

fn publish_readme<'a>(
    package: &'a crate::manifest::Package,
    defaults: Option<&'a crate::manifest::WorkspacePackage>,
) -> Option<(&'a str, bool)> {
    package
        .readme
        .as_deref()
        .map(|value| (value, false))
        .or_else(|| {
            defaults
                .and_then(|defaults| defaults.readme.as_deref())
                .map(|value| (value, true))
        })
}

fn publish_repository<'a>(
    package: &'a crate::manifest::Package,
    defaults: Option<&'a crate::manifest::WorkspacePackage>,
) -> Option<&'a str> {
    package
        .repository
        .as_deref()
        .or_else(|| defaults.and_then(|defaults| defaults.repository.as_deref()))
}

fn format_fetched_source_backend(package: &source::FetchedPackage) -> &'static str {
    package.source.backend.as_str()
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

fn print_compile_actions(render: &Renderer, action_plan: &build_plan::ActionPlan) {
    for action in &action_plan.compile_actions {
        print_compile_action(render, action, &action.artifact_name);
    }
}

fn print_generated_files(render: &Renderer, build_plan: &build_plan::BuildPlan) {
    for package in &build_plan.packages {
        for unit in &package.units {
            print_generated_files_for_unit(render, build_plan, unit);
        }
    }
}

fn print_generated_files_for_unit(
    render: &Renderer,
    _build_plan: &build_plan::BuildPlan,
    unit: &build_plan::BuildUnit,
) {
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

fn print_link_actions(render: &Renderer, action_plan: &build_plan::ActionPlan) {
    for action in &action_plan.link_actions {
        print_link_action(render, action, &action.artifact_name);
    }
}

fn print_compile_actions_for_unit(
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

fn print_link_actions_for_unit(
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

fn render_execution_timings(render: &Renderer, summary: &execute::ExecutionSummary) {
    if !render.timings || summary.phase_timings.is_empty() {
        return;
    }

    render.summary("time", format_duration(summary.total_duration()));
    render.summary("phases", format_phase_timings(&summary.phase_timings));

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
            format_phase_timings(&action.phase_timings),
        );
    }
}

fn format_phase_timings(phases: &[kernc_driver::PhaseTiming]) -> String {
    phases
        .iter()
        .map(|phase| format!("{} {}", phase.name, format_duration(phase.duration)))
        .collect::<Vec<_>>()
        .join(", ")
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

fn parse_args<I>(args: I) -> Result<Command>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    let Some((cmd, rest)) = args.split_first() else {
        return Ok(Command::Help);
    };
    if cmd == "help" || cmd == "--help" || cmd == "-h" {
        return Ok(Command::Help);
    }

    if rest.len() == 1 && (rest[0] == "--help" || rest[0] == "-h") {
        return Ok(Command::Help);
    }

    let (path, feature_selection, ui) = parse_command_options(rest)?;
    match cmd.as_str() {
        "check" => Ok(Command::Check {
            path,
            feature_selection,
            ui,
        }),
        "lock" => Ok(Command::Lock {
            path,
            feature_selection,
            ui,
        }),
        "fetch" => Ok(Command::Fetch {
            path,
            feature_selection,
            ui,
        }),
        "publish" => {
            let mut feature_selection = feature_selection;
            feature_selection.profile = crate::script::ProfileSelection::Release;
            Ok(Command::Publish {
                path,
                feature_selection,
                ui,
            })
        }
        "doc" => Ok(Command::Doc {
            path,
            feature_selection,
            ui,
        }),
        "build" => Ok(Command::Build {
            path,
            feature_selection,
            ui,
        }),
        "run" => Ok(Command::Run {
            path,
            feature_selection,
            ui,
        }),
        "test" => Ok(Command::Test {
            path,
            feature_selection,
            ui,
        }),
        _ => Err(Error::Usage(format!(
            "unsupported command line: {}\n\n{}",
            args.join(" "),
            usage()
        ))),
    }
}

fn parse_command_options(
    args: &[String],
) -> Result<(Option<PathBuf>, elaborate::FeatureSelection, UiOptions)> {
    let mut path: Option<PathBuf> = None;
    let mut feature_selection = elaborate::FeatureSelection::default();
    let mut ui = UiOptions::default();
    let mut idx = 0;

    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--verbose" || arg == "-v" {
            ui.verbose = true;
            idx += 1;
            continue;
        }
        if arg == "--timings" {
            ui.timings = true;
            idx += 1;
            continue;
        }
        if arg == "--no-color" {
            ui.color = ColorChoice::Never;
            idx += 1;
            continue;
        }
        if arg == "--color" {
            let Some(value) = args.get(idx + 1) else {
                return Err(Error::Usage(
                    "`--color` requires one of: auto, always, never".to_string(),
                ));
            };
            ui.color = parse_color_choice(value)?;
            idx += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--color=") {
            ui.color = parse_color_choice(value)?;
            idx += 1;
            continue;
        }
        if arg == "--no-default-features" {
            feature_selection.enable_default = false;
            idx += 1;
            continue;
        }
        if arg == "--project-path" {
            let Some(value) = args.get(idx + 1) else {
                return Err(Error::Usage(
                    "`--project-path` requires a package or workspace path".to_string(),
                ));
            };
            set_project_path(&mut path, value)?;
            idx += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--project-path=") {
            set_project_path(&mut path, value)?;
            idx += 1;
            continue;
        }
        if arg == "--profile" {
            let Some(value) = args.get(idx + 1) else {
                return Err(Error::Usage(
                    "`--profile` requires one of: dev, release".to_string(),
                ));
            };
            feature_selection.profile = parse_profile_selection(value)?;
            idx += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--profile=") {
            feature_selection.profile = parse_profile_selection(value)?;
            idx += 1;
            continue;
        }
        if arg == "--features" {
            let Some(value) = args.get(idx + 1) else {
                return Err(Error::Usage(
                    "`--features` requires a comma-separated feature list".to_string(),
                ));
            };
            extend_feature_selection(&mut feature_selection, value)?;
            idx += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--features=") {
            extend_feature_selection(&mut feature_selection, value)?;
            idx += 1;
            continue;
        }
        if arg.starts_with('-') {
            return Err(Error::Usage(format!(
                "unsupported option `{arg}`\n\n{}",
                usage()
            )));
        }
        return Err(Error::Usage(format!(
            "unexpected positional argument `{arg}`; use `--project-path <PATH>`\n\n{}",
            usage()
        )));
    }

    Ok((path, feature_selection, ui))
}

fn parse_color_choice(raw: &str) -> Result<ColorChoice> {
    match raw {
        "auto" => Ok(ColorChoice::Auto),
        "always" => Ok(ColorChoice::Always),
        "never" => Ok(ColorChoice::Never),
        other => Err(Error::Usage(format!(
            "unsupported `--color` value `{other}`; expected auto, always, or never"
        ))),
    }
}

fn parse_profile_selection(raw: &str) -> Result<crate::script::ProfileSelection> {
    match raw {
        "dev" => Ok(crate::script::ProfileSelection::Dev),
        "release" => Ok(crate::script::ProfileSelection::Release),
        other => Err(Error::Usage(format!(
            "unsupported `--profile` value `{other}`; expected dev or release"
        ))),
    }
}

fn set_project_path(slot: &mut Option<PathBuf>, raw: &str) -> Result<()> {
    if let Some(existing_path) = slot {
        return Err(Error::Usage(format!(
            "multiple `--project-path` values provided: `{}` and `{raw}`",
            existing_path.display()
        )));
    }

    *slot = Some(PathBuf::from(raw));
    Ok(())
}

fn extend_feature_selection(selection: &mut elaborate::FeatureSelection, raw: &str) -> Result<()> {
    for feature in raw.split(',') {
        let feature = feature.trim();
        if feature.is_empty() {
            return Err(Error::Usage(
                "feature names in `--features` must not be empty".to_string(),
            ));
        }
        selection.explicit.insert(feature.to_string());
    }
    Ok(())
}

fn usage() -> &'static str {
    "\
craft
  Kern package manager and builder

usage
  craft help
  craft <command> [--project-path <PATH>] [--profile <dev|release>] [--no-default-features] [--features <FEATURES>] [--verbose] [--timings] [--color <WHEN>]

commands
  help   Show this help text
  check  Validate `Craft.toml`, scripts, sources, and derived analysis inputs
  lock   Write a deterministic `Craft.lock` for the current package graph
  fetch  Materialize external package sources into the local `.craft` cache
  publish  Run release-oriented publish readiness checks without uploading anywhere
  doc    Build library metadata and render native package docs to Markdown
  build  Build the selected package graph and print the derived action plan
  run    Build and run the single runnable `bin` target in the package graph
  test   Build and run all discovered `test` targets

options
  --project-path <PATH>    Select the package or workspace root (or `Craft.toml` path)
  --profile <NAME>         Profile selection: dev (default) or release
  --no-default-features    Disable the implicit `default` feature
  --features <FEATURES>    Enable a comma-separated feature list
  --verbose, -v            Print detailed action logs instead of the default compact summary
  --timings                Print aggregated compiler/linker phase timings
  --color <WHEN>           Color mode: auto, always, never
  --no-color               Alias for `--color never`

examples
  craft check
  craft build --project-path path/to/pkg --profile release
  craft doc --verbose
  craft build --timings
  craft run --features tls,simd
  craft build --verbose --color always
"
}

#[cfg(test)]
mod tests {
    use super::{
        ColorChoice, Command, UiOptions, parse_args, run_command, summarize_check_sources,
        summarize_source_security, validate_check_source_policy,
    };
    use crate::elaborate::FeatureSelection;
    use crate::graph::SourceId;
    use crate::manifest::{Manifest, ReleaseSourcePolicy};
    use crate::operation_lock::WorkspaceOperationLock;
    use crate::resolver::{ExternalPackageId, ResolvedExternalPackage, ResolvedGraph};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_minimal_bin_package(root: &std::path::Path) {
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
        fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
    }

    #[test]
    fn parses_check_with_path_and_feature_options() {
        let cmd = parse_args([
            "check".to_string(),
            "--project-path".to_string(),
            "demo".to_string(),
            "--no-default-features".to_string(),
            "--features".to_string(),
            "tls,simd".to_string(),
        ])
        .unwrap();

        match cmd {
            Command::Check {
                path,
                feature_selection,
                ui,
            } => {
                assert_eq!(path.as_deref(), Some(std::path::Path::new("demo")));
                assert!(!feature_selection.enable_default);
                assert!(feature_selection.explicit.contains("tls"));
                assert!(feature_selection.explicit.contains("simd"));
                assert_eq!(
                    feature_selection.profile,
                    crate::script::ProfileSelection::Dev
                );
                assert_eq!(ui, UiOptions::default());
            }
            other => panic!("expected check command, got {other:?}"),
        }
    }

    #[test]
    fn parses_lock_with_inline_feature_option() {
        let cmd = parse_args(["lock".to_string(), "--features=ssl".to_string()]).unwrap();

        match cmd {
            Command::Lock {
                path,
                feature_selection,
                ui,
            } => {
                assert!(path.is_none());
                assert!(feature_selection.enable_default);
                assert_eq!(feature_selection.explicit.len(), 1);
                assert!(feature_selection.explicit.contains("ssl"));
                assert_eq!(ui, UiOptions::default());
            }
            other => panic!("expected lock command, got {other:?}"),
        }
    }

    #[test]
    fn parses_build_without_path() {
        let cmd = parse_args(["build".to_string()]).unwrap();

        match cmd {
            Command::Build {
                path,
                feature_selection,
                ui,
            } => {
                assert!(path.is_none());
                assert!(feature_selection.enable_default);
                assert!(feature_selection.explicit.is_empty());
                assert_eq!(
                    feature_selection.profile,
                    crate::script::ProfileSelection::Dev
                );
                assert_eq!(ui, UiOptions::default());
            }
            other => panic!("expected build command, got {other:?}"),
        }
    }

    #[test]
    fn parses_build_with_release_profile() {
        let cmd = parse_args([
            "build".to_string(),
            "--profile".to_string(),
            "release".to_string(),
        ])
        .unwrap();

        match cmd {
            Command::Build {
                path,
                feature_selection,
                ui,
            } => {
                assert!(path.is_none());
                assert!(feature_selection.enable_default);
                assert!(feature_selection.explicit.is_empty());
                assert_eq!(
                    feature_selection.profile,
                    crate::script::ProfileSelection::Release
                );
                assert_eq!(ui, UiOptions::default());
            }
            other => panic!("expected build command, got {other:?}"),
        }
    }

    #[test]
    fn parses_fetch_with_path() {
        let cmd = parse_args(["fetch".to_string(), "--project-path=demo".to_string()]).unwrap();

        match cmd {
            Command::Fetch {
                path,
                feature_selection,
                ui,
            } => {
                assert_eq!(path.as_deref(), Some(std::path::Path::new("demo")));
                assert!(feature_selection.enable_default);
                assert!(feature_selection.explicit.is_empty());
                assert_eq!(ui, UiOptions::default());
            }
            other => panic!("expected fetch command, got {other:?}"),
        }
    }

    #[test]
    fn parses_publish_and_forces_release_profile() {
        let cmd = parse_args([
            "publish".to_string(),
            "--project-path".to_string(),
            "demo".to_string(),
        ])
        .unwrap();

        match cmd {
            Command::Publish {
                path,
                feature_selection,
                ui,
            } => {
                assert_eq!(path.as_deref(), Some(std::path::Path::new("demo")));
                assert!(feature_selection.enable_default);
                assert!(feature_selection.explicit.is_empty());
                assert_eq!(
                    feature_selection.profile,
                    crate::script::ProfileSelection::Release
                );
                assert_eq!(ui, UiOptions::default());
            }
            other => panic!("expected publish command, got {other:?}"),
        }
    }

    #[test]
    fn parses_doc_with_verbose_output() {
        let cmd = parse_args(["doc".to_string(), "--verbose".to_string()]).unwrap();

        match cmd {
            Command::Doc {
                path,
                feature_selection,
                ui,
            } => {
                assert!(path.is_none());
                assert!(feature_selection.enable_default);
                assert!(feature_selection.explicit.is_empty());
                assert!(ui.verbose);
                assert_eq!(ui.color, ColorChoice::Auto);
            }
            other => panic!("expected doc command, got {other:?}"),
        }
    }

    #[test]
    fn parses_run_with_path() {
        let cmd = parse_args([
            "run".to_string(),
            "--project-path".to_string(),
            "demo".to_string(),
        ])
        .unwrap();

        match cmd {
            Command::Run {
                path,
                feature_selection,
                ui,
            } => {
                assert_eq!(path.as_deref(), Some(std::path::Path::new("demo")));
                assert!(feature_selection.enable_default);
                assert!(feature_selection.explicit.is_empty());
                assert_eq!(ui, UiOptions::default());
            }
            other => panic!("expected run command, got {other:?}"),
        }
    }

    #[test]
    fn parses_test_with_inline_feature_option() {
        let cmd = parse_args(["test".to_string(), "--features=simd".to_string()]).unwrap();

        match cmd {
            Command::Test {
                path,
                feature_selection,
                ui,
            } => {
                assert!(path.is_none());
                assert!(feature_selection.enable_default);
                assert_eq!(feature_selection.explicit.len(), 1);
                assert!(feature_selection.explicit.contains("simd"));
                assert_eq!(ui, UiOptions::default());
            }
            other => panic!("expected test command, got {other:?}"),
        }
    }

    #[test]
    fn parses_verbose_flag() {
        let cmd = parse_args(["build".to_string(), "--verbose".to_string()]).unwrap();

        match cmd {
            Command::Build { ui, .. } => {
                assert_eq!(
                    ui,
                    UiOptions {
                        verbose: true,
                        timings: false,
                        color: ColorChoice::Auto,
                    }
                );
            }
            other => panic!("expected build command, got {other:?}"),
        }
    }

    #[test]
    fn parses_short_verbose_and_color_always() {
        let cmd = parse_args([
            "build".to_string(),
            "-v".to_string(),
            "--color=always".to_string(),
        ])
        .unwrap();

        match cmd {
            Command::Build { ui, .. } => {
                assert_eq!(
                    ui,
                    UiOptions {
                        verbose: true,
                        timings: false,
                        color: ColorChoice::Always,
                    }
                );
            }
            other => panic!("expected build command, got {other:?}"),
        }
    }

    #[test]
    fn parses_timings_flag() {
        let cmd = parse_args(["build".to_string(), "--timings".to_string()]).unwrap();

        match cmd {
            Command::Build { ui, .. } => {
                assert_eq!(
                    ui,
                    UiOptions {
                        verbose: false,
                        timings: true,
                        color: ColorChoice::Auto,
                    }
                );
            }
            other => panic!("expected build command, got {other:?}"),
        }
    }

    #[test]
    fn parses_no_color_alias() {
        let cmd = parse_args(["check".to_string(), "--no-color".to_string()]).unwrap();

        match cmd {
            Command::Check { ui, .. } => {
                assert_eq!(
                    ui,
                    UiOptions {
                        verbose: false,
                        timings: false,
                        color: ColorChoice::Never,
                    }
                );
            }
            other => panic!("expected check command, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_color_choice() {
        let err = parse_args(["build".to_string(), "--color=rgb".to_string()]).unwrap_err();
        assert!(
            err.to_string().contains("expected auto, always, or never"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rejects_empty_feature_names() {
        let err = parse_args([
            "check".to_string(),
            "--features".to_string(),
            "simd,".to_string(),
        ])
        .unwrap_err();
        assert!(
            err.to_string().contains("must not be empty"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn summarizes_check_sources_by_backend() {
        let resolved = ResolvedGraph {
            workspace_root: PathBuf::from("."),
            packages: Vec::new(),
            external_packages: vec![
                ResolvedExternalPackage {
                    id: ExternalPackageId {
                        package_name: "log".to_string(),
                        source: SourceId::GitDependency {
                            git: "https://example.com/log.git".to_string(),
                            rev: None,
                            branch: Some("main".to_string()),
                            tag: None,
                        },
                        version: Some("1".to_string()),
                    },
                },
                ResolvedExternalPackage {
                    id: ExternalPackageId {
                        package_name: "net".to_string(),
                        source: SourceId::GitDependency {
                            git: "https://example.com/net.git".to_string(),
                            rev: None,
                            branch: None,
                            tag: Some("v2".to_string()),
                        },
                        version: Some("2".to_string()),
                    },
                },
                ResolvedExternalPackage {
                    id: ExternalPackageId {
                        package_name: "util".to_string(),
                        source: SourceId::PathDependency {
                            path: "../util".to_string(),
                        },
                        version: None,
                    },
                },
            ],
        };

        let summary = summarize_check_sources(&resolved);
        assert_eq!(summary.git_sources, 2);
        assert_eq!(summary.git_packages, 2);
        assert_eq!(summary.path_packages, 1);
    }

    #[test]
    fn summarize_source_security_respects_allowlists_and_warn_mode() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.7"

[craft]
release-source-policy = "warn"
allow-floating-git = ["default"]
allow-insecure-source = ["insecure"]

[workspace]
members = []

[workspace.dependencies]
default = { git = "https://example.com/default.git", branch = "main" }
insecure = { git = "http://example.com/insecure.git", branch = "main" }
blocked = { git = "https://example.com/blocked.git", branch = "main" }
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let summary = summarize_source_security(&manifest);
        assert_eq!(summary.policy_mode, ReleaseSourcePolicy::Warn);
        assert_eq!(summary.floating_git_sources, 3);
        assert_eq!(summary.insecure_transport_sources, 1);
        assert_eq!(
            summary.warnings,
            vec![
                "blocked(floating-git)".to_string(),
                "insecure(floating-git)".to_string(),
            ]
        );
        assert_eq!(
            summary.suppressed,
            vec![
                "default(floating-git)".to_string(),
                "insecure(insecure-transport)".to_string(),
            ]
        );
        assert!(summary.release_blockers().is_empty());
    }

    #[test]
    fn validate_check_source_policy_rejects_release_blockers() {
        let summary = super::SourceSecuritySummary {
            policy_mode: ReleaseSourcePolicy::Enforce,
            floating_git_sources: 1,
            insecure_transport_sources: 1,
            warnings: vec![
                "default(floating-git)".to_string(),
                "default(insecure-transport)".to_string(),
            ],
            suppressed: Vec::new(),
            release_blockers: vec![
                "default(floating-git)".to_string(),
                "default(insecure-transport)".to_string(),
            ],
        };
        let selection = FeatureSelection {
            profile: crate::script::ProfileSelection::Release,
            ..FeatureSelection::default()
        };

        let err =
            validate_check_source_policy(std::path::Path::new("Craft.toml"), &selection, &summary)
                .unwrap_err();
        assert!(err.to_string().contains("release source policy rejected"));
    }

    #[test]
    fn check_command_waits_for_workspace_lock() {
        let root = temp_dir("craft-cli-workspace-lock");
        write_minimal_bin_package(&root);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let root_for_holder = root.clone();

        let holder = thread::spawn(move || {
            let _lock = WorkspaceOperationLock::acquire(&root_for_holder, "build").unwrap();
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });

        ready_rx.recv().unwrap();
        let root_for_check = root.clone();
        let start = Instant::now();
        let waiter = thread::spawn(move || {
            run_command(Command::Check {
                path: Some(root_for_check),
                feature_selection: FeatureSelection::default(),
                ui: UiOptions::default(),
            })
            .unwrap();
            start.elapsed()
        });

        thread::sleep(Duration::from_millis(200));
        release_tx.send(()).unwrap();

        holder.join().unwrap();
        let waited = waiter.join().unwrap();
        assert!(waited >= Duration::from_millis(150));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_command_waits_for_workspace_lock() {
        let root = temp_dir("craft-cli-build-workspace-lock");
        write_minimal_bin_package(&root);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let root_for_holder = root.clone();

        let holder = thread::spawn(move || {
            let _lock = WorkspaceOperationLock::acquire(&root_for_holder, "test").unwrap();
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });

        ready_rx.recv().unwrap();
        let root_for_build = root.clone();
        let start = Instant::now();
        let waiter = thread::spawn(move || {
            run_command(Command::Build {
                path: Some(root_for_build),
                feature_selection: FeatureSelection::default(),
                ui: UiOptions::default(),
            })
            .unwrap();
            start.elapsed()
        });

        thread::sleep(Duration::from_millis(200));
        release_tx.send(()).unwrap();

        holder.join().unwrap();
        let waited = waiter.join().unwrap();
        assert!(waited >= Duration::from_millis(150));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn publish_requires_current_release_lock_and_metadata() {
        let root = temp_dir("craft-cli-publish");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.7"
description = "Demo package"
license = "MIT"
authors = ["Demo <demo@example.com>"]
readme = "README.md"
repository = "https://example.com/demo"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
        )
        .unwrap();
        fs::write(root.join("README.md"), "# demo\n").unwrap();
        fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();

        let err = run_command(Command::Publish {
            path: Some(root.clone()),
            feature_selection: FeatureSelection {
                profile: crate::script::ProfileSelection::Release,
                ..Default::default()
            },
            ui: UiOptions::default(),
        })
        .unwrap_err();
        assert!(err.to_string().contains("craft lock --profile release"));
        assert!(!root.join("Craft.lock").exists());

        run_command(Command::Lock {
            path: Some(root.clone()),
            feature_selection: FeatureSelection {
                profile: crate::script::ProfileSelection::Release,
                ..Default::default()
            },
            ui: UiOptions::default(),
        })
        .unwrap();

        run_command(Command::Publish {
            path: Some(root.clone()),
            feature_selection: FeatureSelection {
                profile: crate::script::ProfileSelection::Release,
                ..Default::default()
            },
            ui: UiOptions::default(),
        })
        .unwrap();

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn publish_accepts_workspace_package_metadata_for_members() {
        let root = temp_dir("craft-cli-publish-workspace");
        let member = root.join("member");
        fs::create_dir_all(member.join("src")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
members = ["member"]

[workspace.package]
description = "Shared package metadata"
license = "MIT"
authors = ["Demo <demo@example.com>"]
readme = "README.md"
repository = "https://example.com/workspace"
"#,
        )
        .unwrap();
        fs::write(root.join("README.md"), "# workspace\n").unwrap();
        fs::write(
            member.join("Craft.toml"),
            r#"
[package]
name = "member"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "member"
root = "src/main.rn"
"#,
        )
        .unwrap();
        fs::write(member.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();

        run_command(Command::Lock {
            path: Some(root.clone()),
            feature_selection: FeatureSelection {
                profile: crate::script::ProfileSelection::Release,
                ..Default::default()
            },
            ui: UiOptions::default(),
        })
        .unwrap();

        run_command(Command::Publish {
            path: Some(root.clone()),
            feature_selection: FeatureSelection {
                profile: crate::script::ProfileSelection::Release,
                ..Default::default()
            },
            ui: UiOptions::default(),
        })
        .unwrap();

        let _ = fs::remove_dir_all(root);
    }
}
