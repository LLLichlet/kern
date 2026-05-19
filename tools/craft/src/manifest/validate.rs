//! Semantic validation for parsed Craft manifests.
//!
//! Validation rejects unsupported sections, invalid target/source combinations,
//! malformed profile/style settings, and dependency forms before planning uses
//! the manifest.

use super::{
    CURRENT_KERN_VERSION, CraftFmtConfig, CraftStyleConfig, DependencySpec, Manifest, Package,
    Profile, ResourceSpec, WorkspaceExport, WorkspacePackage,
};
use crate::error::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

impl Manifest {
    pub fn validate(&self, path: &Path) -> Result<()> {
        if self.package.is_none() && self.workspace.is_none() {
            return Err(Error::Validation {
                path: path.to_path_buf(),
                message: "manifest must declare at least one of `[package]` or `[workspace]`"
                    .to_string(),
            });
        }
        if self.package.is_some() && self.workspace.is_some() {
            return Err(Error::Validation {
                path: path.to_path_buf(),
                message:
                    "manifest cannot declare both `[package]` and `[workspace]`; workspace roots are namespace manifests and packages must live in members"
                        .to_string(),
            });
        }

        if let Some(package) = &self.package {
            validate_non_empty(path, "[package].name", &package.name)?;
            validate_non_empty(path, "[package].version", &package.version)?;
            validate_non_empty(path, "[package].kern", &package.kern)?;
            validate_kern_version(path, &package.kern)?;
            validate_optional_package_metadata(path, "[package]", package)?;
        }

        if let Some(craft) = &self.craft {
            if let Some(policy) = craft.release_source_policy {
                let _ = policy;
            }
            for name in &craft.allow_floating_git {
                validate_source_name(path, "[craft].allow-floating-git[]", name)?;
            }
            for name in &craft.allow_insecure_source {
                validate_source_name(path, "[craft].allow-insecure-source[]", name)?;
            }
            if let Some(fmt) = &craft.fmt {
                validate_craft_fmt(path, fmt)?;
            }
            if let Some(style) = &craft.style {
                validate_craft_style(path, style)?;
            }
        }

        if let Some(runtime) = &self.runtime {
            let _ = runtime.entry;
            let _ = runtime.libc;
            let _ = runtime.bundle;
        }

        if let Some(lib) = &self.lib {
            validate_non_empty(path, "[lib].root", &lib.root)?;
        }

        validate_named_targets(path, "[[bin]]", &self.bin)?;
        validate_test_targets(path, &self.test)?;
        validate_root_targets(path, "[example].roots", &self.example)?;

        validate_dependencies(path, "[dependencies]", &self.dependencies)?;
        validate_dependencies(path, "[dev-dependencies]", &self.dev_dependencies)?;
        validate_dependencies(path, "[build-dependencies]", &self.build_dependencies)?;
        validate_resources(path, "[resources]", &self.resources)?;

        for (feature, members) in &self.features {
            validate_non_empty(path, "feature name", feature)?;
            for member in members {
                validate_non_empty(path, &format!("feature `{feature}` member"), member)?;
            }
        }

        if let Some(profile_set) = &self.profile {
            if let Some(dev) = &profile_set.dev {
                validate_profile(path, "[profile.dev]", dev)?;
            }
            if let Some(release) = &profile_set.release {
                validate_profile(path, "[profile.release]", release)?;
            }
        }

        if let Some(workspace) = &self.workspace {
            validate_non_empty(path, "[workspace].name", &workspace.name)?;
            for member in &workspace.members {
                validate_non_empty(path, "[workspace].members[]", member)?;
            }
            for (name, export) in &workspace.exports {
                validate_non_empty(path, "[workspace.exports] key", name)?;
                validate_workspace_export(path, name, export)?;
            }
            validate_dependencies(path, "[workspace.dependencies]", &workspace.dependencies)?;
            if let Some(package) = &workspace.package {
                validate_optional_workspace_package_metadata(path, "[workspace.package]", package)?;
            }
        }

        Ok(())
    }
}

fn validate_craft_fmt(path: &Path, fmt: &CraftFmtConfig) -> Result<()> {
    if let Some(line_width) = fmt.line_width
        && line_width < 40
    {
        return Err(Error::Validation {
            path: path.to_path_buf(),
            message: "[craft.fmt].line-width must be at least 40".to_string(),
        });
    }
    if let Some(threshold) = fmt.postfix_chain_threshold {
        validate_fmt_threshold(path, "[craft.fmt].postfix-chain-threshold", threshold)?;
    }
    if let Some(threshold) = fmt.boolean_chain_threshold {
        validate_fmt_threshold(path, "[craft.fmt].boolean-chain-threshold", threshold)?;
    }
    if let Some(threshold) = fmt.function_parameter_threshold {
        validate_fmt_threshold(path, "[craft.fmt].function-parameter-threshold", threshold)?;
    }
    if let Some(threshold) = fmt.call_argument_threshold {
        validate_fmt_threshold(path, "[craft.fmt].call-argument-threshold", threshold)?;
    }
    for pattern in &fmt.exclude {
        validate_non_empty(path, "[craft.fmt].exclude[]", pattern)?;
    }
    Ok(())
}

fn validate_fmt_threshold(path: &Path, key: &str, threshold: usize) -> Result<()> {
    if threshold < 2 {
        return Err(Error::Validation {
            path: path.to_path_buf(),
            message: format!("{key} must be at least 2"),
        });
    }
    Ok(())
}

fn validate_craft_style(path: &Path, style: &CraftStyleConfig) -> Result<()> {
    if let Some(suggestions) = style.suggestions {
        let _ = suggestions;
    }
    for rule in &style.disabled_rules {
        validate_non_empty(path, "[craft.style].disabled-rules[]", rule)?;
        if !crate::style::StyleRule::is_known_code(rule) {
            return Err(Error::Validation {
                path: path.to_path_buf(),
                message: format!("[craft.style].disabled-rules[] unknown style rule `{rule}`"),
            });
        }
    }
    for pattern in &style.exclude {
        validate_non_empty(path, "[craft.style].exclude[]", pattern)?;
    }
    Ok(())
}

fn validate_named_targets(
    path: &Path,
    section: &str,
    targets: &[super::NamedTarget],
) -> Result<()> {
    let mut names = BTreeSet::new();
    for target in targets {
        validate_non_empty(path, &format!("{section}.name"), &target.name)?;
        validate_non_empty(path, &format!("{section}.root"), &target.root)?;
        if !names.insert(target.name.as_str()) {
            return Err(Error::Validation {
                path: path.to_path_buf(),
                message: format!("duplicate target name `{}` in {section}", target.name),
            });
        }
    }
    Ok(())
}

fn validate_test_targets(path: &Path, targets: &[super::NamedTarget]) -> Result<()> {
    let mut names = BTreeSet::new();
    for target in targets {
        validate_non_empty(path, "[test].roots[]", &target.root)?;
        if contains_glob_pattern(&target.root) {
            continue;
        }
        if !names.insert(target.name.as_str()) {
            return Err(Error::Validation {
                path: path.to_path_buf(),
                message: format!("duplicate file stem `{}` in [test].roots", target.name),
            });
        }
    }
    Ok(())
}

fn validate_root_targets(path: &Path, section: &str, targets: &[super::NamedTarget]) -> Result<()> {
    let mut names = BTreeSet::new();
    for target in targets {
        validate_non_empty(path, &format!("{section}[]"), &target.root)?;
        if !names.insert(target.name.as_str()) {
            return Err(Error::Validation {
                path: path.to_path_buf(),
                message: format!("duplicate file stem `{}` in {section}", target.name),
            });
        }
    }
    Ok(())
}

fn contains_glob_pattern(path: &str) -> bool {
    path.contains('*') || path.contains('?') || path.contains('[')
}

fn validate_dependencies(
    path: &Path,
    section: &str,
    deps: &BTreeMap<String, DependencySpec>,
) -> Result<()> {
    for (name, spec) in deps {
        validate_non_empty(path, &format!("{section} key"), name)?;
        match spec {
            DependencySpec::Version(_) => {
                return Err(Error::Validation {
                    path: path.to_path_buf(),
                    message: format!(
                        "{section}.{name} must use an inline table with `path` or `git`; plain version strings are unsupported"
                    ),
                });
            }
            DependencySpec::Detailed(dep) => {
                if section == "[workspace.dependencies]" && dep.workspace == Some(true) {
                    return Err(Error::Validation {
                        path: path.to_path_buf(),
                        message: format!(
                            "{section}.{name} cannot use `workspace = true` inside `[workspace.dependencies]`"
                        ),
                    });
                }

                if dep.workspace == Some(true)
                    && (dep.version.is_some()
                        || dep.path.is_some()
                        || dep.git.is_some()
                        || dep.export.is_some())
                {
                    return Err(Error::Validation {
                        path: path.to_path_buf(),
                        message: format!(
                            "{section}.{name} cannot combine `workspace = true` with `version`, `path`, `git`, or `export`"
                        ),
                    });
                }

                let has_locator = dep.path.is_some() || dep.git.is_some();
                if dep.workspace != Some(true) && !has_locator {
                    return Err(Error::Validation {
                        path: path.to_path_buf(),
                        message: format!(
                            "{section}.{name} must declare `path`, `git`, or `workspace = true`"
                        ),
                    });
                }

                if dep.path.is_some() && dep.git.is_some() {
                    return Err(Error::Validation {
                        path: path.to_path_buf(),
                        message: format!("{section}.{name} cannot combine `path` and `git`"),
                    });
                }

                let dep_selector_count = usize::from(dep.rev.is_some())
                    + usize::from(dep.branch.is_some())
                    + usize::from(dep.tag.is_some());
                if dep_selector_count > 1 {
                    return Err(Error::Validation {
                        path: path.to_path_buf(),
                        message: format!(
                            "{section}.{name} may set at most one of `rev`, `branch`, or `tag`"
                        ),
                    });
                }

                if dep.git.is_none() && dep_selector_count > 0 {
                    return Err(Error::Validation {
                        path: path.to_path_buf(),
                        message: format!(
                            "{section}.{name} can only use `rev`, `branch`, or `tag` with `git` dependencies"
                        ),
                    });
                }

                if let Some(version) = &dep.version {
                    validate_non_empty(path, &format!("{section}.{name}.version"), version)?;
                }
                if let Some(path_value) = &dep.path {
                    validate_non_empty(path, &format!("{section}.{name}.path"), path_value)?;
                }
                if let Some(git) = &dep.git {
                    validate_non_empty(path, &format!("{section}.{name}.git"), git)?;
                }
                if let Some(rev) = &dep.rev {
                    validate_non_empty(path, &format!("{section}.{name}.rev"), rev)?;
                }
                if let Some(branch) = &dep.branch {
                    validate_non_empty(path, &format!("{section}.{name}.branch"), branch)?;
                }
                if let Some(tag) = &dep.tag {
                    validate_non_empty(path, &format!("{section}.{name}.tag"), tag)?;
                }
                if let Some(export) = &dep.export {
                    validate_non_empty(path, &format!("{section}.{name}.export"), export)?;
                }
                let _ = dep.optional;
                let _ = dep.default_features;
                for feature in &dep.features {
                    validate_non_empty(path, &format!("{section}.{name}.features[]"), feature)?;
                }
            }
        }
    }
    Ok(())
}

fn validate_workspace_export(path: &Path, name: &str, export: &WorkspaceExport) -> Result<()> {
    validate_non_empty(
        path,
        &format!("[workspace.exports].{name}.member"),
        &export.member,
    )?;
    Ok(())
}

fn validate_profile(path: &Path, section: &str, profile: &Profile) -> Result<()> {
    if let Some(opt) = profile.opt
        && opt > 3
    {
        return Err(Error::Validation {
            path: path.to_path_buf(),
            message: format!("{section}.opt must be in the range 0..=3"),
        });
    }
    if let Some(codegen_units) = profile.codegen_units
        && codegen_units == 0
    {
        return Err(Error::Validation {
            path: path.to_path_buf(),
            message: format!("{section}.codegen-units must be greater than zero"),
        });
    }
    if let Some(lto) = profile.lto.as_deref()
        && let Err(message) = kernc_utils::config::LtoMode::parse(lto)
    {
        return Err(Error::Validation {
            path: path.to_path_buf(),
            message: format!("{section}.lto {message}"),
        });
    }
    if let Some(code_model) = profile.code_model.as_deref()
        && let Err(message) = kernc_utils::config::CodeModel::parse(code_model)
    {
        return Err(Error::Validation {
            path: path.to_path_buf(),
            message: format!("{section}.code-model {message}"),
        });
    }
    let _ = profile.debug;
    Ok(())
}

fn validate_resources(
    path: &Path,
    section: &str,
    resources: &BTreeMap<String, ResourceSpec>,
) -> Result<()> {
    for (name, spec) in resources {
        validate_non_empty(path, &format!("{section} key"), name)?;

        let has_locator = spec.path.is_some() || spec.git.is_some();
        if !has_locator {
            return Err(Error::Validation {
                path: path.to_path_buf(),
                message: format!("{section}.{name} must declare `path` or `git`"),
            });
        }

        if spec.path.is_some() && spec.git.is_some() {
            return Err(Error::Validation {
                path: path.to_path_buf(),
                message: format!("{section}.{name} cannot combine `path` and `git`"),
            });
        }

        let selector_count = usize::from(spec.rev.is_some())
            + usize::from(spec.branch.is_some())
            + usize::from(spec.tag.is_some());
        if selector_count > 1 {
            return Err(Error::Validation {
                path: path.to_path_buf(),
                message: format!(
                    "{section}.{name} may set at most one of `rev`, `branch`, or `tag`"
                ),
            });
        }

        if spec.git.is_none() && selector_count > 0 {
            return Err(Error::Validation {
                path: path.to_path_buf(),
                message: format!(
                    "{section}.{name} can only use `rev`, `branch`, or `tag` with `git` resources"
                ),
            });
        }

        if let Some(path_value) = &spec.path {
            validate_non_empty(path, &format!("{section}.{name}.path"), path_value)?;
        }
        if let Some(git) = &spec.git {
            validate_non_empty(path, &format!("{section}.{name}.git"), git)?;
        }
        if let Some(rev) = &spec.rev {
            validate_non_empty(path, &format!("{section}.{name}.rev"), rev)?;
        }
        if let Some(branch) = &spec.branch {
            validate_non_empty(path, &format!("{section}.{name}.branch"), branch)?;
        }
        if let Some(tag) = &spec.tag {
            validate_non_empty(path, &format!("{section}.{name}.tag"), tag)?;
        }
    }

    Ok(())
}

fn validate_optional_package_metadata(path: &Path, section: &str, package: &Package) -> Result<()> {
    if let Some(description) = &package.description {
        validate_non_empty(path, &format!("{section}.description"), description)?;
    }
    if let Some(license) = &package.license {
        validate_non_empty(path, &format!("{section}.license"), license)?;
    }
    for author in &package.authors {
        validate_non_empty(path, &format!("{section}.authors[]"), author)?;
    }
    if let Some(readme) = &package.readme {
        validate_non_empty(path, &format!("{section}.readme"), readme)?;
    }
    if let Some(repository) = &package.repository {
        validate_non_empty(path, &format!("{section}.repository"), repository)?;
    }
    if let Some(homepage) = &package.homepage {
        validate_non_empty(path, &format!("{section}.homepage"), homepage)?;
    }
    if let Some(documentation) = &package.documentation {
        validate_non_empty(path, &format!("{section}.documentation"), documentation)?;
    }
    Ok(())
}

fn validate_optional_workspace_package_metadata(
    path: &Path,
    section: &str,
    package: &WorkspacePackage,
) -> Result<()> {
    if let Some(version) = &package.version {
        validate_non_empty(path, &format!("{section}.version"), version)?;
    }
    if let Some(kern) = &package.kern {
        validate_non_empty(path, &format!("{section}.kern"), kern)?;
        validate_kern_version(path, kern)?;
    }
    if let Some(description) = &package.description {
        validate_non_empty(path, &format!("{section}.description"), description)?;
    }
    if let Some(license) = &package.license {
        validate_non_empty(path, &format!("{section}.license"), license)?;
    }
    for author in &package.authors {
        validate_non_empty(path, &format!("{section}.authors[]"), author)?;
    }
    if let Some(readme) = &package.readme {
        validate_non_empty(path, &format!("{section}.readme"), readme)?;
    }
    if let Some(repository) = &package.repository {
        validate_non_empty(path, &format!("{section}.repository"), repository)?;
    }
    if let Some(homepage) = &package.homepage {
        validate_non_empty(path, &format!("{section}.homepage"), homepage)?;
    }
    if let Some(documentation) = &package.documentation {
        validate_non_empty(path, &format!("{section}.documentation"), documentation)?;
    }
    Ok(())
}

fn validate_non_empty(path: &Path, field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::Validation {
            path: path.to_path_buf(),
            message: format!("{field} must not be empty"),
        });
    }
    Ok(())
}

fn validate_kern_version(path: &Path, value: &str) -> Result<()> {
    if value != CURRENT_KERN_VERSION {
        return Err(Error::Validation {
            path: path.to_path_buf(),
            message: format!(
                "[package].kern must match the current toolchain version `{CURRENT_KERN_VERSION}`, found `{value}`"
            ),
        });
    }
    Ok(())
}

fn validate_source_name(path: &Path, field: &str, value: &str) -> Result<()> {
    validate_non_empty(path, field, value)?;
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        unreachable!("non-empty source names are required");
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return Err(Error::Validation {
            path: path.to_path_buf(),
            message: format!(
                "{field} names must start with an ASCII letter or `_`, found `{value}`"
            ),
        });
    }
    if chars.any(|ch| !(ch == '_' || ch == '-' || ch.is_ascii_alphanumeric())) {
        return Err(Error::Validation {
            path: path.to_path_buf(),
            message: format!(
                "{field} names must contain only ASCII letters, digits, `_`, or `-`, found `{value}`"
            ),
        });
    }
    Ok(())
}
