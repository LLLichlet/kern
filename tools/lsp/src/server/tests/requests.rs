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
    let source = "fn main() i32 {\n    let value = 1i32\n    return value;\n}\n";
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
    let source = "fn main() i32 {\n    let value = 1i32\n    return value;\n}\n";
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
fn code_action_resolve_returns_eager_action_without_analysis() {
    let mut state = initialized_state();
    assert_eq!(state.analysis.last_analysis_tier(), None);

    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(230)),
            method: Some("codeAction/resolve".to_string()),
            params: Some(json!({
                "title": "Insert `;`",
                "kind": "quickfix",
                "edit": {
                    "changes": {
                        "file:///tmp/main.kn": [
                            {
                                "range": {
                                    "start": { "line": 1, "character": 4 },
                                    "end": { "line": 1, "character": 4 }
                                },
                                "newText": ";"
                            }
                        ]
                    }
                },
                "isPreferred": true
            })),
        },
    );

    assert_eq!(response["id"], json!(230));
    assert_eq!(response["result"]["title"], "Insert `;`");
    assert_eq!(response["result"]["kind"], "quickfix");
    assert_eq!(
        response["result"]["edit"]["changes"]["file:///tmp/main.kn"][0]["newText"],
        ";"
    );
    assert_eq!(response["result"]["isPreferred"], true);
    assert_eq!(state.analysis.last_analysis_tier(), None);
}

#[test]
fn workspace_symbol_request_returns_open_document_symbols() {
    let mut state = initialized_state();
    let source = "struct NeedleBox { value: i32 }\nfn helper() void {}\n";
    let uri = temp_file_uri("server_workspace_symbol_open", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(240)),
            method: Some("workspace/symbol".to_string()),
            params: Some(json!({
                "query": "needle"
            })),
        },
    );

    assert_eq!(response["id"], json!(240));
    assert_eq!(response["result"].as_array().unwrap().len(), 1);
    assert_eq!(response["result"][0]["name"], "NeedleBox");
    assert_eq!(response["result"][0]["location"]["uri"], uri);
    assert_eq!(
        response["result"][0]["location"]["range"]["start"],
        json!({ "line": 0, "character": 7 })
    );
}

#[test]
fn workspace_symbol_request_uses_workspace_root_targets() {
    let root = unique_temp_dir("server_workspace_symbol_project");
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        root.join("Craft.toml"),
        format!(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "{}"

[lib]
root = "src/lib.kn"
"#,
            env!("CARGO_PKG_VERSION")
        ),
    )
    .unwrap();
    fs::write(
        src.join("lib.kn"),
        "struct WorkspaceNeedle { value: i32 }\nfn other() void {}\n",
    )
    .unwrap();

    let mut state = initialized_state();
    state.workspace_root = Some(root.clone());
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(241)),
            method: Some("workspace/symbol".to_string()),
            params: Some(json!({
                "query": "workspace"
            })),
        },
    );

    assert_eq!(response["id"], json!(241));
    assert_eq!(response["result"].as_array().unwrap().len(), 1);
    assert_eq!(response["result"][0]["name"], "WorkspaceNeedle");
    assert!(
        response["result"][0]["location"]["uri"]
            .as_str()
            .unwrap()
            .ends_with("/src/lib.kn")
    );
}

#[test]
fn folding_range_request_returns_block_ranges() {
    let mut state = initialized_state();
    let source = "fn main() void {\n    if true {\n        return;\n    }\n}\n";
    let uri = temp_file_uri("server_folding_range", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(24)),
            method: Some("textDocument/foldingRange".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri }
            })),
        },
    );

    assert_eq!(response["id"], json!(24));
    assert_eq!(
        response["result"],
        json!([
            {
                "startLine": 0,
                "startCharacter": 15,
                "endLine": 4,
                "endCharacter": 1
            },
            {
                "startLine": 1,
                "startCharacter": 12,
                "endLine": 3,
                "endCharacter": 5
            }
        ])
    );
}

#[test]
fn selection_range_request_returns_parent_chain() {
    let mut state = initialized_state();
    let source = "fn main() void {\n    let value = helper(1);\n}\n";
    let uri = temp_file_uri("server_selection_range", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25)),
            method: Some("textDocument/selectionRange".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "positions": [
                    { "line": 1, "character": 23 }
                ]
            })),
        },
    );

    assert_eq!(response["id"], json!(25));
    let result = response["result"].as_array().unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0]["range"],
        json!({
            "start": { "line": 1, "character": 23 },
            "end": { "line": 1, "character": 24 }
        })
    );
    assert_eq!(
        result[0]["parent"]["range"],
        json!({
            "start": { "line": 1, "character": 22 },
            "end": { "line": 1, "character": 25 }
        })
    );
    assert_eq!(
        result[0]["parent"]["parent"]["parent"]["range"],
        json!({
            "start": { "line": 0, "character": 15 },
            "end": { "line": 2, "character": 1 }
        })
    );
}

#[test]
fn formatting_request_returns_text_edits_for_dirty_document() {
    let mut state = initialized_state();
    let disk_source = "fn main() void {\n}\n";
    let dirty_source = "fn main() void {  \n}\t";
    let uri = temp_file_uri("server_formatting", disk_source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, dirty_source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(26)),
            method: Some("textDocument/formatting".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "options": {
                    "tabSize": 4,
                    "insertSpaces": true
                }
            })),
        },
    );

    assert_eq!(response["id"], json!(26));
    assert_eq!(
        response["result"],
        json!([
            {
                "range": {
                    "start": { "line": 0, "character": 16 },
                    "end": { "line": 0, "character": 18 }
                },
                "newText": ""
            },
            {
                "range": {
                    "start": { "line": 1, "character": 1 },
                    "end": { "line": 1, "character": 2 }
                },
                "newText": "\n"
            }
        ])
    );
}

#[test]
fn range_formatting_request_filters_unrelated_edits() {
    let mut state = initialized_state();
    let source = "fn first() void {  \n}\t\nfn second() void {  \n}\t";
    let uri = temp_file_uri("server_range_formatting", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(27)),
            method: Some("textDocument/rangeFormatting".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 2, "character": 0 },
                    "end": { "line": 3, "character": 1 }
                },
                "options": {
                    "tabSize": 4,
                    "insertSpaces": true
                }
            })),
        },
    );

    assert_eq!(response["id"], json!(27));
    let edits = response["result"].as_array().unwrap();
    assert_eq!(edits.len(), 2);
    assert_eq!(
        edits[0]["range"]["start"],
        json!({ "line": 2, "character": 18 })
    );
    assert_eq!(
        edits[1]["range"]["start"],
        json!({ "line": 3, "character": 1 })
    );
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
fn definition_request_reports_analysis_errors() {
    let mut state = initialized_state();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper(); }\n";
    let uri = invalid_manifest_document_uri("server_definition_invalid_manifest", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(2501)),
            method: Some("textDocument/definition".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 24 }
            })),
        },
    );

    assert_eq!(response["id"], json!(2501));
    assert_eq!(response["error"]["code"], json!(-32600));
    let message = response["error"]["message"].as_str().unwrap();
    assert!(message.contains("definition analysis failed"), "{message}");
    assert!(message.contains("Craft.toml"), "{message}");
}

#[test]
fn declaration_request_returns_declaration_location() {
    let mut state = initialized_state();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper(); }\n";
    let uri = temp_file_uri("server_declaration", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(2502)),
            method: Some("textDocument/declaration".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 24 }
            })),
        },
    );

    assert_eq!(response["id"], json!(2502));
    assert_eq!(response["result"]["uri"], uri);
    assert_eq!(
        response["result"]["range"]["start"],
        json!({ "line": 0, "character": 3 })
    );
}

#[test]
fn declaration_request_reports_analysis_errors() {
    let mut state = initialized_state();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper(); }\n";
    let uri = invalid_manifest_document_uri("server_declaration_invalid_manifest", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(2503)),
            method: Some("textDocument/declaration".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 24 }
            })),
        },
    );

    assert_eq!(response["id"], json!(2503));
    assert_eq!(response["error"]["code"], json!(-32600));
    let message = response["error"]["message"].as_str().unwrap();
    assert!(message.contains("declaration analysis failed"), "{message}");
    assert!(message.contains("Craft.toml"), "{message}");
}

#[test]
fn implementation_request_returns_trait_method_implementations() {
    let mut state = initialized_state();
    let source = concat!(
        "trait Base { fn foo() i32; }\n",
        "impl &i32 : Base { pub fn foo() i32 { return self.*; } }\n",
        "fn main() i32 {\n",
        "    let value = 3i32;\n",
        "    let base = (value.& as &Base);\n",
        "    return base.foo();\n",
        "}\n",
    );
    let uri = temp_file_uri("server_implementation_trait_method", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(2504)),
            method: Some("textDocument/implementation".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 16 }
            })),
        },
    );

    assert_eq!(response["id"], json!(2504));
    let locations = response["result"].as_array().unwrap();
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0]["uri"], uri);
    assert_eq!(
        locations[0]["range"]["start"],
        json!({ "line": 1, "character": 26 })
    );
}

#[test]
fn implementation_request_returns_empty_for_plain_function() {
    let mut state = initialized_state();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper(); }\n";
    let uri = temp_file_uri("server_implementation_plain_function", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(2505)),
            method: Some("textDocument/implementation".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 24 }
            })),
        },
    );

    assert_eq!(response["id"], json!(2505));
    assert_eq!(response["result"], json!([]));
}

#[test]
fn implementation_request_reports_analysis_errors() {
    let mut state = initialized_state();
    let source = concat!(
        "trait Base { fn foo() i32; }\n",
        "impl &i32 : Base { pub fn foo() i32 { return self.*; } }\n",
        "fn main() i32 {\n",
        "    let value = 3i32;\n",
        "    let base = (value.& as &Base);\n",
        "    return base.foo();\n",
        "}\n",
    );
    let uri = invalid_manifest_document_uri("server_implementation_invalid_manifest", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(2506)),
            method: Some("textDocument/implementation".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 16 }
            })),
        },
    );

    assert_eq!(response["id"], json!(2506));
    assert_eq!(response["error"]["code"], json!(-32600));
    let message = response["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("implementation analysis failed"),
        "{message}"
    );
    assert!(message.contains("Craft.toml"), "{message}");
}

#[test]
fn call_hierarchy_requests_return_direct_calls() {
    let mut state = initialized_state();
    let source = concat!(
        "fn leaf() i32 { return 1; }\n",
        "fn helper() i32 { return leaf(); }\n",
        "fn main() i32 { return helper() + leaf(); }\n",
    );
    let uri = temp_file_uri("server_call_hierarchy", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let prepare = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25070)),
            method: Some("textDocument/prepareCallHierarchy".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 3 }
            })),
        },
    );

    assert_eq!(prepare["id"], json!(25070));
    let items = prepare["result"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "helper");
    assert_eq!(items[0]["uri"], uri);
    assert_eq!(
        items[0]["selectionRange"],
        json!({
            "start": { "line": 1, "character": 3 },
            "end": { "line": 1, "character": 9 }
        })
    );

    let outgoing = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25071)),
            method: Some("callHierarchy/outgoingCalls".to_string()),
            params: Some(json!({
                "item": items[0]
            })),
        },
    );
    assert_eq!(outgoing["id"], json!(25071));
    let outgoing_calls = outgoing["result"].as_array().unwrap();
    assert_eq!(outgoing_calls.len(), 1);
    assert_eq!(outgoing_calls[0]["to"]["name"], "leaf");
    assert_eq!(
        outgoing_calls[0]["fromRanges"],
        json!([
            {
                "start": { "line": 1, "character": 25 },
                "end": { "line": 1, "character": 29 }
            }
        ])
    );

    let incoming = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25072)),
            method: Some("callHierarchy/incomingCalls".to_string()),
            params: Some(json!({
                "item": items[0]
            })),
        },
    );
    assert_eq!(incoming["id"], json!(25072));
    let incoming_calls = incoming["result"].as_array().unwrap();
    assert_eq!(incoming_calls.len(), 1);
    assert_eq!(incoming_calls[0]["from"]["name"], "main");
    assert_eq!(
        incoming_calls[0]["fromRanges"],
        json!([
            {
                "start": { "line": 2, "character": 23 },
                "end": { "line": 2, "character": 29 }
            }
        ])
    );
}

#[test]
fn call_hierarchy_expands_dynamic_dispatch_targets() {
    let mut state = initialized_state();
    let source = concat!(
        "trait Base { fn foo() i32; }\n",
        "impl &i32 : Base { pub fn foo() i32 { return self.*; } }\n",
        "fn main() i32 {\n",
        "    let value = 3i32;\n",
        "    let base = (value.& as &Base);\n",
        "    return base.foo();\n",
        "}\n",
    );
    let uri = temp_file_uri("server_call_hierarchy_dynamic_dispatch_targets", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let prepare = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25074)),
            method: Some("textDocument/prepareCallHierarchy".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 2, "character": 3 }
            })),
        },
    );

    assert_eq!(prepare["id"], json!(25074));
    let items = prepare["result"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "main");

    let outgoing = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25075)),
            method: Some("callHierarchy/outgoingCalls".to_string()),
            params: Some(json!({
                "item": items[0]
            })),
        },
    );

    assert_eq!(outgoing["id"], json!(25075));
    let outgoing_calls = outgoing["result"].as_array().unwrap();
    assert_eq!(outgoing_calls.len(), 1);
    assert_eq!(outgoing_calls[0]["to"]["name"], "foo");
    assert_eq!(
        outgoing_calls[0]["to"]["selectionRange"],
        json!({
            "start": { "line": 1, "character": 26 },
            "end": { "line": 1, "character": 29 }
        })
    );
    assert_eq!(
        outgoing_calls[0]["fromRanges"],
        json!([
            {
                "start": { "line": 5, "character": 11 },
                "end": { "line": 5, "character": 19 }
            }
        ])
    );

    let incoming = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25076)),
            method: Some("callHierarchy/incomingCalls".to_string()),
            params: Some(json!({
                "item": outgoing_calls[0]["to"]
            })),
        },
    );

    assert_eq!(incoming["id"], json!(25076));
    let incoming_calls = incoming["result"].as_array().unwrap();
    assert_eq!(incoming_calls.len(), 1);
    assert_eq!(incoming_calls[0]["from"]["name"], "main");
    assert_eq!(
        incoming_calls[0]["fromRanges"],
        json!([
            {
                "start": { "line": 5, "character": 11 },
                "end": { "line": 5, "character": 19 }
            }
        ])
    );
}

#[test]
fn call_hierarchy_excludes_unresolved_indirect_calls() {
    let mut state = initialized_state();
    let source = concat!(
        "fn leaf() i32 { return 1; }\n",
        "fn apply(cb: &fn() i32) i32 { return cb(); }\n",
        "fn main() i32 { return apply(leaf); }\n",
    );
    let uri = temp_file_uri("server_call_hierarchy_indirect_call", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let prepare = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25077)),
            method: Some("textDocument/prepareCallHierarchy".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 3 }
            })),
        },
    );

    assert_eq!(prepare["id"], json!(25077));
    let items = prepare["result"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "apply");

    let outgoing = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25078)),
            method: Some("callHierarchy/outgoingCalls".to_string()),
            params: Some(json!({
                "item": items[0]
            })),
        },
    );

    assert_eq!(outgoing["id"], json!(25078));
    assert_eq!(outgoing["result"], json!([]));
}

#[test]
fn call_hierarchy_request_reports_analysis_errors() {
    let mut state = initialized_state();
    let source = "fn leaf() i32 { return 1; }\nfn main() i32 { return leaf(); }\n";
    let uri = invalid_manifest_document_uri("server_call_hierarchy_invalid_manifest", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25073)),
            method: Some("textDocument/prepareCallHierarchy".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 3 }
            })),
        },
    );

    assert_eq!(response["id"], json!(25073));
    assert_eq!(response["error"]["code"], json!(-32600));
    let message = response["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("call hierarchy analysis failed"),
        "{message}"
    );
    assert!(message.contains("Craft.toml"), "{message}");
}

#[test]
fn type_definition_request_returns_type_symbol_definition() {
    let mut state = initialized_state();
    let source = "struct Point { x: i32 }\nfn main(point: Point) i32 { return point.x; }\n";
    let uri = temp_file_uri("server_type_definition_type_symbol", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(2507)),
            method: Some("textDocument/typeDefinition".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 16 }
            })),
        },
    );

    assert_eq!(response["id"], json!(2507));
    assert_eq!(response["result"]["uri"], uri);
    assert_eq!(
        response["result"]["range"]["start"],
        json!({ "line": 0, "character": 7 })
    );
}

#[test]
fn type_definition_request_returns_null_for_value_symbol() {
    let mut state = initialized_state();
    let source = "struct Point { x: i32 }\nfn main(point: Point) i32 { return point.x; }\n";
    let uri = temp_file_uri("server_type_definition_value_symbol", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(2508)),
            method: Some("textDocument/typeDefinition".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 36 }
            })),
        },
    );

    assert_eq!(response["id"], json!(2508));
    assert_eq!(response["result"], Value::Null);
}

#[test]
fn type_definition_request_reports_analysis_errors() {
    let mut state = initialized_state();
    let source = "struct Point { x: i32 }\nfn main(point: Point) i32 { return point.x; }\n";
    let uri = invalid_manifest_document_uri("server_type_definition_invalid_manifest", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(2509)),
            method: Some("textDocument/typeDefinition".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 16 }
            })),
        },
    );

    assert_eq!(response["id"], json!(2509));
    assert_eq!(response["error"]["code"], json!(-32600));
    let message = response["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("type definition analysis failed"),
        "{message}"
    );
    assert!(message.contains("Craft.toml"), "{message}");
}

#[test]
fn did_open_reports_analysis_errors_as_diagnostics() {
    let mut state = initialized_state();
    let source = "fn helper() i32 { return 1; }\n";
    let uri = invalid_manifest_document_uri("server_open_invalid_manifest", source);

    let messages = dispatch_messages(&mut state, did_open_message(&uri, source, 1));

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["method"], "textDocument/publishDiagnostics");
    assert_eq!(messages[0]["params"]["uri"], uri);
    let diagnostics = messages[0]["params"]["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1);
    let message = diagnostics[0]["message"].as_str().unwrap();
    assert!(message.contains("analysis failed"), "{message}");
    assert!(message.contains("Craft.toml"), "{message}");
}

#[test]
fn semantic_request_failures_are_error_responses() {
    let cases = [
        (
            "textDocument/references",
            json!({
                "position": { "line": 1, "character": 24 },
                "context": { "includeDeclaration": true }
            }),
            "references analysis failed",
        ),
        (
            "textDocument/hover",
            json!({
                "position": { "line": 1, "character": 24 }
            }),
            "hover analysis failed",
        ),
        (
            "textDocument/signatureHelp",
            json!({
                "position": { "line": 1, "character": 31 }
            }),
            "signature help analysis failed",
        ),
        (
            "textDocument/prepareRename",
            json!({
                "position": { "line": 1, "character": 24 }
            }),
            "prepareRename analysis failed",
        ),
        (
            "textDocument/inlayHint",
            json!({
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 3, "character": 0 }
                }
            }),
            "inlay hint analysis failed",
        ),
        (
            "textDocument/completion",
            json!({
                "position": { "line": 1, "character": 24 }
            }),
            "failed to resolve Craft project for LSP analysis",
        ),
        (
            "textDocument/documentSymbol",
            json!({}),
            "failed to resolve Craft project for LSP analysis",
        ),
        (
            "textDocument/documentLink",
            json!({}),
            "failed to resolve Craft project for LSP analysis",
        ),
        (
            "textDocument/prepareCallHierarchy",
            json!({
                "position": { "line": 1, "character": 3 }
            }),
            "failed to resolve Craft project for LSP analysis",
        ),
        (
            "textDocument/semanticTokens/full",
            json!({}),
            "failed to resolve Craft project for LSP analysis",
        ),
        (
            "textDocument/semanticTokens/range",
            json!({
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 2, "character": 0 }
                }
            }),
            "failed to resolve Craft project for LSP analysis",
        ),
        (
            "textDocument/codeAction",
            json!({
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 3, "character": 0 }
                },
                "context": {
                    "diagnostics": [],
                    "only": ["quickfix"]
                }
            }),
            "failed to resolve Craft project for LSP analysis",
        ),
    ];

    for (index, (method, extra_params, expected)) in cases.into_iter().enumerate() {
        let mut state = initialized_state();
        let source = "fn helper(a: i32) i32 { return a; }\nfn main() i32 { return helper(1); }\n";
        let uri = invalid_manifest_document_uri(&format!("server_semantic_error_{index}"), source);
        let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
        let mut params = extra_params;
        params["textDocument"] = json!({ "uri": uri });

        let response = dispatch_single_response(
            &mut state,
            IncomingMessage {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: Some(json!(2600 + index)),
                method: Some(method.to_string()),
                params: Some(params),
            },
        );

        assert_eq!(response["id"], json!(2600 + index));
        assert_eq!(response["error"]["code"], json!(-32600), "{method}");
        let message = response["error"]["message"].as_str().unwrap();
        assert!(
            message.contains(expected),
            "{method}: expected `{expected}` in `{message}`"
        );
        assert!(message.contains("Craft.toml"), "{method}: {message}");
    }
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
fn references_request_reports_workspace_progress_and_package_uses() {
    let root = unique_temp_dir("server_workspace_references_progress");
    let dep_dir = root.join("dep/src");
    let app_dir = root.join("app/src");
    fs::create_dir_all(&dep_dir).unwrap();
    fs::create_dir_all(&app_dir).unwrap();
    fs::write(
        root.join("Craft.toml"),
        "[workspace]\nname = \"workspace\"\nmembers = [\"dep\", \"app\"]\n",
    )
    .unwrap();
    fs::write(
        root.join("dep/Craft.toml"),
        format!(
            "\
[package]
name = \"dep\"
version = \"0.1.0\"
kern = \"{}\"\n
[lib]
root = \"src/lib.kn\"
",
            env!("CARGO_PKG_VERSION")
        ),
    )
    .unwrap();
    let dep_source = "pub fn helper() i32 { return 1; }\n";
    fs::write(dep_dir.join("lib.kn"), dep_source).unwrap();
    fs::write(
        root.join("app/Craft.toml"),
        format!(
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"{}\"\n
[lib]
root = \"src/lib.kn\"

[dependencies]
dep = {{ path = \"../dep\" }}
",
            env!("CARGO_PKG_VERSION")
        ),
    )
    .unwrap();
    let app_source = "use dep.helper;\npub fn run() i32 { return helper(); }\n";
    fs::write(app_dir.join("lib.kn"), app_source).unwrap();
    let uri = format!("file://{}", dep_dir.join("lib.kn").to_string_lossy());

    let mut state = initialized_state();
    state.work_done_progress = true;
    let _ = dispatch_messages(&mut state, did_open_message(&uri, dep_source, 1));
    let messages = dispatch_messages(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(261)),
            method: Some("textDocument/references".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 8 },
                "context": { "includeDeclaration": true },
                "workDoneToken": "refs-token"
            })),
        },
    );

    assert_eq!(messages.len(), 3, "{messages:#?}");
    assert_eq!(messages[0]["method"], "$/progress");
    assert_eq!(messages[0]["params"]["token"], "refs-token");
    assert_eq!(messages[0]["params"]["value"]["kind"], "begin");
    let response = messages
        .iter()
        .find(|message| message["id"] == json!(261))
        .unwrap();
    let locations = response["result"].as_array().unwrap();
    assert_eq!(locations.len(), 3, "{locations:#?}");
    assert!(
        locations.iter().any(|location| location["uri"]
            .as_str()
            .unwrap()
            .ends_with("/dep/src/lib.kn")),
        "{locations:#?}"
    );
    let app_locations = locations
        .iter()
        .filter(|location| {
            location["uri"]
                .as_str()
                .unwrap()
                .ends_with("/app/src/lib.kn")
        })
        .collect::<Vec<_>>();
    assert_eq!(app_locations.len(), 2, "{locations:#?}");
    assert!(messages.iter().any(|message| {
        message["method"] == "$/progress"
            && message["params"]["token"] == "refs-token"
            && message["params"]["value"]["kind"] == "end"
    }));
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
        "    let value = 2i32;\n",
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
fn completion_item_resolve_returns_eager_item_without_analysis() {
    let mut state = initialized_state();
    assert_eq!(state.analysis.last_analysis_tier(), None);

    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(28)),
            method: Some("completionItem/resolve".to_string()),
            params: Some(json!({
                "label": "helper",
                "kind": 3,
                "detail": "fn helper() void",
                "insertText": "helper()",
                "insertTextFormat": 1
            })),
        },
    );

    assert_eq!(response["id"], json!(28));
    assert_eq!(response["result"]["label"], "helper");
    assert_eq!(response["result"]["kind"], 3);
    assert_eq!(response["result"]["detail"], "fn helper() void");
    assert_eq!(response["result"]["insertText"], "helper()");
    assert_eq!(response["result"]["insertTextFormat"], 1);
    assert_eq!(state.analysis.last_analysis_tier(), None);
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
fn code_lens_request_returns_craft_target_commands() {
    let root = unique_temp_dir("server_code_lens_targets");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        format!(
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"{}\"\n
[lib]
root = \"src/lib.kn\"
",
            env!("CARGO_PKG_VERSION")
        ),
    )
    .unwrap();
    let source = "pub fn value() i32 { return 1; }\n";
    fs::write(root.join("src/lib.kn"), source).unwrap();
    let uri = format!("file://{}", root.join("src/lib.kn").to_string_lossy());

    let mut state = initialized_state();
    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(205)),
            method: Some("textDocument/codeLens".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri }
            })),
        },
    );

    assert_eq!(response["id"], json!(205));
    let lenses = response["result"].as_array().unwrap();
    assert_eq!(lenses.len(), 1, "{lenses:#?}");
    assert_eq!(lenses[0]["command"]["title"], "Build lib");
    assert_eq!(lenses[0]["command"]["command"], "kern.craft.buildPackage");
    assert_eq!(lenses[0]["command"]["arguments"][0]["targetKind"], "lib");
    assert!(
        lenses[0]["command"]["arguments"][0]["manifestPath"]
            .as_str()
            .unwrap()
            .ends_with("/Craft.toml"),
        "{}",
        lenses[0]["command"]["arguments"][0]
    );
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
fn semantic_tokens_range_request_filters_token_data() {
    let mut state = initialized_state();
    let source = concat!(
        "struct Point { x: i32 }\n",
        "fn helper(point: Point) i32 {\n",
        "    return point.x;\n",
        "}\n",
    );
    let uri = temp_file_uri("server_semantic_tokens_range", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(32)),
            method: Some("textDocument/semanticTokens/range".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 1, "character": 0 },
                    "end": { "line": 2, "character": 0 }
                }
            })),
        },
    );

    assert_eq!(response["id"], json!(32));
    let data = response["result"]["data"].as_array().unwrap();
    assert!(!data.is_empty());
    assert_eq!(data.len() % 5, 0);
    let decoded = decode_semantic_token_positions(data);
    assert!(decoded.iter().all(|(line, _)| *line == 1));
    assert_eq!(decoded[0], (1, 0));
}

#[test]
fn document_link_request_returns_external_module_targets() {
    let root = unique_temp_dir("server_document_link");
    fs::write(root.join("mod.kn"), "mod child;\nmod inline {}\n").unwrap();
    fs::write(root.join("child.kn"), "pub fn child() void {}\n").unwrap();
    let source = fs::read_to_string(root.join("mod.kn")).unwrap();
    let uri = format!("file://{}", root.join("mod.kn").to_string_lossy());

    let mut state = initialized_state();
    let _ = dispatch_messages(&mut state, did_open_message(&uri, &source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(330)),
            method: Some("textDocument/documentLink".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri }
            })),
        },
    );

    assert_eq!(response["id"], json!(330));
    let links = response["result"].as_array().unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(
        links[0]["range"],
        json!({
            "start": { "line": 0, "character": 4 },
            "end": { "line": 0, "character": 9 }
        })
    );
    assert!(
        links[0]["target"].as_str().unwrap().ends_with("/child.kn"),
        "{}",
        links[0]["target"]
    );
}

#[test]
fn document_link_request_returns_import_targets() {
    let root = unique_temp_dir("server_document_link_import");
    let dep_dir = root.join("dep/src");
    let app_dir = root.join("app/src");
    fs::create_dir_all(&dep_dir).unwrap();
    fs::create_dir_all(&app_dir).unwrap();
    fs::write(
        root.join("Craft.toml"),
        "[workspace]\nname = \"workspace\"\nmembers = [\"dep\", \"app\"]\n",
    )
    .unwrap();
    fs::write(
        root.join("dep/Craft.toml"),
        format!(
            "\
[package]
name = \"dep\"
version = \"0.1.0\"
kern = \"{}\"\n
[lib]
root = \"src/lib.kn\"
",
            env!("CARGO_PKG_VERSION")
        ),
    )
    .unwrap();
    fs::write(dep_dir.join("lib.kn"), "pub mod child;\n").unwrap();
    fs::write(
        dep_dir.join("child.kn"),
        "pub fn helper() i32 { return 1; }\n",
    )
    .unwrap();
    fs::write(
        root.join("app/Craft.toml"),
        format!(
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"{}\"\n
[lib]
root = \"src/lib.kn\"

[dependencies]
dep = {{ path = \"../dep\" }}
",
            env!("CARGO_PKG_VERSION")
        ),
    )
    .unwrap();
    fs::write(
        app_dir.join("lib.kn"),
        "use dep.child;\nfn main() i32 { return child.helper(); }\n",
    )
    .unwrap();
    let source = fs::read_to_string(app_dir.join("lib.kn")).unwrap();
    let uri = format!("file://{}", app_dir.join("lib.kn").to_string_lossy());

    let mut state = initialized_state();
    let _ = dispatch_messages(&mut state, did_open_message(&uri, &source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(331)),
            method: Some("textDocument/documentLink".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri }
            })),
        },
    );

    assert_eq!(response["id"], json!(331));
    let links = response["result"].as_array().unwrap();
    assert_eq!(links.len(), 1, "{links:#?}");
    assert_eq!(
        links[0]["range"],
        json!({
            "start": { "line": 0, "character": 8 },
            "end": { "line": 0, "character": 13 }
        })
    );
    assert!(
        links[0]["target"].as_str().unwrap().ends_with("/child.kn"),
        "{}",
        links[0]["target"]
    );
}

#[test]
fn document_link_request_returns_manifest_dependency_targets() {
    let root = unique_temp_dir("server_document_link_manifest_dependency");
    fs::create_dir_all(root.join("dep/src")).unwrap();
    fs::create_dir_all(root.join("app/src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        "[workspace]\nname = \"workspace\"\nmembers = [\"dep\", \"app\"]\n",
    )
    .unwrap();
    fs::write(
        root.join("dep/Craft.toml"),
        format!(
            "\
[package]
name = \"dep\"
version = \"0.1.0\"
kern = \"{}\"\n
[lib]
root = \"src/lib.kn\"
",
            env!("CARGO_PKG_VERSION")
        ),
    )
    .unwrap();
    fs::write(root.join("dep/src/lib.kn"), "pub fn dep() void {}\n").unwrap();
    let manifest_source = format!(
        "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"{}\"\n
[lib]
root = \"src/lib.kn\"

[dependencies]
dep = {{ path = \"../dep\" }}
",
        env!("CARGO_PKG_VERSION")
    );
    fs::write(root.join("app/Craft.toml"), &manifest_source).unwrap();
    fs::write(root.join("app/src/lib.kn"), "pub fn app() void {}\n").unwrap();
    let uri = format!("file://{}", root.join("app/Craft.toml").to_string_lossy());

    let mut state = initialized_state();
    let _ = dispatch_messages(&mut state, did_open_message(&uri, &manifest_source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(332)),
            method: Some("textDocument/documentLink".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri }
            })),
        },
    );

    assert_eq!(response["id"], json!(332));
    let links = response["result"].as_array().unwrap();
    assert_eq!(links.len(), 1, "{links:#?}");
    assert_eq!(
        links[0]["range"],
        json!({
            "start": { "line": 9, "character": 0 },
            "end": { "line": 9, "character": 3 }
        })
    );
    assert!(
        links[0]["target"]
            .as_str()
            .unwrap()
            .ends_with("/dep/Craft.toml"),
        "{}",
        links[0]["target"]
    );
}

#[test]
fn inlay_hint_request_returns_type_hints() {
    let mut state = initialized_state();
    let source = concat!(
        "fn helper() usize { return 1usize; }\n",
        "fn main() i32 {\n",
        "    let value = helper();\n",
        "    return 0;\n",
        "}\n",
    );
    let uri = temp_file_uri("server_inlay_hint", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(33)),
            method: Some("textDocument/inlayHint".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 10, "character": 0 }
                }
            })),
        },
    );

    assert_eq!(response["id"], json!(33));
    let hints = response["result"].as_array().unwrap();
    assert!(hints.iter().any(|hint| hint["label"] == ": usize"));
    assert!(
        hints
            .iter()
            .any(|hint| hint["position"] == json!({ "line": 2, "character": 13 }))
    );
}

fn decode_semantic_token_positions(data: &[Value]) -> Vec<(u64, u64)> {
    let mut decoded = Vec::new();
    let mut line = 0;
    let mut start = 0;

    for chunk in data.chunks_exact(5) {
        let delta_line = chunk[0].as_u64().unwrap();
        line += delta_line;
        if delta_line == 0 {
            start += chunk[1].as_u64().unwrap();
        } else {
            start = chunk[1].as_u64().unwrap();
        }
        decoded.push((line, start));
    }

    decoded
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
    let clean = "fn main() i32 {\n    let value = 1i32;\n    return value;\n}\n";
    let dirty = "fn main() i32 {\n    let value = 1i32\n    return value;\n}\n";
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
        "    let value = 2i32;\n",
        "    return helper(1, value);\n",
        "}\n",
    );
    let dirty = concat!(
        "fn helper(first: i32, second: i32) i32 {\n",
        "    return first + second;\n",
        "}\n",
        "fn main() i32 {\n",
        "    let value = 2i32\n",
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
        "    let value = 1i32;\n",
        "    return helper() + helper();\n",
        "}\n",
    );
    let dirty = concat!(
        "fn helper() i32 { return 1; }\n",
        "fn main() i32 {\n",
        "    let value = 1i32\n",
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
