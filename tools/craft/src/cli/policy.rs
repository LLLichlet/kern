use crate::elaborate;
use crate::error::{Error, Result};
use crate::lockfile;
use crate::manifest::Manifest;
use crate::workspace;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CheckSourceSummary {
    pub(super) git_sources: usize,
    pub(super) git_packages: usize,
    pub(super) path_packages: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SourceSecuritySummary {
    pub(super) policy_mode: crate::manifest::ReleaseSourcePolicy,
    pub(super) floating_git_sources: usize,
    pub(super) insecure_transport_sources: usize,
    pub(super) warnings: Vec<String>,
    pub(super) suppressed: Vec<String>,
    pub(super) release_blockers: Vec<String>,
}

impl SourceSecuritySummary {
    pub(super) fn warning_count(&self) -> usize {
        self.warnings.len()
    }

    pub(super) fn suppressed_count(&self) -> usize {
        self.suppressed.len()
    }

    pub(super) fn release_blockers(&self) -> &[String] {
        self.release_blockers.as_slice()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PublishPackageSummary {
    pub(super) name: String,
    pub(super) version: String,
    pub(super) manifest_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PublishIssue {
    package_name: String,
    manifest_path: PathBuf,
    missing_fields: Vec<&'static str>,
    missing_readme_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PublishSummary {
    pub(super) ready: Vec<PublishPackageSummary>,
    pub(super) blocked: Vec<PublishIssue>,
}

pub(super) fn summarize_check_sources(
    resolved: &crate::resolver::ResolvedGraph,
) -> CheckSourceSummary {
    let mut git_packages = 0usize;
    let mut path_packages = 0usize;

    for package in &resolved.external_packages {
        match &package.id.source {
            crate::graph::SourceId::PathDependency { .. } => path_packages += 1,
            crate::graph::SourceId::GitDependency { .. } => git_packages += 1,
            crate::graph::SourceId::Root | crate::graph::SourceId::WorkspaceMember { .. } => {}
        }
    }

    CheckSourceSummary {
        git_sources: git_packages,
        git_packages,
        path_packages,
    }
}

pub(super) fn summarize_source_security(manifest: &Manifest) -> SourceSecuritySummary {
    let policy_mode = manifest
        .craft
        .as_ref()
        .and_then(|craft| craft.release_source_policy)
        .unwrap_or(crate::manifest::ReleaseSourcePolicy::Enforce);
    let allow_floating_git = manifest
        .craft
        .as_ref()
        .map(|craft| {
            craft
                .allow_floating_git
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let allow_insecure_source = manifest
        .craft
        .as_ref()
        .map(|craft| {
            craft
                .allow_insecure_source
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let mut floating_git_sources = 0usize;
    let mut insecure_transport_sources = 0usize;
    let mut warnings = Vec::new();
    let mut suppressed = Vec::new();

    for source in release_policy_dependencies(manifest) {
        let git = source.git;
        if is_insecure_git_source(git) {
            insecure_transport_sources += 1;
            let label = format_release_source_label(&source.name, "insecure-transport");
            if allowlist_contains(&allow_insecure_source, &source.name) {
                suppressed.push(label);
            } else {
                warnings.push(label);
            }
        }

        if source.rev.is_none() && source.tag.is_none() {
            floating_git_sources += 1;
            let label = format_release_source_label(&source.name, "floating-git");
            if allowlist_contains(&allow_floating_git, &source.name) {
                suppressed.push(label);
            } else {
                warnings.push(label);
            }
        }
    }

    let release_blockers = match policy_mode {
        crate::manifest::ReleaseSourcePolicy::Enforce => warnings.clone(),
        crate::manifest::ReleaseSourcePolicy::Warn | crate::manifest::ReleaseSourcePolicy::Off => {
            Vec::new()
        }
    };

    SourceSecuritySummary {
        policy_mode,
        floating_git_sources,
        insecure_transport_sources,
        warnings,
        suppressed,
        release_blockers,
    }
}

struct ReleasePolicySource<'a> {
    name: String,
    git: &'a str,
    rev: Option<&'a str>,
    tag: Option<&'a str>,
}

fn release_policy_dependencies(manifest: &Manifest) -> Vec<ReleasePolicySource<'_>> {
    let mut dependencies = BTreeMap::new();

    if let Some(workspace) = &manifest.workspace {
        collect_release_policy_dependencies(&mut dependencies, &workspace.dependencies);
    }
    collect_release_policy_dependencies(&mut dependencies, &manifest.dependencies);
    collect_release_policy_dependencies(&mut dependencies, &manifest.dev_dependencies);
    collect_release_policy_dependencies(&mut dependencies, &manifest.build_dependencies);
    for (name, resource) in &manifest.resources {
        if let Some(git) = resource.git.as_deref() {
            dependencies.insert(
                format!("resource:{name}"),
                ReleasePolicySource {
                    name: format!("resource:{name}"),
                    git,
                    rev: resource.rev.as_deref(),
                    tag: resource.tag.as_deref(),
                },
            );
        }
    }

    dependencies.into_values().collect()
}

fn collect_release_policy_dependencies<'a>(
    out: &mut BTreeMap<String, ReleasePolicySource<'a>>,
    section: &'a BTreeMap<String, crate::manifest::DependencySpec>,
) {
    for (name, spec) in section {
        let crate::manifest::DependencySpec::Detailed(dep) = spec else {
            continue;
        };
        if let Some(git) = dep.git.as_deref() {
            out.entry(name.clone()).or_insert(ReleasePolicySource {
                name: name.clone(),
                git,
                rev: dep.rev.as_deref(),
                tag: dep.tag.as_deref(),
            });
        }
    }
}

fn allowlist_contains(allowlist: &BTreeSet<&str>, name: &str) -> bool {
    allowlist.contains(name) || name.starts_with("resource:") && allowlist.contains(&name[9..])
}

fn format_release_source_label(name: &str, suffix: &str) -> String {
    format!("{name}({suffix})")
}

fn is_insecure_git_source(locator: &str) -> bool {
    locator.starts_with("http://")
}

pub(super) fn validate_check_source_policy(
    manifest_path: &Path,
    selection: &elaborate::FeatureSelection,
    summary: &SourceSecuritySummary,
) -> Result<()> {
    if selection.profile != crate::script::ProfileSelection::Release
        || summary.release_blockers().is_empty()
    {
        return Ok(());
    }

    Err(Error::Validation {
        path: manifest_path.to_path_buf(),
        message: format!(
            "release source policy rejected: {}",
            summary.release_blockers().join(", ")
        ),
    })
}

pub(super) fn validate_publish_lock_status(
    manifest_path: &Path,
    lock_status: lockfile::LockStatus,
) -> Result<()> {
    if lock_status == lockfile::LockStatus::Current {
        return Ok(());
    }

    Err(Error::Validation {
        path: manifest_path.to_path_buf(),
        message: "publish requires a current canonical `Craft.lock`; run `craft lock` first"
            .to_string(),
    })
}

pub(super) fn publish_summary(
    root_manifest_path: &Path,
    root_manifest: &Manifest,
    workspace_members: &[workspace::WorkspaceMember],
) -> Result<PublishSummary> {
    let workspace_defaults = root_manifest
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.package.as_ref());
    let mut ready = Vec::new();
    let mut blocked = Vec::new();

    if let Some(package) = &root_manifest.package
        && package.publish != Some(false)
    {
        classify_publish_package(
            root_manifest_path,
            root_manifest_path,
            package,
            workspace_defaults,
            &mut ready,
            &mut blocked,
        )?;
    }

    for member in workspace_members {
        let Some(package) = &member.manifest.package else {
            continue;
        };
        if package.publish == Some(false) {
            continue;
        }
        classify_publish_package(
            root_manifest_path,
            &member.manifest_path,
            package,
            workspace_defaults,
            &mut ready,
            &mut blocked,
        )?;
    }

    if ready.is_empty() && blocked.is_empty() {
        return Err(Error::Validation {
            path: root_manifest_path.to_path_buf(),
            message: "publish found no publishable packages; set `[package].publish = true` or omit `publish = false`"
                .to_string(),
        });
    }

    Ok(PublishSummary { ready, blocked })
}

pub(super) fn validate_publish_metadata(summary: &PublishSummary) -> Result<()> {
    if summary.blocked.is_empty() {
        return Ok(());
    }

    let message = summary
        .blocked
        .iter()
        .map(|issue| {
            let mut parts = Vec::new();
            if !issue.missing_fields.is_empty() {
                parts.push(format!("missing {}", issue.missing_fields.join(", ")));
            }
            if let Some(path) = &issue.missing_readme_path {
                parts.push(format!("readme not found at {}", path.display()));
            }
            format!(
                "{} ({}): {}",
                issue.package_name,
                issue.manifest_path.display(),
                parts.join("; ")
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");

    Err(Error::Validation {
        path: summary.blocked[0].manifest_path.clone(),
        message: format!("publish metadata check failed: {message}"),
    })
}

fn classify_publish_package(
    root_manifest_path: &Path,
    manifest_path: &Path,
    package: &crate::manifest::Package,
    defaults: Option<&crate::manifest::WorkspacePackage>,
    ready: &mut Vec<PublishPackageSummary>,
    blocked: &mut Vec<PublishIssue>,
) -> Result<()> {
    let mut missing_fields = Vec::new();
    if publish_description(package, defaults).is_none() {
        missing_fields.push("[package].description");
    }
    if publish_license(package, defaults).is_none() {
        missing_fields.push("[package].license");
    }
    if publish_authors(package, defaults).is_none() {
        missing_fields.push("[package].authors");
    }
    let readme = publish_readme(package, defaults);
    if readme.is_none() {
        missing_fields.push("[package].readme");
    }
    if publish_repository(package, defaults).is_none() {
        missing_fields.push("[package].repository");
    }

    let package_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let workspace_root = root_manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let missing_readme_path = readme
        .map(|(readme, inherited)| {
            if inherited {
                workspace_root.join(readme)
            } else {
                package_root.join(readme)
            }
        })
        .filter(|path| !path.is_file());

    if missing_fields.is_empty() && missing_readme_path.is_none() {
        ready.push(PublishPackageSummary {
            name: package.name.clone(),
            version: package.version.clone(),
            manifest_path: manifest_path.to_path_buf(),
        });
    } else {
        blocked.push(PublishIssue {
            package_name: package.name.clone(),
            manifest_path: manifest_path.to_path_buf(),
            missing_fields,
            missing_readme_path,
        });
    }

    Ok(())
}

fn publish_description<'a>(
    package: &'a crate::manifest::Package,
    defaults: Option<&'a crate::manifest::WorkspacePackage>,
) -> Option<&'a str> {
    package
        .description
        .as_deref()
        .or_else(|| defaults.and_then(|defaults| defaults.description.as_deref()))
}

fn publish_license<'a>(
    package: &'a crate::manifest::Package,
    defaults: Option<&'a crate::manifest::WorkspacePackage>,
) -> Option<&'a str> {
    package
        .license
        .as_deref()
        .or_else(|| defaults.and_then(|defaults| defaults.license.as_deref()))
}

fn publish_authors<'a>(
    package: &'a crate::manifest::Package,
    defaults: Option<&'a crate::manifest::WorkspacePackage>,
) -> Option<&'a [String]> {
    if !package.authors.is_empty() {
        Some(package.authors.as_slice())
    } else {
        defaults
            .filter(|defaults| !defaults.authors.is_empty())
            .map(|defaults| defaults.authors.as_slice())
    }
}

fn publish_readme<'a>(
    package: &'a crate::manifest::Package,
    defaults: Option<&'a crate::manifest::WorkspacePackage>,
) -> Option<(&'a str, bool)> {
    package
        .readme
        .as_deref()
        .map(|value| (value, false))
        .or_else(|| {
            defaults
                .and_then(|defaults| defaults.readme.as_deref())
                .map(|value| (value, true))
        })
}

fn publish_repository<'a>(
    package: &'a crate::manifest::Package,
    defaults: Option<&'a crate::manifest::WorkspacePackage>,
) -> Option<&'a str> {
    package
        .repository
        .as_deref()
        .or_else(|| defaults.and_then(|defaults| defaults.repository.as_deref()))
}
