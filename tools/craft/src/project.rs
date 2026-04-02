use crate::analysis_context;
use crate::build_plan::{self, BuildPlan};
use crate::elaborate::{self, FeatureSelection};
use crate::error::Result;
use crate::graph::{self, DependencyTarget, PackageGraph, PackageId, SourceId};
use crate::manifest::Manifest;
use crate::plan::{PackagePlan, TargetKind};
use crate::script::{ProfileSelection, ScriptCommand};
use crate::workspace::{self, WorkspaceMember};
use kernc_utils::config::{CompileOptions, maybe_inject_std_alias};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AnalysisProject {
    manifest_path: PathBuf,
    workspace_root: PathBuf,
    packages: Vec<AnalysisPackage>,
    workspace_script_roots: Vec<AnalysisScriptRoot>,
}

#[derive(Debug, Clone)]
struct AnalysisPackage {
    id: PackageId,
    manifest_path: PathBuf,
    package_root: PathBuf,
    lib_root: Option<PathBuf>,
    target_roots: Vec<PathBuf>,
    module_aliases: BTreeMap<String, PathBuf>,
    script_roots: Vec<AnalysisScriptRoot>,
}

#[derive(Debug, Clone)]
struct AnalysisScriptRoot {
    root: PathBuf,
    module_aliases: BTreeMap<String, PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ResolvedAnalysis {
    pub input_file: PathBuf,
    pub compile_options: CompileOptions,
    pub source_path_aliases: BTreeMap<PathBuf, PathBuf>,
}

struct AnalysisFileMatch<'a> {
    package: &'a AnalysisPackage,
    input_file: PathBuf,
    target_kind: TargetKind,
    compile_time_values: BTreeMap<String, String>,
    source_path_aliases: BTreeMap<PathBuf, PathBuf>,
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
        let mut resolved_package = None;
        let mut matched_values = None;
        let mut source_path_aliases = BTreeMap::new();

        if let Some(script_root) = self.script_root_for_file(file) {
            input_file = script_root.root.clone();
            for (name, path) in &script_root.module_aliases {
                compile_options
                    .module_aliases
                    .entry(name.clone())
                    .or_insert_with(|| path.to_string_lossy().to_string());
            }
        } else if let Some(matched) = self.analysis_match_for_file(file, &compile_options) {
            input_file = matched.input_file.clone();
            matched_values = Some(matched.compile_time_values);
            source_path_aliases = matched.source_path_aliases;
            resolved_package = Some(matched.package);
            for (name, path) in &matched.package.module_aliases {
                compile_options
                    .module_aliases
                    .entry(name.clone())
                    .or_insert_with(|| path.to_string_lossy().to_string());
            }
            if matched.target_kind == TargetKind::Lib {
                compile_options.root_module_name = Some(matched.package.id.name.clone());
            }
        } else if let Some(package) = self.package_for_file(file) {
            resolved_package = Some(package);
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
        if let Some(values) = matched_values {
            for (name, value) in values {
                compile_options.custom_defines.entry(name).or_insert(value);
            }
        } else if let Some(package) = resolved_package {
            self.apply_craft_compile_options(package, &input_file, &mut compile_options);
        }

        ResolvedAnalysis {
            input_file,
            compile_options,
            source_path_aliases,
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
                manifest_path: entry.manifest_path.clone(),
                package_root: entry.package_root.clone(),
                lib_root: entry.lib_root.clone(),
                target_roots: entry.target_roots.clone(),
                module_aliases,
                script_roots: script_roots_for_package_root(&entry.package_root),
            });
        }

        Self {
            manifest_path: manifest_path.to_path_buf(),
            workspace_root: package_graph.workspace_root,
            packages,
            workspace_script_roots: workspace_script_roots(manifest_path),
        }
    }

    fn package_for_file(&self, file: &Path) -> Option<&AnalysisPackage> {
        self.packages
            .iter()
            .filter(|package| file.starts_with(&package.package_root))
            .max_by_key(|package| package.package_root.components().count())
    }

    fn package_for_manifest_path(&self, manifest_path: &Path) -> Option<&AnalysisPackage> {
        self.packages
            .iter()
            .find(|package| package.manifest_path == manifest_path)
    }

    fn script_root_for_file(&self, file: &Path) -> Option<&AnalysisScriptRoot> {
        self.workspace_script_roots
            .iter()
            .chain(
                self.packages
                    .iter()
                    .flat_map(|package| package.script_roots.iter()),
            )
            .filter_map(|script| {
                target_match_score(&script.root, file).map(|score| (score, script))
            })
            .max_by_key(|(score, script)| (*score, script.root.components().count()))
            .map(|(_, script)| script)
    }

    fn apply_craft_compile_options(
        &self,
        package: &AnalysisPackage,
        input_file: &Path,
        compile_options: &mut CompileOptions,
    ) {
        if self.prefers_persisted_analysis_context(compile_options)
            && let Some(values) = self.persisted_compile_time_values(package, input_file)
        {
            for (name, value) in values {
                compile_options.custom_defines.entry(name).or_insert(value);
            }
            return;
        }

        let Ok(build_plan) = self.build_plan_for_analysis(compile_options) else {
            return;
        };
        let Some(unit) = build_plan.packages.iter().find_map(|build_package| {
            if build_package.domain != crate::graph::BuildDomain::Target
                || build_package.package_id != package.id
            {
                return None;
            }

            build_package.units.iter().find(|unit| {
                unit_source_root_path(build_package.manifest_path.as_path(), &unit.source_root)
                    .as_deref()
                    == Some(input_file)
            })
        }) else {
            return;
        };

        for (name, value) in compile_time_defines(&unit.cfg, &unit.define) {
            compile_options.custom_defines.entry(name).or_insert(value);
        }
    }

    fn analysis_match_for_file<'a>(
        &'a self,
        file: &Path,
        compile_options: &CompileOptions,
    ) -> Option<AnalysisFileMatch<'a>> {
        if self.prefers_persisted_analysis_context(compile_options)
            && let Some(matched) = self.persisted_analysis_match_for_file(file)
        {
            return Some(matched);
        }

        let build_plan = self.build_plan_for_analysis(compile_options).ok()?;
        self.build_plan_match_for_file(file, &build_plan)
    }

    fn prefers_persisted_analysis_context(&self, compile_options: &CompileOptions) -> bool {
        compile_options.craft_default_features && compile_options.craft_features.is_empty()
    }

    fn persisted_compile_time_values(
        &self,
        package: &AnalysisPackage,
        input_file: &Path,
    ) -> Option<BTreeMap<String, String>> {
        let context = analysis_context::load_current_analysis_context(
            &self.manifest_path,
            &self.workspace_root,
        )
        .ok()??;
        context.compile_time_values_for(&package.manifest_path, input_file, &self.workspace_root)
    }

    fn persisted_analysis_match_for_file<'a>(
        &'a self,
        file: &Path,
    ) -> Option<AnalysisFileMatch<'a>> {
        let context = analysis_context::load_current_analysis_context(
            &self.manifest_path,
            &self.workspace_root,
        )
        .ok()??;
        let matched = context.match_unit_for(file, &self.workspace_root)?;
        let package = self.package_for_manifest_path(&matched.manifest_path)?;
        Some(AnalysisFileMatch {
            package,
            input_file: matched.source_root,
            target_kind: target_kind_from_str(&matched.target_kind)?,
            compile_time_values: matched.compile_time_values,
            source_path_aliases: matched.source_path_aliases,
        })
    }

    fn build_plan_match_for_file<'a>(
        &'a self,
        file: &Path,
        build_plan: &BuildPlan,
    ) -> Option<AnalysisFileMatch<'a>> {
        build_plan
            .packages
            .iter()
            .filter(|package| package.domain == crate::graph::BuildDomain::Target)
            .flat_map(|package| {
                package.units.iter().filter_map(|unit| {
                    if unit.domain != crate::graph::BuildDomain::Target {
                        return None;
                    }
                    let source_root = resolve_unit_source_root_path(
                        &self.workspace_root,
                        package.manifest_path.as_path(),
                        &unit.source_root,
                    )?;
                    let source_path_aliases = build_unit_source_aliases(&self.workspace_root, unit);
                    let score = target_match_score(&source_root, file).or_else(|| {
                        source_path_aliases
                            .keys()
                            .find(|source_path| source_path.as_path() == file)
                            .map(|_| usize::MAX)
                    })?;
                    let analysis_package =
                        self.package_for_manifest_path(&package.manifest_path)?;
                    Some((
                        score,
                        source_root,
                        source_path_aliases,
                        analysis_package,
                        unit.target_kind,
                        compile_time_defines(&unit.cfg, &unit.define),
                    ))
                })
            })
            .max_by_key(|(score, source_root, _, _, _, _)| {
                (*score, source_root.components().count())
            })
            .map(
                |(
                    _,
                    source_root,
                    source_path_aliases,
                    package,
                    target_kind,
                    compile_time_values,
                )| AnalysisFileMatch {
                    package,
                    input_file: source_root,
                    target_kind,
                    compile_time_values,
                    source_path_aliases,
                },
            )
    }

    fn build_plan_for_analysis(&self, compile_options: &CompileOptions) -> Result<BuildPlan> {
        let manifest = Manifest::load(&self.manifest_path)?;
        manifest.validate(&self.manifest_path)?;
        let workspace_members = workspace::load_members(&self.manifest_path, &manifest)?;
        let feature_selection = FeatureSelection {
            enable_default: compile_options.craft_default_features,
            explicit: compile_options.craft_features.iter().cloned().collect(),
            profile: ProfileSelection::Dev,
        };
        let elaboration = elaborate::plan(
            &self.manifest_path,
            &manifest,
            &workspace_members,
            manifest.workspace.is_some(),
            ScriptCommand::Build,
            &feature_selection,
        )?;
        build_plan::derive(&elaboration, ScriptCommand::Build)
    }
}

fn compile_time_defines(
    cfg: &BTreeMap<String, crate::plan::PlanValue>,
    define: &BTreeMap<String, crate::plan::PlanValue>,
) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for (name, value) in cfg {
        values.insert(name.clone(), plan_value_string(value));
    }
    for (name, value) in define {
        values.insert(name.clone(), plan_value_string(value));
    }
    values
}

fn build_unit_source_aliases(
    workspace_root: &Path,
    unit: &crate::build_plan::BuildUnit,
) -> BTreeMap<PathBuf, PathBuf> {
    unit.generated_files
        .iter()
        .filter_map(|generated| {
            let crate::build_plan::GeneratedFileOrigin::Copied { source } = &generated.origin
            else {
                return None;
            };
            Some((
                resolve_context_path(workspace_root, source),
                resolve_context_path(workspace_root, &generated.path),
            ))
        })
        .collect()
}

fn plan_value_string(value: &crate::plan::PlanValue) -> String {
    match value {
        crate::plan::PlanValue::Bool(value) => value.to_string(),
        crate::plan::PlanValue::String(value) => value.clone(),
    }
}

fn unit_source_root_path(
    manifest_path: &Path,
    source_root: &crate::build_plan::SourceRootBinding,
) -> Option<PathBuf> {
    let package_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    match source_root {
        crate::build_plan::SourceRootBinding::PackagePath(path) => Some(package_root.join(path)),
        crate::build_plan::SourceRootBinding::AbsolutePath(path) => Some(PathBuf::from(path)),
        crate::build_plan::SourceRootBinding::BuildOutput { .. } => None,
    }
}

fn resolve_unit_source_root_path(
    workspace_root: &Path,
    manifest_path: &Path,
    source_root: &crate::build_plan::SourceRootBinding,
) -> Option<PathBuf> {
    let package_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    match source_root {
        crate::build_plan::SourceRootBinding::PackagePath(path) => Some(package_root.join(path)),
        crate::build_plan::SourceRootBinding::AbsolutePath(path) => Some(PathBuf::from(path)),
        crate::build_plan::SourceRootBinding::BuildOutput { path, .. } => {
            let path = Path::new(path);
            Some(if path.is_absolute() {
                path.to_path_buf()
            } else {
                workspace_root.join(path)
            })
        }
    }
}

fn resolve_context_path(workspace_root: &Path, stored_path: &str) -> PathBuf {
    let path = Path::new(stored_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    }
}

fn target_kind_from_str(raw: &str) -> Option<TargetKind> {
    match raw {
        "lib" => Some(TargetKind::Lib),
        "bin" => Some(TargetKind::Bin),
        "test" => Some(TargetKind::Test),
        "example" => Some(TargetKind::Example),
        _ => None,
    }
}

pub fn resolve_project_manifest_path(input: Option<&Path>) -> Result<PathBuf> {
    let manifest_path = discover_manifest_path(input)?;
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

fn discover_manifest_path(input: Option<&Path>) -> Result<PathBuf> {
    let start = match input {
        Some(path) if path.file_name().and_then(|name| name.to_str()) == Some("Craft.toml") => {
            return Ok(path.to_path_buf());
        }
        Some(path) if path.is_dir() => path.to_path_buf(),
        Some(path) => path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
        None => std::env::current_dir().map_err(crate::error::Error::from_io_plain)?,
    };

    let mut current = Some(start.as_path());
    while let Some(dir) = current {
        let candidate = dir.join("Craft.toml");
        if candidate.is_file() {
            return Ok(candidate);
        }
        current = dir.parent();
    }

    Err(crate::error::Error::ManifestNotFound { start })
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

fn sdk_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk")
}

fn craft_sdk_aliases() -> BTreeMap<String, PathBuf> {
    BTreeMap::from([(String::from("craft"), sdk_root())])
}

fn script_roots_for_package_root(package_root: &Path) -> Vec<AnalysisScriptRoot> {
    ["craft.rn", "build.rn"]
        .into_iter()
        .map(|name| AnalysisScriptRoot {
            root: package_root.join(name),
            module_aliases: craft_sdk_aliases(),
        })
        .collect()
}

fn workspace_script_roots(manifest_path: &Path) -> Vec<AnalysisScriptRoot> {
    let Some(workspace_root) = manifest_path.parent() else {
        return Vec::new();
    };

    vec![AnalysisScriptRoot {
        root: workspace_root.join("craft.rn"),
        module_aliases: craft_sdk_aliases(),
    }]
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
    manifest_path: PathBuf,
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
        manifest_path: manifest_path.to_path_buf(),
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
    use crate::analysis_context;
    use crate::build_plan;
    use crate::elaborate::{FeatureSelection, plan};
    use crate::manifest::Manifest;
    use crate::plan::TargetKind;
    use crate::workspace::load_members;
    use kernc_utils::config::CompileOptions;
    use std::collections::HashMap;
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

    fn with_env_var<T>(name: &str, value: &str, f: impl FnOnce() -> T) -> T {
        let previous = std::env::var_os(name);
        // SAFETY: tests use unique environment variable names and restore the
        // previous value before returning.
        unsafe {
            std::env::set_var(name, value);
        }
        let result = f();
        // SAFETY: restores the process environment to its previous state.
        unsafe {
            if let Some(previous) = previous {
                std::env::set_var(name, previous);
            } else {
                std::env::remove_var(name);
            }
        }
        result
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

    #[test]
    fn resolves_package_craft_script_with_sdk_alias_even_when_library_exists() {
        let root = temp_dir("craft-project-script-analysis");
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
",
        )
        .unwrap();
        fs::write(app_dir.join("src/lib.rn"), "pub fn helper() void {}\n").unwrap();
        fs::write(
            app_dir.join("craft.rn"),
            "use craft.plan;\npub fn craft(p: *mut plan.Plan) void { let _ = p; }\n",
        )
        .unwrap();

        let project = AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
        let resolved =
            project.resolve_for_file(&app_dir.join("craft.rn"), &CompileOptions::default());

        assert_eq!(resolved.input_file, app_dir.join("craft.rn"));
        assert_eq!(
            resolved
                .compile_options
                .module_aliases
                .get("craft")
                .map(PathBuf::from),
            Some(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("sdk")
                    .join("init.rn")
                    .parent()
                    .unwrap()
                    .to_path_buf()
            )
        );
        assert_eq!(resolved.compile_options.root_module_name, None);
    }

    #[test]
    fn resolves_workspace_craft_script_with_sdk_alias() {
        let root = temp_dir("craft-workspace-script-analysis");
        fs::create_dir_all(root.join("app/src")).unwrap();

        fs::write(
            root.join("Craft.toml"),
            "[workspace]\nmembers = [\"app\"]\n",
        )
        .unwrap();
        fs::write(
            root.join("craft.rn"),
            "use craft.plan;\npub fn craft(p: *mut plan.Plan) void { let _ = p; }\n",
        )
        .unwrap();
        fs::write(
            root.join("app/Craft.toml"),
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.7\"

[[bin]]
name = \"app\"
root = \"src/main.rn\"
",
        )
        .unwrap();
        fs::write(
            root.join("app/src/main.rn"),
            "fn main() i32 { return 0; }\n",
        )
        .unwrap();

        let project = AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
        let resolved = project.resolve_for_file(&root.join("craft.rn"), &CompileOptions::default());

        assert_eq!(resolved.input_file, root.join("craft.rn"));
        assert_eq!(
            resolved
                .compile_options
                .module_aliases
                .get("craft")
                .map(PathBuf::from),
            Some(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("sdk")
                    .join("init.rn")
                    .parent()
                    .unwrap()
                    .to_path_buf()
            )
        );
    }

    #[test]
    fn resolve_for_file_applies_craft_cfg_and_define_values() {
        let root = temp_dir("craft-project-custom-defines");
        fs::create_dir_all(root.join("src")).unwrap();
        let env_name = format!(
            "KERN_PROJECT_ANALYSIS_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );

        fs::write(
            root.join("Craft.toml"),
            format!(
                "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.7\"

[features]
experimental = []

[craft]
env = [\"{env_name}\"]

[[bin]]
name = \"app\"
root = \"src/main.rn\"
"
            ),
        )
        .unwrap();
        fs::write(
            root.join("craft.rn"),
            format!(
                "\
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {{
    if (p.feature_enabled(\"experimental\")) {{
        p.cfg_bool(\"enable_telemetry\", true);
        p.define_string(\"GREETING_MSG\", \"Hello from craft\");
    }}

    if (p.env(\"{env_name}\") != .None) {{
        p.cfg_bool(\"is_dev_env\", true);
    }}
}}
"
            ),
        )
        .unwrap();
        fs::write(
            root.join("src/main.rn"),
            "extern fn main() i32 { return 0; }\n",
        )
        .unwrap();

        let project = AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
        let mut options = CompileOptions::default();
        options.craft_features.push("experimental".to_string());

        let resolved = with_env_var(&env_name, "1", || {
            project.resolve_for_file(&root.join("src/main.rn"), &options)
        });

        let defines = &resolved.compile_options.custom_defines;
        let collected = defines
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<HashMap<_, _>>();
        assert_eq!(
            collected.get("enable_telemetry").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            collected.get("is_dev_env").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            collected.get("GREETING_MSG").map(String::as_str),
            Some("Hello from craft")
        );
    }

    #[test]
    fn resolve_for_file_prefers_persisted_analysis_context_without_explicit_features() {
        let root = temp_dir("craft-project-persisted-analysis");
        fs::create_dir_all(root.join("src")).unwrap();
        let env_name = format!(
            "KERN_PROJECT_PERSISTED_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );

        fs::write(
            root.join("Craft.toml"),
            format!(
                "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.7\"

[features]
experimental = []

[craft]
env = [\"{env_name}\"]

[[bin]]
name = \"app\"
root = \"src/main.rn\"
"
            ),
        )
        .unwrap();
        fs::write(
            root.join("craft.rn"),
            format!(
                "\
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {{
    if (p.feature_enabled(\"experimental\")) {{
        p.cfg_bool(\"enable_telemetry\", true);
        p.define_string(\"GREETING_MSG\", \"Hello from craft\");
    }}

    if (p.env(\"{env_name}\") != .None) {{
        p.cfg_bool(\"is_dev_env\", true);
    }}
}}
"
            ),
        )
        .unwrap();
        fs::write(
            root.join("src/main.rn"),
            "extern fn main() i32 { return 0; }\n",
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let workspace_members = load_members(&manifest_path, &manifest).unwrap();
        let mut selection = FeatureSelection::default();
        selection.explicit.insert("experimental".to_string());
        let elaboration = with_env_var(&env_name, "1", || {
            plan(
                &manifest_path,
                &manifest,
                &workspace_members,
                false,
                crate::script::ScriptCommand::Build,
                &selection,
            )
            .unwrap()
        });
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        analysis_context::sync_analysis_context(
            &manifest_path,
            &elaboration,
            &build_plan,
            &selection,
        )
        .unwrap();

        let project = AnalysisProject::load_from_manifest(&manifest_path).unwrap();
        let resolved =
            project.resolve_for_file(&root.join("src/main.rn"), &CompileOptions::default());
        let defines = resolved
            .compile_options
            .custom_defines
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<HashMap<_, _>>();

        assert_eq!(
            defines.get("enable_telemetry").map(String::as_str),
            Some("true")
        );
        assert_eq!(defines.get("is_dev_env").map(String::as_str), Some("true"));
        assert_eq!(
            defines.get("GREETING_MSG").map(String::as_str),
            Some("Hello from craft")
        );
    }

    #[test]
    fn resolve_for_generated_source_root_uses_analysis_unit_matching() {
        let root = temp_dir("craft-project-generated-analysis");

        fs::write(
            root.join("Craft.toml"),
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.7\"

[[bin]]
name = \"app\"
root = \"src/placeholder.rn\"
",
        )
        .unwrap();
        fs::write(
            root.join("build.rn"),
            "\
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    let generated = b.emit_generated(
        \"src/main.rn\",
        \"extern fn main(args: [][]u8) i32 { let _ = args; return 0; }\\n\"
    );
    b.set_source_root(generated);
    b.cfg_bool(\"generated\", true);
    b.define_string(\"ENTRY_KIND\", \"generated\");
}
",
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let workspace_members = load_members(&manifest_path, &manifest).unwrap();
        let selection = FeatureSelection::default();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &workspace_members,
            false,
            crate::script::ScriptCommand::Build,
            &selection,
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let generated_root = build_plan.packages[0]
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Bin)
            .and_then(|unit| match &unit.source_root {
                crate::build_plan::SourceRootBinding::AbsolutePath(path) => {
                    Some(PathBuf::from(path))
                }
                _ => None,
            })
            .expect("expected generated source root");
        analysis_context::sync_analysis_context(
            &manifest_path,
            &elaboration,
            &build_plan,
            &selection,
        )
        .unwrap();

        let project = AnalysisProject::load_from_manifest(&manifest_path).unwrap();
        let resolved = project.resolve_for_file(&generated_root, &CompileOptions::default());
        let defines = resolved
            .compile_options
            .custom_defines
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<HashMap<_, _>>();

        assert_eq!(resolved.input_file, generated_root);
        assert_eq!(defines.get("generated").map(String::as_str), Some("true"));
        assert_eq!(
            defines.get("ENTRY_KIND").map(String::as_str),
            Some("generated")
        );
    }

    #[test]
    fn resolve_for_copied_template_source_uses_generated_unit_root() {
        let root = temp_dir("craft-project-generated-alias");
        fs::create_dir_all(root.join("src")).unwrap();

        fs::write(
            root.join("Craft.toml"),
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.7\"

[[bin]]
name = \"app\"
root = \"src/placeholder.rn\"
",
        )
        .unwrap();
        fs::write(
            root.join("src/main.rn"),
            "mod build_info;\nextern fn main(args: [][]u8) i32 { let _ = args; let _ = build_info.MAGIC_NUMBER; return 0; }\n",
        )
        .unwrap();
        fs::write(
            root.join("build.rn"),
            "\
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    let main = b.stage_copy_package_file(\"src/main.rn\", \"src/main.rn\");
    let _ = b.stage_generated(
        \"src/build_info.rn\",
        \"pub const MAGIC_NUMBER: i32 = 42;\\n\"
    );
    b.set_source_root_from(main);
    b.cfg_bool(\"generated\", true);
}
",
        )
        .unwrap();

        analysis_context::sync_project_analysis_context(&root.join("Craft.toml"), true, &[])
            .unwrap();

        let project = AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
        let resolved =
            project.resolve_for_file(&root.join("src/main.rn"), &CompileOptions::default());
        let generated_main = root
            .join(".craft")
            .join("build")
            .join("dev")
            .join("target")
            .join("gen")
            .join("app-0.1.0")
            .join("bin")
            .join("app")
            .join("src")
            .join("main.rn");
        let generated_info = generated_main.parent().unwrap().join("build_info.rn");

        assert_eq!(resolved.input_file, generated_main);
        assert_eq!(
            resolved
                .source_path_aliases
                .get(&root.join("src/main.rn"))
                .cloned(),
            Some(generated_main.clone())
        );
        assert!(generated_info.is_file());
        assert_eq!(
            resolved
                .compile_options
                .custom_defines
                .get("generated")
                .map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn explicit_feature_selection_overrides_persisted_analysis_context() {
        let root = temp_dir("craft-project-explicit-overrides-persisted");
        fs::create_dir_all(root.join("src")).unwrap();

        fs::write(
            root.join("Craft.toml"),
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.7\"

[features]
experimental = []
stable = []

[[bin]]
name = \"app\"
root = \"src/main.rn\"
",
        )
        .unwrap();
        fs::write(
            root.join("craft.rn"),
            "\
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {
    if (p.feature_enabled(\"experimental\")) {
        p.cfg_bool(\"mode_experimental\", true);
    }
    if (p.feature_enabled(\"stable\")) {
        p.cfg_bool(\"mode_stable\", true);
    }
}
",
        )
        .unwrap();
        fs::write(
            root.join("src/main.rn"),
            "extern fn main() i32 { return 0; }\n",
        )
        .unwrap();

        analysis_context::sync_project_analysis_context(
            &root.join("Craft.toml"),
            true,
            &[String::from("experimental")],
        )
        .unwrap();

        let project = AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
        let mut options = CompileOptions::default();
        options.craft_features.push("stable".to_string());
        let resolved = project.resolve_for_file(&root.join("src/main.rn"), &options);
        let defines = resolved
            .compile_options
            .custom_defines
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<HashMap<_, _>>();

        assert_eq!(defines.get("mode_experimental").map(String::as_str), None);
        assert_eq!(defines.get("mode_stable").map(String::as_str), Some("true"));
    }

    #[test]
    fn resolve_project_manifest_path_handles_nonexistent_generated_descendant() {
        let root = temp_dir("craft-project-discover-generated");
        fs::write(
            root.join("Craft.toml"),
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.7\"
",
        )
        .unwrap();

        let generated_path = root
            .join(".craft")
            .join("build")
            .join("dev")
            .join("target")
            .join("gen")
            .join("app-0.1.0")
            .join("bin")
            .join("app")
            .join("src")
            .join("main.rn");

        let manifest = super::resolve_project_manifest_path(Some(&generated_path)).unwrap();
        assert_eq!(manifest, root.join("Craft.toml"));
    }
}
