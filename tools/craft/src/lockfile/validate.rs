use super::Lockfile;
use crate::error::{Error, Result};
use std::collections::BTreeSet;
use std::path::Path;

impl Lockfile {
    pub fn validate(&self, path: &Path) -> Result<()> {
        if self.version != 1 {
            return Err(Error::LockfileValidation {
                path: path.to_path_buf(),
                message: format!("unsupported lockfile version `{}`", self.version),
            });
        }

        validate_non_empty(path, "manifest", &self.manifest)?;
        validate_digest(path, "manifest-digest", &self.manifest_digest)?;
        validate_optional_path_and_digest(
            path,
            "workspace-script",
            self.workspace_script.as_deref(),
            self.workspace_script_digest.as_deref(),
        )?;
        let mut workspace_env_names = BTreeSet::new();
        if !self.workspace_env.is_empty() && self.workspace_script.is_none() {
            return Err(Error::LockfileValidation {
                path: path.to_path_buf(),
                message: "[[workspace-env]] entries require `workspace-script`".to_string(),
            });
        }
        for input in &self.workspace_env {
            validate_env_input_name(path, "[[workspace-env]].name", &input.name)?;
            if !workspace_env_names.insert(input.name.as_str()) {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!("duplicate workspace env input `{}`", input.name),
                });
            }
        }

        let mut package_ids = BTreeSet::new();
        for package in &self.packages {
            validate_non_empty(path, "[[package]].id", &package.id)?;
            validate_non_empty(path, "[[package]].name", &package.name)?;
            validate_non_empty(path, "[[package]].version", &package.version)?;
            validate_source_kind(path, "[[package]].source", &package.source_kind)?;
            if matches!(package.source_kind.as_str(), "workspace-member" | "path")
                && package.source_value.is_none()
            {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "[[package]] `{}` requires `source-value` for source `{}`",
                        package.id, package.source_kind
                    ),
                });
            }
            if !package_ids.insert(package.id.as_str()) {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!("duplicate package id `{}`", package.id),
                });
            }
            validate_non_empty(path, "[[package]].manifest", &package.manifest)?;
            validate_digest(
                path,
                "[[package]].manifest-digest",
                &package.manifest_digest,
            )?;
            validate_optional_path_and_digest(
                path,
                "[[package]].craft-script",
                package.craft_script.as_deref(),
                package.craft_script_digest.as_deref(),
            )?;
        }

        let mut package_target_keys = BTreeSet::new();
        for target in &self.package_targets {
            validate_non_empty(path, "[[package-target]].package", &target.package_id)?;
            if !package_ids.contains(target.package_id.as_str()) {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "[[package-target]] references unknown package id `{}`",
                        target.package_id
                    ),
                });
            }
            validate_target_kind_name(path, "[[package-target]].kind", &target.kind)?;
            match target.kind.as_str() {
                "lib" => {
                    if target.name.is_some() {
                        return Err(Error::LockfileValidation {
                            path: path.to_path_buf(),
                            message: "[[package-target]] kind `lib` must not set `name`"
                                .to_string(),
                        });
                    }
                }
                _ => {
                    validate_non_empty(
                        path,
                        "[[package-target]].name",
                        target.name.as_deref().unwrap_or(""),
                    )?;
                }
            }
            validate_non_empty(path, "[[package-target]].root", &target.root)?;
            if !package_target_keys.insert((
                target.package_id.as_str(),
                target.kind.as_str(),
                target.name.as_deref().unwrap_or(""),
            )) {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "duplicate package target `{}:{}:{}`",
                        target.package_id,
                        target.kind,
                        target.name.as_deref().unwrap_or("<lib>")
                    ),
                });
            }
        }

        let mut package_env_keys = BTreeSet::new();
        for input in &self.package_env {
            validate_non_empty(path, "[[package-env]].package", &input.package_id)?;
            if !package_ids.contains(input.package_id.as_str()) {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "[[package-env]] references unknown package id `{}`",
                        input.package_id
                    ),
                });
            }
            let has_script = self
                .packages
                .iter()
                .find(|package| package.id == input.package_id)
                .and_then(|package| package.craft_script.as_ref())
                .is_some();
            if !has_script {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "[[package-env]] references package `{}` without `craft-script`",
                        input.package_id
                    ),
                });
            }
            validate_env_input_name(path, "[[package-env]].name", &input.name)?;
            if !package_env_keys.insert((input.package_id.as_str(), input.name.as_str())) {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "duplicate package env input `{}:{}`",
                        input.package_id, input.name
                    ),
                });
            }
        }

        let mut external_ids = BTreeSet::new();
        for package in &self.external_packages {
            validate_non_empty(path, "[[external-package]].id", &package.id)?;
            validate_non_empty(path, "[[external-package]].name", &package.name)?;
            validate_source_kind(path, "[[external-package]].source", &package.source_kind)?;
            if matches!(package.source_kind.as_str(), "path" | "workspace-member")
                && package.source_value.is_none()
            {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "[[external-package]] `{}` requires `source-value` for source `{}`",
                        package.id, package.source_kind
                    ),
                });
            }
            if !external_ids.insert(package.id.as_str()) {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!("duplicate external package id `{}`", package.id),
                });
            }
            if let Some(locator) = &package.source_locator {
                validate_non_empty(path, "[[external-package]].source-locator", locator)?;
            }
            if let Some(selector) = &package.source_selector {
                validate_non_empty(path, "[[external-package]].source-selector", selector)?;
            }
        }

        for dependency in &self.dependencies {
            validate_non_empty(path, "[[dependency]].from", &dependency.from)?;
            if !package_ids.contains(dependency.from.as_str()) {
                return Err(Error::LockfileValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "[[dependency]] references unknown package id `{}` in `from`",
                        dependency.from
                    ),
                });
            }
            validate_kind(path, "[[dependency]].kind", &dependency.kind)?;
            validate_non_empty(path, "[[dependency]].name", &dependency.name)?;
            validate_non_empty(path, "[[dependency]].package", &dependency.package)?;
            validate_target_kind(path, "[[dependency]].target", &dependency.target_kind)?;
            validate_non_empty(path, "[[dependency]].target-id", &dependency.target_id)?;
            match dependency.target_kind.as_str() {
                "local" => {
                    if !package_ids.contains(dependency.target_id.as_str()) {
                        return Err(Error::LockfileValidation {
                            path: path.to_path_buf(),
                            message: format!(
                                "[[dependency]] references unknown local target id `{}`",
                                dependency.target_id
                            ),
                        });
                    }
                }
                "external" => {
                    if !external_ids.contains(dependency.target_id.as_str()) {
                        return Err(Error::LockfileValidation {
                            path: path.to_path_buf(),
                            message: format!(
                                "[[dependency]] references unknown external target id `{}`",
                                dependency.target_id
                            ),
                        });
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }
}

fn validate_non_empty(path: &Path, field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!("{field} must not be empty"),
        });
    }
    Ok(())
}

fn validate_optional_path_and_digest(
    path: &Path,
    field: &str,
    value: Option<&str>,
    digest: Option<&str>,
) -> Result<()> {
    match (value, digest) {
        (None, None) => Ok(()),
        (Some(value), Some(digest)) => {
            validate_non_empty(path, field, value)?;
            validate_digest(path, &format!("{field}-digest"), digest)
        }
        _ => Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!(
                "{field} and {field}-digest must either both be present or both be absent"
            ),
        }),
    }
}

fn validate_env_input_name(path: &Path, field: &str, value: &str) -> Result<()> {
    validate_non_empty(path, field, value)?;
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!("{field} must not be empty"),
        });
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!("{field} must start with an ASCII letter or `_`, found `{value}`"),
        });
    }
    if chars.any(|ch| !(ch == '_' || ch.is_ascii_alphanumeric())) {
        return Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!(
                "{field} must contain only ASCII letters, digits, or `_`, found `{value}`"
            ),
        });
    }
    Ok(())
}

fn validate_digest(path: &Path, field: &str, value: &str) -> Result<()> {
    validate_non_empty(path, field, value)?;
    if !value.starts_with("fnv1a64:") || value.len() != "fnv1a64:".len() + 16 {
        return Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!("{field} must be an `fnv1a64:` digest"),
        });
    }
    Ok(())
}

fn validate_source_kind(path: &Path, field: &str, value: &str) -> Result<()> {
    match value {
        "root" | "workspace-member" | "path" | "git" => Ok(()),
        _ => Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!("{field} has unsupported source kind `{value}`"),
        }),
    }
}

fn validate_kind(path: &Path, field: &str, value: &str) -> Result<()> {
    match value {
        "normal" | "dev" | "build" => Ok(()),
        _ => Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!("{field} has unsupported dependency kind `{value}`"),
        }),
    }
}

fn validate_target_kind(path: &Path, field: &str, value: &str) -> Result<()> {
    match value {
        "local" | "external" => Ok(()),
        _ => Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!("{field} has unsupported dependency target `{value}`"),
        }),
    }
}

fn validate_target_kind_name(path: &Path, field: &str, value: &str) -> Result<()> {
    match value {
        "lib" | "bin" | "test" | "example" => Ok(()),
        _ => Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!("{field} has unsupported package target kind `{value}`"),
        }),
    }
}
