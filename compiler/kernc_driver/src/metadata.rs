use crate::doc::{collect_kmeta_doc_items, render_kmeta_docs_toml};
use kernc_sema::SemaContext;
use kernc_sema::def::{Def, DefId, ModuleDef};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const KMETA_MANIFEST_FILE: &str = "Kmeta.toml";
pub const KMETA_DOCS_FILE: &str = "Kmeta.docs.toml";
const KMETA_FORMAT_VERSION: u32 = 2;
const KMETA_KIND_SOURCE_SNAPSHOT: &str = "source_snapshot";
const KMETA_SOURCE_ROOT: &str = "src";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KmetaManifest {
    pub format_version: u32,
    pub kind: String,
    pub package_name: String,
    pub package_version: Option<String>,
    pub root_module_name: String,
    pub entry_module_path: String,
}

impl KmetaManifest {
    pub fn source_snapshot(
        package_name: String,
        package_version: Option<String>,
        root_module_name: String,
        entry_module_path: String,
    ) -> Self {
        Self {
            format_version: KMETA_FORMAT_VERSION,
            kind: KMETA_KIND_SOURCE_SNAPSHOT.to_string(),
            package_name,
            package_version,
            root_module_name,
            entry_module_path,
        }
    }
}

pub fn emit_package_metadata(
    ctx: &SemaContext<'_>,
    output_root: &Path,
    package_name: &str,
    package_version: Option<&str>,
) -> Result<(), String> {
    let Some(root_id) = ctx.root_module else {
        return Err("missing root module while emitting kmeta metadata".to_string());
    };
    let Some(root_module) = module_def(ctx, root_id) else {
        return Err("root module def is missing while emitting kmeta metadata".to_string());
    };
    let Some(root_path) = ctx
        .sess
        .source_manager
        .get_file_path(root_module.file_id)
        .cloned()
    else {
        return Err("root module source file is missing while emitting kmeta metadata".to_string());
    };
    let Some(root_dir) = root_path.parent() else {
        return Err(format!(
            "root module `{}` has no parent directory",
            root_path.display()
        ));
    };

    if output_root.exists() {
        fs::remove_dir_all(output_root)
            .map_err(|err| format!("failed to clear `{}`: {err}", output_root.display()))?;
    }
    fs::create_dir_all(output_root)
        .map_err(|err| format!("failed to create `{}`: {err}", output_root.display()))?;

    let manifest = KmetaManifest::source_snapshot(
        package_name.to_string(),
        package_version.map(ToOwned::to_owned),
        ctx.resolve(root_module.name).to_string(),
        format!("{}/init.rn", KMETA_SOURCE_ROOT),
    );
    write_manifest(&output_root.join(KMETA_MANIFEST_FILE), &manifest)?;
    write_docs(
        &output_root.join(KMETA_DOCS_FILE),
        &render_kmeta_docs_toml(&collect_kmeta_doc_items(ctx)),
    )?;

    let mut stack = vec![root_id];
    while let Some(module_id) = stack.pop() {
        let Some(module) = module_def(ctx, module_id) else {
            continue;
        };
        if module.is_imported {
            continue;
        }

        let Some(source_path) = ctx
            .sess
            .source_manager
            .get_file_path(module.file_id)
            .cloned()
        else {
            return Err(format!(
                "module `{}` is missing a source file",
                ctx.resolve(module.name)
            ));
        };
        let dest_path =
            snapshot_path_for_module(module_id, &source_path, root_id, root_dir, output_root);
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create `{}`: {err}", parent.display()))?;
        }
        fs::copy(&source_path, &dest_path).map_err(|err| {
            format!(
                "failed to copy `{}` to `{}`: {err}",
                source_path.display(),
                dest_path.display()
            )
        })?;

        let mut children = module.submodules.values().copied().collect::<Vec<_>>();
        children.sort_by_key(|id| id.0);
        stack.extend(children.into_iter().rev());
    }

    Ok(())
}

pub fn load_manifest(metadata_root: &Path) -> Result<Option<KmetaManifest>, String> {
    let manifest_path = metadata_root.join(KMETA_MANIFEST_FILE);
    if !manifest_path.is_file() {
        return Ok(None);
    }

    let contents = fs::read_to_string(&manifest_path)
        .map_err(|err| format!("failed to read `{}`: {err}", manifest_path.display()))?;
    parse_manifest(&contents).map(Some)
}

fn module_def<'a>(ctx: &'a SemaContext<'_>, id: DefId) -> Option<&'a ModuleDef> {
    match ctx.defs.get(id.0 as usize) {
        Some(Def::Module(module)) => Some(module),
        _ => None,
    }
}

fn snapshot_path_for_module(
    module_id: DefId,
    source_path: &Path,
    root_id: DefId,
    root_dir: &Path,
    output_root: &Path,
) -> PathBuf {
    if module_id == root_id {
        return output_root.join(KMETA_SOURCE_ROOT).join("init.rn");
    }

    let relative = source_path.strip_prefix(root_dir).unwrap_or(source_path);
    output_root.join(KMETA_SOURCE_ROOT).join(relative)
}

fn write_manifest(path: &Path, manifest: &KmetaManifest) -> Result<(), String> {
    let mut lines = Vec::new();
    lines.push(format!("format_version = {}", manifest.format_version));
    lines.push(format!("kind = {}", quote(&manifest.kind)));
    lines.push(format!("package_name = {}", quote(&manifest.package_name)));
    if let Some(package_version) = &manifest.package_version {
        lines.push(format!("package_version = {}", quote(package_version)));
    }
    lines.push(format!(
        "root_module_name = {}",
        quote(&manifest.root_module_name)
    ));
    lines.push(format!(
        "entry_module_path = {}",
        quote(&manifest.entry_module_path)
    ));
    lines.push(String::new());

    fs::write(path, lines.join("\n"))
        .map_err(|err| format!("failed to write `{}`: {err}", path.display()))
}

fn write_docs(path: &Path, contents: &str) -> Result<(), String> {
    fs::write(path, contents).map_err(|err| format!("failed to write `{}`: {err}", path.display()))
}

fn parse_manifest(contents: &str) -> Result<KmetaManifest, String> {
    let mut fields = BTreeMap::new();
    for (index, raw_line) in contents.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            return Err(format!(
                "invalid kmeta manifest line {}: expected `key = value`",
                index + 1
            ));
        };
        fields.insert(key.trim().to_string(), value.trim().to_string());
    }

    let format_version = fields
        .remove("format_version")
        .ok_or_else(|| "kmeta manifest is missing `format_version`".to_string())?
        .parse::<u32>()
        .map_err(|_| "kmeta manifest has invalid `format_version`".to_string())?;
    if format_version != KMETA_FORMAT_VERSION {
        return Err(format!(
            "unsupported kmeta format version `{}`",
            format_version
        ));
    }

    let kind = parse_quoted_field(&mut fields, "kind")?;
    if kind != KMETA_KIND_SOURCE_SNAPSHOT {
        return Err(format!("unsupported kmeta kind `{}`", kind));
    }

    let package_name = parse_quoted_field(&mut fields, "package_name")?;
    let package_version = parse_optional_quoted_field(&mut fields, "package_version")?;
    let root_module_name = parse_quoted_field(&mut fields, "root_module_name")?;
    let entry_module_path = parse_quoted_field(&mut fields, "entry_module_path")?;

    if !fields.is_empty() {
        let unknown = fields.keys().cloned().collect::<Vec<_>>().join(", ");
        return Err(format!("unknown kmeta manifest fields: {}", unknown));
    }

    Ok(KmetaManifest {
        format_version,
        kind,
        package_name,
        package_version,
        root_module_name,
        entry_module_path,
    })
}

fn parse_quoted_field(fields: &mut BTreeMap<String, String>, key: &str) -> Result<String, String> {
    let raw = fields
        .remove(key)
        .ok_or_else(|| format!("kmeta manifest is missing `{}`", key))?;
    parse_quoted(&raw).ok_or_else(|| format!("kmeta manifest has invalid `{}`", key))
}

fn parse_optional_quoted_field(
    fields: &mut BTreeMap<String, String>,
    key: &str,
) -> Result<Option<String>, String> {
    let Some(raw) = fields.remove(key) else {
        return Ok(None);
    };
    parse_quoted(&raw)
        .map(Some)
        .ok_or_else(|| format!("kmeta manifest has invalid `{}`", key))
}

fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn parse_quoted(raw: &str) -> Option<String> {
    if !(raw.starts_with('"') && raw.ends_with('"')) {
        return None;
    }

    let mut out = String::new();
    let mut chars = raw[1..raw.len() - 1].chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next()? {
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                _ => return None,
            }
        } else {
            out.push(ch);
        }
    }
    Some(out)
}
