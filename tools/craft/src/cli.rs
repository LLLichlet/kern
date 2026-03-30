use crate::build_plan;
use crate::discover;
use crate::elaborate;
use crate::error::{Error, Result};
use crate::execute;
use crate::graph;
use crate::lockfile;
use crate::manifest::Manifest;
use crate::plan::TargetKind;
use crate::source;
use crate::workspace;
use std::env;
use std::path::PathBuf;

#[derive(Debug)]
pub enum Command {
    Help,
    Check {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
    },
    Lock {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
    },
    Fetch {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
    },
    Build {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
    },
    Run {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
    },
    Test {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
    },
}

pub fn run() -> Result<()> {
    match parse_args(env::args().skip(1))? {
        Command::Help => {
            print!("{}", usage());
            Ok(())
        }
        Command::Check {
            path,
            feature_selection,
        } => {
            let loaded = load_package_graph(
                path.as_deref(),
                crate::script::ScriptCommand::Check,
                &feature_selection,
            )?;
            let lock_status = lockfile::lock_status(&loaded.manifest_path, &loaded.elaboration)?;
            let build_plan =
                build_plan::derive(&loaded.elaboration, crate::script::ScriptCommand::Check)?;
            let action_plan = build_plan.derive_actions(&crate::script::host_target());

            println!("checked {}", loaded.manifest_path.display());
            if let Some(package) = &loaded.manifest.package {
                println!("package: {} {}", package.name, package.version);
            } else {
                println!("package: <none>");
            }
            println!(
                "feature inputs: {}",
                format_feature_inputs(&feature_selection)
            );
            if let Some(workspace) = &loaded.manifest.workspace {
                println!("workspace members: {}", workspace.members.len());
            } else {
                println!("workspace members: 0");
            }
            println!(
                "validated workspace members: {}",
                loaded.workspace_members.len()
            );
            if !loaded.workspace_members.is_empty() {
                let member_names = loaded
                    .workspace_members
                    .iter()
                    .map(|member| {
                        member
                            .manifest
                            .package
                            .as_ref()
                            .map(|package| package.name.as_str())
                            .unwrap_or("<workspace>")
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("member packages: {member_names}");
                println!(
                    "member manifests: {}",
                    loaded
                        .workspace_members
                        .iter()
                        .map(|member| member.manifest_path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            let edge_count = loaded
                .package_graph
                .packages
                .iter()
                .map(|pkg| pkg.dependencies.len())
                .sum::<usize>();
            println!(
                "graph: packages={} dependency_edges={}",
                loaded.package_graph.packages.len(),
                edge_count
            );
            println!(
                "resolved: local_packages={} external_packages={}",
                loaded.elaboration.resolved_graph.packages.len(),
                loaded.elaboration.resolved_graph.external_packages.len()
            );
            println!(
                "targets: lib={} bin={} test={} example={}",
                usize::from(loaded.manifest.lib.is_some()),
                loaded.manifest.bin.len(),
                loaded.manifest.test.len(),
                loaded.manifest.example.len()
            );
            println!(
                "normalized package targets: {}",
                loaded.elaboration.package_target_count()
            );
            println!(
                "dependencies: normal={} dev={} build={}",
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
            println!(
                "scripts: workspace_craft={} package_craft={} build_rn={}",
                if loaded.elaboration.workspace_script.is_some() {
                    "yes"
                } else {
                    "no"
                },
                loaded.elaboration.package_script_count(),
                build_plan.build_script_count()
            );
            println!(
                "env inputs: workspace={} package={}",
                loaded.elaboration.workspace_env_input_count(),
                loaded.elaboration.package_env_input_count()
            );
            println!(
                "build plan: units={} compile_actions={} link_actions={} target_local_edges={} target_external_edges={} build_local_edges={} build_external_edges={} generated_files={} staged_actions={} link_libs={} link_frameworks={} link_searches={} link_args={}",
                build_plan.unit_count(),
                action_plan.compile_count(),
                action_plan.link_count(),
                build_plan.local_dependency_edge_count(),
                build_plan.external_dependency_edge_count(),
                build_plan.build_local_dependency_edge_count(),
                build_plan.build_external_dependency_edge_count(),
                build_plan.generated_file_count(),
                build_plan.staged_action_count(),
                build_plan.link_system_lib_count(),
                build_plan.link_framework_count(),
                build_plan.link_search_path_count(),
                build_plan.link_arg_count()
            );
            println!(
                "lockfile: {}",
                match lock_status {
                    lockfile::LockStatus::Missing => "missing",
                    lockfile::LockStatus::Current => "current",
                    lockfile::LockStatus::Stale => "stale",
                }
            );
            print_generated_files(&build_plan);

            Ok(())
        }
        Command::Lock {
            path,
            feature_selection,
        } => {
            let loaded = load_package_graph(
                path.as_deref(),
                crate::script::ScriptCommand::Lock,
                &feature_selection,
            )?;
            let (lock_path, lock_result) =
                lockfile::sync_lockfile(&loaded.manifest_path, &loaded.elaboration)?;
            let edge_count = loaded
                .package_graph
                .packages
                .iter()
                .map(|pkg| pkg.dependencies.len())
                .sum::<usize>();

            println!(
                "{} {}",
                match lock_result {
                    lockfile::LockWriteResult::Created => "created",
                    lockfile::LockWriteResult::Updated => "updated",
                    lockfile::LockWriteResult::Unchanged => "unchanged",
                },
                lock_path.display()
            );
            println!(
                "graph: packages={} dependency_edges={}",
                loaded.package_graph.packages.len(),
                edge_count
            );
            println!(
                "resolved: local_packages={} external_packages={}",
                loaded.elaboration.resolved_graph.packages.len(),
                loaded.elaboration.resolved_graph.external_packages.len()
            );

            Ok(())
        }
        Command::Fetch {
            path,
            feature_selection,
        } => {
            let loaded = load_package_graph(
                path.as_deref(),
                crate::script::ScriptCommand::Fetch,
                &feature_selection,
            )?;
            let fetched = source::fetch_external_packages(
                &loaded.manifest_path,
                &loaded.manifest,
                &loaded.elaboration.resolved_graph,
            )?;
            let summary = source::summarize_fetch(&fetched);

            println!("fetched {}", loaded.manifest_path.display());
            println!(
                "feature inputs: {}",
                format_feature_inputs(&feature_selection)
            );
            println!(
                "external packages: {} created={} updated={} unchanged={}",
                fetched.len(),
                summary.created,
                summary.updated,
                summary.unchanged
            );
            for package in &fetched {
                println!(
                    "source: {} from={} cache={}",
                    format_external_package_label(&package.id),
                    package.source_path.display(),
                    package.cache_path.display()
                );
            }

            Ok(())
        }
        Command::Build {
            path,
            feature_selection,
        } => {
            let loaded = load_package_graph(
                path.as_deref(),
                crate::script::ScriptCommand::Build,
                &feature_selection,
            )?;
            let build_plan =
                build_plan::derive(&loaded.elaboration, crate::script::ScriptCommand::Build)?;
            let target = crate::script::host_target();
            let action_plan = build_plan.derive_actions(&target);

            println!("planned build {}", loaded.manifest_path.display());
            println!(
                "feature inputs: {}",
                format_feature_inputs(&feature_selection)
            );
            println!(
                "build plan: units={} libs={} bins={} tests={} examples={}",
                build_plan.unit_count(),
                count_units_of_kind(&build_plan, TargetKind::Lib),
                count_units_of_kind(&build_plan, TargetKind::Bin),
                count_units_of_kind(&build_plan, TargetKind::Test),
                count_units_of_kind(&build_plan, TargetKind::Example),
            );
            println!(
                "dependencies: target_local_edges={} target_external_edges={} build_local_edges={} build_external_edges={}",
                build_plan.local_dependency_edge_count(),
                build_plan.external_dependency_edge_count(),
                build_plan.build_local_dependency_edge_count(),
                build_plan.build_external_dependency_edge_count()
            );
            println!(
                "build scripts: {} compile_actions={} link_actions={} generated_files={} staged_actions={} link_libs={} link_frameworks={} link_searches={} link_args={}",
                build_plan.build_script_count(),
                action_plan.compile_count(),
                action_plan.link_count(),
                build_plan.generated_file_count(),
                build_plan.staged_action_count(),
                build_plan.link_system_lib_count(),
                build_plan.link_framework_count(),
                build_plan.link_search_path_count(),
                build_plan.link_arg_count()
            );
            print_generated_files(&build_plan);
            print_compile_actions(&action_plan);
            print_link_actions(&action_plan);
            let execution = execute::build(&build_plan, &action_plan)?;
            println!(
                "executed build: compile_actions={} link_actions={}",
                execution.compile_actions, execution.link_actions
            );

            Ok(())
        }
        Command::Run {
            path,
            feature_selection,
        } => {
            let loaded = load_package_graph(
                path.as_deref(),
                crate::script::ScriptCommand::Run,
                &feature_selection,
            )?;
            let build_plan =
                build_plan::derive(&loaded.elaboration, crate::script::ScriptCommand::Run)?;
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

            println!("planned run {}", loaded.manifest_path.display());
            println!(
                "feature inputs: {}",
                format_feature_inputs(&feature_selection)
            );
            println!("run target: {}", format_unit_label(run_unit));
            println!(
                "build plan: units={} target_local_edges={} target_external_edges={} build_local_edges={} build_external_edges={} generated_files={} staged_actions={} link_libs={} link_frameworks={} link_searches={} link_args={}",
                build_plan.unit_count(),
                build_plan.local_dependency_edge_count(),
                build_plan.external_dependency_edge_count(),
                build_plan.build_local_dependency_edge_count(),
                build_plan.build_external_dependency_edge_count(),
                build_plan.generated_file_count(),
                build_plan.staged_action_count(),
                build_plan.link_system_lib_count(),
                build_plan.link_framework_count(),
                build_plan.link_search_path_count(),
                build_plan.link_arg_count()
            );
            print_generated_files_for_unit(run_unit);
            print_compile_actions_for_unit(&action_plan, run_unit);
            print_link_actions_for_unit(&action_plan, run_unit);
            let execution = execute::run(&build_plan, &action_plan, run_unit)?;
            println!("executed run: {}", execution.executable.display());

            Ok(())
        }
        Command::Test {
            path,
            feature_selection,
        } => {
            let loaded = load_package_graph(
                path.as_deref(),
                crate::script::ScriptCommand::Test,
                &feature_selection,
            )?;
            let build_plan =
                build_plan::derive(&loaded.elaboration, crate::script::ScriptCommand::Test)?;
            let action_plan = build_plan.derive_actions(&crate::script::host_target());
            let tests = units_of_kind(&build_plan, TargetKind::Test);

            println!("planned test {}", loaded.manifest_path.display());
            println!(
                "feature inputs: {}",
                format_feature_inputs(&feature_selection)
            );
            println!("test units: {}", tests.len());
            if !tests.is_empty() {
                println!(
                    "test targets: {}",
                    tests
                        .iter()
                        .map(|unit| format_unit_label(unit))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            println!(
                "build plan: units={} target_local_edges={} target_external_edges={} build_local_edges={} build_external_edges={} generated_files={} staged_actions={} link_libs={} link_frameworks={} link_searches={} link_args={}",
                build_plan.unit_count(),
                build_plan.local_dependency_edge_count(),
                build_plan.external_dependency_edge_count(),
                build_plan.build_local_dependency_edge_count(),
                build_plan.build_external_dependency_edge_count(),
                build_plan.generated_file_count(),
                build_plan.staged_action_count(),
                build_plan.link_system_lib_count(),
                build_plan.link_framework_count(),
                build_plan.link_search_path_count(),
                build_plan.link_arg_count()
            );
            for unit in &tests {
                print_generated_files_for_unit(unit);
                print_compile_actions_for_unit(&action_plan, unit);
                print_link_actions_for_unit(&action_plan, unit);
            }
            let execution = execute::test(&build_plan, &action_plan, &tests)?;
            println!("executed tests: {}", execution.executed);

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

fn load_package_graph(
    path: Option<&std::path::Path>,
    command: crate::script::ScriptCommand,
    feature_selection: &elaborate::FeatureSelection,
) -> Result<LoadedPackageGraph> {
    let manifest_path = discover::resolve_manifest_path(path)?;
    let manifest = Manifest::load(&manifest_path)?;
    manifest.validate(&manifest_path)?;
    let workspace_members = workspace::load_members(&manifest_path, &manifest)?;
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

    Ok(LoadedPackageGraph {
        manifest_path,
        manifest,
        workspace_members,
        package_graph,
        elaboration,
    })
}

fn format_feature_inputs(selection: &elaborate::FeatureSelection) -> String {
    format!(
        "default={} explicit={}",
        if selection.enable_default {
            "yes"
        } else {
            "no"
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

fn count_units_of_kind(plan: &build_plan::BuildPlan, kind: TargetKind) -> usize {
    plan.packages
        .iter()
        .flat_map(|package| &package.units)
        .filter(|unit| unit.target_kind == kind)
        .count()
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
        "{}:{} ({},{})",
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

fn format_action_label(
    package_id: &crate::graph::PackageId,
    domain: crate::graph::BuildDomain,
    target_kind: TargetKind,
    artifact_name: &str,
) -> String {
    format!(
        "{}:{} ({},{})",
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

fn print_compile_actions(action_plan: &build_plan::ActionPlan) {
    for action in &action_plan.compile_actions {
        println!(
            "compile: {} src={} obj={} out={} cfg={} define={}",
            format_action_label(
                &action.package_id,
                action.domain,
                action.target_kind,
                &action.artifact_name
            ),
            action.source_path.display(),
            action.object_path.display(),
            action.artifact_path.display(),
            format_plan_map(&action.cfg),
            format_plan_map(&action.define)
        );
    }
}

fn print_generated_files(build_plan: &build_plan::BuildPlan) {
    for package in &build_plan.packages {
        for unit in &package.units {
            print_generated_files_for_unit(unit);
        }
    }
}

fn print_generated_files_for_unit(unit: &build_plan::BuildUnit) {
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
        .join(",");
    println!("generated: {} files={}", format_unit_label(unit), files);
    if !unit.staged_actions.is_empty() {
        let staged = unit
            .staged_actions
            .iter()
            .map(|action| match &action.kind {
                build_plan::StagedActionKind::WriteFile { .. } => {
                    format!(
                        "{}:write:{}",
                        format_stage_phase(action.phase),
                        action.output
                    )
                }
                build_plan::StagedActionKind::RunTool { tool, args } => {
                    format!(
                        "{}:run_tool:{}<= {}({})",
                        format_stage_phase(action.phase),
                        action.output,
                        tool.executable_path,
                        args.join(" ")
                    )
                }
                build_plan::StagedActionKind::CopyFile { source } => {
                    format!(
                        "{}:copy:{}<= {}",
                        format_stage_phase(action.phase),
                        action.output,
                        source
                    )
                }
                build_plan::StagedActionKind::CopyDirectory { source } => {
                    format!(
                        "{}:copy_dir:{}<= {}",
                        format_stage_phase(action.phase),
                        action.output,
                        source
                    )
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        println!("staged: {} actions={}", format_unit_label(unit), staged);
    }
}

fn format_stage_phase(phase: build_plan::StagedActionPhase) -> &'static str {
    match phase {
        build_plan::StagedActionPhase::PreCompile => "pre_compile",
        build_plan::StagedActionPhase::PostLink => "post_link",
    }
}

fn print_link_actions(action_plan: &build_plan::ActionPlan) {
    for action in &action_plan.link_actions {
        print_link_action(action);
    }
}

fn print_compile_actions_for_unit(
    action_plan: &build_plan::ActionPlan,
    unit: &build_plan::BuildUnit,
) {
    for action in action_plan.compile_actions.iter().filter(|action| {
        action.domain == unit.domain
            && action.package_id == unit.package_id
            && action.target_kind == unit.target_kind
            && action.target_name == unit.target_name
    }) {
        println!(
            "compile: {} src={} obj={} out={} cfg={} define={}",
            format_action_label(
                &action.package_id,
                action.domain,
                action.target_kind,
                &unit.artifact_name
            ),
            action.source_path.display(),
            action.object_path.display(),
            action.artifact_path.display(),
            format_plan_map(&action.cfg),
            format_plan_map(&action.define)
        );
    }
}

fn print_link_actions_for_unit(action_plan: &build_plan::ActionPlan, unit: &build_plan::BuildUnit) {
    for action in action_plan.link_actions.iter().filter(|action| {
        action.domain == unit.domain
            && action.package_id == unit.package_id
            && action.target_kind == unit.target_kind
            && action.target_name == unit.target_name
    }) {
        print_link_action(action);
    }
}

fn print_link_action(action: &build_plan::LinkAction) {
    println!(
        "link: {} obj={} locals={} externals={} out={} system_libs={} frameworks={} search_paths={} args={}",
        format_action_label(
            &action.package_id,
            action.domain,
            action.target_kind,
            &action.artifact_name
        ),
        action.primary_object.display(),
        if action.local_library_objects.is_empty() {
            "<none>".to_string()
        } else {
            action
                .local_library_objects
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(",")
        },
        if action.external_dependencies.is_empty() {
            "<none>".to_string()
        } else {
            action
                .external_dependencies
                .iter()
                .map(|dep| dep.package_id.package_name.as_str())
                .collect::<Vec<_>>()
                .join(",")
        },
        action.artifact_path.display(),
        if action.link.system_libs.is_empty() {
            "<none>".to_string()
        } else {
            action.link.system_libs.join(",")
        },
        if action.link.frameworks.is_empty() {
            "<none>".to_string()
        } else {
            action.link.frameworks.join(",")
        },
        if action.link.search_paths.is_empty() {
            "<none>".to_string()
        } else {
            action.link.search_paths.join(",")
        },
        if action.link.args.is_empty() {
            "<none>".to_string()
        } else {
            action.link.args.join(",")
        }
    );
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

    let (path, feature_selection) = parse_command_options(rest)?;
    match cmd.as_str() {
        "check" => Ok(Command::Check {
            path,
            feature_selection,
        }),
        "lock" => Ok(Command::Lock {
            path,
            feature_selection,
        }),
        "fetch" => Ok(Command::Fetch {
            path,
            feature_selection,
        }),
        "build" => Ok(Command::Build {
            path,
            feature_selection,
        }),
        "run" => Ok(Command::Run {
            path,
            feature_selection,
        }),
        "test" => Ok(Command::Test {
            path,
            feature_selection,
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
) -> Result<(Option<PathBuf>, elaborate::FeatureSelection)> {
    let mut path: Option<PathBuf> = None;
    let mut feature_selection = elaborate::FeatureSelection::default();
    let mut idx = 0;

    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--no-default-features" {
            feature_selection.enable_default = false;
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
        if path.is_some() {
            return Err(Error::Usage(format!(
                "multiple package paths provided: `{}` and `{arg}`",
                path.as_ref().unwrap().display()
            )));
        }
        path = Some(PathBuf::from(arg));
        idx += 1;
    }

    Ok((path, feature_selection))
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
craft - Kern package manager and builder

USAGE:
    craft help
    craft check [PATH] [--no-default-features] [--features <FEATURES>]
    craft lock [PATH] [--no-default-features] [--features <FEATURES>]
    craft fetch [PATH] [--no-default-features] [--features <FEATURES>]
    craft build [PATH] [--no-default-features] [--features <FEATURES>]
    craft run [PATH] [--no-default-features] [--features <FEATURES>]
    craft test [PATH] [--no-default-features] [--features <FEATURES>]

COMMANDS:
    help         Show this help text
    check        Discover, parse, and validate Craft.toml
    lock         Write a deterministic Craft.lock from the current package graph
    fetch        Materialize external package sources into the local craft cache
    build        Derive the build plan for the selected package graph
    run          Select the runnable bin target from the current build plan
    test         Derive the test build plan for the selected package graph

OPTIONS:
    --no-default-features    Disable the implicit `default` feature
    --features <FEATURES>    Enable a comma-separated feature list
"
}

#[cfg(test)]
mod tests {
    use super::{Command, parse_args};

    #[test]
    fn parses_check_with_path_and_feature_options() {
        let cmd = parse_args([
            "check".to_string(),
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
            } => {
                assert_eq!(path.as_deref(), Some(std::path::Path::new("demo")));
                assert!(!feature_selection.enable_default);
                assert!(feature_selection.explicit.contains("tls"));
                assert!(feature_selection.explicit.contains("simd"));
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
            } => {
                assert!(path.is_none());
                assert!(feature_selection.enable_default);
                assert_eq!(feature_selection.explicit.len(), 1);
                assert!(feature_selection.explicit.contains("ssl"));
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
            } => {
                assert!(path.is_none());
                assert!(feature_selection.enable_default);
                assert!(feature_selection.explicit.is_empty());
            }
            other => panic!("expected build command, got {other:?}"),
        }
    }

    #[test]
    fn parses_fetch_with_path() {
        let cmd = parse_args(["fetch".to_string(), "demo".to_string()]).unwrap();

        match cmd {
            Command::Fetch {
                path,
                feature_selection,
            } => {
                assert_eq!(path.as_deref(), Some(std::path::Path::new("demo")));
                assert!(feature_selection.enable_default);
                assert!(feature_selection.explicit.is_empty());
            }
            other => panic!("expected fetch command, got {other:?}"),
        }
    }

    #[test]
    fn parses_run_with_path() {
        let cmd = parse_args(["run".to_string(), "demo".to_string()]).unwrap();

        match cmd {
            Command::Run {
                path,
                feature_selection,
            } => {
                assert_eq!(path.as_deref(), Some(std::path::Path::new("demo")));
                assert!(feature_selection.enable_default);
                assert!(feature_selection.explicit.is_empty());
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
            } => {
                assert!(path.is_none());
                assert!(feature_selection.enable_default);
                assert_eq!(feature_selection.explicit.len(), 1);
                assert!(feature_selection.explicit.contains("simd"));
            }
            other => panic!("expected test command, got {other:?}"),
        }
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
}
