//! Package dependency graph construction.
//!
//! Graph building combines the root manifest, workspace members, inherited
//! dependencies, and external source identities into a resolved graph used by
//! lockfiles, fetching, and build-plan elaboration.

use crate::error::{Error, Result};
use crate::manifest::{DependencySpec, DetailedDependency, Manifest};
use crate::plan::PackagePlan;
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
    WorkspaceMember {
        path: String,
    },
    PathDependency {
        path: String,
    },
    GitDependency {
        git: String,
        rev: Option<String>,
        branch: Option<String>,
        tag: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DependencyKind {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BuildDomain {
    Host,
    Target,
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
    pub domain: BuildDomain,
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
    let mut package_plans = Vec::new();
    if manifest.package.is_some() {
        package_plans.push(PackagePlan::from_manifest(
            manifest_path,
            &local_package_id_from_manifest(manifest_path, manifest, SourceId::Root)?,
            manifest,
        )?);
    }
    for member in workspace_members {
        let relative = member
            .manifest_path
            .parent()
            .and_then(|dir| {
                dir.strip_prefix(manifest_path.parent().unwrap_or_else(|| Path::new(".")))
                    .ok()
            })
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| member.manifest_path.display().to_string());
        package_plans.push(PackagePlan::from_manifest(
            &member.manifest_path,
            &local_package_id_from_manifest(
                &member.manifest_path,
                &member.manifest,
                SourceId::WorkspaceMember { path: relative },
            )?,
            &member.manifest,
        )?);
    }

    build_graph_from_plans(manifest_path, manifest, &package_plans)
}

pub fn build_graph_from_plans<'a>(
    manifest_path: &Path,
    manifest: &Manifest,
    package_plans: impl IntoIterator<Item = &'a PackagePlan>,
) -> Result<PackageGraph> {
    let package_plans = package_plans.into_iter().collect::<Vec<_>>();
    let workspace_root = manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let root_dir = canonical_dir(&workspace_root)?;
    let workspace_dependencies = manifest
        .workspace
        .as_ref()
        .map(|workspace| &workspace.dependencies);

    let mut local_index = BTreeMap::new();
    for package in &package_plans {
        let package_root = package
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        local_index.insert(canonical_dir(&package_root)?, package.package_id.clone());
    }

    let mut graph_nodes = Vec::new();
    for package in &package_plans {
        let package_root = package
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let dependencies = build_edges(
            &package.manifest_path,
            &package_root,
            package,
            workspace_dependencies,
            &root_dir,
            &local_index,
        )?;
        graph_nodes.push(PackageNode {
            id: package.package_id.clone(),
            manifest_path: package.manifest_path.clone(),
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
    manifest_path: &Path,
    package_root: &Path,
    package: &PackagePlan,
    workspace_dependencies: Option<&BTreeMap<String, DependencySpec>>,
    workspace_root: &Path,
    local_index: &BTreeMap<PathBuf, PackageId>,
) -> Result<Vec<DependencyEdge>> {
    let mut edges = Vec::new();
    let ctx = DepEdgeContext {
        manifest_path,
        package_root,
        workspace_dependencies,
        workspace_root,
        local_index,
    };
    collect_dep_edges(
        &ctx,
        &mut edges,
        package.dependencies(DependencyKind::Normal),
        DependencyKind::Normal,
    )?;
    collect_dep_edges(
        &ctx,
        &mut edges,
        package.dependencies(DependencyKind::Dev),
        DependencyKind::Dev,
    )?;
    collect_dep_edges(
        &ctx,
        &mut edges,
        package.dependencies(DependencyKind::Build),
        DependencyKind::Build,
    )?;
    Ok(edges)
}

struct DepEdgeContext<'a> {
    manifest_path: &'a Path,
    package_root: &'a Path,
    workspace_dependencies: Option<&'a BTreeMap<String, DependencySpec>>,
    workspace_root: &'a Path,
    local_index: &'a BTreeMap<PathBuf, PackageId>,
}

fn collect_dep_edges(
    ctx: &DepEdgeContext<'_>,
    edges: &mut Vec<DependencyEdge>,
    deps: &BTreeMap<String, DependencySpec>,
    kind: DependencyKind,
) -> Result<()> {
    for (dependency_name, spec) in deps {
        let spec = normalize_dependency_spec(
            ctx.manifest_path,
            ctx.workspace_root,
            ctx.workspace_dependencies,
            dependency_name,
            spec,
        )?;
        edges.push(DependencyEdge {
            kind,
            domain: dependency_domain(kind),
            dependency_name: dependency_name.clone(),
            package_name: requested_package_name(dependency_name, &spec),
            target: dependency_target(
                ctx.package_root,
                ctx.workspace_root,
                ctx.local_index,
                dependency_name,
                &spec,
            )?,
        });
    }
    Ok(())
}

fn dependency_domain(kind: DependencyKind) -> BuildDomain {
    match kind {
        DependencyKind::Build => BuildDomain::Host,
        DependencyKind::Normal | DependencyKind::Dev => BuildDomain::Target,
    }
}

fn normalize_dependency_spec(
    manifest_path: &Path,
    workspace_root: &Path,
    workspace_dependencies: Option<&BTreeMap<String, DependencySpec>>,
    dependency_name: &str,
    spec: &DependencySpec,
) -> Result<DependencySpec> {
    let DependencySpec::Detailed(overlay) = spec else {
        return Ok(spec.clone());
    };

    if overlay.workspace != Some(true) {
        return Ok(spec.clone());
    }

    let Some(workspace_dependencies) = workspace_dependencies else {
        return Err(Error::Validation {
            path: manifest_path.to_path_buf(),
            message: format!(
                "dependency `{dependency_name}` uses `workspace = true` but no `[workspace.dependencies]` are available"
            ),
        });
    };

    let Some(base_spec) = workspace_dependencies.get(dependency_name) else {
        return Err(Error::Validation {
            path: manifest_path.to_path_buf(),
            message: format!(
                "dependency `{dependency_name}` uses `workspace = true` but is missing from `[workspace.dependencies]`"
            ),
        });
    };

    let mut merged = dependency_spec_to_detailed(base_spec);
    if let Some(path) = &merged.path
        && Path::new(path).is_relative()
    {
        merged.path = Some(workspace_root.join(path).to_string_lossy().to_string());
    }
    merged.workspace = None;

    if let Some(export) = &overlay.export {
        merged.export = Some(export.clone());
    }
    if let Some(optional) = overlay.optional {
        merged.optional = Some(optional);
    }
    if let Some(default_features) = overlay.default_features {
        merged.default_features = Some(default_features);
    }
    for feature in &overlay.features {
        if !merged.features.contains(feature) {
            merged.features.push(feature.clone());
        }
    }

    Ok(DependencySpec::Detailed(merged))
}

fn dependency_spec_to_detailed(spec: &DependencySpec) -> DetailedDependency {
    match spec {
        DependencySpec::Version(version) => DetailedDependency {
            version: Some(version.clone()),
            ..DetailedDependency::default()
        },
        DependencySpec::Detailed(dep) => dep.clone(),
    }
}

fn requested_package_name(name: &str, spec: &DependencySpec) -> String {
    match spec {
        DependencySpec::Version(_) => name.to_string(),
        DependencySpec::Detailed(dep) => dep.export.clone().unwrap_or_else(|| name.to_string()),
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
        DependencySpec::Version(_) => Err(Error::Validation {
            path: package_root.join("Craft.toml"),
            message: format!(
                "dependency `{dependency_name}` must use `path` or `git`; plain version strings are unsupported"
            ),
        }),
        DependencySpec::Detailed(dep) => {
            let package_name = dep
                .export
                .clone()
                .unwrap_or_else(|| dependency_name.to_string());
            if let Some(path_value) = &dep.path {
                let absolute = canonical_dir(&package_root.join(path_value))?;
                if let Some(local) = local_index.get(&absolute) {
                    return Ok(DependencyTarget::Local(local.clone()));
                }

                let display = relative_display_from(workspace_root, &absolute);
                return Ok(DependencyTarget::External(ExternalDependency {
                    package_name,
                    source: SourceId::PathDependency { path: display },
                    version: dep.version.clone(),
                }));
            }

            if let Some(git) = &dep.git {
                return Ok(DependencyTarget::External(ExternalDependency {
                    package_name,
                    source: SourceId::GitDependency {
                        git: git.clone(),
                        rev: dep.rev.clone(),
                        branch: dep.branch.clone(),
                        tag: dep.tag.clone(),
                    },
                    version: dep.version.clone(),
                }));
            }

            Err(Error::Validation {
                path: package_root.join("Craft.toml"),
                message: format!("dependency `{dependency_name}` must declare `path` or `git`"),
            })
        }
    }
}

pub(crate) fn local_package_id_from_manifest(
    manifest_path: &Path,
    manifest: &Manifest,
    source: SourceId,
) -> Result<PackageId> {
    let Some(package) = &manifest.package else {
        return Err(Error::Validation {
            path: manifest_path.to_path_buf(),
            message: "local package graph nodes require `[package]`".to_string(),
        });
    };

    Ok(PackageId {
        name: package.name.clone(),
        version: package.version.clone(),
        source,
    })
}

fn canonical_dir(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path).map_err(|err| Error::from_io(path, err))
}

fn relative_display_from(base: &Path, target: &Path) -> String {
    make_relative_path(base, target)
        .unwrap_or_else(|| target.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
}

fn make_relative_path(base: &Path, target: &Path) -> Option<PathBuf> {
    use std::path::Component;

    let base_components = base.components().collect::<Vec<_>>();
    let target_components = target.components().collect::<Vec<_>>();

    let mut common = 0;
    while common < base_components.len()
        && common < target_components.len()
        && base_components[common] == target_components[common]
    {
        common += 1;
    }

    if common == 0 {
        return None;
    }

    let mut relative = PathBuf::new();
    for component in &base_components[common..] {
        if matches!(component, Component::Normal(_)) {
            relative.push("..");
        }
    }
    for component in &target_components[common..] {
        relative.push(component.as_os_str());
    }

    if relative.as_os_str().is_empty() {
        relative.push(".");
    }

    Some(relative)
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
    fn builds_graph_for_workspace_path_and_git_dependencies() {
        let root = temp_dir("craft-graph");
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
kern = "0.7.6"

[dependencies]
util = { path = "../util" }
toml = { git = "https://example.com/toml.git", tag = "v0.1.0" }
"#,
        )
        .unwrap();
        fs::write(
            util_dir.join("Craft.toml"),
            r#"
[package]
name = "util"
version = "0.1.0"
kern = "0.7.6"
"#,
        )
        .unwrap();

        let root_manifest = Manifest::load(&root.join("Craft.toml")).unwrap();
        let members = load_members(&root.join("Craft.toml"), &root_manifest).unwrap();
        let graph = build_graph(&root.join("Craft.toml"), &root_manifest, &members).unwrap();

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
            dep.dependency_name == "toml"
                && matches!(
                    &dep.target,
                    DependencyTarget::External(ext)
                        if matches!(
                            &ext.source,
                            SourceId::GitDependency { git, tag, .. }
                                if git == "https://example.com/toml.git"
                                    && tag.as_deref() == Some("v0.1.0")
                        )
                )
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn inherits_workspace_dependencies_into_member_graph_edges() {
        let root = temp_dir("craft-workspace-inherit");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
name = "workspace"
members = ["app"]

[workspace.dependencies]
shared = { git = "https://example.com/shared.git", rev = "abc123" }
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.6"

[dependencies]
shared = { workspace = true, features = ["simd"] }
"#,
        )
        .unwrap();

        let root_manifest = Manifest::load(&root.join("Craft.toml")).unwrap();
        let members = load_members(&root.join("Craft.toml"), &root_manifest).unwrap();
        let graph = build_graph(&root.join("Craft.toml"), &root_manifest, &members).unwrap();

        let app = graph
            .packages
            .iter()
            .find(|pkg| pkg.id.name == "app")
            .unwrap();
        let shared = app
            .dependencies
            .iter()
            .find(|dep| dep.dependency_name == "shared")
            .unwrap();

        assert_eq!(shared.kind, DependencyKind::Normal);
        assert_eq!(shared.package_name, "shared");
        match &shared.target {
            DependencyTarget::External(ext) => {
                assert!(matches!(
                    &ext.source,
                    SourceId::GitDependency { git, rev, .. }
                        if git == "https://example.com/shared.git"
                            && rev.as_deref() == Some("abc123")
                ));
            }
            other => panic!("expected external dependency, got {other:?}"),
        }

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn inherits_workspace_path_dependencies_relative_to_workspace_root() {
        let root = temp_dir("craft-workspace-inherit-path");
        let app_dir = root.join("app");
        let shared_dir = root.join("shared");
        fs::create_dir_all(app_dir.join("src")).unwrap();
        fs::create_dir_all(shared_dir.join("src")).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
name = "workspace"
members = ["app", "shared"]

[workspace.dependencies]
shared = { path = "shared" }
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.6"

[dependencies]
shared = { workspace = true }
"#,
        )
        .unwrap();
        fs::write(
            shared_dir.join("Craft.toml"),
            r#"
[package]
name = "shared"
version = "0.1.0"
kern = "0.7.6"

[lib]
root = "src/lib.kn"
"#,
        )
        .unwrap();

        let root_manifest = Manifest::load(&root.join("Craft.toml")).unwrap();
        let members = load_members(&root.join("Craft.toml"), &root_manifest).unwrap();
        let graph = build_graph(&root.join("Craft.toml"), &root_manifest, &members).unwrap();

        let app = graph
            .packages
            .iter()
            .find(|pkg| pkg.id.name == "app")
            .unwrap();
        let shared = app
            .dependencies
            .iter()
            .find(|dep| dep.dependency_name == "shared")
            .unwrap();

        assert!(matches!(
            &shared.target,
            DependencyTarget::Local(pkg) if pkg.name == "shared"
        ));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stores_external_path_dependencies_relative_to_workspace_root() {
        let root = temp_dir("craft-external-relative-path");
        let app_dir = root.join("app");
        let vendor_root = root.join("vendor");
        let util_dir = vendor_root.join("util");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&util_dir).unwrap();

        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.6"

[dependencies]
util = { path = "../vendor/util", version = "1" }
"#,
        )
        .unwrap();
        fs::write(
            util_dir.join("Craft.toml"),
            r#"
[package]
name = "util"
version = "1.0.0"
kern = "0.7.6"
"#,
        )
        .unwrap();

        let manifest_path = app_dir.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let graph = build_graph(&manifest_path, &manifest, &[]).unwrap();
        let app = graph
            .packages
            .iter()
            .find(|pkg| pkg.id.name == "app")
            .unwrap();
        let util = app
            .dependencies
            .iter()
            .find(|dep| dep.dependency_name == "util")
            .unwrap();

        assert!(matches!(
            &util.target,
            DependencyTarget::External(ext)
                if matches!(
                    &ext.source,
                    SourceId::PathDependency { path } if path == "../vendor/util"
                )
        ));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_missing_workspace_dependency_inheritance_entry() {
        let root = temp_dir("craft-workspace-missing-inherit");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
name = "workspace"
members = ["app"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.6"

[dependencies]
shared = { workspace = true }
"#,
        )
        .unwrap();

        let root_manifest = Manifest::load(&root.join("Craft.toml")).unwrap();
        let members = load_members(&root.join("Craft.toml"), &root_manifest).unwrap();
        let err = build_graph(&root.join("Craft.toml"), &root_manifest, &members).unwrap_err();

        assert!(
            err.to_string()
                .contains("uses `workspace = true` but is missing from `[workspace.dependencies]`")
        );

        let _ = fs::remove_dir_all(root);
    }
}
