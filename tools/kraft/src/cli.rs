use crate::build_plan;
use crate::discover;
use crate::elaborate;
use crate::error::{Error, Result};
use crate::graph;
use crate::lockfile;
use crate::manifest::Manifest;
use crate::workspace;
use std::env;
use std::path::{Path, PathBuf};

pub enum Command {
    Help,
    Check { path: Option<PathBuf> },
    Lock { path: Option<PathBuf> },
}

pub fn run() -> Result<()> {
    match parse_args(env::args().skip(1))? {
        Command::Help => {
            print!("{}", usage());
            Ok(())
        }
        Command::Check { path } => {
            let loaded = load_package_graph(path.as_deref())?;
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
        Command::Lock { path } => {
            let loaded = load_package_graph(path.as_deref())?;
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
    }
}

struct LoadedPackageGraph {
    manifest_path: PathBuf,
    manifest: Manifest,
    workspace_members: Vec<workspace::WorkspaceMember>,
    package_graph: graph::PackageGraph,
    elaboration: elaborate::ElaborationPlan,
}

fn load_package_graph(path: Option<&Path>) -> Result<LoadedPackageGraph> {
    let manifest_path = discover::resolve_manifest_path(path)?;
    let manifest = Manifest::load(&manifest_path)?;
    manifest.validate(&manifest_path)?;
    let workspace_members = workspace::load_members(&manifest_path, &manifest)?;
    let elaboration = elaborate::plan(
        &manifest_path,
        &manifest,
        &workspace_members,
        manifest.workspace.is_some(),
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

fn parse_args<I>(args: I) -> Result<Command>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    match args.as_slice() {
        [] => Ok(Command::Help),
        [cmd] if cmd == "help" || cmd == "--help" || cmd == "-h" => Ok(Command::Help),
        [cmd] if cmd == "check" => Ok(Command::Check { path: None }),
        [cmd, path] if cmd == "check" => Ok(Command::Check {
            path: Some(PathBuf::from(path)),
        }),
        [cmd] if cmd == "lock" => Ok(Command::Lock { path: None }),
        [cmd, path] if cmd == "lock" => Ok(Command::Lock {
            path: Some(PathBuf::from(path)),
        }),
        _ => Err(Error::Usage(format!(
            "unsupported command line: {}\n\n{}",
            args.join(" "),
            usage()
        ))),
    }
}

fn usage() -> &'static str {
    "\
kraft - Kern package manager and builder

USAGE:
    kraft help
    kraft check [PATH]
    kraft lock [PATH]

COMMANDS:
    help         Show this help text
    check        Discover, parse, and validate Kraft.toml
    lock         Write a deterministic Kraft.lock from the current package graph
"
}
