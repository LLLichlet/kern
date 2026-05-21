//! Project-resolution support for editor and analysis tooling.
//!
//! A resolved `AnalysisProject` maps arbitrary files back to package targets,
//! generated source roots, compile options, and import aliases needed by the
//! language server.

mod packages;
mod paths;
#[cfg(test)]
mod tests;

pub use self::packages::AnalysisTarget;
use self::packages::{
    AnalysisPackage, AnalysisScriptRoot, PackageEntry, assemble_packages, package_entries,
    target_match_score,
};
use self::paths::{
    build_unit_source_aliases, compile_time_defines, resolve_unit_source_root_path,
    target_kind_from_str, unit_source_root_path,
};
use crate::analysis_context;
use crate::build_plan::{self, BuildPlan};
use crate::elaborate::{self, FeatureSelection};
use crate::error::Result;
use crate::graph::{self, PackageGraph};
use crate::manifest::Manifest;
use crate::plan::TargetKind;
use crate::script::{ProfileSelection, ScriptCommand};
use crate::target_defaults::apply_target_runtime_defaults;
use crate::workspace::{self};
use kernc_utils::config::{CompileOptions, apply_configured_library_aliases};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Debug, Clone)]
pub struct AnalysisProject {
    manifest_path: PathBuf,
    workspace_root: PathBuf,
    packages: Vec<AnalysisPackage>,
    build_plan_cache: Arc<Mutex<BTreeMap<AnalysisBuildPlanKey, BuildPlan>>>,
}

#[derive(Debug, Clone)]
pub struct ResolvedAnalysis {
    pub input_file: PathBuf,
    pub compile_options: CompileOptions,
    pub source_path_aliases: BTreeMap<PathBuf, PathBuf>,
    pub target_roots: Vec<PathBuf>,
    pub target: Option<ResolvedAnalysisTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAnalysisTarget {
    pub manifest_path: PathBuf,
    pub workspace_root: PathBuf,
    pub package_root: PathBuf,
    pub package_name: String,
    pub target_kind: Option<TargetKind>,
    pub target_name: Option<String>,
    pub analysis_context_path: PathBuf,
}

struct AnalysisFileMatch<'a> {
    package: &'a AnalysisPackage,
    input_file: PathBuf,
    target_kind: TargetKind,
    target_name: Option<String>,
    compile_time_values: BTreeMap<String, String>,
    source_path_aliases: BTreeMap<PathBuf, PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct AnalysisBuildPlanKey {
    default_features: bool,
    features: Vec<String>,
}

impl AnalysisBuildPlanKey {
    fn from_compile_options(compile_options: &CompileOptions) -> Self {
        let mut features = compile_options.craft_features.clone();
        features.sort();
        features.dedup();
        Self {
            default_features: compile_options.craft_default_features,
            features,
        }
    }
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

    pub fn contains_path(&self, path: &Path) -> bool {
        self.packages
            .iter()
            .any(|package| path.starts_with(&package.package_root))
    }

    pub fn resolve_for_file(&self, file: &Path, base_options: &CompileOptions) -> ResolvedAnalysis {
        let mut compile_options = base_options.clone();
        let mut input_file = file.to_path_buf();
        let mut resolved_package = None;
        let mut resolved_target_kind = None;
        let mut resolved_target_name = None;
        let mut matched_values = None;
        let mut source_path_aliases = BTreeMap::new();
        let mut target_roots = Vec::new();

        if let Some((package, script_root)) = self.script_root_for_file(file) {
            input_file = script_root.root.clone();
            resolved_package = Some(package);
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
            target_roots = matched
                .package
                .target_roots
                .iter()
                .map(|target_root| target_root.root.clone())
                .collect();
            resolved_package = Some(matched.package);
            resolved_target_kind = Some(matched.target_kind);
            for (name, path) in &matched.package.module_aliases {
                compile_options
                    .module_aliases
                    .entry(name.clone())
                    .or_insert_with(|| path.to_string_lossy().to_string());
            }
            insert_self_library_alias(&mut compile_options, matched.package, matched.target_kind);
            apply_root_module_name(
                &mut compile_options,
                matched.package,
                matched.target_kind,
                matched.target_name.as_deref(),
            );
            resolved_target_name = matched.target_name;
            if matched.target_kind == TargetKind::Test {
                compile_options.test_mode = true;
            }
            compile_options.metadata_package_name = Some(matched.package.id.name.clone());
        } else if let Some(package) = self.package_for_file(file) {
            resolved_package = Some(package);
            target_roots = package
                .target_roots
                .iter()
                .map(|target_root| target_root.root.clone())
                .collect();
            input_file = package.analysis_root_for(file);
            if let Some(target_root) = package.target_root_for(file) {
                resolved_target_kind = Some(target_root.kind);
                resolved_target_name = target_root.name.clone();
            } else if package.lib_root.as_ref() == Some(&input_file) {
                resolved_target_kind = Some(TargetKind::Lib);
            }
            for (name, path) in &package.module_aliases {
                compile_options
                    .module_aliases
                    .entry(name.clone())
                    .or_insert_with(|| path.to_string_lossy().to_string());
            }
            if let Some(target_kind) = resolved_target_kind {
                apply_root_module_name(
                    &mut compile_options,
                    package,
                    target_kind,
                    resolved_target_name.as_deref(),
                );
            }
            if resolved_target_kind == Some(TargetKind::Test) {
                compile_options.test_mode = true;
            }
            insert_self_library_alias(
                &mut compile_options,
                package,
                resolved_target_kind.unwrap_or(TargetKind::Bin),
            );
            compile_options.metadata_package_name = Some(package.id.name.clone());
        }

        if let Some(target_kind) = resolved_target_kind {
            apply_target_runtime_defaults(&mut compile_options, target_kind);
        }

        if let Some(package) = resolved_package {
            if let Some(target_kind) = resolved_target_kind {
                package
                    .manifest
                    .apply_runtime_options_for_target(target_kind, &mut compile_options);
            } else {
                package.manifest.apply_runtime_options(&mut compile_options);
            }
        }

        apply_configured_library_aliases(&mut compile_options);
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
            target_roots,
            target: resolved_package.map(|package| {
                self.resolved_analysis_target(package, resolved_target_kind, resolved_target_name)
            }),
        }
    }

    pub fn workspace_targets(
        &self,
        base_options: &CompileOptions,
    ) -> Result<Vec<ResolvedAnalysis>> {
        let build_plan = self.build_plan_for_analysis(base_options)?;
        let mut targets = Vec::new();

        for build_package in &build_plan.packages {
            if build_package.domain != crate::graph::BuildDomain::Target {
                continue;
            }
            let Some(package) = self.package_for_manifest_path(&build_package.manifest_path) else {
                continue;
            };
            for unit in &build_package.units {
                if unit.domain != crate::graph::BuildDomain::Target {
                    continue;
                }
                let Some(input_file) = resolve_unit_source_root_path(
                    &self.workspace_root,
                    build_package.manifest_path.as_path(),
                    &unit.source_root,
                ) else {
                    continue;
                };

                let mut compile_options = base_options.clone();
                for (name, path) in &package.module_aliases {
                    compile_options
                        .module_aliases
                        .entry(name.clone())
                        .or_insert_with(|| path.to_string_lossy().to_string());
                }
                insert_self_library_alias(&mut compile_options, package, unit.target_kind);
                apply_root_module_name(
                    &mut compile_options,
                    package,
                    unit.target_kind,
                    unit.target_name.as_deref(),
                );
                if unit.target_kind == TargetKind::Test {
                    compile_options.test_mode = true;
                }
                compile_options.metadata_package_name = Some(package.id.name.clone());
                apply_target_runtime_defaults(&mut compile_options, unit.target_kind);
                package
                    .manifest
                    .apply_runtime_options_for_target(unit.target_kind, &mut compile_options);
                apply_configured_library_aliases(&mut compile_options);
                for (name, value) in compile_time_defines(&unit.cfg, &unit.define) {
                    compile_options.custom_defines.entry(name).or_insert(value);
                }

                targets.push(ResolvedAnalysis {
                    input_file: input_file.clone(),
                    compile_options,
                    source_path_aliases: build_unit_source_aliases(&self.workspace_root, unit),
                    target_roots: package
                        .target_roots
                        .iter()
                        .map(|target_root| target_root.root.clone())
                        .collect(),
                    target: Some(self.resolved_analysis_target(
                        package,
                        Some(unit.target_kind),
                        unit.target_name.clone(),
                    )),
                });
            }
        }

        targets.sort_by(|lhs, rhs| lhs.input_file.cmp(&rhs.input_file));
        targets.dedup_by(|lhs, rhs| {
            lhs.input_file == rhs.input_file
                && lhs.compile_options.root_module_name == rhs.compile_options.root_module_name
        });
        Ok(targets)
    }

    pub fn analysis_targets(&self) -> Result<Vec<AnalysisTarget>> {
        let mut targets = self
            .packages
            .iter()
            .flat_map(|package| {
                package.target_roots.iter().map(|target| AnalysisTarget {
                    package_name: package.id.name.clone(),
                    manifest_path: package.manifest_path.clone(),
                    kind: target.kind,
                    name: target.name.clone(),
                    root: target.root.clone(),
                })
            })
            .collect::<Vec<_>>();
        targets.sort_by(|lhs, rhs| {
            (
                lhs.manifest_path.as_path(),
                lhs.kind,
                lhs.name.as_deref().unwrap_or(""),
                lhs.root.as_path(),
            )
                .cmp(&(
                    rhs.manifest_path.as_path(),
                    rhs.kind,
                    rhs.name.as_deref().unwrap_or(""),
                    rhs.root.as_path(),
                ))
        });
        Ok(targets)
    }

    fn from_parts(
        manifest_path: &Path,
        package_graph: PackageGraph,
        package_entries: Vec<PackageEntry>,
    ) -> Self {
        let workspace_root = package_graph.workspace_root.clone();
        let packages = assemble_packages(manifest_path, &package_graph, &package_entries);
        Self {
            manifest_path: manifest_path.to_path_buf(),
            workspace_root,
            packages,
            build_plan_cache: Arc::new(Mutex::new(BTreeMap::new())),
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

    fn script_root_for_file(&self, file: &Path) -> Option<(&AnalysisPackage, &AnalysisScriptRoot)> {
        self.packages.iter().find_map(|package| {
            package
                .script_roots
                .iter()
                .find(|script| script.root == file)
                .map(|script| (package, script))
        })
    }

    fn resolved_analysis_target(
        &self,
        package: &AnalysisPackage,
        target_kind: Option<TargetKind>,
        target_name: Option<String>,
    ) -> ResolvedAnalysisTarget {
        ResolvedAnalysisTarget {
            manifest_path: package.manifest_path.clone(),
            workspace_root: self.workspace_root.clone(),
            package_root: package.package_root.clone(),
            package_name: package.id.name.clone(),
            target_kind,
            target_name,
            analysis_context_path: analysis_context::analysis_context_path(&self.workspace_root),
        }
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
            target_name: None,
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
                        unit.target_name.clone(),
                        compile_time_defines(&unit.cfg, &unit.define),
                    ))
                })
            })
            .max_by_key(|(score, source_root, _, _, _, _, _)| {
                (*score, source_root.components().count())
            })
            .map(
                |(
                    _,
                    source_root,
                    source_path_aliases,
                    package,
                    target_kind,
                    target_name,
                    compile_time_values,
                )| AnalysisFileMatch {
                    package,
                    input_file: source_root,
                    target_kind,
                    target_name,
                    compile_time_values,
                    source_path_aliases,
                },
            )
    }

    fn build_plan_for_analysis(&self, compile_options: &CompileOptions) -> Result<BuildPlan> {
        let cache_key = AnalysisBuildPlanKey::from_compile_options(compile_options);
        {
            let cache = recover_build_plan_cache_lock(&self.build_plan_cache);
            if let Some(plan) = cache.get(&cache_key) {
                return Ok(plan.clone());
            }
        }

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
        let plan = build_plan::derive_with_options(
            &elaboration,
            ScriptCommand::Check,
            build_plan::DeriveOptions {
                include_examples: true,
            },
        )?;
        recover_build_plan_cache_lock(&self.build_plan_cache).insert(cache_key, plan.clone());
        Ok(plan)
    }

    #[cfg(test)]
    fn cached_build_plan_count(&self) -> usize {
        recover_build_plan_cache_lock(&self.build_plan_cache).len()
    }
}

fn recover_build_plan_cache_lock(
    lock: &Mutex<BTreeMap<AnalysisBuildPlanKey, BuildPlan>>,
) -> MutexGuard<'_, BTreeMap<AnalysisBuildPlanKey, BuildPlan>> {
    // The cache only avoids repeated elaboration during editor analysis. If a
    // previous request panicked while updating it, keep the recovered map rather
    // than making every later project resolution fail on a poisoned mutex.
    match lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn insert_self_library_alias(
    compile_options: &mut CompileOptions,
    package: &AnalysisPackage,
    target_kind: TargetKind,
) {
    if target_kind == TargetKind::Lib {
        return;
    }

    let Some(lib_root) = &package.lib_root else {
        return;
    };

    compile_options
        .module_aliases
        .entry(package.id.name.clone())
        .or_insert_with(|| lib_root.to_string_lossy().to_string());
}

fn apply_root_module_name(
    compile_options: &mut CompileOptions,
    package: &AnalysisPackage,
    target_kind: TargetKind,
    target_name: Option<&str>,
) {
    if let Some(name) = official_library_root_module_name(package, target_kind, target_name) {
        compile_options.root_module_name = Some(name);
        return;
    }

    if target_kind == TargetKind::Lib {
        compile_options.root_module_name = Some(package.id.name.clone());
    }
}

fn official_library_root_module_name(
    package: &AnalysisPackage,
    target_kind: TargetKind,
    target_name: Option<&str>,
) -> Option<String> {
    if !package_is_in_official_library_workspace(package) {
        return None;
    }

    match package.id.name.as_str() {
        "base" | "std" | "rt" if target_kind == TargetKind::Lib => Some(package.id.name.clone()),
        "kernlib-test" if target_kind == TargetKind::Test => Some(
            target_name
                .map(sanitize_root_module_name)
                .unwrap_or_else(|| "kernlib_test".to_string()),
        ),
        _ => None,
    }
}

fn package_is_in_official_library_workspace(package: &AnalysisPackage) -> bool {
    package
        .manifest_path
        .parent()
        .and_then(Path::parent)
        .is_some_and(|root| {
            root.join("base").join("mod.kn").is_file()
                && root.join("std").join("mod.kn").is_file()
                && root.join("rt").join("mod.kn").is_file()
        })
}

fn sanitize_root_module_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch == '_' || ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "root".to_string()
    } else {
        out
    }
}

pub fn resolve_project_manifest_path(input: Option<&Path>) -> Result<PathBuf> {
    crate::discover::resolve_project_manifest_path(input)
}
