use super::super::scheduler::{
    drain_scheduler, execute_document_diagnostics, execute_document_request,
    flush_diagnostics_lane, publish_analysis_outcome, write_success_response,
};
use super::super::state::SchedulerDrainDecision;
use super::super::*;
use super::*;

#[test]
fn stale_analysis_generation_drops_publish_result() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_stale_publish", "fn main() void {}\n");
    let stale = state.begin_target_analysis(&uri);
    let _newer = state.begin_target_analysis(&uri);
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    publish_analysis_outcome(
        &mut state,
        &mut writer,
        &uri,
        stale,
        AnalysisOutcome {
            bundles: vec![DiagnosticBundle {
                uri: uri.clone(),
                diagnostics: Vec::new(),
            }],
        },
    )
    .unwrap();

    assert!(output.is_empty());
    assert!(!state.published_by_target.contains_key(&uri));
}

#[test]
fn diagnostics_lane_coalesces_latest_publish_per_target() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_diagnostics_queue", "fn main() void {}\n");
    let first = state.begin_target_analysis(&uri);
    let second = state.begin_target_analysis(&uri);
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    state.queue_diagnostics_publish(
        uri.clone(),
        first,
        AnalysisOutcome {
            bundles: vec![DiagnosticBundle {
                uri: uri.clone(),
                diagnostics: Vec::new(),
            }],
        },
    );
    state.queue_diagnostics_publish(
        uri.clone(),
        second,
        AnalysisOutcome {
            bundles: vec![DiagnosticBundle {
                uri: uri.clone(),
                diagnostics: Vec::new(),
            }],
        },
    );

    flush_diagnostics_lane(&mut state, &mut writer).unwrap();

    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["method"], "textDocument/publishDiagnostics");
    assert!(state.pending_diagnostics.is_empty());
}

#[test]
fn diagnostics_lane_coalesces_target_analysis_tasks() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_diagnostics_task_queue", "fn main() void {}\n");

    state.queue_target_diagnostics_task(uri.clone());
    state.queue_target_diagnostics_task(uri.clone());

    assert_eq!(state.pending_diagnostics_targets.len(), 1);
    assert!(state.pending_diagnostics_targets.contains(&uri));
}

#[test]
fn diagnostics_execution_defers_publish_until_scheduler_drain() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_diagnostics_deferred_flush", "fn main() void {}\n");
    let mut output = Vec::new();
    {
        let mut writer = MessageWriter::new(&mut output);
        execute_document_diagnostics(
            &mut state,
            &mut writer,
            &uri,
            SchedulerLane::Diagnostics,
            |analysis| {
                analysis.open_document_state(DidOpenTextDocumentParams {
                    text_document: crate::protocol::TextDocumentItem {
                        uri: uri.clone(),
                        _language_id: "kern".to_string(),
                        version: 1,
                        text: "fn main() void {}\n".to_string(),
                    },
                })
            },
        )
        .unwrap();
    }

    assert!(output.is_empty());
    assert!(state.pending_diagnostics_targets.contains(&uri));

    {
        let mut writer = MessageWriter::new(&mut output);
        drain_scheduler(&mut state, &mut writer).unwrap();
    }

    let messages = read_all_messages(&output);
    assert!(!messages.is_empty());
    assert_eq!(messages[0]["method"], "textDocument/publishDiagnostics");
}

#[test]
fn did_save_is_an_explicit_scheduler_drain_boundary() {
    let state = initialized_state();
    assert!(
        state
            .diagnostics_flush_policy
            .decide_after_message("textDocument/didSave")
            == SchedulerDrainDecision::Drain
    );
}

#[test]
fn interactive_requests_do_not_auto_drain_deferred_diagnostics() {
    let mut state = initialized_state();
    let invalid_source = "fn main() i32 {\n    let value = i32.{1}\n    return value;\n}\n";
    let uri = temp_file_uri("server_deferred_diagnostics_interactive", invalid_source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, invalid_source, 1));
    let _ = dispatch_messages(
        &mut state,
        did_change_message(
            &uri,
            "fn main() i32 {\n    let value = i32.{2}\n    return value;\n}\n",
            2,
        ),
    );
    assert!(state.has_pending_diagnostics_work());

    let response = dispatch_single_response(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(52)),
            method: Some("textDocument/hover".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 3 }
            })),
        },
    );

    assert_eq!(response["id"], json!(52));
    assert!(state.has_pending_diagnostics_work());
}

#[test]
fn canceled_request_drops_response() {
    let mut state = initialized_state();
    state.cancel_request(json!(42));
    let request = state.request_context(json!(42));
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    write_success_response(&mut state, &mut writer, &request, json!({ "ok": true })).unwrap();

    assert!(output.is_empty());
    assert!(state.canceled_request_ids.is_empty());
}

#[test]
fn canceled_document_request_skips_analysis_work() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_canceled_preflight", "fn main() void {}\n");
    state.cancel_request(json!(44));
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);
    let mut analyzed = false;

    execute_document_request(
        &mut state,
        &mut writer,
        json!(44),
        &uri,
        SchedulerLane::Interactive,
        |_| {
            analyzed = true;
            Ok::<Value, String>(json!({ "ok": true }))
        },
    )
    .unwrap();

    assert!(!analyzed);
    assert!(output.is_empty());
    assert!(state.canceled_request_ids.is_empty());
}

#[test]
fn panicking_document_request_returns_error_response() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_panicking_request", "fn main() void {}\n");
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));

    execute_document_request::<Value, _>(
        &mut state,
        &mut writer,
        json!(47),
        &uri,
        SchedulerLane::Interactive,
        |_| panic!("synthetic analysis panic"),
    )
    .unwrap();
    std::panic::set_hook(previous_hook);

    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["id"], json!(47));
    assert!(
        messages[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("synthetic analysis panic")
    );
}

#[test]
fn stale_document_request_generation_drops_response() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_stale_request", "fn main() void {}\n");
    let _current = state.begin_target_analysis(&uri);
    let request = state.request_context_for_document(json!(43), &uri);
    let _newer = state.begin_target_analysis(&uri);
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    write_success_response(&mut state, &mut writer, &request, json!({ "ok": true })).unwrap();

    assert!(output.is_empty());
}

#[test]
fn stale_document_request_skips_analysis_work() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_stale_preflight", "fn main() void {}\n");
    let stale_generation = state.begin_target_analysis(&uri);
    let _newer = state.begin_target_analysis(&uri);
    let stale_request = RequestContext {
        id: json!(46),
        target_uri: Some(uri),
        generation: Some(stale_generation),
    };

    assert!(state.should_skip_request(&stale_request));
}
