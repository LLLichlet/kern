use super::super::dispatch::handle_message_nonblocking;
use super::super::scheduler::{
    execute_document_request, flush_workspace_refresh_results, schedule_workspace_refresh,
};
use super::*;
use std::collections::BTreeSet;
use std::sync::{Arc, Barrier};

#[test]
fn protocol_stress_opens_many_files_then_symbols_and_diagnostics() {
    const DOCUMENT_COUNT: usize = 100;
    let mut state = initialized_state();
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);
    let mut uris = Vec::new();

    for index in 0..DOCUMENT_COUNT {
        let source = format!(
            "struct StressSymbol{index} {{ value: i32 }}\nfn stress_helper_{index}() i32 {{ return {index}i32; }}\n"
        );
        let uri = temp_file_uri(&format!("server_protocol_stress_{index}"), &source);
        handle_message_nonblocking(&mut state, &mut writer, did_open_message(&uri, &source, 1))
            .unwrap();
        uris.push(uri);
    }
    handle_message_nonblocking(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(9001)),
            method: Some("workspace/symbol".to_string()),
            params: Some(json!({
                "query": "StressSymbol"
            })),
        },
    )
    .unwrap();
    drain_scheduler_to_quiescence(&mut state, &mut writer);

    let messages = read_all_messages(&output);
    let workspace_response = messages
        .iter()
        .find(|message| message["id"] == json!(9001))
        .expect("expected workspace/symbol response");
    assert_eq!(
        workspace_response["result"].as_array().unwrap().len(),
        DOCUMENT_COUNT
    );

    let diagnostic_uris = messages
        .iter()
        .filter(|message| message["method"] == "textDocument/publishDiagnostics")
        .filter_map(|message| message["params"]["uri"].as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(diagnostic_uris.len(), DOCUMENT_COUNT);
    for uri in &uris {
        assert!(
            diagnostic_uris.contains(uri.as_str()),
            "missing diagnostics for {uri}"
        );
    }
    assert!(state.pending_diagnostics_targets.is_empty());
    assert!(state.pending_diagnostics.is_empty());
    assert!(!state.has_pending_worker_work());
    assert!(state.analysis.cached_workspace_symbol_index_count() >= DOCUMENT_COUNT);
}

#[test]
fn protocol_stress_rapid_edit_burst_uses_latest_document_text() {
    let mut state = initialized_state();
    state.diagnostics_flush_policy.target_task_budget = usize::MAX;
    let uri = temp_file_uri(
        "server_protocol_stress_rapid_edits",
        "struct InitialSymbol { value: i32 }\n",
    );

    let _ = dispatch_messages(
        &mut state,
        did_open_message(&uri, "struct InitialSymbol { value: i32 }\n", 1),
    );
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    for version in 2..18 {
        let source = format!("struct BurstSymbol{version} {{ value: i32 }}\n");
        handle_message_nonblocking(
            &mut state,
            &mut writer,
            did_change_message(&uri, &source, version),
        )
        .unwrap();
    }
    assert_eq!(
        state.pending_diagnostics_targets.len(),
        1,
        "rapid edits for one document should coalesce to one diagnostics target"
    );

    handle_message_nonblocking(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(9002)),
            method: Some("workspace/symbol".to_string()),
            params: Some(json!({
                "query": "BurstSymbol17"
            })),
        },
    )
    .unwrap();
    drain_scheduler_to_quiescence(&mut state, &mut writer);

    let messages = read_all_messages(&output);
    let workspace_response = messages
        .iter()
        .find(|message| message["id"] == json!(9002))
        .expect("expected workspace/symbol response");
    assert_eq!(workspace_response["result"].as_array().unwrap().len(), 1);
    assert_eq!(workspace_response["result"][0]["name"], "BurstSymbol17");

    let diagnostics = messages
        .iter()
        .filter(|message| message["method"] == "textDocument/publishDiagnostics")
        .collect::<Vec<_>>();
    assert_eq!(diagnostics.len(), 1, "{messages:#?}");
    assert_eq!(diagnostics[0]["params"]["uri"], uri);
    assert_eq!(diagnostics[0]["params"]["diagnostics"], json!([]));
    assert!(state.pending_diagnostics_targets.is_empty());
    assert!(state.pending_diagnostics.is_empty());
    assert!(!state.has_pending_worker_work());
}

#[test]
fn protocol_stress_alternates_edits_and_completion_requests() {
    let mut state = initialized_state();
    state.diagnostics_flush_policy.target_task_budget = usize::MAX;
    let initial = "fn main() void {\n    let m\n}\n";
    let uri = temp_file_uri("server_protocol_stress_edit_completion", initial);
    let _ = dispatch_messages(&mut state, did_open_message(&uri, initial, 1));
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    for version in 2..26 {
        let source = format!("fn main() void {{\n    let m{version}\n}}\n");
        handle_message_nonblocking(
            &mut state,
            &mut writer,
            did_change_message(&uri, &source, version),
        )
        .unwrap();
        handle_message_nonblocking(
            &mut state,
            &mut writer,
            IncomingMessage {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: Some(json!(9100 + version)),
                method: Some("textDocument/completion".to_string()),
                params: Some(json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": 1, "character": 9 }
                })),
            },
        )
        .unwrap();
    }
    drain_scheduler_to_quiescence(&mut state, &mut writer);

    let messages = read_all_messages(&output);
    let final_response = messages
        .iter()
        .find(|message| message["id"] == json!(9125))
        .expect("expected final completion response");
    let labels = final_response["result"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect::<BTreeSet<_>>();
    assert!(labels.contains("mut"), "{final_response:#?}");
    assert!(state.pending_diagnostics_targets.is_empty());
    assert!(state.pending_diagnostics.is_empty());
    assert!(!state.has_pending_worker_work());
}

#[test]
fn protocol_stress_cancel_references_then_edit_and_hover() {
    let mut state = ServerState::with_options(
        AnalysisEngine::default(),
        ServerOptions { worker_threads: 1 },
    );
    state.initialized = true;
    let initial = "fn helper() i32 { return 1; }\nfn main() i32 { return helper(); }\n";
    let changed = "fn next() i32 { return 2; }\nfn main() i32 { return next(); }\n";
    let uri = temp_file_uri("server_protocol_stress_cancel_edit_hover", initial);
    let _ = dispatch_messages(&mut state, did_open_message(&uri, initial, 1));
    let started = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    execute_document_request(
        &mut state,
        &mut writer,
        json!(9198),
        &uri,
        SchedulerLane::Interactive,
        "textDocument/hover",
        {
            let started = started.clone();
            let release = release.clone();
            move |_, _| {
                started.wait();
                release.wait();
                Ok::<Value, String>(json!(null))
            }
        },
    )
    .unwrap();
    started.wait();

    handle_message_nonblocking(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(9199)),
            method: Some("textDocument/references".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 24 },
                "context": { "includeDeclaration": true }
            })),
        },
    )
    .unwrap();
    handle_message_nonblocking(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: None,
            method: Some("$/cancelRequest".to_string()),
            params: Some(json!({ "id": 9199 })),
        },
    )
    .unwrap();
    handle_message_nonblocking(
        &mut state,
        &mut writer,
        did_change_message(&uri, changed, 2),
    )
    .unwrap();
    handle_message_nonblocking(
        &mut state,
        &mut writer,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(9200)),
            method: Some("textDocument/hover".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 24 }
            })),
        },
    )
    .unwrap();

    release.wait();
    drain_scheduler_to_quiescence(&mut state, &mut writer);

    let messages = read_all_messages(&output);
    assert!(
        messages.iter().all(|message| message["id"] != json!(9199)),
        "stale canceled references response should be dropped after the edit: {messages:#?}"
    );
    let hover = messages
        .iter()
        .find(|message| message["id"] == json!(9200))
        .expect("expected hover response after edit");
    let contents = hover["result"]["contents"]["value"].as_str().unwrap();
    assert!(contents.contains("fn next: &fn() i32"), "{contents}");
    assert!(state.pending_diagnostics_targets.is_empty());
    assert!(state.pending_diagnostics.is_empty());
    assert!(!state.has_pending_worker_work());
}

#[test]
fn protocol_stress_workspace_refresh_does_not_block_interactive_hover() {
    let mut state = ServerState::with_options(
        AnalysisEngine::default(),
        ServerOptions { worker_threads: 1 },
    );
    state.initialized = true;
    state.trace = super::super::lifecycle::TraceValue::Verbose;
    let root = unique_temp_dir("server_protocol_stress_refresh_interactive");
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        root.join("Craft.toml"),
        format!(
            r#"
[package]
name = "refresh_interactive"
version = "0.1.0"
kern = "{}"

[lib]
root = "src/lib.kn"
"#,
            env!("CARGO_PKG_VERSION")
        ),
    )
    .unwrap();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper(); }\n";
    let source_path = src.join("lib.kn");
    fs::write(&source_path, source).unwrap();
    let uri = format!("file://{}", source_path.to_string_lossy());
    state.workspace_roots = vec![root];

    let mut output = Vec::new();
    {
        let mut writer = MessageWriter::new(&mut output);
        handle_message_nonblocking(&mut state, &mut writer, did_open_message(&uri, source, 1))
            .unwrap();
        schedule_workspace_refresh(
            &mut state,
            &mut writer,
            "stress workspace refresh",
            WorkspaceRefreshKind::Sources,
        )
        .unwrap();
        assert_eq!(
            state.pending_workspace_refresh_reason.as_deref(),
            Some("stress workspace refresh")
        );
        assert_eq!(state.pending_workspace_refresh_tasks, 0);

        handle_message_nonblocking(
            &mut state,
            &mut writer,
            IncomingMessage {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: Some(json!(9300)),
                method: Some("textDocument/hover".to_string()),
                params: Some(json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": 1, "character": 24 }
                })),
            },
        )
        .unwrap();
        super::super::scheduler::flush_document_request_results(&mut state, &mut writer, true)
            .unwrap();
    }

    let early_messages = read_all_messages(&output);
    assert!(
        early_messages
            .iter()
            .any(|message| message["id"] == json!(9300)
                && message["result"]["contents"]["value"]
                    .as_str()
                    .is_some_and(|contents| contents.contains("fn helper"))),
        "interactive hover should complete before the queued workspace refresh is drained: {early_messages:#?}"
    );
    assert!(
        early_messages.iter().all(|message| {
            message["params"]["message"] != json!("workspace refresh queued")
                && message["method"] != "textDocument/publishDiagnostics"
        }),
        "workspace refresh work should still be pending before explicit refresh drain: {early_messages:#?}"
    );
    assert_eq!(
        state.pending_workspace_refresh_reason.as_deref(),
        Some("stress workspace refresh")
    );
    assert_eq!(state.pending_workspace_refresh_tasks, 0);

    {
        let mut writer = MessageWriter::new(&mut output);
        super::super::scheduler::drain_scheduler(&mut state, &mut writer).unwrap();
        flush_workspace_refresh_results(&mut state, &mut writer, true).unwrap();
        super::super::scheduler::drain_scheduler(&mut state, &mut writer).unwrap();
        drain_scheduler_to_quiescence(&mut state, &mut writer);
    }

    let messages = read_all_messages(&output);
    assert!(
        messages.iter().any(|message| {
            message["method"] == "$/logTrace"
                && message["params"]["message"] == "workspace refresh queued"
                && message["params"]["verbose"]
                    .as_str()
                    .is_some_and(|verbose| verbose.contains("indexed_targets=1"))
        }),
        "expected workspace refresh to finish after interactive hover: {messages:#?}"
    );
    assert!(state.pending_diagnostics_targets.is_empty());
    assert!(state.pending_diagnostics.is_empty());
    assert!(!state.has_pending_worker_work());
}

#[test]
fn protocol_stress_repeated_invalid_valid_craft_manifest_transitions() {
    let root = unique_temp_dir("server_protocol_stress_manifest_transitions");
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    let manifest_path = root.join("Craft.toml");
    let valid_manifest = format!(
        r#"
[package]
name = "manifest_transitions"
version = "0.1.0"
kern = "{}"

[lib]
root = "src/lib.kn"
"#,
        env!("CARGO_PKG_VERSION")
    );
    fs::write(&manifest_path, &valid_manifest).unwrap();
    let source = "fn main() i32 { return 0; }\n";
    let source_path = src.join("lib.kn");
    fs::write(&source_path, source).unwrap();
    let uri = format!("file://{}", source_path.to_string_lossy());
    let manifest_uri = format!("file://{}", manifest_path.to_string_lossy());
    let mut state = initialized_state();
    state.workspace_roots = vec![root];
    state.trace = super::super::lifecycle::TraceValue::Verbose;

    let mut output = Vec::new();
    {
        let mut writer = MessageWriter::new(&mut output);
        handle_message_nonblocking(&mut state, &mut writer, did_open_message(&uri, source, 1))
            .unwrap();
        drain_scheduler_to_quiescence(&mut state, &mut writer);
    }

    for iteration in 0..3 {
        fs::write(&manifest_path, "not valid craft toml").unwrap();
        {
            let mut writer = MessageWriter::new(&mut output);
            handle_message_nonblocking(
                &mut state,
                &mut writer,
                watched_file_change_message(&manifest_uri),
            )
            .unwrap();
            drain_scheduler_to_quiescence(&mut state, &mut writer);
        }

        let messages = read_all_messages(&output);
        assert!(
            messages.iter().rev().any(|message| {
                message["method"] == "textDocument/publishDiagnostics"
                    && message["params"]["uri"] == uri
                    && message["params"]["diagnostics"]
                        .as_array()
                        .is_some_and(|diagnostics| {
                            diagnostics.iter().any(|diagnostic| {
                                diagnostic["message"].as_str().is_some_and(|message| {
                                    message.contains("analysis failed")
                                        && message.contains("Craft.toml")
                                })
                            })
                        })
            }),
            "invalid manifest transition {iteration} should publish a visible project diagnostic: {messages:#?}"
        );

        fs::write(&manifest_path, &valid_manifest).unwrap();
        {
            let mut writer = MessageWriter::new(&mut output);
            handle_message_nonblocking(
                &mut state,
                &mut writer,
                watched_file_change_message(&manifest_uri),
            )
            .unwrap();
            drain_scheduler_to_quiescence(&mut state, &mut writer);
        }

        let messages = read_all_messages(&output);
        assert!(
            messages.iter().rev().any(|message| {
                message["method"] == "textDocument/publishDiagnostics"
                    && message["params"]["uri"] == uri
                    && message["params"]["diagnostics"] == json!([])
            }),
            "valid manifest transition {iteration} should clear stale project diagnostics: {messages:#?}"
        );
    }

    let messages = read_all_messages(&output);
    let project_reload_traces = messages
        .iter()
        .filter(|message| {
            message["method"] == "$/logTrace"
                && message["params"]["message"] == "workspace refresh queued"
                && message["params"]["verbose"]
                    .as_str()
                    .is_some_and(|verbose| {
                        verbose.contains("reason=workspace project metadata changed")
                    })
        })
        .count();
    assert!(
        project_reload_traces >= 6,
        "expected one project reload trace per manifest transition: {messages:#?}"
    );
    assert!(state.pending_diagnostics_targets.is_empty());
    assert!(state.pending_diagnostics.is_empty());
    assert!(!state.has_pending_worker_work());
}

fn watched_file_change_message(uri: &str) -> IncomingMessage {
    IncomingMessage {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: None,
        method: Some("workspace/didChangeWatchedFiles".to_string()),
        params: Some(json!({
            "changes": [
                { "uri": uri, "type": 2 }
            ]
        })),
    }
}
