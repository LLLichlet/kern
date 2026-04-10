use super::{
    AnalysisContext, AnalysisContextPackage, AnalysisContextUnit, AnalysisContextUnitAlias,
    AnalysisContextUnitValue, Section,
};
use crate::error::{Error, Result};
use std::path::Path;

impl AnalysisContext {
    pub(super) fn parse(source: &str, path: &Path) -> Result<Self> {
        let mut context = Self {
            version: 0,
            manifest: String::new(),
            manifest_digest: String::new(),
            profile: String::new(),
            default_features: true,
            features: Vec::new(),
            workspace_script: None,
            workspace_script_digest: None,
            packages: Vec::new(),
            units: Vec::new(),
            unit_aliases: Vec::new(),
            unit_values: Vec::new(),
        };
        let mut section = Section::Root;

        for line in logical_lines(source).map_err(|message| Error::AnalysisContextParse {
            path: path.to_path_buf(),
            message,
        })? {
            if line.starts_with("[[") {
                section = match line.as_str() {
                    "[[package]]" => {
                        context.packages.push(AnalysisContextPackage {
                            manifest: String::new(),
                            manifest_digest: String::new(),
                            craft_script: None,
                            craft_script_digest: None,
                        });
                        Section::Package(context.packages.len() - 1)
                    }
                    "[[unit]]" => {
                        context.units.push(AnalysisContextUnit {
                            manifest: String::new(),
                            source_root: String::new(),
                            target_kind: String::new(),
                        });
                        Section::Unit(context.units.len() - 1)
                    }
                    "[[unit-alias]]" => {
                        context.unit_aliases.push(AnalysisContextUnitAlias {
                            manifest: String::new(),
                            source_root: String::new(),
                            source_path: String::new(),
                            generated_path: String::new(),
                        });
                        Section::UnitAlias(context.unit_aliases.len() - 1)
                    }
                    "[[unit-value]]" => {
                        context.unit_values.push(AnalysisContextUnitValue {
                            manifest: String::new(),
                            source_root: String::new(),
                            name: String::new(),
                            value: String::new(),
                        });
                        Section::UnitValue(context.unit_values.len() - 1)
                    }
                    _ => {
                        return Err(Error::AnalysisContextParse {
                            path: path.to_path_buf(),
                            message: format!("unsupported array table `{line}`"),
                        });
                    }
                };
                continue;
            }

            let (key, raw_value) =
                split_key_value(&line).map_err(|message| Error::AnalysisContextParse {
                    path: path.to_path_buf(),
                    message,
                })?;
            assign_key_value(&mut context, section, key, raw_value).map_err(|message| {
                Error::AnalysisContextParse {
                    path: path.to_path_buf(),
                    message,
                }
            })?;
        }

        context.validate(path)?;
        Ok(context)
    }
}

fn assign_key_value(
    context: &mut AnalysisContext,
    section: Section,
    key: &str,
    raw_value: &str,
) -> std::result::Result<(), String> {
    match section {
        Section::Root => match key {
            "version" => context.version = parse_u32(raw_value)?,
            "manifest" => context.manifest = parse_string(raw_value)?,
            "manifest-digest" => context.manifest_digest = parse_string(raw_value)?,
            "profile" => context.profile = parse_string(raw_value)?,
            "default-features" => context.default_features = parse_bool(raw_value)?,
            "features" => context.features = parse_string_array(raw_value)?,
            "workspace-script" => context.workspace_script = Some(parse_string(raw_value)?),
            "workspace-script-digest" => {
                context.workspace_script_digest = Some(parse_string(raw_value)?)
            }
            _ => return Err(format!("unsupported root key `{key}`")),
        },
        Section::Package(index) => {
            let package = &mut context.packages[index];
            match key {
                "manifest" => package.manifest = parse_string(raw_value)?,
                "manifest-digest" => package.manifest_digest = parse_string(raw_value)?,
                "craft-script" => package.craft_script = Some(parse_string(raw_value)?),
                "craft-script-digest" => {
                    package.craft_script_digest = Some(parse_string(raw_value)?)
                }
                _ => return Err(format!("unsupported [[package]] key `{key}`")),
            }
        }
        Section::Unit(index) => {
            let unit = &mut context.units[index];
            match key {
                "manifest" => unit.manifest = parse_string(raw_value)?,
                "source-root" => unit.source_root = parse_string(raw_value)?,
                "target-kind" => unit.target_kind = parse_string(raw_value)?,
                _ => return Err(format!("unsupported [[unit]] key `{key}`")),
            }
        }
        Section::UnitAlias(index) => {
            let alias = &mut context.unit_aliases[index];
            match key {
                "manifest" => alias.manifest = parse_string(raw_value)?,
                "source-root" => alias.source_root = parse_string(raw_value)?,
                "source-path" => alias.source_path = parse_string(raw_value)?,
                "generated-path" => alias.generated_path = parse_string(raw_value)?,
                _ => return Err(format!("unsupported [[unit-alias]] key `{key}`")),
            }
        }
        Section::UnitValue(index) => {
            let value = &mut context.unit_values[index];
            match key {
                "manifest" => value.manifest = parse_string(raw_value)?,
                "source-root" => value.source_root = parse_string(raw_value)?,
                "name" => value.name = parse_string(raw_value)?,
                "value" => value.value = parse_string(raw_value)?,
                _ => return Err(format!("unsupported [[unit-value]] key `{key}`")),
            }
        }
    }
    Ok(())
}

fn logical_lines(source: &str) -> std::result::Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for raw_line in source.lines() {
        let stripped = strip_comment(raw_line)?;
        let trimmed = stripped.trim();
        if !trimmed.is_empty() {
            lines.push(trimmed.to_string());
        }
    }
    Ok(lines)
}

fn strip_comment(line: &str) -> std::result::Result<String, String> {
    let mut out = String::new();
    let mut in_string = false;
    let mut escape = false;

    for ch in line.chars() {
        if escape {
            out.push(ch);
            escape = false;
            continue;
        }

        match ch {
            '\\' if in_string => {
                out.push(ch);
                escape = true;
            }
            '"' => {
                out.push(ch);
                in_string = !in_string;
            }
            '#' if !in_string => break,
            _ => out.push(ch),
        }
    }

    if in_string {
        return Err("unterminated string literal".to_string());
    }

    Ok(out)
}

fn split_key_value(line: &str) -> std::result::Result<(&str, &str), String> {
    let mut in_string = false;
    let mut escape = false;

    for (index, ch) in line.char_indices() {
        if escape {
            escape = false;
            continue;
        }

        if in_string {
            match ch {
                '\\' => escape = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '=' => {
                let key = line[..index].trim();
                let value = line[index + 1..].trim();
                if key.is_empty() {
                    return Err("missing key before `=`".to_string());
                }
                if value.is_empty() {
                    return Err(format!("missing value for key `{key}`"));
                }
                return Ok((key, value));
            }
            _ => {}
        }
    }

    Err(format!("expected `key = value`, found `{line}`"))
}

fn parse_u32(raw: &str) -> std::result::Result<u32, String> {
    raw.trim()
        .parse::<u32>()
        .map_err(|_| format!("expected unsigned integer, found `{}`", raw.trim()))
}

fn parse_string(raw: &str) -> std::result::Result<String, String> {
    let raw = raw.trim();
    if !raw.starts_with('"') || !raw.ends_with('"') || raw.len() < 2 {
        return Err(format!("expected string literal, found `{raw}`"));
    }

    let inner = &raw[1..raw.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }

        let Some(escaped) = chars.next() else {
            return Err("unterminated string escape".to_string());
        };
        match escaped {
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            other => return Err(format!("unsupported escape sequence `\\{other}`")),
        }
    }

    Ok(out)
}

fn parse_bool(raw: &str) -> std::result::Result<bool, String> {
    match raw.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("expected boolean, found `{other}`")),
    }
}

fn parse_string_array(raw: &str) -> std::result::Result<Vec<String>, String> {
    let inner = strip_wrapping(raw, '[', ']')?;
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }

    split_top_level(inner, ',')
        .into_iter()
        .map(parse_string)
        .collect()
}

fn strip_wrapping(raw: &str, open: char, close: char) -> std::result::Result<&str, String> {
    let trimmed = raw.trim();
    if !trimmed.starts_with(open) || !trimmed.ends_with(close) || trimmed.len() < 2 {
        return Err(format!("expected `{open}...{close}`, found `{trimmed}`"));
    }
    Ok(&trimmed[1..trimmed.len() - 1])
}

fn split_top_level(input: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_string = false;
    let mut escape = false;
    let mut brace_depth = 0usize;
    let mut bracket_depth = 0usize;

    for (index, ch) in input.char_indices() {
        if escape {
            escape = false;
            continue;
        }

        if in_string {
            match ch {
                '\\' => escape = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            _ if ch == separator && brace_depth == 0 && bracket_depth == 0 => {
                let piece = input[start..index].trim();
                if !piece.is_empty() {
                    parts.push(piece);
                }
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }

    let tail = input[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }

    parts
}
