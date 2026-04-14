use crate::error::{Error, Result};
use crate::manifest::Manifest;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct WorkspaceMember {
    pub manifest_path: PathBuf,
    pub manifest: Manifest,
}

pub fn load_members(manifest_path: &Path, manifest: &Manifest) -> Result<Vec<WorkspaceMember>> {
    let Some(workspace) = &manifest.workspace else {
        return Ok(Vec::new());
    };

    let root_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let mut member_dirs = BTreeSet::new();

    for pattern in &workspace.members {
        for member_dir in expand_member_pattern(root_dir, pattern, manifest_path)? {
            member_dirs.insert(member_dir);
        }
    }

    let mut members = Vec::new();
    for member_dir in member_dirs {
        let member_manifest_path = member_dir.join("Craft.toml");
        if !member_manifest_path.is_file() {
            return Err(Error::Validation {
                path: manifest_path.to_path_buf(),
                message: format!(
                    "workspace member `{}` does not contain `Craft.toml`",
                    member_dir.display()
                ),
            });
        }

        let member_manifest = Manifest::load(&member_manifest_path)?;
        member_manifest.validate(&member_manifest_path)?;
        if member_manifest.package.is_none() {
            return Err(Error::Validation {
                path: member_manifest_path,
                message: "workspace members must declare `[package]`".to_string(),
            });
        }

        members.push(WorkspaceMember {
            manifest_path: member_dir.join("Craft.toml"),
            manifest: member_manifest,
        });
    }

    Ok(members)
}

fn expand_member_pattern(
    root_dir: &Path,
    pattern: &str,
    manifest_path: &Path,
) -> Result<Vec<PathBuf>> {
    if let Some(prefix) = pattern.strip_suffix("/*") {
        let base_dir = root_dir.join(prefix);
        if !base_dir.is_dir() {
            return Err(Error::Validation {
                path: manifest_path.to_path_buf(),
                message: format!(
                    "workspace member pattern `{pattern}` points to a missing directory `{}`",
                    base_dir.display()
                ),
            });
        }

        let mut members = Vec::new();
        let entries = fs::read_dir(&base_dir).map_err(|err| Error::from_io(&base_dir, err))?;
        for entry in entries {
            let entry = entry.map_err(Error::from_io_plain)?;
            let path = entry.path();
            if path.is_dir() {
                members.push(path);
            }
        }
        members.sort();
        return Ok(members);
    }

    let exact = root_dir.join(pattern);
    if exact.is_dir() {
        return Ok(vec![exact]);
    }

    Err(Error::Validation {
        path: manifest_path.to_path_buf(),
        message: format!(
            "workspace member `{pattern}` points to a missing directory `{}`",
            exact.display()
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::load_members;
    use crate::manifest::Manifest;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn loads_workspace_members_from_direct_and_glob_entries() {
        let root = temp_dir("craft-workspace");
        let compiler_dir = root.join("compiler").join("demo");
        let tools_dir = root.join("tools").join("demo");
        fs::create_dir_all(&compiler_dir).unwrap();
        fs::create_dir_all(&tools_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
members = ["compiler/*", "tools/demo"]
"#,
        )
        .unwrap();
        fs::write(
            compiler_dir.join("Craft.toml"),
            r#"
[package]
name = "compiler-demo"
version = "0.1.0"
kern = "0.7.0"
"#,
        )
        .unwrap();
        fs::write(
            tools_dir.join("Craft.toml"),
            r#"
[package]
name = "tools-demo"
version = "0.1.0"
kern = "0.7.0"
"#,
        )
        .unwrap();

        let root_manifest = Manifest::load(&root.join("Craft.toml")).unwrap();
        let members = load_members(&root.join("Craft.toml"), &root_manifest).unwrap();

        assert_eq!(members.len(), 2);
        assert!(
            members
                .iter()
                .any(|member| member.manifest.package.as_ref().unwrap().name == "compiler-demo")
        );
        assert!(
            members
                .iter()
                .any(|member| member.manifest.package.as_ref().unwrap().name == "tools-demo")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_workspace_member_without_package() {
        let root = temp_dir("craft-bad-workspace");
        let member_dir = root.join("compiler").join("demo");
        fs::create_dir_all(&member_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
members = ["compiler/*"]
"#,
        )
        .unwrap();
        fs::write(member_dir.join("Craft.toml"), "[workspace]\nmembers = []\n").unwrap();

        let root_manifest = Manifest::load(&root.join("Craft.toml")).unwrap();
        let err = load_members(&root.join("Craft.toml"), &root_manifest).unwrap_err();
        assert!(
            err.to_string()
                .contains("workspace members must declare `[package]`")
        );

        let _ = fs::remove_dir_all(root);
    }
}
