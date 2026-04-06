#![allow(dead_code)]

use crate::error::{Error, Result};
use crate::graph::{DependencyKind, PackageId};
use crate::manifest::{DependencySpec, DetailedDependency, Manifest};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagePlan {
    pub package_id: PackageId,
    pub manifest_path: PathBuf,
    pub kern: String,
    pub publish: Option<bool>,
    pub targets: Vec<TargetPlan>,
    pub dependencies: BTreeMap<String, DependencySpec>,
    pub dev_dependencies: BTreeMap<String, DependencySpec>,
    pub build_dependencies: BTreeMap<String, DependencySpec>,
    pub cfg: BTreeMap<String, PlanValue>,
    pub define: BTreeMap<String, PlanValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetPlan {
    pub kind: TargetKind,
    pub name: Option<String>,
    pub root: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TargetKind {
    Lib,
    Bin,
    Test,
    Example,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanValue {
    Bool(bool),
    String(String),
}

impl PackagePlan {
    pub fn from_manifest(
        manifest_path: &Path,
        package_id: &PackageId,
        manifest: &Manifest,
    ) -> Result<Self> {
        let Some(package) = &manifest.package else {
            return Err(Error::Validation {
                path: manifest_path.to_path_buf(),
                message: "package plan construction requires `[package]`".to_string(),
            });
        };

        let mut targets = Vec::new();
        if let Some(lib) = &manifest.lib {
            targets.push(TargetPlan {
                kind: TargetKind::Lib,
                name: None,
                root: lib.root.clone(),
            });
        }
        for target in &manifest.bin {
            targets.push(TargetPlan {
                kind: TargetKind::Bin,
                name: Some(target.name.clone()),
                root: target.root.clone(),
            });
        }
        for target in &manifest.test {
            targets.push(TargetPlan {
                kind: TargetKind::Test,
                name: Some(target.name.clone()),
                root: target.root.clone(),
            });
        }
        for target in &manifest.example {
            targets.push(TargetPlan {
                kind: TargetKind::Example,
                name: Some(target.name.clone()),
                root: target.root.clone(),
            });
        }

        Ok(Self {
            package_id: package_id.clone(),
            manifest_path: manifest_path.to_path_buf(),
            kern: package.kern.clone(),
            publish: package.publish,
            targets,
            dependencies: manifest.dependencies.clone(),
            dev_dependencies: manifest.dev_dependencies.clone(),
            build_dependencies: manifest.build_dependencies.clone(),
            cfg: BTreeMap::new(),
            define: BTreeMap::new(),
        })
    }

    pub fn target_count(&self) -> usize {
        self.targets.len()
    }

    pub fn dependency_count(&self, kind: DependencyKind) -> usize {
        self.dependencies(kind).len()
    }

    pub fn dependencies(&self, kind: DependencyKind) -> &BTreeMap<String, DependencySpec> {
        match kind {
            DependencyKind::Normal => &self.dependencies,
            DependencyKind::Dev => &self.dev_dependencies,
            DependencyKind::Build => &self.build_dependencies,
        }
    }

    pub fn set_cfg_bool(&mut self, name: &str, value: bool) -> Result<()> {
        self.set_cfg(name, PlanValue::Bool(value))
    }

    pub fn set_cfg_string(&mut self, name: &str, value: impl Into<String>) -> Result<()> {
        self.set_cfg(name, PlanValue::String(value.into()))
    }

    pub fn set_define_bool(&mut self, name: &str, value: bool) -> Result<()> {
        self.set_define(name, PlanValue::Bool(value))
    }

    pub fn set_define_string(&mut self, name: &str, value: impl Into<String>) -> Result<()> {
        self.set_define(name, PlanValue::String(value.into()))
    }

    pub fn set_lib_root(&mut self, root: impl Into<String>) -> Result<()> {
        let root = normalize_non_empty(root.into(), "target root")?;
        if let Some(target) = self
            .targets
            .iter_mut()
            .find(|target| target.kind == TargetKind::Lib)
        {
            target.root = root;
            return Ok(());
        }

        self.targets.push(TargetPlan {
            kind: TargetKind::Lib,
            name: None,
            root,
        });
        Ok(())
    }

    pub fn add_named_target(
        &mut self,
        kind: TargetKind,
        name: impl Into<String>,
        root: impl Into<String>,
    ) -> Result<()> {
        if kind == TargetKind::Lib {
            return Err(Error::Usage(
                "lib targets must be updated via `set_lib_root`".to_string(),
            ));
        }

        let name = normalize_non_empty(name.into(), "target name")?;
        let root = normalize_non_empty(root.into(), "target root")?;
        if self
            .targets
            .iter()
            .any(|target| target.kind == kind && target.name.as_deref() == Some(name.as_str()))
        {
            return Err(Error::Usage(format!(
                "duplicate {:?} target `{name}` in package plan",
                kind
            )));
        }

        self.targets.push(TargetPlan {
            kind,
            name: Some(name),
            root,
        });
        Ok(())
    }

    pub fn add_test_target(&mut self, root: impl Into<String>) -> Result<()> {
        let root = normalize_non_empty(root.into(), "test root")?;
        let name = test_target_name(&root)?;
        if self.targets.iter().any(|target| {
            target.kind == TargetKind::Test && target.name.as_deref() == Some(name.as_str())
        }) {
            return Err(Error::Usage(format!(
                "duplicate Test target `{name}` in package plan"
            )));
        }

        self.targets.push(TargetPlan {
            kind: TargetKind::Test,
            name: Some(name),
            root,
        });
        Ok(())
    }

    pub fn remove_target(&mut self, kind: TargetKind, name: Option<&str>) -> bool {
        let original_len = self.targets.len();
        self.targets.retain(|target| {
            !(target.kind == kind
                && match (target.name.as_deref(), name) {
                    (None, None) => true,
                    (Some(lhs), Some(rhs)) => lhs == rhs,
                    _ => false,
                })
        });
        self.targets.len() != original_len
    }

    pub fn remove_test_target(&mut self, root: &str) -> bool {
        let Ok(name) = test_target_name(root) else {
            return false;
        };
        self.remove_target(TargetKind::Test, Some(&name))
    }

    pub fn set_dependency_version(
        &mut self,
        kind: DependencyKind,
        name: &str,
        version: impl Into<String>,
    ) -> Result<()> {
        let name = normalize_non_empty(name.to_string(), "dependency name")?;
        let version = normalize_non_empty(version.into(), "dependency version")?;
        match self.dependencies_mut(kind).entry(name) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(DependencySpec::Version(version));
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => match entry.get_mut() {
                DependencySpec::Version(current) => *current = version,
                DependencySpec::Detailed(dep) => {
                    dep.version = Some(version);
                    dep.workspace = None;
                }
            },
        }
        Ok(())
    }

    pub fn set_dependency_path(
        &mut self,
        kind: DependencyKind,
        name: &str,
        path: impl Into<String>,
    ) -> Result<()> {
        let name = normalize_non_empty(name.to_string(), "dependency name")?;
        let path = normalize_non_empty(path.into(), "dependency path")?;
        let dep = self.promote_dependency(kind, name);
        dep.path = Some(path);
        dep.workspace = None;
        Ok(())
    }

    pub fn set_dependency_registry(
        &mut self,
        kind: DependencyKind,
        name: &str,
        registry: impl Into<String>,
    ) -> Result<()> {
        let name = normalize_non_empty(name.to_string(), "dependency name")?;
        let registry = normalize_non_empty(registry.into(), "dependency registry")?;
        let dep = self.promote_dependency(kind, name);
        dep.registry = Some(registry);
        dep.workspace = None;
        Ok(())
    }

    pub fn use_workspace_dependency(&mut self, kind: DependencyKind, name: &str) -> Result<()> {
        let name = normalize_non_empty(name.to_string(), "dependency name")?;
        let dep = self.promote_dependency(kind, name);
        dep.version = None;
        dep.path = None;
        dep.registry = None;
        dep.workspace = Some(true);
        Ok(())
    }

    pub fn remove_dependency(&mut self, kind: DependencyKind, name: &str) -> Result<bool> {
        let name = normalize_non_empty(name.to_string(), "dependency name")?;
        Ok(self.dependencies_mut(kind).remove(&name).is_some())
    }

    fn set_cfg(&mut self, name: &str, value: PlanValue) -> Result<()> {
        let name = normalize_non_empty(name.to_string(), "cfg name")?;
        self.cfg.insert(name, value);
        Ok(())
    }

    fn set_define(&mut self, name: &str, value: PlanValue) -> Result<()> {
        let name = normalize_non_empty(name.to_string(), "define name")?;
        self.define.insert(name, value);
        Ok(())
    }

    fn dependencies_mut(&mut self, kind: DependencyKind) -> &mut BTreeMap<String, DependencySpec> {
        match kind {
            DependencyKind::Normal => &mut self.dependencies,
            DependencyKind::Dev => &mut self.dev_dependencies,
            DependencyKind::Build => &mut self.build_dependencies,
        }
    }

    fn promote_dependency(
        &mut self,
        kind: DependencyKind,
        name: String,
    ) -> &mut DetailedDependency {
        let spec = self
            .dependencies_mut(kind)
            .entry(name)
            .or_insert_with(|| DependencySpec::Detailed(DetailedDependency::default()));
        promote_dependency_spec(spec)
    }
}

impl TargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetKind::Lib => "lib",
            TargetKind::Bin => "bin",
            TargetKind::Test => "test",
            TargetKind::Example => "example",
        }
    }
}

fn normalize_non_empty(value: String, field: &str) -> Result<String> {
    if value.trim().is_empty() {
        return Err(Error::Usage(format!("{field} must not be empty")));
    }
    Ok(value)
}

fn test_target_name(root: &str) -> Result<String> {
    let path = Path::new(root);
    let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return Err(Error::Usage(format!(
            "test root `{root}` must end in a UTF-8 file name"
        )));
    };
    normalize_non_empty(name.to_string(), "test target name")
}

fn promote_dependency_spec(spec: &mut DependencySpec) -> &mut DetailedDependency {
    if let DependencySpec::Version(version) = spec.clone() {
        *spec = DependencySpec::Detailed(DetailedDependency {
            version: Some(version),
            ..DetailedDependency::default()
        });
    }

    match spec {
        DependencySpec::Detailed(dep) => dep,
        DependencySpec::Version(_) => unreachable!("dependency spec promotion must succeed"),
    }
}

#[cfg(test)]
mod tests {
    use super::{PackagePlan, PlanValue, TargetKind};
    use crate::graph::{DependencyKind, PackageId, SourceId};
    use crate::manifest::{DependencySpec, Manifest};
    use std::path::Path;

    fn package_id() -> PackageId {
        PackageId {
            name: "demo".to_string(),
            version: "0.1.0".to_string(),
            source: SourceId::Root,
        }
    }

    #[test]
    fn builds_package_plan_from_manifest_targets() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"
publish = false

[lib]
root = "src/lib.rn"

[[bin]]
name = "demo"
root = "src/main.rn"

[test]
roots = ["tests/smoke.rn"]

[[example]]
name = "hello"
root = "examples/hello.rn"
"#,
            Path::new("Craft.toml"),
        )
        .unwrap();

        let plan =
            PackagePlan::from_manifest(Path::new("Craft.toml"), &package_id(), &manifest).unwrap();

        assert_eq!(plan.kern, "0.6.6");
        assert_eq!(plan.manifest_path, Path::new("Craft.toml"));
        assert_eq!(plan.publish, Some(false));
        assert_eq!(plan.targets.len(), 4);
        assert!(
            plan.targets
                .iter()
                .any(|target| target.kind == TargetKind::Lib && target.root == "src/lib.rn")
        );
        assert!(plan.targets.iter().any(|target| {
            target.kind == TargetKind::Bin
                && target.name.as_deref() == Some("demo")
                && target.root == "src/main.rn"
        }));
        assert!(plan.targets.iter().any(|target| {
            target.kind == TargetKind::Test
                && target.name.as_deref() == Some("smoke")
                && target.root == "tests/smoke.rn"
        }));
    }

    #[test]
    fn mutates_cfg_define_and_targets() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"
"#,
            Path::new("Craft.toml"),
        )
        .unwrap();

        let mut plan =
            PackagePlan::from_manifest(Path::new("Craft.toml"), &package_id(), &manifest).unwrap();
        plan.set_cfg_bool("simd", true).unwrap();
        plan.set_cfg_string("abi", "sysv").unwrap();
        plan.set_define_bool("aggressive_checks", false).unwrap();
        plan.set_define_string("mode", "strict").unwrap();
        plan.set_lib_root("src/lib.rn").unwrap();
        plan.add_named_target(TargetKind::Bin, "demo", "src/main.rn")
            .unwrap();
        plan.add_test_target("tests/smoke.rn").unwrap();

        assert_eq!(plan.cfg.get("simd"), Some(&PlanValue::Bool(true)));
        assert_eq!(
            plan.cfg.get("abi"),
            Some(&PlanValue::String("sysv".to_string()))
        );
        assert_eq!(
            plan.define.get("aggressive_checks"),
            Some(&PlanValue::Bool(false))
        );
        assert_eq!(
            plan.define.get("mode"),
            Some(&PlanValue::String("strict".to_string()))
        );
        assert_eq!(plan.target_count(), 3);
        assert!(plan.remove_test_target("tests/smoke.rn"));
        assert!(!plan.remove_test_target("tests/smoke.rn"));
        assert!(plan.remove_target(TargetKind::Bin, Some("demo")));
        assert_eq!(plan.target_count(), 1);
        assert!(!plan.remove_target(TargetKind::Bin, Some("demo")));
    }

    #[test]
    fn mutates_dependencies_across_dependency_kinds() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"

[dependencies]
log = "1"
"#,
            Path::new("Craft.toml"),
        )
        .unwrap();

        let mut plan =
            PackagePlan::from_manifest(Path::new("Craft.toml"), &package_id(), &manifest).unwrap();
        plan.set_dependency_path(DependencyKind::Normal, "log", "../vendor/log")
            .unwrap();
        plan.set_dependency_registry(DependencyKind::Normal, "log", "corp")
            .unwrap();
        plan.set_dependency_version(DependencyKind::Dev, "insta", "2")
            .unwrap();
        plan.use_workspace_dependency(DependencyKind::Build, "cc")
            .unwrap();

        match plan.dependencies(DependencyKind::Normal).get("log") {
            Some(DependencySpec::Detailed(dep)) => {
                assert_eq!(dep.version.as_deref(), Some("1"));
                assert_eq!(dep.path.as_deref(), Some("../vendor/log"));
                assert_eq!(dep.registry.as_deref(), Some("corp"));
                assert_eq!(dep.workspace, None);
            }
            other => panic!("expected detailed dependency, got {other:?}"),
        }

        assert_eq!(
            plan.dependencies(DependencyKind::Dev).get("insta"),
            Some(&DependencySpec::Version("2".to_string()))
        );
        assert_eq!(plan.dependency_count(DependencyKind::Build), 1);

        match plan.dependencies(DependencyKind::Build).get("cc") {
            Some(DependencySpec::Detailed(dep)) => {
                assert_eq!(dep.workspace, Some(true));
                assert_eq!(dep.version, None);
                assert_eq!(dep.path, None);
                assert_eq!(dep.registry, None);
            }
            other => panic!("expected workspace dependency, got {other:?}"),
        }
    }
}
