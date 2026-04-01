use crate::discover;
use crate::error::Result;
use crate::graph::{self, DependencyTarget, PackageGraph, PackageId, SourceId};
use crate::manifest::Manifest;
use crate::plan::{PackagePlan, TargetKind};
use crate::workspace::{self, WorkspaceMember};
use kernc_utils::config::{CompileOptions, maybe_inject_std_alias};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AnalysisProject {
    manifest_path: PathBuf,
    workspace_root: PathBuf,
    packages: Vec<AnalysisPackage>,
}

#[derive(Debug, Clone)]
struct AnalysisPackage {
    id: PackageId,
    package_root: PathBuf,
    lib_root: Option<PathBuf>,
    target_roots: Vec<PathBuf>,
    module_aliases: BTreeMap<String, PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ResolvedAnalysis {
    pub input_file: PathBuf,
    pub compile_options: CompileOptions,
}

impl AnalysisProject {
    pub fn load_from_path(input: Option<&Path>) -> Result<Self> {
        let manifest_path = resolve_project_manifest_path(input)?;
        Self::load_from_manifest(&manifest_path)
    }

    pub fn load_from_manifest(manifest_path: &Path) -> Result<Self> {
        let manifest = Manifest::load(manifest_path)?;
        manifest.validate(manifest_path)?;
        let workspace_members = workspace::load_members(manifest_path, &manifest)?;
        let package_graph = graph::build_graph(manifest_path, &manifest, &workspace_members)?;
        let package_entries = package_entries(manifest_path, &manifest, &workspace_members)?;
        Ok(Self::from_parts(
            manifest_path,
            package_graph,
            package_entries,
        ))
    }

    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn resolve_for_file(&self, file: &Path, base_options: &CompileOptions) -> ResolvedAnalysis {
        let mut compile_options = base_options.clone();
        let mut input_file = file.to_path_buf();

        if let Some(package) = self.package_for_file(file) {
            input_file = package.analysis_root_for(file);
            for (name, path) in &package.module_aliases {
                compile_options
                    .module_aliases
                    .entry(name.clone())
                    .or_insert_with(|| path.to_string_lossy().to_string());
            }
            if package.lib_root.as_ref() == Some(&input_file) {
                compile_options.root_module_name = Some(package.id.name.clone());
            }
        }

        maybe_inject_std_alias(&mut compile_options);

        ResolvedAnalysis {
            input_file,
            compile_options,
        }
    }

    fn from_parts(
        manifest_path: &Path,
        package_graph: PackageGraph,
        package_entries: Vec<PackageEntry>,
    ) -> Self {
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
        for entry in &package_entries {
            let module_aliases = graph_index
                .get(&entry.id)
                .map(|node| {
                    let mut aliases = BTreeMap::new();
                    let mut visited = BTreeSet::new();
                    collect_local_module_aliases(
                        node,
                        &graph_index,
                        &package_index,
                        &mut visited,
                        &mut aliases,
                    );
                    aliases
                })
                .unwrap_or_default();
            packages.push(AnalysisPackage {
                id: entry.id.clone(),
                package_root: entry.package_root.clone(),
                lib_root: entry.lib_root.clone(),
                target_roots: entry.target_roots.clone(),
                module_aliases,
            });
        }

        Self {
            manifest_path: manifest_path.to_path_buf(),
            workspace_root: package_graph.workspace_root,
            packages,
        }
    }

    fn package_for_file(&self, file: &Path) -> Option<&AnalysisPackage> {
        self.packages
            .iter()
            .filter(|package| file.starts_with(&package.package_root))
            .max_by_key(|package| package.package_root.components().count())
    }
}

pub fn resolve_project_manifest_path(input: Option<&Path>) -> Result<PathBuf> {
    let manifest_path = discover::resolve_manifest_path(input)?;
    let mut current = manifest_path
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf);

    while let Some(dir) = current {
        let candidate = dir.join("Craft.toml");
        if !candidate.is_file() {
            current = dir.parent().map(Path::to_path_buf);
            continue;
        }

        let manifest = Manifest::load(&candidate)?;
        manifest.validate(&candidate)?;
        if manifest.workspace.is_some()
            && workspace::load_members(&candidate, &manifest)?
                .iter()
                .any(|member| member.manifest_path == manifest_path)
        {
            return Ok(candidate);
        }

        current = dir.parent().map(Path::to_path_buf);
    }

    Ok(manifest_path)
}

impl AnalysisPackage {
    fn analysis_root_for(&self, file: &Path) -> PathBuf {
        if let Some(root) = self.best_matching_target_root(file) {
            return root.clone();
        }

        if let Some(root) = &self.lib_root {
            return root.clone();
        }

        file.to_path_buf()
    }

    fn best_matching_target_root(&self, file: &Path) -> Option<&PathBuf> {
        self.target_roots
            .iter()
            .filter_map(|root| target_match_score(root, file).map(|score| (score, root)))
            .max_by_key(|(score, root)| (*score, root.components().count()))
            .map(|(_, root)| root)
    }
}

fn target_match_score(root: &Path, file: &Path) -> Option<usize> {
    if root == file {
        return Some(usize::MAX);
    }

    let stem = root.file_stem()?;
    let module_dir = root.parent()?.join(stem);
    if file.starts_with(&module_dir) {
        return Some(module_dir.components().count());
    }

    None
}

#[derive(Debug, Clone)]
struct PackageEntry {
    id: PackageId,
    package_root: PathBuf,
    lib_root: Option<PathBuf>,
    target_roots: Vec<PathBuf>,
}

fn package_entries(
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
        .map(|target| package_root.join(&target.root))
        .collect();

    Ok(PackageEntry {
        id: package_id,
        package_root,
        lib_root,
        target_roots,
    })
}

fn collect_local_module_aliases<'a>(
    node: &'a crate::graph::PackageNode,
    graph_index: &BTreeMap<PackageId, &'a crate::graph::PackageNode>,
    package_index: &BTreeMap<PackageId, &PackageEntry>,
    visited: &mut BTreeSet<PackageId>,
    aliases: &mut BTreeMap<String, PathBuf>,
) {
    for dependency in &node.dependencies {
        let DependencyTarget::Local(package_id) = &dependency.target else {
            continue;
        };
        if !visited.insert(package_id.clone()) {
            continue;
        }

        if let Some(package) = package_index.get(package_id) {
            if let Some(lib_root) = &package.lib_root {
                aliases.insert(dependency.dependency_name.clone(), lib_root.clone());
            }
            if let Some(dep_node) = graph_index.get(package_id) {
                collect_local_module_aliases(
                    dep_node,
                    graph_index,
                    package_index,
                    visited,
                    aliases,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AnalysisProject;
    use kernc_utils::config::CompileOptions;
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolves_workspace_local_library_aliases_for_analysis() {
        let root = temp_dir("craft-project-analysis");
        let app_dir = root.join("app");
        let util_dir = root.join("util");
        fs::create_dir_all(app_dir.join("src")).unwrap();
        fs::create_dir_all(util_dir.join("src")).unwrap();

        fs::write(
            root.join("Craft.toml"),
            "[workspace]\nmembers = [\"app\", \"util\"]\n",
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.7\"

[lib]
root = \"src/lib.rn\"

[dependencies]
util = { path = \"../util\" }
",
        )
        .unwrap();
        fs::write(app_dir.join("src/lib.rn"), "use util;\n").unwrap();
        fs::write(
            util_dir.join("Craft.toml"),
            "\
[package]
name = \"util\"
version = \"0.1.0\"
kern = \"0.7\"

[lib]
root = \"src/lib.rn\"
",
        )
        .unwrap();
        fs::write(
            util_dir.join("src/lib.rn"),
            "fn helper() i32 { return 1; }\n",
        )
        .unwrap();

        let project = AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
        let resolved =
            project.resolve_for_file(&app_dir.join("src/lib.rn"), &CompileOptions::default());

        assert_eq!(resolved.input_file, app_dir.join("src/lib.rn"));
        assert_eq!(
            resolved.compile_options.root_module_name,
            Some("app".to_string())
        );
        assert_eq!(
            resolved
                .compile_options
                .module_aliases
                .get("util")
                .map(PathBuf::from),
            Some(util_dir.join("src/lib.rn"))
        );
    }

    #[test]
    fn prefers_exact_named_target_root_over_library_root() {
        let root = temp_dir("craft-project-multi-target-analysis");
        let app_dir = root.join("app");
        fs::create_dir_all(app_dir.join("src")).unwrap();

        fs::write(
            root.join("Craft.toml"),
            "[workspace]\nmembers = [\"app\"]\n",
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.7\"

[lib]
root = \"src/lib.rn\"

[[bin]]
name = \"demo\"
root = \"src/demo.rn\"
",
        )
        .unwrap();
        fs::write(
            app_dir.join("src/lib.rn"),
            "fn helper() i32 { return 1; }\n",
        )
        .unwrap();
        fs::write(app_dir.join("src/demo.rn"), "fn main() i32 { return 0; }\n").unwrap();

        let project = AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
        let resolved =
            project.resolve_for_file(&app_dir.join("src/demo.rn"), &CompileOptions::default());

        assert_eq!(resolved.input_file, app_dir.join("src/demo.rn"));
        assert_eq!(resolved.compile_options.root_module_name, None);
    }

    #[test]
    fn prefers_named_target_module_directory_over_library_root() {
        let root = temp_dir("craft-project-module-dir-analysis");
        let app_dir = root.join("app");
        fs::create_dir_all(app_dir.join("src/demo")).unwrap();

        fs::write(
            root.join("Craft.toml"),
            "[workspace]\nmembers = [\"app\"]\n",
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.7\"

[lib]
root = \"src/lib.rn\"

[[bin]]
name = \"demo\"
root = \"src/demo.rn\"
",
        )
        .unwrap();
        fs::write(
            app_dir.join("src/lib.rn"),
            "fn helper() i32 { return 1; }\n",
        )
        .unwrap();
        fs::write(
            app_dir.join("src/demo.rn"),
            "mod extra;\nfn main() i32 { return extra::run(); }\n",
        )
        .unwrap();
        fs::write(
            app_dir.join("src/demo/extra.rn"),
            "pub fn run() i32 { return 0; }\n",
        )
        .unwrap();

        let project = AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
        let resolved = project.resolve_for_file(
            &app_dir.join(Path::new("src/demo/extra.rn")),
            &CompileOptions::default(),
        );

        assert_eq!(resolved.input_file, app_dir.join("src/demo.rn"));
        assert_eq!(resolved.compile_options.root_module_name, None);
    }
}
