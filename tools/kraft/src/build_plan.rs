use crate::elaborate::ElaborationPlan;
use crate::error::Result;
use crate::graph::PackageId;
use crate::plan::TargetKind;
use crate::resolver::{ExternalPackageId, ResolvedDependencyTarget};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub workspace_root: PathBuf,
    pub packages: Vec<PackageBuildPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageBuildPlan {
    pub package_id: PackageId,
    pub units: Vec<BuildUnit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildUnit {
    pub package_id: PackageId,
    pub target_kind: TargetKind,
    pub target_name: Option<String>,
    pub source_root: String,
    pub artifact_kind: ArtifactKind,
    pub artifact_name: String,
    pub local_dependencies: Vec<PackageId>,
    pub external_dependencies: Vec<ExternalPackageId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    Library,
    Executable,
}

impl BuildPlan {
    pub fn unit_count(&self) -> usize {
        self.packages.iter().map(|package| package.units.len()).sum()
    }

    pub fn local_dependency_edge_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.local_dependencies.len())
            .sum()
    }

    pub fn external_dependency_edge_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.external_dependencies.len())
            .sum()
    }
}

pub fn derive(elaboration: &ElaborationPlan) -> Result<BuildPlan> {
    let mut packages = Vec::new();

    for package in &elaboration.resolved_graph.packages {
        let package_elaboration = elaboration
            .packages
            .iter()
            .find(|entry| entry.package_id == package.id)
            .expect("elaboration must contain package plan");
        let mut local_dependencies = Vec::new();
        let mut external_dependencies = Vec::new();
        for dependency in &package.dependencies {
            match &dependency.target {
                ResolvedDependencyTarget::Local(target) => {
                    if !local_dependencies.contains(target) {
                        local_dependencies.push(target.clone());
                    }
                }
                ResolvedDependencyTarget::External(target) => {
                    if !external_dependencies.contains(target) {
                        external_dependencies.push(target.clone());
                    }
                }
            }
        }

        let mut units = Vec::new();
        for target in &package_elaboration.plan.targets {
            units.push(BuildUnit {
                package_id: package.id.clone(),
                target_kind: target.kind,
                target_name: target.name.clone(),
                source_root: target.root.clone(),
                artifact_kind: artifact_kind(target.kind),
                artifact_name: artifact_name(&package.id, target.kind, target.name.as_deref()),
                local_dependencies: local_dependencies.clone(),
                external_dependencies: external_dependencies.clone(),
            });
        }

        packages.push(PackageBuildPlan {
            package_id: package.id.clone(),
            units,
        });
    }

    Ok(BuildPlan {
        workspace_root: elaboration.resolved_graph.workspace_root.clone(),
        packages,
    })
}

fn artifact_kind(kind: TargetKind) -> ArtifactKind {
    match kind {
        TargetKind::Lib => ArtifactKind::Library,
        TargetKind::Bin | TargetKind::Test | TargetKind::Example => ArtifactKind::Executable,
    }
}

fn artifact_name(package_id: &PackageId, kind: TargetKind, target_name: Option<&str>) -> String {
    match kind {
        TargetKind::Lib => package_id.name.clone(),
        TargetKind::Bin | TargetKind::Test | TargetKind::Example => target_name
            .expect("named targets must provide a name")
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{ArtifactKind, derive};
    use crate::elaborate::plan;
    use crate::graph::build_graph;
    use crate::manifest::Manifest;
    use crate::plan::TargetKind;
    use crate::resolver::resolve_graph;
    use crate::workspace::load_members;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn derives_workspace_build_units_from_package_targets() {
        let root = temp_dir("kraft-build-plan-targets");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Kraft.toml"),
            r#"
[workspace]
members = ["app"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Kraft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[lib]
root = "src/lib.kr"

[[bin]]
name = "app"
root = "src/main.kr"

[[test]]
name = "smoke"
root = "tests/smoke.kr"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &manifest).unwrap();
        let graph = build_graph(&manifest_path, &manifest, &members).unwrap();
        let resolved = resolve_graph(&graph);
        let elaboration = plan(&manifest_path, &manifest, &members, true, &resolved).unwrap();
        let build_plan = derive(&elaboration).unwrap();

        assert_eq!(build_plan.unit_count(), 3);
        let app_package = build_plan
            .packages
            .iter()
            .find(|package| package.package_id.name == "app")
            .unwrap();
        assert!(app_package.units.iter().any(|unit| {
            unit.target_kind == TargetKind::Lib
                && unit.artifact_kind == ArtifactKind::Library
                && unit.artifact_name == "app"
        }));
        assert!(app_package.units.iter().any(|unit| {
            unit.target_kind == TargetKind::Bin
                && unit.artifact_kind == ArtifactKind::Executable
                && unit.artifact_name == "app"
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn carries_local_and_external_dependencies_into_build_units() {
        let root = temp_dir("kraft-build-plan-deps");
        let app_dir = root.join("app");
        let util_dir = root.join("util");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&util_dir).unwrap();

        fs::write(
            root.join("Kraft.toml"),
            r#"
[workspace]
members = ["app", "util"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Kraft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "app"
root = "src/main.kr"

[dependencies]
util = { path = "../util" }
log = "1"
"#,
        )
        .unwrap();
        fs::write(
            util_dir.join("Kraft.toml"),
            r#"
[package]
name = "util"
version = "0.1.0"
kern = "0.7"

[lib]
root = "src/lib.kr"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &manifest).unwrap();
        let graph = build_graph(&manifest_path, &manifest, &members).unwrap();
        let resolved = resolve_graph(&graph);
        let elaboration = plan(&manifest_path, &manifest, &members, true, &resolved).unwrap();
        let build_plan = derive(&elaboration).unwrap();

        let app_unit = build_plan
            .packages
            .iter()
            .find(|package| package.package_id.name == "app")
            .unwrap()
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Bin)
            .unwrap();
        assert_eq!(app_unit.local_dependencies.len(), 1);
        assert_eq!(app_unit.local_dependencies[0].name, "util");
        assert_eq!(app_unit.external_dependencies.len(), 1);
        assert_eq!(app_unit.external_dependencies[0].package_name, "log");
        assert_eq!(build_plan.local_dependency_edge_count(), 1);
        assert_eq!(build_plan.external_dependency_edge_count(), 1);

        let _ = fs::remove_dir_all(root);
    }
}
