use crate::error::{Error, Result};
use crate::graph;
use crate::graph::{PackageId, SourceId};
use crate::manifest::Manifest;
use crate::plan::PackagePlan;
use crate::resolver;
use crate::resolver::ResolvedGraph;
use crate::script;
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
    manifest_path: &Path,
    manifest: &Manifest,
    workspace_members: &[WorkspaceMember],
    has_workspace: bool,
) -> Result<ElaborationPlan> {
    let workspace_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let workspace_script = if has_workspace {
        discover_script(
            workspace_root,
            workspace_root,
            manifest.kraft_env_names(),
            workspace_root,
        )?
    } else {
        None
    };

    let mut packages = Vec::new();
    if manifest.package.is_some() {
        let mut plan = PackagePlan::from_manifest(
            manifest_path,
            &root_package_id(manifest_path, manifest)?,
            manifest,
        )?;
        if let Some(workspace_script) = &workspace_script {
            script::apply_kraft_script(&workspace_script.path, &mut plan)?;
        }
        let script = if has_workspace {
            None
        } else {
            discover_script(
                workspace_root,
                workspace_root,
                manifest.kraft_env_names(),
                manifest_path,
            )?
        };
        if let Some(script) = &script {
            script::apply_kraft_script(&script.path, &mut plan)?;
        }
        packages.push(PackageElaboration {
            package_id: plan.package_id.clone(),
            plan,
            script,
        });
    }

    for member in workspace_members {
        let package_root = member
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let mut plan = PackagePlan::from_manifest(
            &member.manifest_path,
            &member_package_id(member, workspace_root)?,
            &member.manifest,
        )?;
        if let Some(workspace_script) = &workspace_script {
            script::apply_kraft_script(&workspace_script.path, &mut plan)?;
        }
        let script = discover_script(
            workspace_root,
            package_root,
            member.manifest.kraft_env_names(),
            &member.manifest_path,
        )?;
        if let Some(script) = &script {
            script::apply_kraft_script(&script.path, &mut plan)?;
        }
        packages.push(PackageElaboration {
            package_id: plan.package_id.clone(),
            plan,
            script,
        });
    }

    let package_graph = graph::build_graph_from_plans(
        manifest_path,
        manifest,
        packages.iter().map(|pkg| &pkg.plan),
    )?;
    let resolved_graph = resolver::resolve_graph(&package_graph);

    Ok(ElaborationPlan {
        resolved_graph,
        workspace_script,
        packages,
    })
}

fn root_package_id(manifest_path: &Path, manifest: &Manifest) -> Result<PackageId> {
    let Some(package) = &manifest.package else {
        return Err(Error::Validation {
            path: manifest_path.to_path_buf(),
            message: "package elaboration requires `[package]`".to_string(),
        });
    };

    Ok(PackageId {
        name: package.name.clone(),
        version: package.version.clone(),
        source: SourceId::Root,
    })
}

fn member_package_id(member: &WorkspaceMember, workspace_root: &Path) -> Result<PackageId> {
    let Some(package) = &member.manifest.package else {
        return Err(Error::Validation {
            path: member.manifest_path.clone(),
            message: "workspace members must declare `[package]`".to_string(),
        });
    };

    let relative = member
        .manifest_path
        .parent()
        .and_then(|dir| dir.strip_prefix(workspace_root).ok())
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| member.manifest_path.display().to_string());

    Ok(PackageId {
        name: package.name.clone(),
        version: package.version.clone(),
        source: SourceId::WorkspaceMember { path: relative },
    })
}

fn discover_script(
    workspace_root: &Path,
    package_root: &Path,
    env_names: &[String],
    manifest_path: &Path,
) -> Result<Option<ScriptInput>> {
    let path = package_root.join("kraft.kr");
    if !path.is_file() {
        return Ok(None);
    }

    script::validate_kraft_script(&path)?;

    Ok(Some(ScriptInput {
        relative_path: relative_display(workspace_root, &path),
        digest: digest_file(&path)?,
        env_inputs: snapshot_env_inputs(env_names, manifest_path)?,
        path,
    }))
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
    use crate::manifest::Manifest;
    use crate::resolver::ResolvedDependencyTarget;
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
        fs::write(
            root.join("kraft.kr"),
            "use kraft.plan;\npub fn kraft(p: *mut plan.Plan) void { let _ = p; }\n",
        )
        .unwrap();
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
        fs::write(
            app_dir.join("kraft.kr"),
            "use kraft.plan;\npub fn kraft(p: *mut plan.Plan) void { let _ = p; }\n",
        )
        .unwrap();
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
        let elaboration = plan(&manifest_path, &manifest, &members, true).unwrap();

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
    fn applies_workspace_script_before_package_script() {
        let root = temp_dir("kraft-elaborate-workspace-policy");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Kraft.toml"),
            r#"
[workspace]
members = ["app"]
"#,
        )
        .unwrap();
        fs::write(
            root.join("kraft.kr"),
            r#"
use kraft.plan;

pub fn kraft(p: *mut plan.Plan) void {
    p.cfg_bool("workspace_policy", true);
    p.dep_registry(plan.DependencyKind.{ normal }, "log", "workspace");
}
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Kraft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[dependencies]
log = "1"
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("kraft.kr"),
            r#"
use kraft.plan;

pub fn kraft(p: *mut plan.Plan) void {
    p.dep_registry(plan.DependencyKind.{ normal }, "log", "package");
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(&manifest_path, &manifest, &members, true).unwrap();

        let package = &elaboration.packages[0].plan;
        assert_eq!(
            package.cfg.get("workspace_policy"),
            Some(&crate::plan::PlanValue::Bool(true))
        );
        let resolved = &elaboration.resolved_graph.packages[0];
        assert!(resolved.dependencies.iter().any(|dep| {
            dep.dependency_name == "log"
                && matches!(
                    &dep.target,
                    ResolvedDependencyTarget::External(target)
                        if target.package_name == "log"
                            && matches!(
                                &target.source,
                                crate::graph::SourceId::Registry { name }
                                    if name.as_deref() == Some("package")
                            )
                )
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
        fs::write(
            root.join("kraft.kr"),
            "use kraft.plan;\npub fn kraft(p: *mut plan.Plan) void { let _ = p; }\n",
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(&manifest_path, &manifest, &[], false).unwrap();

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
        fs::write(
            root.join("kraft.kr"),
            "use kraft.plan;\npub fn kraft(p: *mut plan.Plan) void { let _ = p; }\n",
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(&manifest_path, &manifest, &[], false).unwrap();

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

    #[test]
    fn applies_package_script_mutations_to_plan() {
        let root = temp_dir("kraft-elaborate-mutations");
        let vendor_trace = root.join("vendor").join("trace");
        fs::create_dir_all(&vendor_trace).unwrap();
        fs::write(
            root.join("Kraft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"

[lib]
root = "src/lib.kr"

[dependencies]
log = "1"
trace = "1"
"#,
        )
        .unwrap();
        fs::write(
            root.join("kraft.kr"),
            r#"
use kraft.plan;

pub fn kraft(p: *mut plan.Plan) void {
    p.cfg_bool("simd", true);
    p.define_string("abi", "sysv");
    p.set_lib_root("src/alt_lib.kr");
    p.add_bin("demo", "src/main.kr");
    p.dep_registry(plan.DependencyKind.{ normal }, "log", "corp");
    p.dep_path(plan.DependencyKind.{ normal }, "trace", "vendor/trace");
    p.dep_version(plan.DependencyKind.{ dev }, "insta", "2");
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(&manifest_path, &manifest, &[], false).unwrap();

        let package = &elaboration.packages[0].plan;
        assert_eq!(
            package.cfg.get("simd"),
            Some(&crate::plan::PlanValue::Bool(true))
        );
        assert_eq!(
            package.define.get("abi"),
            Some(&crate::plan::PlanValue::String("sysv".to_string()))
        );
        assert!(package.targets.iter().any(|target| {
            target.kind == crate::plan::TargetKind::Lib && target.root == "src/alt_lib.kr"
        }));
        assert!(package.targets.iter().any(|target| {
            target.kind == crate::plan::TargetKind::Bin
                && target.name.as_deref() == Some("demo")
                && target.root == "src/main.kr"
        }));
        let resolved = &elaboration.resolved_graph.packages[0];
        assert!(resolved.dependencies.iter().any(|dep| {
            dep.dependency_name == "log"
                && matches!(
                    &dep.target,
                    ResolvedDependencyTarget::External(target)
                        if target.package_name == "log"
                            && target.version.as_deref() == Some("1")
                            && matches!(
                                &target.source,
                                crate::graph::SourceId::Registry { name }
                                    if name.as_deref() == Some("corp")
                            )
                )
        }));
        assert!(resolved.dependencies.iter().any(|dep| {
            dep.dependency_name == "trace"
                && matches!(
                    &dep.target,
                    ResolvedDependencyTarget::External(target)
                        if target.package_name == "trace"
                            && target.version.as_deref() == Some("1")
                            && matches!(
                                &target.source,
                                crate::graph::SourceId::PathDependency { path }
                                    if path == "vendor/trace"
                            )
                )
        }));
        assert!(resolved.dependencies.iter().any(|dep| {
            dep.dependency_name == "insta"
                && matches!(
                    &dep.target,
                    ResolvedDependencyTarget::External(target)
                        if target.package_name == "insta"
                            && target.version.as_deref() == Some("2")
                )
        }));

        let _ = fs::remove_dir_all(root);
    }
}
