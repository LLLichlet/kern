use super::super::dispatch::handle_message_nonblocking;
use super::*;
use std::collections::BTreeSet;

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
