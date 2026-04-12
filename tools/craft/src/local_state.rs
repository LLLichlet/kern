use crate::error::{Error, Result};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const CRAFT_GITIGNORE_ENTRY: &str = ".craft/\n";
static ATOMIC_WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|err| Error::from_io(path, err))
}

pub(crate) fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    Ok(())
}

pub(crate) fn write_file_atomic(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    ensure_parent_dir(path)?;
    let contents = contents.as_ref();

    match fs::read(path) {
        Ok(existing) if existing == contents => return Ok(()),
        Ok(_) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(Error::from_io(path, err)),
    }

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

pub(crate) fn ensure_workspace_gitignore_entry(workspace_root: &Path) -> Result<bool> {
    let gitignore_path = workspace_root.join(".gitignore");
    match fs::read_to_string(&gitignore_path) {
        Ok(contents) => {
            if ignores_all_craft_outputs(&contents) {
                return Ok(false);
            }

            let mut updated = contents;
            if !updated.is_empty() && !updated.ends_with('\n') {
                updated.push('\n');
            }
            if !updated.is_empty() {
                updated.push('\n');
            }
            updated.push_str(CRAFT_GITIGNORE_ENTRY);
            fs::write(&gitignore_path, updated)
                .map_err(|err| Error::from_io(&gitignore_path, err))?;
            Ok(true)
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {
            fs::write(&gitignore_path, CRAFT_GITIGNORE_ENTRY)
                .map_err(|err| Error::from_io(&gitignore_path, err))?;
            Ok(true)
        }
        Err(err) => Err(Error::from_io(&gitignore_path, err)),
    }
}

fn ignores_all_craft_outputs(contents: &str) -> bool {
    for line in contents.lines().map(str::trim) {
        if line == ".craft/" || line == ".craft" {
            return true;
        }
    }
    false
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
    use super::{ensure_dir, ensure_workspace_gitignore_entry, write_file_atomic};
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
    fn ensure_dir_does_not_touch_workspace_gitignore() {
        let root = temp_dir("craft-local-state");
        let path = root.join(".craft").join("build").join("dev");

        ensure_dir(&path).unwrap();

        assert!(!root.join(".gitignore").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn creates_gitignore_entry_when_requested() {
        let root = temp_dir("craft-local-state");

        assert!(ensure_workspace_gitignore_entry(&root).unwrap());

        let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
        assert_eq!(gitignore, ".craft/\n");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn preserves_existing_gitignore_entries() {
        let root = temp_dir("craft-local-state-existing");
        let craft_root = root.join(".craft");
        fs::create_dir_all(&craft_root).unwrap();
        let gitignore_path = root.join(".gitignore");
        fs::write(&gitignore_path, "# keep custom rule\n!README.md\n").unwrap();

        assert!(ensure_workspace_gitignore_entry(&root).unwrap());

        let gitignore = fs::read_to_string(gitignore_path).unwrap();
        assert!(gitignore.contains("!README.md"));
        assert!(gitignore.contains(".craft/"));

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

    #[test]
    fn atomic_write_skips_unchanged_contents() {
        let root = temp_dir("craft-local-state-atomic-skip");
        let path = root.join(".craft").join("analysis.toml");

        write_file_atomic(&path, "version = 1\n").unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        write_file_atomic(&path, "version = 1\n").unwrap();
        let after = fs::metadata(&path).unwrap().modified().unwrap();

        assert_eq!(before, after);

        let _ = fs::remove_dir_all(root);
    }
}
