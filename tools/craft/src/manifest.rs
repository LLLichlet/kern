mod parse;
#[cfg(test)]
mod tests;
mod validate;

use crate::error::{Error, Result};
use crate::plan::TargetKind;
use kernc_utils::config::{CompileOptions, LibraryBundle, RuntimeEntry};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub package: Option<Package>,
    pub craft: Option<CraftConfig>,
    pub runtime: Option<RuntimeConfig>,
    pub lib: Option<LibTarget>,
    pub bin: Vec<NamedTarget>,
    pub test: Vec<NamedTarget>,
    pub example: Vec<NamedTarget>,
    pub dependencies: BTreeMap<String, DependencySpec>,
    pub dev_dependencies: BTreeMap<String, DependencySpec>,
    pub build_dependencies: BTreeMap<String, DependencySpec>,
    pub features: BTreeMap<String, Vec<String>>,
    pub profile: Option<Profiles>,
    pub workspace: Option<Workspace>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub kern: String,
    pub publish: Option<bool>,
    pub description: Option<String>,
    pub license: Option<String>,
    pub authors: Vec<String>,
    pub readme: Option<String>,
    pub repository: Option<String>,
    pub homepage: Option<String>,
    pub documentation: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CraftConfig {
    pub env: Vec<String>,
    pub release_source_policy: Option<ReleaseSourcePolicy>,
    pub allow_floating_git: Vec<String>,
    pub allow_insecure_source: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseSourcePolicy {
    Enforce,
    Warn,
    Off,
}

impl ReleaseSourcePolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enforce => "enforce",
            Self::Warn => "warn",
            Self::Off => "off",
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub entry: Option<RuntimeEntry>,
    pub libc: Option<bool>,
    pub bundle: Option<LibraryBundle>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LibTarget {
    pub root: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NamedTarget {
    pub name: String,
    pub root: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencySpec {
    Version(String),
    Detailed(DetailedDependency),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DetailedDependency {
    pub version: Option<String>,
    pub path: Option<String>,
    pub git: Option<String>,
    pub rev: Option<String>,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub workspace: Option<bool>,
    pub package: Option<String>,
    pub optional: Option<bool>,
    pub default_features: Option<bool>,
    pub features: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Profiles {
    pub dev: Option<Profile>,
    pub release: Option<Profile>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Profile {
    pub opt: Option<u8>,
    pub debug: Option<bool>,
    pub codegen_units: Option<usize>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub members: Vec<String>,
    pub package: Option<WorkspacePackage>,
    pub dependencies: BTreeMap<String, DependencySpec>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WorkspacePackage {
    pub version: Option<String>,
    pub description: Option<String>,
    pub license: Option<String>,
    pub authors: Vec<String>,
    pub readme: Option<String>,
    pub repository: Option<String>,
    pub homepage: Option<String>,
    pub documentation: Option<String>,
}

const CURRENT_KERN_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub(super) enum Section {
    Root,
    Package,
    Craft,
    Runtime,
    Lib,
    Bin(usize),
    Test,
    Example(usize),
    Dependencies,
    DevDependencies,
    BuildDependencies,
    Features,
    ProfileDev,
    ProfileRelease,
    Workspace,
    WorkspacePackage,
    WorkspaceDependencies,
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self> {
        let source = fs::read_to_string(path).map_err(|err| Error::from_io(path, err))?;
        Self::parse(&source, path)
    }

    pub fn craft_env_names(&self) -> &[String] {
        self.craft
            .as_ref()
            .map(|craft| craft.env.as_slice())
            .unwrap_or(&[])
    }

    pub fn apply_runtime_options(&self, options: &mut CompileOptions) {
        let Some(runtime) = &self.runtime else {
            return;
        };

        if let Some(entry) = runtime.entry {
            options.runtime_entry = entry;
        }
        if let Some(libc) = runtime.libc {
            options.runtime_libc = libc;
        }
        if let Some(bundle) = runtime.bundle {
            options.library_bundle = bundle;
        }
    }

    pub fn apply_runtime_options_for_target(
        &self,
        target_kind: TargetKind,
        options: &mut CompileOptions,
    ) {
        let Some(runtime) = &self.runtime else {
            return;
        };

        if target_kind != TargetKind::Lib {
            if let Some(entry) = runtime.entry {
                options.runtime_entry = entry;
            }
            if let Some(libc) = runtime.libc {
                options.runtime_libc = libc;
            }
        }

        if let Some(bundle) = runtime.bundle {
            options.library_bundle = bundle;
        }
    }
}
