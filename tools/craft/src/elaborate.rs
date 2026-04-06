use crate::error::{Error, Result};
use crate::graph;
use crate::graph::{PackageId, SourceId};
use crate::manifest::Manifest;
use crate::plan::PackagePlan;
use crate::resolver;
use crate::resolver::ResolvedGraph;
use crate::script;
use crate::script::{
    ScriptCommand, ScriptContext, ScriptExecution, ScriptPackage, ScriptProfile, ScriptTarget,
    ScriptWorkspace,
};
use crate::workspace::WorkspaceMember;
use std::collections::{BTreeMap, BTreeSet};
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
    pub selected_features: BTreeSet<String>,
    pub profile: ScriptProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElaborationPlan {
    pub has_workspace: bool,
    pub profile_selection: script::ProfileSelection,
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
    command: ScriptCommand,
    feature_selection: &FeatureSelection,
) -> Result<ElaborationPlan> {
    let workspace_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let mut workspace_script = if has_workspace {
        discover_script(workspace_root, workspace_root)?
    } else {
        None
    };
    let workspace_env = declared_env_map(manifest_path, manifest.craft_env_names())?;
    let host = script::host_target();
    let target = host.clone();

    let mut packages = Vec::new();
    if manifest.package.is_some() {
        let features = selected_features(manifest_path, manifest, feature_selection)?;
        let mut plan = PackagePlan::from_manifest(
            manifest_path,
            &root_package_id(manifest_path, manifest)?,
            manifest,
        )?;
        let package_ctx = script_context(
            &plan,
            workspace_root,
            has_workspace,
            command,
            &host,
            &target,
            &script::manifest_profile(manifest, feature_selection.profile),
            features,
            declared_env_map(manifest_path, manifest.craft_env_names())?,
        );
        if let Some(workspace_script) = &mut workspace_script {
            let execution = script::apply_craft_script(
                &workspace_script.path,
                &mut plan,
                &ScriptContext {
                    env: workspace_env.clone(),
                    ..package_ctx.clone()
                },
            )?;
            merge_script_env_inputs(&mut workspace_script.env_inputs, execution);
        }
        let script = if has_workspace {
            None
        } else {
            discover_script(workspace_root, workspace_root)?
        };
        let mut script = script;
        if let Some(script) = &mut script {
            let execution = script::apply_craft_script(&script.path, &mut plan, &package_ctx)?;
            script.env_inputs = execution
                .env_inputs
                .into_iter()
                .map(into_env_input)
                .collect();
        }
        packages.push(PackageElaboration {
            package_id: plan.package_id.clone(),
            plan,
            script,
            selected_features: package_ctx.features.clone(),
            profile: package_ctx.profile.clone(),
        });
    }

    for member in workspace_members {
        let package_root = member
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let features =
            selected_features(&member.manifest_path, &member.manifest, feature_selection)?;
        let mut plan = PackagePlan::from_manifest(
            &member.manifest_path,
            &member_package_id(member, workspace_root)?,
            &member.manifest,
        )?;
        let package_ctx = script_context(
            &plan,
            workspace_root,
            has_workspace,
            command,
            &host,
            &target,
            &script::manifest_profile(&member.manifest, feature_selection.profile),
            features,
            declared_env_map(&member.manifest_path, member.manifest.craft_env_names())?,
        );
        if let Some(workspace_script) = &mut workspace_script {
            let execution = script::apply_craft_script(
                &workspace_script.path,
                &mut plan,
                &ScriptContext {
                    env: workspace_env.clone(),
                    ..package_ctx.clone()
                },
            )?;
            merge_script_env_inputs(&mut workspace_script.env_inputs, execution);
        }
        let mut script = discover_script(workspace_root, package_root)?;
        if let Some(script) = &mut script {
            let execution = script::apply_craft_script(&script.path, &mut plan, &package_ctx)?;
            script.env_inputs = execution
                .env_inputs
                .into_iter()
                .map(into_env_input)
                .collect();
        }
        packages.push(PackageElaboration {
            package_id: plan.package_id.clone(),
            plan,
            script,
            selected_features: package_ctx.features.clone(),
            profile: package_ctx.profile.clone(),
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
        env_inputs: Vec::new(),
        path,
    }))
}

fn declared_env_map(
    env_names_path: &Path,
    env_names: &[String],
) -> Result<BTreeMap<String, Option<String>>> {
    let mut inputs = BTreeMap::new();
    for name in env_names {
        let value = match std::env::var(name) {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(Error::Validation {
                    path: env_names_path.to_path_buf(),
                    message: format!(
                        "[craft].env `{name}` produced a non-UTF-8 value, which `craft.rn` does not accept"
                    ),
                });
            }
        };
        inputs.insert(name.clone(), value);
    }
    Ok(inputs)
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

#[allow(clippy::too_many_arguments)]
fn script_context(
    plan: &PackagePlan,
    workspace_root: &Path,
    has_workspace: bool,
    command: ScriptCommand,
    host: &ScriptTarget,
    target: &ScriptTarget,
    profile: &ScriptProfile,
    features: BTreeSet<String>,
    env: BTreeMap<String, Option<String>>,
) -> ScriptContext {
    let package_root = plan
        .manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    ScriptContext {
        package: ScriptPackage {
            name: plan.package_id.name.clone(),
            version: plan.package_id.version.clone(),
            root: relative_display(workspace_root, package_root),
            is_root: plan.package_id.source == SourceId::Root,
        },
        workspace: ScriptWorkspace {
            root: relative_display(workspace_root, workspace_root),
            has_workspace,
        },
        host: host.clone(),
        target: target.clone(),
        profile: profile.clone(),
        command,
        features,
        env,
    }
}

fn merge_script_env_inputs(into: &mut Vec<EnvInput>, execution: ScriptExecution) {
    for input in execution.env_inputs {
        if !into.iter().any(|existing| existing.name == input.name) {
            into.push(into_env_input(input));
        }
    }
    into.sort_by(|lhs, rhs| lhs.name.cmp(&rhs.name));
}

fn into_env_input(input: script::ScriptEnvInput) -> EnvInput {
    EnvInput {
        name: input.name,
        value: input.value,
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
    use crate::script::ScriptOs;
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

    fn os_variant_name(os: ScriptOs) -> &'static str {
        match os {
            ScriptOs::Unknown => "unknown",
            ScriptOs::Linux => "linux",
            ScriptOs::Windows => "windows",
            ScriptOs::Darwin => "darwin",
        }
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
            "use craft.plan;\npub fn craft(p: *mut plan.Plan) void { let _ = p; }\n",
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.6.7"
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("craft.rn"),
            "use craft.plan;\npub fn craft(p: *mut plan.Plan) void { let _ = p; }\n",
        )
        .unwrap();
        fs::write(
            tool_dir.join("Craft.toml"),
            r#"
[package]
name = "tool"
version = "0.1.0"
kern = "0.6.7"
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

pub fn craft(p: *mut plan.Plan) void {
    p.cfg_bool("workspace_policy", true);
    p.dep_git(plan.DependencyKind.{ normal }, "log", "https://example.com/workspace-log.git");
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
kern = "0.6.7"

[dependencies]
log = { git = "https://example.com/log.git", tag = "v1" }
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("craft.rn"),
            r#"
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {
    p.dep_git(plan.DependencyKind.{ normal }, "log", "https://example.com/package-log.git");
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
kern = "0.6.7"
"#,
        )
        .unwrap();
        fs::write(
            root.join("craft.rn"),
            "use craft.plan;\npub fn craft(p: *mut plan.Plan) void { let _ = p; }\n",
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
    fn craft_script_receives_host_and_target_context() {
        let root = temp_dir("craft-elaborate-host-target");
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.7"
"#,
        )
        .unwrap();
        fs::write(
            root.join("craft.rn"),
            r#"
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {
    p.cfg_string("host_arch", p.host.arch);
    p.define_string("target_arch", p.target.arch);
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

        assert_eq!(
            elaboration.packages[0].plan.cfg.get("host_arch"),
            Some(&crate::plan::PlanValue::String(
                crate::script::host_target().arch.to_string()
            ))
        );
        assert_eq!(
            elaboration.packages[0].plan.define.get("target_arch"),
            Some(&crate::plan::PlanValue::String(
                crate::script::host_target().arch.to_string()
            ))
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pure_enum_equality_checks_work_in_craft_scripts() {
        let root = temp_dir("craft-elaborate-enum-equality");
        let os_variant = os_variant_name(crate::script::host_target().os);
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.7"
"#,
        )
        .unwrap();
        fs::write(
            root.join("craft.rn"),
            format!(
                r#"
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {{
    if (p.target.os == .{os_variant}) {{
        p.cfg_bool("target_os_match", true);
    }}

    if (p.command == .check) {{
        p.define_bool("check_mode", true);
    }}
}}
"#
            ),
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

        assert_eq!(
            elaboration.packages[0].plan.cfg.get("target_os_match"),
            Some(&crate::plan::PlanValue::Bool(true))
        );
        assert_eq!(
            elaboration.packages[0].plan.define.get("check_mode"),
            Some(&crate::plan::PlanValue::Bool(true))
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn records_only_env_inputs_read_by_craft_script() {
        let root = temp_dir("craft-elaborate-env");
        let env_name = format!(
            "KRAFT_TEST_ENV_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        unsafe { std::env::set_var(&env_name, "enabled") };

        fs::write(
            root.join("Craft.toml"),
            format!(
                r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.7"

[craft]
env = ["{env_name}"]
"#
            ),
        )
        .unwrap();
        fs::write(
            root.join("craft.rn"),
            format!(
                r#"
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {{
    match (p.env("{env_name}")) {{
        .{{ Some: value }} => p.define_string("env_value", value),
        .None => {{}},
    }}
}}
"#
            ),
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

        assert_eq!(
            elaboration.packages[0].script.as_ref().unwrap().env_inputs,
            vec![super::EnvInput {
                name: env_name.clone(),
                value: Some("enabled".to_string()),
            }]
        );
        assert_eq!(
            elaboration.packages[0].plan.define.get("env_value"),
            Some(&crate::plan::PlanValue::String("enabled".to_string()))
        );

        unsafe { std::env::remove_var(&env_name) };
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn env_presence_checks_work_with_none_comparisons() {
        let root = temp_dir("craft-elaborate-env-none-compare");
        let env_name = format!(
            "KRAFT_COMPARE_ENV_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        unsafe { std::env::set_var(&env_name, "enabled") };

        fs::write(
            root.join("Craft.toml"),
            format!(
                r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.7"

[craft]
env = ["{env_name}"]
"#
            ),
        )
        .unwrap();
        fs::write(
            root.join("craft.rn"),
            format!(
                r#"
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {{
    if (p.env("{env_name}") != .None) {{
        p.cfg_bool("env_present", true);
    }}
}}
"#
            ),
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

        assert_eq!(
            elaboration.packages[0].plan.cfg.get("env_present"),
            Some(&crate::plan::PlanValue::Bool(true))
        );

        unsafe { std::env::remove_var(&env_name) };
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_env_reads_outside_declared_allowlist() {
        let root = temp_dir("craft-elaborate-env-undeclared");
        let env_name = format!(
            "KRAFT_UNDECLARED_ENV_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        unsafe { std::env::set_var(&env_name, "enabled") };

        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.7"
"#,
        )
        .unwrap();
        fs::write(
            root.join("craft.rn"),
            format!(
                r#"
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {{
    match (p.env("{env_name}")) {{
        .{{ Some: value }} => p.define_string("env_value", value),
        .None => {{}},
    }}
}}
"#
            ),
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
                .contains("was not declared under `[craft].env`"),
            "unexpected error: {err}"
        );

        unsafe { std::env::remove_var(&env_name) };
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn enables_transitive_default_features_for_craft_scripts() {
        let root = temp_dir("craft-elaborate-features");
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.7"

[features]
default = ["tls"]
tls = ["simd"]
simd = []
"#,
        )
        .unwrap();
        fs::write(
            root.join("craft.rn"),
            r#"
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {
    if (p.feature_enabled("simd")) {
        p.cfg_bool("simd", true);
    }
    if (p.feature_enabled("tls")) {
        p.define_bool("tls", true);
    }
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

        assert_eq!(
            elaboration.packages[0].plan.cfg.get("simd"),
            Some(&crate::plan::PlanValue::Bool(true))
        );
        assert_eq!(
            elaboration.packages[0].plan.define.get("tls"),
            Some(&crate::plan::PlanValue::Bool(true))
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
kern = "0.6.7"

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
kern = "0.6.7"

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

pub fn craft(p: *mut plan.Plan) void {
    p.cfg_bool("simd", true);
    p.define_string("abi", "sysv");
    p.define_string("pkg", p.package.name);
    match (p.command) {
        .check => p.define_bool("is_check", true),
        .lock => {},
        .fetch => {},
        .build => {},
        .run => {},
        .test => {},
    }
    p.set_lib_root("src/alt_lib.rn");
    p.add_bin("demo", "src/main.rn");
    p.dep_git(plan.DependencyKind.{ normal }, "log", "https://example.com/corp-log.git");
    p.dep_path(plan.DependencyKind.{ normal }, "trace", "vendor/trace");
    p.dep_git(plan.DependencyKind.{ dev }, "insta", "https://example.com/insta.git");
    p.dep_version(plan.DependencyKind.{ dev }, "insta", "2");
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
        assert_eq!(
            package.define.get("is_check"),
            Some(&crate::plan::PlanValue::Bool(true))
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
