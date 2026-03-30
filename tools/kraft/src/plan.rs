#![allow(dead_code)]

use crate::error::{Error, Result};
use crate::graph::PackageId;
use crate::manifest::Manifest;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagePlan {
    pub package_id: PackageId,
    pub kern: String,
    pub edition: Option<String>,
    pub publish: Option<bool>,
    pub targets: Vec<TargetPlan>,
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
            kern: package.kern.clone(),
            edition: package.edition.clone(),
            publish: package.publish,
            targets,
            cfg: BTreeMap::new(),
            define: BTreeMap::new(),
        })
    }

    pub fn target_count(&self) -> usize {
        self.targets.len()
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

#[cfg(test)]
mod tests {
    use super::{PackagePlan, PlanValue, TargetKind};
    use crate::graph::{PackageId, SourceId};
    use crate::manifest::Manifest;
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
kern = "0.7"
edition = "2027"
publish = false

[lib]
root = "src/lib.kr"

[[bin]]
name = "demo"
root = "src/main.kr"

[[test]]
name = "smoke"
root = "tests/smoke.kr"

[[example]]
name = "hello"
root = "examples/hello.kr"
"#,
            Path::new("Kraft.toml"),
        )
        .unwrap();

        let plan =
            PackagePlan::from_manifest(Path::new("Kraft.toml"), &package_id(), &manifest).unwrap();

        assert_eq!(plan.kern, "0.7");
        assert_eq!(plan.edition.as_deref(), Some("2027"));
        assert_eq!(plan.publish, Some(false));
        assert_eq!(plan.targets.len(), 4);
        assert!(
            plan.targets
                .iter()
                .any(|target| target.kind == TargetKind::Lib && target.root == "src/lib.kr")
        );
        assert!(plan.targets.iter().any(|target| {
            target.kind == TargetKind::Bin
                && target.name.as_deref() == Some("demo")
                && target.root == "src/main.kr"
        }));
    }

    #[test]
    fn mutates_cfg_define_and_targets() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"
"#,
            Path::new("Kraft.toml"),
        )
        .unwrap();

        let mut plan =
            PackagePlan::from_manifest(Path::new("Kraft.toml"), &package_id(), &manifest).unwrap();
        plan.set_cfg_bool("simd", true).unwrap();
        plan.set_cfg_string("abi", "sysv").unwrap();
        plan.set_define_bool("aggressive_checks", false).unwrap();
        plan.set_define_string("mode", "strict").unwrap();
        plan.set_lib_root("src/lib.kr").unwrap();
        plan.add_named_target(TargetKind::Bin, "demo", "src/main.kr")
            .unwrap();

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
        assert_eq!(plan.target_count(), 2);
        assert!(plan.remove_target(TargetKind::Bin, Some("demo")));
        assert_eq!(plan.target_count(), 1);
        assert!(!plan.remove_target(TargetKind::Bin, Some("demo")));
    }
}
