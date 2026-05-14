use super::util::{canonical_or_self, path_relative_to, push_unique};
use shared_ops::{OpsResult, file_size, sha256_file, write_json_value};
use std::fs;
use std::path::{Path, PathBuf};

pub fn resolve_checksum_inputs(root: &Path, patterns: &[String]) -> OpsResult<Vec<PathBuf>> {
    let mut matched = Vec::new();
    for pattern in patterns {
        if has_wildcard(pattern) {
            let mut files = Vec::new();
            collect_all_files(root, &mut files)?;
            files.sort();
            for file in files {
                let relative = path_relative_to(&file, root)?;
                if wildcard_match(pattern, &relative) {
                    push_unique(&mut matched, canonical_or_self(&file));
                }
            }
            continue;
        }
        let candidate = if Path::new(pattern).is_absolute() {
            PathBuf::from(pattern)
        } else {
            root.join(pattern)
        };
        if candidate.is_file() {
            push_unique(&mut matched, canonical_or_self(&candidate));
        }
    }
    Ok(matched)
}

fn has_wildcard(value: &str) -> bool {
    value.contains('*') || value.contains('?')
}

pub(crate) fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let (mut p, mut t) = (0, 0);
    let mut star = None;
    let mut star_match = 0;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            star_match = t;
            p += 1;
        } else if let Some(star_index) = star {
            if text[star_match] == b'/' {
                return false;
            }
            p = star_index + 1;
            star_match += 1;
            t = star_match;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

fn collect_all_files(root: &Path, out: &mut Vec<PathBuf>) -> OpsResult<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_all_files(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

pub fn write_checksums(args: crate::args::ReleaseChecksumsArgs) -> OpsResult<()> {
    let root = shared_ops::repo_root()?;
    let artifacts = resolve_checksum_inputs(&root, &args.paths)?;
    if artifacts.is_empty() {
        return Err(shared_ops::OpsError::new(
            "no release artifacts matched for checksum generation",
        ));
    }

    let mut records = Vec::new();
    for artifact in &artifacts {
        let digest = sha256_file(artifact)?;
        let name = artifact
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                shared_ops::OpsError::new("release artifact has an invalid file name")
            })?;
        let sidecar = artifact.with_file_name(format!("{name}.sha256"));
        fs::write(&sidecar, format!("{digest}  {name}\n"))?;
        records.push(serde_json::json!({
            "name": name,
            "path": name,
            "sha256": digest,
            "size": file_size(artifact)?,
            "sha256_sidecar": sidecar.file_name().and_then(|name| name.to_str()).unwrap_or_default(),
        }));
    }

    if let Some(path) = args.manifest_path {
        let manifest_path = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        write_json_value(
            &manifest_path,
            &serde_json::json!({
                "schema_version": 1,
                "channel": args.channel,
                "release_tag": args.release_tag,
                "assets": records,
            }),
        )?;
    }

    println!(
        "Generated checksums for {} release artifact(s)",
        artifacts.len()
    );
    Ok(())
}
