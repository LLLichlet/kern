use crate::error::{Error, Result};
use crate::graph::SourceId;
use crate::manifest::Manifest;
use crate::resolver::ExternalPackageId;
use crate::source::{FetchedSource, FetchedSourceBackend};
use crate::workspace;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PublishProof {
    pub(crate) package_id: String,
    pub(crate) path: String,
    pub(crate) package: String,
    pub(crate) version: String,
    pub(crate) kern: String,
    pub(crate) description: String,
    pub(crate) license: String,
    pub(crate) authors: Vec<String>,
    pub(crate) readme: String,
    pub(crate) repository: String,
    pub(crate) manifest_sha256: String,
    pub(crate) source_sha256: String,
}

pub(crate) struct PublishPackageInput {
    pub(crate) path: String,
    pub(crate) package_root: PathBuf,
    pub(crate) manifest: Manifest,
    pub(crate) description: String,
    pub(crate) license: String,
    pub(crate) authors: Vec<String>,
    pub(crate) readme: String,
    pub(crate) repository: String,
}

pub(crate) fn expected_publish_proof_for_lock(
    package_id: impl Into<String>,
    input: PublishPackageInput,
) -> Result<PublishProof> {
    let package = input
        .manifest
        .package
        .as_ref()
        .ok_or_else(|| Error::Validation {
            path: input.package_root.join("Craft.toml"),
            message: "publish proof requires `[package]` metadata".to_string(),
        })?;
    Ok(PublishProof {
        package_id: package_id.into(),
        path: input.path,
        package: package.name.clone(),
        version: package.version.clone(),
        kern: package.kern.clone(),
        description: input.description,
        license: input.license,
        authors: input.authors,
        readme: input.readme,
        repository: input.repository,
        manifest_sha256: sha256_file_prefixed(&input.package_root.join("Craft.toml"))?,
        source_sha256: sha256_tree_prefixed(&input.package_root)?,
    })
}

pub(crate) fn validate_git_dependency_publish_proof(
    package_root: &Path,
    dependency: &ExternalPackageId,
    source: &FetchedSource,
) -> Result<()> {
    if source.backend != FetchedSourceBackend::GitDependency {
        return Ok(());
    }

    let lockfile_path = package_root.join("Craft.lock");
    if !lockfile_path.is_file() {
        return Err(Error::Validation {
            path: lockfile_path,
            message: "git dependency is missing committed `Craft.lock` publish proof; run `craft check`, commit Craft.lock, and depend on that validated revision"
                .to_string(),
        });
    }
    let root_manifest_path = package_root.join("Craft.toml");
    let root_manifest = Manifest::load(&root_manifest_path)?;
    let (member_root, member_manifest_path, manifest) = git_dependency_package_manifest(
        package_root,
        &root_manifest_path,
        &root_manifest,
        dependency,
    )?;
    let package = manifest.package.as_ref().ok_or_else(|| Error::Validation {
        path: member_manifest_path.clone(),
        message: "git dependency manifest is missing `[package]`".to_string(),
    })?;
    let package_name = package.name.clone();
    let package_version = package.version.clone();
    let package_kern = package.kern.clone();
    let lockfile = crate::lockfile::Lockfile::load(&lockfile_path)?;
    let Some(actual) = lockfile
        .publish_proofs
        .iter()
        .find(|entry| entry.package == package_name && entry.version == package_version)
    else {
        return Err(Error::Validation {
            path: lockfile_path,
            message: "git dependency is missing a matching `Craft.lock` publish proof".to_string(),
        });
    };
    let expected_path = relative_display(package_root, &member_root);
    if actual.path != expected_path {
        return Err(Error::Validation {
            path: lockfile_path,
            message: format!(
                "git dependency publish proof path `{}` does not match package path `{expected_path}`",
                actual.path
            ),
        });
    }
    if actual.kern != package_kern {
        return Err(Error::Validation {
            path: lockfile_path,
            message: "git dependency publish proof Kern version does not match package metadata"
                .to_string(),
        });
    }
    let expected = expected_publish_proof_for_lock(
        actual.package_id.clone(),
        PublishPackageInput {
            path: expected_path,
            package_root: member_root.clone(),
            manifest,
            description: actual.description.clone(),
            license: actual.license.clone(),
            authors: actual.authors.clone(),
            readme: actual.readme.clone(),
            repository: actual.repository.clone(),
        },
    )?;
    if *actual != expected {
        return Err(Error::Validation {
            path: lockfile_path,
            message: "git dependency publish proof does not match package contents or metadata"
                .to_string(),
        });
    }

    if let Some(version) = dependency.version.as_deref()
        && package_version != version
    {
        return Err(Error::Validation {
            path: member_manifest_path.clone(),
            message: format!(
                "git dependency `{}` requested version `{version}` but publish proof describes `{}`",
                dependency.package_name, package_version
            ),
        });
    }
    if !repository_urls_match(&actual.repository, &source.locator) {
        return Err(Error::Validation {
            path: lockfile_path,
            message: format!(
                "git dependency repository `{}` does not match fetched source `{}`",
                actual.repository, source.locator
            ),
        });
    }
    if !matches!(dependency.source, SourceId::GitDependency { .. }) {
        return Err(Error::Validation {
            path: package_root.to_path_buf(),
            message: "publish validation only supports git dependencies".to_string(),
        });
    }

    Ok(())
}

fn git_dependency_package_manifest(
    package_root: &Path,
    manifest_path: &Path,
    root_manifest: &Manifest,
    dependency: &ExternalPackageId,
) -> Result<(PathBuf, PathBuf, Manifest)> {
    if root_manifest.package.is_some() {
        return Ok((
            package_root.to_path_buf(),
            manifest_path.to_path_buf(),
            root_manifest.clone(),
        ));
    }

    let exported =
        workspace::exported_package(manifest_path, root_manifest, &dependency.package_name)?;
    let member_root = exported
        .manifest_path
        .parent()
        .unwrap_or(package_root)
        .to_path_buf();
    Ok((member_root, exported.manifest_path, exported.manifest))
}

pub(crate) fn valid_sha256_digest(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha256_file_prefixed(path: &Path) -> Result<String> {
    let bytes = fs::read(path).map_err(|err| Error::from_io(path, err))?;
    Ok(format!("sha256:{}", sha256_hex(&bytes)))
}

fn sha256_tree_prefixed(root: &Path) -> Result<String> {
    let mut entries = Vec::new();
    collect_tree_entries(root, root, &mut entries)?;
    entries.sort();

    let mut bytes = Vec::new();
    for entry in entries {
        match entry {
            TreeEntry::Dir(relative) => {
                bytes.extend_from_slice(b"dir:");
                bytes.extend_from_slice(relative.as_bytes());
                bytes.push(0);
            }
            TreeEntry::File(relative, contents) => {
                bytes.extend_from_slice(b"file:");
                bytes.extend_from_slice(relative.as_bytes());
                bytes.push(0);
                bytes.extend_from_slice(&contents);
                bytes.push(0);
            }
        }
    }
    Ok(format!("sha256:{}", sha256_hex(&bytes)))
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum TreeEntry {
    Dir(String),
    File(String, Vec<u8>),
}

fn collect_tree_entries(root: &Path, current: &Path, entries: &mut Vec<TreeEntry>) -> Result<()> {
    for entry in fs::read_dir(current).map_err(|err| Error::from_io(current, err))? {
        let entry = entry.map_err(Error::from_io_plain)?;
        let name = entry.file_name();
        if name == std::ffi::OsStr::new(".git")
            || name == std::ffi::OsStr::new(".craft")
            || name == std::ffi::OsStr::new("Craft.lock")
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
                "unsupported filesystem entry `{}` while hashing publish source tree",
                path.display()
            )));
        }
    }
    Ok(())
}

pub(crate) fn repository_urls_match(repository: &str, remote: &str) -> bool {
    normalize_repository_url(repository) == normalize_repository_url(remote)
}

fn normalize_repository_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    let without_git = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    if let Some(path) = without_git.strip_prefix("file://") {
        return normalize_local_repository_path(path);
    }
    if let Some(rest) = without_git.strip_prefix("git@github.com:") {
        return format!("https://github.com/{rest}");
    }
    if looks_like_local_repository_path(without_git) {
        return normalize_local_repository_path(without_git);
    }
    without_git.to_string()
}

fn looks_like_local_repository_path(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with("\\\\")
        || value.as_bytes().get(1).is_some_and(|byte| *byte == b':')
}

fn normalize_local_repository_path(value: &str) -> String {
    let mut text = value.trim().trim_end_matches('/').trim_end_matches('\\');
    if let Some(stripped) = text.strip_prefix("\\\\?\\") {
        text = stripped;
    } else if let Some(stripped) = text.strip_prefix("//?/") {
        text = stripped;
    }

    let path = Path::new(text);
    if let Ok(canonical) = path.canonicalize() {
        return normalize_local_repository_path_text(&canonical.to_string_lossy());
    }

    normalize_local_repository_path_text(text)
}

fn normalize_local_repository_path_text(value: &str) -> String {
    let mut text = value.replace('\\', "/");
    while text.ends_with('/') {
        text.pop();
    }
    if let Some(stripped) = text.strip_suffix(".git") {
        text = stripped.to_string();
    }
    if text.as_bytes().get(1).is_some_and(|byte| *byte == b':') {
        text = text.to_ascii_lowercase();
    }
    text
}

fn relative_display(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let text = relative.to_string_lossy().replace('\\', "/");
    if text.is_empty() {
        ".".to_string()
    } else {
        text
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut data = bytes.to_vec();
    let bit_len = (data.len() as u64) * 8;
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_be_bytes());

    let mut h = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];

    for chunk in data.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            let offset = i * 4;
            *word = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    h.iter().map(|word| format!("{word:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::repository_urls_match;

    #[test]
    fn repository_urls_match_normalizes_common_remote_spellings() {
        assert!(repository_urls_match(
            "git@github.com:kern-project/json-kern.git",
            "https://github.com/kern-project/json-kern"
        ));
        assert!(repository_urls_match(
            "https://github.com/kern-project/json-kern.git/",
            "https://github.com/kern-project/json-kern"
        ));
    }

    #[test]
    fn repository_urls_match_normalizes_windows_verbatim_paths() {
        assert!(repository_urls_match(
            r"C:\Users\runneradmin\AppData\Local\Temp\repo.git",
            r"\\?\C:\Users\runneradmin\AppData\Local\Temp\repo.git"
        ));
        assert!(repository_urls_match(
            r"file://C:\Users\runneradmin\AppData\Local\Temp\repo.git",
            r"\\?\C:\Users\runneradmin\AppData\Local\Temp\repo.git"
        ));
    }
}
