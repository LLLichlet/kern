use crate::error::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

#[derive(Debug, Default)]
pub struct Manifest {
    pub package: Option<Package>,
    pub craft: Option<CraftConfig>,
    pub sources: BTreeMap<String, SourceConfig>,
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

#[derive(Debug, Default)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub kern: String,
    pub publish: Option<bool>,
}

#[derive(Debug, Default)]
pub struct CraftConfig {
    pub env: Vec<String>,
    pub release_source_policy: Option<ReleaseSourcePolicy>,
    pub allow_floating_git: Vec<String>,
    pub allow_insecure_source: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SourceConfig {
    pub directory: Option<String>,
    pub git: Option<String>,
    pub rev: Option<String>,
    pub branch: Option<String>,
    pub tag: Option<String>,
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

#[derive(Debug, Default)]
pub struct LibTarget {
    pub root: String,
}

#[derive(Debug, Default)]
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
    pub registry: Option<String>,
    pub workspace: Option<bool>,
    pub package: Option<String>,
    pub optional: Option<bool>,
    pub default_features: Option<bool>,
    pub features: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Profiles {
    pub dev: Option<Profile>,
    pub release: Option<Profile>,
}

#[derive(Debug, Default)]
pub struct Profile {
    pub opt: Option<u8>,
    pub debug: Option<bool>,
}

#[derive(Debug, Default)]
pub struct Workspace {
    pub members: Vec<String>,
    pub package: Option<WorkspacePackage>,
    pub dependencies: BTreeMap<String, DependencySpec>,
}

#[derive(Debug, Default)]
pub struct WorkspacePackage {
    pub version: Option<String>,
    pub license: Option<String>,
    pub authors: Vec<String>,
}

const CURRENT_KERN_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
enum Section {
    Root,
    Package,
    Craft,
    Source(String),
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

    pub fn validate(&self, path: &Path) -> Result<()> {
        if self.package.is_none() && self.workspace.is_none() {
            return Err(Error::Validation {
                path: path.to_path_buf(),
                message: "manifest must declare at least one of `[package]` or `[workspace]`"
                    .to_string(),
            });
        }

        if let Some(package) = &self.package {
            validate_non_empty(path, "[package].name", &package.name)?;
            validate_non_empty(path, "[package].version", &package.version)?;
            validate_non_empty(path, "[package].kern", &package.kern)?;
            validate_kern_version(path, &package.kern)?;
            let _ = package.publish;
        }

        if let Some(craft) = &self.craft {
            for name in &craft.env {
                validate_env_name(path, "[craft].env[]", name)?;
            }
            if let Some(policy) = craft.release_source_policy {
                let _ = policy;
            }
            for name in &craft.allow_floating_git {
                validate_source_name(path, "[craft].allow-floating-git[]", name)?;
            }
            for name in &craft.allow_insecure_source {
                validate_source_name(path, "[craft].allow-insecure-source[]", name)?;
            }
        }

        for (name, source) in &self.sources {
            validate_source_name(path, "[source]", name)?;
            validate_source_config(path, name, source)?;
        }

        if let Some(lib) = &self.lib {
            validate_non_empty(path, "[lib].root", &lib.root)?;
        }

        validate_named_targets(path, "[[bin]]", &self.bin)?;
        validate_test_targets(path, &self.test)?;
        validate_named_targets(path, "[[example]]", &self.example)?;

        validate_dependencies(path, "[dependencies]", &self.dependencies)?;
        validate_dependencies(path, "[dev-dependencies]", &self.dev_dependencies)?;
        validate_dependencies(path, "[build-dependencies]", &self.build_dependencies)?;

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
            for member in &workspace.members {
                validate_non_empty(path, "[workspace].members[]", member)?;
            }
            validate_dependencies(path, "[workspace.dependencies]", &workspace.dependencies)?;
            if let Some(package) = &workspace.package {
                if let Some(version) = &package.version {
                    validate_non_empty(path, "[workspace.package].version", version)?;
                }
                if let Some(license) = &package.license {
                    validate_non_empty(path, "[workspace.package].license", license)?;
                }
                for author in &package.authors {
                    validate_non_empty(path, "[workspace.package].authors[]", author)?;
                }
            }
        }

        Ok(())
    }

    fn enter_array_section(&mut self, line: &str) -> std::result::Result<Section, String> {
        match line {
            "[[bin]]" => {
                self.bin.push(NamedTarget::default());
                Ok(Section::Bin(self.bin.len() - 1))
            }
            "[[example]]" => {
                self.example.push(NamedTarget::default());
                Ok(Section::Example(self.example.len() - 1))
            }
            _ => Err(format!("unsupported array table `{line}`")),
        }
    }
}

fn enter_table_section(
    manifest: &mut Manifest,
    line: &str,
) -> std::result::Result<Section, String> {
    if let Some(name) = line
        .strip_prefix("[source.")
        .and_then(|tail| tail.strip_suffix(']'))
    {
        manifest.sources.entry(name.to_string()).or_default();
        return Ok(Section::Source(name.to_string()));
    }

    match line {
        "[package]" => {
            manifest.package.get_or_insert_with(Package::default);
            Ok(Section::Package)
        }
        "[craft]" => {
            manifest.craft.get_or_insert_with(CraftConfig::default);
            Ok(Section::Craft)
        }
        "[lib]" => {
            manifest.lib.get_or_insert_with(LibTarget::default);
            Ok(Section::Lib)
        }
        "[test]" => Ok(Section::Test),
        "[dependencies]" => Ok(Section::Dependencies),
        "[dev-dependencies]" => Ok(Section::DevDependencies),
        "[build-dependencies]" => Ok(Section::BuildDependencies),
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
                _ => return Err(format!("unsupported [package] key `{key}`")),
            }
            Ok(())
        }
        Section::Craft => {
            let craft = manifest.craft.get_or_insert_with(CraftConfig::default);
            match key {
                "env" => craft.env = parse_string_array(raw_value)?,
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
        Section::Source(name) => {
            let source = manifest.sources.entry(name.clone()).or_default();
            match key {
                "directory" => source.directory = Some(parse_string(raw_value)?),
                "git" => source.git = Some(parse_string(raw_value)?),
                "rev" => source.rev = Some(parse_string(raw_value)?),
                "branch" => source.branch = Some(parse_string(raw_value)?),
                "tag" => source.tag = Some(parse_string(raw_value)?),
                _ => return Err(format!("unsupported [source] key `{key}`")),
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
        Section::Example(index) => {
            assign_named_target(&mut manifest.example[*index], "[[example]]", key, raw_value)
        }
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
                "license" => workspace_package.license = Some(parse_string(raw_value)?),
                "authors" => workspace_package.authors = parse_string_array(raw_value)?,
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
        "roots" => manifest.test = parse_test_roots(raw_value)?,
        _ => return Err(format!("unsupported [test] key `{key}`")),
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
        _ => return Err(format!("unsupported {section} key `{key}`")),
    }
    Ok(())
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

fn parse_u8(raw: &str) -> std::result::Result<u8, String> {
    raw.trim()
        .parse::<u8>()
        .map_err(|_| format!("expected integer in 0..=255, found `{}`", raw.trim()))
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
            "registry" => dep.registry = Some(parse_string(value)?),
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

fn parse_test_roots(raw_value: &str) -> std::result::Result<Vec<NamedTarget>, String> {
    let roots = parse_string_array(raw_value)?;
    let mut targets = Vec::new();
    for root in roots {
        if contains_glob_pattern(&root) {
            return Err(format!(
                "[test].roots does not support glob patterns, list files explicitly: `{root}`"
            ));
        }
        let path = Path::new(&root);
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            return Err(format!(
                "[test].roots entries must end in a UTF-8 file name, found `{root}`"
            ));
        };
        if name.is_empty() {
            return Err(format!(
                "[test].roots entries must provide a non-empty file stem, found `{root}`"
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

fn validate_named_targets(path: &Path, section: &str, targets: &[NamedTarget]) -> Result<()> {
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

fn validate_test_targets(path: &Path, targets: &[NamedTarget]) -> Result<()> {
    let mut names = BTreeSet::new();
    for target in targets {
        validate_non_empty(path, "[test].roots[]", &target.root)?;
        if !names.insert(target.name.as_str()) {
            return Err(Error::Validation {
                path: path.to_path_buf(),
                message: format!("duplicate test file stem `{}` in [test].roots", target.name),
            });
        }
    }
    Ok(())
}

fn validate_dependencies(
    path: &Path,
    section: &str,
    deps: &BTreeMap<String, DependencySpec>,
) -> Result<()> {
    for (name, spec) in deps {
        validate_non_empty(path, &format!("{section} key"), name)?;
        match spec {
            DependencySpec::Version(version) => {
                validate_non_empty(path, &format!("{section}.{name}"), version)?;
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
                    && (dep.version.is_some() || dep.path.is_some() || dep.registry.is_some())
                {
                    return Err(Error::Validation {
                        path: path.to_path_buf(),
                        message: format!(
                            "{section}.{name} cannot combine `workspace = true` with `version`, `path`, or `registry`"
                        ),
                    });
                }

                let has_locator =
                    dep.version.is_some() || dep.path.is_some() || dep.registry.is_some();
                if dep.workspace != Some(true) && !has_locator {
                    return Err(Error::Validation {
                        path: path.to_path_buf(),
                        message: format!(
                            "{section}.{name} must declare at least one of `version`, `path`, or `registry`"
                        ),
                    });
                }

                if let Some(version) = &dep.version {
                    validate_non_empty(path, &format!("{section}.{name}.version"), version)?;
                }
                if let Some(path_value) = &dep.path {
                    validate_non_empty(path, &format!("{section}.{name}.path"), path_value)?;
                }
                if let Some(registry) = &dep.registry {
                    validate_non_empty(path, &format!("{section}.{name}.registry"), registry)?;
                }
                if let Some(package) = &dep.package {
                    validate_non_empty(path, &format!("{section}.{name}.package"), package)?;
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

fn validate_profile(path: &Path, section: &str, profile: &Profile) -> Result<()> {
    if let Some(opt) = profile.opt
        && opt > 3
    {
        return Err(Error::Validation {
            path: path.to_path_buf(),
            message: format!("{section}.opt must be in the range 0..=3"),
        });
    }
    let _ = profile.debug;
    Ok(())
}

fn validate_source_config(path: &Path, name: &str, source: &SourceConfig) -> Result<()> {
    let has_directory = source.directory.is_some();
    let has_git = source.git.is_some();
    match (has_directory, has_git) {
        (false, false) => {
            return Err(Error::Validation {
                path: path.to_path_buf(),
                message: format!("[source.{name}] must declare either `directory` or `git`"),
            });
        }
        (true, true) => {
            return Err(Error::Validation {
                path: path.to_path_buf(),
                message: format!(
                    "[source.{name}] cannot combine `directory` and `git` source definitions"
                ),
            });
        }
        _ => {}
    }

    if let Some(directory) = &source.directory {
        validate_non_empty(path, &format!("[source.{name}].directory"), directory)?;
    }
    if let Some(git) = &source.git {
        validate_non_empty(path, &format!("[source.{name}].git"), git)?;
    }

    let selector_count = usize::from(source.rev.is_some())
        + usize::from(source.branch.is_some())
        + usize::from(source.tag.is_some());
    if selector_count > 1 {
        return Err(Error::Validation {
            path: path.to_path_buf(),
            message: format!("[source.{name}] may set at most one of `rev`, `branch`, or `tag`"),
        });
    }

    if !has_git && selector_count > 0 {
        return Err(Error::Validation {
            path: path.to_path_buf(),
            message: format!(
                "[source.{name}] can only use `rev`, `branch`, or `tag` with `git` sources"
            ),
        });
    }

    if let Some(rev) = &source.rev {
        validate_non_empty(path, &format!("[source.{name}].rev"), rev)?;
    }
    if let Some(branch) = &source.branch {
        validate_non_empty(path, &format!("[source.{name}].branch"), branch)?;
    }
    if let Some(tag) = &source.tag {
        validate_non_empty(path, &format!("[source.{name}].tag"), tag)?;
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

fn validate_env_name(path: &Path, field: &str, value: &str) -> Result<()> {
    validate_non_empty(path, field, value)?;
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(Error::Validation {
            path: path.to_path_buf(),
            message: format!("{field} must not be empty"),
        });
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return Err(Error::Validation {
            path: path.to_path_buf(),
            message: format!("{field} must start with an ASCII letter or `_`, found `{value}`"),
        });
    }
    if chars.any(|ch| !(ch == '_' || ch.is_ascii_alphanumeric())) {
        return Err(Error::Validation {
            path: path.to_path_buf(),
            message: format!(
                "{field} must contain only ASCII letters, digits, or `_`, found `{value}`"
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

#[cfg(test)]
mod tests {
    use super::{DependencySpec, Manifest, ReleaseSourcePolicy};

    #[test]
    fn parses_package_manifest() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"

[lib]
root = "src/lib.rn"

[[bin]]
name = "demo"
root = "src/main.rn"

[test]
roots = ["tests/smoke.rn", "tests/env.rn"]

[dependencies]
log = "1"
alloc = { path = "../alloc", features = ["arena"] }

[features]
default = []
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        assert_eq!(manifest.package.unwrap().name, "demo");
        assert!(manifest.lib.is_some());
        assert_eq!(manifest.bin.len(), 1);
        assert_eq!(manifest.test.len(), 2);
        assert_eq!(manifest.test[0].name, "smoke");
        assert_eq!(manifest.test[1].name, "env");
        assert_eq!(manifest.dependencies.len(), 2);
    }

    #[test]
    fn parses_workspace_manifest() {
        let manifest = Manifest::parse(
            r#"
[workspace]
members = [
  "compiler/*",
  "tools/*",
]

[workspace.package]
license = "MIT"
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let workspace = manifest.workspace.unwrap();
        assert_eq!(workspace.members.len(), 2);
        assert_eq!(workspace.package.unwrap().license.unwrap(), "MIT");
    }

    #[test]
    fn rejects_invalid_profile_opt() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"

[profile.dev]
opt = 7
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let path = std::path::Path::new("Craft.toml");
        let err = manifest.validate(path).unwrap_err();
        assert!(err.to_string().contains("0..=3"));
    }

    #[test]
    fn parses_workspace_inherited_dependency() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"

[dependencies]
shared = { workspace = true, features = ["simd"] }
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let dep = manifest.dependencies.get("shared").unwrap();
        let DependencySpec::Detailed(dep) = dep else {
            panic!("expected detailed dependency");
        };

        assert_eq!(dep.workspace, Some(true));
        assert_eq!(dep.features, vec!["simd"]);
    }

    #[test]
    fn rejects_workspace_inheritance_locators_in_member_dependency() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"

[dependencies]
shared = { workspace = true, version = "2" }
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let path = std::path::Path::new("Craft.toml");
        let err = manifest.validate(path).unwrap_err();
        assert!(
            err.to_string()
                .contains("cannot combine `workspace = true`")
        );
    }

    #[test]
    fn rejects_workspace_inheritance_inside_workspace_dependencies() {
        let manifest = Manifest::parse(
            r#"
[workspace]
members = ["app"]

[workspace.dependencies]
shared = { workspace = true }
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let path = std::path::Path::new("Craft.toml");
        let err = manifest.validate(path).unwrap_err();
        assert!(err.to_string().contains("cannot use `workspace = true`"));
    }

    #[test]
    fn parses_craft_env_allowlist() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"

[craft]
env = ["USE_SYSTEM_SSL", "KERN_SYSROOT"]
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let craft = manifest.craft.unwrap();
        assert_eq!(craft.env, vec!["USE_SYSTEM_SSL", "KERN_SYSROOT"]);
    }

    #[test]
    fn parses_craft_release_source_policy_overrides() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"

[craft]
release-source-policy = "warn"
allow-floating-git = ["default"]
allow-insecure-source = ["mirror"]
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let craft = manifest.craft.unwrap();
        assert_eq!(craft.release_source_policy, Some(ReleaseSourcePolicy::Warn));
        assert_eq!(craft.allow_floating_git, vec!["default"]);
        assert_eq!(craft.allow_insecure_source, vec!["mirror"]);
    }

    #[test]
    fn parses_named_source_tables() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"

[source.default]
directory = "vendor/default"

[source.corp]
git = "https://example.com/corp.git"
branch = "stable"
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        assert_eq!(
            manifest
                .sources
                .get("default")
                .and_then(|source| source.directory.as_deref()),
            Some("vendor/default")
        );
        assert_eq!(
            manifest
                .sources
                .get("corp")
                .and_then(|source| source.git.as_deref()),
            Some("https://example.com/corp.git")
        );
        assert_eq!(
            manifest
                .sources
                .get("corp")
                .and_then(|source| source.branch.as_deref()),
            Some("stable")
        );
    }

    #[test]
    fn rejects_source_without_directory() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"

[source.default]
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let path = std::path::Path::new("Craft.toml");
        let err = manifest.validate(path).unwrap_err();
        assert!(
            err.to_string()
                .contains("must declare either `directory` or `git`")
        );
    }

    #[test]
    fn rejects_source_with_multiple_backends() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"

[source.default]
directory = "vendor/default"
git = "https://example.com/default.git"
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let path = std::path::Path::new("Craft.toml");
        let err = manifest.validate(path).unwrap_err();
        assert!(
            err.to_string()
                .contains("cannot combine `directory` and `git`")
        );
    }

    #[test]
    fn rejects_git_source_with_multiple_selectors() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"

[source.default]
git = "https://example.com/default.git"
branch = "main"
tag = "v1"
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let path = std::path::Path::new("Craft.toml");
        let err = manifest.validate(path).unwrap_err();
        assert!(
            err.to_string()
                .contains("at most one of `rev`, `branch`, or `tag`")
        );
    }

    #[test]
    fn rejects_non_git_source_with_git_selector() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"

[source.default]
directory = "vendor/default"
rev = "abc123"
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let path = std::path::Path::new("Craft.toml");
        let err = manifest.validate(path).unwrap_err();
        assert!(
            err.to_string()
                .contains("can only use `rev`, `branch`, or `tag` with `git` sources")
        );
    }

    #[test]
    fn rejects_invalid_craft_env_names() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"

[craft]
env = ["1BAD-NAME"]
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let path = std::path::Path::new("Craft.toml");
        let err = manifest.validate(path).unwrap_err();
        assert!(err.to_string().contains("[craft].env[]"));
    }

    #[test]
    fn rejects_invalid_release_source_policy_value() {
        let err = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"

[craft]
release-source-policy = "strict"
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap_err();

        assert!(err.to_string().contains("release-source-policy"));
    }

    #[test]
    fn rejects_package_edition_field() {
        let err = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"
edition = "2027"
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("unsupported [package] key `edition`")
        );
    }

    #[test]
    fn rejects_workspace_package_edition_field() {
        let err = Manifest::parse(
            r#"
[workspace]
members = ["app"]

[workspace.package]
edition = "2027"
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("unsupported [workspace.package] key `edition`")
        );
    }

    #[test]
    fn rejects_mismatched_kern_version() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let err = manifest
            .validate(std::path::Path::new("Craft.toml"))
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("must match the current toolchain version")
        );
    }

    #[test]
    fn rejects_duplicate_test_file_stems() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"

[test]
roots = ["tests/smoke.rn", "alt/smoke.rn"]
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let err = manifest
            .validate(std::path::Path::new("Craft.toml"))
            .unwrap_err();
        assert!(err.to_string().contains("duplicate test file stem `smoke`"));
    }

    #[test]
    fn rejects_glob_patterns_in_test_roots() {
        let err = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"

[test]
roots = ["tests/*"]
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap_err();

        assert!(err.to_string().contains("does not support glob patterns"));
    }

    #[test]
    fn rejects_legacy_array_style_test_targets() {
        let err = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.6"

[[test]]
name = "smoke"
root = "tests/smoke.rn"
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("unsupported array table `[[test]]`")
        );
    }
}
