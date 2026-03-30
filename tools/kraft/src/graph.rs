use crate::error::{Error, Result};
use crate::manifest::{DependencySpec, Manifest};
use crate::workspace::WorkspaceMember;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PackageId {
    pub name: String,
    pub version: String,
    pub source: SourceId,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceId {
    Root,
    WorkspaceMember { path: String },
    PathDependency { path: String },
    Registry { name: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DependencyKind {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageGraph {
    pub workspace_root: PathBuf,
    pub packages: Vec<PackageNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageNode {
    pub id: PackageId,
    pub manifest_path: PathBuf,
    pub dependencies: Vec<DependencyEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyEdge {
    pub kind: DependencyKind,
    pub dependency_name: String,
    pub package_name: String,
    pub target: DependencyTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyTarget {
    Local(PackageId),
    External(ExternalDependency),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalDependency {
    pub package_name: String,
    pub source: SourceId,
    pub version: Option<String>,
}

pub fn build_graph(
    manifest_path: &Path,
    manifest: &Manifest,
    workspace_members: &[WorkspaceMember],
) -> Result<PackageGraph> {
    let workspace_root = manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let root_dir = canonical_dir(&workspace_root)?;

    let mut packages = Vec::new();
    if manifest.package.is_some() {
        packages.push(local_package_from_manifest(
            manifest_path,
            manifest,
            SourceId::Root,
        )?);
    }
    for member in workspace_members {
        let relative = member
            .manifest_path
            .parent()
            .and_then(|dir| dir.strip_prefix(&workspace_root).ok())
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| member.manifest_path.display().to_string());
        packages.push(local_package_from_manifest(
            &member.manifest_path,
            &member.manifest,
            SourceId::WorkspaceMember { path: relative },
        )?);
    }

    let mut local_index = BTreeMap::new();
    for package in &packages {
        let package_root = package
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        local_index.insert(canonical_dir(&package_root)?, package.id.clone());
    }

    let mut graph_nodes = Vec::new();
    for package in packages {
        let package_root = package
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let manifest = if package.id.source == SourceId::Root {
            manifest
        } else {
            &workspace_members
                .iter()
                .find(|member| member.manifest_path == package.manifest_path)
                .expect("workspace member manifest must exist")
                .manifest
        };
        let dependencies = build_edges(&package_root, manifest, &root_dir, &local_index)?;
        graph_nodes.push(PackageNode {
            id: package.id,
            manifest_path: package.manifest_path,
            dependencies,
        });
    }

    graph_nodes.sort_by(|lhs, rhs| lhs.id.cmp(&rhs.id));

    Ok(PackageGraph {
        workspace_root,
        packages: graph_nodes,
    })
}

fn build_edges(
    package_root: &Path,
    manifest: &Manifest,
    workspace_root: &Path,
    local_index: &BTreeMap<PathBuf, PackageId>,
) -> Result<Vec<DependencyEdge>> {
    let mut edges = Vec::new();
    collect_dep_edges(
        &mut edges,
        package_root,
        workspace_root,
        local_index,
        &manifest.dependencies,
        DependencyKind::Normal,
    )?;
    collect_dep_edges(
        &mut edges,
        package_root,
        workspace_root,
        local_index,
        &manifest.dev_dependencies,
        DependencyKind::Dev,
    )?;
    collect_dep_edges(
        &mut edges,
        package_root,
        workspace_root,
        local_index,
        &manifest.build_dependencies,
        DependencyKind::Build,
    )?;
    Ok(edges)
}

fn collect_dep_edges(
    edges: &mut Vec<DependencyEdge>,
    package_root: &Path,
    workspace_root: &Path,
    local_index: &BTreeMap<PathBuf, PackageId>,
    deps: &BTreeMap<String, DependencySpec>,
    kind: DependencyKind,
) -> Result<()> {
    for (dependency_name, spec) in deps {
        edges.push(DependencyEdge {
            kind,
            dependency_name: dependency_name.clone(),
            package_name: requested_package_name(dependency_name, spec),
            target: dependency_target(
                package_root,
                workspace_root,
                local_index,
                dependency_name,
                spec,
            )?,
        });
    }
    Ok(())
}

fn requested_package_name(name: &str, spec: &DependencySpec) -> String {
    match spec {
        DependencySpec::Version(_) => name.to_string(),
        DependencySpec::Detailed(dep) => dep.package.clone().unwrap_or_else(|| name.to_string()),
    }
}

fn dependency_target(
    package_root: &Path,
    workspace_root: &Path,
    local_index: &BTreeMap<PathBuf, PackageId>,
    dependency_name: &str,
    spec: &DependencySpec,
) -> Result<DependencyTarget> {
    match spec {
        DependencySpec::Version(version) => Ok(DependencyTarget::External(ExternalDependency {
            package_name: dependency_name.to_string(),
            source: SourceId::Registry { name: None },
            version: Some(version.clone()),
        })),
        DependencySpec::Detailed(dep) => {
            let package_name = dep
                .package
                .clone()
                .unwrap_or_else(|| dependency_name.to_string());
            if let Some(path_value) = &dep.path {
                let absolute = canonical_dir(&package_root.join(path_value))?;
                if let Some(local) = local_index.get(&absolute) {
                    return Ok(DependencyTarget::Local(local.clone()));
                }

                let display = absolute
                    .strip_prefix(workspace_root)
                    .map(|path| path.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|_| absolute.display().to_string());
                return Ok(DependencyTarget::External(ExternalDependency {
                    package_name,
                    source: SourceId::PathDependency { path: display },
                    version: dep.version.clone(),
                }));
            }

            Ok(DependencyTarget::External(ExternalDependency {
                package_name,
                source: SourceId::Registry {
                    name: dep.registry.clone(),
                },
                version: dep.version.clone(),
            }))
        }
    }
}

fn local_package_from_manifest(
    manifest_path: &Path,
    manifest: &Manifest,
    source: SourceId,
) -> Result<PackageNode> {
    let Some(package) = &manifest.package else {
        return Err(Error::Validation {
            path: manifest_path.to_path_buf(),
            message: "local package graph nodes require `[package]`".to_string(),
        });
    };

    Ok(PackageNode {
        id: PackageId {
            name: package.name.clone(),
            version: package.version.clone(),
            source,
        },
        manifest_path: manifest_path.to_path_buf(),
        dependencies: Vec::new(),
    })
}

fn canonical_dir(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path).map_err(|err| Error::from_io(path, err))
}

#[cfg(test)]
mod tests {
    use super::{DependencyKind, DependencyTarget, SourceId, build_graph};
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
    fn builds_graph_for_workspace_and_local_path_dependencies() {
        let root = temp_dir("kraft-graph");
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
"#,
        )
        .unwrap();

        let root_manifest = Manifest::load(&root.join("Kraft.toml")).unwrap();
        let members = load_members(&root.join("Kraft.toml"), &root_manifest).unwrap();
        let graph = build_graph(&root.join("Kraft.toml"), &root_manifest, &members).unwrap();

        assert_eq!(graph.packages.len(), 2);
        let app = graph
            .packages
            .iter()
            .find(|pkg| pkg.id.name == "app")
            .unwrap();
        assert_eq!(app.dependencies.len(), 2);
        assert!(app.dependencies.iter().any(|dep| {
            dep.kind == DependencyKind::Normal
                && dep.dependency_name == "util"
                && matches!(&dep.target, DependencyTarget::Local(pkg) if pkg.name == "util")
        }));
        assert!(app.dependencies.iter().any(|dep| {
            dep.dependency_name == "log"
                && matches!(
                    &dep.target,
                    DependencyTarget::External(ext)
                        if ext.source == SourceId::Registry { name: None }
                )
        }));

        let _ = fs::remove_dir_all(root);
    }
}
