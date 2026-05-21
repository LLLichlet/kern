//! Incremental action-state tracking for Craft build outputs.
//!
//! Fingerprint files record input/output metadata beside generated artifacts so
//! later builds can skip unchanged compile, link, and staging actions while
//! still detecting missing or externally modified outputs.

use crate::error::{Error, Result};
use crate::local_state;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

static CURRENT_PROCESS_DIGEST: OnceLock<String> = OnceLock::new();
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActionState {
    fingerprint: String,
    inputs: Vec<ActionStatePath>,
    outputs: Vec<ActionStatePath>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActionStatePath {
    path: PathBuf,
    digest: String,
    len: Option<u64>,
    modified_nanos: Option<u128>,
    changed_nanos: Option<u128>,
}

impl ActionStatePath {
    fn has_required_fields(&self) -> bool {
        !self.path.as_os_str().is_empty() && !self.digest.is_empty()
    }

    fn supports_metadata_fast_path(&self) -> bool {
        self.len.is_some() && self.modified_nanos.is_some() && self.changed_nanos.is_some()
    }
}

#[derive(Clone, Copy, Debug)]
enum Section {
    Root,
    Input(usize),
    Output(usize),
}

type DigestedPath = (String, Option<u64>, Option<u128>, Option<u128>);

pub(crate) fn action_state_path(primary_output: &Path) -> PathBuf {
    let file_name = primary_output
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    primary_output
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".{file_name}.craft-state"))
}

pub(crate) fn current_process_digest() -> Result<String> {
    if let Some(digest) = CURRENT_PROCESS_DIGEST.get() {
        return Ok(digest.clone());
    }

    let exe = std::env::current_exe().map_err(Error::from_io_plain)?;
    let metadata = fs::metadata(&exe).map_err(|err| Error::from_io(&exe, err))?;
    let modified_nanos =
        metadata_modified_nanos(&metadata.modified().map_err(Error::from_io_plain)?)?;
    let digest = hash_string(&format!(
        "exe={};len={};modified={modified_nanos}",
        exe.display(),
        metadata.len()
    ));
    let _ = CURRENT_PROCESS_DIGEST.set(digest.clone());
    Ok(digest)
}

pub(crate) fn hash_string(value: &str) -> String {
    format!("fnv1a64:{:016x}", fnv1a64(value.as_bytes()))
}

pub(crate) fn record_action_state(
    primary_output: &Path,
    fingerprint: String,
    inputs: &[PathBuf],
    outputs: &[PathBuf],
) -> Result<()> {
    let state = ActionState {
        fingerprint,
        inputs: collect_state_paths(inputs)?,
        outputs: collect_state_paths(outputs)?,
    };
    let state_path = action_state_path(primary_output);
    local_state::write_file_atomic(&state_path, state.render())
}

pub(crate) fn action_state_is_current(primary_output: &Path, fingerprint: &str) -> Result<bool> {
    let Some(state) = load_action_state(&action_state_path(primary_output))? else {
        return Ok(false);
    };
    if state.fingerprint != fingerprint {
        return Ok(false);
    }

    for entry in state.inputs.iter().chain(&state.outputs) {
        if !path_matches_digest(&entry.path, entry)? {
            return Ok(false);
        }
    }

    Ok(true)
}

fn digest_file_contents(path: &Path) -> Result<String> {
    let bytes = fs::read(path).map_err(|err| Error::from_io(path, err))?;
    Ok(format!("fnv1a64:{:016x}", fnv1a64(&bytes)))
}

fn load_action_state(path: &Path) -> Result<Option<ActionState>> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(Error::from_io(path, err)),
    };

    match ActionState::parse(&source, path) {
        Ok(state) => Ok(Some(state)),
        Err(_) => Ok(None),
    }
}

fn collect_state_paths(paths: &[PathBuf]) -> Result<Vec<ActionStatePath>> {
    let mut entries = Vec::with_capacity(paths.len());
    for path in paths {
        let Some((digest, len, modified_nanos, changed_nanos)) = digest_path(path)? else {
            let paths = paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Error::Execution(format!(
                "cannot record build state for missing path `{}` among [{}]",
                path.display(),
                paths
            )));
        };
        entries.push(ActionStatePath {
            path: normalize_state_path(path),
            digest,
            len,
            modified_nanos,
            changed_nanos,
        });
    }
    Ok(entries)
}

fn digest_path(path: &Path) -> Result<Option<DigestedPath>> {
    if !path.exists() {
        return Ok(None);
    }
    if path.is_file() {
        let metadata = fs::metadata(path).map_err(|err| Error::from_io(path, err))?;
        let modified_nanos =
            metadata_modified_nanos(&metadata.modified().map_err(Error::from_io_plain)?)?;
        return Ok(Some((
            digest_file_contents(path)?,
            Some(metadata.len()),
            Some(modified_nanos),
            file_changed_nanos(path, &metadata),
        )));
    }
    if path.is_dir() {
        return Ok(Some((
            format!("fnv1a64:{:016x}", digest_tree(path)?),
            None,
            None,
            None,
        )));
    }
    Ok(None)
}

fn path_matches_digest(path: &Path, entry: &ActionStatePath) -> Result<bool> {
    if let Some(expected_len) = entry.len {
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(err) => return Err(Error::from_io(path, err)),
        };
        if !metadata.is_file() || metadata.len() != expected_len {
            return Ok(false);
        }
        if entry.supports_metadata_fast_path()
            && let (Some(expected_modified_nanos), Some(expected_changed_nanos)) =
                (entry.modified_nanos, entry.changed_nanos)
        {
            let modified_nanos =
                metadata_modified_nanos(&metadata.modified().map_err(Error::from_io_plain)?)?;
            let changed_nanos = file_changed_nanos(path, &metadata);
            if modified_nanos == expected_modified_nanos
                && changed_nanos == Some(expected_changed_nanos)
            {
                return Ok(true);
            }
        }
    }

    Ok(digest_path(path)?.map(|(digest, _, _, _)| digest) == Some(entry.digest.clone()))
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum TreeEntry {
    Dir(PathBuf),
    File(PathBuf),
}

fn digest_tree(root: &Path) -> Result<u64> {
    let mut entries = Vec::new();
    collect_tree_entries(root, root, &mut entries)?;
    entries.sort();

    let mut hash = 0xcbf29ce484222325_u64;
    for entry in entries {
        match entry {
            TreeEntry::Dir(path) => {
                let relative = path
                    .strip_prefix(root)
                    .unwrap_or(path.as_path())
                    .to_string_lossy();
                hash = fnv1a64_update(hash, b"dir:");
                hash = fnv1a64_update(hash, relative.as_bytes());
            }
            TreeEntry::File(path) => {
                let relative = path
                    .strip_prefix(root)
                    .unwrap_or(path.as_path())
                    .to_string_lossy();
                hash = fnv1a64_update(hash, b"file:");
                hash = fnv1a64_update(hash, relative.as_bytes());
                let bytes = fs::read(&path).map_err(|err| Error::from_io(&path, err))?;
                hash = fnv1a64_update(hash, &bytes);
            }
        }
    }

    Ok(hash)
}

fn collect_tree_entries(root: &Path, dir: &Path, entries: &mut Vec<TreeEntry>) -> Result<()> {
    let _ = root;
    let dir_entries = fs::read_dir(dir).map_err(|err| Error::from_io(dir, err))?;
    for entry in dir_entries {
        let entry = entry.map_err(Error::from_io_plain)?;
        let path = entry.path();
        if path.is_dir() {
            entries.push(TreeEntry::Dir(path.clone()));
            collect_tree_entries(root, &path, entries)?;
            continue;
        }
        if path.is_file() {
            entries.push(TreeEntry::File(path));
        }
    }
    Ok(())
}

fn metadata_modified_nanos(modified: &SystemTime) -> Result<u128> {
    Ok(modified
        .duration_since(UNIX_EPOCH)
        .map_err(|err| Error::Execution(format!("failed to normalize file timestamp: {err}")))?
        .as_nanos())
}

#[cfg(unix)]
fn file_changed_nanos(_path: &Path, metadata: &fs::Metadata) -> Option<u128> {
    use std::os::unix::fs::MetadataExt;

    let seconds = u128::try_from(metadata.ctime()).ok()?;
    let nanos = u128::try_from(metadata.ctime_nsec()).ok()?;
    Some(seconds.saturating_mul(1_000_000_000).saturating_add(nanos))
}

#[cfg(windows)]
fn file_changed_nanos(path: &Path, _metadata: &fs::Metadata) -> Option<u128> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_BASIC_INFO, FileBasicInfo, GetFileInformationByHandleEx,
    };

    let file = fs::File::open(path).ok()?;
    let mut info = MaybeUninit::<FILE_BASIC_INFO>::uninit();
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileBasicInfo,
            info.as_mut_ptr().cast(),
            u32::try_from(std::mem::size_of::<FILE_BASIC_INFO>()).ok()?,
        )
    };
    if ok == 0 {
        return None;
    }
    u128::try_from(unsafe { info.assume_init() }.ChangeTime).ok()
}

#[cfg(not(any(unix, windows)))]
fn file_changed_nanos(_path: &Path, _metadata: &fs::Metadata) -> Option<u128> {
    None
}

impl ActionState {
    fn parse(source: &str, path: &Path) -> Result<Self> {
        let mut state = Self {
            fingerprint: String::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
        };
        let mut section = Section::Root;

        for raw_line in source.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with("[[") {
                section = match line {
                    "[[input]]" => {
                        state.inputs.push(ActionStatePath {
                            path: PathBuf::new(),
                            digest: String::new(),
                            len: None,
                            modified_nanos: None,
                            changed_nanos: None,
                        });
                        Section::Input(state.inputs.len() - 1)
                    }
                    "[[output]]" => {
                        state.outputs.push(ActionStatePath {
                            path: PathBuf::new(),
                            digest: String::new(),
                            len: None,
                            modified_nanos: None,
                            changed_nanos: None,
                        });
                        Section::Output(state.outputs.len() - 1)
                    }
                    other => {
                        return Err(Error::Execution(format!(
                            "failed to parse build state `{}`: unsupported table `{other}`",
                            path.display()
                        )));
                    }
                };
                continue;
            }

            let (key, raw_value) = split_key_value(line).map_err(|message| {
                Error::Execution(format!(
                    "failed to parse build state `{}`: {message}",
                    path.display()
                ))
            })?;

            match section {
                Section::Root => match key {
                    "fingerprint" => {
                        state.fingerprint = parse_string(raw_value).map_err(|message| {
                            Error::Execution(format!(
                                "failed to parse build state `{}`: {message}",
                                path.display()
                            ))
                        })?
                    }
                    _ => {
                        return Err(Error::Execution(format!(
                            "failed to parse build state `{}`: unsupported key `{key}`",
                            path.display()
                        )));
                    }
                },
                Section::Input(index) => {
                    let entry = &mut state.inputs[index];
                    match key {
                        "path" => {
                            entry.path = normalize_state_path(Path::new(
                                &parse_string(raw_value).map_err(|message| {
                                    Error::Execution(format!(
                                        "failed to parse build state `{}`: {message}",
                                        path.display()
                                    ))
                                })?,
                            ))
                        }
                        "digest" => {
                            entry.digest = parse_string(raw_value).map_err(|message| {
                                Error::Execution(format!(
                                    "failed to parse build state `{}`: {message}",
                                    path.display()
                                ))
                            })?
                        }
                        "len" => {
                            entry.len = Some(parse_u64(raw_value).map_err(|message| {
                                Error::Execution(format!(
                                    "failed to parse build state `{}`: {message}",
                                    path.display()
                                ))
                            })?)
                        }
                        "modified-nanos" => {
                            entry.modified_nanos =
                                Some(parse_u128(raw_value).map_err(|message| {
                                    Error::Execution(format!(
                                        "failed to parse build state `{}`: {message}",
                                        path.display()
                                    ))
                                })?)
                        }
                        "changed-nanos" => {
                            entry.changed_nanos =
                                Some(parse_u128(raw_value).map_err(|message| {
                                    Error::Execution(format!(
                                        "failed to parse build state `{}`: {message}",
                                        path.display()
                                    ))
                                })?)
                        }
                        _ => {
                            return Err(Error::Execution(format!(
                                "failed to parse build state `{}`: unsupported [[input]] key `{key}`",
                                path.display()
                            )));
                        }
                    }
                }
                Section::Output(index) => {
                    let entry = &mut state.outputs[index];
                    match key {
                        "path" => {
                            entry.path = normalize_state_path(Path::new(
                                &parse_string(raw_value).map_err(|message| {
                                    Error::Execution(format!(
                                        "failed to parse build state `{}`: {message}",
                                        path.display()
                                    ))
                                })?,
                            ))
                        }
                        "digest" => {
                            entry.digest = parse_string(raw_value).map_err(|message| {
                                Error::Execution(format!(
                                    "failed to parse build state `{}`: {message}",
                                    path.display()
                                ))
                            })?
                        }
                        "len" => {
                            entry.len = Some(parse_u64(raw_value).map_err(|message| {
                                Error::Execution(format!(
                                    "failed to parse build state `{}`: {message}",
                                    path.display()
                                ))
                            })?)
                        }
                        "modified-nanos" => {
                            entry.modified_nanos =
                                Some(parse_u128(raw_value).map_err(|message| {
                                    Error::Execution(format!(
                                        "failed to parse build state `{}`: {message}",
                                        path.display()
                                    ))
                                })?)
                        }
                        "changed-nanos" => {
                            entry.changed_nanos =
                                Some(parse_u128(raw_value).map_err(|message| {
                                    Error::Execution(format!(
                                        "failed to parse build state `{}`: {message}",
                                        path.display()
                                    ))
                                })?)
                        }
                        _ => {
                            return Err(Error::Execution(format!(
                                "failed to parse build state `{}`: unsupported [[output]] key `{key}`",
                                path.display()
                            )));
                        }
                    }
                }
            }
        }

        if state.fingerprint.is_empty()
            || state
                .inputs
                .iter()
                .chain(&state.outputs)
                .any(|entry| !entry.has_required_fields())
        {
            return Err(Error::Execution(format!(
                "failed to parse build state `{}`: invalid or missing required fields",
                path.display()
            )));
        }

        Ok(state)
    }

    fn render(&self) -> String {
        let mut out = String::new();
        push_string_line(&mut out, "fingerprint", &self.fingerprint);

        for entry in &self.inputs {
            out.push_str("\n[[input]]\n");
            push_string_line(&mut out, "path", &entry.path.to_string_lossy());
            push_string_line(&mut out, "digest", &entry.digest);
            if let Some(len) = entry.len {
                out.push_str(&format!("len = {len}\n"));
            }
            if let Some(modified_nanos) = entry.modified_nanos {
                out.push_str(&format!("modified-nanos = {modified_nanos}\n"));
            }
            if let Some(changed_nanos) = entry.changed_nanos {
                out.push_str(&format!("changed-nanos = {changed_nanos}\n"));
            }
        }

        for entry in &self.outputs {
            out.push_str("\n[[output]]\n");
            push_string_line(&mut out, "path", &entry.path.to_string_lossy());
            push_string_line(&mut out, "digest", &entry.digest);
            if let Some(len) = entry.len {
                out.push_str(&format!("len = {len}\n"));
            }
            if let Some(modified_nanos) = entry.modified_nanos {
                out.push_str(&format!("modified-nanos = {modified_nanos}\n"));
            }
            if let Some(changed_nanos) = entry.changed_nanos {
                out.push_str(&format!("changed-nanos = {changed_nanos}\n"));
            }
        }

        out
    }
}

fn normalize_state_path(path: &Path) -> PathBuf {
    strip_macos_private_var_prefix(strip_windows_verbatim_prefix(path.to_path_buf()))
}

#[cfg(windows)]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix("\\\\?\\UNC\\") {
        return PathBuf::from(format!("\\\\{stripped}"));
    }
    if let Some(stripped) = raw.strip_prefix("\\\\?\\") {
        return PathBuf::from(stripped);
    }
    path
}

#[cfg(not(windows))]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    path
}

#[cfg(target_os = "macos")]
fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix("/private/var/") {
        return PathBuf::from(format!("/var/{stripped}"));
    }
    if raw == "/private/var" {
        return PathBuf::from("/var");
    }
    path
}

#[cfg(not(target_os = "macos"))]
fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
    path
}

fn split_key_value(line: &str) -> std::result::Result<(&str, &str), String> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(format!("expected `key = value`, found `{line}`"));
    };
    let key = key.trim();
    let value = value.trim();
    if key.is_empty() || value.is_empty() {
        return Err(format!("expected `key = value`, found `{line}`"));
    }
    Ok((key, value))
}

fn parse_u64(raw: &str) -> std::result::Result<u64, String> {
    raw.trim()
        .parse::<u64>()
        .map_err(|_| format!("expected unsigned integer, found `{}`", raw.trim()))
}

fn parse_u128(raw: &str) -> std::result::Result<u128, String> {
    raw.trim()
        .parse::<u128>()
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

fn fnv1a64(bytes: &[u8]) -> u64 {
    fnv1a64_update(0xcbf29ce484222325, bytes)
}

fn fnv1a64_update(mut hash: u64, bytes: &[u8]) -> u64 {
    const PRIME: u64 = 0x100000001b3;

    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }

    hash
}

#[cfg(test)]
mod tests {
    use super::{action_state_is_current, record_action_state};
    #[cfg(unix)]
    use std::ffi::CString;
    use std::fs;
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;
    use std::time::{SystemTime, UNIX_EPOCH};

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
    fn action_state_detects_file_changes_after_cached_check() {
        let root = temp_dir("craft-build-state");
        let input = root.join("input.txt");
        let output = root.join("output.txt");

        fs::write(&input, "input").unwrap();
        fs::write(&output, "alpha").unwrap();
        record_action_state(
            &output,
            "fingerprint".to_string(),
            std::slice::from_ref(&input),
            std::slice::from_ref(&output),
        )
        .unwrap();

        assert!(action_state_is_current(&output, "fingerprint").unwrap());
        fs::write(&output, "changed-output").unwrap();
        assert!(!action_state_is_current(&output, "fingerprint").unwrap());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn action_state_detects_same_size_file_changes() {
        let root = temp_dir("craft-build-state-same-size");
        let input = root.join("input.txt");
        let output = root.join("output.txt");

        fs::write(&input, "input").unwrap();
        fs::write(&output, "alpha").unwrap();
        record_action_state(
            &output,
            "fingerprint".to_string(),
            std::slice::from_ref(&input),
            std::slice::from_ref(&output),
        )
        .unwrap();

        assert!(action_state_is_current(&output, "fingerprint").unwrap());
        thread::sleep(Duration::from_millis(20));
        fs::write(&output, "bravo").unwrap();
        assert!(!action_state_is_current(&output, "fingerprint").unwrap());

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn action_state_detects_same_size_file_changes_with_unchanged_timestamp() {
        let root = temp_dir("craft-build-state-same-timestamp");
        let input = root.join("input.txt");
        let output = root.join("output.txt");

        fs::write(&input, "alpha").unwrap();
        fs::write(&output, "output").unwrap();
        record_action_state(
            &output,
            "fingerprint".to_string(),
            std::slice::from_ref(&input),
            std::slice::from_ref(&output),
        )
        .unwrap();

        let metadata = fs::metadata(&input).unwrap();
        let modified = metadata.modified().unwrap();
        thread::sleep(Duration::from_millis(20));
        fs::write(&input, "bravo").unwrap();
        set_modified_time(&input, modified);

        assert!(!action_state_is_current(&output, "fingerprint").unwrap());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn action_state_without_complete_metadata_uses_digest_path() {
        let root = temp_dir("craft-build-state-metadata");
        let input = root.join("input.txt");
        let output = root.join("output.txt");

        fs::write(&input, "input").unwrap();
        fs::write(&output, "output").unwrap();
        record_action_state(
            &output,
            "fingerprint".to_string(),
            std::slice::from_ref(&input),
            std::slice::from_ref(&output),
        )
        .unwrap();
        let state_path = super::action_state_path(&output);
        let mut state = fs::read_to_string(&state_path).unwrap();
        state = state
            .lines()
            .filter(|line| !line.trim_start().starts_with("changed-nanos"))
            .collect::<Vec<_>>()
            .join("\n");
        state.push('\n');
        fs::write(&state_path, state).unwrap();

        assert!(action_state_is_current(&output, "fingerprint").unwrap());

        thread::sleep(Duration::from_millis(20));
        fs::write(&input, "bravo").unwrap();

        assert!(!action_state_is_current(&output, "fingerprint").unwrap());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn action_state_rejects_legacy_version_field() {
        let root = temp_dir("craft-build-state-legacy-version");
        let input = root.join("input.txt");
        let output = root.join("output.txt");

        fs::write(&input, "input").unwrap();
        fs::write(&output, "output").unwrap();
        record_action_state(
            &output,
            "fingerprint".to_string(),
            std::slice::from_ref(&input),
            std::slice::from_ref(&output),
        )
        .unwrap();
        let state_path = super::action_state_path(&output);
        let mut state = fs::read_to_string(&state_path).unwrap();
        state.insert_str(0, "version = 3\n");
        fs::write(&state_path, state).unwrap();

        assert!(!action_state_is_current(&output, "fingerprint").unwrap());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn action_state_detects_empty_directory_changes() {
        let root = temp_dir("craft-build-state-empty-dir");
        let input = root.join("input");
        let output = root.join("output");

        fs::create_dir_all(input.join("nested")).unwrap();
        fs::create_dir_all(output.join("nested")).unwrap();
        fs::write(input.join("data.txt"), "input").unwrap();
        fs::write(output.join("data.txt"), "output").unwrap();
        record_action_state(
            &output,
            "fingerprint".to_string(),
            std::slice::from_ref(&input),
            std::slice::from_ref(&output),
        )
        .unwrap();

        assert!(action_state_is_current(&output, "fingerprint").unwrap());
        fs::create_dir_all(output.join("extra-empty")).unwrap();
        assert!(!action_state_is_current(&output, "fingerprint").unwrap());

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    fn set_modified_time(path: &std::path::Path, modified: SystemTime) {
        use std::os::unix::ffi::OsStrExt;

        let duration = modified.duration_since(UNIX_EPOCH).unwrap();
        let seconds = duration.as_secs() as libc::time_t;
        let nanoseconds = duration.subsec_nanos() as libc::c_long;
        let times = [
            libc::timespec {
                tv_sec: seconds,
                tv_nsec: nanoseconds,
            },
            libc::timespec {
                tv_sec: seconds,
                tv_nsec: nanoseconds,
            },
        ];
        let c_path = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: c_path is a nul-terminated copy of the test path and times points to the two
        // valid timespec values required by utimensat for atime and mtime.
        let result = unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
        assert_eq!(result, 0);
    }
}
