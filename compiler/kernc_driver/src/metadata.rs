use crate::doc::{
    KernDoc, KernDocEntry, KernDocSection, KernDocSectionKind, KmetaDocItem,
    collect_kmeta_doc_items, render_kmeta_docs_toml,
};
use kernc_sema::SemaContext;
use kernc_sema::def::{Def, DefId, ModuleDef};
use std::collections::BTreeMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const KMETA_MANIFEST_FILE: &str = "Kmeta.toml";
pub const KMETA_DOCS_FILE: &str = "Kmeta.docs.toml";
const KMETA_FORMAT_VERSION: u32 = 2;
const KMETA_KIND_SOURCE_SNAPSHOT: &str = "source_snapshot";
const KMETA_SOURCE_ROOT: &str = "src";
const KMETA_OUTPUT_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(100);

struct MetadataOutputLock {
    path: PathBuf,
}

#[derive(Clone, Copy, Debug)]
struct MetadataLockOwner {
    pid: u32,
    #[cfg(unix)]
    start_ticks: Option<u64>,
}

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
    let _lock = MetadataOutputLock::acquire(output_root)?;
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

    copy_source_snapshot_tree(&root_path, root_dir, output_root)?;

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

pub fn load_docs(metadata_root: &Path) -> Result<Option<Vec<KmetaDocItem>>, String> {
    let docs_path = metadata_root.join(KMETA_DOCS_FILE);
    if !docs_path.is_file() {
        return Ok(None);
    }

    let contents = fs::read_to_string(&docs_path)
        .map_err(|err| format!("failed to read `{}`: {err}", docs_path.display()))?;
    parse_docs(&contents).map(Some)
}

fn module_def<'a>(ctx: &'a SemaContext<'_>, id: DefId) -> Option<&'a ModuleDef> {
    match ctx.defs.get(id.0 as usize) {
        Some(Def::Module(module)) => Some(module),
        _ => None,
    }
}

impl MetadataOutputLock {
    fn acquire(output_root: &Path) -> Result<Self, String> {
        let path = metadata_output_lock_path(output_root);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create `{}`: {err}", parent.display()))?;
        }

        loop {
            match try_acquire_metadata_output_lock(&path, output_root) {
                Ok(lock) => return Ok(lock),
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                    if reclaim_stale_metadata_output_lock(&path)? {
                        continue;
                    }
                    thread::sleep(KMETA_OUTPUT_LOCK_POLL_INTERVAL);
                }
                Err(err) => {
                    return Err(format!(
                        "failed to lock kmeta output `{}`: {err}",
                        output_root.display()
                    ));
                }
            }
        }
    }
}

impl Drop for MetadataOutputLock {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_file(&self.path)
            && err.kind() != ErrorKind::NotFound
        {
            let _ = err;
        }
    }
}

fn metadata_output_lock_path(output_root: &Path) -> PathBuf {
    let lock_name = output_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!(".{name}.kmeta.lock"))
        .unwrap_or_else(|| ".kmeta.lock".to_string());
    output_root
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(lock_name)
}

fn try_acquire_metadata_output_lock(
    path: &Path,
    output_root: &Path,
) -> std::io::Result<MetadataOutputLock> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(metadata_lock_contents(output_root).as_bytes())?;
    file.sync_all()?;
    Ok(MetadataOutputLock {
        path: path.to_path_buf(),
    })
}

fn metadata_lock_contents(output_root: &Path) -> String {
    let created_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let pid = std::process::id();
    let mut contents = format!(
        "pid={}\noutput={}\ncreated_unix_ms={}\n",
        pid,
        output_root.display(),
        created_ms
    );
    #[cfg(unix)]
    if let Some(start_ticks) = read_process_start_ticks(pid) {
        contents.push_str(&format!("start_ticks={start_ticks}\n"));
    }
    contents
}

fn reclaim_stale_metadata_output_lock(path: &Path) -> Result<bool, String> {
    let Some(owner) = read_metadata_lock_owner(path)? else {
        return Ok(false);
    };

    if metadata_lock_owner_is_alive(owner) {
        return Ok(false);
    }

    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(true),
        Err(err) => Err(format!("failed to clear stale `{}`: {err}", path.display())),
    }
}

fn read_metadata_lock_owner(path: &Path) -> Result<Option<MetadataLockOwner>, String> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("failed to read `{}`: {err}", path.display())),
    };

    let mut pid = None;
    #[cfg(unix)]
    let mut start_ticks = None;
    for line in contents.lines() {
        if let Some(raw_pid) = line.strip_prefix("pid=") {
            pid = raw_pid.parse::<u32>().ok();
            continue;
        }
        #[cfg(unix)]
        if let Some(raw_start_ticks) = line.strip_prefix("start_ticks=") {
            start_ticks = raw_start_ticks.parse::<u64>().ok();
        }
    }

    Ok(pid.map(|pid| MetadataLockOwner {
        pid,
        #[cfg(unix)]
        start_ticks,
    }))
}

#[cfg(unix)]
fn metadata_lock_owner_is_alive(owner: MetadataLockOwner) -> bool {
    let Some(current_start_ticks) = read_process_start_ticks(owner.pid) else {
        return false;
    };

    match owner.start_ticks {
        Some(lock_start_ticks) => current_start_ticks == lock_start_ticks,
        None => owner.pid != std::process::id(),
    }
}

#[cfg(not(unix))]
fn metadata_lock_owner_is_alive(_owner: MetadataLockOwner) -> bool {
    true
}

#[cfg(unix)]
fn read_process_start_ticks(pid: u32) -> Option<u64> {
    let path = Path::new("/proc").join(pid.to_string()).join("stat");
    let contents = fs::read_to_string(path).ok()?;
    let end = contents.rfind(") ")?;
    let fields = contents[end + 2..].split_whitespace().collect::<Vec<_>>();
    fields.get(19)?.parse::<u64>().ok()
}

fn snapshot_path_for_source(
    source_path: &Path,
    root_path: &Path,
    root_dir: &Path,
    output_root: &Path,
) -> PathBuf {
    if source_path == root_path {
        return output_root.join(KMETA_SOURCE_ROOT).join("init.rn");
    }

    let relative = source_path.strip_prefix(root_dir).unwrap_or(source_path);
    output_root.join(KMETA_SOURCE_ROOT).join(relative)
}

fn copy_source_snapshot_tree(
    root_path: &Path,
    root_dir: &Path,
    output_root: &Path,
) -> Result<(), String> {
    let mut pending = vec![root_dir.to_path_buf()];

    while let Some(dir) = pending.pop() {
        let entries = fs::read_dir(&dir)
            .map_err(|err| format!("failed to read `{}`: {err}", dir.display()))?;
        let mut paths = entries
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| format!("failed to enumerate `{}`: {err}", dir.display()))?;
        paths.sort();

        for path in paths {
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rn") {
                continue;
            }

            let dest_path = snapshot_path_for_source(&path, root_path, root_dir, output_root);
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|err| format!("failed to create `{}`: {err}", parent.display()))?;
            }
            fs::copy(&path, &dest_path).map_err(|err| {
                format!(
                    "failed to copy `{}` to `{}`: {err}",
                    path.display(),
                    dest_path.display()
                )
            })?;
        }
    }

    Ok(())
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

fn parse_docs(contents: &str) -> Result<Vec<KmetaDocItem>, String> {
    enum SectionKind {
        Item,
        ItemSection,
        ItemSectionEntry,
    }

    let mut items = Vec::new();
    let mut current_item: Option<KmetaDocItem> = None;
    let mut current_section: Option<KernDocSection> = None;
    let mut active = None;
    let mut seen_format_version = false;

    for (index, raw_line) in contents.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        match line {
            "[[item]]" => {
                flush_section(&mut current_item, &mut current_section)?;
                flush_item(&mut items, &mut current_item);
                current_item = Some(KmetaDocItem {
                    path: String::new(),
                    kind: String::new(),
                    signature: None,
                    docs: KernDoc {
                        summary: String::new(),
                        details: String::new(),
                        sections: Vec::new(),
                        raw_text: String::new(),
                    },
                });
                active = Some(SectionKind::Item);
                continue;
            }
            "[[item.section]]" => {
                flush_section(&mut current_item, &mut current_section)?;
                ensure_item(index + 1, &current_item)?;
                current_section = Some(KernDocSection {
                    kind: KernDocSectionKind::Custom,
                    title: String::new(),
                    body: String::new(),
                    entries: Vec::new(),
                });
                active = Some(SectionKind::ItemSection);
                continue;
            }
            "[[item.section.entry]]" => {
                ensure_section(index + 1, &current_section)?;
                if let Some(section) = current_section.as_mut() {
                    section.entries.push(KernDocEntry {
                        name: None,
                        body: String::new(),
                    });
                }
                active = Some(SectionKind::ItemSectionEntry);
                continue;
            }
            _ => {}
        }

        let Some((key, value)) = line.split_once('=') else {
            return Err(format!(
                "invalid docs line {}: expected `key = value` or a table header",
                index + 1
            ));
        };
        let key = key.trim();

        if key == "format_version" {
            let version = value.trim().parse::<u32>().map_err(|_| {
                format!(
                    "invalid docs line {}: expected an integer `format_version`",
                    index + 1
                )
            })?;
            if version != 1 {
                return Err(format!("unsupported docs format version `{version}`"));
            }
            seen_format_version = true;
            continue;
        }

        let value = parse_quoted(value.trim()).ok_or_else(|| {
            format!(
                "invalid docs line {}: expected a quoted string value for `{}`",
                index + 1,
                key
            )
        })?;

        match active {
            Some(SectionKind::Item) => {
                let item = current_item
                    .as_mut()
                    .ok_or_else(|| format!("docs line {} appears outside `[[item]]`", index + 1))?;
                match key {
                    "path" => item.path = value,
                    "kind" => item.kind = value,
                    "signature" => item.signature = Some(value),
                    "summary" => item.docs.summary = value,
                    "details" => item.docs.details = value,
                    "raw" => item.docs.raw_text = value,
                    _ => {
                        return Err(format!(
                            "unknown docs item field `{}` on line {}",
                            key,
                            index + 1
                        ));
                    }
                }
            }
            Some(SectionKind::ItemSection) => {
                let section = current_section.as_mut().ok_or_else(|| {
                    format!("docs line {} appears outside `[[item.section]]`", index + 1)
                })?;
                match key {
                    "kind" => section.kind = parse_section_kind(&value),
                    "title" => section.title = value,
                    "body" => section.body = value,
                    _ => {
                        return Err(format!(
                            "unknown docs section field `{}` on line {}",
                            key,
                            index + 1
                        ));
                    }
                }
            }
            Some(SectionKind::ItemSectionEntry) => {
                let section = current_section.as_mut().ok_or_else(|| {
                    format!(
                        "docs line {} appears outside `[[item.section.entry]]`",
                        index + 1
                    )
                })?;
                let entry = section.entries.last_mut().ok_or_else(|| {
                    format!(
                        "docs line {} appears before an active `[[item.section.entry]]`",
                        index + 1
                    )
                })?;
                match key {
                    "name" => entry.name = Some(value),
                    "body" => entry.body = value,
                    _ => {
                        return Err(format!(
                            "unknown docs section entry field `{}` on line {}",
                            key,
                            index + 1
                        ));
                    }
                }
            }
            None => {
                return Err(format!(
                    "docs line {} appears before any docs table header",
                    index + 1
                ));
            }
        }
    }

    flush_section(&mut current_item, &mut current_section)?;
    flush_item(&mut items, &mut current_item);
    if !seen_format_version {
        return Err("docs metadata is missing `format_version`".to_string());
    }
    Ok(items)
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

fn flush_section(
    current_item: &mut Option<KmetaDocItem>,
    current_section: &mut Option<KernDocSection>,
) -> Result<(), String> {
    let Some(section) = current_section.take() else {
        return Ok(());
    };
    let item = current_item
        .as_mut()
        .ok_or_else(|| "doc section appeared without an owning item".to_string())?;
    item.docs.sections.push(section);
    Ok(())
}

fn flush_item(items: &mut Vec<KmetaDocItem>, current_item: &mut Option<KmetaDocItem>) {
    if let Some(item) = current_item.take() {
        items.push(item);
    }
}

fn ensure_item(line: usize, current_item: &Option<KmetaDocItem>) -> Result<(), String> {
    if current_item.is_none() {
        return Err(format!(
            "docs line {} starts a section before any item",
            line
        ));
    }
    Ok(())
}

fn ensure_section(line: usize, current_section: &Option<KernDocSection>) -> Result<(), String> {
    if current_section.is_none() {
        return Err(format!(
            "docs line {} starts a section entry before any section",
            line
        ));
    }
    Ok(())
}

fn parse_section_kind(raw: &str) -> KernDocSectionKind {
    match raw {
        "args" => KernDocSectionKind::Args,
        "returns" => KernDocSectionKind::Returns,
        "errors" => KernDocSectionKind::Errors,
        "safety" => KernDocSectionKind::Safety,
        "effects" => KernDocSectionKind::Effects,
        "requires" => KernDocSectionKind::Requires,
        "ensures" => KernDocSectionKind::Ensures,
        "state" => KernDocSectionKind::State,
        "boundary" => KernDocSectionKind::Boundary,
        "design" => KernDocSectionKind::Design,
        "rationale" => KernDocSectionKind::Rationale,
        "example" => KernDocSectionKind::Example,
        "see" => KernDocSectionKind::See,
        "note" => KernDocSectionKind::Note,
        "warning" => KernDocSectionKind::Warning,
        _ => KernDocSectionKind::Custom,
    }
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

#[cfg(test)]
mod tests {
    use super::{MetadataOutputLock, metadata_output_lock_path, parse_docs};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_native_docs_toml_with_nested_sections() {
        let contents = r#"
format_version = 1

[[item]]
path = "root::uart::Uart::read"
kind = "function"
signature = "fn read: fn(u16) u8"
summary = "Read one byte."
details = ""
raw = "Read one byte."

[[item.section]]
kind = "safety"
title = "Safety"
body = ""

[[item.section.entry]]
name = "self"
body = "must point to a mapped UART object."
"#;

        let docs = parse_docs(contents).expect("expected docs to parse");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].path, "root::uart::Uart::read");
        assert_eq!(docs[0].docs.summary, "Read one byte.");
        assert_eq!(docs[0].docs.sections.len(), 1);
        assert_eq!(docs[0].docs.sections[0].title, "Safety");
        assert_eq!(docs[0].docs.sections[0].entries.len(), 1);
        assert_eq!(
            docs[0].docs.sections[0].entries[0].name.as_deref(),
            Some("self")
        );
    }

    #[test]
    fn metadata_output_lock_waits_for_release() {
        let root = temp_dir("kernc-kmeta-output-lock");
        let output_root = root.join("meta").join("std");
        let lock_path = metadata_output_lock_path(&output_root);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let output_root_for_holder = output_root.clone();

        let holder = thread::spawn(move || {
            let _lock = MetadataOutputLock::acquire(&output_root_for_holder).unwrap();
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });

        ready_rx.recv().unwrap();
        assert!(lock_path.is_file(), "expected metadata lock file to exist");

        let output_root_for_waiter = output_root.clone();
        let start = Instant::now();
        let waiter = thread::spawn(move || {
            let _lock = MetadataOutputLock::acquire(&output_root_for_waiter).unwrap();
            start.elapsed()
        });

        thread::sleep(Duration::from_millis(200));
        release_tx.send(()).unwrap();

        holder.join().unwrap();
        let waited = waiter.join().unwrap();
        assert!(waited >= Duration::from_millis(150));
        assert!(
            !lock_path.exists(),
            "expected metadata lock file to be removed after release"
        );

        let _ = fs::remove_dir_all(root);
    }
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
