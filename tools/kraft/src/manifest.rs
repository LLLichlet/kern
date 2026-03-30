use crate::error::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

#[derive(Debug, Default)]
pub struct Manifest {
    pub package: Option<Package>,
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
    pub edition: Option<String>,
    pub publish: Option<bool>,
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

#[derive(Debug)]
pub enum DependencySpec {
    Version(String),
    Detailed(DetailedDependency),
}

#[derive(Debug, Default)]
pub struct DetailedDependency {
    pub version: Option<String>,
    pub path: Option<String>,
    pub registry: Option<String>,
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
    pub edition: Option<String>,
    pub license: Option<String>,
    pub authors: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
enum Section {
    Root,
    Package,
    Lib,
    Bin(usize),
    Test(usize),
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

    fn parse(source: &str, path: &Path) -> Result<Self> {
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
            assign_key_value(&mut manifest, section, key, raw_value).map_err(|message| {
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
            if let Some(edition) = &package.edition {
                validate_non_empty(path, "[package].edition", edition)?;
            }
            let _ = package.publish;
        }

        if let Some(lib) = &self.lib {
            validate_non_empty(path, "[lib].root", &lib.root)?;
        }

        validate_named_targets(path, "[[bin]]", &self.bin)?;
        validate_named_targets(path, "[[test]]", &self.test)?;
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
                if let Some(edition) = &package.edition {
                    validate_non_empty(path, "[workspace.package].edition", edition)?;
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
            "[[test]]" => {
                self.test.push(NamedTarget::default());
                Ok(Section::Test(self.test.len() - 1))
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
    match line {
        "[package]" => {
            manifest.package.get_or_insert_with(Package::default);
            Ok(Section::Package)
        }
        "[lib]" => {
            manifest.lib.get_or_insert_with(LibTarget::default);
            Ok(Section::Lib)
        }
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
    section: Section,
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
                "edition" => package.edition = Some(parse_string(raw_value)?),
                "publish" => package.publish = Some(parse_bool(raw_value)?),
                _ => return Err(format!("unsupported [package] key `{key}`")),
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
            assign_named_target(&mut manifest.bin[index], "[[bin]]", key, raw_value)
        }
        Section::Test(index) => {
            assign_named_target(&mut manifest.test[index], "[[test]]", key, raw_value)
        }
        Section::Example(index) => {
            assign_named_target(&mut manifest.example[index], "[[example]]", key, raw_value)
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
                "edition" => workspace_package.edition = Some(parse_string(raw_value)?),
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
                let has_locator =
                    dep.version.is_some() || dep.path.is_some() || dep.registry.is_some();
                if !has_locator {
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

fn validate_non_empty(path: &Path, field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::Validation {
            path: path.to_path_buf(),
            message: format!("{field} must not be empty"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Manifest;

    #[test]
    fn parses_package_manifest() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"

[lib]
root = "src/lib.kr"

[[bin]]
name = "demo"
root = "src/main.kr"

[dependencies]
log = "1"
alloc = { path = "../alloc", features = ["arena"] }

[features]
default = []
"#,
            std::path::Path::new("Kraft.toml"),
        )
        .unwrap();

        assert_eq!(manifest.package.unwrap().name, "demo");
        assert!(manifest.lib.is_some());
        assert_eq!(manifest.bin.len(), 1);
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
            std::path::Path::new("Kraft.toml"),
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
kern = "0.7"

[profile.dev]
opt = 7
"#,
            std::path::Path::new("Kraft.toml"),
        )
        .unwrap();

        let path = std::path::Path::new("Kraft.toml");
        let err = manifest.validate(path).unwrap_err();
        assert!(err.to_string().contains("0..=3"));
    }
}
