use crate::elaborate::ElaborationPlan;
use crate::error::{Error, Result};
use crate::graph::{DependencyKind, PackageId, SourceId};
use crate::manifest::Manifest;
use crate::plan::TargetKind;
use crate::resolver::{ExternalPackageId, ResolvedDependencyTarget};
use crate::source;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lockfile {
    pub version: u32,
    pub manifest: String,
    pub manifest_digest: String,
    pub workspace_script: Option<String>,
    pub workspace_script_digest: Option<String>,
    pub workspace_env: Vec<LockedEnvInput>,
    pub packages: Vec<LockedPackage>,
    pub package_targets: Vec<LockedPackageTarget>,
    pub external_packages: Vec<LockedExternalPackage>,
    pub package_env: Vec<LockedPackageEnvInput>,
    pub dependencies: Vec<LockedDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackage {
    pub id: String,
    pub name: String,
    pub version: String,
    pub source_kind: String,
    pub source_value: Option<String>,
    pub manifest: String,
    pub manifest_digest: String,
    pub craft_script: Option<String>,
    pub craft_script_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedDependency {
    pub from: String,
    pub kind: String,
    pub name: String,
    pub package: String,
    pub target_kind: String,
    pub target_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedExternalPackage {
    pub id: String,
    pub name: String,
    pub source_kind: String,
    pub source_value: Option<String>,
    pub version: Option<String>,
    pub source_locator: Option<String>,
    pub source_selector: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackageTarget {
    pub package_id: String,
    pub kind: String,
    pub name: Option<String>,
    pub root: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedEnvInput {
    pub name: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackageEnvInput {
    pub package_id: String,
    pub name: String,
    pub value: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockStatus {
    Missing,
    Current,
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockWriteResult {
    Created,
    Updated,
    Unchanged,
}

#[derive(Clone, Copy, Debug)]
enum Section {
    Root,
    Package(usize),
    PackageTarget(usize),
    ExternalPackage(usize),
    WorkspaceEnv(usize),
    PackageEnv(usize),
    Dependency(usize),
}

pub fn sync_lockfile(
    manifest_path: &Path,
    elaboration: &ElaborationPlan,
) -> Result<(PathBuf, LockWriteResult)> {
    let lock_path = elaboration.resolved_graph.workspace_root.join("Craft.lock");
    let expected = Lockfile::from_elaboration(manifest_path, elaboration)?;

    if lock_path.is_file() {
        let actual = Lockfile::load(&lock_path)?;
        if actual == expected {
            return Ok((lock_path, LockWriteResult::Unchanged));
        }
    }

    let result = if lock_path.is_file() {
        LockWriteResult::Updated
    } else {
        LockWriteResult::Created
    };

    fs::write(&lock_path, expected.render()).map_err(|err| Error::from_io(&lock_path, err))?;
    Ok((lock_path, result))
}

pub fn lock_status(manifest_path: &Path, elaboration: &ElaborationPlan) -> Result<LockStatus> {
    let lock_path = elaboration.resolved_graph.workspace_root.join("Craft.lock");
    if !lock_path.is_file() {
        return Ok(LockStatus::Missing);
    }

    let actual = Lockfile::load(&lock_path)?;
    let expected = Lockfile::from_elaboration(manifest_path, elaboration)?;
    if actual == expected {
        Ok(LockStatus::Current)
    } else {
        Ok(LockStatus::Stale)
    }
}

impl Lockfile {
    pub fn load(path: &Path) -> Result<Self> {
        let source = fs::read_to_string(path).map_err(|err| Error::from_io(path, err))?;
        let lockfile = Self::parse(&source, path)?;
        lockfile.validate(path)?;
        Ok(lockfile)
    }

    pub fn from_elaboration(manifest_path: &Path, elaboration: &ElaborationPlan) -> Result<Self> {
        let resolved = &elaboration.resolved_graph;
        let root = &resolved.workspace_root;
        let manifest = relative_display(root, manifest_path);
        let manifest_digest = digest_file(manifest_path)?;
        let root_manifest = Manifest::load(manifest_path)?;
        root_manifest.validate(manifest_path)?;

        let mut packages = Vec::new();
        let mut package_targets = Vec::new();
        let mut external_packages = Vec::new();
        let mut package_env = Vec::new();
        let mut dependencies = Vec::new();

        for package in &resolved.packages {
            let package_id = package_lock_id(&package.id);
            let script = elaboration
                .packages
                .iter()
                .find(|entry| entry.package_id == package.id)
                .and_then(|entry| entry.script.as_ref());
            packages.push(LockedPackage {
                id: package_id.clone(),
                name: package.id.name.clone(),
                version: package.id.version.clone(),
                source_kind: source_kind(&package.id.source).to_string(),
                source_value: source_value(&package.id.source),
                manifest: relative_display(root, &package.manifest_path),
                manifest_digest: digest_file(&package.manifest_path)?,
                craft_script: script.map(|script| script.relative_path.clone()),
                craft_script_digest: script.map(|script| script.digest.clone()),
            });
            let package_plan = &elaboration
                .packages
                .iter()
                .find(|entry| entry.package_id == package.id)
                .expect("elaboration must contain package plan")
                .plan;
            for target in &package_plan.targets {
                package_targets.push(LockedPackageTarget {
                    package_id: package_id.clone(),
                    kind: target_kind(target.kind).to_string(),
                    name: target.name.clone(),
                    root: target.root.clone(),
                });
            }
            if let Some(script) = script {
                for input in &script.env_inputs {
                    package_env.push(LockedPackageEnvInput {
                        package_id: package_id.clone(),
                        name: input.name.clone(),
                        value: input.value.clone(),
                    });
                }
            }

            for dep in &package.dependencies {
                let (target_kind, target_id) = match &dep.target {
                    ResolvedDependencyTarget::Local(target) => {
                        ("local".to_string(), package_lock_id(target))
                    }
                    ResolvedDependencyTarget::External(target) => {
                        ("external".to_string(), external_package_lock_id(target))
                    }
                };

                dependencies.push(LockedDependency {
                    from: package_id.clone(),
                    kind: dependency_kind(dep.kind).to_string(),
                    name: dep.dependency_name.clone(),
                    package: dep.package_name.clone(),
                    target_kind,
                    target_id,
                });
            }
        }

        for package in &resolved.external_packages {
            external_packages.push(LockedExternalPackage {
                id: external_package_lock_id(&package.id),
                name: package.id.package_name.clone(),
                source_kind: source_kind(&package.id.source).to_string(),
                source_value: source_value(&package.id.source),
                version: package.id.version.clone(),
                source_locator: source_locator(&root_manifest, &package.id.source),
                source_selector: source_selector(&root_manifest, &package.id.source),
            });
        }

        Ok(Self {
            version: 1,
            manifest,
            manifest_digest,
            workspace_script: elaboration
                .workspace_script
                .as_ref()
                .map(|script| script.relative_path.clone()),
            workspace_script_digest: elaboration
                .workspace_script
                .as_ref()
                .map(|script| script.digest.clone()),
            workspace_env: elaboration
                .workspace_script
                .as_ref()
                .map(|script| {
                    script
                        .env_inputs
                        .iter()
                        .map(|input| LockedEnvInput {
                            name: input.name.clone(),
                            value: input.value.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            packages,
            package_targets,
            external_packages,
            package_env,
            dependencies,
        })
    }

    fn parse(source: &str, path: &Path) -> Result<Self> {
        let mut lockfile = Self {
            version: 0,
            manifest: String::new(),
            manifest_digest: String::new(),
            workspace_script: None,
            workspace_script_digest: None,
            workspace_env: Vec::new(),
            packages: Vec::new(),
            package_targets: Vec::new(),
            external_packages: Vec::new(),
            package_env: Vec::new(),
            dependencies: Vec::new(),
        };
        let mut section = Section::Root;

        for line in logical_lines(source).map_err(|message| Error::LockfileParse {
            path: path.to_path_buf(),
            message,
        })? {
            if line.starts_with("[[") {
                section = match line.as_str() {
                    "[[package]]" => {
                        lockfile.packages.push(LockedPackage {
                            id: String::new(),
                            name: String::new(),
                            version: String::new(),
                            source_kind: String::new(),
                            source_value: None,
                            manifest: String::new(),
                            manifest_digest: String::new(),
                            craft_script: None,
                            craft_script_digest: None,
                        });
                        Section::Package(lockfile.packages.len() - 1)
                    }
                    "[[external-package]]" => {
                        lockfile.external_packages.push(LockedExternalPackage {
                            id: String::new(),
                            name: String::new(),
                            source_kind: String::new(),
                            source_value: None,
                            version: None,
                            source_locator: None,
                            source_selector: None,
                        });
                        Section::ExternalPackage(lockfile.external_packages.len() - 1)
                    }
                    "[[package-target]]" => {
                        lockfile.package_targets.push(LockedPackageTarget {
                            package_id: String::new(),
                            kind: String::new(),
                            name: None,
                            root: String::new(),
                        });
                        Section::PackageTarget(lockfile.package_targets.len() - 1)
                    }
                    "[[workspace-env]]" => {
                        lockfile.workspace_env.push(LockedEnvInput {
                            name: String::new(),
                            value: None,
                        });
                        Section::WorkspaceEnv(lockfile.workspace_env.len() - 1)
                    }
                    "[[package-env]]" => {
                        lockfile.package_env.push(LockedPackageEnvInput {
                            package_id: String::new(),
                            name: String::new(),
                            value: None,
                        });
                        Section::PackageEnv(lockfile.package_env.len() - 1)
                    }
                    "[[dependency]]" => {
                        lockfile.dependencies.push(LockedDependency {
                            from: String::new(),
                            kind: String::new(),
                            name: String::new(),
                            package: String::new(),
                            target_kind: String::new(),
                            target_id: String::new(),
                        });
                        Section::Dependency(lockfile.dependencies.len() - 1)
                    }
                    _ => {
                        return Err(Error::LockfileParse {
                            path: path.to_path_buf(),
                            message: format!("unsupported array table `{line}`"),
                        });
                    }
                };
                continue;
            }

            let (key, raw_value) =
                split_key_value(&line).map_err(|message| Error::LockfileParse {
                    path: path.to_path_buf(),
                    message,
                })?;
            assign_key_value(&mut lockfile, section, key, raw_value).map_err(|message| {
                Error::LockfileParse {
                    path: path.to_path_buf(),
                    message,
                }
            })?;
        }

        Ok(lockfile)
    }

    pub fn validate(&self, path: &Path) -> Result<()> {
        if self.version != 1 {
            return Err(Error::LockfileValidation {
                path: path.to_path_buf(),
                message: format!("unsupported lockfile version `{}`", self.version),
            });
        }

        validate_non_empty(path, "manifest", &self.manifest)?;
        validate_digest(path, "manifest-digest", &self.manifest_digest)?;
        validate_optional_path_and_digest(
            path,
            "workspace-script",
            self.workspace_script.as_deref(),
            self.workspace_script_digest.as_deref(),
        )?;
        let mut workspace_env_names = BTreeSet::new();
        if !self.workspace_env.is_empty() && self.workspace_script.is_none() {
            return Err(Error::LockfileValidation {
                path: path.to_path_buf(),
                message: "[[workspace-env]] entries require `workspace-script`".to_string(),
            });
        }
        for input in &self.workspace_env {
            validate_env_input_name(path, "[[workspace-env]].name", &input.name)?;
            if !workspace_env_names.insert(input.name.as_str()) {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!("duplicate workspace env input `{}`", input.name),
                });
            }
        }

        let mut package_ids = BTreeSet::new();
        for package in &self.packages {
            validate_non_empty(path, "[[package]].id", &package.id)?;
            validate_non_empty(path, "[[package]].name", &package.name)?;
            validate_non_empty(path, "[[package]].version", &package.version)?;
            validate_source_kind(path, "[[package]].source", &package.source_kind)?;
            if matches!(package.source_kind.as_str(), "workspace-member" | "path")
                && package.source_value.is_none()
            {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "[[package]] `{}` requires `source-value` for source `{}`",
                        package.id, package.source_kind
                    ),
                });
            }
            if !package_ids.insert(package.id.as_str()) {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!("duplicate package id `{}`", package.id),
                });
            }
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
        }

        let mut package_target_keys = BTreeSet::new();
        for target in &self.package_targets {
            validate_non_empty(path, "[[package-target]].package", &target.package_id)?;
            if !package_ids.contains(target.package_id.as_str()) {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "[[package-target]] references unknown package id `{}`",
                        target.package_id
                    ),
                });
            }
            validate_target_kind_name(path, "[[package-target]].kind", &target.kind)?;
            match target.kind.as_str() {
                "lib" => {
                    if target.name.is_some() {
                        return Err(Error::LockfileValidation {
                            path: path.to_path_buf(),
                            message: "[[package-target]] kind `lib` must not set `name`"
                                .to_string(),
                        });
                    }
                }
                _ => {
                    validate_non_empty(
                        path,
                        "[[package-target]].name",
                        target.name.as_deref().unwrap_or(""),
                    )?;
                }
            }
            validate_non_empty(path, "[[package-target]].root", &target.root)?;
            if !package_target_keys.insert((
                target.package_id.as_str(),
                target.kind.as_str(),
                target.name.as_deref().unwrap_or(""),
            )) {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "duplicate package target `{}:{}:{}`",
                        target.package_id,
                        target.kind,
                        target.name.as_deref().unwrap_or("<lib>")
                    ),
                });
            }
        }

        let mut package_env_keys = BTreeSet::new();
        for input in &self.package_env {
            validate_non_empty(path, "[[package-env]].package", &input.package_id)?;
            if !package_ids.contains(input.package_id.as_str()) {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "[[package-env]] references unknown package id `{}`",
                        input.package_id
                    ),
                });
            }
            let has_script = self
                .packages
                .iter()
                .find(|package| package.id == input.package_id)
                .and_then(|package| package.craft_script.as_ref())
                .is_some();
            if !has_script {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "[[package-env]] references package `{}` without `craft-script`",
                        input.package_id
                    ),
                });
            }
            validate_env_input_name(path, "[[package-env]].name", &input.name)?;
            if !package_env_keys.insert((input.package_id.as_str(), input.name.as_str())) {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "duplicate package env input `{}:{}`",
                        input.package_id, input.name
                    ),
                });
            }
        }

        let mut external_ids = BTreeSet::new();
        for package in &self.external_packages {
            validate_non_empty(path, "[[external-package]].id", &package.id)?;
            validate_non_empty(path, "[[external-package]].name", &package.name)?;
            validate_source_kind(path, "[[external-package]].source", &package.source_kind)?;
            if matches!(package.source_kind.as_str(), "path" | "workspace-member")
                && package.source_value.is_none()
            {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "[[external-package]] `{}` requires `source-value` for source `{}`",
                        package.id, package.source_kind
                    ),
                });
            }
            if !external_ids.insert(package.id.as_str()) {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!("duplicate external package id `{}`", package.id),
                });
            }
            if let Some(locator) = &package.source_locator {
                validate_non_empty(path, "[[external-package]].source-locator", locator)?;
            }
            if let Some(selector) = &package.source_selector {
                validate_non_empty(path, "[[external-package]].source-selector", selector)?;
            }
        }

        for dependency in &self.dependencies {
            validate_non_empty(path, "[[dependency]].from", &dependency.from)?;
            if !package_ids.contains(dependency.from.as_str()) {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "[[dependency]] references unknown package id `{}` in `from`",
                        dependency.from
                    ),
                });
            }
            validate_kind(path, "[[dependency]].kind", &dependency.kind)?;
            validate_non_empty(path, "[[dependency]].name", &dependency.name)?;
            validate_non_empty(path, "[[dependency]].package", &dependency.package)?;
            validate_target_kind(path, "[[dependency]].target", &dependency.target_kind)?;
            validate_non_empty(path, "[[dependency]].target-id", &dependency.target_id)?;
            match dependency.target_kind.as_str() {
                "local" => {
                    if !package_ids.contains(dependency.target_id.as_str()) {
                        return Err(Error::LockfileValidation {
                            path: path.to_path_buf(),
                            message: format!(
                                "[[dependency]] references unknown local target id `{}`",
                                dependency.target_id
                            ),
                        });
                    }
                }
                "external" => {
                    if !external_ids.contains(dependency.target_id.as_str()) {
                        return Err(Error::LockfileValidation {
                            path: path.to_path_buf(),
                            message: format!(
                                "[[dependency]] references unknown external target id `{}`",
                                dependency.target_id
                            ),
                        });
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("# This file is generated by craft.\n");
        out.push_str("version = ");
        out.push_str(&self.version.to_string());
        out.push('\n');
        push_string_line(&mut out, "manifest", &self.manifest);
        push_string_line(&mut out, "manifest-digest", &self.manifest_digest);
        if let Some(path) = &self.workspace_script {
            push_string_line(&mut out, "workspace-script", path);
        }
        if let Some(digest) = &self.workspace_script_digest {
            push_string_line(&mut out, "workspace-script-digest", digest);
        }
        for input in &self.workspace_env {
            out.push('\n');
            out.push_str("[[workspace-env]]\n");
            push_string_line(&mut out, "name", &input.name);
            if let Some(value) = &input.value {
                push_string_line(&mut out, "value", value);
            }
        }

        for package in &self.packages {
            out.push('\n');
            out.push_str("[[package]]\n");
            push_string_line(&mut out, "id", &package.id);
            push_string_line(&mut out, "name", &package.name);
            push_string_line(&mut out, "version", &package.version);
            push_string_line(&mut out, "source", &package.source_kind);
            if let Some(value) = &package.source_value {
                push_string_line(&mut out, "source-value", value);
            }
            push_string_line(&mut out, "manifest", &package.manifest);
            push_string_line(&mut out, "manifest-digest", &package.manifest_digest);
            if let Some(path) = &package.craft_script {
                push_string_line(&mut out, "craft-script", path);
            }
            if let Some(digest) = &package.craft_script_digest {
                push_string_line(&mut out, "craft-script-digest", digest);
            }
        }

        for target in &self.package_targets {
            out.push('\n');
            out.push_str("[[package-target]]\n");
            push_string_line(&mut out, "package", &target.package_id);
            push_string_line(&mut out, "kind", &target.kind);
            if let Some(name) = &target.name {
                push_string_line(&mut out, "name", name);
            }
            push_string_line(&mut out, "root", &target.root);
        }

        for package in &self.external_packages {
            out.push('\n');
            out.push_str("[[external-package]]\n");
            push_string_line(&mut out, "id", &package.id);
            push_string_line(&mut out, "name", &package.name);
            push_string_line(&mut out, "source", &package.source_kind);
            if let Some(value) = &package.source_value {
                push_string_line(&mut out, "source-value", value);
            }
            if let Some(version) = &package.version {
                push_string_line(&mut out, "version", version);
            }
            if let Some(locator) = &package.source_locator {
                push_string_line(&mut out, "source-locator", locator);
            }
            if let Some(selector) = &package.source_selector {
                push_string_line(&mut out, "source-selector", selector);
            }
        }

        for input in &self.package_env {
            out.push('\n');
            out.push_str("[[package-env]]\n");
            push_string_line(&mut out, "package", &input.package_id);
            push_string_line(&mut out, "name", &input.name);
            if let Some(value) = &input.value {
                push_string_line(&mut out, "value", value);
            }
        }

        for dependency in &self.dependencies {
            out.push('\n');
            out.push_str("[[dependency]]\n");
            push_string_line(&mut out, "from", &dependency.from);
            push_string_line(&mut out, "kind", &dependency.kind);
            push_string_line(&mut out, "name", &dependency.name);
            push_string_line(&mut out, "package", &dependency.package);
            push_string_line(&mut out, "target", &dependency.target_kind);
            push_string_line(&mut out, "target-id", &dependency.target_id);
        }

        out
    }
}

fn assign_key_value(
    lockfile: &mut Lockfile,
    section: Section,
    key: &str,
    raw_value: &str,
) -> std::result::Result<(), String> {
    match section {
        Section::Root => match key {
            "version" => lockfile.version = parse_u32(raw_value)?,
            "manifest" => lockfile.manifest = parse_string(raw_value)?,
            "manifest-digest" => lockfile.manifest_digest = parse_string(raw_value)?,
            "workspace-script" => lockfile.workspace_script = Some(parse_string(raw_value)?),
            "workspace-script-digest" => {
                lockfile.workspace_script_digest = Some(parse_string(raw_value)?)
            }
            _ => return Err(format!("unsupported root key `{key}`")),
        },
        Section::Package(index) => {
            let package = &mut lockfile.packages[index];
            match key {
                "id" => package.id = parse_string(raw_value)?,
                "name" => package.name = parse_string(raw_value)?,
                "version" => package.version = parse_string(raw_value)?,
                "source" => package.source_kind = parse_string(raw_value)?,
                "source-value" => package.source_value = Some(parse_string(raw_value)?),
                "manifest" => package.manifest = parse_string(raw_value)?,
                "manifest-digest" => package.manifest_digest = parse_string(raw_value)?,
                "craft-script" => package.craft_script = Some(parse_string(raw_value)?),
                "craft-script-digest" => {
                    package.craft_script_digest = Some(parse_string(raw_value)?)
                }
                _ => return Err(format!("unsupported [[package]] key `{key}`")),
            }
        }
        Section::ExternalPackage(index) => {
            let package = &mut lockfile.external_packages[index];
            match key {
                "id" => package.id = parse_string(raw_value)?,
                "name" => package.name = parse_string(raw_value)?,
                "source" => package.source_kind = parse_string(raw_value)?,
                "source-value" => package.source_value = Some(parse_string(raw_value)?),
                "version" => package.version = Some(parse_string(raw_value)?),
                "source-locator" => package.source_locator = Some(parse_string(raw_value)?),
                "source-selector" => package.source_selector = Some(parse_string(raw_value)?),
                _ => return Err(format!("unsupported [[external-package]] key `{key}`")),
            }
        }
        Section::PackageTarget(index) => {
            let target = &mut lockfile.package_targets[index];
            match key {
                "package" => target.package_id = parse_string(raw_value)?,
                "kind" => target.kind = parse_string(raw_value)?,
                "name" => target.name = Some(parse_string(raw_value)?),
                "root" => target.root = parse_string(raw_value)?,
                _ => return Err(format!("unsupported [[package-target]] key `{key}`")),
            }
        }
        Section::WorkspaceEnv(index) => {
            let input = &mut lockfile.workspace_env[index];
            match key {
                "name" => input.name = parse_string(raw_value)?,
                "value" => input.value = Some(parse_string(raw_value)?),
                _ => return Err(format!("unsupported [[workspace-env]] key `{key}`")),
            }
        }
        Section::PackageEnv(index) => {
            let input = &mut lockfile.package_env[index];
            match key {
                "package" => input.package_id = parse_string(raw_value)?,
                "name" => input.name = parse_string(raw_value)?,
                "value" => input.value = Some(parse_string(raw_value)?),
                _ => return Err(format!("unsupported [[package-env]] key `{key}`")),
            }
        }
        Section::Dependency(index) => {
            let dependency = &mut lockfile.dependencies[index];
            match key {
                "from" => dependency.from = parse_string(raw_value)?,
                "kind" => dependency.kind = parse_string(raw_value)?,
                "name" => dependency.name = parse_string(raw_value)?,
                "package" => dependency.package = parse_string(raw_value)?,
                "target" => dependency.target_kind = parse_string(raw_value)?,
                "target-id" => dependency.target_id = parse_string(raw_value)?,
                _ => return Err(format!("unsupported [[dependency]] key `{key}`")),
            }
        }
    }

    Ok(())
}

fn dependency_kind(kind: DependencyKind) -> &'static str {
    match kind {
        DependencyKind::Normal => "normal",
        DependencyKind::Dev => "dev",
        DependencyKind::Build => "build",
    }
}

fn target_kind(kind: TargetKind) -> &'static str {
    kind.as_str()
}

fn package_lock_id(id: &PackageId) -> String {
    match &id.source {
        SourceId::Root => format!("{} {} root", id.name, id.version),
        SourceId::WorkspaceMember { path } => {
            format!("{} {} workspace-member:{path}", id.name, id.version)
        }
        SourceId::PathDependency { path } => {
            format!("{} {} path:{path}", id.name, id.version)
        }
        SourceId::Registry { name: Some(name) } => {
            format!("{} {} registry:{name}", id.name, id.version)
        }
        SourceId::Registry { name: None } => format!("{} {} registry", id.name, id.version),
    }
}

fn external_package_lock_id(id: &ExternalPackageId) -> String {
    match &id.source {
        SourceId::PathDependency { path } => match &id.version {
            Some(version) => format!("{} {} path:{path}", id.package_name, version),
            None => format!("{} path:{path}", id.package_name),
        },
        SourceId::Registry { name: Some(name) } => match &id.version {
            Some(version) => format!("{} {} registry:{name}", id.package_name, version),
            None => format!("{} registry:{name}", id.package_name),
        },
        SourceId::Registry { name: None } => match &id.version {
            Some(version) => format!("{} {} registry", id.package_name, version),
            None => format!("{} registry", id.package_name),
        },
        SourceId::WorkspaceMember { path } => match &id.version {
            Some(version) => format!("{} {} workspace-member:{path}", id.package_name, version),
            None => format!("{} workspace-member:{path}", id.package_name),
        },
        SourceId::Root => match &id.version {
            Some(version) => format!("{} {} root", id.package_name, version),
            None => format!("{} root", id.package_name),
        },
    }
}

fn source_kind(source: &SourceId) -> &'static str {
    match source {
        SourceId::Root => "root",
        SourceId::WorkspaceMember { .. } => "workspace-member",
        SourceId::PathDependency { .. } => "path",
        SourceId::Registry { .. } => "registry",
    }
}

fn source_value(source: &SourceId) -> Option<String> {
    match source {
        SourceId::Root => None,
        SourceId::WorkspaceMember { path } => Some(path.clone()),
        SourceId::PathDependency { path } => Some(path.clone()),
        SourceId::Registry { name } => name.clone(),
    }
}

fn source_locator(manifest: &Manifest, source: &SourceId) -> Option<String> {
    let SourceId::Registry { name } = source else {
        return None;
    };
    manifest
        .sources
        .get(name.as_deref().unwrap_or("default"))
        .and_then(source::source_locator)
}

fn source_selector(manifest: &Manifest, source: &SourceId) -> Option<String> {
    let SourceId::Registry { name } = source else {
        return None;
    };
    manifest
        .sources
        .get(name.as_deref().unwrap_or("default"))
        .and_then(source::source_selector)
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

fn validate_non_empty(path: &Path, field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::LockfileValidation {
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
        _ => Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!(
                "{field} and {field}-digest must either both be present or both be absent"
            ),
        }),
    }
}

fn validate_env_input_name(path: &Path, field: &str, value: &str) -> Result<()> {
    validate_non_empty(path, field, value)?;
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!("{field} must not be empty"),
        });
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!("{field} must start with an ASCII letter or `_`, found `{value}`"),
        });
    }
    if chars.any(|ch| !(ch == '_' || ch.is_ascii_alphanumeric())) {
        return Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!(
                "{field} must contain only ASCII letters, digits, or `_`, found `{value}`"
            ),
        });
    }
    Ok(())
}

fn validate_digest(path: &Path, field: &str, value: &str) -> Result<()> {
    validate_non_empty(path, field, value)?;
    if !value.starts_with("fnv1a64:") || value.len() != "fnv1a64:".len() + 16 {
        return Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!("{field} must be an `fnv1a64:` digest"),
        });
    }
    Ok(())
}

fn validate_source_kind(path: &Path, field: &str, value: &str) -> Result<()> {
    match value {
        "root" | "workspace-member" | "path" | "registry" => Ok(()),
        _ => Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!("{field} has unsupported source kind `{value}`"),
        }),
    }
}

fn validate_kind(path: &Path, field: &str, value: &str) -> Result<()> {
    match value {
        "normal" | "dev" | "build" => Ok(()),
        _ => Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!("{field} has unsupported dependency kind `{value}`"),
        }),
    }
}

fn validate_target_kind(path: &Path, field: &str, value: &str) -> Result<()> {
    match value {
        "local" | "external" => Ok(()),
        _ => Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!("{field} has unsupported dependency target `{value}`"),
        }),
    }
}

fn validate_target_kind_name(path: &Path, field: &str, value: &str) -> Result<()> {
    match value {
        "lib" | "bin" | "test" | "example" => Ok(()),
        _ => Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!("{field} has unsupported package target kind `{value}`"),
        }),
    }
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

fn push_string_line(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push_str(" = \"");
    out.push_str(&escape_string(value));
    out.push_str("\"\n");
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
    use super::{LockStatus, LockWriteResult, Lockfile, lock_status, sync_lockfile};
    use crate::elaborate::plan;
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
    fn renders_workspace_lockfile_from_package_graph() {
        let root = temp_dir("craft-lockfile");
        let app_dir = root.join("app");
        let util_dir = root.join("util");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&util_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
members = ["app", "util"]

[workspace.dependencies]
shared = "2"

[source.default]
git = "https://example.com/shared.git"
branch = "stable"
"#,
        )
        .unwrap();
        fs::write(
            root.join("craft.rn"),
            "use craft.plan;\npub fn craft(p: *mut plan.Plan) void { let _ = p; }\n",
        )
        .unwrap();
        let env_name = format!(
            "KRAFT_LOCK_ENV_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        unsafe { std::env::set_var(&env_name, "enabled") };
        fs::write(
            app_dir.join("Craft.toml"),
            format!(
                r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[craft]
env = ["{env_name}"]

[[bin]]
name = "app"
root = "src/main.rn"

[dependencies]
util = {{ path = "../util" }}
shared = {{ workspace = true, features = ["simd"] }}
"#
            ),
        )
        .unwrap();
        fs::write(
            app_dir.join("craft.rn"),
            format!(
                r#"
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {{
    match (p.env("{env_name}")) {{
        .{{ Some: value }} => p.define_string("env_value", value),
        .None => {{}},
    }}
}}
"#
            ),
        )
        .unwrap();
        fs::write(
            util_dir.join("Craft.toml"),
            r#"
[package]
name = "util"
version = "0.1.0"
kern = "0.7"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &root_manifest,
            &members,
            true,
            crate::script::ScriptCommand::Lock,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let lockfile = Lockfile::from_elaboration(&manifest_path, &elaboration).unwrap();
        let rendered = lockfile.render();

        assert!(rendered.contains("version = 1"));
        assert!(rendered.contains("[[package]]"));
        assert!(rendered.contains("[[package-target]]"));
        assert!(rendered.contains("[[external-package]]"));
        assert!(rendered.contains("[[package-env]]"));
        assert!(rendered.contains("id = \"app 0.1.0 workspace-member:app\""));
        assert!(
            rendered.contains("package = \"app 0.1.0 workspace-member:app\"")
                && rendered.contains("kind = \"bin\"")
        );
        assert!(rendered.contains("workspace-script = \"craft.rn\""));
        assert!(rendered.contains("craft-script = \"app/craft.rn\""));
        assert!(rendered.contains(&format!("name = \"{env_name}\"")));
        assert!(rendered.contains("value = \"enabled\""));
        assert!(rendered.contains("target-id = \"util 0.1.0 workspace-member:util\""));
        assert!(rendered.contains("name = \"shared\""));
        assert!(rendered.contains("target = \"external\""));
        assert!(rendered.contains("id = \"shared 2 registry\""));
        assert!(rendered.contains("source-locator = \"https://example.com/shared.git\""));
        assert!(rendered.contains("source-selector = \"branch:stable\""));
        assert!(rendered.contains("manifest-digest = \"fnv1a64:"));

        unsafe { std::env::remove_var(&env_name) };
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn loads_rendered_lockfile_roundtrip() {
        let root = temp_dir("craft-lockfile-load");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
members = ["app"]

[workspace.dependencies]
shared = "2"
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[dependencies]
shared = { workspace = true }
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &root_manifest,
            &members,
            true,
            crate::script::ScriptCommand::Lock,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let expected = Lockfile::from_elaboration(&manifest_path, &elaboration).unwrap();
        let (lock_path, _) = sync_lockfile(&manifest_path, &elaboration).unwrap();
        let loaded = Lockfile::load(&lock_path).unwrap();

        assert_eq!(loaded, expected);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn writes_lockfile_into_workspace_root() {
        let root = temp_dir("craft-lockfile-write");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
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
kern = "0.7"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &root_manifest,
            &members,
            true,
            crate::script::ScriptCommand::Lock,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let (lock_path, _) = sync_lockfile(&manifest_path, &elaboration).unwrap();
        let contents = fs::read_to_string(&lock_path).unwrap();

        assert_eq!(lock_path, root.join("Craft.lock"));
        assert!(contents.contains("manifest = \"Craft.toml\""));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sync_lockfile_reports_created_updated_and_unchanged() {
        let root = temp_dir("craft-lockfile-sync");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
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
kern = "0.7"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &root_manifest,
            &members,
            true,
            crate::script::ScriptCommand::Lock,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();

        let (_, created) = sync_lockfile(&manifest_path, &elaboration).unwrap();
        assert_eq!(created, LockWriteResult::Created);

        let (_, unchanged) = sync_lockfile(&manifest_path, &elaboration).unwrap();
        assert_eq!(unchanged, LockWriteResult::Unchanged);

        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.2.0"
kern = "0.7"
"#,
        )
        .unwrap();

        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &root_manifest,
            &members,
            true,
            crate::script::ScriptCommand::Lock,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let (_, updated) = sync_lockfile(&manifest_path, &elaboration).unwrap();
        assert_eq!(updated, LockWriteResult::Updated);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn lockfile_status_tracks_registry_source_identity_changes() {
        let root = temp_dir("craft-lockfile-source-identity");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
members = ["app"]

[workspace.dependencies]
shared = "2"

[source.default]
git = "https://example.com/shared.git"
branch = "stable"
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[dependencies]
shared = { workspace = true }
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &root_manifest,
            &members,
            true,
            crate::script::ScriptCommand::Lock,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();

        let _ = sync_lockfile(&manifest_path, &elaboration).unwrap();
        assert_eq!(
            lock_status(&manifest_path, &elaboration).unwrap(),
            LockStatus::Current
        );

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
members = ["app"]

[workspace.dependencies]
shared = "2"

[source.default]
git = "https://example.com/shared.git"
rev = "abc123"
"#,
        )
        .unwrap();

        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &root_manifest,
            &members,
            true,
            crate::script::ScriptCommand::Lock,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        assert_eq!(
            lock_status(&manifest_path, &elaboration).unwrap(),
            LockStatus::Stale
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reports_current_and_stale_lockfile_status() {
        let root = temp_dir("craft-lockfile-status");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();
        let env_name = format!(
            "KRAFT_STATUS_ENV_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        unsafe { std::env::set_var(&env_name, "v1") };

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
members = ["app"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            format!(
                r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[craft]
env = ["{env_name}"]
"#
            ),
        )
        .unwrap();
        fs::write(
            app_dir.join("craft.rn"),
            format!(
                r#"
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {{
    match (p.env("{env_name}")) {{
        .{{ Some: value }} => p.define_string("env_value", value),
        .None => {{}},
    }}
}}
"#
            ),
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &root_manifest,
            &members,
            true,
            crate::script::ScriptCommand::Lock,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();

        assert_eq!(
            lock_status(&manifest_path, &elaboration).unwrap(),
            LockStatus::Missing
        );

        let _ = sync_lockfile(&manifest_path, &elaboration).unwrap();
        assert_eq!(
            lock_status(&manifest_path, &elaboration).unwrap(),
            LockStatus::Current
        );

        unsafe { std::env::set_var(&env_name, "v2") };
        let elaboration = plan(
            &manifest_path,
            &root_manifest,
            &members,
            true,
            crate::script::ScriptCommand::Lock,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        assert_eq!(
            lock_status(&manifest_path, &elaboration).unwrap(),
            LockStatus::Stale
        );

        fs::write(
            app_dir.join("Craft.toml"),
            format!(
                r#"
[package]
name = "app"
version = "0.2.0"
kern = "0.7"

[craft]
env = ["{env_name}"]
"#
            ),
        )
        .unwrap();

        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &root_manifest,
            &members,
            true,
            crate::script::ScriptCommand::Lock,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        assert_eq!(
            lock_status(&manifest_path, &elaboration).unwrap(),
            LockStatus::Stale
        );

        unsafe { std::env::remove_var(&env_name) };
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_invalid_dependency_target_reference() {
        let root = temp_dir("craft-lockfile-invalid");
        let lock_path = root.join("Craft.lock");
        fs::write(
            &lock_path,
            r#"
version = 1
manifest = "Craft.toml"
manifest-digest = "fnv1a64:1234567890abcdef"

[[package]]
id = "app 0.1.0 workspace-member:app"
name = "app"
version = "0.1.0"
source = "workspace-member"
source-value = "app"
manifest = "app/Craft.toml"
manifest-digest = "fnv1a64:1234567890abcdef"

[[external-package]]
id = "util 0.1.0 registry"
name = "util"
source = "registry"
version = "0.1.0"

[[dependency]]
from = "app 0.1.0 workspace-member:app"
kind = "normal"
name = "util"
package = "util"
target = "external"
target-id = "missing 0.1.0 registry"
"#,
        )
        .unwrap();

        let err = Lockfile::load(&lock_path).unwrap_err();
        assert!(err.to_string().contains("unknown external target id"));

        let _ = fs::remove_dir_all(root);
    }
}
