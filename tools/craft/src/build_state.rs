use crate::error::{Error, Result};
use crate::local_state;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const ACTION_STATE_VERSION: u32 = 2;

static CURRENT_PROCESS_DIGEST: OnceLock<String> = OnceLock::new();
static FILE_DIGEST_CACHE: OnceLock<Mutex<HashMap<PathBuf, FileDigestCacheEntry>>> = OnceLock::new();

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
}

#[derive(Clone, Copy, Debug)]
enum Section {
    Root,
    Input(usize),
    Output(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileDigestCacheEntry {
    len: u64,
    modified_nanos: u128,
    digest: String,
}

type DigestedPath = (String, Option<u64>, Option<u128>);

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
    invalidate_file_digest(primary_output);
    for path in outputs {
        invalidate_file_digest(path);
    }

    let state = ActionState {
        fingerprint,
        inputs: collect_state_paths(inputs)?,
        outputs: collect_state_paths(outputs)?,
    };
    let state_path = action_state_path(primary_output);
    invalidate_file_digest(&state_path);
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
        let Some((digest, len, modified_nanos)) = digest_path(path)? else {
            return Err(Error::Execution(format!(
                "cannot record build state for missing path `{}`",
                path.display()
            )));
        };
        entries.push(ActionStatePath {
            path: normalize_state_path(path),
            digest,
            len,
            modified_nanos,
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
            digest_file_cached(path)?,
            Some(metadata.len()),
            Some(modified_nanos),
        )));
    }
    if path.is_dir() {
        return Ok(Some((
            format!("fnv1a64:{:016x}", digest_tree(path)?),
            None,
            None,
        )));
    }
    Ok(None)
}

fn path_matches_digest(path: &Path, entry: &ActionStatePath) -> Result<bool> {
    if let (Some(expected_len), Some(expected_modified_nanos)) = (entry.len, entry.modified_nanos) {
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(err) => return Err(Error::from_io(path, err)),
        };
        let modified_nanos =
            metadata_modified_nanos(&metadata.modified().map_err(Error::from_io_plain)?)?;
        if metadata.is_file()
            && metadata.len() == expected_len
            && modified_nanos == expected_modified_nanos
        {
            return Ok(true);
        }
    }

    Ok(digest_path(path)?.map(|(digest, _, _)| digest) == Some(entry.digest.clone()))
}

fn digest_tree(root: &Path) -> Result<u64> {
    let mut paths = Vec::new();
    collect_tree_paths(root, root, &mut paths)?;
    paths.sort();

    let mut hash = 0xcbf29ce484222325_u64;
    for path in paths {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path.as_path())
            .to_string_lossy();
        hash = fnv1a64_update(hash, relative.as_bytes());
        let bytes = fs::read(&path).map_err(|err| Error::from_io(&path, err))?;
        hash = fnv1a64_update(hash, &bytes);
    }

    Ok(hash)
}

fn collect_tree_paths(root: &Path, dir: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    let _ = root;
    let entries = fs::read_dir(dir).map_err(|err| Error::from_io(dir, err))?;
    for entry in entries {
        let entry = entry.map_err(Error::from_io_plain)?;
        let path = entry.path();
        if path.is_dir() {
            collect_tree_paths(root, &path, paths)?;
            continue;
        }
        if path.is_file() {
            paths.push(path);
        }
    }
    Ok(())
}

fn file_digest_cache() -> &'static Mutex<HashMap<PathBuf, FileDigestCacheEntry>> {
    FILE_DIGEST_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn invalidate_file_digest(path: &Path) {
    let cache = file_digest_cache();
    let mut cache = cache.lock().unwrap();
    cache.remove(path);
}

fn digest_file_cached(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path).map_err(|err| Error::from_io(path, err))?;
    let modified_nanos =
        metadata_modified_nanos(&metadata.modified().map_err(Error::from_io_plain)?)?;
    let len = metadata.len();

    let cache = file_digest_cache();
    {
        let cache = cache.lock().unwrap();
        if let Some(entry) = cache.get(path)
            && entry.len == len
            && entry.modified_nanos == modified_nanos
        {
            return Ok(entry.digest.clone());
        }
    }

    let digest = digest_file_contents(path)?;
    let mut cache = cache.lock().unwrap();
    cache.insert(
        path.to_path_buf(),
        FileDigestCacheEntry {
            len,
            modified_nanos,
            digest: digest.clone(),
        },
    );
    Ok(digest)
}

fn metadata_modified_nanos(modified: &SystemTime) -> Result<u128> {
    Ok(modified
        .duration_since(UNIX_EPOCH)
        .map_err(|err| Error::Execution(format!("failed to normalize file timestamp: {err}")))?
        .as_nanos())
}

impl ActionState {
    fn parse(source: &str, path: &Path) -> Result<Self> {
        let mut state = Self {
            fingerprint: String::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
        };
        let mut version = 0_u32;
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
                        });
                        Section::Input(state.inputs.len() - 1)
                    }
                    "[[output]]" => {
                        state.outputs.push(ActionStatePath {
                            path: PathBuf::new(),
                            digest: String::new(),
                            len: None,
                            modified_nanos: None,
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
                    "version" => {
                        version = parse_u32(raw_value).map_err(|message| {
                            Error::Execution(format!(
                                "failed to parse build state `{}`: {message}",
                                path.display()
                            ))
                        })?
                    }
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

        if version != ACTION_STATE_VERSION || state.fingerprint.is_empty() {
            return Err(Error::Execution(format!(
                "failed to parse build state `{}`: invalid or missing required fields",
                path.display()
            )));
        }

        Ok(state)
    }

    fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("version = {ACTION_STATE_VERSION}\n"));
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

fn parse_u32(raw: &str) -> std::result::Result<u32, String> {
    raw.trim()
        .parse::<u32>()
        .map_err(|_| format!("expected unsigned integer, found `{}`", raw.trim()))
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
}
