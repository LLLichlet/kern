use super::lifecycle::emit_trace;
use super::{ServerError, ServerState};
use crate::analysis::AnalysisSettings;
use crate::protocol::{DidChangeConfigurationParams, log_message};
use crate::transport::MessageWriter;
use kernc_utils::config::LibraryBundle;
use serde_json::{Map, Value};
use std::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConfigurationChange {
    Changed,
    Unchanged,
}

pub(super) fn handle_configuration_change(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    params: DidChangeConfigurationParams,
) -> Result<ConfigurationChange, ServerError> {
    let report = parse_analysis_settings(&params.settings, state.analysis.settings())?;
    for warning in report.warnings {
        writer.write_json(&log_message(2, warning))?;
    }

    let Some(settings) = report.settings else {
        emit_trace(
            state,
            writer,
            "workspace configuration unchanged",
            Some("no supported kern.project analysis settings were present".to_string()),
            true,
        )?;
        return Ok(ConfigurationChange::Unchanged);
    };

    if !state.analysis.replace_settings(settings) {
        emit_trace(
            state,
            writer,
            "workspace configuration unchanged",
            Some("supported analysis settings matched the active settings".to_string()),
            true,
        )?;
        return Ok(ConfigurationChange::Unchanged);
    }

    emit_trace(
        state,
        writer,
        "workspace configuration changed",
        Some("analysis settings were updated".to_string()),
        false,
    )?;
    Ok(ConfigurationChange::Changed)
}

#[derive(Debug)]
struct ConfigurationReport {
    settings: Option<AnalysisSettings>,
    warnings: Vec<String>,
}

fn parse_analysis_settings(
    settings: &Value,
    active: &AnalysisSettings,
) -> Result<ConfigurationReport, ServerError> {
    let mut warnings = Vec::new();
    let Some(root) = settings.as_object() else {
        if !settings.is_null() {
            warnings.push(
                "Ignoring workspace configuration because the settings payload is not an object."
                    .to_string(),
            );
        }
        return Ok(ConfigurationReport {
            settings: None,
            warnings,
        });
    };

    let project = match project_settings(root) {
        ProjectSettings::Missing => {
            warn_unknown_root_keys(root, &mut warnings);
            return Ok(ConfigurationReport {
                settings: None,
                warnings,
            });
        }
        ProjectSettings::Present(project) => project,
    };

    warn_unknown_root_keys(root, &mut warnings);
    warn_unknown_project_keys(project, &mut warnings);

    let mut compile_options = active.compile_options.clone();
    if let Some(value) = project.get("features") {
        compile_options.craft_features = parse_features(value)?;
    }
    if let Some(value) = project.get("noDefaultFeatures") {
        compile_options.craft_default_features =
            !parse_bool(value, "kern.project.noDefaultFeatures")?;
    }
    if let Some(value) = project.get("libraryBundle") {
        compile_options.library_bundle = parse_library_bundle(value)?;
    }
    if let Some(value) = project.get("modulePaths") {
        compile_options.module_aliases = parse_string_map(value, "kern.project.modulePaths")?;
    }
    if let Some(value) = project.get("moduleInterfacePaths") {
        compile_options.module_interface_aliases =
            parse_string_map(value, "kern.project.moduleInterfacePaths")?;
    }

    Ok(ConfigurationReport {
        settings: Some(AnalysisSettings { compile_options }),
        warnings,
    })
}

enum ProjectSettings<'a> {
    Present(&'a Map<String, Value>),
    Missing,
}

fn project_settings(root: &Map<String, Value>) -> ProjectSettings<'_> {
    match root.get("project").and_then(Value::as_object) {
        Some(project) => ProjectSettings::Present(project),
        None => ProjectSettings::Missing,
    }
}

fn parse_features(value: &Value) -> Result<Vec<String>, ServerError> {
    let features = value.as_array().ok_or_else(|| {
        ServerError::Protocol("kern.project.features must be an array of strings".to_string())
    })?;
    let mut parsed = Vec::new();
    for feature in features {
        let feature = feature.as_str().ok_or_else(|| {
            ServerError::Protocol("kern.project.features must be an array of strings".to_string())
        })?;
        let feature = feature.trim();
        if feature.is_empty() {
            return Err(ServerError::Protocol(
                "kern.project.features cannot contain empty feature names".to_string(),
            ));
        }
        if !parsed.iter().any(|existing| existing == feature) {
            parsed.push(feature.to_string());
        }
    }
    Ok(parsed)
}

fn parse_bool(value: &Value, name: &str) -> Result<bool, ServerError> {
    value
        .as_bool()
        .ok_or_else(|| ServerError::Protocol(format!("{name} must be a boolean")))
}

fn parse_library_bundle(value: &Value) -> Result<LibraryBundle, ServerError> {
    let raw = value.as_str().ok_or_else(|| {
        ServerError::Protocol("kern.project.libraryBundle must be a string".to_string())
    })?;
    LibraryBundle::parse(raw).map_err(ServerError::Protocol)
}

fn parse_string_map(
    value: &Value,
    name: &str,
) -> Result<std::collections::HashMap<String, String>, ServerError> {
    let object = value
        .as_object()
        .ok_or_else(|| ServerError::Protocol(format!("{name} must be an object")))?;
    let mut parsed = std::collections::HashMap::new();
    for (key, value) in object {
        if key.trim().is_empty() {
            return Err(ServerError::Protocol(format!(
                "{name} cannot contain an empty alias name"
            )));
        }
        let value = value
            .as_str()
            .ok_or_else(|| ServerError::Protocol(format!("{name}.{key} must be a string")))?;
        if value.trim().is_empty() {
            return Err(ServerError::Protocol(format!(
                "{name}.{key} cannot be an empty path"
            )));
        }
        parsed.insert(key.clone(), value.to_string());
    }
    Ok(parsed)
}

fn warn_unknown_root_keys(root: &Map<String, Value>, warnings: &mut Vec<String>) {
    for key in root.keys() {
        if key != "project" {
            warnings.push(format!(
                "Ignoring unsupported workspace configuration key `{key}`."
            ));
        }
    }
}

fn warn_unknown_project_keys(project: &Map<String, Value>, warnings: &mut Vec<String>) {
    for key in project.keys() {
        if !matches!(
            key.as_str(),
            "features"
                | "noDefaultFeatures"
                | "libraryBundle"
                | "modulePaths"
                | "moduleInterfacePaths"
        ) {
            warnings.push(format!(
                "Ignoring unsupported workspace configuration key `kern.project.{key}`."
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_vscode_project_settings_shape() {
        let active = AnalysisSettings::default();
        let report = parse_analysis_settings(
            &json!({
                "project": {
                    "features": [" simd ", "simd", "tls"],
                    "noDefaultFeatures": true
                },
                "editor": {
                    "autoSuggest": "keywords"
                }
            }),
            &active,
        )
        .unwrap();

        let settings = report.settings.unwrap();
        assert_eq!(
            settings.compile_options.craft_features,
            vec!["simd".to_string(), "tls".to_string()]
        );
        assert!(!settings.compile_options.craft_default_features);
        assert_eq!(
            report.warnings,
            vec!["Ignoring unsupported workspace configuration key `editor`.".to_string()]
        );
    }

    #[test]
    fn parses_project_module_paths() {
        let active = AnalysisSettings::default();
        let report = parse_analysis_settings(
            &json!({
                "project": {
                    "noDefaultFeatures": true,
                    "libraryBundle": "base",
                    "modulePaths": {
                        "demo": "./src/demo"
                    },
                    "moduleInterfacePaths": {
                        "std": "./meta/std"
                    }
                }
            }),
            &active,
        )
        .unwrap();

        let options = report.settings.unwrap().compile_options;
        assert_eq!(options.library_bundle, LibraryBundle::Base);
        assert!(!options.craft_default_features);
        assert_eq!(
            options.module_aliases.get("demo").map(String::as_str),
            Some("./src/demo")
        );
        assert_eq!(
            options
                .module_interface_aliases
                .get("std")
                .map(String::as_str),
            Some("./meta/std")
        );
    }

    #[test]
    fn invalid_supported_settings_are_protocol_errors() {
        let active = AnalysisSettings::default();
        let err = parse_analysis_settings(
            &json!({
                "project": {
                    "features": ["ok", ""]
                }
            }),
            &active,
        )
        .unwrap_err();

        assert!(matches!(err, ServerError::Protocol(message) if message.contains("empty feature")));
    }
}
