//! Dependency source resolver for package graphs.
//!
//! Resolution assigns stable external package identifiers, deduplicates shared
//! git/path sources, and records local versus external source origins for later
//! fetching and build planning.

use crate::graph::{
    BuildDomain, DependencyKind, DependencyTarget, ExternalDependency, PackageGraph, PackageId,
    SourceId,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGraph {
    pub workspace_root: PathBuf,
    pub packages: Vec<ResolvedPackageNode>,
    pub external_packages: Vec<ResolvedExternalPackage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPackageNode {
    pub id: PackageId,
    pub manifest_path: PathBuf,
    pub dependencies: Vec<ResolvedDependencyEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDependencyEdge {
    pub kind: DependencyKind,
    pub domain: BuildDomain,
    pub dependency_name: String,
    pub package_name: String,
    pub target: ResolvedDependencyTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedDependencyTarget {
    Local(PackageId),
    External(ExternalPackageId),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExternalPackageId {
    pub package_name: String,
    pub source: SourceId,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedExternalPackage {
    pub id: ExternalPackageId,
}

pub fn resolve_graph(graph: &PackageGraph) -> ResolvedGraph {
    let mut external_index = BTreeMap::new();
    let mut packages = Vec::new();

    for package in &graph.packages {
        let mut dependencies = Vec::new();
        for dep in &package.dependencies {
            let target = match &dep.target {
                DependencyTarget::Local(target) => ResolvedDependencyTarget::Local(target.clone()),
                DependencyTarget::External(target) => {
                    let id = external_package_id(target);
                    external_index
                        .entry(id.clone())
                        .or_insert_with(|| ResolvedExternalPackage { id: id.clone() });
                    ResolvedDependencyTarget::External(id)
                }
            };

            dependencies.push(ResolvedDependencyEdge {
                kind: dep.kind,
                domain: dep.domain,
                dependency_name: dep.dependency_name.clone(),
                package_name: dep.package_name.clone(),
                target,
            });
        }

        packages.push(ResolvedPackageNode {
            id: package.id.clone(),
            manifest_path: package.manifest_path.clone(),
            dependencies,
        });
    }

    let external_packages = external_index.into_values().collect::<Vec<_>>();

    ResolvedGraph {
        workspace_root: graph.workspace_root.clone(),
        packages,
        external_packages,
    }
}

fn external_package_id(dep: &ExternalDependency) -> ExternalPackageId {
    ExternalPackageId {
        package_name: dep.package_name.clone(),
        source: dep.source.clone(),
        version: dep.version.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{ResolvedDependencyTarget, resolve_graph};
    use crate::graph::{DependencyKind, SourceId, build_graph};
    use crate::manifest::Manifest;
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
    fn deduplicates_shared_external_git_dependencies() {
        let root = temp_dir("craft-resolver-dedupe");
        let app_dir = root.join("app");
        let tool_dir = root.join("tool");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&tool_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
name = "workspace"
members = ["app", "tool"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.9"

[dependencies]
log = { git = "https://example.com/log.git", tag = "v1" }
"#,
        )
        .unwrap();
        fs::write(
            tool_dir.join("Craft.toml"),
            r#"
[package]
name = "tool"
version = "0.1.0"
kern = "0.7.9"

[dependencies]
log = { git = "https://example.com/log.git", tag = "v1" }
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &manifest).unwrap();
        let graph = build_graph(&manifest_path, &manifest, &members).unwrap();
        let resolved = resolve_graph(&graph);

        assert_eq!(resolved.packages.len(), 2);
        assert_eq!(resolved.external_packages.len(), 1);
        assert_eq!(resolved.external_packages[0].id.package_name, "log");
        assert!(matches!(
            &resolved.external_packages[0].id.source,
            SourceId::GitDependency { git, tag, .. }
                if git == "https://example.com/log.git" && tag.as_deref() == Some("v1")
        ));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn preserves_local_and_external_resolution_targets() {
        let root = temp_dir("craft-resolver-targets");
        let app_dir = root.join("app");
        let util_dir = root.join("util");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&util_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
name = "workspace"
members = ["app", "util"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.9"

[dependencies]
util = { path = "../util" }
log = { git = "https://example.com/log.git", branch = "main" }
"#,
        )
        .unwrap();
        fs::write(
            util_dir.join("Craft.toml"),
            r#"
[package]
name = "util"
version = "0.1.0"
kern = "0.7.9"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &manifest).unwrap();
        let graph = build_graph(&manifest_path, &manifest, &members).unwrap();
        let resolved = resolve_graph(&graph);

        let app = resolved
            .packages
            .iter()
            .find(|pkg| pkg.id.name == "app")
            .unwrap();
        assert!(app.dependencies.iter().any(|dep| {
            dep.kind == DependencyKind::Normal
                && dep.dependency_name == "util"
                && matches!(&dep.target, ResolvedDependencyTarget::Local(pkg) if pkg.name == "util")
        }));
        assert!(app.dependencies.iter().any(|dep| {
            dep.kind == DependencyKind::Normal
                && dep.dependency_name == "log"
                && matches!(
                    &dep.target,
                    ResolvedDependencyTarget::External(pkg)
                        if pkg.package_name == "log"
                            && matches!(
                                &pkg.source,
                                SourceId::GitDependency { git, branch, .. }
                                    if git == "https://example.com/log.git"
                                        && branch.as_deref() == Some("main")
                            )
                )
        }));

        let _ = fs::remove_dir_all(root);
    }
}
