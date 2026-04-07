use super::*;

#[test]
fn completion_request_returns_visible_items() {
    let mut state = initialized_state();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 {\n    hel\n}\n";
    let uri = temp_file_uri("server_completion", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(30)),
            method: Some("textDocument/completion".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 2, "character": 7 }
            })),
        },
    );

    assert_eq!(response["id"], json!(30));
    let items = response["result"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let helper = items
        .iter()
        .find(|item| item["label"] == json!("helper"))
        .unwrap();
    assert_eq!(helper["insertText"], json!("helper()$0"));
    assert_eq!(helper["insertTextFormat"], json!(2));
}

#[test]
fn completion_request_returns_kern_keyword_snippets() {
    let mut state = initialized_state();
    let source = "fn main() i32 {\n    le\n}\n";
    let uri = temp_file_uri("server_completion_keyword", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(35)),
            method: Some("textDocument/completion".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 6 }
            })),
        },
    );

    assert_eq!(response["id"], json!(35));
    let let_item = response["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == json!("let"))
        .unwrap();
    assert_eq!(let_item["insertText"], json!("let ${1:name} = ${0};"));
    assert_eq!(let_item["insertTextFormat"], json!(2));
}

#[test]
fn completion_request_returns_top_level_extern_snippet() {
    let mut state = initialized_state();
    let source = "ex\n";
    let uri = temp_file_uri("server_completion_extern", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(36)),
            method: Some("textDocument/completion".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 2 }
            })),
        },
    );

    assert_eq!(response["id"], json!(36));
    let extern_item = response["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == json!("extern"))
        .unwrap();
    assert_eq!(
        extern_item["insertText"],
        json!("extern fn ${1:name}(${2:args}) ${3:i32} {\n    $0\n}")
    );
    assert_eq!(extern_item["insertTextFormat"], json!(2));
}

#[test]
fn completion_request_returns_top_level_type_snippet() {
    let mut state = initialized_state();
    let source = "ty\n";
    let uri = temp_file_uri("server_completion_type_keyword", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(38)),
            method: Some("textDocument/completion".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 2 }
            })),
        },
    );

    assert_eq!(response["id"], json!(38));
    let type_item = response["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == json!("type"))
        .unwrap();
    assert_eq!(type_item["insertText"], json!("type ${1:Name} = ${0};"));
    assert_eq!(type_item["insertTextFormat"], json!(2));
}

#[test]
fn completion_request_returns_type_context_struct_snippet() {
    let mut state = initialized_state();
    let source = "type Packet = st\n";
    let uri = temp_file_uri("server_completion_struct_keyword", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(39)),
            method: Some("textDocument/completion".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 16 }
            })),
        },
    );

    assert_eq!(response["id"], json!(39));
    let struct_item = response["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == json!("struct"))
        .unwrap();
    assert_eq!(struct_item["insertText"], json!("struct {\n    $0\n}"));
    assert_eq!(struct_item["insertTextFormat"], json!(2));
}

#[test]
fn completion_request_does_not_offer_keywords_after_member_access() {
    let mut state = initialized_state();
    let source = concat!(
        "type Console = struct { len: i32 };\n",
        "fn main() i32 {\n",
        "    let console = Console.{ len: i32.{1} };\n",
        "    return console.le;\n",
        "}\n",
    );
    let uri = temp_file_uri("server_completion_member_access", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(37)),
            method: Some("textDocument/completion".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 3, "character": 21 }
            })),
        },
    );

    assert_eq!(response["id"], json!(37));
    let labels = response["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["label"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(!labels.contains(&"let"));
}

#[test]
fn completion_request_avoids_duplicate_call_parentheses() {
    let mut state = initialized_state();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 {\n    return hel();\n}\n";
    let uri = temp_file_uri("server_completion_existing_paren", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(31)),
            method: Some("textDocument/completion".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 2, "character": 14 }
            })),
        },
    );

    assert_eq!(response["id"], json!(31));
    let helper = response["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["label"] == json!("helper"))
        .unwrap();
    assert_eq!(helper.get("insertText"), None);
    assert_eq!(helper.get("insertTextFormat"), None);
}

#[test]
fn completion_request_prefers_types_in_type_positions() {
    let mut state = initialized_state();
    let source = concat!(
        "type MarkerType = struct {};\n",
        "fn Mark() MarkerType { return MarkerType.{}; }\n",
        "fn main() void {\n",
        "    let value = Mark() as Mar;\n",
        "}\n",
    );
    let uri = temp_file_uri("server_completion_type_context", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(33)),
            method: Some("textDocument/completion".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 3, "character": 29 }
            })),
        },
    );

    assert_eq!(response["id"], json!(33));
    let labels = response["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["label"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(labels.starts_with(&["MarkerType", "Mark"]));
}

#[test]
fn completion_request_on_broken_document_returns_empty_result_instead_of_error() {
    let mut state = initialized_state();
    let source = "fn main() i32 {\n    return \n";
    let uri = temp_file_uri("server_completion_broken", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(34)),
            method: Some("textDocument/completion".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 11 }
            })),
        },
    );

    assert_eq!(response["id"], json!(34));
    assert!(response.get("error").is_none());
    assert_eq!(response["result"], json!([]));
}
