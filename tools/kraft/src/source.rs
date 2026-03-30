use crate::error::{Error, Result};
use crate::graph::SourceId;
use crate::manifest::{Manifest, SourceConfig};
use crate::resolver::{ExternalPackageId, ResolvedGraph};
use std::fs;
use std::path::{Path, PathBuf};

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchStatus {
    Created,
    Updated,
    Unchanged,
}

pub fn fetch_external_packages(
    manifest_path: &Path,
    manifest: &Manifest,
    resolved: &ResolvedGraph,
) -> Result<Vec<FetchedPackage>> {
    let config_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let cache_root = resolved.workspace_root.join(".kraft").join("sources");
    let mut packages = Vec::new();

    for package in &resolved.external_packages {
        let source_path = source_path_for_external(config_root, manifest, resolved, &package.id)?;
        let cache_path = cache_path_for_external(&cache_root, &package.id)?;
        let status = materialize_tree(&source_path, &cache_path)?;
        validate_fetched_manifest(&cache_path)?;
        packages.push(FetchedPackage {
            id: package.id.clone(),
            source_path,
            cache_path,
            status,
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
    config_root: &Path,
    manifest: &Manifest,
    resolved: &ResolvedGraph,
    package: &ExternalPackageId,
) -> Result<PathBuf> {
    match &package.source {
        SourceId::PathDependency { path } => {
            let absolute = resolved.workspace_root.join(path);
            absolute
                .canonicalize()
                .map_err(|err| Error::from_io(&absolute, err))
        }
        SourceId::Registry { name } => {
            let source_name = name.as_deref().unwrap_or("default");
            let source = manifest
                .sources
                .get(source_name)
                .ok_or_else(|| Error::Validation {
                    path: config_root.join("Kraft.toml"),
                    message: format!(
                        "external package `{}` requires `[source.{source_name}]`",
                        package.package_name
                    ),
                })?;
            registry_source_path(config_root, source_name, source, package)
        }
        SourceId::Root | SourceId::WorkspaceMember { .. } => Err(Error::Validation {
            path: config_root.join("Kraft.toml"),
            message: format!(
                "unsupported external source kind for `{}`",
                package.package_name
            ),
        }),
    }
}

fn registry_source_path(
    config_root: &Path,
    source_name: &str,
    source: &SourceConfig,
    package: &ExternalPackageId,
) -> Result<PathBuf> {
    let Some(directory) = &source.directory else {
        return Err(Error::Validation {
            path: config_root.join("Kraft.toml"),
            message: format!("[source.{source_name}] must declare `directory`"),
        });
    };
    let Some(version) = &package.version else {
        return Err(Error::Validation {
            path: config_root.join("Kraft.toml"),
            message: format!(
                "registry package `{}` must resolve to an explicit version before fetch",
                package.package_name
            ),
        });
    };

    let root = config_root.join(directory);
    let package_root = root.join(&package.package_name).join(version);
    package_root
        .canonicalize()
        .map_err(|err| Error::from_io(&package_root, err))
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
    let manifest_path = root.join("Kraft.toml");
    if !manifest_path.is_file() {
        return Err(Error::Validation {
            path: root.to_path_buf(),
            message: "fetched package is missing `Kraft.toml`".to_string(),
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
    use super::{FetchStatus, fetch_external_packages, summarize_fetch};
    use crate::elaborate::{FeatureSelection, plan};
    use crate::manifest::Manifest;
    use std::fs;
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

    #[test]
    fn fetches_registry_packages_from_directory_sources() {
        let root = temp_dir("kraft-fetch-registry");
        let registry_root = root.join("vendor-registry");
        let package_root = registry_root.join("log").join("1");
        fs::create_dir_all(package_root.join("src")).unwrap();
        fs::write(
            root.join("Kraft.toml"),
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
            package_root.join("Kraft.toml"),
            r#"
[package]
name = "log"
version = "1"
kern = "0.7"

[lib]
root = "src/lib.kr"
"#,
        )
        .unwrap();
        fs::write(
            package_root.join("src/lib.kr"),
            "pub fn x() i32 { return 0; }\n",
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
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
        assert!(fetched[0].cache_path.join("Kraft.toml").is_file());
        let summary = summarize_fetch(&fetched);
        assert_eq!(summary.created, 1);

        let _ = fs::remove_dir_all(root);
    }
}
