use super::super::lifecycle::TraceValue;
use super::super::*;
use super::*;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Barrier, Mutex};

#[derive(Clone)]
struct SharedOutput(Arc<Mutex<Vec<u8>>>);

impl Write for SharedOutput {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn initialize_result_advertises_precise_capabilities() {
    let result = initialize_result(InitializeResultOptions::default());

    assert_eq!(result["positionEncoding"], "utf-16");
    assert_eq!(
        result["capabilities"]["completionProvider"]["resolveProvider"],
        true
    );
    assert_eq!(
        result["capabilities"]["completionProvider"]["triggerCharacters"],
        json!(["."])
    );
    assert_eq!(result["capabilities"]["documentHighlightProvider"], true);
    assert_eq!(result["capabilities"]["declarationProvider"], true);
    assert_eq!(result["capabilities"]["typeDefinitionProvider"], true);
    assert_eq!(result["capabilities"]["implementationProvider"], true);
    assert_eq!(
        result["capabilities"]["callHierarchyProvider"]["workDoneProgress"],
        false
    );
    assert_eq!(result["capabilities"]["foldingRangeProvider"], true);
    assert_eq!(result["capabilities"]["selectionRangeProvider"], true);
    assert_eq!(
        result["capabilities"]["documentLinkProvider"]["resolveProvider"],
        false
    );
    assert_eq!(result["capabilities"]["documentFormattingProvider"], true);
    assert_eq!(
        result["capabilities"]["documentRangeFormattingProvider"],
        true
    );
    assert_eq!(result["capabilities"]["workspaceSymbolProvider"], true);
    assert_eq!(
        result["capabilities"]["signatureHelpProvider"]["triggerCharacters"],
        json!(["(", ","])
    );
    assert_eq!(
        result["capabilities"]["codeActionProvider"]["codeActionKinds"],
        json!(["quickfix"])
    );
    assert_eq!(
        result["capabilities"]["codeActionProvider"]["resolveProvider"],
        true
    );
    assert_eq!(
        result["capabilities"]["semanticTokensProvider"]["range"],
        true
    );
    assert_eq!(
        result["capabilities"]["semanticTokensProvider"]["full"]["delta"],
        false
    );
    assert_eq!(result["capabilities"]["inlayHintProvider"], true);
    assert_eq!(
        result["capabilities"]["workspace"]["workspaceFolders"]["supported"],
        false
    );
    assert_eq!(
        result["capabilities"]["workspace"]["workspaceFolders"]["changeNotifications"],
        false
    );
}

#[test]
fn initialize_negotiates_work_done_progress() {
    let mut state = ServerState::new();
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    handle_message(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(14)),
            method: Some("initialize".to_string()),
            params: Some(json!({
                "capabilities": {
                    "window": {
                        "workDoneProgress": true
                    }
                }
            })),
        },
    )
    .unwrap();

    assert!(state.work_done_progress);
}

#[test]
fn initialize_records_root_uri_as_primary_workspace_root() {
    let root = unique_temp_dir("server_initialize_root_uri");
    let mut state = ServerState::new();
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    handle_message(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(15)),
            method: Some("initialize".to_string()),
            params: Some(json!({
                "rootUri": file_path_to_uri_for_test(&root),
                "capabilities": {}
            })),
        },
    )
    .unwrap();

    assert_eq!(state.workspace_root.as_ref(), Some(&root));
    assert!(state.ignored_workspace_folders.is_empty());
}

#[test]
fn initialize_uses_first_workspace_folder_and_warns_about_ignored_folders() {
    let root_a = unique_temp_dir("server_initialize_workspace_a");
    let root_b = unique_temp_dir("server_initialize_workspace_b");
    let mut state = ServerState::new();
    state.trace = TraceValue::Verbose;
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    handle_message(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(16)),
            method: Some("initialize".to_string()),
            params: Some(json!({
                "rootUri": file_path_to_uri_for_test(&root_b),
                "workspaceFolders": [
                    { "uri": file_path_to_uri_for_test(&root_a), "name": "a" },
                    { "uri": file_path_to_uri_for_test(&root_b), "name": "b" }
                ],
                "capabilities": {}
            })),
        },
    )
    .unwrap();

    assert_eq!(state.workspace_root.as_ref(), Some(&root_a));
    assert_eq!(
        state.ignored_workspace_folders,
        vec![file_path_to_uri_for_test(&root_b)]
    );
    let messages = read_all_messages(&output);
    assert!(messages.iter().any(|message| {
        message["method"] == "window/logMessage"
            && message["params"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("single primary workspace folder"))
    }));
}

fn file_path_to_uri_for_test(path: &PathBuf) -> String {
    let mut rendered = path.to_string_lossy().replace('\\', "/");
    if !rendered.starts_with('/') {
        rendered.insert(0, '/');
    }
    format!("file://{rendered}")
}

#[test]
fn rejects_requests_before_initialize() {
    let mut state = ServerState::new();
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    let should_exit = handle_message(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(1)),
            method: Some("textDocument/hover".to_string()),
            params: Some(json!({
                "textDocument": { "uri": "file:///main.kn" },
                "position": { "line": 0, "character": 0 }
            })),
        },
    )
    .unwrap();

    assert!(!should_exit);
    let response = read_single_response(&output);
    assert_eq!(response["id"], json!(1));
    assert_eq!(response["error"]["code"], json!(SERVER_NOT_INITIALIZED));
}

#[test]
fn accepts_common_post_initialize_notifications() {
    let mut state = ServerState::new();
    state.initialized = true;
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    for method in [
        "$/setTrace",
        "$/cancelRequest",
        "workspace/didChangeConfiguration",
        "workspace/didChangeWatchedFiles",
    ] {
        let should_exit = handle_message(
            &mut state,
            &mut writer,
            IncomingMessage {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: None,
                method: Some(method.to_string()),
                params: Some(json!({})),
            },
        )
        .unwrap();

        assert!(!should_exit);
    }

    assert!(output.is_empty());
}

#[test]
fn did_change_configuration_updates_analysis_settings_and_refreshes_workspace() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_configuration_refresh", "fn main() void {}\n");
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    handle_message(
        &mut state,
        &mut writer,
        did_open_message(&uri, "fn main() void {}\n", 1),
    )
    .unwrap();
    drain_scheduler_to_quiescence(&mut state, &mut writer);
    drop(writer);
    output.clear();
    let mut writer = MessageWriter::new(&mut output);

    handle_message(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: None,
            method: Some("workspace/didChangeConfiguration".to_string()),
            params: Some(json!({
                "settings": {
                    "project": {
                        "features": [" experimental ", "experimental", "simd"],
                        "noDefaultFeatures": true,
                        "libraryBundle": "base"
                    }
                }
            })),
        },
    )
    .unwrap();

    assert_eq!(
        state.analysis.settings().compile_options.craft_features,
        vec!["experimental".to_string(), "simd".to_string()]
    );
    assert!(
        !state
            .analysis
            .settings()
            .compile_options
            .craft_default_features
    );
    assert_eq!(
        state.analysis.settings().compile_options.library_bundle,
        kernc_utils::config::LibraryBundle::Base
    );
    drain_scheduler_to_quiescence(&mut state, &mut writer);
    let messages = read_all_messages(&output);
    assert!(messages.iter().any(|message| {
        message["method"] == "textDocument/publishDiagnostics"
            && message["params"]["uri"] == json!(uri)
    }));
}

#[test]
fn did_change_configuration_ignores_equal_settings_without_refresh() {
    let mut state = initialized_state();
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    handle_message(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: None,
            method: Some("workspace/didChangeConfiguration".to_string()),
            params: Some(json!({
                "settings": {
                    "project": {
                        "features": [],
                        "noDefaultFeatures": false,
                        "libraryBundle": "std"
                    }
                }
            })),
        },
    )
    .unwrap();

    assert!(state.pending_workspace_refresh_reason.is_none());
    assert!(output.is_empty());
}

#[test]
fn did_change_configuration_reports_invalid_supported_settings() {
    let mut state = initialized_state();
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    let err = handle_message(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: None,
            method: Some("workspace/didChangeConfiguration".to_string()),
            params: Some(json!({
                "settings": {
                    "project": {
                        "features": ["ok", ""]
                    }
                }
            })),
        },
    )
    .unwrap_err();

    assert!(matches!(err, ServerError::Protocol(message) if message.contains("empty feature")));
    assert!(state.pending_workspace_refresh_reason.is_none());
}

#[test]
fn rejects_requests_after_shutdown() {
    let mut state = ServerState::new();
    state.initialized = true;
    state.shutdown_requested = true;
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    let should_exit = handle_message(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(7)),
            method: Some("textDocument/documentSymbol".to_string()),
            params: Some(json!({
                "textDocument": { "uri": "file:///main.kn" }
            })),
        },
    )
    .unwrap();

    assert!(!should_exit);
    let response = read_single_response(&output);
    assert_eq!(response["error"]["code"], json!(INVALID_REQUEST));
}

#[test]
fn cancel_notification_registers_request_id() {
    let mut state = initialized_state();
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    let should_exit = handle_message(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: None,
            method: Some("$/cancelRequest".to_string()),
            params: Some(json!({ "id": 41 })),
        },
    )
    .unwrap();

    assert!(!should_exit);
    assert_eq!(state.canceled_request_ids, vec![json!(41)]);
    assert!(output.is_empty());
}

#[test]
fn initialize_negotiates_capabilities_from_client_support() {
    let mut state = ServerState::new();
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    handle_message(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(11)),
            method: Some("initialize".to_string()),
            params: Some(json!({
                "capabilities": {
                    "general": {
                        "positionEncodings": ["utf-16", "utf-8"]
                    },
                    "textDocument": {
                        "semanticTokens": {
                            "requests": {
                                "range": false,
                                "full": { "delta": false }
                            }
                        }
                    }
                }
            })),
        },
    )
    .unwrap();

    let messages = read_all_messages(&output);
    assert_eq!(
        messages[0]["result"]["capabilities"]["codeActionProvider"],
        false
    );
    assert_eq!(
        messages[0]["result"]["capabilities"]["renameProvider"],
        true
    );
    assert!(
        messages[0]["result"]["capabilities"]
            .get("semanticTokensProvider")
            .is_some()
    );
    assert_eq!(messages[1]["method"], "window/logMessage");
    assert_eq!(messages[2]["method"], "window/logMessage");
}

#[test]
fn initialize_rejects_clients_without_utf16_support() {
    let mut state = ServerState::new();
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    let err = handle_message(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(12)),
            method: Some("initialize".to_string()),
            params: Some(json!({
                "capabilities": {
                    "general": {
                        "positionEncodings": ["utf-8"]
                    }
                }
            })),
        },
    )
    .unwrap_err();

    assert!(matches!(err, ServerError::Protocol(_)));
    let response = read_single_response(&output);
    assert_eq!(response["error"]["code"], json!(INVALID_REQUEST));
}

#[test]
fn initialize_trace_emits_log_trace_notification() {
    let mut state = ServerState::new();
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    handle_message(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(13)),
            method: Some("initialize".to_string()),
            params: Some(json!({
                "trace": "verbose",
                "clientInfo": { "name": "Example", "version": "1.0" },
                "capabilities": {
                    "general": { "positionEncodings": ["utf-16"] },
                    "textDocument": {
                        "codeAction": {
                            "codeActionLiteralSupport": {
                                "codeActionKind": { "valueSet": ["quickfix"] }
                            }
                        },
                        "rename": { "prepareSupport": true },
                        "semanticTokens": {
                            "requests": { "full": true }
                        }
                    }
                }
            })),
        },
    )
    .unwrap();

    let messages = read_all_messages(&output);
    assert_eq!(messages[1]["method"], "$/logTrace");
    assert_eq!(messages[1]["params"]["message"], "initialize completed");
    assert_eq!(
        messages[1]["params"]["verbose"],
        "client=Example 1.0 | positionEncodings=utf-16"
    );
}

#[test]
fn set_trace_updates_state_and_emits_trace_notification() {
    let mut state = ServerState::new();
    state.trace = TraceValue::Messages;
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    handle_message(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: None,
            method: Some("$/setTrace".to_string()),
            params: Some(json!({
                "value": "verbose"
            })),
        },
    )
    .unwrap();

    assert_eq!(state.trace, TraceValue::Verbose);
    let response = read_single_response(&output);
    assert_eq!(response["method"], "$/logTrace");
    assert_eq!(
        response["params"]["message"],
        "trace level set to `verbose`"
    );
}

#[test]
fn run_loop_reports_parse_errors_and_keeps_processing_messages() {
    let invalid = "{\"oops\":1";
    let valid = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"shutdown\",\"params\":{}}";
    let payload = format!(
        "Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}",
        invalid.len(),
        invalid,
        valid.len(),
        valid
    );
    let reader = MessageReader::new(Cursor::new(payload.into_bytes()));
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);
    let mut state = initialized_state();

    run_message_loop(&mut state, reader, &mut writer).unwrap();

    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["error"]["code"], json!(PARSE_ERROR));
    assert_eq!(messages[0]["id"], json!(null));
    assert_eq!(messages[1]["id"], json!(1));
    assert_eq!(messages[1]["result"], json!(null));
}

#[test]
fn run_loop_ignores_server_request_responses() {
    let workspace_refresh = "{\"jsonrpc\":\"2.0\",\"method\":\"workspace/didChangeWatchedFiles\",\"params\":{\"changes\":[]}}";
    let response = "{\"jsonrpc\":\"2.0\",\"id\":\"kern-lsp/1\",\"result\":null}";
    let shutdown = "{\"jsonrpc\":\"2.0\",\"id\":72,\"method\":\"shutdown\",\"params\":{}}";
    let payload = format!(
        "Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}",
        workspace_refresh.len(),
        workspace_refresh,
        response.len(),
        response,
        shutdown.len(),
        shutdown
    );
    let reader = MessageReader::new(Cursor::new(payload.into_bytes()));
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);
    let mut state = initialized_state();
    state.work_done_progress = true;

    run_message_loop(&mut state, reader, &mut writer).unwrap();

    let messages = read_all_messages(&output);
    assert!(messages.iter().any(|message| {
        message["method"] == "window/workDoneProgress/create"
            && message["id"] == json!("kern-lsp/1")
    }));
    assert!(messages.iter().any(|message| message["id"] == json!(72)));
    assert!(
        messages
            .iter()
            .all(|message| { message["error"]["message"] != "message did not contain a method" })
    );
}

#[test]
fn run_loop_accepts_second_request_while_first_worker_is_running() {
    let uri = temp_file_uri(
        "server_loop_parallel_hover",
        "fn helper() i32 { return 1; }\nfn main() i32 { return helper(); }\n",
    );
    let first = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":70,\"method\":\"textDocument/hover\",\"params\":{{\"textDocument\":{{\"uri\":\"{}\"}},\"position\":{{\"line\":1,\"character\":27}}}}}}",
        uri
    );
    let second = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":71,\"method\":\"textDocument/hover\",\"params\":{{\"textDocument\":{{\"uri\":\"{}\"}},\"position\":{{\"line\":1,\"character\":27}}}}}}",
        uri
    );
    let payload = format!(
        "Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}",
        first.len(),
        first,
        second.len(),
        second
    );
    let reader = MessageReader::new(Cursor::new(payload.into_bytes()));
    let output = Arc::new(Mutex::new(Vec::new()));
    let mut writer = MessageWriter::new(SharedOutput(output.clone()));
    let mut state = initialized_state();
    let started = Arc::new(Barrier::new(3));
    let release = Arc::new(Barrier::new(3));
    *TEST_DOCUMENT_REQUEST_BARRIERS.lock().unwrap() = Some((started.clone(), release.clone()));

    let handle = std::thread::spawn(move || {
        run_message_loop(&mut state, reader, &mut writer).unwrap();
    });

    started.wait();
    release.wait();
    handle.join().unwrap();
    *TEST_DOCUMENT_REQUEST_BARRIERS.lock().unwrap() = None;

    let output = output.lock().unwrap();
    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 2);
    let mut ids = messages
        .iter()
        .map(|message| message["id"].as_i64().unwrap())
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, vec![70, 71]);
}
