use crate::error::{Error, Result};
use crate::graph::SourceId;
use crate::local_state;
use crate::manifest::Manifest;
use crate::resolver::ExternalPackageId;
use crate::source::{FetchedSource, FetchedSourceBackend};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const PROOF_FILE: &str = "Craft.publish.toml";
const PROOF_FORMAT: &str = "craft.publish.v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublishProof {
    pub(crate) package: String,
    pub(crate) version: String,
    pub(crate) kern: String,
    pub(crate) repository: String,
    pub(crate) manifest_sha256: String,
    pub(crate) lockfile_sha256: String,
    pub(crate) source_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProofWriteResult {
    Current,
    Created,
    Updated,
}

pub(crate) fn proof_path(package_root: &Path) -> PathBuf {
    package_root.join(PROOF_FILE)
}

pub(crate) fn ensure_publish_proof_current(
    package_root: &Path,
    lockfile_path: &Path,
    manifest: &Manifest,
    repository: &str,
) -> Result<ProofWriteResult> {
    let expected = PublishProof::expected(package_root, lockfile_path, manifest, repository)?;
    let path = proof_path(package_root);
    let rendered = expected.render();
    let result = match fs::read_to_string(&path) {
        Ok(existing) if existing == rendered => ProofWriteResult::Current,
        Ok(_) => ProofWriteResult::Updated,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => ProofWriteResult::Created,
        Err(err) => return Err(Error::from_io(&path, err)),
    };
    if result != ProofWriteResult::Current {
        local_state::write_file_atomic(&path, rendered)?;
    }
    Ok(result)
}

#[cfg(test)]
pub(crate) fn write_test_publish_proof(package_root: &Path, repository: &str) -> Result<()> {
    let manifest = Manifest::load(&package_root.join("Craft.toml"))?;
    let expected = PublishProof::expected(
        package_root,
        &package_root.join("Craft.lock"),
        &manifest,
        repository,
    )?;
    local_state::write_file_atomic(&proof_path(package_root), expected.render())
}

pub(crate) fn validate_git_dependency_publish_proof(
    package_root: &Path,
    dependency: &ExternalPackageId,
    source: &FetchedSource,
) -> Result<()> {
    if source.backend != FetchedSourceBackend::GitDependency {
        return Ok(());
    }

    let proof_path = proof_path(package_root);
    let proof_source = fs::read_to_string(&proof_path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            Error::Validation {
                path: proof_path.clone(),
                message: "git dependency is missing `Craft.publish.toml`; run `craft publish`, commit the proof, and depend on that published revision"
                    .to_string(),
            }
        } else {
            Error::from_io(&proof_path, err)
        }
    })?;
    let actual = PublishProof::parse(&proof_source, &proof_path)?;
    let manifest_path = package_root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path)?;
    let package = manifest.package.as_ref().ok_or_else(|| Error::Validation {
        path: manifest_path.clone(),
        message: "git dependency manifest is missing `[package]`".to_string(),
    })?;
    let repository =
        package.repository.as_ref().ok_or_else(|| {
            Error::Validation {
        path: manifest_path.clone(),
        message:
            "git dependency manifest is missing `[package].repository` for publish proof validation"
                .to_string(),
    }
        })?;
    let expected = PublishProof::expected(
        package_root,
        &package_root.join("Craft.lock"),
        &manifest,
        repository,
    )?;

    if actual != expected {
        return Err(Error::Validation {
            path: proof_path,
            message: "git dependency publish proof does not match package contents or metadata"
                .to_string(),
        });
    }

    if package.name != dependency.package_name {
        return Err(Error::Validation {
            path: manifest_path.clone(),
            message: format!(
                "git dependency requested package `{}` but publish proof describes `{}`",
                dependency.package_name, package.name
            ),
        });
    }
    if let Some(version) = dependency.version.as_deref()
        && package.version != version
    {
        return Err(Error::Validation {
            path: manifest_path.clone(),
            message: format!(
                "git dependency `{}` requested version `{version}` but publish proof describes `{}`",
                dependency.package_name, package.version
            ),
        });
    }
    if !repository_urls_match(&actual.repository, &source.locator) {
        return Err(Error::Validation {
            path: manifest_path,
            message: format!(
                "git dependency repository `{}` does not match fetched source `{}`",
                actual.repository, source.locator
            ),
        });
    }
    if !matches!(dependency.source, SourceId::GitDependency { .. }) {
        return Err(Error::Validation {
            path: package_root.to_path_buf(),
            message: "publish proof validation only supports git dependencies".to_string(),
        });
    }

    Ok(())
}

impl PublishProof {
    fn expected(
        package_root: &Path,
        lockfile_path: &Path,
        manifest: &Manifest,
        repository: &str,
    ) -> Result<Self> {
        let package = manifest.package.as_ref().ok_or_else(|| Error::Validation {
            path: package_root.join("Craft.toml"),
            message: "publish proof requires `[package]` metadata".to_string(),
        })?;
        Ok(Self {
            package: package.name.clone(),
            version: package.version.clone(),
            kern: package.kern.clone(),
            repository: repository.to_string(),
            manifest_sha256: sha256_file_prefixed(&package_root.join("Craft.toml"))?,
            lockfile_sha256: sha256_file_prefixed(lockfile_path)?,
            source_sha256: sha256_tree_prefixed(package_root)?,
        })
    }

    fn render(&self) -> String {
        let mut out = String::new();
        push_string_line(&mut out, "format", PROOF_FORMAT);
        push_string_line(&mut out, "package", &self.package);
        push_string_line(&mut out, "version", &self.version);
        push_string_line(&mut out, "kern", &self.kern);
        push_string_line(&mut out, "repository", &self.repository);
        push_string_line(&mut out, "manifest-sha256", &self.manifest_sha256);
        push_string_line(&mut out, "lockfile-sha256", &self.lockfile_sha256);
        push_string_line(&mut out, "source-sha256", &self.source_sha256);
        out
    }

    fn parse(source: &str, path: &Path) -> Result<Self> {
        let mut values = BTreeMap::new();
        for raw_line in source.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(proof_parse_error(
                    path,
                    format!("expected `key = value`, got `{line}`"),
                ));
            };
            values.insert(key.trim().to_string(), parse_string(value.trim(), path)?);
        }

        let format = take_field(&mut values, path, "format")?;
        if format != PROOF_FORMAT {
            return Err(proof_parse_error(
                path,
                format!("unsupported publish proof format `{format}`"),
            ));
        }
        let proof = Self {
            package: take_field(&mut values, path, "package")?,
            version: take_field(&mut values, path, "version")?,
            kern: take_field(&mut values, path, "kern")?,
            repository: take_field(&mut values, path, "repository")?,
            manifest_sha256: take_sha256_field(&mut values, path, "manifest-sha256")?,
            lockfile_sha256: take_sha256_field(&mut values, path, "lockfile-sha256")?,
            source_sha256: take_sha256_field(&mut values, path, "source-sha256")?,
        };
        if let Some(extra) = values.keys().next() {
            return Err(proof_parse_error(
                path,
                format!("unsupported publish proof key `{extra}`"),
            ));
        }
        Ok(proof)
    }
}

fn take_field(values: &mut BTreeMap<String, String>, path: &Path, key: &str) -> Result<String> {
    values
        .remove(key)
        .ok_or_else(|| proof_parse_error(path, format!("missing `{key}`")))
}

fn take_sha256_field(
    values: &mut BTreeMap<String, String>,
    path: &Path,
    key: &str,
) -> Result<String> {
    let value = take_field(values, path, key)?;
    if !valid_sha256_digest(&value) {
        return Err(proof_parse_error(
            path,
            format!("`{key}` must be a `sha256:` digest"),
        ));
    }
    Ok(value)
}

fn parse_string(raw: &str, path: &Path) -> Result<String> {
    if !raw.starts_with('"') || !raw.ends_with('"') || raw.len() < 2 {
        return Err(proof_parse_error(path, "expected a string literal"));
    }
    let mut out = String::new();
    let mut chars = raw[1..raw.len() - 1].chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let Some(escaped) = chars.next() else {
                return Err(proof_parse_error(path, "unterminated string escape"));
            };
            match escaped {
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                'n' => out.push('\n'),
                't' => out.push('\t'),
                _ => return Err(proof_parse_error(path, "unsupported string escape")),
            }
        } else {
            out.push(ch);
        }
    }
    Ok(out)
}

fn proof_parse_error(path: &Path, message: impl Into<String>) -> Error {
    Error::Validation {
        path: path.to_path_buf(),
        message: format!("invalid publish proof: {}", message.into()),
    }
}

fn push_string_line(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push_str(" = \"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push_str("\"\n");
}

fn valid_sha256_digest(value: &str) -> bool {
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
            || name == std::ffi::OsStr::new(PROOF_FILE)
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
                "unsupported filesystem entry `{}` while hashing publish proof source tree",
                path.display()
            )));
        }
    }
    Ok(())
}

fn repository_urls_match(repository: &str, remote: &str) -> bool {
    normalize_repository_url(repository) == normalize_repository_url(remote)
}

fn normalize_repository_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    let without_git = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    if let Some(rest) = without_git.strip_prefix("git@github.com:") {
        return format!("https://github.com/{rest}");
    }
    if let Some(path) = without_git.strip_prefix("file://") {
        return Path::new(path)
            .canonicalize()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.replace('\\', "/"));
    }
    if without_git.starts_with('/') {
        return Path::new(without_git)
            .canonicalize()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| without_git.replace('\\', "/"));
    }
    without_git.to_string()
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = sha256(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(hex_digit(byte >> 4));
        out.push(hex_digit(byte & 0x0f));
    }
    out
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        10..=15 => char::from(b'a' + value - 10),
        _ => unreachable!("hex digit range"),
    }
}

fn sha256(input: &[u8]) -> [u8; 32] {
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

    let bit_len = (input.len() as u64) * 8;
    let mut data = input.to_vec();
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in data.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (index, word) in w.iter_mut().take(16).enumerate() {
            let offset = index * 4;
            *word = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
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

        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
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

    let mut out = [0u8; 32];
    for (index, word) in h.into_iter().enumerate() {
        out[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::sha256_hex;

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
