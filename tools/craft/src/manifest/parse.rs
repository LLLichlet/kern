use super::{
    CraftConfig, CraftFmtConfig, CraftStyleConfig, CraftStyleSuggestionLevel, DependencySpec,
    DetailedDependency, LibTarget, Manifest, NamedTarget, Package, Profile, Profiles,
    ReleaseSourcePolicy, ResourceSpec, RuntimeConfig, Section, Workspace, WorkspacePackage,
};
use crate::error::{Error, Result};
use kernc_utils::config::{LibraryBundle, LtoMode, RuntimeEntry};
use std::path::Path;

impl Manifest {
    pub(crate) fn parse(source: &str, path: &Path) -> Result<Self> {
        let mut manifest = Self::default();
        let mut section = Section::Root;

        for line in logical_lines(source).map_err(|message| Error::ManifestParse {
            path: path.to_path_buf(),
            message,
        })? {
            if line.starts_with("[[") {
                section = manifest.enter_array_section(&line).map_err(|message| {
                    Error::ManifestParse {
                        path: path.to_path_buf(),
                        message,
                    }
                })?;
                continue;
            }

            if line.starts_with('[') {
                section = enter_table_section(&mut manifest, &line).map_err(|message| {
                    Error::ManifestParse {
                        path: path.to_path_buf(),
                        message,
                    }
                })?;
                continue;
            }

            let (key, raw_value) =
                split_key_value(&line).map_err(|message| Error::ManifestParse {
                    path: path.to_path_buf(),
                    message,
                })?;
            assign_key_value(&mut manifest, &section, key, raw_value).map_err(|message| {
                Error::ManifestParse {
                    path: path.to_path_buf(),
                    message,
                }
            })?;
        }

        Ok(manifest)
    }

    fn enter_array_section(&mut self, line: &str) -> std::result::Result<Section, String> {
        match line {
            "[[bin]]" => {
                self.bin.push(NamedTarget::default());
                Ok(Section::Bin(self.bin.len() - 1))
            }
            _ => Err(format!("unsupported array table `{line}`")),
        }
    }
}

fn enter_table_section(
    manifest: &mut Manifest,
    line: &str,
) -> std::result::Result<Section, String> {
    match line {
        "[package]" => {
            manifest.package.get_or_insert_with(Package::default);
            Ok(Section::Package)
        }
        "[craft]" => {
            manifest.craft.get_or_insert_with(CraftConfig::default);
            Ok(Section::Craft)
        }
        "[craft.fmt]" => {
            let craft = manifest.craft.get_or_insert_with(CraftConfig::default);
            craft.fmt.get_or_insert_with(CraftFmtConfig::default);
            Ok(Section::CraftFmt)
        }
        "[craft.style]" => {
            let craft = manifest.craft.get_or_insert_with(CraftConfig::default);
            craft.style.get_or_insert_with(CraftStyleConfig::default);
            Ok(Section::CraftStyle)
        }
        "[runtime]" => {
            manifest.runtime.get_or_insert_with(RuntimeConfig::default);
            Ok(Section::Runtime)
        }
        "[lib]" => {
            manifest.lib.get_or_insert_with(LibTarget::default);
            Ok(Section::Lib)
        }
        "[test]" => Ok(Section::Test),
        "[example]" => Ok(Section::Example),
        "[dependencies]" => Ok(Section::Dependencies),
        "[dev-dependencies]" => Ok(Section::DevDependencies),
        "[build-dependencies]" => Ok(Section::BuildDependencies),
        "[resources]" => Ok(Section::Resources),
        "[features]" => Ok(Section::Features),
        "[profile.dev]" => {
            let profiles = manifest.profile.get_or_insert_with(Profiles::default);
            profiles.dev.get_or_insert_with(Profile::default);
            Ok(Section::ProfileDev)
        }
        "[profile.release]" => {
            let profiles = manifest.profile.get_or_insert_with(Profiles::default);
            profiles.release.get_or_insert_with(Profile::default);
            Ok(Section::ProfileRelease)
        }
        "[workspace]" => {
            manifest.workspace.get_or_insert_with(Workspace::default);
            Ok(Section::Workspace)
        }
        "[workspace.package]" => {
            let workspace = manifest.workspace.get_or_insert_with(Workspace::default);
            workspace
                .package
                .get_or_insert_with(WorkspacePackage::default);
            Ok(Section::WorkspacePackage)
        }
        "[workspace.dependencies]" => {
            manifest.workspace.get_or_insert_with(Workspace::default);
            Ok(Section::WorkspaceDependencies)
        }
        _ => Err(format!("unsupported table `{line}`")),
    }
}

fn assign_key_value(
    manifest: &mut Manifest,
    section: &Section,
    key: &str,
    raw_value: &str,
) -> std::result::Result<(), String> {
    match section {
        Section::Root => Err(format!("unexpected root key `{key}`")),
        Section::Package => {
            let package = manifest.package.get_or_insert_with(Package::default);
            match key {
                "name" => package.name = parse_string(raw_value)?,
                "version" => package.version = parse_string(raw_value)?,
                "kern" => package.kern = parse_string(raw_value)?,
                "publish" => package.publish = Some(parse_bool(raw_value)?),
                "description" => package.description = Some(parse_string(raw_value)?),
                "license" => package.license = Some(parse_string(raw_value)?),
                "authors" => package.authors = parse_string_array(raw_value)?,
                "readme" => package.readme = Some(parse_string(raw_value)?),
                "repository" => package.repository = Some(parse_string(raw_value)?),
                "homepage" => package.homepage = Some(parse_string(raw_value)?),
                "documentation" => package.documentation = Some(parse_string(raw_value)?),
                _ => return Err(format!("unsupported [package] key `{key}`")),
            }
            Ok(())
        }
        Section::Craft => {
            let craft = manifest.craft.get_or_insert_with(CraftConfig::default);
            match key {
                "release-source-policy" => {
                    craft.release_source_policy = Some(parse_release_source_policy(raw_value)?)
                }
                "allow-floating-git" => craft.allow_floating_git = parse_string_array(raw_value)?,
                "allow-insecure-source" => {
                    craft.allow_insecure_source = parse_string_array(raw_value)?
                }
                _ => return Err(format!("unsupported [craft] key `{key}`")),
            }
            Ok(())
        }
        Section::CraftFmt => {
            let fmt = manifest
                .craft
                .get_or_insert_with(CraftConfig::default)
                .fmt
                .get_or_insert_with(CraftFmtConfig::default);
            match key {
                "line-width" => fmt.line_width = Some(parse_usize(raw_value)?),
                "postfix-chain-threshold" => {
                    fmt.postfix_chain_threshold = Some(parse_usize(raw_value)?)
                }
                "boolean-chain-threshold" => {
                    fmt.boolean_chain_threshold = Some(parse_usize(raw_value)?)
                }
                "function-parameter-threshold" => {
                    fmt.function_parameter_threshold = Some(parse_usize(raw_value)?)
                }
                "call-argument-threshold" => {
                    fmt.call_argument_threshold = Some(parse_usize(raw_value)?)
                }
                "exclude" => fmt.exclude = parse_string_array(raw_value)?,
                _ => return Err(format!("unsupported [craft.fmt] key `{key}`")),
            }
            Ok(())
        }
        Section::CraftStyle => {
            let style = manifest
                .craft
                .get_or_insert_with(CraftConfig::default)
                .style
                .get_or_insert_with(CraftStyleConfig::default);
            match key {
                "suggestions" => {
                    style.suggestions = Some(parse_craft_style_suggestion_level(raw_value)?)
                }
                "disabled-rules" => style.disabled_rules = parse_string_array(raw_value)?,
                "exclude" => style.exclude = parse_string_array(raw_value)?,
                _ => return Err(format!("unsupported [craft.style] key `{key}`")),
            }
            Ok(())
        }
        Section::Runtime => {
            let runtime = manifest.runtime.get_or_insert_with(RuntimeConfig::default);
            match key {
                "entry" => runtime.entry = Some(parse_runtime_entry(raw_value)?),
                "libc" => runtime.libc = Some(parse_bool(raw_value)?),
                "bundle" => runtime.bundle = Some(parse_library_bundle(raw_value)?),
                _ => return Err(format!("unsupported [runtime] key `{key}`")),
            }
            Ok(())
        }
        Section::Lib => {
            let lib = manifest.lib.get_or_insert_with(LibTarget::default);
            match key {
                "root" => lib.root = parse_string(raw_value)?,
                _ => return Err(format!("unsupported [lib] key `{key}`")),
            }
            Ok(())
        }
        Section::Bin(index) => {
            assign_named_target(&mut manifest.bin[*index], "[[bin]]", key, raw_value)
        }
        Section::Test => assign_test_targets(manifest, key, raw_value),
        Section::Example => assign_example_targets(manifest, key, raw_value),
        Section::Dependencies => {
            manifest
                .dependencies
                .insert(key.to_string(), parse_dependency(raw_value)?);
            Ok(())
        }
        Section::DevDependencies => {
            manifest
                .dev_dependencies
                .insert(key.to_string(), parse_dependency(raw_value)?);
            Ok(())
        }
        Section::BuildDependencies => {
            manifest
                .build_dependencies
                .insert(key.to_string(), parse_dependency(raw_value)?);
            Ok(())
        }
        Section::Resources => {
            manifest
                .resources
                .insert(key.to_string(), parse_resource(raw_value)?);
            Ok(())
        }
        Section::Features => {
            manifest
                .features
                .insert(key.to_string(), parse_string_array(raw_value)?);
            Ok(())
        }
        Section::ProfileDev => {
            let profile = manifest
                .profile
                .get_or_insert_with(Profiles::default)
                .dev
                .get_or_insert_with(Profile::default);
            assign_profile(profile, "[profile.dev]", key, raw_value)
        }
        Section::ProfileRelease => {
            let profile = manifest
                .profile
                .get_or_insert_with(Profiles::default)
                .release
                .get_or_insert_with(Profile::default);
            assign_profile(profile, "[profile.release]", key, raw_value)
        }
        Section::Workspace => {
            let workspace = manifest.workspace.get_or_insert_with(Workspace::default);
            match key {
                "members" => workspace.members = parse_string_array(raw_value)?,
                _ => return Err(format!("unsupported [workspace] key `{key}`")),
            }
            Ok(())
        }
        Section::WorkspacePackage => {
            let workspace_package = manifest
                .workspace
                .get_or_insert_with(Workspace::default)
                .package
                .get_or_insert_with(WorkspacePackage::default);
            match key {
                "version" => workspace_package.version = Some(parse_string(raw_value)?),
                "description" => workspace_package.description = Some(parse_string(raw_value)?),
                "license" => workspace_package.license = Some(parse_string(raw_value)?),
                "authors" => workspace_package.authors = parse_string_array(raw_value)?,
                "readme" => workspace_package.readme = Some(parse_string(raw_value)?),
                "repository" => workspace_package.repository = Some(parse_string(raw_value)?),
                "homepage" => workspace_package.homepage = Some(parse_string(raw_value)?),
                "documentation" => workspace_package.documentation = Some(parse_string(raw_value)?),
                _ => return Err(format!("unsupported [workspace.package] key `{key}`")),
            }
            Ok(())
        }
        Section::WorkspaceDependencies => {
            manifest
                .workspace
                .get_or_insert_with(Workspace::default)
                .dependencies
                .insert(key.to_string(), parse_dependency(raw_value)?);
            Ok(())
        }
    }
}

fn assign_named_target(
    target: &mut NamedTarget,
    section: &str,
    key: &str,
    raw_value: &str,
) -> std::result::Result<(), String> {
    match key {
        "name" => target.name = parse_string(raw_value)?,
        "root" => target.root = parse_string(raw_value)?,
        _ => return Err(format!("unsupported {section} key `{key}`")),
    }
    Ok(())
}

fn assign_test_targets(
    manifest: &mut Manifest,
    key: &str,
    raw_value: &str,
) -> std::result::Result<(), String> {
    match key {
        "roots" => manifest.test = parse_target_roots("[test].roots", raw_value)?,
        _ => return Err(format!("unsupported [test] key `{key}`")),
    }
    Ok(())
}

fn assign_example_targets(
    manifest: &mut Manifest,
    key: &str,
    raw_value: &str,
) -> std::result::Result<(), String> {
    match key {
        "roots" => manifest.example = parse_target_roots("[example].roots", raw_value)?,
        _ => return Err(format!("unsupported [example] key `{key}`")),
    }
    Ok(())
}

fn assign_profile(
    profile: &mut Profile,
    section: &str,
    key: &str,
    raw_value: &str,
) -> std::result::Result<(), String> {
    match key {
        "opt" => profile.opt = Some(parse_u8(raw_value)?),
        "debug" => profile.debug = Some(parse_bool(raw_value)?),
        "codegen-units" => profile.codegen_units = Some(parse_usize(raw_value)?),
        "lto" => profile.lto = Some(parse_lto_mode(raw_value)?.as_str().to_string()),
        "code-model" => {
            profile.code_model = Some(parse_code_model(raw_value)?.as_str().to_string())
        }
        _ => return Err(format!("unsupported {section} key `{key}`")),
    }
    Ok(())
}

fn parse_code_model(
    raw_value: &str,
) -> std::result::Result<kernc_utils::config::CodeModel, String> {
    kernc_utils::config::CodeModel::parse(&parse_string(raw_value)?)
}

fn parse_lto_mode(raw_value: &str) -> std::result::Result<LtoMode, String> {
    LtoMode::parse(&parse_string(raw_value)?)
}

fn logical_lines(source: &str) -> std::result::Result<Vec<String>, String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut brace_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut in_string = false;
    let mut escape = false;

    for raw_line in source.lines() {
        let stripped = strip_comment(raw_line)?;
        let trimmed = stripped.trim();

        if trimmed.is_empty() && current.is_empty() {
            continue;
        }

        if !current.is_empty() && !trimmed.is_empty() {
            current.push(' ');
        }
        current.push_str(trimmed);

        scan_balance(
            trimmed,
            &mut brace_depth,
            &mut bracket_depth,
            &mut in_string,
            &mut escape,
        )?;

        if !in_string && brace_depth == 0 && bracket_depth == 0 && !current.trim().is_empty() {
            lines.push(current.trim().to_string());
            current.clear();
        }
    }

    if in_string || brace_depth != 0 || bracket_depth != 0 {
        return Err("unterminated string, array, or inline table".to_string());
    }

    if !current.trim().is_empty() {
        lines.push(current.trim().to_string());
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

fn scan_balance(
    input: &str,
    brace_depth: &mut usize,
    bracket_depth: &mut usize,
    in_string: &mut bool,
    escape: &mut bool,
) -> std::result::Result<(), String> {
    for ch in input.chars() {
        if *escape {
            *escape = false;
            continue;
        }

        if *in_string {
            match ch {
                '\\' => *escape = true,
                '"' => *in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => *in_string = true,
            '{' => *brace_depth += 1,
            '}' => {
                *brace_depth = brace_depth
                    .checked_sub(1)
                    .ok_or_else(|| "unexpected `}`".to_string())?;
            }
            '[' => *bracket_depth += 1,
            ']' => {
                *bracket_depth = bracket_depth
                    .checked_sub(1)
                    .ok_or_else(|| "unexpected `]`".to_string())?;
            }
            _ => {}
        }
    }

    Ok(())
}

fn split_key_value(line: &str) -> std::result::Result<(&str, &str), String> {
    let mut in_string = false;
    let mut escape = false;
    let mut brace_depth = 0usize;
    let mut bracket_depth = 0usize;

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
            '{' => brace_depth += 1,
            '}' => brace_depth -= 1,
            '[' => bracket_depth += 1,
            ']' => bracket_depth -= 1,
            '=' if brace_depth == 0 && bracket_depth == 0 => {
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

fn parse_runtime_entry(raw: &str) -> std::result::Result<RuntimeEntry, String> {
    RuntimeEntry::parse(&parse_string(raw)?)
}

fn parse_library_bundle(raw: &str) -> std::result::Result<LibraryBundle, String> {
    LibraryBundle::parse(&parse_string(raw)?)
}

fn parse_u8(raw: &str) -> std::result::Result<u8, String> {
    raw.trim()
        .parse::<u8>()
        .map_err(|_| format!("expected integer in 0..=255, found `{}`", raw.trim()))
}

fn parse_usize(raw: &str) -> std::result::Result<usize, String> {
    raw.trim()
        .parse::<usize>()
        .map_err(|_| format!("expected non-negative integer, found `{}`", raw.trim()))
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

fn parse_dependency(raw: &str) -> std::result::Result<DependencySpec, String> {
    let trimmed = raw.trim();
    if trimmed.starts_with('"') {
        return Ok(DependencySpec::Version(parse_string(trimmed)?));
    }

    let inner = strip_wrapping(trimmed, '{', '}')?;
    let mut dep = DetailedDependency::default();

    for item in split_top_level(inner, ',') {
        let (key, value) = split_key_value(item)?;
        match key {
            "version" => dep.version = Some(parse_string(value)?),
            "path" => dep.path = Some(parse_string(value)?),
            "git" => dep.git = Some(parse_string(value)?),
            "rev" => dep.rev = Some(parse_string(value)?),
            "branch" => dep.branch = Some(parse_string(value)?),
            "tag" => dep.tag = Some(parse_string(value)?),
            "workspace" => dep.workspace = Some(parse_bool(value)?),
            "package" => dep.package = Some(parse_string(value)?),
            "optional" => dep.optional = Some(parse_bool(value)?),
            "default-features" => dep.default_features = Some(parse_bool(value)?),
            "features" => dep.features = parse_string_array(value)?,
            _ => return Err(format!("unsupported dependency key `{key}`")),
        }
    }

    Ok(DependencySpec::Detailed(dep))
}

fn parse_resource(raw: &str) -> std::result::Result<ResourceSpec, String> {
    let inner = strip_wrapping(raw.trim(), '{', '}')?;
    let mut resource = ResourceSpec::default();

    for item in split_top_level(inner, ',') {
        let (key, value) = split_key_value(item)?;
        match key {
            "path" => resource.path = Some(parse_string(value)?),
            "git" => resource.git = Some(parse_string(value)?),
            "rev" => resource.rev = Some(parse_string(value)?),
            "branch" => resource.branch = Some(parse_string(value)?),
            "tag" => resource.tag = Some(parse_string(value)?),
            _ => return Err(format!("unsupported resource key `{key}`")),
        }
    }

    Ok(resource)
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

fn parse_target_roots(
    section: &str,
    raw_value: &str,
) -> std::result::Result<Vec<NamedTarget>, String> {
    let roots = parse_string_array(raw_value)?;
    let mut targets = Vec::new();
    for root in roots {
        if contains_glob_pattern(&root) {
            return Err(format!(
                "{section} does not support glob patterns, list files explicitly: `{root}`"
            ));
        }
        let path = Path::new(&root);
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            return Err(format!(
                "{section} entries must end in a UTF-8 file name, found `{root}`"
            ));
        };
        if name.is_empty() {
            return Err(format!(
                "{section} entries must provide a non-empty file stem, found `{root}`"
            ));
        }
        targets.push(NamedTarget {
            name: name.to_string(),
            root,
        });
    }
    Ok(targets)
}

fn contains_glob_pattern(path: &str) -> bool {
    path.contains('*') || path.contains('?') || path.contains('[')
}

fn parse_release_source_policy(
    raw_value: &str,
) -> std::result::Result<ReleaseSourcePolicy, String> {
    match parse_string(raw_value)?.as_str() {
        "enforce" => Ok(ReleaseSourcePolicy::Enforce),
        "warn" => Ok(ReleaseSourcePolicy::Warn),
        "off" => Ok(ReleaseSourcePolicy::Off),
        other => Err(format!(
            "[craft].release-source-policy has unsupported value `{other}`"
        )),
    }
}

fn parse_craft_style_suggestion_level(
    raw_value: &str,
) -> std::result::Result<CraftStyleSuggestionLevel, String> {
    match parse_string(raw_value)?.as_str() {
        "off" => Ok(CraftStyleSuggestionLevel::Off),
        "info" => Ok(CraftStyleSuggestionLevel::Info),
        "warn" => Ok(CraftStyleSuggestionLevel::Warn),
        other => Err(format!(
            "[craft.style].suggestions has unsupported value `{other}`"
        )),
    }
}
