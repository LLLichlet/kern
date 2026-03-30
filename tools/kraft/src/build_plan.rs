use crate::elaborate::ElaborationPlan;
use crate::error::Result;
use crate::graph::PackageId;
use crate::plan::{PlanValue, TargetKind};
use crate::resolver::{ExternalPackageId, ResolvedDependencyTarget};
use crate::script;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub workspace_root: PathBuf,
    pub packages: Vec<PackageBuildPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageBuildPlan {
    pub package_id: PackageId,
    pub manifest_path: PathBuf,
    pub build_script: Option<BuildScriptInput>,
    pub units: Vec<BuildUnit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildUnit {
    pub package_id: PackageId,
    pub target_kind: TargetKind,
    pub target_name: Option<String>,
    pub source_root: String,
    pub artifact_kind: ArtifactKind,
    pub artifact_name: String,
    pub local_dependencies: Vec<PackageId>,
    pub external_dependencies: Vec<ExternalPackageId>,
    pub profile: script::ScriptProfile,
    pub cfg: BTreeMap<String, PlanValue>,
    pub define: BTreeMap<String, PlanValue>,
    pub link: LinkPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    Library,
    Executable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildScriptInput {
    pub path: PathBuf,
    pub relative_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LinkPlan {
    pub system_libs: Vec<String>,
    pub frameworks: Vec<String>,
    pub search_paths: Vec<String>,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionPlan {
    pub compile_actions: Vec<CompileAction>,
    pub link_actions: Vec<LinkAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileAction {
    pub package_id: PackageId,
    pub target_kind: TargetKind,
    pub target_name: Option<String>,
    pub artifact_name: String,
    pub source_path: PathBuf,
    pub object_path: PathBuf,
    pub artifact_path: PathBuf,
    pub profile: script::ScriptProfile,
    pub cfg: BTreeMap<String, PlanValue>,
    pub define: BTreeMap<String, PlanValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkAction {
    pub package_id: PackageId,
    pub target_kind: TargetKind,
    pub target_name: Option<String>,
    pub artifact_name: String,
    pub artifact_path: PathBuf,
    pub primary_object: PathBuf,
    pub local_library_objects: Vec<PathBuf>,
    pub external_dependencies: Vec<ExternalPackageId>,
    pub link: LinkPlan,
}

impl ActionPlan {
    pub fn compile_count(&self) -> usize {
        self.compile_actions.len()
    }

    pub fn link_count(&self) -> usize {
        self.link_actions.len()
    }
}

impl BuildPlan {
    pub fn unit_count(&self) -> usize {
        self.packages
            .iter()
            .map(|package| package.units.len())
            .sum()
    }

    pub fn local_dependency_edge_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.local_dependencies.len())
            .sum()
    }

    pub fn external_dependency_edge_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.external_dependencies.len())
            .sum()
    }

    pub fn build_script_count(&self) -> usize {
        self.packages
            .iter()
            .filter(|package| package.build_script.is_some())
            .count()
    }

    pub fn link_system_lib_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.link.system_libs.len())
            .sum()
    }

    pub fn link_framework_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.link.frameworks.len())
            .sum()
    }

    pub fn link_search_path_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.link.search_paths.len())
            .sum()
    }

    pub fn link_arg_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| &package.units)
            .map(|unit| unit.link.args.len())
            .sum()
    }

    pub fn derive_actions(&self, target: &script::ScriptTarget) -> ActionPlan {
        let mut compile_actions = Vec::new();

        for package in &self.packages {
            let package_root = package
                .manifest_path
                .parent()
                .unwrap_or_else(|| Path::new("."));
            for unit in &package.units {
                compile_actions.push(CompileAction {
                    package_id: unit.package_id.clone(),
                    target_kind: unit.target_kind,
                    target_name: unit.target_name.clone(),
                    artifact_name: unit.artifact_name.clone(),
                    source_path: package_root.join(&unit.source_root),
                    object_path: object_path(
                        &self.workspace_root,
                        &unit.package_id,
                        &unit.profile.name,
                        unit.target_kind,
                        &unit.artifact_name,
                    ),
                    artifact_path: artifact_path(
                        &self.workspace_root,
                        target,
                        &unit.package_id,
                        &unit.profile.name,
                        unit.target_kind,
                        &unit.artifact_name,
                    ),
                    profile: unit.profile.clone(),
                    cfg: unit.cfg.clone(),
                    define: unit.define.clone(),
                });
            }
        }

        let mut package_lib_objects = BTreeMap::new();
        for action in &compile_actions {
            if action.target_kind == TargetKind::Lib {
                package_lib_objects.insert(action.package_id.clone(), action.object_path.clone());
            }
        }

        let mut link_actions = Vec::new();
        for package in &self.packages {
            for unit in &package.units {
                if unit.artifact_kind != ArtifactKind::Executable {
                    continue;
                }

                link_actions.push(LinkAction {
                    package_id: unit.package_id.clone(),
                    target_kind: unit.target_kind,
                    target_name: unit.target_name.clone(),
                    artifact_name: unit.artifact_name.clone(),
                    artifact_path: artifact_path(
                        &self.workspace_root,
                        target,
                        &unit.package_id,
                        &unit.profile.name,
                        unit.target_kind,
                        &unit.artifact_name,
                    ),
                    primary_object: object_path(
                        &self.workspace_root,
                        &unit.package_id,
                        &unit.profile.name,
                        unit.target_kind,
                        &unit.artifact_name,
                    ),
                    local_library_objects: unit
                        .local_dependencies
                        .iter()
                        .filter_map(|package_id| package_lib_objects.get(package_id).cloned())
                        .collect(),
                    external_dependencies: unit.external_dependencies.clone(),
                    link: unit.link.clone(),
                });
            }
        }

        ActionPlan {
            compile_actions,
            link_actions,
        }
    }
}

pub fn derive(
    elaboration: &ElaborationPlan,
    command: crate::script::ScriptCommand,
) -> Result<BuildPlan> {
    let mut packages = Vec::new();
    let host_target = script::host_target();

    for package in &elaboration.resolved_graph.packages {
        let package_elaboration = elaboration
            .packages
            .iter()
            .find(|entry| entry.package_id == package.id)
            .expect("elaboration must contain package plan");
        let package_root = package_elaboration
            .plan
            .manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let build_script =
            discover_build_script(&elaboration.resolved_graph.workspace_root, package_root)?;
        let mut local_dependencies = Vec::new();
        let mut external_dependencies = Vec::new();
        for dependency in &package.dependencies {
            match &dependency.target {
                ResolvedDependencyTarget::Local(target) => {
                    if !local_dependencies.contains(target) {
                        local_dependencies.push(target.clone());
                    }
                }
                ResolvedDependencyTarget::External(target) => {
                    if !external_dependencies.contains(target) {
                        external_dependencies.push(target.clone());
                    }
                }
            }
        }

        let mut units = Vec::new();
        for target in &package_elaboration.plan.targets {
            units.push(BuildUnit {
                package_id: package.id.clone(),
                target_kind: target.kind,
                target_name: target.name.clone(),
                source_root: target.root.clone(),
                artifact_kind: artifact_kind(target.kind),
                artifact_name: artifact_name(&package.id, target.kind, target.name.as_deref()),
                local_dependencies: local_dependencies.clone(),
                external_dependencies: external_dependencies.clone(),
                profile: package_elaboration.profile.clone(),
                cfg: package_elaboration.plan.cfg.clone(),
                define: package_elaboration.plan.define.clone(),
                link: LinkPlan::default(),
            });
        }

        if let Some(build_script) = &build_script {
            for unit in &mut units {
                let build_context = script::BuildScriptContext {
                    script: script::ScriptContext {
                        package: script::ScriptPackage {
                            name: package.id.name.clone(),
                            version: package.id.version.clone(),
                            root: relative_display(
                                &elaboration.resolved_graph.workspace_root,
                                package_root,
                            ),
                            is_root: package.id.source == crate::graph::SourceId::Root,
                        },
                        workspace: script::ScriptWorkspace {
                            root: relative_display(
                                &elaboration.resolved_graph.workspace_root,
                                &elaboration.resolved_graph.workspace_root,
                            ),
                            has_workspace: elaboration.has_workspace,
                        },
                        target: host_target.clone(),
                        profile: package_elaboration.profile.clone(),
                        command,
                        features: package_elaboration.selected_features.clone(),
                        env: std::collections::BTreeMap::new(),
                    },
                    unit: script::BuildScriptUnit {
                        target_kind: unit.target_kind,
                        target_name: unit.target_name.clone(),
                        source_root: unit.source_root.clone(),
                        artifact_name: unit.artifact_name.clone(),
                    },
                };
                script::apply_build_script(&build_script.path, unit, &build_context)?;
            }
        }

        packages.push(PackageBuildPlan {
            package_id: package.id.clone(),
            manifest_path: package_elaboration.plan.manifest_path.clone(),
            build_script,
            units,
        });
    }

    Ok(BuildPlan {
        workspace_root: elaboration.resolved_graph.workspace_root.clone(),
        packages,
    })
}

fn discover_build_script(
    workspace_root: &Path,
    package_root: &Path,
) -> Result<Option<BuildScriptInput>> {
    let path = package_root.join("build.kr");
    if !path.is_file() {
        return Ok(None);
    }

    script::validate_build_script(&path)?;

    Ok(Some(BuildScriptInput {
        relative_path: relative_display(workspace_root, &path),
        path,
    }))
}

fn artifact_kind(kind: TargetKind) -> ArtifactKind {
    match kind {
        TargetKind::Lib => ArtifactKind::Library,
        TargetKind::Bin | TargetKind::Test | TargetKind::Example => ArtifactKind::Executable,
    }
}

fn object_path(
    workspace_root: &Path,
    package_id: &PackageId,
    profile: &str,
    kind: TargetKind,
    artifact_name: &str,
) -> PathBuf {
    workspace_root
        .join(".kraft")
        .join("build")
        .join(profile)
        .join("obj")
        .join(package_dir_name(package_id))
        .join(kind.as_str())
        .join(format!("{artifact_name}.o"))
}

fn artifact_path(
    workspace_root: &Path,
    target: &script::ScriptTarget,
    package_id: &PackageId,
    profile: &str,
    kind: TargetKind,
    artifact_name: &str,
) -> PathBuf {
    let file_name = match kind {
        TargetKind::Lib => format!("lib{artifact_name}.o"),
        TargetKind::Bin | TargetKind::Test | TargetKind::Example => {
            if target.os == script::ScriptOs::Windows {
                format!("{artifact_name}.exe")
            } else {
                artifact_name.to_string()
            }
        }
    };

    workspace_root
        .join(".kraft")
        .join("build")
        .join(profile)
        .join("out")
        .join(package_dir_name(package_id))
        .join(kind.as_str())
        .join(file_name)
}

fn package_dir_name(package_id: &PackageId) -> String {
    format!("{}-{}", package_id.name, package_id.version)
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

fn artifact_name(package_id: &PackageId, kind: TargetKind, target_name: Option<&str>) -> String {
    match kind {
        TargetKind::Lib => package_id.name.clone(),
        TargetKind::Bin | TargetKind::Test | TargetKind::Example => target_name
            .expect("named targets must provide a name")
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{ArtifactKind, derive};
    use crate::elaborate::plan;
    use crate::manifest::Manifest;
    use crate::plan::TargetKind;
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
    fn derives_workspace_build_units_from_package_targets() {
        let root = temp_dir("kraft-build-plan-targets");
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
            app_dir.join("Kraft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[lib]
root = "src/lib.kr"

[[bin]]
name = "app"
root = "src/main.kr"

[[test]]
name = "smoke"
root = "tests/smoke.kr"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Check,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let build_plan = derive(&elaboration, crate::script::ScriptCommand::Check).unwrap();
        let actions = build_plan.derive_actions(&crate::script::host_target());

        assert_eq!(build_plan.unit_count(), 3);
        assert_eq!(actions.compile_count(), 3);
        assert_eq!(actions.link_count(), 2);
        let app_package = build_plan
            .packages
            .iter()
            .find(|package| package.package_id.name == "app")
            .unwrap();
        assert!(app_package.units.iter().any(|unit| {
            unit.target_kind == TargetKind::Lib
                && unit.artifact_kind == ArtifactKind::Library
                && unit.artifact_name == "app"
        }));
        assert!(app_package.units.iter().any(|unit| {
            unit.target_kind == TargetKind::Bin
                && unit.artifact_kind == ArtifactKind::Executable
                && unit.artifact_name == "app"
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn carries_local_and_external_dependencies_into_build_units() {
        let root = temp_dir("kraft-build-plan-deps");
        let app_dir = root.join("app");
        let util_dir = root.join("util");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&util_dir).unwrap();

        fs::write(
            root.join("Kraft.toml"),
            r#"
[workspace]
members = ["app", "util"]
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

[[bin]]
name = "app"
root = "src/main.kr"

[dependencies]
util = { path = "../util" }
log = "1"
"#,
        )
        .unwrap();
        fs::write(
            util_dir.join("Kraft.toml"),
            r#"
[package]
name = "util"
version = "0.1.0"
kern = "0.7"

[lib]
root = "src/lib.kr"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Check,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let build_plan = derive(&elaboration, crate::script::ScriptCommand::Check).unwrap();
        let actions = build_plan.derive_actions(&crate::script::host_target());

        let app_unit = build_plan
            .packages
            .iter()
            .find(|package| package.package_id.name == "app")
            .unwrap()
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Bin)
            .unwrap();
        assert_eq!(app_unit.local_dependencies.len(), 1);
        assert_eq!(app_unit.local_dependencies[0].name, "util");
        assert_eq!(app_unit.external_dependencies.len(), 1);
        assert_eq!(app_unit.external_dependencies[0].package_name, "log");
        assert_eq!(build_plan.local_dependency_edge_count(), 1);
        assert_eq!(build_plan.external_dependency_edge_count(), 1);
        let link = actions
            .link_actions
            .iter()
            .find(|action| action.package_id.name == "app" && action.target_kind == TargetKind::Bin)
            .unwrap();
        assert_eq!(link.local_library_objects.len(), 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn applies_build_script_link_directives_per_unit() {
        let root = temp_dir("kraft-build-plan-script");
        fs::write(
            root.join("Kraft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"

[features]
default = ["simd"]
simd = []

[[bin]]
name = "demo"
root = "src/main.kr"

[[test]]
name = "smoke"
root = "tests/smoke.kr"
"#,
        )
        .unwrap();
        fs::write(
            root.join("build.kr"),
            r#"
use kraft.builder;

pub fn build(b: *mut builder.Builder) void {
    if (b.feature_enabled("simd")) {
        b.link_arg("-flto");
    }

    match (b.target.os) {
        .windows => b.link_system_lib("ws2_32"),
        .linux => {},
        .darwin => {},
        .unknown => {},
    }

    match (b.unit.kind) {
        .bin => b.link_framework("Security"),
        .test => b.link_search("native/test"),
        .lib => {},
        .example => {},
    }
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Build,
            &crate::elaborate::FeatureSelection::default(),
        )
        .unwrap();
        let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let actions = build_plan.derive_actions(&crate::script::host_target());
        let package = build_plan
            .packages
            .iter()
            .find(|package| package.package_id.name == "demo")
            .unwrap();
        assert_eq!(
            package
                .build_script
                .as_ref()
                .map(|script| script.relative_path.as_str()),
            Some("build.kr")
        );

        let bin = package
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Bin)
            .unwrap();
        assert!(bin.link.args.iter().any(|arg| arg == "-flto"));
        assert!(bin.link.frameworks.iter().any(|name| name == "Security"));

        let test = package
            .units
            .iter()
            .find(|unit| unit.target_kind == TargetKind::Test)
            .unwrap();
        assert!(test.link.args.iter().any(|arg| arg == "-flto"));
        assert!(
            test.link
                .search_paths
                .iter()
                .any(|path| path == "native/test")
        );
        let bin_action = actions
            .link_actions
            .iter()
            .find(|action| {
                action.package_id.name == "demo" && action.target_kind == TargetKind::Bin
            })
            .unwrap();
        assert!(
            bin_action
                .link
                .frameworks
                .iter()
                .any(|name| name == "Security")
        );
        assert!(bin_action.link.args.iter().any(|arg| arg == "-flto"));
        let test_action = actions
            .link_actions
            .iter()
            .find(|action| {
                action.package_id.name == "demo" && action.target_kind == TargetKind::Test
            })
            .unwrap();
        assert!(
            test_action
                .link
                .search_paths
                .iter()
                .any(|path| path == "native/test")
        );

        let _ = fs::remove_dir_all(root);
    }
}
