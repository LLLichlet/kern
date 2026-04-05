use crate::error::{Error, Result};
use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

const CRAFT_GITIGNORE_BLOCK: &str =
    "# Managed by craft. Keep local derived state out of git.\n*\n!.gitignore\n";

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

#[cfg(test)]
mod tests {
    use super::{ensure_dir, ensure_parent_dir};
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
}
