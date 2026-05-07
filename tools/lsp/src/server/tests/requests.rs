use super::*;

#[test]
fn document_highlight_request_returns_same_file_spans() {
    let mut state = initialized_state();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper() + helper(); }\n";
    let uri = temp_file_uri("server_document_highlight", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(21)),
            method: Some("textDocument/documentHighlight".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 24 }
            })),
        },
    );

    assert_eq!(response["id"], json!(21));
    let highlights = response["result"].as_array().unwrap();
    assert_eq!(highlights.len(), 3);
    assert_eq!(
        highlights[0]["range"]["start"],
        json!({ "line": 0, "character": 3 })
    );
    assert_eq!(
        highlights[1]["range"]["start"],
        json!({ "line": 1, "character": 23 })
    );
    assert_eq!(
        highlights[2]["range"]["start"],
        json!({ "line": 1, "character": 34 })
    );
    assert!(
        highlights
            .iter()
            .all(|highlight| highlight["kind"] == json!(1))
    );
}

#[test]
fn code_action_request_returns_quick_fix_edits() {
    let mut state = initialized_state();
    let source = "fn main() i32 {\n    let value = i32.{1}\n    return value;\n}\n";
    let uri = temp_file_uri("server_code_action", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(22)),
            method: Some("textDocument/codeAction".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 2, "character": 0 },
                    "end": { "line": 2, "character": 20 }
                },
                "context": {
                    "diagnostics": [],
                    "only": ["quickfix"]
                }
            })),
        },
    );

    assert_eq!(response["id"], json!(22));
    let actions = response["result"].as_array().unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0]["title"], "Insert `;`");
    assert_eq!(actions[0]["kind"], "quickfix");
    assert_eq!(
        actions[0]["edit"]["changes"][&uri][0]["range"]["start"],
        json!({ "line": 2, "character": 4 })
    );
    assert_eq!(actions[0]["edit"]["changes"][&uri][0]["newText"], ";");
}

#[test]
fn code_action_request_skips_analysis_for_non_quickfix_filters() {
    let mut state = initialized_state();
    let source = "fn main() i32 {\n    let value = i32.{1}\n    return value;\n}\n";
    let uri = temp_file_uri("server_code_action_filter", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(23)),
            method: Some("textDocument/codeAction".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 3, "character": 1 }
                },
                "context": {
                    "diagnostics": [],
                    "only": ["refactor"]
                }
            })),
        },
    );

    assert_eq!(response["id"], json!(23));
    assert_eq!(response["result"], json!([]));
}

#[test]
fn non_analyzing_request_does_not_trace_stale_analysis_tier() {
    let mut state = initialized_state();
    state.trace = super::super::lifecycle::TraceValue::Verbose;
    let source = "fn main() void {\n    let m\n}\n";
    let uri = temp_file_uri("server_stale_tier_trace", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let completion_messages = dispatch_messages(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(1230)),
            method: Some("textDocument/completion".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 9 }
            })),
        },
    );
    assert_eq!(completion_messages.len(), 2);

    let code_action_messages = dispatch_messages(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(1231)),
            method: Some("textDocument/codeAction".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 2, "character": 1 }
                },
                "context": {
                    "diagnostics": [],
                    "only": ["refactor"]
                }
            })),
        },
    );

    assert_eq!(code_action_messages.len(), 1);
    assert_eq!(code_action_messages[0]["id"], json!(1231));
    assert_eq!(code_action_messages[0]["result"], json!([]));
}

#[test]
fn rename_request_returns_workspace_edit() {
    let mut state = initialized_state();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper() + helper(); }\n";
    let uri = temp_file_uri("server_rename", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(24)),
            method: Some("textDocument/rename".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 24 },
                "newName": "assist"
            })),
        },
    );

    assert_eq!(response["id"], json!(24));
    let edits = response["result"]["changes"][&uri].as_array().unwrap();
    assert_eq!(edits.len(), 3);
    assert!(edits.iter().all(|edit| edit["newText"] == json!("assist")));
    assert_eq!(
        edits[0]["range"]["start"],
        json!({ "line": 0, "character": 3 })
    );
    assert_eq!(
        edits[1]["range"]["start"],
        json!({ "line": 1, "character": 23 })
    );
    assert_eq!(
        edits[2]["range"]["start"],
        json!({ "line": 1, "character": 34 })
    );
}

#[test]
fn definition_request_returns_definition_location() {
    let mut state = initialized_state();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper() + helper(); }\n";
    let uri = temp_file_uri("server_definition", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25)),
            method: Some("textDocument/definition".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 24 }
            })),
        },
    );

    assert_eq!(response["id"], json!(25));
    assert_eq!(response["result"]["uri"], uri);
    assert_eq!(
        response["result"]["range"]["start"],
        json!({ "line": 0, "character": 3 })
    );
}

#[test]
fn references_request_returns_sorted_locations() {
    let mut state = initialized_state();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper() + helper(); }\n";
    let uri = temp_file_uri("server_references", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(26)),
            method: Some("textDocument/references".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 24 },
                "context": { "includeDeclaration": false }
            })),
        },
    );

    assert_eq!(response["id"], json!(26));
    let locations = response["result"].as_array().unwrap();
    assert_eq!(locations.len(), 2);
    assert_eq!(locations[0]["uri"], uri);
    assert_eq!(
        locations[0]["range"]["start"],
        json!({ "line": 1, "character": 23 })
    );
    assert_eq!(
        locations[1]["range"]["start"],
        json!({ "line": 1, "character": 34 })
    );
}

#[test]
fn hover_request_returns_signature_markup() {
    let mut state = initialized_state();
    let source = "fn helper(x: i32) i32 { return x; }\nfn main() i32 { return helper(1); }\n";
    let uri = temp_file_uri("server_hover", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(27)),
            method: Some("textDocument/hover".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 24 }
            })),
        },
    );

    assert_eq!(response["id"], json!(27));
    assert_eq!(response["result"]["contents"]["kind"], "markdown");
    let contents = response["result"]["contents"]["value"].as_str().unwrap();
    assert!(contents.contains("fn helper: &fn(i32) i32"));
}

#[test]
fn signature_help_request_returns_active_parameter_information() {
    let mut state = initialized_state();
    let source = concat!(
        "fn helper(first: i32, second: i32) i32 {\n",
        "    return first + second;\n",
        "}\n",
        "fn main() i32 {\n",
        "    let value = i32.{2};\n",
        "    return helper(1, value);\n",
        "}\n",
    );
    let uri = temp_file_uri("server_signature_help", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(32)),
            method: Some("textDocument/signatureHelp".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 5, "character": 22 }
            })),
        },
    );

    assert_eq!(response["id"], json!(32));
    assert_eq!(response["result"]["activeSignature"], 0);
    assert_eq!(response["result"]["activeParameter"], 1);
    assert_eq!(
        response["result"]["signatures"][0]["label"],
        "helper(first: i32, second: i32) i32"
    );
}

#[test]
fn prepare_rename_request_returns_placeholder_and_range() {
    let mut state = initialized_state();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper(); }\n";
    let uri = temp_file_uri("server_prepare_rename", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(28)),
            method: Some("textDocument/prepareRename".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 24 }
            })),
        },
    );

    assert_eq!(response["id"], json!(28));
    assert_eq!(response["result"]["placeholder"], "helper");
    assert_eq!(
        response["result"]["range"]["start"],
        json!({ "line": 1, "character": 23 })
    );
}

#[test]
fn document_symbol_request_returns_top_level_symbols() {
    let mut state = initialized_state();
    let source = concat!(
        "struct Point { x: i32 }\n",
        "fn helper(point: Point) i32 {\n",
        "    return point.x;\n",
        "}\n",
    );
    let uri = temp_file_uri("server_document_symbol", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(29)),
            method: Some("textDocument/documentSymbol".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri }
            })),
        },
    );

    assert_eq!(response["id"], json!(29));
    let symbols = response["result"].as_array().unwrap();
    let names = symbols
        .iter()
        .map(|symbol| symbol["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(names.contains(&"Point"));
    assert!(names.contains(&"helper"));
}

#[test]
fn semantic_tokens_request_returns_encoded_token_data() {
    let mut state = initialized_state();
    let source = concat!(
        "struct Point { x: i32 }\n",
        "fn helper(point: Point) i32 {\n",
        "    return point.x;\n",
        "}\n",
    );
    let uri = temp_file_uri("server_semantic_tokens", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(31)),
            method: Some("textDocument/semanticTokens/full".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri }
            })),
        },
    );

    assert_eq!(response["id"], json!(31));
    let data = response["result"]["data"].as_array().unwrap();
    assert!(!data.is_empty());
    assert_eq!(data.len() % 5, 0);
}

#[test]
fn verbose_trace_reports_dirty_semantic_tokens_as_lexical() {
    let mut state = initialized_state();
    state.trace = super::super::lifecycle::TraceValue::Verbose;
    let clean = "fn main() i32 {\n    return 0;\n}\n";
    let dirty = "fn main() i32 {\n    let value = 0\n}\n";
    let uri = temp_file_uri("server_dirty_semantic_tokens_trace", clean);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, clean, 1));
    assert!(dispatch_messages(&mut state, did_change_message(&uri, dirty, 2)).is_empty());
    let messages = dispatch_messages(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(3101)),
            method: Some("textDocument/semanticTokens/full".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri }
            })),
        },
    );

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["id"], json!(3101));
    assert_eq!(messages[1]["method"], "$/logTrace");
    assert_eq!(messages[1]["params"]["message"], "analysis tier selected");
    let verbose = messages[1]["params"]["verbose"].as_str().unwrap();
    assert!(verbose.contains("tier=lexical"), "{verbose}");
    assert!(verbose.contains("budget=ok"), "{verbose}");
    assert!(verbose.contains("lane=Interactive"), "{verbose}");
}

#[test]
fn verbose_trace_reports_dirty_code_actions_as_parse_only() {
    let mut state = initialized_state();
    state.trace = super::super::lifecycle::TraceValue::Verbose;
    let clean = "fn main() i32 {\n    let value = i32.{1};\n    return value;\n}\n";
    let dirty = "fn main() i32 {\n    let value = i32.{1}\n    return value;\n}\n";
    let uri = temp_file_uri("server_dirty_code_action_trace", clean);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, clean, 1));
    assert!(dispatch_messages(&mut state, did_change_message(&uri, dirty, 2)).is_empty());
    let messages = dispatch_messages(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(3102)),
            method: Some("textDocument/codeAction".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 2, "character": 0 },
                    "end": { "line": 2, "character": 20 }
                },
                "context": {
                    "diagnostics": [],
                    "only": ["quickfix"]
                }
            })),
        },
    );

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["id"], json!(3102));
    assert_eq!(messages[1]["method"], "$/logTrace");
    assert_eq!(messages[1]["params"]["message"], "analysis tier selected");
    let verbose = messages[1]["params"]["verbose"].as_str().unwrap();
    assert!(verbose.contains("tier=parse-only"), "{verbose}");
    assert!(verbose.contains("budget=ok"), "{verbose}");
    assert!(verbose.contains("lane=Interactive"), "{verbose}");
}

#[test]
fn verbose_trace_reports_dirty_signature_help_as_clean_semantic() {
    let mut state = initialized_state();
    state.trace = super::super::lifecycle::TraceValue::Verbose;
    let clean = concat!(
        "fn helper(first: i32, second: i32) i32 {\n",
        "    return first + second;\n",
        "}\n",
        "fn main() i32 {\n",
        "    let value = i32.{2};\n",
        "    return helper(1, value);\n",
        "}\n",
    );
    let dirty = concat!(
        "fn helper(first: i32, second: i32) i32 {\n",
        "    return first + second;\n",
        "}\n",
        "fn main() i32 {\n",
        "    let value = i32.{2}\n",
        "    return helper(1, value);\n",
        "}\n",
    );
    let uri = temp_file_uri("server_dirty_signature_help_trace", clean);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, clean, 1));
    assert!(dispatch_messages(&mut state, did_change_message(&uri, dirty, 2)).is_empty());
    let messages = dispatch_messages(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(3103)),
            method: Some("textDocument/signatureHelp".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 5, "character": 22 }
            })),
        },
    );

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["id"], json!(3103));
    assert_eq!(messages[1]["method"], "$/logTrace");
    assert_eq!(messages[1]["params"]["message"], "analysis tier selected");
    let verbose = messages[1]["params"]["verbose"].as_str().unwrap();
    assert!(verbose.contains("tier=clean-semantic"), "{verbose}");
    assert!(verbose.contains("budget=ok"), "{verbose}");
    assert!(verbose.contains("lane=Interactive"), "{verbose}");
}

#[test]
fn verbose_trace_reports_dirty_definition_as_clean_semantic() {
    let messages = dirty_navigation_trace_messages(
        "server_dirty_definition_trace",
        3104,
        "textDocument/definition",
        json!({ "line": 3, "character": 11 }),
        None,
    );

    assert_dirty_navigation_trace(messages, 3104);
}

#[test]
fn verbose_trace_reports_dirty_references_as_clean_semantic() {
    let messages = dirty_navigation_trace_messages(
        "server_dirty_references_trace",
        3105,
        "textDocument/references",
        json!({ "line": 3, "character": 11 }),
        Some(json!({ "includeDeclaration": true })),
    );

    assert_dirty_navigation_trace(messages, 3105);
}

#[test]
fn verbose_trace_reports_dirty_document_highlights_as_clean_semantic() {
    let messages = dirty_navigation_trace_messages(
        "server_dirty_document_highlight_trace",
        3106,
        "textDocument/documentHighlight",
        json!({ "line": 3, "character": 11 }),
        None,
    );

    assert_dirty_navigation_trace(messages, 3106);
}

fn dirty_navigation_trace_messages(
    prefix: &str,
    id: i64,
    method: &str,
    position: Value,
    context: Option<Value>,
) -> Vec<Value> {
    let mut state = initialized_state();
    state.trace = super::super::lifecycle::TraceValue::Verbose;
    let clean = concat!(
        "fn helper() i32 { return 1; }\n",
        "fn main() i32 {\n",
        "    let value = i32.{1};\n",
        "    return helper() + helper();\n",
        "}\n",
    );
    let dirty = concat!(
        "fn helper() i32 { return 1; }\n",
        "fn main() i32 {\n",
        "    let value = i32.{1}\n",
        "    return helper() + helper();\n",
        "}\n",
    );
    let uri = temp_file_uri(prefix, clean);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, clean, 1));
    assert!(dispatch_messages(&mut state, did_change_message(&uri, dirty, 2)).is_empty());

    let mut params = json!({
        "textDocument": { "uri": uri },
        "position": position,
    });
    if let Some(context) = context {
        params["context"] = context;
    }

    dispatch_messages(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(id)),
            method: Some(method.to_string()),
            params: Some(params),
        },
    )
}

fn assert_dirty_navigation_trace(messages: Vec<Value>, id: i64) {
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["id"], json!(id));
    assert_eq!(messages[1]["method"], "$/logTrace");
    assert_eq!(messages[1]["params"]["message"], "analysis tier selected");
    let verbose = messages[1]["params"]["verbose"].as_str().unwrap();
    assert!(verbose.contains("tier=clean-semantic"), "{verbose}");
    assert!(verbose.contains("budget=ok"), "{verbose}");
    assert!(verbose.contains("lane=Interactive"), "{verbose}");
}
