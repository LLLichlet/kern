use super::{
    ANALYSIS_CONTEXT_VERSION, AnalysisContext, digest_file, path_and_digest_current,
    relative_display, resolve_context_path,
};
use crate::error::{Error, Result};
use std::collections::BTreeSet;
use std::path::Path;

impl AnalysisContext {
    pub(super) fn validate(&self, path: &Path) -> Result<()> {
        if self.version != ANALYSIS_CONTEXT_VERSION {
            return Err(Error::AnalysisContextValidation {
                path: path.to_path_buf(),
                message: format!("unsupported analysis context version `{}`", self.version),
            });
        }

        validate_non_empty(path, "manifest", &self.manifest)?;
        validate_digest(path, "manifest-digest", &self.manifest_digest)?;
        validate_non_empty(path, "profile", &self.profile)?;
        validate_optional_path_and_digest(
            path,
            "workspace-script",
            self.workspace_script.as_deref(),
            self.workspace_script_digest.as_deref(),
        )?;

        let mut package_manifests = BTreeSet::new();
        for package in &self.packages {
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
            if !package_manifests.insert(package.manifest.as_str()) {
                return Err(Error::AnalysisContextValidation {
                    path: path.to_path_buf(),
                    message: format!("duplicate [[package]] manifest `{}`", package.manifest),
                });
            }
        }

        let mut unit_keys = BTreeSet::new();
        for unit in &self.units {
            validate_non_empty(path, "[[unit]].manifest", &unit.manifest)?;
            validate_non_empty(path, "[[unit]].source-root", &unit.source_root)?;
            validate_target_kind(path, "[[unit]].target-kind", &unit.target_kind)?;
            if !package_manifests.contains(unit.manifest.as_str()) {
                return Err(Error::AnalysisContextValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "[[unit]] references unknown package manifest `{}`",
                        unit.manifest
                    ),
                });
            }
            if !unit_keys.insert((unit.manifest.as_str(), unit.source_root.as_str())) {
                return Err(Error::AnalysisContextValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "duplicate [[unit]] `{}` -> `{}`",
                        unit.manifest, unit.source_root
                    ),
                });
            }
        }

        let mut alias_keys = BTreeSet::new();
        for alias in &self.unit_aliases {
            validate_non_empty(path, "[[unit-alias]].manifest", &alias.manifest)?;
            validate_non_empty(path, "[[unit-alias]].source-root", &alias.source_root)?;
            validate_non_empty(path, "[[unit-alias]].source-path", &alias.source_path)?;
            validate_non_empty(path, "[[unit-alias]].generated-path", &alias.generated_path)?;
            if !unit_keys.contains(&(alias.manifest.as_str(), alias.source_root.as_str())) {
                return Err(Error::AnalysisContextValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "[[unit-alias]] references unknown unit `{}` -> `{}`",
                        alias.manifest, alias.source_root
                    ),
                });
            }
            if !alias_keys.insert((
                alias.manifest.as_str(),
                alias.source_root.as_str(),
                alias.source_path.as_str(),
            )) {
                return Err(Error::AnalysisContextValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "duplicate [[unit-alias]] `{}` -> `{}` -> `{}`",
                        alias.manifest, alias.source_root, alias.source_path
                    ),
                });
            }
        }

        let mut features = BTreeSet::new();
        for feature in &self.features {
            validate_non_empty(path, "features[]", feature)?;
            if !features.insert(feature.as_str()) {
                return Err(Error::AnalysisContextValidation {
                    path: path.to_path_buf(),
                    message: format!("duplicate feature `{feature}`"),
                });
            }
        }

        let mut value_keys = BTreeSet::new();
        for value in &self.unit_values {
            validate_non_empty(path, "[[unit-value]].manifest", &value.manifest)?;
            validate_non_empty(path, "[[unit-value]].source-root", &value.source_root)?;
            validate_non_empty(path, "[[unit-value]].name", &value.name)?;
            if !unit_keys.contains(&(value.manifest.as_str(), value.source_root.as_str())) {
                return Err(Error::AnalysisContextValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "[[unit-value]] references unknown unit `{}` -> `{}`",
                        value.manifest, value.source_root
                    ),
                });
            }
            if !value_keys.insert((
                value.manifest.as_str(),
                value.source_root.as_str(),
                value.name.as_str(),
            )) {
                return Err(Error::AnalysisContextValidation {
                    path: path.to_path_buf(),
                    message: format!(
                        "duplicate [[unit-value]] `{}` -> `{}` -> `{}`",
                        value.manifest, value.source_root, value.name
                    ),
                });
            }
        }

        Ok(())
    }

    pub(super) fn is_current(&self, manifest_path: &Path, workspace_root: &Path) -> Result<bool> {
        if self.manifest != relative_display(workspace_root, manifest_path) {
            return Ok(false);
        }
        if self.manifest_digest != digest_file(manifest_path)? {
            return Ok(false);
        }

        if !path_and_digest_current(
            workspace_root,
            self.workspace_script.as_deref(),
            self.workspace_script_digest.as_deref(),
        )? {
            return Ok(false);
        }

        for package in &self.packages {
            let manifest_path = resolve_context_path(workspace_root, &package.manifest);
            if !manifest_path.is_file() {
                return Ok(false);
            }
            if package.manifest_digest != digest_file(&manifest_path)? {
                return Ok(false);
            }
            if !path_and_digest_current(
                workspace_root,
                package.craft_script.as_deref(),
                package.craft_script_digest.as_deref(),
            )? {
                return Ok(false);
            }
        }

        Ok(true)
    }
}

fn validate_non_empty(path: &Path, field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::AnalysisContextValidation {
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
        _ => Err(Error::AnalysisContextValidation {
            path: path.to_path_buf(),
            message: format!(
                "{field} and {field}-digest must either both be present or both be absent"
            ),
        }),
    }
}

fn validate_digest(path: &Path, field: &str, value: &str) -> Result<()> {
    validate_non_empty(path, field, value)?;
    if !value.starts_with("fnv1a64:") || value.len() != "fnv1a64:".len() + 16 {
        return Err(Error::AnalysisContextValidation {
            path: path.to_path_buf(),
            message: format!("{field} must be an `fnv1a64:` digest"),
        });
    }
    Ok(())
}

fn validate_target_kind(path: &Path, field: &str, value: &str) -> Result<()> {
    match value {
        "lib" | "bin" | "test" | "example" => Ok(()),
        _ => Err(Error::AnalysisContextValidation {
            path: path.to_path_buf(),
            message: format!("{field} has unsupported unit target kind `{value}`"),
        }),
    }
}
