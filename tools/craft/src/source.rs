use crate::error::{Error, Result};
use crate::graph::SourceId;
use crate::manifest::{Manifest, SourceConfig};
use crate::resolver::{ExternalPackageId, ResolvedGraph};
use std::collections::BTreeMap;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchStatus {
    Created,
    Updated,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedSource {
    pub backend: FetchedSourceBackend,
    pub source_name: Option<String>,
    pub locator: String,
    pub selector: Option<FetchedGitSelector>,
    pub resolved_revision: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchedSourceBackend {
    PathDependency,
    DirectoryRegistry,
    GitRegistry,
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
            Self::DirectoryRegistry => "directory",
            Self::GitRegistry => "git",
        }
    }
}

impl FetchedGitSelector {
    pub fn label(&self) -> String {
        match self {
            Self::Default => "default".to_string(),
            Self::Rev(rev) => format!("rev:{rev}"),
            Self::Branch(branch) => format!("branch:{branch}"),
            Self::Tag(tag) => format!("tag:{tag}"),
        }
    }
}

impl FetchedSource {
    pub fn selector_label(&self) -> Option<String> {
        self.selector.as_ref().map(FetchedGitSelector::label)
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

#[derive(Debug, Clone, Copy)]
pub struct SourceLookup<'a> {
    pub manifest_path: &'a Path,
    pub sources: &'a BTreeMap<String, SourceConfig>,
}

pub fn source_backend(source: &SourceConfig) -> &'static str {
    if source.directory.is_some() {
        "directory"
    } else if source.git.is_some() {
        "git"
    } else {
        "missing"
    }
}

pub fn source_locator(source: &SourceConfig) -> Option<String> {
    source.directory.clone().or_else(|| source.git.clone())
}

pub fn source_selector(source: &SourceConfig) -> Option<String> {
    if let Some(rev) = &source.rev {
        Some(format!("rev:{rev}"))
    } else if let Some(branch) = &source.branch {
        Some(format!("branch:{branch}"))
    } else if let Some(tag) = &source.tag {
        Some(format!("tag:{tag}"))
    } else if source.git.is_some() {
        Some("default".to_string())
    } else {
        None
    }
}

pub fn is_insecure_source_locator(locator: &str) -> bool {
    locator.starts_with("http://") || locator.starts_with("git://")
}

pub fn fetch_external_packages(
    manifest_path: &Path,
    manifest: &Manifest,
    resolved: &ResolvedGraph,
) -> Result<Vec<FetchedPackage>> {
    fetch_external_packages_with_lookup(
        &[SourceLookup {
            manifest_path,
            sources: &manifest.sources,
        }],
        resolved,
    )
}

pub fn fetch_external_packages_with_lookup(
    lookup_chain: &[SourceLookup<'_>],
    resolved: &ResolvedGraph,
) -> Result<Vec<FetchedPackage>> {
    let primary_manifest_path = lookup_chain
        .first()
        .map(|entry| entry.manifest_path)
        .unwrap_or_else(|| Path::new("Craft.toml"));
    let cache_root = resolved.workspace_root.join(".craft").join("sources");
    let mut packages = Vec::new();
    let mut prepared_sources = BTreeMap::new();

    for package in &resolved.external_packages {
        let resolved_source = source_path_for_external(
            primary_manifest_path,
            lookup_chain,
            resolved,
            &package.id,
            &mut prepared_sources,
        )?;
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

fn source_path_for_external(
    primary_manifest_path: &Path,
    lookup_chain: &[SourceLookup<'_>],
    resolved: &ResolvedGraph,
    package: &ExternalPackageId,
    prepared_sources: &mut BTreeMap<String, PreparedSource>,
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
                    source_name: None,
                    locator: source_path.display().to_string(),
                    selector: None,
                    resolved_revision: None,
                },
                source_path,
            })
        }
        SourceId::Registry { name } => {
            let source_name = name.as_deref().unwrap_or("default");
            let (config_root, source) = lookup_chain
                .iter()
                .find_map(|entry| {
                    entry.sources.get(source_name).map(|source| {
                        (
                            entry
                                .manifest_path
                                .parent()
                                .unwrap_or_else(|| Path::new(".")),
                            source,
                        )
                    })
                })
                .ok_or_else(|| Error::Validation {
                    path: primary_manifest_path.to_path_buf(),
                    message: format!(
                        "external package `{}` requires `[source.{source_name}]`",
                        package.package_name
                    ),
                })?;
            let prepared = prepared_named_source_root(
                config_root,
                &resolved.workspace_root,
                source_name,
                source,
                prepared_sources,
            )?;
            let source_path = named_source_package_path(config_root, &prepared.root, package)?;
            Ok(ResolvedSourcePath {
                source_path,
                identity: prepared.identity,
            })
        }
        SourceId::Root | SourceId::WorkspaceMember { .. } => Err(Error::Validation {
            path: primary_manifest_path.to_path_buf(),
            message: format!(
                "unsupported external source kind for `{}`",
                package.package_name
            ),
        }),
    }
}

fn prepared_named_source_root(
    config_root: &Path,
    workspace_root: &Path,
    source_name: &str,
    source: &SourceConfig,
    prepared_sources: &mut BTreeMap<String, PreparedSource>,
) -> Result<PreparedSource> {
    let key = format!(
        "{source_name}:{}:{}:{}:{}:{}:{}",
        config_root.display(),
        source.directory.as_deref().unwrap_or(""),
        source.git.as_deref().unwrap_or(""),
        source.rev.as_deref().unwrap_or(""),
        source.branch.as_deref().unwrap_or(""),
        source.tag.as_deref().unwrap_or("")
    );
    if let Some(root) = prepared_sources.get(&key) {
        return Ok(root.clone());
    }

    let prepared = if let Some(directory) = &source.directory {
        let root = config_root.join(directory);
        PreparedSource {
            root,
            identity: FetchedSource {
                backend: FetchedSourceBackend::DirectoryRegistry,
                source_name: Some(source_name.to_string()),
                locator: source_locator(source).unwrap_or_else(|| directory.clone()),
                selector: None,
                resolved_revision: None,
            },
        }
    } else if source.git.is_some() {
        prepare_git_source_root(config_root, workspace_root, source_name, source)?
    } else {
        return Err(Error::Validation {
            path: config_root.join("Craft.toml"),
            message: format!("[source.{source_name}] must declare either `directory` or `git`"),
        });
    };

    let canonical = prepared
        .root
        .canonicalize()
        .map_err(|err| Error::from_io(&prepared.root, err))?;
    let prepared = PreparedSource {
        root: canonical.clone(),
        identity: FetchedSource {
            locator: canonical.display().to_string(),
            ..prepared.identity
        },
    };
    prepared_sources.insert(key, prepared.clone());
    Ok(prepared)
}

fn named_source_package_path(
    config_root: &Path,
    source_root: &Path,
    package: &ExternalPackageId,
) -> Result<PathBuf> {
    let Some(version) = &package.version else {
        return Err(Error::Validation {
            path: config_root.join("Craft.toml"),
            message: format!(
                "registry package `{}` must resolve to an explicit version before fetch",
                package.package_name
            ),
        });
    };

    let package_root = source_root.join(&package.package_name).join(version);
    package_root
        .canonicalize()
        .map_err(|err| Error::from_io(&package_root, err))
}

fn prepare_git_source_root(
    config_root: &Path,
    workspace_root: &Path,
    source_name: &str,
    source: &SourceConfig,
) -> Result<PreparedSource> {
    let git_url = source.git.as_deref().ok_or_else(|| Error::Validation {
        path: config_root.join("Craft.toml"),
        message: format!("[source.{source_name}] must declare `git`"),
    })?;
    let cache_root = workspace_root
        .join(".craft")
        .join("git-sources")
        .join(source_name)
        .join(format!(
            "{:016x}",
            fnv1a64_update(0xcbf29ce484222325, git_url.as_bytes())
        ));

    if !cache_root.join(".git").is_dir() {
        if cache_root.exists() {
            fs::remove_dir_all(&cache_root).map_err(|err| Error::from_io(&cache_root, err))?;
        }
        if let Some(parent) = cache_root.parent() {
            fs::create_dir_all(parent).map_err(|err| Error::from_io(parent, err))?;
        }
        run_git(
            config_root,
            [
                "clone",
                "--no-checkout",
                git_url,
                &cache_root.to_string_lossy(),
            ],
        )?;
    }

    run_git(&cache_root, ["remote", "set-url", "origin", git_url])?;
    git_fetch(&cache_root, source)?;
    git_checkout_selected_ref(&cache_root, source)?;
    run_git(&cache_root, ["clean", "-ffdqx"])?;
    let resolved_revision = git_head_revision(&cache_root)?;
    Ok(PreparedSource {
        root: cache_root,
        identity: FetchedSource {
            backend: FetchedSourceBackend::GitRegistry,
            source_name: Some(source_name.to_string()),
            locator: source_locator(source).unwrap_or_else(|| git_url.to_string()),
            selector: Some(git_selector(source)),
            resolved_revision: Some(resolved_revision),
        },
    })
}

fn git_fetch(repo_root: &Path, source: &SourceConfig) -> Result<()> {
    if let Some(rev) = &source.rev {
        run_git(repo_root, ["fetch", "--tags", "--force", "origin", rev])
    } else if let Some(branch) = &source.branch {
        run_git(
            repo_root,
            ["fetch", "--tags", "--force", "origin", branch.as_str()],
        )
    } else if let Some(tag) = &source.tag {
        run_git(
            repo_root,
            ["fetch", "--tags", "--force", "origin", tag.as_str()],
        )
    } else {
        run_git(repo_root, ["fetch", "--tags", "--force", "origin"])
    }
}

fn git_checkout_selected_ref(repo_root: &Path, source: &SourceConfig) -> Result<()> {
    let target = if let Some(rev) = &source.rev {
        rev.clone()
    } else if let Some(branch) = &source.branch {
        format!("origin/{branch}")
    } else if let Some(tag) = &source.tag {
        format!("refs/tags/{tag}")
    } else {
        "FETCH_HEAD".to_string()
    };
    run_git(
        repo_root,
        ["checkout", "--force", "--detach", target.as_str()],
    )
}

fn git_selector(source: &SourceConfig) -> FetchedGitSelector {
    if let Some(rev) = &source.rev {
        FetchedGitSelector::Rev(rev.clone())
    } else if let Some(branch) = &source.branch {
        FetchedGitSelector::Branch(branch.clone())
    } else if let Some(tag) = &source.tag {
        FetchedGitSelector::Tag(tag.clone())
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
        SourceId::Registry { name } => Ok(cache_root
            .join("registry")
            .join(name.as_deref().unwrap_or("default"))
            .join(package.package_name.as_str())
            .join(package.version.as_deref().ok_or_else(|| {
                Error::Usage("registry fetch requires a concrete version".to_string())
            })?)),
        SourceId::Root | SourceId::WorkspaceMember { .. } => Err(Error::Usage(
            "cannot materialize local package sources as external sources".to_string(),
        )),
    }
}

fn materialize_tree(source: &Path, dest: &Path) -> Result<FetchStatus> {
    let source_digest = digest_tree(source)?;
    if dest.is_dir() && digest_tree(dest)? == source_digest {
        return Ok(FetchStatus::Unchanged);
    }

    let existed = dest.exists();
    if existed {
        fs::remove_dir_all(dest).map_err(|err| Error::from_io(dest, err))?;
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|err| Error::from_io(parent, err))?;
    }
    copy_dir_all(source, dest)?;

    Ok(if existed {
        FetchStatus::Updated
    } else {
        FetchStatus::Created
    })
}

fn copy_dir_all(source: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).map_err(|err| Error::from_io(dest, err))?;
    for entry in fs::read_dir(source).map_err(|err| Error::from_io(source, err))? {
        let entry = entry.map_err(Error::from_io_plain)?;
        let file_type = entry.file_type().map_err(Error::from_io_plain)?;
        let source_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&source_path, &dest_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &dest_path).map_err(|err| Error::from_io(&dest_path, err))?;
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

fn digest_tree(root: &Path) -> Result<u64> {
    let mut entries = Vec::new();
    collect_tree_entries(root, root, &mut entries)?;
    entries.sort_by(|lhs, rhs| lhs.0.cmp(&rhs.0));

    let mut hash = 0xcbf29ce484222325u64;
    for (relative, bytes) in entries {
        hash = fnv1a64_update(hash, relative.as_bytes());
        hash = fnv1a64_update(hash, &bytes);
    }
    Ok(hash)
}

fn collect_tree_entries(
    root: &Path,
    current: &Path,
    entries: &mut Vec<(String, Vec<u8>)>,
) -> Result<()> {
    for entry in fs::read_dir(current).map_err(|err| Error::from_io(current, err))? {
        let entry = entry.map_err(Error::from_io_plain)?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(Error::from_io_plain)?;
        if file_type.is_dir() {
            collect_tree_entries(root, &path, entries)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = fs::read(&path).map_err(|err| Error::from_io(&path, err))?;
            entries.push((relative, bytes));
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

#[cfg(test)]
mod tests {
    use super::{
        FetchStatus, FetchedGitSelector, FetchedSourceBackend, fetch_external_packages,
        summarize_fetch,
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
    fn fetches_registry_packages_from_directory_sources() {
        let root = temp_dir("craft-fetch-registry");
        let registry_root = root.join("vendor-registry");
        let package_root = registry_root.join("log").join("1");
        fs::create_dir_all(package_root.join("src")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[dependencies]
log = "1"

[source.default]
directory = "vendor-registry"
"#,
        )
        .unwrap();
        fs::write(
            package_root.join("Craft.toml"),
            r#"
[package]
name = "log"
version = "1"
kern = "0.7"

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

        let fetched =
            fetch_external_packages(&manifest_path, &manifest, &elaboration.resolved_graph)
                .unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].status, FetchStatus::Created);
        assert_eq!(
            fetched[0].source.backend,
            FetchedSourceBackend::DirectoryRegistry
        );
        assert_eq!(fetched[0].source.source_name.as_deref(), Some("default"));
        assert_eq!(fetched[0].source.selector, None);
        assert_eq!(fetched[0].source.resolved_revision, None);
        assert!(fetched[0].cache_path.join("Craft.toml").is_file());
        let summary = summarize_fetch(&fetched);
        assert_eq!(summary.created, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fetches_registry_packages_from_git_sources() {
        let root = temp_dir("craft-fetch-git-registry");
        let registry_repo = root.join("registry.git");
        init_git_registry(
            &registry_repo,
            &[
                (
                    "log/1/Craft.toml",
                    r#"
[package]
name = "log"
version = "1"
kern = "0.7"

[lib]
root = "src/lib.rn"
"#,
                ),
                ("log/1/src/lib.rn", "pub fn x() i32 { return 0; }\n"),
            ],
        );

        fs::write(
            root.join("Craft.toml"),
            format!(
                r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[dependencies]
log = "1"

[source.default]
git = "{}"
"#,
                registry_repo.display()
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

        let fetched =
            fetch_external_packages(&manifest_path, &manifest, &elaboration.resolved_graph)
                .unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].status, FetchStatus::Created);
        assert_eq!(fetched[0].source.backend, FetchedSourceBackend::GitRegistry);
        assert_eq!(fetched[0].source.source_name.as_deref(), Some("default"));
        assert_eq!(
            fetched[0].source.selector,
            Some(FetchedGitSelector::Default)
        );
        assert_eq!(
            fetched[0].source.resolved_revision.as_deref(),
            Some(git_head(&registry_repo).as_str())
        );
        assert!(fetched[0].cache_path.join("Craft.toml").is_file());
        assert_eq!(
            fs::read_to_string(fetched[0].cache_path.join("src/lib.rn")).unwrap(),
            "pub fn x() i32 { return 0; }\n"
        );
    }

    #[test]
    fn updates_git_source_cache_when_registry_revision_changes() {
        let root = temp_dir("craft-fetch-git-update");
        let registry_repo = root.join("registry.git");
        init_git_registry(
            &registry_repo,
            &[
                (
                    "log/1/Craft.toml",
                    r#"
[package]
name = "log"
version = "1"
kern = "0.7"

[lib]
root = "src/lib.rn"
"#,
                ),
                ("log/1/src/lib.rn", "pub fn x() i32 { return 0; }\n"),
            ],
        );

        fs::write(
            root.join("Craft.toml"),
            format!(
                r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[dependencies]
log = "1"

[source.default]
git = "{}"
branch = "main"
"#,
                registry_repo.display()
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

        let fetched =
            fetch_external_packages(&manifest_path, &manifest, &elaboration.resolved_graph)
                .unwrap();
        assert_eq!(fetched[0].status, FetchStatus::Created);
        assert_eq!(
            fetched[0].source.selector,
            Some(FetchedGitSelector::Branch("main".to_string()))
        );
        assert_eq!(
            fetched[0].source.resolved_revision.as_deref(),
            Some(git_head(&registry_repo).as_str())
        );

        commit_git_registry(
            &registry_repo,
            &[("log/1/src/lib.rn", "pub fn x() i32 { return 1; }\n")],
        );

        let fetched =
            fetch_external_packages(&manifest_path, &manifest, &elaboration.resolved_graph)
                .unwrap();
        assert_eq!(fetched[0].status, FetchStatus::Updated);
        assert_eq!(
            fetched[0].source.resolved_revision.as_deref(),
            Some(git_head(&registry_repo).as_str())
        );
        assert_eq!(
            fs::read_to_string(fetched[0].cache_path.join("src/lib.rn")).unwrap(),
            "pub fn x() i32 { return 1; }\n"
        );
    }

    #[test]
    fn fetches_registry_packages_from_git_tag_selector() {
        let root = temp_dir("craft-fetch-git-tag");
        let registry_repo = root.join("registry.git");
        init_git_registry(
            &registry_repo,
            &[
                (
                    "log/1/Craft.toml",
                    r#"
[package]
name = "log"
version = "1"
kern = "0.7"

[lib]
root = "src/lib.rn"
"#,
                ),
                ("log/1/src/lib.rn", "pub fn x() i32 { return 0; }\n"),
            ],
        );
        let tagged_revision = git_head(&registry_repo);
        run_git(&registry_repo, ["tag", "v1"]).unwrap();
        commit_git_registry(
            &registry_repo,
            &[("log/1/src/lib.rn", "pub fn x() i32 { return 1; }\n")],
        );

        fs::write(
            root.join("Craft.toml"),
            format!(
                r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[dependencies]
log = "1"

[source.default]
git = "{}"
tag = "v1"
"#,
                registry_repo.display()
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

        let fetched =
            fetch_external_packages(&manifest_path, &manifest, &elaboration.resolved_graph)
                .unwrap();
        assert_eq!(fetched[0].status, FetchStatus::Created);
        assert_eq!(
            fetched[0].source.selector,
            Some(FetchedGitSelector::Tag("v1".to_string()))
        );
        assert_eq!(
            fetched[0].source.resolved_revision.as_deref(),
            Some(tagged_revision.as_str())
        );
        assert_eq!(
            fs::read_to_string(fetched[0].cache_path.join("src/lib.rn")).unwrap(),
            "pub fn x() i32 { return 0; }\n"
        );
    }

    #[test]
    fn fetches_registry_packages_from_exact_git_revision() {
        let root = temp_dir("craft-fetch-git-rev");
        let registry_repo = root.join("registry.git");
        init_git_registry(
            &registry_repo,
            &[
                (
                    "log/1/Craft.toml",
                    r#"
[package]
name = "log"
version = "1"
kern = "0.7"

[lib]
root = "src/lib.rn"
"#,
                ),
                ("log/1/src/lib.rn", "pub fn x() i32 { return 0; }\n"),
            ],
        );
        let pinned_revision = git_head(&registry_repo);
        commit_git_registry(
            &registry_repo,
            &[("log/1/src/lib.rn", "pub fn x() i32 { return 1; }\n")],
        );

        fs::write(
            root.join("Craft.toml"),
            format!(
                r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[dependencies]
log = "1"

[source.default]
git = "{}"
rev = "{}"
"#,
                registry_repo.display(),
                pinned_revision
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

        let fetched =
            fetch_external_packages(&manifest_path, &manifest, &elaboration.resolved_graph)
                .unwrap();
        assert_eq!(fetched[0].status, FetchStatus::Created);
        assert_eq!(
            fetched[0].source.selector,
            Some(FetchedGitSelector::Rev(pinned_revision.clone()))
        );
        assert_eq!(
            fetched[0].source.resolved_revision.as_deref(),
            Some(pinned_revision.as_str())
        );
        assert_eq!(
            fs::read_to_string(fetched[0].cache_path.join("src/lib.rn")).unwrap(),
            "pub fn x() i32 { return 0; }\n"
        );
    }

    fn init_git_registry(repo: &PathBuf, files: &[(&str, &str)]) {
        fs::create_dir_all(repo).unwrap();
        run_git(repo, ["init", "--initial-branch=main"]).unwrap();
        run_git(repo, ["config", "user.name", "Craft Tests"]).unwrap();
        run_git(
            repo,
            ["config", "user.email", "craft-tests@example.invalid"],
        )
        .unwrap();
        for (relative, contents) in files {
            let path = repo.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, contents).unwrap();
        }
        run_git(repo, ["add", "."]).unwrap();
        run_git(repo, ["commit", "-m", "initial"]).unwrap();
    }

    fn commit_git_registry(repo: &PathBuf, files: &[(&str, &str)]) {
        for (relative, contents) in files {
            fs::write(repo.join(relative), contents).unwrap();
        }
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
