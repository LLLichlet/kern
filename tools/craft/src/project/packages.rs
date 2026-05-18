use crate::error::Result;
use crate::graph::{self, DependencyTarget, PackageGraph, PackageId, SourceId};
use crate::manifest::Manifest;
use crate::plan::{PackagePlan, TargetKind};
use crate::sdk;
use crate::workspace::WorkspaceMember;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(super) struct AnalysisPackage {
    pub(super) id: PackageId,
    pub(super) manifest_path: PathBuf,
    pub(super) package_root: PathBuf,
    pub(super) lib_root: Option<PathBuf>,
    pub(super) target_roots: Vec<AnalysisTargetRoot>,
    pub(super) module_aliases: BTreeMap<String, PathBuf>,
    pub(super) script_roots: Vec<AnalysisScriptRoot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisTarget {
    pub package_name: String,
    pub manifest_path: PathBuf,
    pub kind: TargetKind,
    pub name: Option<String>,
    pub root: PathBuf,
}

#[derive(Debug, Clone)]
pub(super) struct AnalysisScriptRoot {
    pub(super) root: PathBuf,
    pub(super) module_aliases: BTreeMap<String, PathBuf>,
}

#[derive(Debug, Clone)]
pub(super) struct PackageEntry {
    pub(super) id: PackageId,
    pub(super) manifest_path: PathBuf,
    pub(super) package_root: PathBuf,
    pub(super) lib_root: Option<PathBuf>,
    pub(super) target_roots: Vec<AnalysisTargetRoot>,
}

#[derive(Debug, Clone)]
pub(super) struct AnalysisTargetRoot {
    pub(super) root: PathBuf,
    pub(super) kind: TargetKind,
    pub(super) name: Option<String>,
}

impl AnalysisPackage {
    pub(super) fn analysis_root_for(&self, file: &Path) -> PathBuf {
        if let Some(target_root) = self.best_matching_target_root(file) {
            return target_root.root.clone();
        }

        if let Some(root) = &self.lib_root
            && target_match_score(root, file).is_some()
        {
            return root.clone();
        }

        file.to_path_buf()
    }

    pub(super) fn target_root_for(&self, file: &Path) -> Option<&AnalysisTargetRoot> {
        self.best_matching_target_root(file)
    }

    fn best_matching_target_root(&self, file: &Path) -> Option<&AnalysisTargetRoot> {
        self.target_roots
            .iter()
            .filter_map(|target_root| {
                target_match_score(&target_root.root, file).map(|score| (score, target_root))
            })
            .max_by_key(|(score, target_root)| (*score, target_root.root.components().count()))
            .map(|(_, target_root)| target_root)
    }
}

pub(super) fn assemble_packages(
    manifest_path: &Path,
    package_graph: &PackageGraph,
    package_entries: &[PackageEntry],
) -> Vec<AnalysisPackage> {
    let package_index = package_entries
        .iter()
        .map(|entry| (entry.id.clone(), entry))
        .collect::<BTreeMap<_, _>>();
    let graph_index = package_graph
        .packages
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect::<BTreeMap<_, _>>();

    let mut packages = Vec::new();
    for entry in package_entries {
        let module_aliases = graph_index
            .get(&entry.id)
            .map(|node| {
                let mut aliases = BTreeMap::new();
                let mut visited = BTreeSet::new();
                collect_dependency_module_aliases(
                    node,
                    &graph_index,
                    &package_index,
                    &package_graph.workspace_root,
                    &mut visited,
                    &mut aliases,
                );
                aliases
            })
            .unwrap_or_default();
        packages.push(AnalysisPackage {
            id: entry.id.clone(),
            manifest_path: entry.manifest_path.clone(),
            package_root: entry.package_root.clone(),
            lib_root: entry.lib_root.clone(),
            target_roots: entry.target_roots.clone(),
            module_aliases,
            script_roots: script_roots_for_package_root(&entry.package_root),
        });
    }

    let _ = manifest_path;
    packages
}

fn craft_sdk_aliases() -> BTreeMap<String, PathBuf> {
    BTreeMap::from([(String::from("craft"), sdk::sdk_root())])
}

fn script_roots_for_package_root(package_root: &Path) -> Vec<AnalysisScriptRoot> {
    vec![AnalysisScriptRoot {
        root: package_root.join("build.kn"),
        module_aliases: craft_sdk_aliases(),
    }]
}

pub(super) fn target_match_score(root: &Path, file: &Path) -> Option<usize> {
    if root == file {
        return Some(usize::MAX);
    }

    let stem = root.file_stem()?;
    let module_dir = root.parent()?.join(stem);
    if file.starts_with(&module_dir) {
        return Some(module_dir.components().count());
    }

    let sibling_module_root = root.parent()?;
    if file.starts_with(sibling_module_root) {
        return Some(sibling_module_root.components().count());
    }

    None
}

pub(super) fn package_entries(
    manifest_path: &Path,
    manifest: &Manifest,
    workspace_members: &[WorkspaceMember],
) -> Result<Vec<PackageEntry>> {
    let mut packages = Vec::new();
    if manifest.package.is_some() {
        packages.push(package_entry(
            manifest_path,
            manifest,
            SourceId::Root,
            None,
        )?);
    }

    let workspace_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    for member in workspace_members {
        let relative = member
            .manifest_path
            .parent()
            .and_then(|dir| dir.strip_prefix(workspace_root).ok())
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|| member.manifest_path.display().to_string());
        packages.push(package_entry(
            &member.manifest_path,
            &member.manifest,
            SourceId::WorkspaceMember { path: relative },
            None,
        )?);
    }

    Ok(packages)
}

fn package_entry(
    manifest_path: &Path,
    manifest: &Manifest,
    source: SourceId,
    override_package_root: Option<PathBuf>,
) -> Result<PackageEntry> {
    let package_id = graph::local_package_id_from_manifest(manifest_path, manifest, source)?;
    let package_plan = PackagePlan::from_manifest(manifest_path, &package_id, manifest)?;
    let package_root = override_package_root.unwrap_or_else(|| {
        manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    });
    let lib_root = package_plan
        .targets
        .iter()
        .find(|target| target.kind == TargetKind::Lib)
        .map(|target| package_root.join(&target.root));
    let target_roots = package_plan
        .targets
        .iter()
        .map(|target| AnalysisTargetRoot {
            root: package_root.join(&target.root),
            kind: target.kind,
            name: target.name.clone(),
        })
        .collect();

    Ok(PackageEntry {
        id: package_id,
        manifest_path: manifest_path.to_path_buf(),
        package_root,
        lib_root,
        target_roots,
    })
}

fn collect_dependency_module_aliases<'a>(
    node: &'a crate::graph::PackageNode,
    graph_index: &BTreeMap<PackageId, &'a crate::graph::PackageNode>,
    package_index: &BTreeMap<PackageId, &PackageEntry>,
    workspace_root: &Path,
    visited: &mut BTreeSet<PackageId>,
    aliases: &mut BTreeMap<String, PathBuf>,
) {
    for dependency in &node.dependencies {
        match &dependency.target {
            DependencyTarget::Local(package_id) => {
                if !visited.insert(package_id.clone()) {
                    continue;
                }

                if let Some(package) = package_index.get(package_id) {
                    if let Some(lib_root) = &package.lib_root {
                        aliases.insert(dependency.dependency_name.clone(), lib_root.clone());
                    }
                    if let Some(dep_node) = graph_index.get(package_id) {
                        collect_dependency_module_aliases(
                            dep_node,
                            graph_index,
                            package_index,
                            workspace_root,
                            visited,
                            aliases,
                        );
                    }
                }
            }
            DependencyTarget::External(package_id) => {
                if let Some(lib_root) =
                    external_dependency_lib_root(workspace_root, package_id).as_ref()
                {
                    aliases.insert(dependency.dependency_name.clone(), lib_root.clone());
                }
            }
        }
    }
}

fn external_dependency_lib_root(
    workspace_root: &Path,
    package_id: &crate::graph::ExternalDependency,
) -> Option<PathBuf> {
    let external_package_id = crate::resolver::ExternalPackageId {
        package_name: package_id.package_name.clone(),
        source: package_id.source.clone(),
        version: package_id.version.clone(),
    };
    let package_root =
        crate::source::analysis_source_root_for_external(workspace_root, &external_package_id)
            .ok()
            .flatten()?;
    let manifest_path = package_root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path).ok()?;
    manifest.validate(&manifest_path).ok()?;
    let entry = package_entry(
        &manifest_path,
        &manifest,
        external_package_id.source.clone(),
        Some(package_root),
    )
    .ok()?;
    entry.lib_root
}
