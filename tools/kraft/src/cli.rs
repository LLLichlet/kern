use crate::build_plan;
use crate::discover;
use crate::elaborate;
use crate::error::{Error, Result};
use crate::graph;
use crate::lockfile;
use crate::manifest::Manifest;
use crate::plan::TargetKind;
use crate::workspace;
use std::env;
use std::path::{Path, PathBuf};

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
            let build_plan = build_plan::derive(&loaded.elaboration)?;

            let package_root = loaded
                .manifest_path
                .parent()
                .unwrap_or_else(|| Path::new("."));
            let build_script = package_root.join("build.kr");

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
                "scripts: workspace_kraft={} package_kraft={} build.kr={}",
                if loaded.elaboration.workspace_script.is_some() {
                    "yes"
                } else {
                    "no"
                },
                loaded.elaboration.package_script_count(),
                if build_script.is_file() { "yes" } else { "no" }
            );
            println!(
                "env inputs: workspace={} package={}",
                loaded.elaboration.workspace_env_input_count(),
                loaded.elaboration.package_env_input_count()
            );
            println!(
                "build plan: units={} local_edges={} external_edges={}",
                build_plan.unit_count(),
                build_plan.local_dependency_edge_count(),
                build_plan.external_dependency_edge_count()
            );
            println!(
                "lockfile: {}",
                match lock_status {
                    lockfile::LockStatus::Missing => "missing",
                    lockfile::LockStatus::Current => "current",
                    lockfile::LockStatus::Stale => "stale",
                }
            );

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
        Command::Build {
            path,
            feature_selection,
        } => {
            let loaded = load_package_graph(
                path.as_deref(),
                crate::script::ScriptCommand::Build,
                &feature_selection,
            )?;
            let build_plan = build_plan::derive(&loaded.elaboration)?;

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
                "dependencies: local_edges={} external_edges={}",
                build_plan.local_dependency_edge_count(),
                build_plan.external_dependency_edge_count()
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
            let build_plan = build_plan::derive(&loaded.elaboration)?;
            let runnable = units_of_kind(&build_plan, TargetKind::Bin);

            let run_unit = match runnable.as_slice() {
                [] => {
                    return Err(Error::Usage(
                        "`kraft run` requires exactly one runnable `bin` target, but none were found"
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
                        "`kraft run` requires exactly one runnable `bin` target, but found {}: {}",
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
                "build plan: units={} local_edges={} external_edges={}",
                build_plan.unit_count(),
                build_plan.local_dependency_edge_count(),
                build_plan.external_dependency_edge_count()
            );

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
            let build_plan = build_plan::derive(&loaded.elaboration)?;
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
                "build plan: units={} local_edges={} external_edges={}",
                build_plan.unit_count(),
                build_plan.local_dependency_edge_count(),
                build_plan.external_dependency_edge_count()
            );

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
    path: Option<&Path>,
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
        "{}:{} ({})",
        unit.package_id.name,
        unit.artifact_name,
        unit.target_kind.as_str()
    )
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
kraft - Kern package manager and builder

USAGE:
    kraft help
    kraft check [PATH] [--no-default-features] [--features <FEATURES>]
    kraft lock [PATH] [--no-default-features] [--features <FEATURES>]
    kraft build [PATH] [--no-default-features] [--features <FEATURES>]
    kraft run [PATH] [--no-default-features] [--features <FEATURES>]
    kraft test [PATH] [--no-default-features] [--features <FEATURES>]

COMMANDS:
    help         Show this help text
    check        Discover, parse, and validate Kraft.toml
    lock         Write a deterministic Kraft.lock from the current package graph
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
