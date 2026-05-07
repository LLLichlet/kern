use crate::error::{Error, Result};
use crate::graph;
use crate::graph::{PackageId, SourceId};
use crate::manifest::Manifest;
use crate::plan::PackagePlan;
use crate::resolver;
use crate::resolver::ResolvedGraph;
use crate::script;
use crate::script::{
    CraftScriptContext, ScriptCommand, ScriptPackage, ScriptProfile, ScriptWorkspace,
};
use crate::workspace::WorkspaceMember;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptInput {
    pub path: PathBuf,
    pub relative_path: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageElaboration {
    pub package_id: PackageId,
    pub plan: PackagePlan,
    pub script: Option<ScriptInput>,
    pub selected_features: BTreeSet<String>,
    pub profile: ScriptProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElaborationPlan {
    pub has_workspace: bool,
    pub profile_selection: script::ProfileSelection,
    pub manifest: Manifest,
    pub package_graph: graph::PackageGraph,
    pub resolved_graph: ResolvedGraph,
    pub workspace_script: Option<ScriptInput>,
    pub packages: Vec<PackageElaboration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureSelection {
    pub enable_default: bool,
    pub explicit: BTreeSet<String>,
    pub profile: script::ProfileSelection,
}

impl Default for FeatureSelection {
    fn default() -> Self {
        Self {
            enable_default: true,
            explicit: BTreeSet::new(),
            profile: script::ProfileSelection::Dev,
        }
    }
}

impl ElaborationPlan {
    pub fn package_script_count(&self) -> usize {
        self.packages
            .iter()
            .filter(|pkg| pkg.script.is_some())
            .count()
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
    _command: ScriptCommand,
    feature_selection: &FeatureSelection,
) -> Result<ElaborationPlan> {
    let workspace_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let mut workspace_script = if has_workspace {
        discover_script(workspace_root, workspace_root)?
    } else {
        None
    };
    let mut packages = Vec::new();
    if manifest.package.is_some() {
        let features = selected_features(manifest_path, manifest, feature_selection)?;
        let profile = script::manifest_profile(manifest, feature_selection.profile);
        let mut plan = PackagePlan::from_manifest(
            manifest_path,
            &root_package_id(manifest_path, manifest)?,
            manifest,
        )?;
        let package_ctx = craft_script_context(&plan, workspace_root, has_workspace);
        if let Some(workspace_script) = &mut workspace_script {
            script::apply_craft_script(&workspace_script.path, &mut plan, &package_ctx)?;
        }
        let script = if has_workspace {
            None
        } else {
            discover_script(workspace_root, workspace_root)?
        };
        let mut script = script;
        if let Some(script) = &mut script {
            script::apply_craft_script(&script.path, &mut plan, &package_ctx)?;
        }
        packages.push(PackageElaboration {
            package_id: plan.package_id.clone(),
            plan,
            script,
            selected_features: features,
            profile,
        });
    }

    for member in workspace_members {
        let package_root = member
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let features =
            selected_features(&member.manifest_path, &member.manifest, feature_selection)?;
        let profile = script::manifest_profile(&member.manifest, feature_selection.profile);
        let mut plan = PackagePlan::from_manifest(
            &member.manifest_path,
            &member_package_id(member, workspace_root)?,
            &member.manifest,
        )?;
        let package_ctx = craft_script_context(&plan, workspace_root, has_workspace);
        if let Some(workspace_script) = &mut workspace_script {
            script::apply_craft_script(&workspace_script.path, &mut plan, &package_ctx)?;
        }
        let mut script = discover_script(workspace_root, package_root)?;
        if let Some(script) = &mut script {
            script::apply_craft_script(&script.path, &mut plan, &package_ctx)?;
        }
        packages.push(PackageElaboration {
            package_id: plan.package_id.clone(),
            plan,
            script,
            selected_features: features,
            profile,
        });
    }

    let package_graph = graph::build_graph_from_plans(
        manifest_path,
        manifest,
        packages.iter().map(|pkg| &pkg.plan),
    )?;
    let resolved_graph = resolver::resolve_graph(&package_graph);

    Ok(ElaborationPlan {
        has_workspace,
        profile_selection: feature_selection.profile,
        manifest: manifest.clone(),
        package_graph,
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

fn discover_script(workspace_root: &Path, package_root: &Path) -> Result<Option<ScriptInput>> {
    let path = package_root.join("craft.rn");
    if !path.is_file() {
        return Ok(None);
    }

    script::validate_craft_script(&path)?;

    Ok(Some(ScriptInput {
        relative_path: relative_display(workspace_root, &path),
        digest: digest_file(&path)?,
        path,
    }))
}

fn selected_features(
    manifest_path: &Path,
    manifest: &Manifest,
    selection: &FeatureSelection,
) -> Result<BTreeSet<String>> {
    let mut enabled = BTreeSet::new();
    let mut pending = Vec::new();

    if selection.enable_default && manifest.features.contains_key("default") {
        pending.push("default".to_string());
    }

    for feature in &selection.explicit {
        if !manifest.features.contains_key(feature) {
            return Err(Error::Validation {
                path: manifest_path.to_path_buf(),
                message: format!("selected feature `{feature}` is not declared in `[features]`"),
            });
        }
        pending.push(feature.clone());
    }

    while let Some(feature) = pending.pop() {
        if !enabled.insert(feature.clone()) {
            continue;
        }

        let Some(members) = manifest.features.get(&feature) else {
            continue;
        };

        for member in members {
            if !manifest.features.contains_key(member) {
                return Err(Error::Validation {
                    path: manifest_path.to_path_buf(),
                    message: format!("feature `{feature}` references unknown feature `{member}`"),
                });
            }
            pending.push(member.clone());
        }
    }

    Ok(enabled)
}

fn craft_script_context(
    plan: &PackagePlan,
    workspace_root: &Path,
    has_workspace: bool,
) -> CraftScriptContext {
    let package_root = plan
        .manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    CraftScriptContext {
        package: ScriptPackage {
            name: plan.package_id.name.clone(),
            version: plan.package_id.version.clone(),
            root: script_root_display(workspace_root, package_root),
            is_root: plan.package_id.source == SourceId::Root,
        },
        workspace: ScriptWorkspace {
            root: script_root_display(workspace_root, workspace_root),
            has_workspace,
        },
    }
}

fn script_root_display(root: &Path, path: &Path) -> String {
    let display = relative_display(root, path);
    if display.is_empty() {
        ".".to_string()
    } else {
        display
    }
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
    fn discovers_workspace_and_member_craft_scripts() {
        let root = temp_dir("craft-elaborate-workspace");
        let app_dir = root.join("app");
        let tool_dir = root.join("tool");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&tool_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
members = ["app", "tool"]
"#,
        )
        .unwrap();
        fs::write(
            root.join("craft.rn"),
            "use craft.plan;\npub fn craft(p: &mut plan.Plan) void { let _ = p; }\n",
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.5"
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("craft.rn"),
            "use craft.plan;\npub fn craft(p: &mut plan.Plan) void { let _ = p; }\n",
        )
        .unwrap();
        fs::write(
            tool_dir.join("Craft.toml"),
            r#"
[package]
name = "tool"
version = "0.1.0"
kern = "0.7.5"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Check,
            &super::FeatureSelection::default(),
        )
        .unwrap();

        assert_eq!(
            elaboration
                .workspace_script
                .as_ref()
                .map(|script| script.relative_path.as_str()),
            Some("craft.rn")
        );
        assert_eq!(elaboration.package_script_count(), 1);
        assert!(elaboration.packages.iter().any(|pkg| {
            pkg.package_id.name == "app"
                && pkg
                    .script
                    .as_ref()
                    .map(|script| script.relative_path.as_str())
                    == Some("app/craft.rn")
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn applies_workspace_script_before_package_script() {
        let root = temp_dir("craft-elaborate-workspace-policy");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
members = ["app"]
"#,
        )
        .unwrap();
        fs::write(
            root.join("craft.rn"),
            r#"
use craft.plan;

pub fn craft(p: &mut plan.Plan) void {
    p.cfg_bool("workspace_policy", true);
    p.dep_git(plan.DependencyKind.normal, "log", "https://example.com/workspace-log.git");
}
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.5"

[dependencies]
log = { git = "https://example.com/log.git", tag = "v1" }
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("craft.rn"),
            r#"
use craft.plan;

pub fn craft(p: &mut plan.Plan) void {
    p.dep_git(plan.DependencyKind.normal, "log", "https://example.com/package-log.git");
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Check,
            &super::FeatureSelection::default(),
        )
        .unwrap();

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
                                crate::graph::SourceId::GitDependency { git, .. }
                                    if git == "https://example.com/package-log.git"
                            )
                )
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn treats_root_craft_script_as_package_script_without_workspace() {
        let root = temp_dir("craft-elaborate-single");
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.5"
"#,
        )
        .unwrap();
        fs::write(
            root.join("craft.rn"),
            "use craft.plan;\npub fn craft(p: &mut plan.Plan) void { let _ = p; }\n",
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Check,
            &super::FeatureSelection::default(),
        )
        .unwrap();

        assert!(elaboration.workspace_script.is_none());
        assert_eq!(elaboration.package_script_count(), 1);
        assert_eq!(
            elaboration.packages[0]
                .script
                .as_ref()
                .map(|script| script.relative_path.as_str()),
            Some("craft.rn")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn craft_scripts_can_read_package_and_workspace_metadata() {
        let root = temp_dir("craft-elaborate-metadata");
        let member_dir = root.join("member");
        fs::create_dir_all(&member_dir).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
members = ["member"]
"#,
        )
        .unwrap();
        fs::write(
            root.join("craft.rn"),
            r#"
use craft.plan;

pub fn craft(p: &mut plan.Plan) void {
    p.define_string("workspace_root", p.workspace.root);
    p.define_string("package_root", p.package.root);
    if (p.workspace.has_workspace) {
        p.cfg_bool("has_workspace", true);
    }
    p.define_string("package_name", p.package.name);
}
"#,
        )
        .unwrap();
        fs::write(
            member_dir.join("Craft.toml"),
            r#"
[package]
name = "member"
version = "0.1.0"
kern = "0.7.5"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Check,
            &super::FeatureSelection::default(),
        )
        .unwrap();

        let package = &elaboration.packages[0].plan;
        assert_eq!(
            package.cfg.get("has_workspace"),
            Some(&crate::plan::PlanValue::Bool(true))
        );
        assert_eq!(
            package.define.get("workspace_root"),
            Some(&crate::plan::PlanValue::String(".".to_string()))
        );
        assert_eq!(
            package.define.get("package_root"),
            Some(&crate::plan::PlanValue::String("member".to_string()))
        );
        assert_eq!(
            package.define.get("package_name"),
            Some(&crate::plan::PlanValue::String("member".to_string()))
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_unknown_feature_members() {
        let root = temp_dir("craft-elaborate-bad-features");
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.5"

[features]
default = ["missing"]
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let err = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Check,
            &super::FeatureSelection::default(),
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("feature `default` references unknown feature `missing`"),
            "unexpected error: {err}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn applies_package_script_mutations_to_plan() {
        let root = temp_dir("craft-elaborate-mutations");
        let vendor_trace = root.join("vendor").join("trace");
        fs::create_dir_all(&vendor_trace).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.5"

[lib]
root = "src/lib.rn"

[dependencies]
log = { git = "https://example.com/log.git", version = "1" }
trace = { git = "https://example.com/trace.git", version = "1" }
"#,
        )
        .unwrap();
        fs::write(
            root.join("craft.rn"),
            r#"
use craft.plan;

pub fn craft(p: &mut plan.Plan) void {
    p.cfg_bool("simd", true);
    p.define_string("abi", "sysv");
    p.define_string("pkg", p.package.name);
    p.set_lib_root("src/alt_lib.rn");
    p.add_bin("demo", "src/main.rn");
    p.dep_git(plan.DependencyKind.normal, "log", "https://example.com/corp-log.git");
    p.dep_path(plan.DependencyKind.normal, "trace", "vendor/trace");
    p.dep_git(plan.DependencyKind.dev, "insta", "https://example.com/insta.git");
    p.dep_version(plan.DependencyKind.dev, "insta", "2");
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Check,
            &super::FeatureSelection::default(),
        )
        .unwrap();

        let package = &elaboration.packages[0].plan;
        assert_eq!(
            package.cfg.get("simd"),
            Some(&crate::plan::PlanValue::Bool(true))
        );
        assert_eq!(
            package.define.get("abi"),
            Some(&crate::plan::PlanValue::String("sysv".to_string()))
        );
        assert_eq!(
            package.define.get("pkg"),
            Some(&crate::plan::PlanValue::String("demo".to_string()))
        );
        assert!(package.targets.iter().any(|target| {
            target.kind == crate::plan::TargetKind::Lib && target.root == "src/alt_lib.rn"
        }));
        assert!(package.targets.iter().any(|target| {
            target.kind == crate::plan::TargetKind::Bin
                && target.name.as_deref() == Some("demo")
                && target.root == "src/main.rn"
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
                                crate::graph::SourceId::GitDependency { git, .. }
                                    if git == "https://example.com/corp-log.git"
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
