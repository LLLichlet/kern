use crate::error::{Error, Result};
use crate::manifest::Manifest;
use crate::workspace;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const MANIFEST_FILE: &str = "Craft.toml";

pub fn resolve_manifest_path(input: Option<&Path>) -> Result<PathBuf> {
    let manifest_path = match input {
        Some(path) => resolve_explicit_path(path),
        None => {
            let cwd = env::current_dir().map_err(Error::from_io_plain)?;
            discover_from_dir(&cwd)
        }
    }?;
    fs::canonicalize(&manifest_path)
        .map(normalize_manifest_path)
        .map_err(|err| Error::from_io(&manifest_path, err))
}

pub fn resolve_project_manifest_path(input: Option<&Path>) -> Result<PathBuf> {
    let manifest_path = discover_project_entry_manifest_path(input)?;
    let manifest_path = fs::canonicalize(&manifest_path)
        .map(normalize_manifest_path)
        .map_err(|err| Error::from_io(&manifest_path, err))?;
    let mut current = manifest_path
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf);

    while let Some(dir) = current {
        let candidate = dir.join(MANIFEST_FILE);
        if !candidate.is_file() {
            current = dir.parent().map(Path::to_path_buf);
            continue;
        }

        let manifest = Manifest::load(&candidate)?;
        manifest.validate(&candidate)?;
        if manifest.workspace.is_some()
            && workspace::load_members(&candidate, &manifest)?
                .iter()
                .any(|member| member.manifest_path == manifest_path)
        {
            return fs::canonicalize(&candidate)
                .map(normalize_manifest_path)
                .map_err(|err| Error::from_io(&candidate, err));
        }

        current = dir.parent().map(Path::to_path_buf);
    }

    Ok(manifest_path)
}

fn normalize_manifest_path(path: PathBuf) -> PathBuf {
    let path = strip_windows_verbatim_prefix(path);
    strip_macos_private_var_prefix(path)
}

#[cfg(windows)]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix("\\\\?\\UNC\\") {
        return PathBuf::from(format!("\\\\{stripped}"));
    }
    if let Some(stripped) = raw.strip_prefix("\\\\?\\") {
        return PathBuf::from(stripped);
    }
    path
}

#[cfg(not(windows))]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    path
}

#[cfg(target_os = "macos")]
fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix("/private/var/") {
        return PathBuf::from(format!("/var/{stripped}"));
    }
    if raw == "/private/var" {
        return PathBuf::from("/var");
    }
    path
}

#[cfg(not(target_os = "macos"))]
fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
    path
}

fn discover_project_entry_manifest_path(input: Option<&Path>) -> Result<PathBuf> {
    let start = match input {
        Some(path) if path.file_name().and_then(|name| name.to_str()) == Some(MANIFEST_FILE) => {
            return Ok(path.to_path_buf());
        }
        Some(path) if path.is_dir() => path.to_path_buf(),
        Some(path) => path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
        None => env::current_dir().map_err(Error::from_io_plain)?,
    };

    let mut current = Some(start.as_path());
    while let Some(dir) = current {
        let candidate = dir.join(MANIFEST_FILE);
        if candidate.is_file() {
            return Ok(candidate);
        }
        current = dir.parent();
    }

    Err(Error::ManifestNotFound { start })
}

fn resolve_explicit_path(path: &Path) -> Result<PathBuf> {
    if path.is_file() {
        if path.file_name().and_then(|name| name.to_str()) == Some(MANIFEST_FILE) {
            return Ok(path.to_path_buf());
        }

        return Err(Error::Usage(format!(
            "expected a directory or `{MANIFEST_FILE}`, found `{}`",
            path.display()
        )));
    }

    if path.is_dir() {
        return discover_from_dir(path);
    }

    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == MANIFEST_FILE)
    {
        return Ok(path.to_path_buf());
    }

    Err(Error::Usage(format!(
        "path `{}` does not exist",
        path.display()
    )))
}

fn discover_from_dir(start: &Path) -> Result<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        let candidate = dir.join(MANIFEST_FILE);
        if candidate.is_file() {
            return Ok(candidate);
        }
        current = dir.parent();
    }

    Err(Error::ManifestNotFound {
        start: start.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::{normalize_manifest_path, resolve_manifest_path, resolve_project_manifest_path};
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
    fn canonicalizes_explicit_manifest_paths() {
        let root = temp_dir("craft-discover-explicit");
        fs::write(root.join("Craft.toml"), "[package]\nname = \"demo\"\n").unwrap();

        let resolved = resolve_manifest_path(Some(&root.join(".").join("Craft.toml"))).unwrap();
        let expected = normalize_manifest_path(fs::canonicalize(root.join("Craft.toml")).unwrap());
        assert_eq!(resolved, expected);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn canonicalizes_discovered_manifest_paths_from_directories() {
        let root = temp_dir("craft-discover-dir");
        let nested = root.join("pkg").join("src");
        fs::create_dir_all(&nested).unwrap();
        fs::write(
            root.join("pkg").join("Craft.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();

        let resolved = resolve_manifest_path(Some(&root.join("pkg").join("src"))).unwrap();
        let expected =
            normalize_manifest_path(fs::canonicalize(root.join("pkg").join("Craft.toml")).unwrap());
        assert_eq!(resolved, expected);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolves_workspace_root_manifest_for_member_directories() {
        let root = temp_dir("craft-discover-workspace-root");
        let member = root.join("member");
        fs::create_dir_all(member.join("src")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            "[workspace]\nmembers = [\"member\"]\n",
        )
        .unwrap();
        fs::write(
            member.join("Craft.toml"),
            "[package]\nname = \"member\"\nversion = \"0.1.0\"\nkern = \"0.7.0\"\n",
        )
        .unwrap();

        let resolved = resolve_project_manifest_path(Some(&member)).unwrap();
        let expected = normalize_manifest_path(fs::canonicalize(root.join("Craft.toml")).unwrap());
        assert_eq!(resolved, expected);

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn normalize_manifest_path_strips_private_var_prefix() {
        assert_eq!(
            normalize_manifest_path(PathBuf::from("/private/var/folders/example/Craft.toml")),
            PathBuf::from("/var/folders/example/Craft.toml")
        );
    }
}
