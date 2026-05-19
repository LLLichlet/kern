//! Craft manifest model, parsing, and validation entry points.
//!
//! The manifest layer keeps raw `Craft.toml` data close to user-facing
//! validation so later planning stages can rely on normalized package,
//! workspace, dependency, profile, and style settings.

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
    pub test_roots_explicit: bool,
    pub example: Vec<NamedTarget>,
    pub dependencies: BTreeMap<String, DependencySpec>,
    pub dev_dependencies: BTreeMap<String, DependencySpec>,
    pub build_dependencies: BTreeMap<String, DependencySpec>,
    pub resources: BTreeMap<String, ResourceSpec>,
    pub features: BTreeMap<String, Vec<String>>,
    pub profile: Option<Profiles>,
    pub workspace: Option<Workspace>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub kern: String,
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
    pub release_source_policy: Option<ReleaseSourcePolicy>,
    pub allow_floating_git: Vec<String>,
    pub allow_insecure_source: Vec<String>,
    pub fmt: Option<CraftFmtConfig>,
    pub style: Option<CraftStyleConfig>,
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
pub struct CraftStyleConfig {
    pub suggestions: Option<CraftStyleSuggestionLevel>,
    pub disabled_rules: Vec<String>,
    pub exclude: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CraftFmtConfig {
    pub line_width: Option<usize>,
    pub postfix_chain_threshold: Option<usize>,
    pub boolean_chain_threshold: Option<usize>,
    pub function_parameter_threshold: Option<usize>,
    pub call_argument_threshold: Option<usize>,
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CraftStyleSuggestionLevel {
    Off,
    Info,
    Warn,
}

impl CraftStyleSuggestionLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Info => "info",
            Self::Warn => "warn",
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
    pub export: Option<String>,
    pub optional: Option<bool>,
    pub default_features: Option<bool>,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceSpec {
    pub path: Option<String>,
    pub git: Option<String>,
    pub rev: Option<String>,
    pub branch: Option<String>,
    pub tag: Option<String>,
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
    pub lto: Option<String>,
    pub code_model: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub name: String,
    pub members: Vec<String>,
    pub exports: BTreeMap<String, WorkspaceExport>,
    pub package: Option<WorkspacePackage>,
    pub dependencies: BTreeMap<String, DependencySpec>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WorkspaceExport {
    pub member: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WorkspacePackage {
    pub version: Option<String>,
    pub kern: Option<String>,
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
    CraftFmt,
    CraftStyle,
    Runtime,
    Lib,
    Bin(usize),
    Test,
    Example,
    Dependencies,
    DevDependencies,
    BuildDependencies,
    Resources,
    Features,
    ProfileDev,
    ProfileRelease,
    Workspace,
    WorkspaceExports,
    WorkspacePackage,
    WorkspaceDependencies,
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self> {
        let source = fs::read_to_string(path).map_err(|err| Error::from_io(path, err))?;
        Self::parse(&source, path)
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
