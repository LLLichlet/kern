use crate::error::{Error, Result};
use crate::manifest::{Manifest, WorkspacePackage};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct WorkspaceMember {
    pub manifest_path: PathBuf,
    pub manifest: Manifest,
}

#[derive(Debug, Clone)]
pub struct WorkspaceExportPackage {
    pub export_name: String,
    pub member: String,
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

        let mut member_manifest = Manifest::load(&member_manifest_path)?;
        inherit_workspace_package_defaults(&mut member_manifest, workspace.package.as_ref());
        if member_manifest.package.is_none() {
            return Err(Error::Validation {
                path: member_manifest_path,
                message: "workspace members must declare `[package]`".to_string(),
            });
        }
        member_manifest.validate(&member_manifest_path)?;

        members.push(WorkspaceMember {
            manifest_path: member_manifest_path,
            manifest: member_manifest,
        });
    }

    validate_exports(manifest_path, manifest, &members)?;

    Ok(members)
}

pub fn load_member_manifest(
    workspace_manifest_path: &Path,
    workspace_manifest: &Manifest,
    member_manifest_path: &Path,
) -> Result<Manifest> {
    let mut manifest = Manifest::load(member_manifest_path)?;
    if manifest.package.is_some()
        && workspace_manifest.workspace.is_some()
        && member_belongs_to_workspace(
            workspace_manifest_path,
            workspace_manifest,
            member_manifest_path,
        )
    {
        inherit_workspace_package_defaults(
            &mut manifest,
            workspace_manifest
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.package.as_ref()),
        );
    }
    manifest.validate(member_manifest_path)?;
    Ok(manifest)
}

pub fn load_manifest_with_project_defaults(manifest_path: &Path) -> Result<Manifest> {
    let manifest = Manifest::load(manifest_path)?;
    if manifest.package.is_some()
        && let Some(member_manifest) = inherited_member_manifest_for_path(manifest_path)?
    {
        return Ok(member_manifest);
    }

    manifest.validate(manifest_path)?;
    Ok(manifest)
}

pub fn exported_package(
    manifest_path: &Path,
    manifest: &Manifest,
    export_name: &str,
) -> Result<WorkspaceExportPackage> {
    if manifest.package.is_some() {
        return Ok(WorkspaceExportPackage {
            export_name: export_name.to_string(),
            member: ".".to_string(),
            manifest_path: manifest_path.to_path_buf(),
            manifest: manifest.clone(),
        });
    }

    let Some(workspace) = &manifest.workspace else {
        return Err(Error::Validation {
            path: manifest_path.to_path_buf(),
            message: "dependency root must declare `[package]` or `[workspace]`".to_string(),
        });
    };
    let Some(export) = workspace.exports.get(export_name) else {
        return Err(Error::Validation {
            path: manifest_path.to_path_buf(),
            message: format!(
                "workspace `{}` does not export `{}`{}",
                workspace.name,
                export_name,
                export_list_suffix(workspace.exports.keys())
            ),
        });
    };
    let export_name = export_name.to_string();
    let export_member = export.member.clone();

    let members = load_members(manifest_path, manifest)?;
    members
        .into_iter()
        .find(|member| member_path(manifest_path, &member.manifest_path) == export_member)
        .map(|member| WorkspaceExportPackage {
            export_name: export_name.clone(),
            member: export_member.clone(),
            manifest_path: member.manifest_path,
            manifest: member.manifest,
        })
        .ok_or_else(|| Error::Validation {
            path: manifest_path.to_path_buf(),
            message: format!(
                "[workspace.exports].{export_name}.member `{}` is not listed in `[workspace].members`",
                export.member
            ),
        })
}

fn inherit_workspace_package_defaults(
    member_manifest: &mut Manifest,
    workspace_package: Option<&WorkspacePackage>,
) {
    let (Some(package), Some(defaults)) = (&mut member_manifest.package, workspace_package) else {
        return;
    };

    if package.version.is_empty()
        && let Some(version) = &defaults.version
    {
        package.version = version.clone();
    }
    if package.kern.is_empty()
        && let Some(kern) = &defaults.kern
    {
        package.kern = kern.clone();
    }
}

fn validate_exports(
    manifest_path: &Path,
    manifest: &Manifest,
    members: &[WorkspaceMember],
) -> Result<()> {
    let Some(workspace) = &manifest.workspace else {
        return Ok(());
    };

    let member_paths = members
        .iter()
        .map(|member| member_path(manifest_path, &member.manifest_path))
        .collect::<BTreeSet<_>>();
    for (name, export) in &workspace.exports {
        if !member_paths.contains(&export.member) {
            return Err(Error::Validation {
                path: manifest_path.to_path_buf(),
                message: format!(
                    "[workspace.exports].{name}.member `{}` is not listed in `[workspace].members`",
                    export.member
                ),
            });
        }
    }
    Ok(())
}

fn member_belongs_to_workspace(
    workspace_manifest_path: &Path,
    workspace_manifest: &Manifest,
    member_manifest_path: &Path,
) -> bool {
    let Some(workspace) = &workspace_manifest.workspace else {
        return false;
    };
    let relative = member_path(workspace_manifest_path, member_manifest_path);
    workspace.members.iter().any(|member| member == &relative)
}

fn inherited_member_manifest_for_path(member_manifest_path: &Path) -> Result<Option<Manifest>> {
    let mut current = member_manifest_path
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf);

    while let Some(dir) = current {
        let candidate = dir.join("Craft.toml");
        if candidate.is_file() {
            let workspace_manifest = Manifest::load(&candidate)?;
            workspace_manifest.validate(&candidate)?;
            if workspace_manifest.workspace.is_some() {
                for member in load_members(&candidate, &workspace_manifest)? {
                    if same_manifest_path(&member.manifest_path, member_manifest_path) {
                        return Ok(Some(member.manifest));
                    }
                }
            }
        }
        current = dir.parent().map(Path::to_path_buf);
    }

    Ok(None)
}

fn same_manifest_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn member_path(workspace_manifest_path: &Path, member_manifest_path: &Path) -> String {
    let workspace_root = workspace_manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    member_manifest_path
        .parent()
        .and_then(|dir| dir.strip_prefix(workspace_root).ok())
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| member_manifest_path.display().to_string())
}

fn export_list_suffix<'a>(exports: impl Iterator<Item = &'a String>) -> String {
    let names = exports.cloned().collect::<Vec<_>>();
    if names.is_empty() {
        return "; no exports are declared".to_string();
    }
    format!("; available exports: {}", names.join(", "))
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
name = "workspace"
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
kern = "0.7.5"
"#,
        )
        .unwrap();
        fs::write(
            tools_dir.join("Craft.toml"),
            r#"
[package]
name = "tools-demo"
version = "0.1.0"
kern = "0.7.5"
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
name = "workspace"
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

    #[test]
    fn members_inherit_workspace_package_defaults() {
        let root = temp_dir("craft-workspace-defaults");
        let member_dir = root.join("json");
        fs::create_dir_all(&member_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
name = "json-kern"
members = ["json"]

[workspace.package]
version = "0.1.0"
kern = "0.7.5"
license = "MIT"
"#,
        )
        .unwrap();
        fs::write(
            member_dir.join("Craft.toml"),
            r#"
[package]
name = "json"
"#,
        )
        .unwrap();

        let root_manifest = Manifest::load(&root.join("Craft.toml")).unwrap();
        let members = load_members(&root.join("Craft.toml"), &root_manifest).unwrap();
        let package = members[0].manifest.package.as_ref().unwrap();
        assert_eq!(package.version, "0.1.0");
        assert_eq!(package.kern, "0.7.5");

        let _ = fs::remove_dir_all(root);
    }
}
