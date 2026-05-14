use shared_ops::{
    ArtifactRecord, BundledToolchain, OpsError, OpsResult, artifact_record_json, file_size,
    sha256_file,
};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

pub fn insert_file_record(
    records: &mut serde_json::Map<String, serde_json::Value>,
    component: &str,
    path: &Path,
    dist_dir: &Path,
) -> OpsResult<()> {
    insert_record(
        records,
        component,
        ArtifactRecord {
            path: path_relative_to(path, dist_dir)?,
            kind: "file".into(),
            sha256: Some(sha256_file(path)?),
            size: Some(file_size(path)?),
        },
    );
    Ok(())
}

pub fn insert_record(
    records: &mut serde_json::Map<String, serde_json::Value>,
    component: &str,
    record: ArtifactRecord,
) {
    records.insert(component.into(), artifact_record_json(&record));
}

pub fn bundled_resource_dir_path(bundled_toolchain: &BundledToolchain) -> OpsResult<String> {
    let resource_dir = bundled_toolchain
        .resource_dir
        .as_ref()
        .ok_or_else(|| OpsError::new("bundled toolchain has no clang resource dir"))?;
    let name = resource_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| OpsError::new("clang resource dir has an invalid final path component"))?;
    Ok(format!("toolchain/host/lib/clang/{name}"))
}

pub fn relative_under_prefix(path: &Path, prefix: &Path) -> OpsResult<PathBuf> {
    path.strip_prefix(prefix)
        .map(Path::to_path_buf)
        .map_err(|_| {
            OpsError::new(format!(
                "toolchain path `{}` does not live under prefix `{}`",
                path.display(),
                prefix.display()
            ))
        })
}

pub fn path_relative_to(path: &Path, root: &Path) -> OpsResult<String> {
    Ok(path
        .strip_prefix(root)
        .map_err(|err| OpsError::new(err.to_string()))?
        .to_string_lossy()
        .replace('\\', "/"))
}

pub fn canonical_toolchain_component_name(
    host: &shared_ops::HostTarget,
    component: &str,
    source: &Path,
) -> OsString {
    let exe_suffix = if host.archive_target.ends_with("windows-msvc") {
        ".exe"
    } else {
        ""
    };
    let name = match component {
        "clang" => format!("clang{exe_suffix}"),
        "clangxx" => format!("clang++{exe_suffix}"),
        "lld" if host.archive_target.ends_with("windows-msvc") => "lld-link.exe".into(),
        "lld" if host.archive_target.ends_with("apple-darwin") => "ld64.lld".into(),
        "lld" => "ld.lld".into(),
        "llvm_ar" => format!("llvm-ar{exe_suffix}"),
        "llvm_config" => format!("llvm-config{exe_suffix}"),
        "llvm_lib" => "llvm-lib.exe".into(),
        _ => return source.file_name().unwrap_or_default().to_owned(),
    };
    OsString::from(name)
}

pub fn files_with_extension(root: &Path, extension: &str) -> OpsResult<Vec<PathBuf>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some(extension) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

pub fn direct_files(root: &Path) -> OpsResult<Vec<PathBuf>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

pub fn is_empty_dir(path: &Path) -> OpsResult<bool> {
    Ok(path.is_dir() && fs::read_dir(path)?.next().is_none())
}

pub fn canonical_or_self(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub fn push_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    if !items.contains(&item) {
        items.push(item);
    }
}

pub fn find_program_local(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        if cfg!(windows) {
            let candidate = dir.join(format!("{name}.exe"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
