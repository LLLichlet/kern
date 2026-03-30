use crate::error::{Error, Result};
use crate::graph::{DependencyKind, DependencyTarget, PackageGraph, PackageId, SourceId};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lockfile {
    pub version: u32,
    pub manifest: String,
    pub manifest_digest: String,
    pub packages: Vec<LockedPackage>,
    pub dependencies: Vec<LockedDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackage {
    pub id: String,
    pub name: String,
    pub version: String,
    pub source_kind: String,
    pub source_value: Option<String>,
    pub manifest: String,
    pub manifest_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedDependency {
    pub from: String,
    pub kind: String,
    pub name: String,
    pub package: String,
    pub target_kind: String,
    pub target_value: Option<String>,
    pub target_id: Option<String>,
    pub version: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockStatus {
    Missing,
    Current,
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockWriteResult {
    Created,
    Updated,
    Unchanged,
}

#[derive(Clone, Copy, Debug)]
enum Section {
    Root,
    Package(usize),
    Dependency(usize),
}

pub fn sync_lockfile(
    manifest_path: &Path,
    graph: &PackageGraph,
) -> Result<(PathBuf, LockWriteResult)> {
    let lock_path = graph.workspace_root.join("Kraft.lock");
    let expected = Lockfile::from_graph(manifest_path, graph)?;

    if lock_path.is_file() {
        let actual = Lockfile::load(&lock_path)?;
        if actual == expected {
            return Ok((lock_path, LockWriteResult::Unchanged));
        }
    }

    let result = if lock_path.is_file() {
        LockWriteResult::Updated
    } else {
        LockWriteResult::Created
    };

    fs::write(&lock_path, expected.render()).map_err(|err| Error::from_io(&lock_path, err))?;
    Ok((lock_path, result))
}

pub fn lock_status(manifest_path: &Path, graph: &PackageGraph) -> Result<LockStatus> {
    let lock_path = graph.workspace_root.join("Kraft.lock");
    if !lock_path.is_file() {
        return Ok(LockStatus::Missing);
    }

    let actual = Lockfile::load(&lock_path)?;
    let expected = Lockfile::from_graph(manifest_path, graph)?;
    if actual == expected {
        Ok(LockStatus::Current)
    } else {
        Ok(LockStatus::Stale)
    }
}

impl Lockfile {
    pub fn load(path: &Path) -> Result<Self> {
        let source = fs::read_to_string(path).map_err(|err| Error::from_io(path, err))?;
        let lockfile = Self::parse(&source, path)?;
        lockfile.validate(path)?;
        Ok(lockfile)
    }

    pub fn from_graph(manifest_path: &Path, graph: &PackageGraph) -> Result<Self> {
        let root = &graph.workspace_root;
        let manifest = relative_display(root, manifest_path);
        let manifest_digest = digest_file(manifest_path)?;

        let mut packages = Vec::new();
        let mut dependencies = Vec::new();

        for package in &graph.packages {
            let package_id = package_lock_id(&package.id);
            packages.push(LockedPackage {
                id: package_id.clone(),
                name: package.id.name.clone(),
                version: package.id.version.clone(),
                source_kind: source_kind(&package.id.source).to_string(),
                source_value: source_value(&package.id.source),
                manifest: relative_display(root, &package.manifest_path),
                manifest_digest: digest_file(&package.manifest_path)?,
            });

            for dep in &package.dependencies {
                let (target_kind, target_value, target_id, version) = match &dep.target {
                    DependencyTarget::Local(target) => (
                        "local",
                        None,
                        Some(package_lock_id(target)),
                        Some(target.version.clone()),
                    ),
                    DependencyTarget::External(target) => (
                        source_kind(&target.source),
                        source_value(&target.source),
                        None,
                        target.version.clone(),
                    ),
                };

                dependencies.push(LockedDependency {
                    from: package_id.clone(),
                    kind: dependency_kind(dep.kind).to_string(),
                    name: dep.dependency_name.clone(),
                    package: dep.package_name.clone(),
                    target_kind: target_kind.to_string(),
                    target_value,
                    target_id,
                    version,
                });
            }
        }

        Ok(Self {
            version: 1,
            manifest,
            manifest_digest,
            packages,
            dependencies,
        })
    }

    fn parse(source: &str, path: &Path) -> Result<Self> {
        let mut lockfile = Self {
            version: 0,
            manifest: String::new(),
            manifest_digest: String::new(),
            packages: Vec::new(),
            dependencies: Vec::new(),
        };
        let mut section = Section::Root;

        for line in logical_lines(source).map_err(|message| Error::LockfileParse {
            path: path.to_path_buf(),
            message,
        })? {
            if line.starts_with("[[") {
                section = match line.as_str() {
                    "[[package]]" => {
                        lockfile.packages.push(LockedPackage {
                            id: String::new(),
                            name: String::new(),
                            version: String::new(),
                            source_kind: String::new(),
                            source_value: None,
                            manifest: String::new(),
                            manifest_digest: String::new(),
                        });
                        Section::Package(lockfile.packages.len() - 1)
                    }
                    "[[dependency]]" => {
                        lockfile.dependencies.push(LockedDependency {
                            from: String::new(),
                            kind: String::new(),
                            name: String::new(),
                            package: String::new(),
                            target_kind: String::new(),
                            target_value: None,
                            target_id: None,
                            version: None,
                        });
                        Section::Dependency(lockfile.dependencies.len() - 1)
                    }
                    _ => {
                        return Err(Error::LockfileParse {
                            path: path.to_path_buf(),
                            message: format!("unsupported array table `{line}`"),
                        });
                    }
                };
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

    pub fn validate(&self, path: &Path) -> Result<()> {
        if self.version != 1 {
            return Err(Error::LockfileValidation {
                path: path.to_path_buf(),
                message: format!("unsupported lockfile version `{}`", self.version),
            });
        }

        validate_non_empty(path, "manifest", &self.manifest)?;
        validate_digest(path, "manifest-digest", &self.manifest_digest)?;

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
            match dependency.target_kind.as_str() {
                "local" => {
                    let Some(target_id) = &dependency.target_id else {
                        return Err(Error::LockfileValidation {
                            path: path.to_path_buf(),
                            message: "[[dependency]] with target `local` requires `target-id`"
                                .to_string(),
                        });
                    };
                    if !package_ids.contains(target_id.as_str()) {
                        return Err(Error::LockfileValidation {
                            path: path.to_path_buf(),
                            message: format!(
                                "[[dependency]] references unknown local target id `{target_id}`"
                            ),
                        });
                    }
                }
                "path" => {
                    if dependency.target_value.is_none() {
                        return Err(Error::LockfileValidation {
                            path: path.to_path_buf(),
                            message: "[[dependency]] with target `path` requires `target-value`"
                                .to_string(),
                        });
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("# This file is generated by kraft.\n");
        out.push_str("version = ");
        out.push_str(&self.version.to_string());
        out.push('\n');
        push_string_line(&mut out, "manifest", &self.manifest);
        push_string_line(&mut out, "manifest-digest", &self.manifest_digest);

        for package in &self.packages {
            out.push('\n');
            out.push_str("[[package]]\n");
            push_string_line(&mut out, "id", &package.id);
            push_string_line(&mut out, "name", &package.name);
            push_string_line(&mut out, "version", &package.version);
            push_string_line(&mut out, "source", &package.source_kind);
            if let Some(value) = &package.source_value {
                push_string_line(&mut out, "source-value", value);
            }
            push_string_line(&mut out, "manifest", &package.manifest);
            push_string_line(&mut out, "manifest-digest", &package.manifest_digest);
        }

        for dependency in &self.dependencies {
            out.push('\n');
            out.push_str("[[dependency]]\n");
            push_string_line(&mut out, "from", &dependency.from);
            push_string_line(&mut out, "kind", &dependency.kind);
            push_string_line(&mut out, "name", &dependency.name);
            push_string_line(&mut out, "package", &dependency.package);
            push_string_line(&mut out, "target", &dependency.target_kind);
            if let Some(value) = &dependency.target_value {
                push_string_line(&mut out, "target-value", value);
            }
            if let Some(target_id) = &dependency.target_id {
                push_string_line(&mut out, "target-id", target_id);
            }
            if let Some(version) = &dependency.version {
                push_string_line(&mut out, "version", version);
            }
        }

        out
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
                _ => return Err(format!("unsupported [[package]] key `{key}`")),
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
                "target-value" => dependency.target_value = Some(parse_string(raw_value)?),
                "target-id" => dependency.target_id = Some(parse_string(raw_value)?),
                "version" => dependency.version = Some(parse_string(raw_value)?),
                _ => return Err(format!("unsupported [[dependency]] key `{key}`")),
            }
        }
    }

    Ok(())
}

fn dependency_kind(kind: DependencyKind) -> &'static str {
    match kind {
        DependencyKind::Normal => "normal",
        DependencyKind::Dev => "dev",
        DependencyKind::Build => "build",
    }
}

fn package_lock_id(id: &PackageId) -> String {
    match &id.source {
        SourceId::Root => format!("{} {} root", id.name, id.version),
        SourceId::WorkspaceMember { path } => {
            format!("{} {} workspace-member:{path}", id.name, id.version)
        }
        SourceId::PathDependency { path } => {
            format!("{} {} path:{path}", id.name, id.version)
        }
        SourceId::Registry { name: Some(name) } => {
            format!("{} {} registry:{name}", id.name, id.version)
        }
        SourceId::Registry { name: None } => format!("{} {} registry", id.name, id.version),
    }
}

fn source_kind(source: &SourceId) -> &'static str {
    match source {
        SourceId::Root => "root",
        SourceId::WorkspaceMember { .. } => "workspace-member",
        SourceId::PathDependency { .. } => "path",
        SourceId::Registry { .. } => "registry",
    }
}

fn source_value(source: &SourceId) -> Option<String> {
    match source {
        SourceId::Root => None,
        SourceId::WorkspaceMember { path } => Some(path.clone()),
        SourceId::PathDependency { path } => Some(path.clone()),
        SourceId::Registry { name } => name.clone(),
    }
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

fn digest_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).map_err(|err| Error::from_io(path, err))?;
    Ok(format!("fnv1a64:{:016x}", fnv1a64(&bytes)))
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

fn validate_non_empty(path: &Path, field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!("{field} must not be empty"),
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
        "root" | "workspace-member" | "path" | "registry" => Ok(()),
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
        "local" | "path" | "registry" => Ok(()),
        _ => Err(Error::LockfileValidation {
            path: path.to_path_buf(),
            message: format!("{field} has unsupported dependency target `{value}`"),
        }),
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    let mut hash = OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

fn push_string_line(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push_str(" = \"");
    out.push_str(&escape_string(value));
    out.push_str("\"\n");
}

fn escape_string(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{LockStatus, LockWriteResult, Lockfile, lock_status, sync_lockfile};
    use crate::graph::build_graph;
    use crate::manifest::Manifest;
    use crate::workspace::load_members;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn renders_workspace_lockfile_from_package_graph() {
        let root = temp_dir("kraft-lockfile");
        let app_dir = root.join("app");
        let util_dir = root.join("util");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&util_dir).unwrap();

        fs::write(
            root.join("Kraft.toml"),
            r#"
[workspace]
members = ["app", "util"]

[workspace.dependencies]
shared = "2"
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Kraft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[dependencies]
util = { path = "../util" }
shared = { workspace = true, features = ["simd"] }
"#,
        )
        .unwrap();
        fs::write(
            util_dir.join("Kraft.toml"),
            r#"
[package]
name = "util"
version = "0.1.0"
kern = "0.7"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let graph = build_graph(&manifest_path, &root_manifest, &members).unwrap();
        let lockfile = Lockfile::from_graph(&manifest_path, &graph).unwrap();
        let rendered = lockfile.render();

        assert!(rendered.contains("version = 1"));
        assert!(rendered.contains("[[package]]"));
        assert!(rendered.contains("id = \"app 0.1.0 workspace-member:app\""));
        assert!(rendered.contains("target-id = \"util 0.1.0 workspace-member:util\""));
        assert!(rendered.contains("name = \"shared\""));
        assert!(rendered.contains("target = \"registry\""));
        assert!(rendered.contains("version = \"2\""));
        assert!(rendered.contains("manifest-digest = \"fnv1a64:"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn loads_rendered_lockfile_roundtrip() {
        let root = temp_dir("kraft-lockfile-load");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Kraft.toml"),
            r#"
[workspace]
members = ["app"]

[workspace.dependencies]
shared = "2"
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Kraft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[dependencies]
shared = { workspace = true }
"#,
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let graph = build_graph(&manifest_path, &root_manifest, &members).unwrap();
        let expected = Lockfile::from_graph(&manifest_path, &graph).unwrap();
        let (lock_path, _) = sync_lockfile(&manifest_path, &graph).unwrap();
        let loaded = Lockfile::load(&lock_path).unwrap();

        assert_eq!(loaded, expected);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn writes_lockfile_into_workspace_root() {
        let root = temp_dir("kraft-lockfile-write");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Kraft.toml"),
            r#"
[workspace]
members = ["app"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Kraft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let graph = build_graph(&manifest_path, &root_manifest, &members).unwrap();
        let (lock_path, _) = sync_lockfile(&manifest_path, &graph).unwrap();
        let contents = fs::read_to_string(&lock_path).unwrap();

        assert_eq!(lock_path, root.join("Kraft.lock"));
        assert!(contents.contains("manifest = \"Kraft.toml\""));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sync_lockfile_reports_created_updated_and_unchanged() {
        let root = temp_dir("kraft-lockfile-sync");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Kraft.toml"),
            r#"
[workspace]
members = ["app"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Kraft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let graph = build_graph(&manifest_path, &root_manifest, &members).unwrap();

        let (_, created) = sync_lockfile(&manifest_path, &graph).unwrap();
        assert_eq!(created, LockWriteResult::Created);

        let (_, unchanged) = sync_lockfile(&manifest_path, &graph).unwrap();
        assert_eq!(unchanged, LockWriteResult::Unchanged);

        fs::write(
            app_dir.join("Kraft.toml"),
            r#"
[package]
name = "app"
version = "0.2.0"
kern = "0.7"
"#,
        )
        .unwrap();

        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let graph = build_graph(&manifest_path, &root_manifest, &members).unwrap();
        let (_, updated) = sync_lockfile(&manifest_path, &graph).unwrap();
        assert_eq!(updated, LockWriteResult::Updated);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reports_current_and_stale_lockfile_status() {
        let root = temp_dir("kraft-lockfile-status");
        let app_dir = root.join("app");
        fs::create_dir_all(&app_dir).unwrap();

        fs::write(
            root.join("Kraft.toml"),
            r#"
[workspace]
members = ["app"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Kraft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"
"#,
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let graph = build_graph(&manifest_path, &root_manifest, &members).unwrap();

        assert_eq!(
            lock_status(&manifest_path, &graph).unwrap(),
            LockStatus::Missing
        );

        let _ = sync_lockfile(&manifest_path, &graph).unwrap();
        assert_eq!(
            lock_status(&manifest_path, &graph).unwrap(),
            LockStatus::Current
        );

        fs::write(
            app_dir.join("Kraft.toml"),
            r#"
[package]
name = "app"
version = "0.2.0"
kern = "0.7"
"#,
        )
        .unwrap();

        let root_manifest = Manifest::load(&manifest_path).unwrap();
        let members = load_members(&manifest_path, &root_manifest).unwrap();
        let graph = build_graph(&manifest_path, &root_manifest, &members).unwrap();
        assert_eq!(
            lock_status(&manifest_path, &graph).unwrap(),
            LockStatus::Stale
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_invalid_dependency_target_reference() {
        let root = temp_dir("kraft-lockfile-invalid");
        let lock_path = root.join("Kraft.lock");
        fs::write(
            &lock_path,
            r#"
version = 1
manifest = "Kraft.toml"
manifest-digest = "fnv1a64:1234567890abcdef"

[[package]]
id = "app 0.1.0 workspace-member:app"
name = "app"
version = "0.1.0"
source = "workspace-member"
source-value = "app"
manifest = "app/Kraft.toml"
manifest-digest = "fnv1a64:1234567890abcdef"

[[dependency]]
from = "app 0.1.0 workspace-member:app"
kind = "normal"
name = "util"
package = "util"
target = "local"
target-id = "missing 0.1.0 workspace-member:missing"
version = "0.1.0"
"#,
        )
        .unwrap();

        let err = Lockfile::load(&lock_path).unwrap_err();
        assert!(err.to_string().contains("unknown local target id"));

        let _ = fs::remove_dir_all(root);
    }
}
