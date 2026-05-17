use super::*;

#[test]
fn did_open_publishes_related_information_and_hints() {
    let mut state = initialized_state();
    let source = "fn main() i32 {\n    let value = 1i32\n    return value;\n}\n";
    let uri = temp_file_uri("server_related_diagnostics", source);

    let messages = dispatch_messages(&mut state, did_open_message(&uri, source, 1));

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["method"], "textDocument/publishDiagnostics");
    assert_eq!(messages[0]["params"]["uri"], uri);
    let diagnostics = messages[0]["params"]["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], json!("expected-semicolon"));
    assert!(
        diagnostics[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("Hint: consider adding a `;` here"))
    );
}

#[test]
fn verbose_trace_reports_diagnostics_lane_analysis() {
    let mut state = initialized_state();
    state.trace = super::super::lifecycle::TraceValue::Verbose;
    let source = "fn main() i32 {\n    let value = 1i32\n    return value;\n}\n";
    let uri = temp_file_uri("server_diagnostics_lane_trace", source);

    let messages = dispatch_messages(&mut state, did_open_message(&uri, source, 1));

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["method"], "$/logTrace");
    assert_eq!(
        messages[0]["params"]["message"],
        "diagnostics analysis completed"
    );
    let verbose = messages[0]["params"]["verbose"].as_str().unwrap();
    assert!(verbose.contains("tier=parse-only"), "{verbose}");
    assert!(verbose.contains("mode=Structure"), "{verbose}");
    assert!(verbose.contains("queue_wait_ms="), "{verbose}");
    assert!(verbose.contains("elapsed_ms="), "{verbose}");
    assert!(verbose.contains("request_id=None"), "{verbose}");
    assert!(verbose.contains("document_generation=1"), "{verbose}");
    assert!(verbose.contains("document_version=1"), "{verbose}");
    assert!(verbose.contains("snapshot_generation="), "{verbose}");
    assert!(verbose.contains("cache="), "{verbose}");
    assert!(verbose.contains("status=completed"), "{verbose}");
    assert!(verbose.contains("budget=ok"), "{verbose}");
    assert!(verbose.contains("error_class=None"), "{verbose}");
    assert_eq!(messages[1]["method"], "textDocument/publishDiagnostics");
}

#[test]
fn verbose_trace_reports_interactive_request_method() {
    let mut state = initialized_state();
    state.trace = super::super::lifecycle::TraceValue::Verbose;
    let source = "fn main() void {}\n";
    let uri = temp_file_uri("server_interactive_method_trace", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let messages = dispatch_messages(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(81)),
            method: Some("textDocument/documentSymbol".to_string()),
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
        .expect("expected interactive analysis trace");
    let verbose = trace["params"]["verbose"].as_str().unwrap();
    assert!(verbose.contains("tier=surface"), "{verbose}");
    assert!(verbose.contains("request_id=81"), "{verbose}");
    assert!(verbose.contains("document_generation=1"), "{verbose}");
    assert!(verbose.contains("document_version=1"), "{verbose}");
    assert!(verbose.contains("snapshot_generation="), "{verbose}");
    assert!(
        verbose.contains("method=textDocument/documentSymbol"),
        "{verbose}"
    );
    assert!(verbose.contains("elapsed_ms="), "{verbose}");
    assert!(verbose.contains("queue_wait_ms="), "{verbose}");
    assert!(verbose.contains("status=completed"), "{verbose}");
    assert!(verbose.contains("budget=ok"), "{verbose}");
    assert!(verbose.contains("cache="), "{verbose}");
    assert!(verbose.contains("error_class=None"), "{verbose}");
}

#[test]
fn verbose_trace_reports_workspace_refresh_latency() {
    let mut state = initialized_state();
    state.trace = super::super::lifecycle::TraceValue::Verbose;
    let source = "fn main() void {}\n";
    let uri = temp_file_uri("server_workspace_refresh_trace", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let messages = dispatch_messages(
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

    assert!(messages.iter().any(|message| {
        message["method"] == "$/logTrace"
            && message["params"]["message"] == "workspace refresh queued"
            && message["params"]["verbose"]
                .as_str()
                .is_some_and(|verbose| {
                    verbose.contains("reason=workspace source files changed")
                        && verbose.contains("targets=")
                        && verbose.contains("queue_wait_ms=")
                        && verbose.contains("elapsed_ms=")
                        && verbose.contains("request_id=None")
                        && verbose.contains("document_generation=None")
                        && verbose.contains("document_version=None")
                        && verbose.contains("snapshot_generation=")
                        && verbose.contains("status=completed")
                        && verbose.contains("budget=ok")
                        && verbose.contains("cache=")
                        && verbose.contains("error_class=None")
                })
    }));
}

#[test]
fn watched_source_file_change_uses_source_refresh() {
    let mut state = initialized_state();
    state.trace = super::super::lifecycle::TraceValue::Verbose;
    let source = "fn main() void {}\n";
    let uri = temp_file_uri("server_source_watched_refresh", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let messages = dispatch_messages(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: None,
            method: Some("workspace/didChangeWatchedFiles".to_string()),
            params: Some(json!({
                "changes": [
                    { "uri": uri, "type": 2 }
                ]
            })),
        },
    );

    assert!(messages.iter().any(|message| {
        message["method"] == "$/logTrace"
            && message["params"]["message"] == "workspace refresh queued"
            && message["params"]["verbose"]
                .as_str()
                .is_some_and(|verbose| verbose.contains("reason=workspace source files changed"))
    }));
}

#[test]
fn watched_project_metadata_change_uses_project_reload() {
    let mut state = initialized_state();
    state.trace = super::super::lifecycle::TraceValue::Verbose;
    let source = "fn main() void {}\n";
    let uri = temp_file_uri("server_metadata_watched_refresh", source);
    let manifest_uri = format!(
        "file://{}",
        crate::analysis::uri_to_file_path(&uri)
            .unwrap()
            .parent()
            .unwrap()
            .join("Craft.toml")
            .to_string_lossy()
    );

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let messages = dispatch_messages(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: None,
            method: Some("workspace/didChangeWatchedFiles".to_string()),
            params: Some(json!({
                "changes": [
                    { "uri": manifest_uri, "type": 2 }
                ]
            })),
        },
    );

    assert!(messages.iter().any(|message| {
        message["method"] == "$/logTrace"
            && message["params"]["message"] == "workspace refresh queued"
            && message["params"]["verbose"]
                .as_str()
                .is_some_and(|verbose| {
                    verbose.contains("reason=workspace project metadata changed")
                        && verbose.contains("snapshot_generation=")
                        && verbose.contains("cache=")
                        && verbose.contains("error_class=None")
                })
    }));
}

#[test]
fn workspace_refresh_reports_work_done_progress() {
    let mut state = initialized_state();
    state.work_done_progress = true;
    let source = "fn main() void {}\n";
    let uri = temp_file_uri("server_workspace_refresh_progress", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let messages = dispatch_messages(
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

    let create = messages
        .iter()
        .find(|message| message["method"] == "window/workDoneProgress/create")
        .unwrap();
    let token = create["params"]["token"].clone();
    assert_eq!(token, json!("kern-lsp/workspace-refresh/1"));

    let progress_messages: Vec<_> = messages
        .iter()
        .filter(|message| message["method"] == "$/progress" && message["params"]["token"] == token)
        .collect();
    assert_eq!(progress_messages.len(), 2);
    assert_eq!(progress_messages[0]["params"]["value"]["kind"], "begin");
    assert_eq!(
        progress_messages[0]["params"]["value"]["title"],
        "Kern workspace refresh"
    );
    assert_eq!(
        progress_messages[0]["params"]["value"]["message"],
        "workspace source files changed"
    );
    assert_eq!(progress_messages[1]["params"]["value"]["kind"], "end");
    assert!(
        progress_messages[1]["params"]["value"]["message"]
            .as_str()
            .unwrap()
            .contains("refreshed")
    );
    assert!(
        progress_messages[1]["params"]["value"]["message"]
            .as_str()
            .unwrap()
            .contains("indexed 1 targets")
    );
}

#[test]
fn workspace_refresh_skips_progress_without_client_support() {
    let mut state = initialized_state();
    let source = "fn main() void {}\n";
    let uri = temp_file_uri("server_workspace_refresh_no_progress", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let messages = dispatch_messages(
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

    assert!(
        !messages
            .iter()
            .any(|message| message["method"] == "window/workDoneProgress/create")
    );
    assert!(
        !messages
            .iter()
            .any(|message| message["method"] == "$/progress")
    );
}

#[test]
fn verbose_trace_marks_exceeded_workspace_refresh_budget() {
    let mut state = initialized_state();
    state.trace = super::super::lifecycle::TraceValue::Verbose;
    state.request_budget_policy.workspace_refresh_ms = 0;
    let source = "fn main() void {}\n";
    let uri = temp_file_uri("server_workspace_refresh_budget_trace", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let messages = dispatch_messages(
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

    assert!(messages.iter().any(|message| {
        message["method"] == "$/logTrace"
            && message["params"]["message"] == "workspace refresh queued"
            && message["params"]["verbose"]
                .as_str()
                .is_some_and(|verbose| verbose.contains("budget=exceeded"))
    }));
    assert!(messages.iter().any(|message| {
        message["method"] == "$/logTrace"
            && message["params"]["message"] == "workspace refresh queued"
            && message["params"]["verbose"]
                .as_str()
                .is_some_and(|verbose| verbose.contains("indexed_targets=1"))
    }));
    assert!(messages.iter().any(|message| {
        message["method"] == "$/logTrace"
            && message["params"]["message"] == "workspace refresh queued"
            && message["params"]["verbose"]
                .as_str()
                .is_some_and(|verbose| verbose.contains("index_generation="))
    }));
}

#[test]
fn workspace_refresh_reuses_diagnostics_budget_yielding() {
    let mut state = initialized_state();
    state.diagnostics_flush_policy.target_task_budget = 1;
    let source = "fn main() void {}\n";
    let uri_a = temp_file_uri("server_workspace_budget_yield_a", source);
    let uri_b = temp_file_uri("server_workspace_budget_yield_b", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri_a, source, 1));
    let _ = dispatch_messages(&mut state, did_open_message(&uri_b, source, 1));
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);
    let should_exit = handle_message(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: None,
            method: Some("workspace/didChangeWatchedFiles".to_string()),
            params: Some(json!({
                "changes": []
            })),
        },
    );
    assert!(!should_exit.unwrap());
    super::super::scheduler::flush_workspace_refresh_results(&mut state, &mut writer, true)
        .unwrap();
    super::super::scheduler::drain_scheduler(&mut state, &mut writer).unwrap();

    assert_eq!(state.pending_diagnostics_worker_tasks, 1);
    assert_eq!(state.pending_diagnostics_targets.len(), 1);
    assert!(state.has_pending_diagnostics_work());
}

#[test]
fn verbose_trace_marks_exceeded_diagnostics_budget() {
    let mut state = initialized_state();
    state.trace = super::super::lifecycle::TraceValue::Verbose;
    state.request_budget_policy.diagnostics_ms = 0;
    let source = "fn main() i32 {\n    let value = 1i32\n    return value;\n}\n";
    let uri = temp_file_uri("server_diagnostics_budget_trace", source);

    let messages = dispatch_messages(&mut state, did_open_message(&uri, source, 1));

    let verbose = messages[0]["params"]["verbose"].as_str().unwrap();
    assert!(verbose.contains("budget=exceeded"), "{verbose}");
}

#[test]
fn did_change_republishes_empty_diagnostics_after_fix() {
    let mut state = initialized_state();
    let invalid_source = "fn main() i32 {\n    let value = 1i32\n    return value;\n}\n";
    let valid_source = "fn main() i32 {\n    let value = 1i32;\n    return value;\n}\n";
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
fn did_save_publishes_unnecessary_tags_for_flow_warnings() {
    let mut state = initialized_state();
    let clean_source = "fn main() i32 { return 0; }\n";
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    value = seed + 2;\n",
        "    return value;\n",
        "}\n",
        "fn main() i32 { return helper(1); }\n",
    );
    let uri = temp_file_uri("server_unnecessary_tags", clean_source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, clean_source, 1));
    assert!(dispatch_messages(&mut state, did_change_message(&uri, source, 2)).is_empty());
    fs::write(crate::analysis::uri_to_file_path(&uri).unwrap(), source).unwrap();

    let messages = dispatch_messages(&mut state, did_save_message(&uri));
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
    let invalid_source = "fn main() i32 {\n    let value = 1i32\n    return value;\n}\n";
    let still_invalid = "fn main() i32 {\n    let value = 2i32\n    return value;\n}\n";
    let valid_source = "fn main() i32 {\n    let value = 2i32;\n    return value;\n}\n";
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
    let source = "fn main() i32 {\n    let value = 1i32;\n    return value;\n}\n";
    let changed = "fn main() i32 {\n    let value = 2i32;\n    return value;\n}\n";
    let uri = temp_file_uri("server_diagnostic_budget_single", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    let change_messages = dispatch_messages(&mut state, did_change_message(&uri, changed, 2));

    assert!(change_messages.is_empty());
    assert_eq!(state.pending_diagnostics_targets.len(), 1);
}

#[test]
fn did_change_reaching_target_budget_triggers_auto_drain() {
    let mut state = initialized_state();
    let source = "fn main() i32 {\n    let value = 1i32;\n    return value;\n}\n";
    let changed_a = "fn main() i32 {\n    let value = 2i32;\n    return value;\n}\n";
    let changed_b = "fn main() i32 {\n    let value = 3i32;\n    return value;\n}\n";
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

#[test]
fn did_change_auto_drain_uses_structure_diagnostics_only() {
    let mut state = initialized_state();
    let source = "fn main() i32 {\n    return 0;\n}\n";
    let changed_a = "fn helper() i32 { return 1; }\nfn main() i32 {\n    return 0;\n}\n";
    let changed_b = "fn helper() i32 { return 2; }\nfn main() i32 {\n    return 0;\n}\n";
    let uri_a = temp_file_uri("server_structure_budget_a", source);
    let uri_b = temp_file_uri("server_structure_budget_b", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri_a, source, 1));
    let _ = dispatch_messages(&mut state, did_open_message(&uri_b, source, 1));

    assert!(dispatch_messages(&mut state, did_change_message(&uri_a, changed_a, 2)).is_empty());
    let change_messages = dispatch_messages(&mut state, did_change_message(&uri_b, changed_b, 2));

    assert_eq!(change_messages.len(), 2);
    for message in change_messages {
        assert_eq!(message["method"], "textDocument/publishDiagnostics");
        assert!(
            message["params"]["diagnostics"]
                .as_array()
                .unwrap()
                .is_empty(),
            "{message:#}"
        );
    }
}

#[test]
fn did_save_upgrades_pending_structure_diagnostics_to_full_analysis() {
    let mut state = initialized_state();
    let source = "fn main() i32 {\n    return 0;\n}\n";
    let changed = "fn helper() i32 { return 1; }\nfn main() i32 {\n    return 0;\n}\n";
    let uri = temp_file_uri("server_save_full_analysis", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, source, 1));
    assert!(dispatch_messages(&mut state, did_change_message(&uri, changed, 2)).is_empty());
    fs::write(crate::analysis::uri_to_file_path(&uri).unwrap(), changed).unwrap();

    let save_messages = dispatch_messages(&mut state, did_save_message(&uri));

    assert_eq!(save_messages.len(), 1);
    let diagnostics = save_messages[0]["params"]["diagnostics"]
        .as_array()
        .unwrap();
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.contains("private function `helper` is never used"))
    }));
}
