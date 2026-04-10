use super::{
    LockedDependency, LockedEnvInput, LockedExternalPackage, LockedPackage, LockedPackageEnvInput,
    LockedPackageTarget, Lockfile,
};
use crate::error::{Error, Result};
use std::path::Path;

#[derive(Clone, Copy, Debug)]
enum Section {
    Root,
    Package(usize),
    PackageTarget(usize),
    ExternalPackage(usize),
    WorkspaceEnv(usize),
    PackageEnv(usize),
    Dependency(usize),
}

impl Lockfile {
    pub(super) fn parse(source: &str, path: &Path) -> Result<Self> {
        let mut lockfile = Self {
            version: 0,
            manifest: String::new(),
            manifest_digest: String::new(),
            workspace_script: None,
            workspace_script_digest: None,
            workspace_env: Vec::new(),
            packages: Vec::new(),
            package_targets: Vec::new(),
            external_packages: Vec::new(),
            package_env: Vec::new(),
            dependencies: Vec::new(),
        };
        let mut section = Section::Root;

        for line in logical_lines(source).map_err(|message| Error::LockfileParse {
            path: path.to_path_buf(),
            message,
        })? {
            if line.starts_with("[[") {
                section = start_section(&mut lockfile, &line).map_err(|message| {
                    Error::LockfileParse {
                        path: path.to_path_buf(),
                        message,
                    }
                })?;
                continue;
            }

            let (key, raw_value) =
                split_key_value(&line).map_err(|message| Error::LockfileParse {
                    path: path.to_path_buf(),
                    message,
                })?;
            assign_key_value(&mut lockfile, section, key, raw_value).map_err(|message| {
                Error::LockfileParse {
                    path: path.to_path_buf(),
                    message,
                }
            })?;
        }

        Ok(lockfile)
    }
}

fn start_section(lockfile: &mut Lockfile, line: &str) -> std::result::Result<Section, String> {
    match line {
        "[[package]]" => {
            lockfile.packages.push(LockedPackage {
                id: String::new(),
                name: String::new(),
                version: String::new(),
                source_kind: String::new(),
                source_value: None,
                manifest: String::new(),
                manifest_digest: String::new(),
                craft_script: None,
                craft_script_digest: None,
            });
            Ok(Section::Package(lockfile.packages.len() - 1))
        }
        "[[external-package]]" => {
            lockfile.external_packages.push(LockedExternalPackage {
                id: String::new(),
                name: String::new(),
                source_kind: String::new(),
                source_value: None,
                version: None,
                source_locator: None,
                source_selector: None,
            });
            Ok(Section::ExternalPackage(
                lockfile.external_packages.len() - 1,
            ))
        }
        "[[package-target]]" => {
            lockfile.package_targets.push(LockedPackageTarget {
                package_id: String::new(),
                kind: String::new(),
                name: None,
                root: String::new(),
            });
            Ok(Section::PackageTarget(lockfile.package_targets.len() - 1))
        }
        "[[workspace-env]]" => {
            lockfile.workspace_env.push(LockedEnvInput {
                name: String::new(),
                value: None,
            });
            Ok(Section::WorkspaceEnv(lockfile.workspace_env.len() - 1))
        }
        "[[package-env]]" => {
            lockfile.package_env.push(LockedPackageEnvInput {
                package_id: String::new(),
                name: String::new(),
                value: None,
            });
            Ok(Section::PackageEnv(lockfile.package_env.len() - 1))
        }
        "[[dependency]]" => {
            lockfile.dependencies.push(LockedDependency {
                from: String::new(),
                kind: String::new(),
                name: String::new(),
                package: String::new(),
                target_kind: String::new(),
                target_id: String::new(),
            });
            Ok(Section::Dependency(lockfile.dependencies.len() - 1))
        }
        _ => Err(format!("unsupported array table `{line}`")),
    }
}

fn assign_key_value(
    lockfile: &mut Lockfile,
    section: Section,
    key: &str,
    raw_value: &str,
) -> std::result::Result<(), String> {
    match section {
        Section::Root => match key {
            "version" => lockfile.version = parse_u32(raw_value)?,
            "manifest" => lockfile.manifest = parse_string(raw_value)?,
            "manifest-digest" => lockfile.manifest_digest = parse_string(raw_value)?,
            "workspace-script" => lockfile.workspace_script = Some(parse_string(raw_value)?),
            "workspace-script-digest" => {
                lockfile.workspace_script_digest = Some(parse_string(raw_value)?)
            }
            _ => return Err(format!("unsupported root key `{key}`")),
        },
        Section::Package(index) => {
            let package = &mut lockfile.packages[index];
            match key {
                "id" => package.id = parse_string(raw_value)?,
                "name" => package.name = parse_string(raw_value)?,
                "version" => package.version = parse_string(raw_value)?,
                "source" => package.source_kind = parse_string(raw_value)?,
                "source-value" => package.source_value = Some(parse_string(raw_value)?),
                "manifest" => package.manifest = parse_string(raw_value)?,
                "manifest-digest" => package.manifest_digest = parse_string(raw_value)?,
                "craft-script" => package.craft_script = Some(parse_string(raw_value)?),
                "craft-script-digest" => {
                    package.craft_script_digest = Some(parse_string(raw_value)?)
                }
                _ => return Err(format!("unsupported [[package]] key `{key}`")),
            }
        }
        Section::ExternalPackage(index) => {
            let package = &mut lockfile.external_packages[index];
            match key {
                "id" => package.id = parse_string(raw_value)?,
                "name" => package.name = parse_string(raw_value)?,
                "source" => package.source_kind = parse_string(raw_value)?,
                "source-value" => package.source_value = Some(parse_string(raw_value)?),
                "version" => package.version = Some(parse_string(raw_value)?),
                "source-locator" => package.source_locator = Some(parse_string(raw_value)?),
                "source-selector" => package.source_selector = Some(parse_string(raw_value)?),
                _ => return Err(format!("unsupported [[external-package]] key `{key}`")),
            }
        }
        Section::PackageTarget(index) => {
            let target = &mut lockfile.package_targets[index];
            match key {
                "package" => target.package_id = parse_string(raw_value)?,
                "kind" => target.kind = parse_string(raw_value)?,
                "name" => target.name = Some(parse_string(raw_value)?),
                "root" => target.root = parse_string(raw_value)?,
                _ => return Err(format!("unsupported [[package-target]] key `{key}`")),
            }
        }
        Section::WorkspaceEnv(index) => {
            let input = &mut lockfile.workspace_env[index];
            match key {
                "name" => input.name = parse_string(raw_value)?,
                "value" => input.value = Some(parse_string(raw_value)?),
                _ => return Err(format!("unsupported [[workspace-env]] key `{key}`")),
            }
        }
        Section::PackageEnv(index) => {
            let input = &mut lockfile.package_env[index];
            match key {
                "package" => input.package_id = parse_string(raw_value)?,
                "name" => input.name = parse_string(raw_value)?,
                "value" => input.value = Some(parse_string(raw_value)?),
                _ => return Err(format!("unsupported [[package-env]] key `{key}`")),
            }
        }
        Section::Dependency(index) => {
            let dependency = &mut lockfile.dependencies[index];
            match key {
                "from" => dependency.from = parse_string(raw_value)?,
                "kind" => dependency.kind = parse_string(raw_value)?,
                "name" => dependency.name = parse_string(raw_value)?,
                "package" => dependency.package = parse_string(raw_value)?,
                "target" => dependency.target_kind = parse_string(raw_value)?,
                "target-id" => dependency.target_id = parse_string(raw_value)?,
                _ => return Err(format!("unsupported [[dependency]] key `{key}`")),
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
