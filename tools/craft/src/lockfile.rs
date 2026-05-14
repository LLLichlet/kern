mod build;
#[cfg(test)]
mod parse;
mod render;
#[cfg(test)]
mod validate;

use crate::elaborate::ElaborationPlan;
use crate::error::{Error, Result};
use crate::local_state;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lockfile {
    pub version: u32,
    pub manifest: String,
    pub manifest_digest: String,
    pub packages: Vec<LockedPackage>,
    pub package_targets: Vec<LockedPackageTarget>,
    pub package_resources: Vec<LockedPackageResource>,
    pub external_packages: Vec<LockedExternalPackage>,
    pub dependencies: Vec<LockedDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackage {
    pub id: String,
    pub name: String,
    pub version: String,
    pub source_kind: String,
    pub source_value: Option<String>,
    pub manifest: String,
    pub manifest_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedDependency {
    pub from: String,
    pub kind: String,
    pub name: String,
    pub package: String,
    pub target_kind: String,
    pub target_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedExternalPackage {
    pub id: String,
    pub name: String,
    pub source_kind: String,
    pub source_value: Option<String>,
    pub version: Option<String>,
    pub source_locator: Option<String>,
    pub source_selector: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackageResource {
    pub package_id: String,
    pub name: String,
    pub source_kind: String,
    pub source_value: Option<String>,
    pub source_locator: Option<String>,
    pub source_selector: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackageTarget {
    pub package_id: String,
    pub kind: String,
    pub name: Option<String>,
    pub root: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockWriteResult {
    Created,
    Updated,
    Unchanged,
}

pub fn sync_lockfile(
    manifest_path: &Path,
    elaboration: &ElaborationPlan,
) -> Result<(PathBuf, LockWriteResult)> {
    let lock_path = elaboration.resolved_graph.workspace_root.join("Craft.lock");
    let rendered = render_lockfile(manifest_path, elaboration)?;

    if lock_path.is_file() {
        let actual =
            fs::read_to_string(&lock_path).map_err(|err| Error::from_io(&lock_path, err))?;
        if actual == rendered {
            return Ok((lock_path, LockWriteResult::Unchanged));
        }
    }

    let result = if lock_path.is_file() {
        LockWriteResult::Updated
    } else {
        LockWriteResult::Created
    };

    local_state::write_file_atomic(&lock_path, rendered)?;
    Ok((lock_path, result))
}

pub fn check_lockfile_current(
    manifest_path: &Path,
    elaboration: &ElaborationPlan,
) -> Result<(PathBuf, LockWriteResult)> {
    let lock_path = elaboration.resolved_graph.workspace_root.join("Craft.lock");
    let rendered = render_lockfile(manifest_path, elaboration)?;
    if !lock_path.is_file() {
        return Ok((lock_path, LockWriteResult::Created));
    }
    let actual = fs::read_to_string(&lock_path).map_err(|err| Error::from_io(&lock_path, err))?;
    if actual == rendered {
        Ok((lock_path, LockWriteResult::Unchanged))
    } else {
        Ok((lock_path, LockWriteResult::Updated))
    }
}

fn render_lockfile(manifest_path: &Path, elaboration: &ElaborationPlan) -> Result<String> {
    Ok(Lockfile::from_elaboration(manifest_path, elaboration)?.render())
}

#[cfg(test)]
impl Lockfile {
    pub fn load(path: &Path) -> Result<Self> {
        let source = fs::read_to_string(path).map_err(|err| Error::from_io(path, err))?;
        let lockfile = Self::parse(&source, path)?;
        lockfile.validate(path)?;
        Ok(lockfile)
    }
}

#[cfg(test)]
mod tests {
    use super::{LockWriteResult, Lockfile, sync_lockfile};
    use crate::elaborate::plan;
    use crate::manifest::Manifest;
    use crate::workspace::load_members;
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
    fn renders_workspace_lockfile_from_package_graph() {
        let root = temp_dir("craft-lockfile");
        let app_dir = root.join("app");
        let util_dir = root.join("util");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&util_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
name = "workspace"
members = ["app", "util"]

[workspace.dependencies]
shared = { git = "https://example.com/shared.git", branch = "stable", version = "2" }
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.6"

[[bin]]
name = "app"
root = "src/main.kn"

[dependencies]
util = { path = "../util" }
shared = { workspace = true, features = ["simd"] }

[resources]
limine = { git = "https://example.com/limine.git", branch = "main" }
"#,
        )
        .unwrap();
        fs::write(
            util_dir.join("Craft.toml"),
            r#"
[package]
name = "util"
version = "0.1.0"
kern = "0.7.6"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &root_manifest,
            &members,
            true,
            crate::script::ScriptCommand::Check,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let lockfile = Lockfile::from_elaboration(&manifest_path, &elaboration).unwrap();
        let rendered = lockfile.render();

        assert!(rendered.contains("version = 1"));
        assert!(rendered.contains("[[package]]"));
        assert!(rendered.contains("[[package-target]]"));
        assert!(rendered.contains("[[package-resource]]"));
        assert!(rendered.contains("[[external-package]]"));
        assert!(rendered.contains("id = \"app 0.1.0 workspace-member:app\""));
        assert!(
            rendered.contains("package = \"app 0.1.0 workspace-member:app\"")
                && rendered.contains("kind = \"bin\"")
        );
        assert!(!rendered.contains("workspace-script"));
        assert!(!rendered.contains("craft-script"));
        assert!(rendered.contains("name = \"limine\""));
        assert!(rendered.contains("source-locator = \"https://example.com/limine.git\""));
        assert!(rendered.contains("source-selector = \"branch:main\""));
        assert!(rendered.contains("target-id = \"util 0.1.0 workspace-member:util\""));
        assert!(rendered.contains("name = \"shared\""));
        assert!(rendered.contains("target = \"external\""));
        assert!(
            rendered.contains("id = \"shared 2 git:https://example.com/shared.git#branch:stable\"")
        );
        assert!(rendered.contains("source-locator = \"https://example.com/shared.git\""));
        assert!(rendered.contains("source-selector = \"branch:stable\""));
        assert!(rendered.contains("manifest-digest = \"fnv1a64:"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn loads_rendered_lockfile_roundtrip() {
        let root = temp_dir("craft-lockfile-load");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
name = "workspace"
members = ["app"]

[workspace.dependencies]
shared = { git = "https://example.com/shared.git", tag = "v2", version = "2" }
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.6"

[dependencies]
shared = { workspace = true }
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &root_manifest,
            &members,
            true,
            crate::script::ScriptCommand::Check,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let expected = Lockfile::from_elaboration(&manifest_path, &elaboration).unwrap();
        let (lock_path, _) = sync_lockfile(&manifest_path, &elaboration).unwrap();
        let loaded = Lockfile::load(&lock_path).unwrap();

        assert_eq!(loaded, expected);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn writes_lockfile_into_workspace_root() {
        let root = temp_dir("craft-lockfile-write");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
name = "workspace"
members = ["app"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.6"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &root_manifest,
            &members,
            true,
            crate::script::ScriptCommand::Check,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let (lock_path, _) = sync_lockfile(&manifest_path, &elaboration).unwrap();
        let contents = fs::read_to_string(&lock_path).unwrap();

        assert_eq!(lock_path, root.join("Craft.lock"));
        assert!(contents.contains("manifest = \"Craft.toml\""));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sync_lockfile_reports_created_updated_and_unchanged() {
        let root = temp_dir("craft-lockfile-sync");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
name = "workspace"
members = ["app"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.6"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &root_manifest,
            &members,
            true,
            crate::script::ScriptCommand::Check,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();

        let (_, created) = sync_lockfile(&manifest_path, &elaboration).unwrap();
        assert_eq!(created, LockWriteResult::Created);

        let (_, unchanged) = sync_lockfile(&manifest_path, &elaboration).unwrap();
        assert_eq!(unchanged, LockWriteResult::Unchanged);

        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.2.0"
kern = "0.7.6"
"#,
        )
        .unwrap();

        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &root_manifest,
            &members,
            true,
            crate::script::ScriptCommand::Check,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let (_, updated) = sync_lockfile(&manifest_path, &elaboration).unwrap();
        assert_eq!(updated, LockWriteResult::Updated);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sync_lockfile_overwrites_invalid_existing_contents() {
        let root = temp_dir("craft-lockfile-invalid-sync");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
name = "workspace"
members = ["app"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.6"
"#,
        )
        .unwrap();
        fs::write(root.join("Craft.lock"), "not valid lockfile\n").unwrap();

        let manifest_path = root.join("Craft.toml");
        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &root_manifest,
            &members,
            true,
            crate::script::ScriptCommand::Check,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();

        let (lock_path, result) = sync_lockfile(&manifest_path, &elaboration).unwrap();
        assert_eq!(result, LockWriteResult::Updated);
        let loaded = Lockfile::load(&lock_path).unwrap();
        assert_eq!(
            loaded.packages[0].id,
            "app 0.1.0 workspace-member:app".to_string()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sync_lockfile_updates_when_git_source_identity_changes() {
        let root = temp_dir("craft-lockfile-source-identity");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
name = "workspace"
members = ["app"]

[workspace.dependencies]
shared = { git = "https://example.com/shared.git", branch = "stable", version = "2" }
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.6"

[dependencies]
shared = { workspace = true }
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &root_manifest,
            &members,
            true,
            crate::script::ScriptCommand::Check,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();

        let (_, created) = sync_lockfile(&manifest_path, &elaboration).unwrap();
        assert_eq!(created, LockWriteResult::Created);

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
name = "workspace"
members = ["app"]

[workspace.dependencies]
shared = { git = "https://example.com/shared.git", rev = "abc123", version = "2" }
"#,
        )
        .unwrap();

        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &root_manifest,
            &members,
            true,
            crate::script::ScriptCommand::Check,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();

        let (_, updated) = sync_lockfile(&manifest_path, &elaboration).unwrap();
        assert_eq!(updated, LockWriteResult::Updated);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_invalid_dependency_target_reference() {
        let root = temp_dir("craft-lockfile-invalid");
        let lock_path = root.join("Craft.lock");
        fs::write(
            &lock_path,
            r#"
version = 1
manifest = "Craft.toml"
manifest-digest = "fnv1a64:1234567890abcdef"

[[package]]
id = "app 0.1.0 workspace-member:app"
name = "app"
version = "0.1.0"
source = "workspace-member"
source-value = "app"
manifest = "app/Craft.toml"
manifest-digest = "fnv1a64:1234567890abcdef"

[[external-package]]
id = "util 0.1.0 git:https://example.com/util.git#tag:v1"
name = "util"
source = "git"
source-value = "https://example.com/util.git"
version = "0.1.0"
source-locator = "https://example.com/util.git"
source-selector = "tag:v1"

[[dependency]]
from = "app 0.1.0 workspace-member:app"
kind = "normal"
name = "util"
package = "util"
target = "external"
target-id = "missing 0.1.0 git:https://example.com/missing.git#tag:v1"
"#,
        )
        .unwrap();

        let err = Lockfile::load(&lock_path).unwrap_err();
        assert!(err.to_string().contains("unknown external target id"));

        let _ = fs::remove_dir_all(root);
    }
}
