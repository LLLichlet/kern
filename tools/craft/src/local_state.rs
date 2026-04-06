use crate::error::{Error, Result};
use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const CRAFT_GITIGNORE_BLOCK: &str =
    "# Managed by craft. Keep local derived state out of git.\n*\n!.gitignore\n";
static ATOMIC_WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|err| Error::from_io(path, err))?;
    ensure_craft_gitignore(path)
}

pub(crate) fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    Ok(())
}

pub(crate) fn write_file_atomic(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    ensure_parent_dir(path)?;

    let temp_path = atomic_temp_path(path);
    let write_result = fs::write(&temp_path, contents);
    if let Err(err) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(Error::from_io(&temp_path, err));
    }

    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path).map_err(|err| Error::from_io(path, err))?;
    }

    fs::rename(&temp_path, path).map_err(|err| {
        let _ = fs::remove_file(&temp_path);
        Error::from_io(path, err)
    })?;
    Ok(())
}

fn ensure_craft_gitignore(path: &Path) -> Result<()> {
    let Some(craft_dir) = path
        .ancestors()
        .find(|ancestor| ancestor.file_name() == Some(OsStr::new(".craft")))
    else {
        return Ok(());
    };

    let gitignore_path = craft_dir.join(".gitignore");
    match fs::read_to_string(&gitignore_path) {
        Ok(contents) => {
            if ignores_all_craft_outputs(&contents) {
                return Ok(());
            }

            let mut updated = contents;
            if !updated.is_empty() && !updated.ends_with('\n') {
                updated.push('\n');
            }
            if !updated.is_empty() {
                updated.push('\n');
            }
            updated.push_str(CRAFT_GITIGNORE_BLOCK);
            fs::write(&gitignore_path, updated).map_err(|err| Error::from_io(&gitignore_path, err))
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {
            fs::write(&gitignore_path, CRAFT_GITIGNORE_BLOCK)
                .map_err(|err| Error::from_io(&gitignore_path, err))
        }
        Err(err) => Err(Error::from_io(&gitignore_path, err)),
    }
}

fn ignores_all_craft_outputs(contents: &str) -> bool {
    let mut ignore_all = false;
    let mut keep_gitignore = false;

    for line in contents.lines().map(str::trim) {
        if line == "*" {
            ignore_all = true;
        } else if line == "!.gitignore" {
            keep_gitignore = true;
        }
    }

    ignore_all && keep_gitignore
}

fn atomic_temp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("tmp");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = ATOMIC_WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(
            ".{file_name}.tmp-{}-{nonce}-{counter}",
            std::process::id()
        ))
}

#[cfg(test)]
mod tests {
    use super::{ensure_dir, ensure_parent_dir, write_file_atomic};
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
    fn creates_gitignore_for_craft_subtrees() {
        let root = temp_dir("craft-local-state");
        let path = root.join(".craft").join("build").join("dev");

        ensure_dir(&path).unwrap();

        let gitignore = fs::read_to_string(root.join(".craft").join(".gitignore")).unwrap();
        assert!(gitignore.contains("*"));
        assert!(gitignore.contains("!.gitignore"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn preserves_existing_gitignore_entries() {
        let root = temp_dir("craft-local-state-existing");
        let craft_root = root.join(".craft");
        fs::create_dir_all(&craft_root).unwrap();
        let gitignore_path = craft_root.join(".gitignore");
        fs::write(&gitignore_path, "# keep custom rule\n!README.md\n").unwrap();

        ensure_parent_dir(&craft_root.join("build").join("dev").join("artifact.o")).unwrap();

        let gitignore = fs::read_to_string(gitignore_path).unwrap();
        assert!(gitignore.contains("!README.md"));
        assert!(gitignore.contains("*"));
        assert!(gitignore.contains("!.gitignore"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn atomic_write_replaces_existing_file_contents() {
        let root = temp_dir("craft-local-state-atomic-write");
        let path = root.join(".craft").join("analysis.toml");

        write_file_atomic(&path, "version = 1\n").unwrap();
        write_file_atomic(&path, "version = 2\n").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "version = 2\n");

        let _ = fs::remove_dir_all(root);
    }
}
