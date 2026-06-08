//! Persisted editor-analysis context for Craft projects.
//!
//! The context captures the resolved build plan and manifest digest that the
//! LSP server can reuse between requests without re-running full project
//! elaboration when the workspace has not changed.

use crate::build_plan::{BuildPlan, SourceRootBinding};
use crate::elaborate::{ElaborationPlan, FeatureSelection};
use crate::error::{Error, Result};
use crate::execute;
use crate::local_state;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

mod parse;
mod render;
#[cfg(test)]
mod tests;
mod validate;

use self::render::compile_time_values;

const ANALYSIS_CONTEXT_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisContext {
    version: u32,
    manifest: String,
    manifest_digest: String,
    profile: String,
    default_features: bool,
    features: Vec<String>,
    packages: Vec<AnalysisContextPackage>,
    units: Vec<AnalysisContextUnit>,
    unit_aliases: Vec<AnalysisContextUnitAlias>,
    unit_values: Vec<AnalysisContextUnitValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AnalysisContextPackage {
    manifest: String,
    manifest_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct AnalysisContextUnit {
    manifest: String,
    source_root: String,
    target_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct AnalysisContextUnitAlias {
    manifest: String,
    source_root: String,
    source_path: String,
    generated_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct AnalysisContextUnitValue {
    manifest: String,
    source_root: String,
    name: String,
    value: String,
}

#[derive(Clone, Copy, Debug)]
enum Section {
    Root,
    Package(usize),
    Unit(usize),
    UnitAlias(usize),
    UnitValue(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedAnalysisContextUnit {
    pub manifest_path: PathBuf,
    pub source_root: PathBuf,
    pub target_kind: String,
    pub source_path_aliases: BTreeMap<PathBuf, PathBuf>,
    pub compile_time_values: BTreeMap<String, String>,
}

pub fn analysis_context_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".craft").join("analysis.toml")
}

pub fn sync_analysis_context(
    manifest_path: &Path,
    elaboration: &ElaborationPlan,
    build_plan: &BuildPlan,
    feature_selection: &FeatureSelection,
) -> Result<PathBuf> {
    let context =
        AnalysisContext::from_inputs(manifest_path, elaboration, build_plan, feature_selection)?;
    let path = analysis_context_path(&build_plan.workspace_root);
    local_state::write_file_atomic(&path, context.render())?;
    Ok(path)
}

pub fn sync_project_analysis_context(
    manifest_path: &Path,
    default_features: bool,
    features: &[String],
) -> Result<PathBuf> {
    let manifest = crate::manifest::Manifest::load(manifest_path)?;
    manifest.validate(manifest_path)?;
    let workspace_members = crate::workspace::load_members(manifest_path, &manifest)?;
    let feature_selection = FeatureSelection {
        enable_default: default_features,
        explicit: features.iter().cloned().collect(),
        profile: crate::script::ProfileSelection::Dev,
    };
    let elaboration = crate::elaborate::plan(
        manifest_path,
        &manifest,
        &workspace_members,
        manifest.workspace.is_some(),
        crate::script::ScriptCommand::Build,
        &feature_selection,
    )?;
    let build_plan = crate::build_plan::derive(&elaboration, crate::script::ScriptCommand::Build)?;
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    execute::materialize_analysis_inputs(&build_plan, &action_plan)?;
    sync_analysis_context(manifest_path, &elaboration, &build_plan, &feature_selection)
}

pub fn load_current_analysis_context(
    manifest_path: &Path,
    workspace_root: &Path,
) -> Result<Option<AnalysisContext>> {
    let path = analysis_context_path(workspace_root);
    if !path.is_file() {
        return Ok(None);
    }

    let source = fs::read_to_string(&path).map_err(|err| Error::from_io(&path, err))?;
    let context = match AnalysisContext::parse(&source, &path) {
        Ok(context) => context,
        Err(_) => return Ok(None),
    };
    if !context.is_current(manifest_path, workspace_root)? {
        return Ok(None);
    }
    Ok(Some(context))
}

impl AnalysisContext {
    fn from_inputs(
        manifest_path: &Path,
        elaboration: &ElaborationPlan,
        build_plan: &BuildPlan,
        feature_selection: &FeatureSelection,
    ) -> Result<Self> {
        let workspace_root = &build_plan.workspace_root;
        let manifest = relative_display(workspace_root, manifest_path);
        let manifest_digest = digest_file(manifest_path)?;

        let mut packages = elaboration
            .resolved_graph
            .packages
            .iter()
            .map(|package| {
                Ok(AnalysisContextPackage {
                    manifest: relative_display(workspace_root, &package.manifest_path),
                    manifest_digest: digest_file(&package.manifest_path)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        packages.sort_by(|lhs, rhs| lhs.manifest.cmp(&rhs.manifest));

        let mut units = BTreeSet::new();
        let mut unit_aliases = BTreeSet::new();
        let mut unit_values = BTreeMap::new();
        for package in &build_plan.packages {
            if package.domain != crate::graph::BuildDomain::Target {
                continue;
            }

            let manifest_key = relative_display(workspace_root, &package.manifest_path);
            for unit in &package.units {
                if unit.domain != crate::graph::BuildDomain::Target {
                    continue;
                }

                let Some(source_root) = resolve_source_root_path(
                    workspace_root,
                    &package.manifest_path,
                    &unit.source_root,
                ) else {
                    continue;
                };
                let source_root = relative_display(workspace_root, &source_root);
                units.insert(AnalysisContextUnit {
                    manifest: manifest_key.clone(),
                    source_root: source_root.clone(),
                    target_kind: unit.target_kind.as_str().to_string(),
                });

                for generated in &unit.generated_files {
                    let crate::build_plan::GeneratedFileOrigin::Copied { source } =
                        &generated.origin
                    else {
                        continue;
                    };
                    unit_aliases.insert(AnalysisContextUnitAlias {
                        manifest: manifest_key.clone(),
                        source_root: source_root.clone(),
                        source_path: source.clone(),
                        generated_path: generated.path.clone(),
                    });
                }

                for (name, value) in compile_time_values(&unit.cfg, &unit.define) {
                    unit_values.insert(
                        (manifest_key.clone(), source_root.clone(), name.clone()),
                        AnalysisContextUnitValue {
                            manifest: manifest_key.clone(),
                            source_root: source_root.clone(),
                            name,
                            value,
                        },
                    );
                }
            }
        }

        Ok(Self {
            version: ANALYSIS_CONTEXT_VERSION,
            manifest,
            manifest_digest,
            profile: feature_selection.profile.name().to_string(),
            default_features: feature_selection.enable_default,
            features: feature_selection.explicit.iter().cloned().collect(),
            packages,
            units: units.into_iter().collect(),
            unit_aliases: unit_aliases.into_iter().collect(),
            unit_values: unit_values.into_values().collect(),
        })
    }

    pub fn compile_time_values_for(
        &self,
        package_manifest_path: &Path,
        input_file: &Path,
        workspace_root: &Path,
    ) -> Option<BTreeMap<String, String>> {
        let manifest = relative_display(workspace_root, package_manifest_path);
        let source_root = relative_display(workspace_root, input_file);
        if !self
            .units
            .iter()
            .any(|unit| unit.manifest == manifest && unit.source_root == source_root)
        {
            return None;
        }

        Some(
            self.unit_values
                .iter()
                .filter(|value| value.manifest == manifest && value.source_root == source_root)
                .map(|value| (value.name.clone(), value.value.clone()))
                .collect(),
        )
    }

    pub fn match_unit_for(
        &self,
        file: &Path,
        workspace_root: &Path,
    ) -> Option<MatchedAnalysisContextUnit> {
        self.units
            .iter()
            .filter_map(|unit| {
                let source_root = resolve_context_path(workspace_root, &unit.source_root);
                let alias_map = self
                    .unit_aliases
                    .iter()
                    .filter(|alias| {
                        alias.manifest == unit.manifest && alias.source_root == unit.source_root
                    })
                    .map(|alias| {
                        (
                            resolve_context_path(workspace_root, &alias.source_path),
                            resolve_context_path(workspace_root, &alias.generated_path),
                        )
                    })
                    .collect::<BTreeMap<_, _>>();
                let score = target_match_score(&source_root, file).or_else(|| {
                    alias_map
                        .keys()
                        .find(|source_path| source_path.as_path() == file)
                        .map(|_| usize::MAX)
                })?;
                Some((score, unit, source_root, alias_map))
            })
            .max_by_key(|(score, _, source_root, _)| (*score, source_root.components().count()))
            .map(
                |(_, unit, source_root, source_path_aliases)| MatchedAnalysisContextUnit {
                    manifest_path: resolve_context_path(workspace_root, &unit.manifest),
                    source_root,
                    target_kind: unit.target_kind.clone(),
                    source_path_aliases,
                    compile_time_values: self
                        .unit_values
                        .iter()
                        .filter(|value| {
                            value.manifest == unit.manifest && value.source_root == unit.source_root
                        })
                        .map(|value| (value.name.clone(), value.value.clone()))
                        .collect(),
                },
            )
    }
}

fn resolve_source_root_path(
    workspace_root: &Path,
    manifest_path: &Path,
    source_root: &SourceRootBinding,
) -> Option<PathBuf> {
    let package_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    match source_root {
        SourceRootBinding::PackagePath(path) => Some(package_root.join(path)),
        SourceRootBinding::AbsolutePath(path) => Some(PathBuf::from(path)),
        SourceRootBinding::BuildOutput { path, .. } => {
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
        normalize_platform_path(path.to_path_buf())
    } else {
        normalize_platform_path(workspace_root.join(path))
    }
}

fn normalize_platform_path(path: PathBuf) -> PathBuf {
    let path = strip_windows_verbatim_prefix(path);
    strip_macos_private_var_prefix(path)
}

#[cfg(windows)]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix("\\\\?\\UNC\\") {
        return PathBuf::from(format!("\\\\{stripped}"));
    }
    if let Some(stripped) = raw.strip_prefix("\\\\?\\") {
        return PathBuf::from(stripped);
    }
    path
}

#[cfg(not(windows))]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    path
}

#[cfg(target_os = "macos")]
fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix("/private/var/") {
        return PathBuf::from(format!("/var/{stripped}"));
    }
    if raw == "/private/var" {
        return PathBuf::from("/var");
    }
    path
}

#[cfg(not(target_os = "macos"))]
fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
    path
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

fn digest_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).map_err(|err| Error::from_io(path, err))?;
    Ok(format!("fnv1a64:{:016x}", fnv1a64(&bytes)))
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

    let sibling_module_root = root.parent()?;
    if file.starts_with(sibling_module_root) {
        return Some(sibling_module_root.components().count());
    }

    None
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    let mut hash = OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}
