mod basics;
mod completion;
mod diagnostics;
mod requests;
mod scheduler;
mod stress;

pub(super) use super::*;
pub(super) use crate::analysis::{AnalysisOutcome, DiagnosticBundle};
pub(super) use crate::protocol::{
    DidChangeTextDocumentParams, DidOpenTextDocumentParams, IncomingMessage,
    InitializeResultOptions, JSONRPC_VERSION, TextDocumentContentChangeEvent,
    VersionedTextDocumentIdentifier,
};
pub(super) use crate::transport::{MessageReader, MessageWriter};
pub(super) use serde_json::{Value, json};
pub(super) use std::fs;
pub(super) use std::io::Cursor;
pub(super) use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
pub(super) use std::time::{SystemTime, UNIX_EPOCH};

static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) fn initialized_state() -> ServerState {
    let mut state = ServerState::new();
    state.initialized = true;
    state
}

pub(super) fn dispatch_single_response(state: &mut ServerState, message: IncomingMessage) -> Value {
    let messages = dispatch_messages(state, message);
    assert_eq!(messages.len(), 1);
    messages.into_iter().next().unwrap()
}

pub(super) fn dispatch_messages(state: &mut ServerState, message: IncomingMessage) -> Vec<Value> {
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);
    let should_exit = handle_message(state, &mut writer, message).unwrap();
    assert!(!should_exit);
    drain_scheduler_to_quiescence(state, &mut writer);
    read_all_messages(&output)
}

pub(super) fn drain_scheduler_to_quiescence(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl std::io::Write>,
) {
    while state.pending_workspace_refresh_tasks > 0
        || state.pending_diagnostics_worker_tasks > 0
        || state.pending_prewarm_tasks > 0
        || state.has_pending_document_request_work()
        || !state.pending_diagnostics.is_empty()
    {
        if state.pending_prewarm_tasks > 0 {
            super::scheduler::flush_prewarm_results(state, writer, true).unwrap();
        }
        if state.pending_workspace_refresh_tasks > 0 {
            super::scheduler::flush_workspace_refresh_results(state, writer, true).unwrap();
        }
        if state.pending_diagnostics_worker_tasks > 0 {
            super::scheduler::flush_diagnostics_results(state, writer, true).unwrap();
        }
        if state.has_pending_document_request_work() {
            super::scheduler::flush_document_request_results(state, writer, true).unwrap();
        }
        if state.pending_workspace_refresh_reason.is_some()
            || !state.pending_diagnostics.is_empty()
            || (!state.pending_diagnostics_targets.is_empty()
                && state.pending_diagnostics_worker_tasks == 0)
        {
            super::scheduler::drain_scheduler(state, writer).unwrap();
        }
    }
}

pub(super) fn did_open_message(uri: &str, text: &str, version: i64) -> IncomingMessage {
    IncomingMessage {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: None,
        method: Some("textDocument/didOpen".to_string()),
        params: Some(json!({
            "textDocument": {
                "uri": uri,
                "languageId": "kern",
                "version": version,
                "text": text
            }
        })),
    }
}

pub(super) fn did_change_message(uri: &str, text: &str, version: i64) -> IncomingMessage {
    IncomingMessage {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: None,
        method: Some("textDocument/didChange".to_string()),
        params: Some(json!({
            "textDocument": {
                "uri": uri,
                "version": version
            },
            "contentChanges": [
                {
                    "text": text
                }
            ]
        })),
    }
}

pub(super) fn did_save_message(uri: &str) -> IncomingMessage {
    IncomingMessage {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: None,
        method: Some("textDocument/didSave".to_string()),
        params: Some(json!({
            "textDocument": {
                "uri": uri
            }
        })),
    }
}

pub(super) fn temp_file_uri(prefix: &str, initial_text: &str) -> String {
    let path = unique_temp_file_path(prefix);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, initial_text).unwrap();
    file_path_to_uri_for_test(&path)
}

pub(super) fn invalid_manifest_document_uri(prefix: &str, source: &str) -> String {
    let root = unique_temp_file_path(prefix);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("Craft.toml"), "not valid craft toml").unwrap();
    let source_path = root.join("src/main.kn");
    fs::write(&source_path, source).unwrap();
    file_path_to_uri_for_test(&source_path)
}

pub(super) fn unique_temp_dir(prefix: &str) -> PathBuf {
    let path = unique_temp_file_path(prefix);
    fs::create_dir_all(&path).unwrap();
    path
}

fn unique_temp_file_path(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let counter = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "{}_{}_{}_{}.kn",
        prefix,
        std::process::id(),
        nanos,
        counter
    ))
}

pub(super) fn file_path_to_uri_for_test(path: &Path) -> String {
    let mut rendered = std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/");
    if !rendered.starts_with('/') {
        rendered.insert(0, '/');
    }
    format!("file://{rendered}")
}

pub(super) fn assert_uri_path_ends_with(uri: &str, suffix: impl AsRef<Path>) {
    let path = crate::analysis::uri_to_file_path(uri)
        .unwrap_or_else(|| panic!("expected file uri path, got {uri}"));
    assert!(
        path.ends_with(suffix.as_ref()),
        "expected {path:?} to end with {:?}",
        suffix.as_ref()
    );
}

pub(super) fn read_single_response(output: &[u8]) -> Value {
    let mut reader = MessageReader::new(Cursor::new(output));
    let payload = reader.read_message().unwrap().unwrap();
    serde_json::from_slice(&payload).unwrap()
}

pub(super) fn read_all_messages(output: &[u8]) -> Vec<Value> {
    let mut reader = MessageReader::new(Cursor::new(output));
    let mut messages = Vec::new();
    while let Some(payload) = reader.read_message().unwrap() {
        messages.push(serde_json::from_slice(&payload).unwrap());
    }
    messages
}
