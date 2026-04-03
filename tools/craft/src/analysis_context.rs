use crate::build_plan::{BuildPlan, SourceRootBinding};
use crate::elaborate::{ElaborationPlan, FeatureSelection};
use crate::error::{Error, Result};
use crate::execute;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const ANALYSIS_CONTEXT_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisContext {
    version: u32,
    manifest: String,
    manifest_digest: String,
    profile: String,
    default_features: bool,
    features: Vec<String>,
    workspace_script: Option<String>,
    workspace_script_digest: Option<String>,
    packages: Vec<AnalysisContextPackage>,
    units: Vec<AnalysisContextUnit>,
    unit_aliases: Vec<AnalysisContextUnitAlias>,
    unit_values: Vec<AnalysisContextUnitValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AnalysisContextPackage {
    manifest: String,
    manifest_digest: String,
    craft_script: Option<String>,
    craft_script_digest: Option<String>,
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| Error::from_io(parent, err))?;
    }
    fs::write(&path, context.render()).map_err(|err| Error::from_io(&path, err))?;
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
    let context = AnalysisContext::parse(&source, &path)?;
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
        let workspace_script = elaboration
            .workspace_script
            .as_ref()
            .map(|script| script.relative_path.clone());
        let workspace_script_digest = elaboration
            .workspace_script
            .as_ref()
            .map(|script| script.digest.clone());

        let mut packages = elaboration
            .resolved_graph
            .packages
            .iter()
            .map(|package| {
                let script = elaboration
                    .packages
                    .iter()
                    .find(|entry| entry.package_id == package.id)
                    .and_then(|entry| entry.script.as_ref());
                Ok(AnalysisContextPackage {
                    manifest: relative_display(workspace_root, &package.manifest_path),
                    manifest_digest: digest_file(&package.manifest_path)?,
                    craft_script: script.map(|script| script.relative_path.clone()),
                    craft_script_digest: script.map(|script| script.digest.clone()),
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
            workspace_script,
            workspace_script_digest,
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

    fn parse(source: &str, path: &Path) -> Result<Self> {
        let mut context = Self {
            version: 0,
            manifest: String::new(),
            manifest_digest: String::new(),
            profile: String::new(),
            default_features: true,
            features: Vec::new(),
            workspace_script: None,
            workspace_script_digest: None,
            packages: Vec::new(),
            units: Vec::new(),
            unit_aliases: Vec::new(),
            unit_values: Vec::new(),
        };
        let mut section = Section::Root;

        for line in logical_lines(source).map_err(|message| Error::AnalysisContextParse {
            path: path.to_path_buf(),
            message,
        })? {
            if line.starts_with("[[") {
                section = match line.as_str() {
                    "[[package]]" => {
                        context.packages.push(AnalysisContextPackage {
                            manifest: String::new(),
                            manifest_digest: String::new(),
                            craft_script: None,
                            craft_script_digest: None,
                        });
                        Section::Package(context.packages.len() - 1)
                    }
                    "[[unit]]" => {
                        context.units.push(AnalysisContextUnit {
                            manifest: String::new(),
                            source_root: String::new(),
                            target_kind: String::new(),
                        });
                        Section::Unit(context.units.len() - 1)
                    }
                    "[[unit-alias]]" => {
                        context.unit_aliases.push(AnalysisContextUnitAlias {
                            manifest: String::new(),
                            source_root: String::new(),
                            source_path: String::new(),
                            generated_path: String::new(),
                        });
                        Section::UnitAlias(context.unit_aliases.len() - 1)
                    }
                    "[[unit-value]]" => {
                        context.unit_values.push(AnalysisContextUnitValue {
                            manifest: String::new(),
                            source_root: String::new(),
                            name: String::new(),
                            value: String::new(),
                        });
                        Section::UnitValue(context.unit_values.len() - 1)
                    }
                    _ => {
                        return Err(Error::AnalysisContextParse {
                            path: path.to_path_buf(),
                            message: format!("unsupported array table `{line}`"),
                        });
                    }
                };
                continue;
            }

            let (key, raw_value) =
                split_key_value(&line).map_err(|message| Error::AnalysisContextParse {
                    path: path.to_path_buf(),
                    message,
                })?;
            assign_key_value(&mut context, section, key, raw_value).map_err(|message| {
                Error::AnalysisContextParse {
                    path: path.to_path_buf(),
                    message,
                }
            })?;
        }

        context.validate(path)?;
        Ok(context)
    }

    fn validate(&self, path: &Path) -> Result<()> {
        if self.version != ANALYSIS_CONTEXT_VERSION {
            return Err(Error::AnalysisContextValidation {
                path: path.to_path_buf(),
                message: format!("unsupported analysis context version `{}`", self.version),
            });
        }

        validate_non_empty(path, "manifest", &self.manifest)?;
        validate_digest(path, "manifest-digest", &self.manifest_digest)?;
        validate_non_empty(path, "profile", &self.profile)?;
        validate_optional_path_and_digest(
            path,
            "workspace-script",
            self.workspace_script.as_deref(),
            self.workspace_script_digest.as_deref(),
        )?;

        let mut package_manifests = BTreeSet::new();
        for package in &self.packages {
            validate_non_empty(path, "[[package]].manifest", &package.manifest)?;
            validate_digest(
                path,
                "[[package]].manifest-digest",
                &package.manifest_digest,
            )?;
            validate_optional_path_and_digest(
                path,
                "[[package]].craft-script",
                package.craft_script.as_deref(),
                package.craft_script_digest.as_deref(),
            )?;
            if !package_manifests.insert(package.manifest.as_str()) {
                return Err(Error::AnalysisContextValidation {
                    path: path.to_path_buf(),
                    message: format!("duplicate [[package]] manifest `{}`", package.manifest),
                });
            }
        }

        let mut unit_keys = BTreeSet::new();
        for unit in &self.units {
            validate_non_empty(path, "[[unit]].manifest", &unit.manifest)?;
            validate_non_empty(path, "[[unit]].source-root", &unit.source_root)?;
            validate_target_kind(path, "[[unit]].target-kind", &unit.target_kind)?;
            if !package_manifests.contains(unit.manifest.as_str()) {
                return Err(Error::AnalysisContextValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "[[unit]] references unknown package manifest `{}`",
                        unit.manifest
                    ),
                });
            }
            if !unit_keys.insert((unit.manifest.as_str(), unit.source_root.as_str())) {
                return Err(Error::AnalysisContextValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "duplicate [[unit]] `{}` -> `{}`",
                        unit.manifest, unit.source_root
                    ),
                });
            }
        }

        let mut alias_keys = BTreeSet::new();
        for alias in &self.unit_aliases {
            validate_non_empty(path, "[[unit-alias]].manifest", &alias.manifest)?;
            validate_non_empty(path, "[[unit-alias]].source-root", &alias.source_root)?;
            validate_non_empty(path, "[[unit-alias]].source-path", &alias.source_path)?;
            validate_non_empty(path, "[[unit-alias]].generated-path", &alias.generated_path)?;
            if !unit_keys.contains(&(alias.manifest.as_str(), alias.source_root.as_str())) {
                return Err(Error::AnalysisContextValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "[[unit-alias]] references unknown unit `{}` -> `{}`",
                        alias.manifest, alias.source_root
                    ),
                });
            }
            if !alias_keys.insert((
                alias.manifest.as_str(),
                alias.source_root.as_str(),
                alias.source_path.as_str(),
            )) {
                return Err(Error::AnalysisContextValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "duplicate [[unit-alias]] `{}` -> `{}` -> `{}`",
                        alias.manifest, alias.source_root, alias.source_path
                    ),
                });
            }
        }

        let mut features = BTreeSet::new();
        for feature in &self.features {
            validate_non_empty(path, "features[]", feature)?;
            if !features.insert(feature.as_str()) {
                return Err(Error::AnalysisContextValidation {
                    path: path.to_path_buf(),
                    message: format!("duplicate feature `{feature}`"),
                });
            }
        }

        let mut value_keys = BTreeSet::new();
        for value in &self.unit_values {
            validate_non_empty(path, "[[unit-value]].manifest", &value.manifest)?;
            validate_non_empty(path, "[[unit-value]].source-root", &value.source_root)?;
            validate_non_empty(path, "[[unit-value]].name", &value.name)?;
            if !unit_keys.contains(&(value.manifest.as_str(), value.source_root.as_str())) {
                return Err(Error::AnalysisContextValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "[[unit-value]] references unknown unit `{}` -> `{}`",
                        value.manifest, value.source_root
                    ),
                });
            }
            if !value_keys.insert((
                value.manifest.as_str(),
                value.source_root.as_str(),
                value.name.as_str(),
            )) {
                return Err(Error::AnalysisContextValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "duplicate [[unit-value]] `{}` -> `{}` -> `{}`",
                        value.manifest, value.source_root, value.name
                    ),
                });
            }
        }

        Ok(())
    }

    fn is_current(&self, manifest_path: &Path, workspace_root: &Path) -> Result<bool> {
        if self.manifest != relative_display(workspace_root, manifest_path) {
            return Ok(false);
        }
        if self.manifest_digest != digest_file(manifest_path)? {
            return Ok(false);
        }

        if !path_and_digest_current(
            workspace_root,
            self.workspace_script.as_deref(),
            self.workspace_script_digest.as_deref(),
        )? {
            return Ok(false);
        }

        for package in &self.packages {
            let manifest_path = resolve_context_path(workspace_root, &package.manifest);
            if !manifest_path.is_file() {
                return Ok(false);
            }
            if package.manifest_digest != digest_file(&manifest_path)? {
                return Ok(false);
            }
            if !path_and_digest_current(
                workspace_root,
                package.craft_script.as_deref(),
                package.craft_script_digest.as_deref(),
            )? {
                return Ok(false);
            }
        }

        Ok(true)
    }

    fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("# This file is generated by craft.\n");
        out.push_str("version = ");
        out.push_str(&self.version.to_string());
        out.push('\n');
        push_string_line(&mut out, "manifest", &self.manifest);
        push_string_line(&mut out, "manifest-digest", &self.manifest_digest);
        push_string_line(&mut out, "profile", &self.profile);
        push_bool_line(&mut out, "default-features", self.default_features);
        push_string_array_line(&mut out, "features", &self.features);
        if let Some(path) = &self.workspace_script {
            push_string_line(&mut out, "workspace-script", path);
        }
        if let Some(digest) = &self.workspace_script_digest {
            push_string_line(&mut out, "workspace-script-digest", digest);
        }

        for package in &self.packages {
            out.push('\n');
            out.push_str("[[package]]\n");
            push_string_line(&mut out, "manifest", &package.manifest);
            push_string_line(&mut out, "manifest-digest", &package.manifest_digest);
            if let Some(path) = &package.craft_script {
                push_string_line(&mut out, "craft-script", path);
            }
            if let Some(digest) = &package.craft_script_digest {
                push_string_line(&mut out, "craft-script-digest", digest);
            }
        }

        for unit in &self.units {
            out.push('\n');
            out.push_str("[[unit]]\n");
            push_string_line(&mut out, "manifest", &unit.manifest);
            push_string_line(&mut out, "source-root", &unit.source_root);
            push_string_line(&mut out, "target-kind", &unit.target_kind);
        }

        for alias in &self.unit_aliases {
            out.push('\n');
            out.push_str("[[unit-alias]]\n");
            push_string_line(&mut out, "manifest", &alias.manifest);
            push_string_line(&mut out, "source-root", &alias.source_root);
            push_string_line(&mut out, "source-path", &alias.source_path);
            push_string_line(&mut out, "generated-path", &alias.generated_path);
        }

        for value in &self.unit_values {
            out.push('\n');
            out.push_str("[[unit-value]]\n");
            push_string_line(&mut out, "manifest", &value.manifest);
            push_string_line(&mut out, "source-root", &value.source_root);
            push_string_line(&mut out, "name", &value.name);
            push_string_line(&mut out, "value", &value.value);
        }

        out
    }
}

fn assign_key_value(
    context: &mut AnalysisContext,
    section: Section,
    key: &str,
    raw_value: &str,
) -> std::result::Result<(), String> {
    match section {
        Section::Root => match key {
            "version" => context.version = parse_u32(raw_value)?,
            "manifest" => context.manifest = parse_string(raw_value)?,
            "manifest-digest" => context.manifest_digest = parse_string(raw_value)?,
            "profile" => context.profile = parse_string(raw_value)?,
            "default-features" => context.default_features = parse_bool(raw_value)?,
            "features" => context.features = parse_string_array(raw_value)?,
            "workspace-script" => context.workspace_script = Some(parse_string(raw_value)?),
            "workspace-script-digest" => {
                context.workspace_script_digest = Some(parse_string(raw_value)?)
            }
            _ => return Err(format!("unsupported root key `{key}`")),
        },
        Section::Package(index) => {
            let package = &mut context.packages[index];
            match key {
                "manifest" => package.manifest = parse_string(raw_value)?,
                "manifest-digest" => package.manifest_digest = parse_string(raw_value)?,
                "craft-script" => package.craft_script = Some(parse_string(raw_value)?),
                "craft-script-digest" => {
                    package.craft_script_digest = Some(parse_string(raw_value)?)
                }
                _ => return Err(format!("unsupported [[package]] key `{key}`")),
            }
        }
        Section::Unit(index) => {
            let unit = &mut context.units[index];
            match key {
                "manifest" => unit.manifest = parse_string(raw_value)?,
                "source-root" => unit.source_root = parse_string(raw_value)?,
                "target-kind" => unit.target_kind = parse_string(raw_value)?,
                _ => return Err(format!("unsupported [[unit]] key `{key}`")),
            }
        }
        Section::UnitAlias(index) => {
            let alias = &mut context.unit_aliases[index];
            match key {
                "manifest" => alias.manifest = parse_string(raw_value)?,
                "source-root" => alias.source_root = parse_string(raw_value)?,
                "source-path" => alias.source_path = parse_string(raw_value)?,
                "generated-path" => alias.generated_path = parse_string(raw_value)?,
                _ => return Err(format!("unsupported [[unit-alias]] key `{key}`")),
            }
        }
        Section::UnitValue(index) => {
            let value = &mut context.unit_values[index];
            match key {
                "manifest" => value.manifest = parse_string(raw_value)?,
                "source-root" => value.source_root = parse_string(raw_value)?,
                "name" => value.name = parse_string(raw_value)?,
                "value" => value.value = parse_string(raw_value)?,
                _ => return Err(format!("unsupported [[unit-value]] key `{key}`")),
            }
        }
    }
    Ok(())
}

fn compile_time_values(
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

fn plan_value_string(value: &crate::plan::PlanValue) -> String {
    match value {
        crate::plan::PlanValue::Bool(value) => value.to_string(),
        crate::plan::PlanValue::String(value) => value.clone(),
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

fn path_and_digest_current(
    workspace_root: &Path,
    path: Option<&str>,
    digest: Option<&str>,
) -> Result<bool> {
    match (path, digest) {
        (None, None) => Ok(true),
        (Some(path), Some(digest)) => {
            let resolved = resolve_context_path(workspace_root, path);
            if !resolved.is_file() {
                return Ok(false);
            }
            Ok(digest_file(&resolved)? == digest)
        }
        _ => Ok(false),
    }
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

fn logical_lines(source: &str) -> std::result::Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for raw_line in source.lines() {
        let stripped = strip_comment(raw_line)?;
        let trimmed = stripped.trim();
        if !trimmed.is_empty() {
            lines.push(trimmed.to_string());
        }
    }
    Ok(lines)
}

fn strip_comment(line: &str) -> std::result::Result<String, String> {
    let mut out = String::new();
    let mut in_string = false;
    let mut escape = false;

    for ch in line.chars() {
        if escape {
            out.push(ch);
            escape = false;
            continue;
        }

        match ch {
            '\\' if in_string => {
                out.push(ch);
                escape = true;
            }
            '"' => {
                out.push(ch);
                in_string = !in_string;
            }
            '#' if !in_string => break,
            _ => out.push(ch),
        }
    }

    if in_string {
        return Err("unterminated string literal".to_string());
    }

    Ok(out)
}

fn split_key_value(line: &str) -> std::result::Result<(&str, &str), String> {
    let mut in_string = false;
    let mut escape = false;

    for (index, ch) in line.char_indices() {
        if escape {
            escape = false;
            continue;
        }

        if in_string {
            match ch {
                '\\' => escape = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '=' => {
                let key = line[..index].trim();
                let value = line[index + 1..].trim();
                if key.is_empty() {
                    return Err("missing key before `=`".to_string());
                }
                if value.is_empty() {
                    return Err(format!("missing value for key `{key}`"));
                }
                return Ok((key, value));
            }
            _ => {}
        }
    }

    Err(format!("expected `key = value`, found `{line}`"))
}

fn parse_u32(raw: &str) -> std::result::Result<u32, String> {
    raw.trim()
        .parse::<u32>()
        .map_err(|_| format!("expected unsigned integer, found `{}`", raw.trim()))
}

fn parse_string(raw: &str) -> std::result::Result<String, String> {
    let raw = raw.trim();
    if !raw.starts_with('"') || !raw.ends_with('"') || raw.len() < 2 {
        return Err(format!("expected string literal, found `{raw}`"));
    }

    let inner = &raw[1..raw.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }

        let Some(escaped) = chars.next() else {
            return Err("unterminated string escape".to_string());
        };
        match escaped {
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            other => return Err(format!("unsupported escape sequence `\\{other}`")),
        }
    }

    Ok(out)
}

fn parse_bool(raw: &str) -> std::result::Result<bool, String> {
    match raw.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("expected boolean, found `{other}`")),
    }
}

fn parse_string_array(raw: &str) -> std::result::Result<Vec<String>, String> {
    let inner = strip_wrapping(raw, '[', ']')?;
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }

    split_top_level(inner, ',')
        .into_iter()
        .map(parse_string)
        .collect()
}

fn strip_wrapping(raw: &str, open: char, close: char) -> std::result::Result<&str, String> {
    let trimmed = raw.trim();
    if !trimmed.starts_with(open) || !trimmed.ends_with(close) || trimmed.len() < 2 {
        return Err(format!("expected `{open}...{close}`, found `{trimmed}`"));
    }
    Ok(&trimmed[1..trimmed.len() - 1])
}

fn split_top_level(input: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_string = false;
    let mut escape = false;
    let mut brace_depth = 0usize;
    let mut bracket_depth = 0usize;

    for (index, ch) in input.char_indices() {
        if escape {
            escape = false;
            continue;
        }

        if in_string {
            match ch {
                '\\' => escape = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            _ if ch == separator && brace_depth == 0 && bracket_depth == 0 => {
                let piece = input[start..index].trim();
                if !piece.is_empty() {
                    parts.push(piece);
                }
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }

    let tail = input[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }

    parts
}

fn validate_non_empty(path: &Path, field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::AnalysisContextValidation {
            path: path.to_path_buf(),
            message: format!("{field} must not be empty"),
        });
    }
    Ok(())
}

fn validate_optional_path_and_digest(
    path: &Path,
    field: &str,
    value: Option<&str>,
    digest: Option<&str>,
) -> Result<()> {
    match (value, digest) {
        (None, None) => Ok(()),
        (Some(value), Some(digest)) => {
            validate_non_empty(path, field, value)?;
            validate_digest(path, &format!("{field}-digest"), digest)
        }
        _ => Err(Error::AnalysisContextValidation {
            path: path.to_path_buf(),
            message: format!(
                "{field} and {field}-digest must either both be present or both be absent"
            ),
        }),
    }
}

fn validate_digest(path: &Path, field: &str, value: &str) -> Result<()> {
    validate_non_empty(path, field, value)?;
    if !value.starts_with("fnv1a64:") || value.len() != "fnv1a64:".len() + 16 {
        return Err(Error::AnalysisContextValidation {
            path: path.to_path_buf(),
            message: format!("{field} must be an `fnv1a64:` digest"),
        });
    }
    Ok(())
}

fn validate_target_kind(path: &Path, field: &str, value: &str) -> Result<()> {
    match value {
        "lib" | "bin" | "test" | "example" => Ok(()),
        _ => Err(Error::AnalysisContextValidation {
            path: path.to_path_buf(),
            message: format!("{field} has unsupported unit target kind `{value}`"),
        }),
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

fn push_bool_line(out: &mut String, key: &str, value: bool) {
    out.push_str(key);
    out.push_str(" = ");
    out.push_str(if value { "true" } else { "false" });
    out.push('\n');
}

fn push_string_line(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push_str(" = \"");
    out.push_str(&escape_string(value));
    out.push_str("\"\n");
}

fn push_string_array_line(out: &mut String, key: &str, values: &[String]) {
    out.push_str(key);
    out.push_str(" = [");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push('"');
        out.push_str(&escape_string(value));
        out.push('"');
    }
    out.push_str("]\n");
}

fn escape_string(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{load_current_analysis_context, sync_analysis_context};
    use crate::build_plan;
    use crate::elaborate::{FeatureSelection, plan};
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

    fn with_env_var<T>(name: &str, value: &str, f: impl FnOnce() -> T) -> T {
        let previous = std::env::var_os(name);
        unsafe {
            std::env::set_var(name, value);
        }
        let result = f();
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
    fn syncs_and_loads_current_analysis_context() {
        let root = temp_dir("craft-analysis-context");
        fs::create_dir_all(root.join("src")).unwrap();
        let env_name = format!(
            "KERN_ANALYSIS_CONTEXT_{}",
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
        let mut feature_selection = FeatureSelection::default();
        feature_selection
            .explicit
            .insert("experimental".to_string());

        let elaboration = with_env_var(&env_name, "1", || {
            plan(
                &manifest_path,
                &manifest,
                &workspace_members,
                false,
                crate::script::ScriptCommand::Build,
                &feature_selection,
            )
            .unwrap()
        });
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        sync_analysis_context(
            &manifest_path,
            &elaboration,
            &build_plan,
            &feature_selection,
        )
        .unwrap();

        let context = load_current_analysis_context(&manifest_path, &root)
            .unwrap()
            .unwrap();
        let values = context
            .compile_time_values_for(&manifest_path, &root.join("src/main.rn"), &root)
            .unwrap();

        assert_eq!(
            values.get("enable_telemetry").map(String::as_str),
            Some("true")
        );
        assert_eq!(values.get("is_dev_env").map(String::as_str), Some("true"));
        assert_eq!(
            values.get("GREETING_MSG").map(String::as_str),
            Some("Hello from craft")
        );
    }

    #[test]
    fn stale_manifest_digest_invalidates_analysis_context() {
        let root = temp_dir("craft-analysis-context-stale");
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
root = \"src/main.rn\"
",
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
        let feature_selection = FeatureSelection::default();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &workspace_members,
            false,
            crate::script::ScriptCommand::Build,
            &feature_selection,
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        sync_analysis_context(
            &manifest_path,
            &elaboration,
            &build_plan,
            &feature_selection,
        )
        .unwrap();

        fs::write(
            root.join("Craft.toml"),
            "\
[package]
name = \"app\"
version = \"0.1.1\"
kern = \"0.7\"

[[bin]]
name = \"app\"
root = \"src/main.rn\"
",
        )
        .unwrap();

        assert!(
            load_current_analysis_context(&manifest_path, &root)
                .unwrap()
                .is_none()
        );
    }
}
