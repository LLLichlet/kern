use super::super::lifecycle::TraceValue;
use super::super::*;
use super::*;

#[test]
fn initialize_result_advertises_precise_capabilities() {
    let result = initialize_result(InitializeResultOptions::default());

    assert_eq!(result["positionEncoding"], "utf-16");
    assert_eq!(
        result["capabilities"]["completionProvider"]["resolveProvider"],
        false
    );
    assert_eq!(
        result["capabilities"]["completionProvider"]["triggerCharacters"],
        json!(["."])
    );
    assert_eq!(result["capabilities"]["documentHighlightProvider"], true);
    assert_eq!(
        result["capabilities"]["signatureHelpProvider"]["triggerCharacters"],
        json!(["(", ","])
    );
    assert_eq!(
        result["capabilities"]["codeActionProvider"]["codeActionKinds"],
        json!(["quickfix"])
    );
    assert_eq!(
        result["capabilities"]["semanticTokensProvider"]["range"],
        false
    );
    assert_eq!(
        result["capabilities"]["semanticTokensProvider"]["full"]["delta"],
        false
    );
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
                "textDocument": { "uri": "file:///main.rn" },
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
                "textDocument": { "uri": "file:///main.rn" }
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
    assert_eq!(messages[0]["result"]["capabilities"]["renameProvider"], true);
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
    let mut reader = MessageReader::new(Cursor::new(payload.as_bytes()));
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);
    let mut state = initialized_state();

    run_message_loop(&mut state, &mut reader, &mut writer).unwrap();

    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["error"]["code"], json!(PARSE_ERROR));
    assert_eq!(messages[0]["id"], json!(null));
    assert_eq!(messages[1]["id"], json!(1));
    assert_eq!(messages[1]["result"], json!(null));
}
