//! Lockfile construction from elaborated package graphs.
//!
//! Building a lockfile extracts local/external package identities, dependency
//! edges, source proofs, and stable target references from the resolved graph.

use super::{
    LockedDependency, LockedExternalPackage, LockedPackage, LockedPackageResource,
    LockedPackageTarget, Lockfile,
};
use crate::elaborate::ElaborationPlan;
use crate::error::{Error, Result};
use crate::graph::{DependencyKind, PackageId, SourceId};
use crate::manifest::ResourceSpec;
use crate::plan::TargetKind;
use crate::publish;
use crate::resolver::{ExternalPackageId, ResolvedDependencyTarget};
use std::fs;
use std::path::Path;

impl Lockfile {
    pub fn from_elaboration(manifest_path: &Path, elaboration: &ElaborationPlan) -> Result<Self> {
        let resolved = &elaboration.resolved_graph;
        let root = &resolved.workspace_root;
        let manifest = relative_display(root, manifest_path);
        let manifest_digest = digest_file(manifest_path)?;
        let mut packages = Vec::new();
        let mut package_targets = Vec::new();
        let mut package_resources = Vec::new();
        let mut external_packages = Vec::new();
        let mut dependencies = Vec::new();
        let mut publish_proofs = Vec::new();

        for package in &resolved.packages {
            let package_id = package_lock_id(&package.id);
            packages.push(LockedPackage {
                id: package_id.clone(),
                name: package.id.name.clone(),
                version: package.id.version.clone(),
                source_kind: source_kind(&package.id.source).to_string(),
                source_value: source_value(&package.id.source),
                manifest: relative_display(root, &package.manifest_path),
                manifest_digest: digest_file(&package.manifest_path)?,
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
            for (name, spec) in &package_plan.resources {
                package_resources.push(LockedPackageResource {
                    package_id: package_id.clone(),
                    name: name.clone(),
                    source_kind: resource_source_kind(spec).to_string(),
                    source_value: resource_source_value(spec),
                    source_locator: resource_source_locator(spec),
                    source_selector: resource_source_selector(spec),
                });
            }
            if let Some(input) = publish_input_for_locked_package(
                root,
                manifest_path,
                &elaboration.manifest,
                package,
            )? {
                publish_proofs.push(publish::expected_publish_proof_for_lock(
                    package_id.clone(),
                    input,
                )?);
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
                source_locator: source_locator(&package.id.source),
                source_selector: source_selector(&package.id.source),
            });
        }

        Ok(Self {
            version: 1,
            manifest,
            manifest_digest,
            packages,
            package_targets,
            package_resources,
            external_packages,
            dependencies,
            publish_proofs,
        })
    }
}

fn publish_input_for_locked_package(
    workspace_root: &Path,
    root_manifest_path: &Path,
    root_manifest: &crate::manifest::Manifest,
    package: &crate::resolver::ResolvedPackageNode,
) -> Result<Option<publish::PublishPackageInput>> {
    let workspace_defaults = root_manifest
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.package.as_ref());
    let package_root = package
        .manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let package_manifest = crate::workspace::load_member_manifest(
        root_manifest_path,
        root_manifest,
        &package.manifest_path,
    )?;
    let Some(manifest_package) = &package_manifest.package else {
        return Ok(None);
    };

    match &package.id.source {
        SourceId::Root => {}
        SourceId::WorkspaceMember { .. } => {
            let Some(workspace) = &root_manifest.workspace else {
                return Ok(None);
            };
            let member_path = relative_display(workspace_root, package_root);
            if !workspace
                .exports
                .values()
                .any(|export| export.member == member_path)
            {
                return Ok(None);
            }
        }
        SourceId::PathDependency { .. } | SourceId::GitDependency { .. } => return Ok(None),
    }

    let Some(description) = manifest_package
        .description
        .clone()
        .or_else(|| workspace_defaults.and_then(|package| package.description.clone()))
    else {
        return Ok(None);
    };
    let Some(license) = manifest_package
        .license
        .clone()
        .or_else(|| workspace_defaults.and_then(|package| package.license.clone()))
    else {
        return Ok(None);
    };
    let authors = if manifest_package.authors.is_empty() {
        workspace_defaults
            .map(|package| package.authors.clone())
            .unwrap_or_default()
    } else {
        manifest_package.authors.clone()
    };
    if authors.is_empty() {
        return Ok(None);
    }
    let (readme, inherited_readme) = match manifest_package.readme.clone() {
        Some(readme) => (readme, false),
        None => {
            let Some(readme) = workspace_defaults.and_then(|package| package.readme.clone()) else {
                return Ok(None);
            };
            (readme, true)
        }
    };
    let readme_path = if inherited_readme {
        workspace_root.join(&readme)
    } else {
        package_root.join(&readme)
    };
    if !readme_path.is_file() {
        return Ok(None);
    }
    let Some(repository) = manifest_package
        .repository
        .clone()
        .or_else(|| workspace_defaults.and_then(|package| package.repository.clone()))
    else {
        return Ok(None);
    };

    Ok(Some(publish::PublishPackageInput {
        path: relative_display(workspace_root, package_root),
        package_root: package_root.to_path_buf(),
        manifest: package_manifest,
        description,
        license,
        authors,
        readme,
        repository,
    }))
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
        SourceId::GitDependency { git, .. } => {
            format!(
                "{} {} git:{}#{}",
                id.name,
                id.version,
                git,
                git_selector(&id.source)
            )
        }
    }
}

fn external_package_lock_id(id: &ExternalPackageId) -> String {
    match &id.source {
        SourceId::PathDependency { path } => match &id.version {
            Some(version) => format!("{} {} path:{path}", id.package_name, version),
            None => format!("{} path:{path}", id.package_name),
        },
        SourceId::GitDependency { git, .. } => match &id.version {
            Some(version) => format!(
                "{} {} git:{}#{}",
                id.package_name,
                version,
                git,
                git_selector(&id.source)
            ),
            None => format!(
                "{} git:{}#{}",
                id.package_name,
                git,
                git_selector(&id.source)
            ),
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
        SourceId::GitDependency { .. } => "git",
    }
}

fn source_value(source: &SourceId) -> Option<String> {
    match source {
        SourceId::Root => None,
        SourceId::WorkspaceMember { path } => Some(path.clone()),
        SourceId::PathDependency { path } => Some(path.clone()),
        SourceId::GitDependency { git, .. } => Some(git.clone()),
    }
}

fn source_locator(source: &SourceId) -> Option<String> {
    if let SourceId::GitDependency { git, .. } = source {
        return Some(git.clone());
    }
    None
}

fn source_selector(source: &SourceId) -> Option<String> {
    if matches!(source, SourceId::GitDependency { .. }) {
        return Some(git_selector(source));
    }
    None
}

fn resource_source_kind(spec: &ResourceSpec) -> &'static str {
    if spec.path.is_some() {
        "path"
    } else if spec.git.is_some() {
        "git"
    } else {
        unreachable!("validated resources must declare a source")
    }
}

fn resource_source_value(spec: &ResourceSpec) -> Option<String> {
    spec.path.clone().or_else(|| spec.git.clone())
}

fn resource_source_locator(spec: &ResourceSpec) -> Option<String> {
    spec.git.clone()
}

fn resource_source_selector(spec: &ResourceSpec) -> Option<String> {
    spec.git.as_ref().map(|_| {
        if let Some(rev) = &spec.rev {
            format!("rev:{rev}")
        } else if let Some(branch) = &spec.branch {
            format!("branch:{branch}")
        } else if let Some(tag) = &spec.tag {
            format!("tag:{tag}")
        } else {
            "default".to_string()
        }
    })
}

fn git_selector(source: &SourceId) -> String {
    match source {
        SourceId::GitDependency {
            rev, branch, tag, ..
        } => {
            if let Some(rev) = rev {
                format!("rev:{rev}")
            } else if let Some(branch) = branch {
                format!("branch:{branch}")
            } else if let Some(tag) = tag {
                format!("tag:{tag}")
            } else {
                "default".to_string()
            }
        }
        _ => unreachable!("git_selector only accepts git sources"),
    }
}

fn relative_display(root: &Path, path: &Path) -> String {
    let text = path
        .strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));
    if text.is_empty() {
        ".".to_string()
    } else {
        text
    }
}

fn digest_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).map_err(|err| Error::from_io(path, err))?;
    Ok(format!("fnv1a64:{:016x}", fnv1a64(&bytes)))
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
