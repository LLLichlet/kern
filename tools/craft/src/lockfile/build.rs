use super::{
    LockedDependency, LockedExternalPackage, LockedPackage, LockedPackageResource,
    LockedPackageTarget, Lockfile,
};
use crate::elaborate::ElaborationPlan;
use crate::error::{Error, Result};
use crate::graph::{DependencyKind, PackageId, SourceId};
use crate::manifest::ResourceSpec;
use crate::plan::TargetKind;
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
        })
    }
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
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
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
