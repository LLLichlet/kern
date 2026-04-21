use crate::elaborate::ElaborationPlan;
use crate::error::{Error, Result};
use crate::graph::{PackageId, SourceId};
use crate::local_state;
use crate::manifest::{Manifest, ResourceSpec};
use crate::resolver::{ExternalPackageId, ResolvedGraph};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchSummary {
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedPackage {
    pub id: ExternalPackageId,
    pub source_path: PathBuf,
    pub cache_path: PathBuf,
    pub status: FetchStatus,
    pub source: FetchedSource,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResourceId {
    pub package_id: PackageId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedResource {
    pub id: ResourceId,
    pub source_path: PathBuf,
    pub cache_path: PathBuf,
    pub status: FetchStatus,
    pub source: FetchedSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchStatus {
    Created,
    Updated,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedSource {
    pub backend: FetchedSourceBackend,
    pub locator: String,
    pub selector: Option<FetchedGitSelector>,
    pub resolved_revision: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchedSourceBackend {
    PathDependency,
    GitDependency,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchedGitSelector {
    Default,
    Rev(String),
    Branch(String),
    Tag(String),
}

impl FetchedSourceBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PathDependency => "path",
            Self::GitDependency => "git",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedSource {
    root: PathBuf,
    identity: FetchedSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedSourcePath {
    source_path: PathBuf,
    identity: FetchedSource,
}

pub fn fetch_external_packages(resolved: &ResolvedGraph) -> Result<Vec<FetchedPackage>> {
    let cache_root = resolved.workspace_root.join(".craft").join("sources");
    let mut packages = Vec::new();

    for package in &resolved.external_packages {
        let resolved_source = source_path_for_external(resolved, &package.id)?;
        let cache_path = cache_path_for_external(&cache_root, &package.id)?;
        let status = materialize_tree(&resolved_source.source_path, &cache_path)?;
        validate_fetched_manifest(&cache_path)?;
        packages.push(FetchedPackage {
            id: package.id.clone(),
            source_path: resolved_source.source_path,
            cache_path,
            status,
            source: resolved_source.identity,
        });
    }

    Ok(packages)
}

pub fn fetch_package_resources(elaboration: &ElaborationPlan) -> Result<Vec<FetchedResource>> {
    let cache_root = elaboration
        .resolved_graph
        .workspace_root
        .join(".craft")
        .join("resources");
    let mut fetched = Vec::new();

    for package in &elaboration.packages {
        if package.plan.resources.is_empty() {
            continue;
        }

        let package_root = package
            .plan
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        for (name, spec) in &package.plan.resources {
            let resolved_source = source_path_for_resource(
                package_root,
                &elaboration.resolved_graph.workspace_root,
                &package.package_id,
                name,
                spec,
            )?;
            let cache_path = cache_path_for_resource(&cache_root, &package.package_id, name);
            let status = materialize_tree(&resolved_source.source_path, &cache_path)?;
            fetched.push(FetchedResource {
                id: ResourceId {
                    package_id: package.package_id.clone(),
                    name: name.clone(),
                },
                source_path: resolved_source.source_path,
                cache_path,
                status,
                source: resolved_source.identity,
            });
        }
    }

    Ok(fetched)
}

pub fn summarize_fetch(packages: &[FetchedPackage]) -> FetchSummary {
    let mut summary = FetchSummary {
        created: 0,
        updated: 0,
        unchanged: 0,
    };
    for package in packages {
        match package.status {
            FetchStatus::Created => summary.created += 1,
            FetchStatus::Updated => summary.updated += 1,
            FetchStatus::Unchanged => summary.unchanged += 1,
        }
    }
    summary
}

pub fn summarize_fetch_resources(resources: &[FetchedResource]) -> FetchSummary {
    let mut summary = FetchSummary {
        created: 0,
        updated: 0,
        unchanged: 0,
    };
    for resource in resources {
        match resource.status {
            FetchStatus::Created => summary.created += 1,
            FetchStatus::Updated => summary.updated += 1,
            FetchStatus::Unchanged => summary.unchanged += 1,
        }
    }
    summary
}

pub fn analysis_source_root_for_external(
    workspace_root: &Path,
    package: &ExternalPackageId,
) -> Result<Option<PathBuf>> {
    match &package.source {
        SourceId::PathDependency { path } => {
            let absolute = workspace_root.join(path);
            let source_path = absolute
                .canonicalize()
                .map_err(|err| Error::from_io(&absolute, err))?;
            Ok(Some(source_path))
        }
        SourceId::GitDependency { .. } => {
            let cache_root = workspace_root.join(".craft").join("sources");
            let cache_path = cache_path_for_external(&cache_root, package)?;
            Ok(cache_path.is_dir().then_some(cache_path))
        }
        SourceId::Root | SourceId::WorkspaceMember { .. } => Ok(None),
    }
}

fn source_path_for_external(
    resolved: &ResolvedGraph,
    package: &ExternalPackageId,
) -> Result<ResolvedSourcePath> {
    match &package.source {
        SourceId::PathDependency { path } => {
            let absolute = resolved.workspace_root.join(path);
            let source_path = absolute
                .canonicalize()
                .map_err(|err| Error::from_io(&absolute, err))?;
            Ok(ResolvedSourcePath {
                identity: FetchedSource {
                    backend: FetchedSourceBackend::PathDependency,
                    locator: source_path.display().to_string(),
                    selector: None,
                    resolved_revision: None,
                },
                source_path,
            })
        }
        SourceId::GitDependency {
            git,
            rev,
            branch,
            tag,
        } => {
            let prepared = prepare_git_dependency_root(
                &resolved.workspace_root,
                &resolved.workspace_root,
                package.package_name.as_str(),
                git,
                rev.as_deref(),
                branch.as_deref(),
                tag.as_deref(),
            )?;
            Ok(ResolvedSourcePath {
                source_path: prepared.root,
                identity: prepared.identity,
            })
        }
        SourceId::Root | SourceId::WorkspaceMember { .. } => Err(Error::Validation {
            path: resolved.workspace_root.join("Craft.toml"),
            message: format!(
                "unsupported external source kind for `{}`",
                package.package_name
            ),
        }),
    }
}

fn source_path_for_resource(
    package_root: &Path,
    workspace_root: &Path,
    package_id: &PackageId,
    name: &str,
    spec: &ResourceSpec,
) -> Result<ResolvedSourcePath> {
    if let Some(path) = &spec.path {
        let absolute = package_root.join(path);
        let source_path = absolute
            .canonicalize()
            .map_err(|err| Error::from_io(&absolute, err))?;
        return Ok(ResolvedSourcePath {
            identity: FetchedSource {
                backend: FetchedSourceBackend::PathDependency,
                locator: source_path.display().to_string(),
                selector: None,
                resolved_revision: None,
            },
            source_path,
        });
    }

    let Some(git) = spec.git.as_deref() else {
        return Err(Error::Validation {
            path: package_root.join("Craft.toml"),
            message: format!("resource `{name}` must declare `path` or `git`"),
        });
    };

    let prepared = prepare_git_dependency_root(
        package_root,
        workspace_root,
        &format!("{}-{}", package_id.name, name),
        git,
        spec.rev.as_deref(),
        spec.branch.as_deref(),
        spec.tag.as_deref(),
    )?;
    Ok(ResolvedSourcePath {
        source_path: prepared.root,
        identity: prepared.identity,
    })
}

fn prepare_git_dependency_root(
    config_root: &Path,
    workspace_root: &Path,
    package_name: &str,
    git_url: &str,
    rev: Option<&str>,
    branch: Option<&str>,
    tag: Option<&str>,
) -> Result<PreparedSource> {
    if let Some(local_repo_root) = resolve_local_git_repo(config_root, git_url) {
        return prepare_local_git_dependency_root(
            workspace_root,
            package_name,
            &local_repo_root,
            rev,
            branch,
            tag,
        );
    }

    let git_locator = git_url.to_string();
    let cache_root = workspace_root
        .join(".craft")
        .join("git-dependencies")
        .join(sanitize_segment(package_name))
        .join(format!(
            "{:016x}",
            fnv1a64_update(0xcbf29ce484222325, git_locator.as_bytes())
        ));

    if !cache_root.join(".git").is_dir() {
        if cache_root.exists() {
            fs::remove_dir_all(&cache_root).map_err(|err| Error::from_io(&cache_root, err))?;
        }
        local_state::ensure_parent_dir(&cache_root)?;
        run_git(
            config_root,
            [
                "clone",
                "--no-checkout",
                git_locator.as_str(),
                &cache_root.to_string_lossy(),
            ],
        )?;
    }

    run_git(
        &cache_root,
        ["remote", "set-url", "origin", git_locator.as_str()],
    )?;
    git_fetch_ref(&cache_root, rev, branch, tag)?;
    git_checkout_ref(&cache_root, rev, branch, tag)?;
    run_git(&cache_root, ["clean", "-ffdqx"])?;
    let resolved_revision = git_head_revision(&cache_root)?;
    Ok(PreparedSource {
        root: cache_root,
        identity: FetchedSource {
            backend: FetchedSourceBackend::GitDependency,
            locator: git_url.to_string(),
            selector: Some(git_selector_from_parts(rev, branch, tag)),
            resolved_revision: Some(resolved_revision),
        },
    })
}

fn prepare_local_git_dependency_root(
    workspace_root: &Path,
    package_name: &str,
    repo_root: &Path,
    rev: Option<&str>,
    branch: Option<&str>,
    tag: Option<&str>,
) -> Result<PreparedSource> {
    let repo_root = repo_root
        .canonicalize()
        .map_err(|err| Error::from_io(repo_root, err))?;
    let cache_root = workspace_root
        .join(".craft")
        .join("git-dependencies")
        .join(sanitize_segment(package_name))
        .join(format!(
            "{:016x}",
            fnv1a64_update(0xcbf29ce484222325, repo_root.to_string_lossy().as_bytes())
        ));

    let head_revision = git_head_revision(&repo_root)?;
    validate_local_git_selector(&repo_root, &head_revision, rev, branch, tag)?;

    if cache_root.exists() {
        fs::remove_dir_all(&cache_root).map_err(|err| Error::from_io(&cache_root, err))?;
    }
    local_state::ensure_parent_dir(&cache_root)?;
    copy_git_worktree(repo_root.as_path(), &cache_root)?;

    Ok(PreparedSource {
        root: cache_root,
        identity: FetchedSource {
            backend: FetchedSourceBackend::GitDependency,
            locator: repo_root.display().to_string(),
            selector: Some(git_selector_from_parts(rev, branch, tag)),
            resolved_revision: Some(head_revision),
        },
    })
}

fn validate_local_git_selector(
    repo_root: &Path,
    head_revision: &str,
    rev: Option<&str>,
    branch: Option<&str>,
    tag: Option<&str>,
) -> Result<()> {
    let selector_revision = if let Some(rev) = rev {
        Some(rev.to_string())
    } else if let Some(branch) = branch {
        Some(git_output(
            repo_root,
            ["rev-parse", &format!("refs/heads/{branch}")],
        )?)
    } else if let Some(tag) = tag {
        Some(git_output(
            repo_root,
            ["rev-parse", &format!("refs/tags/{tag}")],
        )?)
    } else {
        None
    };

    if let Some(selector_revision) = selector_revision
        && selector_revision.trim() != head_revision.trim()
    {
        return Err(Error::Execution(format!(
            "local git dependency `{}` must be checked out at `{}` before it can be used",
            repo_root.display(),
            selector_revision.trim()
        )));
    }

    Ok(())
}

fn copy_git_worktree(source: &Path, dest: &Path) -> Result<()> {
    local_state::ensure_dir(dest)?;
    for entry in fs::read_dir(source).map_err(|err| Error::from_io(source, err))? {
        let entry = entry.map_err(Error::from_io_plain)?;
        let file_type = entry.file_type().map_err(Error::from_io_plain)?;
        let source_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == ".git" || name == ".craft" {
            continue;
        }
        if file_type.is_dir() {
            copy_git_worktree(&source_path, &dest_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &dest_path).map_err(|err| Error::from_io(&dest_path, err))?;
        } else {
            return Err(Error::Execution(format!(
                "unsupported filesystem entry `{}` while copying git worktree",
                source_path.display()
            )));
        }
    }
    Ok(())
}

fn resolve_local_git_repo(config_root: &Path, git_url: &str) -> Option<PathBuf> {
    let direct_path = Path::new(git_url);
    let resolved = if direct_path.is_absolute() {
        Some(direct_path.to_path_buf())
    } else {
        let candidate = config_root.join(direct_path);
        candidate.exists().then_some(candidate)
    };
    let path = resolved?;
    path.join(".git").exists().then_some(path)
}

fn git_fetch_ref(
    repo_root: &Path,
    rev: Option<&str>,
    branch: Option<&str>,
    tag: Option<&str>,
) -> Result<()> {
    if let Some(rev) = rev {
        run_git(repo_root, ["fetch", "--tags", "--force", "origin", rev])
    } else if let Some(branch) = branch {
        run_git(repo_root, ["fetch", "--tags", "--force", "origin", branch])
    } else if let Some(tag) = tag {
        run_git(repo_root, ["fetch", "--tags", "--force", "origin", tag])
    } else {
        run_git(repo_root, ["fetch", "--tags", "--force", "origin"])
    }
}

fn git_checkout_ref(
    repo_root: &Path,
    rev: Option<&str>,
    branch: Option<&str>,
    tag: Option<&str>,
) -> Result<()> {
    let target = if let Some(rev) = rev {
        rev.to_string()
    } else if let Some(branch) = branch {
        format!("origin/{branch}")
    } else if let Some(tag) = tag {
        format!("refs/tags/{tag}")
    } else {
        "FETCH_HEAD".to_string()
    };
    run_git(
        repo_root,
        ["checkout", "--force", "--detach", target.as_str()],
    )
}

fn git_selector_from_parts(
    rev: Option<&str>,
    branch: Option<&str>,
    tag: Option<&str>,
) -> FetchedGitSelector {
    if let Some(rev) = rev {
        FetchedGitSelector::Rev(rev.to_string())
    } else if let Some(branch) = branch {
        FetchedGitSelector::Branch(branch.to_string())
    } else if let Some(tag) = tag {
        FetchedGitSelector::Tag(tag.to_string())
    } else {
        FetchedGitSelector::Default
    }
}

fn run_git<const N: usize>(cwd: &Path, args: [&str; N]) -> Result<()> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|err| Error::Execution(format!("failed to run git: {err}")))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if !stderr.trim().is_empty() {
        stderr.trim().to_string()
    } else if !stdout.trim().is_empty() {
        stdout.trim().to_string()
    } else {
        format!("git exited with status {}", output.status)
    };
    Err(Error::Execution(detail))
}

fn git_output<const N: usize>(cwd: &Path, args: [&str; N]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|err| Error::Execution(format!("failed to run git: {err}")))?;

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if !stderr.trim().is_empty() {
        stderr.trim().to_string()
    } else if !stdout.trim().is_empty() {
        stdout.trim().to_string()
    } else {
        format!("git exited with status {}", output.status)
    };
    Err(Error::Execution(detail))
}

fn git_head_revision(repo_root: &Path) -> Result<String> {
    git_output(repo_root, ["rev-parse", "HEAD"])
}

fn cache_path_for_external(cache_root: &Path, package: &ExternalPackageId) -> Result<PathBuf> {
    match &package.source {
        SourceId::PathDependency { path } => Ok(cache_root
            .join("path")
            .join(sanitize_segment(path))
            .join(package.package_name.as_str())),
        SourceId::GitDependency {
            git,
            rev,
            branch,
            tag,
        } => {
            let selector =
                git_selector_cache_key(rev.as_deref(), branch.as_deref(), tag.as_deref());
            Ok(cache_root
                .join("git")
                .join(format!(
                    "{:016x}",
                    fnv1a64_update(0xcbf29ce484222325, git.as_bytes())
                ))
                .join(selector)
                .join(package.package_name.as_str()))
        }
        SourceId::Root | SourceId::WorkspaceMember { .. } => Err(Error::Usage(
            "cannot materialize local package sources as external sources".to_string(),
        )),
    }
}

fn cache_path_for_resource(cache_root: &Path, package_id: &PackageId, name: &str) -> PathBuf {
    cache_root
        .join(package_cache_segment(package_id))
        .join(sanitize_segment(name))
}

fn materialize_tree(source: &Path, dest: &Path) -> Result<FetchStatus> {
    let source_digest = digest_tree(source)?;
    if dest.is_dir() && digest_tree(dest)? == source_digest {
        return Ok(FetchStatus::Unchanged);
    }

    let existed = dest.exists();
    if dest.is_file() {
        fs::remove_file(dest).map_err(|err| Error::from_io(dest, err))?;
    }
    sync_dir_all(source, dest)?;

    Ok(if existed {
        FetchStatus::Updated
    } else {
        FetchStatus::Created
    })
}

fn sync_dir_all(source: &Path, dest: &Path) -> Result<()> {
    local_state::ensure_dir(dest)?;

    let mut source_names = std::collections::BTreeSet::new();
    for entry in fs::read_dir(source).map_err(|err| Error::from_io(source, err))? {
        let entry = entry.map_err(Error::from_io_plain)?;
        let file_type = entry.file_type().map_err(Error::from_io_plain)?;
        let name = entry.file_name();
        if name == std::ffi::OsStr::new(".git") || name == std::ffi::OsStr::new(".craft") {
            continue;
        }
        let source_path = entry.path();
        let dest_path = dest.join(&name);
        source_names.insert(name.clone());

        if file_type.is_dir() {
            if dest_path.is_file() {
                fs::remove_file(&dest_path).map_err(|err| Error::from_io(&dest_path, err))?;
            }
            sync_dir_all(&source_path, &dest_path)?;
        } else if file_type.is_file() {
            if dest_path.is_dir() {
                fs::remove_dir_all(&dest_path).map_err(|err| Error::from_io(&dest_path, err))?;
            }
            fs::copy(&source_path, &dest_path).map_err(|err| Error::from_io(&dest_path, err))?;
        } else {
            return Err(Error::Execution(format!(
                "unsupported filesystem entry `{}` while syncing source tree",
                source_path.display()
            )));
        }
    }

    for entry in fs::read_dir(dest).map_err(|err| Error::from_io(dest, err))? {
        let entry = entry.map_err(Error::from_io_plain)?;
        let name = entry.file_name();
        if name == std::ffi::OsStr::new(".craft") || source_names.contains(&name) {
            continue;
        }

        let path = entry.path();
        let file_type = entry.file_type().map_err(Error::from_io_plain)?;
        if file_type.is_dir() {
            fs::remove_dir_all(&path).map_err(|err| Error::from_io(&path, err))?;
        } else {
            fs::remove_file(&path).map_err(|err| Error::from_io(&path, err))?;
        }
    }

    Ok(())
}

fn validate_fetched_manifest(root: &Path) -> Result<()> {
    let manifest_path = root.join("Craft.toml");
    if !manifest_path.is_file() {
        return Err(Error::Validation {
            path: root.to_path_buf(),
            message: "fetched package is missing `Craft.toml`".to_string(),
        });
    }

    let manifest = Manifest::load(&manifest_path)?;
    manifest.validate(&manifest_path)
}

fn package_cache_segment(package_id: &PackageId) -> String {
    let mut identity = format!("{}:{}:", package_id.name, package_id.version);
    match &package_id.source {
        SourceId::Root => identity.push_str("root"),
        SourceId::WorkspaceMember { path } => {
            identity.push_str("workspace:");
            identity.push_str(path);
        }
        SourceId::PathDependency { path } => {
            identity.push_str("path:");
            identity.push_str(path);
        }
        SourceId::GitDependency {
            git,
            rev,
            branch,
            tag,
        } => {
            identity.push_str("git:");
            identity.push_str(git);
            identity.push('#');
            identity.push_str(&git_selector_cache_key(
                rev.as_deref(),
                branch.as_deref(),
                tag.as_deref(),
            ));
        }
    }

    format!(
        "{}-{}-{:016x}",
        sanitize_segment(package_id.name.as_str()),
        sanitize_segment(package_id.version.as_str()),
        fnv1a64_update(0xcbf29ce484222325, identity.as_bytes())
    )
}

fn digest_tree(root: &Path) -> Result<u64> {
    let mut entries = Vec::new();
    collect_tree_entries(root, root, &mut entries)?;
    entries.sort();

    let mut hash = 0xcbf29ce484222325u64;
    for entry in entries {
        match entry {
            TreeEntry::Dir(relative) => {
                hash = fnv1a64_update(hash, b"dir:");
                hash = fnv1a64_update(hash, relative.as_bytes());
            }
            TreeEntry::File(relative, bytes) => {
                hash = fnv1a64_update(hash, b"file:");
                hash = fnv1a64_update(hash, relative.as_bytes());
                hash = fnv1a64_update(hash, &bytes);
            }
        }
    }
    Ok(hash)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum TreeEntry {
    Dir(String),
    File(String, Vec<u8>),
}

fn collect_tree_entries(root: &Path, current: &Path, entries: &mut Vec<TreeEntry>) -> Result<()> {
    for entry in fs::read_dir(current).map_err(|err| Error::from_io(current, err))? {
        let entry = entry.map_err(Error::from_io_plain)?;
        if entry.file_name() == std::ffi::OsStr::new(".git")
            || entry.file_name() == std::ffi::OsStr::new(".craft")
        {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type().map_err(Error::from_io_plain)?;
        if file_type.is_dir() {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            entries.push(TreeEntry::Dir(relative));
            collect_tree_entries(root, &path, entries)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = fs::read(&path).map_err(|err| Error::from_io(&path, err))?;
            entries.push(TreeEntry::File(relative, bytes));
        } else {
            return Err(Error::Execution(format!(
                "unsupported filesystem entry `{}` while hashing source tree",
                path.display()
            )));
        }
    }
    Ok(())
}

fn fnv1a64_update(mut hash: u64, bytes: &[u8]) -> u64 {
    const PRIME: u64 = 0x100000001b3;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

fn sanitize_segment(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' => '_',
            _ => ch,
        })
        .collect()
}

fn git_selector_cache_key(rev: Option<&str>, branch: Option<&str>, tag: Option<&str>) -> String {
    if let Some(rev) = rev {
        return format!(
            "rev-{:016x}",
            fnv1a64_update(0xcbf29ce484222325, rev.as_bytes())
        );
    }
    if let Some(branch) = branch {
        return format!(
            "branch-{:016x}",
            fnv1a64_update(0xcbf29ce484222325, branch.as_bytes())
        );
    }
    if let Some(tag) = tag {
        return format!(
            "tag-{:016x}",
            fnv1a64_update(0xcbf29ce484222325, tag.as_bytes())
        );
    }
    "default".to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        FetchStatus, FetchedGitSelector, FetchedSourceBackend, fetch_external_packages,
        fetch_package_resources, summarize_fetch, summarize_fetch_resources,
    };
    use crate::elaborate::{FeatureSelection, plan};
    use crate::manifest::Manifest;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
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
    fn fetches_path_dependencies() {
        let root = temp_dir("craft-fetch-path");
        let package_root = root.join("vendor").join("log");
        fs::create_dir_all(package_root.join("src")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.0"

[dependencies]
log = { path = "vendor/log", version = "1" }
"#,
        )
        .unwrap();
        fs::write(
            package_root.join("Craft.toml"),
            r#"
[package]
name = "log"
version = "1"
kern = "0.7.0"

[lib]
root = "src/lib.rn"
"#,
        )
        .unwrap();
        fs::write(
            package_root.join("src/lib.rn"),
            "pub fn x() i32 { return 0; }\n",
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Lock,
            &FeatureSelection::default(),
        )
        .unwrap();

        let fetched = fetch_external_packages(&elaboration.resolved_graph).unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].status, FetchStatus::Created);
        assert_eq!(
            fetched[0].source.backend,
            FetchedSourceBackend::PathDependency
        );
        assert_eq!(fetched[0].source.selector, None);
        assert_eq!(fetched[0].source.resolved_revision, None);
        assert!(fetched[0].cache_path.join("Craft.toml").is_file());
        assert_eq!(summarize_fetch(&fetched).created, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fetches_package_resources() {
        let root = temp_dir("craft-fetch-resource");
        let resource_root = root.join("vendor").join("limine");
        fs::create_dir_all(resource_root.join("cfg")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "kernel"
version = "0.1.0"
kern = "0.7.0"

[[bin]]
name = "kernel"
root = "src/main.rn"

[resources]
limine = { path = "vendor/limine" }
"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
        fs::write(resource_root.join("cfg").join("limine.conf"), "TIMEOUT=0\n").unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Fetch,
            &FeatureSelection::default(),
        )
        .unwrap();

        let fetched = fetch_package_resources(&elaboration).unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].id.name, "limine");
        assert_eq!(fetched[0].status, FetchStatus::Created);
        assert_eq!(
            fetched[0].source.backend,
            FetchedSourceBackend::PathDependency
        );
        assert!(fetched[0].cache_path.join("cfg/limine.conf").is_file());
        assert_eq!(summarize_fetch_resources(&fetched).created, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn refetch_updates_resources_when_source_adds_empty_directory() {
        let root = temp_dir("craft-fetch-resource-empty-dir");
        let resource_root = root.join("vendor").join("limine");
        fs::create_dir_all(resource_root.join("cfg")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.0"

[[bin]]
name = "app"
root = "src/main.rn"

[resources]
limine = { path = "vendor/limine" }
"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
        fs::write(resource_root.join("cfg").join("limine.conf"), "TIMEOUT=0\n").unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Build,
            &FeatureSelection::default(),
        )
        .unwrap();

        let fetched = fetch_package_resources(&elaboration).unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].status, FetchStatus::Created);

        fs::create_dir_all(resource_root.join("EFI").join("BOOT")).unwrap();

        let refetched = fetch_package_resources(&elaboration).unwrap();
        assert_eq!(refetched[0].status, FetchStatus::Updated);
        assert!(refetched[0].cache_path.join("EFI").join("BOOT").is_dir());

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn fetch_package_resources_rejects_symlink_entries() {
        use std::os::unix::fs::symlink;

        let root = temp_dir("craft-fetch-resource-symlink");
        let resource_root = root.join("vendor").join("limine");
        fs::create_dir_all(resource_root.join("cfg")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.0"

[[bin]]
name = "app"
root = "src/main.rn"

[resources]
limine = { path = "vendor/limine" }
"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
        fs::write(resource_root.join("cfg").join("limine.conf"), "TIMEOUT=0\n").unwrap();
        symlink(
            resource_root.join("cfg").join("limine.conf"),
            resource_root.join("cfg").join("limine-link.conf"),
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Build,
            &FeatureSelection::default(),
        )
        .unwrap();

        let err = fetch_package_resources(&elaboration).unwrap_err();
        assert!(err.to_string().contains("unsupported filesystem entry"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn refetch_preserves_cached_local_craft_state() {
        let root = temp_dir("craft-fetch-preserve-craft-state");
        let package_root = root.join("vendor").join("log");
        fs::create_dir_all(package_root.join("src")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.0"

[dependencies]
log = { path = "vendor/log", version = "1" }
"#,
        )
        .unwrap();
        fs::write(
            package_root.join("Craft.toml"),
            r#"
[package]
name = "log"
version = "1"
kern = "0.7.0"

[lib]
root = "src/lib.rn"
"#,
        )
        .unwrap();
        fs::write(
            package_root.join("src/lib.rn"),
            "pub fn x() i32 { return 0; }\n",
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Lock,
            &FeatureSelection::default(),
        )
        .unwrap();

        let fetched = fetch_external_packages(&elaboration.resolved_graph).unwrap();
        let cache_path = fetched[0].cache_path.clone();
        let preserved = cache_path.join(".craft/build/release/obj/log.o");
        fs::create_dir_all(preserved.parent().unwrap()).unwrap();
        fs::write(&preserved, b"thinlto-cache").unwrap();

        let refetched = fetch_external_packages(&elaboration.resolved_graph).unwrap();
        assert_eq!(refetched[0].status, FetchStatus::Unchanged);
        assert_eq!(fs::read(&preserved).unwrap(), b"thinlto-cache");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fetches_and_updates_git_dependencies() {
        let root = temp_dir("craft-fetch-git");
        let repo = root.join("log.git");
        init_git_package(&repo, "pub fn x() i32 { return 0; }\n");

        fs::write(
            root.join("Craft.toml"),
            format!(
                r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.0"

[dependencies]
log = {{ git = "{}", branch = "main", version = "1" }}
"#,
                toml_string_literal(&repo)
            ),
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Lock,
            &FeatureSelection::default(),
        )
        .unwrap();

        let fetched = fetch_external_packages(&elaboration.resolved_graph).unwrap();
        assert_eq!(fetched[0].status, FetchStatus::Created);
        assert_eq!(
            fetched[0].source.backend,
            FetchedSourceBackend::GitDependency
        );
        assert_eq!(
            fetched[0].source.selector,
            Some(FetchedGitSelector::Branch("main".to_string()))
        );
        assert_eq!(
            fetched[0].source.resolved_revision.as_deref(),
            Some(git_head(&repo).as_str())
        );

        commit_git_package(&repo, "pub fn x() i32 { return 1; }\n");

        let fetched = fetch_external_packages(&elaboration.resolved_graph).unwrap();
        assert_eq!(fetched[0].status, FetchStatus::Updated);
        assert_eq!(
            fetched[0].source.resolved_revision.as_deref(),
            Some(git_head(&repo).as_str())
        );
        assert_eq!(
            normalized_text_file(&fetched[0].cache_path.join("src/lib.rn")),
            "pub fn x() i32 { return 1; }\n"
        );
    }

    #[test]
    fn fetches_git_resources_without_materializing_git_metadata() {
        let root = temp_dir("craft-fetch-git-resource");
        let repo = root.join("limine.git");
        init_git_package(&repo, "pub fn x() i32 { return 0; }\n");
        fs::write(repo.join("resource.txt"), "limine\n").unwrap();
        run_git(&repo, ["add", "."]).unwrap();
        run_git(&repo, ["commit", "-m", "resource"]).unwrap();

        fs::write(
            root.join("Craft.toml"),
            format!(
                r#"
[package]
name = "kernel"
version = "0.1.0"
kern = "0.7.0"

[[bin]]
name = "kernel"
root = "src/main.rn"

[resources]
limine = {{ git = "{}", branch = "main" }}
"#,
                toml_string_literal(&repo)
            ),
        )
        .unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Fetch,
            &FeatureSelection::default(),
        )
        .unwrap();

        let fetched = fetch_package_resources(&elaboration).unwrap();
        assert_eq!(fetched.len(), 1);
        assert!(fetched[0].cache_path.join("resource.txt").is_file());
        assert!(!fetched[0].cache_path.join(".git").exists());

        let _ = fs::remove_dir_all(root);
    }

    fn init_git_package(repo: &PathBuf, lib_source: &str) {
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(
            repo.join("Craft.toml"),
            r#"
[package]
name = "log"
version = "1"
kern = "0.7.0"

[lib]
root = "src/lib.rn"
"#,
        )
        .unwrap();
        fs::write(repo.join("src/lib.rn"), lib_source).unwrap();
        run_git(repo, ["init", "--initial-branch=main"]).unwrap();
        run_git(repo, ["config", "user.name", "Craft Tests"]).unwrap();
        run_git(
            repo,
            ["config", "user.email", "craft-tests@example.invalid"],
        )
        .unwrap();
        run_git(repo, ["add", "."]).unwrap();
        run_git(repo, ["commit", "-m", "initial"]).unwrap();
    }

    fn toml_string_literal(path: &std::path::Path) -> String {
        path.to_string_lossy().replace('\\', "\\\\")
    }

    fn normalized_text_file(path: &std::path::Path) -> String {
        fs::read_to_string(path).unwrap().replace("\r\n", "\n")
    }

    fn commit_git_package(repo: &PathBuf, lib_source: &str) {
        fs::write(repo.join("src/lib.rn"), lib_source).unwrap();
        run_git(repo, ["add", "."]).unwrap();
        run_git(repo, ["commit", "-m", "update"]).unwrap();
    }

    fn git_head(repo: &PathBuf) -> String {
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn run_git<const N: usize>(cwd: &PathBuf, args: [&str; N]) -> Result<(), String> {
        let output = Command::new("git")
            .args(["-c", "commit.gpgsign=false"])
            .args(["-c", "tag.gpgSign=false"])
            .args(args)
            .current_dir(cwd)
            .output()
            .map_err(|err| err.to_string())?;
        if output.status.success() {
            return Ok(());
        }
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}
