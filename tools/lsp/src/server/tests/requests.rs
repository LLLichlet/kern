use super::*;

fn apply_single_lsp_text_edit(source: &str, edit: &Value) -> String {
    let start = lsp_position_to_byte_offset(source, &edit["range"]["start"]);
    let end = lsp_position_to_byte_offset(source, &edit["range"]["end"]);
    let mut result = String::new();
    result.push_str(&source[..start]);
    result.push_str(edit["newText"].as_str().unwrap());
    result.push_str(&source[end..]);
    result
}

fn lsp_position_to_byte_offset(source: &str, position: &Value) -> usize {
    let target_line = position["line"].as_u64().unwrap() as usize;
    let target_character = position["character"].as_u64().unwrap() as usize;
    let mut offset = 0;
    for (line_index, line) in source.split_inclusive('\n').enumerate() {
        if line_index == target_line {
            return offset + target_character;
        }
        offset += line.len();
    }
    offset + target_character
}

fn write_workspace_symbol_project(root: &std::path::Path, package: &str, symbol: &str) {
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        root.join("Craft.toml"),
        format!(
            r#"
[package]
name = "{package}"
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
        format!("struct {symbol} {{ value: i32 }}\nfn other() void {{}}\n"),
    )
    .unwrap();
}

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
fn code_action_resolve_materializes_deferred_edit() {
    let mut state = initialized_state();
    let source = "fn main() i32 {\n    return match (1) {\n        1 => 1,\n    };\n}\n";
    let uri = temp_file_uri("server_code_action_resolve", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let code_action_response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(230)),
            method: Some("textDocument/codeAction".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 1, "character": 4 },
                    "end": { "line": 3, "character": 5 }
                },
                "context": {
                    "diagnostics": [],
                    "only": ["quickfix"]
                }
            })),
        },
    );
    let code_action = code_action_response["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == json!("Add `_ => @unreachable()` arm"))
        .unwrap()
        .clone();
    assert!(code_action.get("edit").is_none());
    assert_eq!(code_action["data"]["uri"], json!(uri));
    assert_eq!(code_action["data"]["version"], json!(1));
    assert_eq!(code_action["data"]["fixId"], json!("add-match-catch-all"));
    assert_eq!(
        code_action["data"]["diagnosticCode"],
        json!("nonexhaustive-match")
    );

    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(231)),
            method: Some("codeAction/resolve".to_string()),
            params: Some(code_action),
        },
    );

    assert_eq!(response["id"], json!(231));
    assert_eq!(response["result"]["title"], "Add `_ => @unreachable()` arm");
    assert_eq!(
        response["result"]["edit"]["changes"][&uri][0]["range"]["start"],
        json!({ "line": 3, "character": 4 })
    );
    assert_eq!(
        response["result"]["edit"]["changes"][&uri][0]["newText"],
        "        _ => @unreachable(),\n"
    );
    assert!(response["result"].get("data").is_none());
}

#[test]
fn code_action_resolve_materializes_import_insertion() {
    let mut state = initialized_state();
    let root = unique_temp_dir("server_code_action_import_resolve");
    fs::write(
        root.join("mod.kn"),
        "mod helper;\nfn main() i32 { return answer(); }\n",
    )
    .unwrap();
    fs::write(
        root.join("helper.kn"),
        "pub fn answer() i32 { return 1; }\n",
    )
    .unwrap();
    let uri = file_path_to_uri_for_test(&root.join("mod.kn"));
    let source = fs::read_to_string(root.join("mod.kn")).unwrap();

    let _ = dispatch_messages(&mut state, did_open_message(&uri, &source, 1));
    let code_action_response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(232)),
            method: Some("textDocument/codeAction".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 1, "character": 25 },
                    "end": { "line": 1, "character": 31 }
                },
                "context": {
                    "diagnostics": [],
                    "only": ["quickfix"]
                }
            })),
        },
    );
    let code_action = code_action_response["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == json!("Import `/helper.answer`"))
        .unwrap()
        .clone();
    assert!(code_action.get("edit").is_none());
    assert_eq!(code_action["data"]["uri"], json!(uri));
    assert_eq!(code_action["data"]["version"], json!(1));
    assert_eq!(code_action["data"]["fixId"], json!("insert-import"));
    assert_eq!(
        code_action["data"]["diagnosticCode"],
        json!("unresolved-identifier")
    );

    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(233)),
            method: Some("codeAction/resolve".to_string()),
            params: Some(code_action),
        },
    );

    assert_eq!(response["id"], json!(233));
    assert_eq!(response["result"]["title"], "Import `/helper.answer`");
    assert_eq!(
        response["result"]["edit"]["changes"][&uri][0]["range"]["start"],
        json!({ "line": 0, "character": 0 })
    );
    assert_eq!(
        response["result"]["edit"]["changes"][&uri][0]["newText"],
        "use /helper.answer;\n"
    );
    assert!(response["result"].get("data").is_none());
}

#[test]
fn code_action_resolve_materializes_type_import_insertion() {
    let mut state = initialized_state();
    let root = unique_temp_dir("server_code_action_type_import_resolve");
    fs::write(
        root.join("mod.kn"),
        "mod model;\nfn make() Widget { return 0; }\n",
    )
    .unwrap();
    fs::write(root.join("model.kn"), "pub type Widget = i32;\n").unwrap();
    let uri = file_path_to_uri_for_test(&root.join("mod.kn"));
    let source = fs::read_to_string(root.join("mod.kn")).unwrap();

    let _ = dispatch_messages(&mut state, did_open_message(&uri, &source, 1));
    let code_action_response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(234)),
            method: Some("textDocument/codeAction".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 1, "character": 10 },
                    "end": { "line": 1, "character": 16 }
                },
                "context": {
                    "diagnostics": [],
                    "only": ["quickfix"]
                }
            })),
        },
    );
    let code_action = code_action_response["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == json!("Import `/model.Widget`"))
        .unwrap()
        .clone();
    assert!(code_action.get("edit").is_none());
    assert_eq!(code_action["data"]["uri"], json!(uri));
    assert_eq!(code_action["data"]["version"], json!(1));
    assert_eq!(code_action["data"]["fixId"], json!("insert-import"));
    assert_eq!(
        code_action["data"]["diagnosticCode"],
        json!("unresolved-type")
    );

    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(235)),
            method: Some("codeAction/resolve".to_string()),
            params: Some(code_action),
        },
    );

    assert_eq!(response["id"], json!(235));
    assert_eq!(response["result"]["title"], "Import `/model.Widget`");
    assert_eq!(
        response["result"]["edit"]["changes"][&uri][0]["range"]["start"],
        json!({ "line": 0, "character": 0 })
    );
    assert_eq!(
        response["result"]["edit"]["changes"][&uri][0]["newText"],
        "use /model.Widget;\n"
    );
    assert!(response["result"].get("data").is_none());
}

#[test]
fn code_action_resolve_materializes_let_mut_fix() {
    let mut state = initialized_state();
    let source = "fn main() void {\n    let value = 1;\n    value = 2;\n}\n";
    let uri = temp_file_uri("server_code_action_resolve_let_mut", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let code_action_response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(235)),
            method: Some("textDocument/codeAction".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 2, "character": 4 },
                    "end": { "line": 2, "character": 13 }
                },
                "context": {
                    "diagnostics": [],
                    "only": ["quickfix"]
                }
            })),
        },
    );
    let code_action = code_action_response["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == json!("Change to `let mut`"))
        .unwrap()
        .clone();
    assert!(code_action.get("edit").is_none());
    assert_eq!(code_action["data"]["fixId"], json!("change-let-mut"));
    assert_eq!(
        code_action["data"]["diagnosticCode"],
        json!("requires-let-mut")
    );

    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(236)),
            method: Some("codeAction/resolve".to_string()),
            params: Some(code_action),
        },
    );

    assert_eq!(response["id"], json!(236));
    assert_eq!(response["result"]["title"], "Change to `let mut`");
    assert_eq!(
        response["result"]["edit"]["changes"][&uri][0]["range"]["start"],
        json!({ "line": 1, "character": 8 })
    );
    assert_eq!(
        response["result"]["edit"]["changes"][&uri][0]["newText"],
        "mut "
    );
    assert!(response["result"].get("data").is_none());
}

#[test]
fn code_action_resolve_materializes_trait_impl_method_stub() {
    let mut state = initialized_state();
    let source = concat!(
        "trait Render { fn render(value: i32) i32; }\n",
        "struct Widget {}\n",
        "impl Widget: Render {}\n",
    );
    let uri = temp_file_uri("server_code_action_trait_impl_method_stub", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let code_action_response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(239)),
            method: Some("textDocument/codeAction".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 2, "character": 0 },
                    "end": { "line": 2, "character": 21 }
                },
                "context": {
                    "diagnostics": [],
                    "only": ["quickfix"]
                }
            })),
        },
    );
    let code_action = code_action_response["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == json!("Add `render` method stub"))
        .unwrap()
        .clone();
    assert!(code_action.get("edit").is_none());
    assert_eq!(
        code_action["data"]["fixId"],
        json!("add-trait-impl-method-stub")
    );
    assert_eq!(
        code_action["data"]["diagnosticCode"],
        json!("missing-trait-impl-method")
    );

    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(240)),
            method: Some("codeAction/resolve".to_string()),
            params: Some(code_action),
        },
    );

    assert_eq!(response["id"], json!(240));
    assert_eq!(response["result"]["title"], "Add `render` method stub");
    assert_eq!(
        response["result"]["edit"]["changes"][&uri][0]["range"]["start"],
        json!({ "line": 2, "character": 21 })
    );
    assert_eq!(
        response["result"]["edit"]["changes"][&uri][0]["newText"],
        "\n    fn render(value: i32) i32 {\n        @unreachable();\n    }\n"
    );
    assert!(response["result"].get("data").is_none());
}

#[test]
fn code_action_resolve_edit_applies_to_document_text() {
    let mut state = initialized_state();
    let source = "fn main() void {\n    let value = 1;\n    value = 2;\n}\n";
    let uri = temp_file_uri("server_code_action_apply_resolved", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let code_action_response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(237)),
            method: Some("textDocument/codeAction".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 2, "character": 4 },
                    "end": { "line": 2, "character": 13 }
                },
                "context": {
                    "diagnostics": [],
                    "only": ["quickfix"]
                }
            })),
        },
    );
    let code_action = code_action_response["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == json!("Change to `let mut`"))
        .unwrap()
        .clone();
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(238)),
            method: Some("codeAction/resolve".to_string()),
            params: Some(code_action),
        },
    );
    let edit = &response["result"]["edit"]["changes"][&uri][0];
    let applied = apply_single_lsp_text_edit(source, edit);

    assert_eq!(
        applied,
        "fn main() void {\n    let mut value = 1;\n    value = 2;\n}\n"
    );
}

#[test]
fn code_action_resolve_does_not_apply_stale_deferred_edit() {
    let mut state = initialized_state();
    let source = "fn main() i32 {\n    return match (1) {\n        1 => 1,\n    };\n}\n";
    let changed = "fn main() i32 {\n    return 1;\n}\n";
    let uri = temp_file_uri("server_code_action_resolve_stale", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let code_action_response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(232)),
            method: Some("textDocument/codeAction".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 1, "character": 4 },
                    "end": { "line": 3, "character": 5 }
                },
                "context": {
                    "diagnostics": [],
                    "only": ["quickfix"]
                }
            })),
        },
    );
    let code_action = code_action_response["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["title"] == json!("Add `_ => @unreachable()` arm"))
        .unwrap()
        .clone();
    assert!(code_action.get("edit").is_none());
    assert!(dispatch_messages(&mut state, did_change_message(&uri, changed, 2)).is_empty());

    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(233)),
            method: Some("codeAction/resolve".to_string()),
            params: Some(code_action),
        },
    );

    assert_eq!(response["id"], json!(233));
    assert_eq!(response["result"]["title"], "Add `_ => @unreachable()` arm");
    assert!(response["result"].get("edit").is_none());
    assert!(response["result"].get("data").is_none());
}

#[test]
fn code_action_resolve_strips_invalid_data_without_analysis() {
    let mut state = initialized_state();
    assert_eq!(state.analysis.last_analysis_tier(), None);

    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(234)),
            method: Some("codeAction/resolve".to_string()),
            params: Some(json!({
                "title": "Insert `;`",
                "kind": "quickfix",
                "data": {
                    "fixId": "unknown"
                }
            })),
        },
    );

    assert_eq!(response["id"], json!(234));
    assert_eq!(response["result"]["title"], "Insert `;`");
    assert_eq!(response["result"]["kind"], "quickfix");
    assert!(response["result"].get("edit").is_none());
    assert!(response["result"].get("data").is_none());
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
fn workspace_symbol_request_reports_work_done_progress() {
    let mut state = initialized_state();
    state.work_done_progress = true;
    let source = "struct ProgressNeedle { value: i32 }\nfn helper() void {}\n";
    let uri = temp_file_uri("server_workspace_symbol_progress", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let messages = dispatch_messages(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(243)),
            method: Some("workspace/symbol".to_string()),
            params: Some(json!({
                "query": "progress",
                "workDoneToken": "workspace-symbol-token"
            })),
        },
    );

    assert_eq!(messages.len(), 3, "{messages:#?}");
    assert_eq!(messages[0]["method"], "$/progress");
    assert_eq!(messages[0]["params"]["token"], "workspace-symbol-token");
    assert_eq!(messages[0]["params"]["value"]["kind"], "begin");
    assert_eq!(
        messages[0]["params"]["value"]["title"],
        "Kern workspace symbols"
    );
    let response = messages
        .iter()
        .find(|message| message["id"] == json!(243))
        .unwrap();
    assert_eq!(response["result"].as_array().unwrap().len(), 1);
    let progress_messages = messages
        .iter()
        .filter(|message| message["method"] == "$/progress")
        .collect::<Vec<_>>();
    assert_eq!(progress_messages.len(), 2, "{messages:#?}");
    assert_eq!(
        progress_messages[1]["params"]["token"],
        "workspace-symbol-token"
    );
    assert_eq!(progress_messages[1]["params"]["value"]["kind"], "end");
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
    state.workspace_roots = vec![root.clone()];
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
    assert_uri_path_ends_with(
        response["result"][0]["location"]["uri"].as_str().unwrap(),
        "src/lib.kn",
    );
}

#[test]
fn workspace_symbol_request_uses_all_workspace_roots() {
    let root_a = unique_temp_dir("server_workspace_symbol_multi_a");
    let root_b = unique_temp_dir("server_workspace_symbol_multi_b");
    write_workspace_symbol_project(&root_a, "demo_a", "WorkspaceAlphaNeedle");
    write_workspace_symbol_project(&root_b, "demo_b", "WorkspaceBetaNeedle");

    let mut state = initialized_state();
    state.workspace_roots = vec![root_a, root_b];
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(243)),
            method: Some("workspace/symbol".to_string()),
            params: Some(json!({
                "query": "workspace"
            })),
        },
    );

    let names = response["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|symbol| symbol["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"WorkspaceAlphaNeedle".to_string()));
    assert!(names.contains(&"WorkspaceBetaNeedle".to_string()));
}

#[test]
fn workspace_folder_change_updates_roots_and_refreshes_index() {
    let root_a = unique_temp_dir("server_workspace_folder_change_a");
    let root_b = unique_temp_dir("server_workspace_folder_change_b");
    write_workspace_symbol_project(&root_a, "demo_a", "WorkspaceAlphaNeedle");
    write_workspace_symbol_project(&root_b, "demo_b", "WorkspaceBetaNeedle");

    let mut state = initialized_state();
    state.workspace_roots = vec![root_a.clone()];
    let _ = dispatch_messages(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: None,
            method: Some("workspace/didChangeWorkspaceFolders".to_string()),
            params: Some(json!({
                "event": {
                    "added": [
                        { "uri": file_path_to_uri_for_test(&root_b), "name": "b" }
                    ],
                    "removed": [
                        { "uri": file_path_to_uri_for_test(&root_a), "name": "a" }
                    ]
                }
            })),
        },
    );

    assert_eq!(state.workspace_roots, vec![root_b.clone()]);
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(244)),
            method: Some("workspace/symbol".to_string()),
            params: Some(json!({
                "query": "workspace"
            })),
        },
    );

    let names = response["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|symbol| symbol["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["WorkspaceBetaNeedle".to_string()]);
    assert_eq!(state.analysis.cached_workspace_symbol_index_count(), 1);
}

#[test]
fn workspace_symbol_request_reuses_refreshed_workspace_index() {
    let root = unique_temp_dir("server_workspace_symbol_refreshed_index");
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
        "struct RefreshedNeedle { value: i32 }\nfn other() void {}\n",
    )
    .unwrap();

    let mut state = initialized_state();
    state.trace = super::super::lifecycle::TraceValue::Verbose;
    state.workspace_roots = vec![root];
    let _ = dispatch_messages(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: None,
            method: Some("workspace/didChangeWatchedFiles".to_string()),
            params: Some(json!({
                "changes": []
            })),
        },
    );
    assert_eq!(state.analysis.cached_workspace_symbol_index_count(), 1);

    let messages = dispatch_messages(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(242)),
            method: Some("workspace/symbol".to_string()),
            params: Some(json!({
                "query": "refreshed"
            })),
        },
    );
    let response = messages
        .iter()
        .find(|message| message.get("id") == Some(&json!(242)))
        .expect("expected workspace symbol response");

    assert_eq!(response["id"], json!(242));
    assert_eq!(response["result"].as_array().unwrap().len(), 1);
    assert_eq!(response["result"][0]["name"], "RefreshedNeedle");
    assert_eq!(state.analysis.cached_workspace_symbol_index_count(), 1);
    let trace = messages
        .iter()
        .find(|message| {
            message["method"] == "$/logTrace"
                && message["params"]["message"] == "analysis tier selected"
        })
        .expect("expected workspace symbol trace");
    let verbose = trace["params"]["verbose"].as_str().unwrap();
    assert!(verbose.contains("request_id=242"), "{verbose}");
    assert!(verbose.contains("method=workspace/symbol"), "{verbose}");
    assert!(verbose.contains("snapshot_generation="), "{verbose}");
    assert!(
        verbose.contains("workspace-symbol-index:hit=1,miss=0,store=0"),
        "{verbose}"
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
        "fn apply(cb: &fn() i32) i32 { return cb(); }\n",
        "fn main() i32 { return 0; }\n",
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
                "position": { "line": 0, "character": 3 }
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
fn call_hierarchy_keeps_known_targets_when_parameter_arguments_are_partial() {
    let mut state = initialized_state();
    let source = concat!(
        "fn known() i32 { return 1; }\n",
        "fn apply(cb: &fn() i32) i32 { return cb(); }\n",
        "fn main(flag: bool, incoming: &fn() i32) i32 {\n",
        "    if (flag) {\n",
        "        return apply(known);\n",
        "    }\n",
        "    return apply(incoming);\n",
        "}\n",
    );
    let uri = temp_file_uri("server_call_hierarchy_partial_parameter_argument", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let prepare_apply = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25110)),
            method: Some("textDocument/prepareCallHierarchy".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 3 }
            })),
        },
    );

    assert_eq!(prepare_apply["id"], json!(25110));
    let apply_items = prepare_apply["result"].as_array().unwrap();
    assert_eq!(apply_items.len(), 1);
    assert_eq!(apply_items[0]["name"], "apply");

    let outgoing = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25111)),
            method: Some("callHierarchy/outgoingCalls".to_string()),
            params: Some(json!({
                "item": apply_items[0]
            })),
        },
    );

    assert_eq!(outgoing["id"], json!(25111));
    let outgoing_calls = outgoing["result"].as_array().unwrap();
    assert_eq!(outgoing_calls.len(), 1);
    assert_eq!(outgoing_calls[0]["to"]["name"], "known");
    assert_eq!(
        outgoing_calls[0]["fromRanges"],
        json!([
            {
                "start": { "line": 1, "character": 37 },
                "end": { "line": 1, "character": 39 }
            }
        ])
    );
}

#[test]
fn call_hierarchy_preserves_partial_parameter_targets_through_forwarding() {
    let mut state = initialized_state();
    let source = concat!(
        "fn known() i32 { return 1; }\n",
        "fn apply(cb: &fn() i32) i32 { return cb(); }\n",
        "fn forward(cb: &fn() i32) i32 { return apply(cb); }\n",
        "fn main(flag: bool, incoming: &fn() i32) i32 {\n",
        "    if (flag) {\n",
        "        return forward(known);\n",
        "    }\n",
        "    return forward(incoming);\n",
        "}\n",
    );
    let uri = temp_file_uri(
        "server_call_hierarchy_partial_forwarded_parameter_argument",
        source,
    );

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let prepare_apply = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25112)),
            method: Some("textDocument/prepareCallHierarchy".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 3 }
            })),
        },
    );

    assert_eq!(prepare_apply["id"], json!(25112));
    let apply_items = prepare_apply["result"].as_array().unwrap();
    assert_eq!(apply_items.len(), 1);
    assert_eq!(apply_items[0]["name"], "apply");

    let outgoing = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25113)),
            method: Some("callHierarchy/outgoingCalls".to_string()),
            params: Some(json!({
                "item": apply_items[0]
            })),
        },
    );

    assert_eq!(outgoing["id"], json!(25113));
    let outgoing_calls = outgoing["result"].as_array().unwrap();
    assert_eq!(outgoing_calls.len(), 1);
    assert_eq!(outgoing_calls[0]["to"]["name"], "known");
    assert_eq!(
        outgoing_calls[0]["fromRanges"],
        json!([
            {
                "start": { "line": 1, "character": 37 },
                "end": { "line": 1, "character": 39 }
            }
        ])
    );
}

#[test]
fn call_hierarchy_propagates_arguments_through_indirect_callees() {
    let mut state = initialized_state();
    let source = concat!(
        "fn leaf() i32 { return 1; }\n",
        "fn apply(cb: &fn() i32) i32 { return cb(); }\n",
        "fn forward(cb: &fn() i32) i32 { return apply(cb); }\n",
        "fn main() i32 {\n",
        "    let f = forward;\n",
        "    return f(leaf);\n",
        "}\n",
    );
    let uri = temp_file_uri("server_call_hierarchy_indirect_callee_argument", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let prepare_apply = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25114)),
            method: Some("textDocument/prepareCallHierarchy".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 3 }
            })),
        },
    );

    assert_eq!(prepare_apply["id"], json!(25114));
    let apply_items = prepare_apply["result"].as_array().unwrap();
    assert_eq!(apply_items.len(), 1);
    assert_eq!(apply_items[0]["name"], "apply");

    let outgoing = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25115)),
            method: Some("callHierarchy/outgoingCalls".to_string()),
            params: Some(json!({
                "item": apply_items[0]
            })),
        },
    );

    assert_eq!(outgoing["id"], json!(25115));
    let outgoing_calls = outgoing["result"].as_array().unwrap();
    assert_eq!(outgoing_calls.len(), 1);
    assert_eq!(outgoing_calls[0]["to"]["name"], "leaf");
    assert_eq!(
        outgoing_calls[0]["fromRanges"],
        json!([
            {
                "start": { "line": 1, "character": 37 },
                "end": { "line": 1, "character": 39 }
            }
        ])
    );
}

#[test]
fn call_hierarchy_propagates_arguments_through_parameter_callees() {
    let mut state = initialized_state();
    let source = concat!(
        "fn leaf() i32 { return 1; }\n",
        "fn apply(cb: &fn() i32) i32 { return cb(); }\n",
        "fn forward(cb: &fn() i32) i32 { return apply(cb); }\n",
        "fn route(run: &fn(&fn() i32) i32, cb: &fn() i32) i32 {\n",
        "    return run(cb);\n",
        "}\n",
        "fn main() i32 {\n",
        "    return route(forward, leaf);\n",
        "}\n",
    );
    let uri = temp_file_uri("server_call_hierarchy_parameter_callee_argument", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let prepare_apply = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25116)),
            method: Some("textDocument/prepareCallHierarchy".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 3 }
            })),
        },
    );

    assert_eq!(prepare_apply["id"], json!(25116));
    let apply_items = prepare_apply["result"].as_array().unwrap();
    assert_eq!(apply_items.len(), 1);
    assert_eq!(apply_items[0]["name"], "apply");

    let outgoing = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25117)),
            method: Some("callHierarchy/outgoingCalls".to_string()),
            params: Some(json!({
                "item": apply_items[0]
            })),
        },
    );

    assert_eq!(outgoing["id"], json!(25117));
    let outgoing_calls = outgoing["result"].as_array().unwrap();
    assert_eq!(outgoing_calls.len(), 1);
    assert_eq!(outgoing_calls[0]["to"]["name"], "leaf");
    assert_eq!(
        outgoing_calls[0]["fromRanges"],
        json!([
            {
                "start": { "line": 1, "character": 37 },
                "end": { "line": 1, "character": 39 }
            }
        ])
    );
}

#[test]
fn call_hierarchy_expands_local_function_value_targets() {
    let mut state = initialized_state();
    let source = concat!(
        "fn leaf() i32 { return 1; }\n",
        "fn main() i32 {\n",
        "    let cb = leaf;\n",
        "    return cb();\n",
        "}\n",
    );
    let uri = temp_file_uri("server_call_hierarchy_local_indirect_call", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let prepare_main = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25079)),
            method: Some("textDocument/prepareCallHierarchy".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 3 }
            })),
        },
    );

    assert_eq!(prepare_main["id"], json!(25079));
    let main_items = prepare_main["result"].as_array().unwrap();
    assert_eq!(main_items.len(), 1);
    assert_eq!(main_items[0]["name"], "main");

    let outgoing = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25080)),
            method: Some("callHierarchy/outgoingCalls".to_string()),
            params: Some(json!({
                "item": main_items[0]
            })),
        },
    );

    assert_eq!(outgoing["id"], json!(25080));
    let outgoing_calls = outgoing["result"].as_array().unwrap();
    assert_eq!(outgoing_calls.len(), 1);
    assert_eq!(outgoing_calls[0]["to"]["name"], "leaf");
    assert_eq!(
        outgoing_calls[0]["fromRanges"],
        json!([
            {
                "start": { "line": 3, "character": 11 },
                "end": { "line": 3, "character": 13 }
            }
        ])
    );

    let prepare_leaf = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25081)),
            method: Some("textDocument/prepareCallHierarchy".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 3 }
            })),
        },
    );

    assert_eq!(prepare_leaf["id"], json!(25081));
    let leaf_items = prepare_leaf["result"].as_array().unwrap();
    assert_eq!(leaf_items.len(), 1);
    assert_eq!(leaf_items[0]["name"], "leaf");

    let incoming = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25082)),
            method: Some("callHierarchy/incomingCalls".to_string()),
            params: Some(json!({
                "item": leaf_items[0]
            })),
        },
    );

    assert_eq!(incoming["id"], json!(25082));
    let incoming_calls = incoming["result"].as_array().unwrap();
    assert_eq!(incoming_calls.len(), 1);
    assert_eq!(incoming_calls[0]["from"]["name"], "main");
    assert_eq!(
        incoming_calls[0]["fromRanges"],
        json!([
            {
                "start": { "line": 3, "character": 11 },
                "end": { "line": 3, "character": 13 }
            }
        ])
    );
}

#[test]
fn call_hierarchy_expands_forwarded_local_function_value_targets() {
    let mut state = initialized_state();
    let source = concat!(
        "fn leaf() i32 { return 1; }\n",
        "fn main() i32 {\n",
        "    let first = leaf;\n",
        "    let second = first;\n",
        "    return second();\n",
        "}\n",
    );
    let uri = temp_file_uri(
        "server_call_hierarchy_forwarded_local_indirect_call",
        source,
    );

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let prepare_main = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25083)),
            method: Some("textDocument/prepareCallHierarchy".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 3 }
            })),
        },
    );

    assert_eq!(prepare_main["id"], json!(25083));
    let main_items = prepare_main["result"].as_array().unwrap();
    assert_eq!(main_items.len(), 1);
    assert_eq!(main_items[0]["name"], "main");

    let outgoing = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25084)),
            method: Some("callHierarchy/outgoingCalls".to_string()),
            params: Some(json!({
                "item": main_items[0]
            })),
        },
    );

    assert_eq!(outgoing["id"], json!(25084));
    let outgoing_calls = outgoing["result"].as_array().unwrap();
    assert_eq!(outgoing_calls.len(), 1);
    assert_eq!(outgoing_calls[0]["to"]["name"], "leaf");
    assert_eq!(
        outgoing_calls[0]["fromRanges"],
        json!([
            {
                "start": { "line": 4, "character": 11 },
                "end": { "line": 4, "character": 17 }
            }
        ])
    );

    let prepare_leaf = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25085)),
            method: Some("textDocument/prepareCallHierarchy".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 3 }
            })),
        },
    );

    assert_eq!(prepare_leaf["id"], json!(25085));
    let leaf_items = prepare_leaf["result"].as_array().unwrap();
    assert_eq!(leaf_items.len(), 1);
    assert_eq!(leaf_items[0]["name"], "leaf");

    let incoming = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25086)),
            method: Some("callHierarchy/incomingCalls".to_string()),
            params: Some(json!({
                "item": leaf_items[0]
            })),
        },
    );

    assert_eq!(incoming["id"], json!(25086));
    let incoming_calls = incoming["result"].as_array().unwrap();
    assert_eq!(incoming_calls.len(), 1);
    assert_eq!(incoming_calls[0]["from"]["name"], "main");
    assert_eq!(
        incoming_calls[0]["fromRanges"],
        json!([
            {
                "start": { "line": 4, "character": 11 },
                "end": { "line": 4, "character": 17 }
            }
        ])
    );
}

#[test]
fn call_hierarchy_expands_multi_source_function_value_targets() {
    let mut state = initialized_state();
    let source = concat!(
        "fn first() i32 { return 1; }\n",
        "fn second() i32 { return 2; }\n",
        "fn main(flag: bool) i32 {\n",
        "    let mut cb = first;\n",
        "    if (flag) {\n",
        "        cb = second;\n",
        "    }\n",
        "    return cb();\n",
        "}\n",
    );
    let uri = temp_file_uri("server_call_hierarchy_multi_source_indirect_call", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let prepare_main = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25096)),
            method: Some("textDocument/prepareCallHierarchy".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 2, "character": 3 }
            })),
        },
    );

    assert_eq!(prepare_main["id"], json!(25096));
    let main_items = prepare_main["result"].as_array().unwrap();
    assert_eq!(main_items.len(), 1);
    assert_eq!(main_items[0]["name"], "main");

    let outgoing = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25097)),
            method: Some("callHierarchy/outgoingCalls".to_string()),
            params: Some(json!({
                "item": main_items[0]
            })),
        },
    );

    assert_eq!(outgoing["id"], json!(25097));
    let outgoing_calls = outgoing["result"].as_array().unwrap();
    let mut outgoing_names = outgoing_calls
        .iter()
        .map(|call| call["to"]["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    outgoing_names.sort();
    assert_eq!(outgoing_names, vec!["first", "second"]);
    assert!(outgoing_calls.iter().all(|call| {
        call["fromRanges"]
            == json!([
                {
                    "start": { "line": 7, "character": 11 },
                    "end": { "line": 7, "character": 13 }
                }
            ])
    }));

    let prepare_second = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25098)),
            method: Some("textDocument/prepareCallHierarchy".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 3 }
            })),
        },
    );

    assert_eq!(prepare_second["id"], json!(25098));
    let second_items = prepare_second["result"].as_array().unwrap();
    assert_eq!(second_items.len(), 1);
    assert_eq!(second_items[0]["name"], "second");

    let incoming = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25099)),
            method: Some("callHierarchy/incomingCalls".to_string()),
            params: Some(json!({
                "item": second_items[0]
            })),
        },
    );

    assert_eq!(incoming["id"], json!(25099));
    let incoming_calls = incoming["result"].as_array().unwrap();
    assert_eq!(incoming_calls.len(), 1);
    assert_eq!(incoming_calls[0]["from"]["name"], "main");
    assert_eq!(
        incoming_calls[0]["fromRanges"],
        json!([
            {
                "start": { "line": 7, "character": 11 },
                "end": { "line": 7, "character": 13 }
            }
        ])
    );
}

#[test]
fn call_hierarchy_expands_function_value_parameter_targets() {
    let mut state = initialized_state();
    let source = concat!(
        "fn first() i32 { return 1; }\n",
        "fn second() i32 { return 2; }\n",
        "fn apply(cb: &fn() i32) i32 { return cb(); }\n",
        "fn main() i32 { return apply(first) + apply(second); }\n",
    );
    let uri = temp_file_uri("server_call_hierarchy_parameter_indirect_call", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let prepare_apply = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25087)),
            method: Some("textDocument/prepareCallHierarchy".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 2, "character": 3 }
            })),
        },
    );

    assert_eq!(prepare_apply["id"], json!(25087));
    let apply_items = prepare_apply["result"].as_array().unwrap();
    assert_eq!(apply_items.len(), 1);
    assert_eq!(apply_items[0]["name"], "apply");

    let outgoing = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25088)),
            method: Some("callHierarchy/outgoingCalls".to_string()),
            params: Some(json!({
                "item": apply_items[0]
            })),
        },
    );

    assert_eq!(outgoing["id"], json!(25088));
    let outgoing_calls = outgoing["result"].as_array().unwrap();
    let mut outgoing_names = outgoing_calls
        .iter()
        .map(|call| call["to"]["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    outgoing_names.sort();
    assert_eq!(outgoing_names, vec!["first", "second"]);
    assert!(outgoing_calls.iter().all(|call| {
        call["fromRanges"]
            == json!([
                {
                    "start": { "line": 2, "character": 37 },
                    "end": { "line": 2, "character": 39 }
                }
            ])
    }));

    let prepare_first = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25089)),
            method: Some("textDocument/prepareCallHierarchy".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 3 }
            })),
        },
    );

    assert_eq!(prepare_first["id"], json!(25089));
    let first_items = prepare_first["result"].as_array().unwrap();
    assert_eq!(first_items.len(), 1);
    assert_eq!(first_items[0]["name"], "first");

    let incoming = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25090)),
            method: Some("callHierarchy/incomingCalls".to_string()),
            params: Some(json!({
                "item": first_items[0]
            })),
        },
    );

    assert_eq!(incoming["id"], json!(25090));
    let incoming_calls = incoming["result"].as_array().unwrap();
    assert_eq!(incoming_calls.len(), 1);
    assert_eq!(incoming_calls[0]["from"]["name"], "apply");
    assert_eq!(
        incoming_calls[0]["fromRanges"],
        json!([
            {
                "start": { "line": 2, "character": 37 },
                "end": { "line": 2, "character": 39 }
            }
        ])
    );
}

#[test]
fn call_hierarchy_expands_closure_object_targets() {
    let mut state = initialized_state();
    let source = concat!(
        "fn leaf() i32 { return 1; }\n",
        "fn apply(cb: &Fn() i32) i32 { return cb(); }\n",
        "fn main() i32 {\n",
        "    let base = 2i32;\n",
        "    let local = [base]() i32 { return base + leaf(); };\n",
        "    let erased = (local.& as &Fn() i32);\n",
        "    return erased() + apply(erased);\n",
        "}\n",
    );
    let uri = temp_file_uri("server_call_hierarchy_closure_object_call", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let prepare_apply = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25091)),
            method: Some("textDocument/prepareCallHierarchy".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 3 }
            })),
        },
    );

    assert_eq!(prepare_apply["id"], json!(25091));
    let apply_items = prepare_apply["result"].as_array().unwrap();
    assert_eq!(apply_items.len(), 1);
    assert_eq!(apply_items[0]["name"], "apply");

    let outgoing = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25092)),
            method: Some("callHierarchy/outgoingCalls".to_string()),
            params: Some(json!({
                "item": apply_items[0]
            })),
        },
    );

    assert_eq!(outgoing["id"], json!(25092));
    let outgoing_calls = outgoing["result"].as_array().unwrap();
    assert_eq!(outgoing_calls.len(), 1);
    assert_eq!(outgoing_calls[0]["to"]["name"], "local");
    assert_eq!(
        outgoing_calls[0]["fromRanges"],
        json!([
            {
                "start": { "line": 1, "character": 37 },
                "end": { "line": 1, "character": 39 }
            }
        ])
    );

    let prepare_local = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25093)),
            method: Some("textDocument/prepareCallHierarchy".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 4, "character": 9 }
            })),
        },
    );

    assert_eq!(prepare_local["id"], json!(25093));
    let local_items = prepare_local["result"].as_array().unwrap();
    assert_eq!(local_items.len(), 1);
    assert_eq!(local_items[0]["name"], "local");

    let incoming = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25094)),
            method: Some("callHierarchy/incomingCalls".to_string()),
            params: Some(json!({
                "item": local_items[0]
            })),
        },
    );

    assert_eq!(incoming["id"], json!(25094));
    let incoming_calls = incoming["result"].as_array().unwrap();
    assert_eq!(incoming_calls.len(), 2);
    let mut incoming_by_name = incoming_calls
        .iter()
        .map(|call| (call["from"]["name"].as_str().unwrap(), &call["fromRanges"]))
        .collect::<Vec<_>>();
    incoming_by_name.sort_by_key(|(name, _)| *name);
    assert_eq!(incoming_by_name[0].0, "apply");
    assert_eq!(
        incoming_by_name[0].1,
        &json!([
            {
                "start": { "line": 1, "character": 37 },
                "end": { "line": 1, "character": 39 }
            }
        ])
    );
    assert_eq!(incoming_by_name[1].0, "main");
    assert_eq!(
        incoming_by_name[1].1,
        &json!([
            {
                "start": { "line": 6, "character": 11 },
                "end": { "line": 6, "character": 17 }
            }
        ])
    );

    let local_outgoing = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(25095)),
            method: Some("callHierarchy/outgoingCalls".to_string()),
            params: Some(json!({
                "item": local_items[0]
            })),
        },
    );

    assert_eq!(local_outgoing["id"], json!(25095));
    let local_outgoing_calls = local_outgoing["result"].as_array().unwrap();
    assert_eq!(local_outgoing_calls.len(), 1);
    assert_eq!(local_outgoing_calls[0]["to"]["name"], "leaf");
    assert_eq!(
        local_outgoing_calls[0]["fromRanges"],
        json!([
            {
                "start": { "line": 4, "character": 45 },
                "end": { "line": 4, "character": 49 }
            }
        ])
    );
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
    let uri = file_path_to_uri_for_test(&dep_dir.join("lib.kn"));

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
        locations
            .iter()
            .any(
                |location| crate::analysis::uri_to_file_path(location["uri"].as_str().unwrap())
                    .is_some_and(|path| path.ends_with("dep/src/lib.kn"))
            ),
        "{locations:#?}"
    );
    let app_locations = locations
        .iter()
        .filter(|location| {
            crate::analysis::uri_to_file_path(location["uri"].as_str().unwrap())
                .is_some_and(|path| path.ends_with("app/src/lib.kn"))
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
fn references_request_includes_cross_root_workspace_uses() {
    let dep_root = unique_temp_dir("server_references_cross_root_dep");
    let app_root = unique_temp_dir("server_references_cross_root_app");
    fs::create_dir_all(dep_root.join("src")).unwrap();
    fs::create_dir_all(app_root.join("src")).unwrap();
    fs::write(
        dep_root.join("Craft.toml"),
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
    fs::write(dep_root.join("src/lib.kn"), dep_source).unwrap();
    fs::write(
        app_root.join("Craft.toml"),
        format!(
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"{}\"\n
[lib]
root = \"src/lib.kn\"

[dependencies]
dep = {{ path = \"{}\" }}
",
            env!("CARGO_PKG_VERSION"),
            dep_root.to_string_lossy().replace('\\', "\\\\")
        ),
    )
    .unwrap();
    let app_source = "use dep.helper;\npub fn run() i32 { return helper(); }\n";
    fs::write(app_root.join("src/lib.kn"), app_source).unwrap();
    let dep_uri = file_path_to_uri_for_test(&dep_root.join("src/lib.kn"));

    let mut state = initialized_state();
    state.workspace_roots = vec![dep_root, app_root];
    let _ = dispatch_messages(&mut state, did_open_message(&dep_uri, dep_source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(42)),
            method: Some("textDocument/references".to_string()),
            params: Some(json!({
                "textDocument": { "uri": dep_uri },
                "position": { "line": 0, "character": 7 },
                "context": { "includeDeclaration": true }
            })),
        },
    );

    assert_eq!(response["id"], json!(42));
    let locations = response["result"].as_array().unwrap();
    assert_eq!(locations.len(), 3, "{locations:#?}");
    let app_locations = locations
        .iter()
        .filter(|location| {
            location["uri"].as_str().is_some_and(|uri| {
                crate::analysis::uri_to_file_path(uri).is_some_and(|path| {
                    path.ends_with("src/lib.kn")
                        && path.components().any(|component| {
                            component
                                .as_os_str()
                                .to_string_lossy()
                                .contains("cross_root_app")
                        })
                })
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(app_locations.len(), 2, "{locations:#?}");
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
    assert!(contents.contains("fn helper(x: i32) i32"));
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
fn completion_item_resolve_adds_documentation_from_resolve_data() {
    let mut state = initialized_state();
    let source = "/// Helper docs.\nfn helper() i32 { return 1; }\nfn main() i32 {\n    hel\n}\n";
    let uri = temp_file_uri("server_completion_resolve", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let completion_response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(27)),
            method: Some("textDocument/completion".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 3, "character": 7 }
            })),
        },
    );
    let completion_item = completion_response["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == json!("helper"))
        .unwrap()
        .clone();
    assert!(completion_item.get("documentation").is_none());
    assert!(completion_item["data"].get("documentation").is_none());

    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(28)),
            method: Some("completionItem/resolve".to_string()),
            params: Some(completion_item),
        },
    );

    assert_eq!(response["id"], json!(28));
    assert_eq!(response["result"]["label"], "helper");
    assert_eq!(response["result"]["documentation"]["kind"], "markdown");
    let documentation = response["result"]["documentation"]["value"]
        .as_str()
        .unwrap();
    assert!(documentation.contains("```kern"), "{documentation}");
    assert!(documentation.contains("Helper docs."), "{documentation}");
    assert!(response["result"].get("data").is_none());
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
fn code_lens_request_returns_deferred_craft_target_commands() {
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
    let uri = file_path_to_uri_for_test(&root.join("src/lib.kn"));

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
    assert!(lenses[0].get("command").is_none(), "{lenses:#?}");
    assert_eq!(lenses[0]["data"]["title"], "Build lib");
    assert_eq!(lenses[0]["data"]["command"], "kern.craft.buildPackage");
    assert_eq!(lenses[0]["data"]["arguments"][0]["targetKind"], "lib");
    assert!(
        PathBuf::from(
            lenses[0]["data"]["arguments"][0]["manifestPath"]
                .as_str()
                .unwrap()
        )
        .ends_with("Craft.toml"),
        "{}",
        lenses[0]["data"]["arguments"][0]
    );
}

#[test]
fn code_lens_resolve_materializes_craft_target_command() {
    let root = unique_temp_dir("server_code_lens_resolve");
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
    let uri = file_path_to_uri_for_test(&root.join("src/lib.kn"));

    let mut state = initialized_state();
    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let lens_response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(206)),
            method: Some("textDocument/codeLens".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri }
            })),
        },
    );
    let lens = lens_response["result"].as_array().unwrap()[0].clone();

    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(207)),
            method: Some("codeLens/resolve".to_string()),
            params: Some(lens),
        },
    );

    assert_eq!(response["id"], json!(207));
    assert!(response["result"].get("data").is_none());
    assert_eq!(response["result"]["command"]["title"], "Build lib");
    assert_eq!(
        response["result"]["command"]["command"],
        "kern.craft.buildPackage"
    );
    assert_eq!(
        response["result"]["command"]["arguments"][0]["targetKind"],
        "lib"
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
    assert!(response["result"]["resultId"].as_str().is_some());
}

#[test]
fn semantic_tokens_delta_request_returns_edits() {
    let mut state = initialized_state();
    let source = concat!(
        "struct Point { x: i32 }\n",
        "fn helper(point: Point) i32 {\n",
        "    return point.x;\n",
        "}\n",
    );
    let uri = temp_file_uri("server_semantic_tokens_delta", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let full = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(310)),
            method: Some("textDocument/semanticTokens/full".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri }
            })),
        },
    );
    let previous_result_id = full["result"]["resultId"].as_str().unwrap().to_string();

    let delta = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(311)),
            method: Some("textDocument/semanticTokens/full/delta".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "previousResultId": previous_result_id
            })),
        },
    );

    assert_eq!(delta["id"], json!(311));
    assert!(delta["result"]["resultId"].as_str().is_some());
    let edits = delta["result"]["edits"].as_array().unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0]["deleteCount"], json!(0));
    assert!(edits[0].get("data").is_none());
}

#[test]
fn semantic_tokens_delta_with_unknown_result_id_returns_full_tokens() {
    let mut state = initialized_state();
    let source = concat!(
        "struct Point { x: i32 }\n",
        "fn helper(point: Point) i32 {\n",
        "    return point.x;\n",
        "}\n",
    );
    let uri = temp_file_uri("server_semantic_tokens_delta_unknown", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(312)),
            method: Some("textDocument/semanticTokens/full/delta".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "previousResultId": "missing"
            })),
        },
    );

    assert_eq!(response["id"], json!(312));
    assert!(response["result"]["resultId"].as_str().is_some());
    let data = response["result"]["data"].as_array().unwrap();
    assert!(!data.is_empty());
    assert!(response["result"].get("edits").is_none());
}

#[test]
fn semantic_tokens_delta_after_document_change_returns_full_tokens() {
    let mut state = initialized_state();
    let source = concat!(
        "struct Point { x: i32 }\n",
        "fn helper(point: Point) i32 {\n",
        "    return point.x;\n",
        "}\n",
    );
    let changed = concat!(
        "struct Point { x: i32 }\n",
        "fn helper(point: Point) i32 {\n",
        "    let value = point.x;\n",
        "    return value;\n",
        "}\n",
    );
    let uri = temp_file_uri("server_semantic_tokens_delta_changed", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let full = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(313)),
            method: Some("textDocument/semanticTokens/full".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri }
            })),
        },
    );
    let previous_result_id = full["result"]["resultId"].as_str().unwrap().to_string();

    assert!(dispatch_messages(&mut state, did_change_message(&uri, changed, 2)).is_empty());
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(314)),
            method: Some("textDocument/semanticTokens/full/delta".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "previousResultId": previous_result_id
            })),
        },
    );

    assert_eq!(response["id"], json!(314));
    assert!(response["result"]["resultId"].as_str().is_some());
    let data = response["result"]["data"].as_array().unwrap();
    assert!(!data.is_empty());
    assert!(response["result"].get("edits").is_none());
}

#[test]
fn verbose_trace_reports_semantic_tokens_cache_hit() {
    let mut state = initialized_state();
    state.trace = super::super::lifecycle::TraceValue::Verbose;
    let source = concat!(
        "struct Point { x: i32 }\n",
        "fn helper(point: Point) i32 {\n",
        "    return point.x;\n",
        "}\n",
    );
    let uri = temp_file_uri("server_semantic_tokens_cache_trace", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let _ = dispatch_messages(
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
    let messages = dispatch_messages(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(3102)),
            method: Some("textDocument/semanticTokens/full".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri }
            })),
        },
    );

    let trace = messages
        .iter()
        .find(|message| {
            message["method"] == "$/logTrace"
                && message["params"]["message"] == "analysis tier selected"
        })
        .expect("expected semantic token trace");
    let verbose = trace["params"]["verbose"].as_str().unwrap();
    assert!(verbose.contains("request_id=3102"), "{verbose}");
    assert!(
        verbose.contains("method=textDocument/semanticTokens/full"),
        "{verbose}"
    );
    assert!(verbose.contains("snapshot_generation="), "{verbose}");
    assert!(
        verbose.contains("semantic-tokens:hit=1,miss=0,store=0"),
        "{verbose}"
    );
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
    assert_eq!(decoded[0], (1, 3));
}

#[test]
fn document_link_request_returns_external_module_targets() {
    let root = unique_temp_dir("server_document_link");
    fs::write(root.join("mod.kn"), "mod child;\nmod inline {}\n").unwrap();
    fs::write(root.join("child.kn"), "pub fn child() void {}\n").unwrap();
    let source = fs::read_to_string(root.join("mod.kn")).unwrap();
    let uri = file_path_to_uri_for_test(&root.join("mod.kn"));

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
    assert!(links[0].get("target").is_none(), "{links:#?}");
    assert_uri_path_ends_with(links[0]["data"]["target"].as_str().unwrap(), "child.kn");
}

#[test]
fn document_link_request_returns_deferred_import_targets() {
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
    let uri = file_path_to_uri_for_test(&app_dir.join("lib.kn"));

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
    assert!(links[0].get("target").is_none(), "{links:#?}");
    assert_uri_path_ends_with(links[0]["data"]["target"].as_str().unwrap(), "child.kn");
}

#[test]
fn document_link_resolve_materializes_import_target() {
    let root = unique_temp_dir("server_document_link_resolve_import");
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
    let uri = file_path_to_uri_for_test(&app_dir.join("lib.kn"));

    let mut state = initialized_state();
    let _ = dispatch_messages(&mut state, did_open_message(&uri, &source, 1));
    let link_response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(333)),
            method: Some("textDocument/documentLink".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri }
            })),
        },
    );
    let link = link_response["result"].as_array().unwrap()[0].clone();

    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(334)),
            method: Some("documentLink/resolve".to_string()),
            params: Some(link),
        },
    );

    assert_eq!(response["id"], json!(334));
    assert!(response["result"].get("data").is_none());
    assert_uri_path_ends_with(response["result"]["target"].as_str().unwrap(), "child.kn");
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
    let uri = file_path_to_uri_for_test(&root.join("app/Craft.toml"));

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
    assert!(links[0].get("target").is_none(), "{links:#?}");
    assert_uri_path_ends_with(
        links[0]["data"]["target"].as_str().unwrap(),
        "dep/Craft.toml",
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
fn verbose_trace_reports_dirty_semantic_tokens_as_semantic() {
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
    assert!(
        verbose.contains("tier=dirty-semantic") || verbose.contains("tier=clean-semantic"),
        "{verbose}"
    );
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
    state.request_budget_policy.interactive_ms = u128::MAX;
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
    state.request_budget_policy.interactive_ms = u128::MAX;
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
