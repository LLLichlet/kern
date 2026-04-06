use crate::error::{Error, Result};
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
    fs::canonicalize(&manifest_path).map_err(|err| Error::from_io(&manifest_path, err))
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
    use super::resolve_manifest_path;
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
        let expected = fs::canonicalize(root.join("Craft.toml")).unwrap();
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
        let expected = fs::canonicalize(root.join("pkg").join("Craft.toml")).unwrap();
        assert_eq!(resolved, expected);

        let _ = fs::remove_dir_all(root);
    }
}
