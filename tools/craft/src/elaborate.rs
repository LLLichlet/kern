//! Workspace elaboration from manifests into package and dependency plans.
//!
//! Elaboration applies feature selection, loads members, resolves dependencies,
//! fetches external sources, and produces the graph inputs used by build-plan
//! construction.

use crate::error::{Error, Result};
use crate::graph;
use crate::graph::{PackageId, SourceId};
use crate::manifest::Manifest;
use crate::plan::PackagePlan;
use crate::resolver;
use crate::resolver::ResolvedGraph;
use crate::script;
use crate::script::{ScriptCommand, ScriptProfile};
use crate::workspace::WorkspaceMember;
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageElaboration {
    pub package_id: PackageId,
    pub plan: PackagePlan,
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
    let _ = has_workspace;
    let mut packages = Vec::new();
    if manifest.package.is_some() {
        let features = selected_features(manifest_path, manifest, feature_selection)?;
        let profile = script::manifest_profile(manifest, feature_selection.profile);
        let plan = PackagePlan::from_manifest(
            manifest_path,
            &root_package_id(manifest_path, manifest)?,
            manifest,
        )?;
        packages.push(PackageElaboration {
            package_id: plan.package_id.clone(),
            plan,
            selected_features: features,
            profile,
        });
    }

    for member in workspace_members {
        let features =
            selected_features(&member.manifest_path, &member.manifest, feature_selection)?;
        let profile = script::manifest_profile(&member.manifest, feature_selection.profile);
        let plan = PackagePlan::from_manifest(
            &member.manifest_path,
            &member_package_id(member, workspace_root)?,
            &member.manifest,
        )?;
        packages.push(PackageElaboration {
            package_id: plan.package_id.clone(),
            plan,
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

#[cfg(test)]
mod tests {
    use super::plan;
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
    fn rejects_unknown_feature_members() {
        let root = temp_dir("craft-elaborate-bad-features");
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.9"

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
    fn workspace_members_discover_default_test_roots_independently() {
        let root = temp_dir("craft-elaborate-workspace-default-tests");
        let app_dir = root.join("app");
        let driver_dir = root.join("driver");
        fs::create_dir_all(app_dir.join("tests")).unwrap();
        fs::create_dir_all(driver_dir.join("tests")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[workspace]
name = "demo"
members = ["app", "driver"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Craft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.9"
"#,
        )
        .unwrap();
        fs::write(app_dir.join("tests/smoke.kn"), "").unwrap();
        fs::write(
            driver_dir.join("Craft.toml"),
            r#"
[package]
name = "driver"
version = "0.1.0"
kern = "0.7.9"
"#,
        )
        .unwrap();
        fs::write(driver_dir.join("tests/pci.kn"), "").unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Test,
            &super::FeatureSelection::default(),
        )
        .unwrap();

        let test_roots = elaboration
            .packages
            .iter()
            .map(|package| {
                (
                    package.package_id.name.as_str(),
                    package
                        .plan
                        .targets
                        .iter()
                        .filter(|target| target.kind == crate::plan::TargetKind::Test)
                        .map(|target| target.root.as_str())
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            test_roots,
            vec![
                ("app", vec!["tests/smoke.kn"]),
                ("driver", vec!["tests/pci.kn"])
            ]
        );

        let _ = fs::remove_dir_all(root);
    }
}
