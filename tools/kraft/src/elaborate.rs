use crate::error::{Error, Result};
use crate::graph::{PackageId, SourceId};
use crate::manifest::Manifest;
use crate::plan::PackagePlan;
use crate::resolver::ResolvedGraph;
use crate::workspace::WorkspaceMember;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvInput {
    pub name: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptInput {
    pub path: PathBuf,
    pub relative_path: String,
    pub digest: String,
    pub env_inputs: Vec<EnvInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageElaboration {
    pub package_id: PackageId,
    pub plan: PackagePlan,
    pub script: Option<ScriptInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElaborationPlan {
    pub resolved_graph: ResolvedGraph,
    pub workspace_script: Option<ScriptInput>,
    pub packages: Vec<PackageElaboration>,
}

impl ElaborationPlan {
    pub fn package_script_count(&self) -> usize {
        self.packages
            .iter()
            .filter(|pkg| pkg.script.is_some())
            .count()
    }

    pub fn workspace_env_input_count(&self) -> usize {
        self.workspace_script
            .as_ref()
            .map(|script| script.env_inputs.len())
            .unwrap_or(0)
    }

    pub fn package_env_input_count(&self) -> usize {
        self.packages
            .iter()
            .map(|pkg| {
                pkg.script
                    .as_ref()
                    .map(|script| script.env_inputs.len())
                    .unwrap_or(0)
            })
            .sum()
    }

    pub fn package_target_count(&self) -> usize {
        self.packages
            .iter()
            .map(|pkg| pkg.plan.target_count())
            .sum()
    }
}

pub fn plan(
    _manifest_path: &Path,
    manifest: &Manifest,
    workspace_members: &[WorkspaceMember],
    has_workspace: bool,
    resolved_graph: &ResolvedGraph,
) -> Result<ElaborationPlan> {
    let workspace_root = &resolved_graph.workspace_root;
    let workspace_script = if has_workspace {
        discover_script(
            workspace_root,
            workspace_root,
            manifest.kraft_env_names(),
            workspace_root,
        )
    } else {
        None
    };

    let mut packages = Vec::new();
    for package in &resolved_graph.packages {
        let package_root = package
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let package_manifest = if package.id.source == SourceId::Root {
            manifest
        } else {
            &workspace_members
                .iter()
                .find(|member| member.manifest_path == package.manifest_path)
                .expect("workspace member manifest must exist")
                .manifest
        };
        let plan =
            PackagePlan::from_manifest(&package.manifest_path, &package.id, package_manifest)?;
        let script = if package.id.source == SourceId::Root && has_workspace {
            None
        } else {
            discover_script(
                workspace_root,
                package_root,
                package_manifest.kraft_env_names(),
                &package.manifest_path,
            )
        };
        packages.push(PackageElaboration {
            package_id: package.id.clone(),
            plan,
            script,
        });
    }

    Ok(ElaborationPlan {
        resolved_graph: resolved_graph.clone(),
        workspace_script,
        packages,
    })
}

fn discover_script(
    workspace_root: &Path,
    package_root: &Path,
    env_names: &[String],
    manifest_path: &Path,
) -> Option<ScriptInput> {
    let path = package_root.join("kraft.kr");
    if !path.is_file() {
        return None;
    }

    Some(ScriptInput {
        relative_path: relative_display(workspace_root, &path),
        digest: digest_file(&path).ok()?,
        env_inputs: snapshot_env_inputs(env_names, manifest_path).ok()?,
        path,
    })
}

fn snapshot_env_inputs(env_names: &[String], manifest_path: &Path) -> Result<Vec<EnvInput>> {
    let mut inputs = Vec::new();
    for name in env_names {
        let value = match std::env::var(name) {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(Error::Validation {
                    path: manifest_path.to_path_buf(),
                    message: format!(
                        "[kraft].env `{name}` produced a non-UTF-8 value, which `kraft.kr` does not accept"
                    ),
                });
            }
        };
        inputs.push(EnvInput {
            name: name.clone(),
            value,
        });
    }
    Ok(inputs)
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

fn digest_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).map_err(|err| Error::from_io(path, err))?;
    Ok(format!("fnv1a64:{:016x}", fnv1a64(&bytes)))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    let mut hash = OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::plan;
    use crate::graph::build_graph;
    use crate::manifest::Manifest;
    use crate::resolver::resolve_graph;
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
    fn discovers_workspace_and_member_kraft_scripts() {
        let root = temp_dir("kraft-elaborate-workspace");
        let app_dir = root.join("app");
        let tool_dir = root.join("tool");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&tool_dir).unwrap();

        fs::write(
            root.join("Kraft.toml"),
            r#"
[workspace]
members = ["app", "tool"]
"#,
        )
        .unwrap();
        fs::write(root.join("kraft.kr"), "pub fn kraft() void {}\n").unwrap();
        fs::write(
            app_dir.join("Kraft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"
"#,
        )
        .unwrap();
        fs::write(app_dir.join("kraft.kr"), "pub fn kraft() void {}\n").unwrap();
        fs::write(
            tool_dir.join("Kraft.toml"),
            r#"
[package]
name = "tool"
version = "0.1.0"
kern = "0.7"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &manifest).unwrap();
        let graph = build_graph(&manifest_path, &manifest, &members).unwrap();
        let resolved = resolve_graph(&graph);
        let elaboration = plan(&manifest_path, &manifest, &members, true, &resolved).unwrap();

        assert_eq!(
            elaboration
                .workspace_script
                .as_ref()
                .map(|script| script.relative_path.as_str()),
            Some("kraft.kr")
        );
        assert_eq!(elaboration.package_script_count(), 1);
        assert!(elaboration.packages.iter().any(|pkg| {
            pkg.package_id.name == "app"
                && pkg
                    .script
                    .as_ref()
                    .map(|script| script.relative_path.as_str())
                    == Some("app/kraft.kr")
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn treats_root_kraft_script_as_package_script_without_workspace() {
        let root = temp_dir("kraft-elaborate-single");
        fs::write(
            root.join("Kraft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"
"#,
        )
        .unwrap();
        fs::write(root.join("kraft.kr"), "pub fn kraft() void {}\n").unwrap();

        let manifest_path = root.join("Kraft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let graph = build_graph(&manifest_path, &manifest, &[]).unwrap();
        let resolved = resolve_graph(&graph);
        let elaboration = plan(&manifest_path, &manifest, &[], false, &resolved).unwrap();

        assert!(elaboration.workspace_script.is_none());
        assert_eq!(elaboration.package_script_count(), 1);
        assert_eq!(
            elaboration.packages[0]
                .script
                .as_ref()
                .map(|script| script.relative_path.as_str()),
            Some("kraft.kr")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn snapshots_declared_env_inputs() {
        let root = temp_dir("kraft-elaborate-env");
        let env_name = format!(
            "KRAFT_TEST_ENV_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        unsafe { std::env::set_var(&env_name, "enabled") };

        fs::write(
            root.join("Kraft.toml"),
            format!(
                r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"

[kraft]
env = ["{env_name}"]
"#
            ),
        )
        .unwrap();
        fs::write(root.join("kraft.kr"), "pub fn kraft() void {}\n").unwrap();

        let manifest_path = root.join("Kraft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let graph = build_graph(&manifest_path, &manifest, &[]).unwrap();
        let resolved = resolve_graph(&graph);
        let elaboration = plan(&manifest_path, &manifest, &[], false, &resolved).unwrap();

        assert_eq!(
            elaboration.packages[0].script.as_ref().unwrap().env_inputs,
            vec![super::EnvInput {
                name: env_name.clone(),
                value: Some("enabled".to_string()),
            }]
        );

        unsafe { std::env::remove_var(&env_name) };
        let _ = fs::remove_dir_all(root);
    }
}
