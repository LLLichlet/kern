use super::*;

#[test]
fn did_open_publishes_related_information_and_hints() {
    let mut state = initialized_state();
    let source = "fn helper() i32 { return 1; }\nfn helper() i32 { return 2; }\n";
    let uri = temp_file_uri("server_related_diagnostics", source);

    let messages = dispatch_messages(&mut state, did_open_message(&uri, source, 1));

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["method"], "textDocument/publishDiagnostics");
    assert_eq!(messages[0]["params"]["uri"], uri);
    let diagnostics = messages[0]["params"]["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0]["message"],
        "the name `helper` is defined multiple times\n\nHint: `helper` must be defined only once in the same scope"
    );
    let related = diagnostics[0]["relatedInformation"].as_array().unwrap();
    assert_eq!(related.len(), 1);
    assert_eq!(
        related[0]["message"],
        "previous definition of `helper` was here"
    );
}

#[test]
fn did_change_republishes_empty_diagnostics_after_fix() {
    let mut state = initialized_state();
    let invalid_source = "fn main() i32 {\n    let value = i32.{1}\n    return value;\n}\n";
    let valid_source = "fn main() i32 {\n    let value = i32.{1};\n    return value;\n}\n";
    let uri = temp_file_uri("server_diagnostic_clear", invalid_source);

    let open_messages = dispatch_messages(&mut state, did_open_message(&uri, invalid_source, 1));
    assert_eq!(open_messages.len(), 1);
    assert!(
        !open_messages[0]["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let change_messages = dispatch_messages(&mut state, did_change_message(&uri, valid_source, 2));

    assert!(change_messages.is_empty());

    let save_messages = dispatch_messages(&mut state, did_save_message(&uri));

    assert_eq!(save_messages.len(), 1);
    assert_eq!(
        save_messages[0]["method"],
        "textDocument/publishDiagnostics"
    );
    assert_eq!(save_messages[0]["params"]["uri"], uri);
    assert!(
        save_messages[0]["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn did_open_publishes_unnecessary_tags_for_flow_warnings() {
    let mut state = initialized_state();
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    value = seed + 2;\n",
        "    return value;\n",
        "}\n",
        "fn main() i32 { return helper(1); }\n",
    );
    let uri = temp_file_uri("server_unnecessary_tags", source);

    let messages = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let diagnostics = messages
        .iter()
        .find(|message| {
            message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
        })
        .and_then(|message| message["params"]["diagnostics"].as_array())
        .expect("expected publishDiagnostics for target uri");
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("value assigned to `value` is never read"))
        })
        .expect("expected dead-store warning");
    assert_eq!(diagnostic["code"], json!("dead-store"));
    assert_eq!(diagnostic["tags"], json!([1]));
}

#[test]
fn multiple_did_change_notifications_coalesce_until_save() {
    let mut state = initialized_state();
    let invalid_source = "fn main() i32 {\n    let value = i32.{1}\n    return value;\n}\n";
    let still_invalid = "fn main() i32 {\n    let value = i32.{2}\n    return value;\n}\n";
    let valid_source = "fn main() i32 {\n    let value = i32.{2};\n    return value;\n}\n";
    let uri = temp_file_uri("server_diagnostic_coalesce", invalid_source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, invalid_source, 1));
    assert!(dispatch_messages(&mut state, did_change_message(&uri, still_invalid, 2)).is_empty());
    assert!(dispatch_messages(&mut state, did_change_message(&uri, valid_source, 3)).is_empty());

    let save_messages = dispatch_messages(&mut state, did_save_message(&uri));

    assert_eq!(save_messages.len(), 1);
    assert_eq!(
        save_messages[0]["method"],
        "textDocument/publishDiagnostics"
    );
    assert!(
        save_messages[0]["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn did_change_under_budget_stays_deferred() {
    let mut state = initialized_state();
    let source = "fn main() i32 {\n    let value = i32.{1};\n    return value;\n}\n";
    let changed = "fn main() i32 {\n    let value = i32.{2};\n    return value;\n}\n";
    let uri = temp_file_uri("server_diagnostic_budget_single", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let change_messages = dispatch_messages(&mut state, did_change_message(&uri, changed, 2));

    assert!(change_messages.is_empty());
    assert_eq!(state.pending_diagnostics_targets.len(), 1);
}

#[test]
fn did_change_reaching_target_budget_triggers_auto_drain() {
    let mut state = initialized_state();
    let source = "fn main() i32 {\n    let value = i32.{1};\n    return value;\n}\n";
    let changed_a = "fn main() i32 {\n    let value = i32.{2};\n    return value;\n}\n";
    let changed_b = "fn main() i32 {\n    let value = i32.{3};\n    return value;\n}\n";
    let uri_a = temp_file_uri("server_diagnostic_budget_a", source);
    let uri_b = temp_file_uri("server_diagnostic_budget_b", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri_a, source, 1));
    let _ = dispatch_messages(&mut state, did_open_message(&uri_b, source, 1));

    assert!(dispatch_messages(&mut state, did_change_message(&uri_a, changed_a, 2)).is_empty());
    let change_messages = dispatch_messages(&mut state, did_change_message(&uri_b, changed_b, 2));

    assert_eq!(change_messages.len(), 2);
    assert!(
        change_messages
            .iter()
            .all(|message| message["method"] == "textDocument/publishDiagnostics")
    );
    assert!(!state.has_pending_diagnostics_work());
}
