use crate::error::{Error, Result};
use std::env;
use std::path::{Path, PathBuf};

const MANIFEST_FILE: &str = "Craft.toml";

pub fn resolve_manifest_path(input: Option<&Path>) -> Result<PathBuf> {
    match input {
        Some(path) => resolve_explicit_path(path),
        None => {
            let cwd = env::current_dir().map_err(Error::from_io_plain)?;
            discover_from_dir(&cwd)
        }
    }
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
