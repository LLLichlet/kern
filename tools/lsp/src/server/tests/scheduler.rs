use super::super::scheduler::{
    drain_scheduler, execute_document_diagnostics, execute_document_request,
    execute_optional_document_request, flush_diagnostics_lane, flush_document_request_results,
    publish_analysis_outcome, submit_document_request_result, write_error_response,
    write_success_response,
};
use super::super::state::{
    DocumentRequestResponse, DocumentRequestTaskResult, SchedulerDrainDecision,
};
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
    let first = state.begin_target_analysis(&uri);
    let second = state.begin_target_analysis(&uri);

    state.queue_target_diagnostics_task(uri.clone(), first, DiagnosticsAnalysisMode::Structure);
    state.queue_target_diagnostics_task(uri.clone(), second, DiagnosticsAnalysisMode::Structure);

    assert_eq!(state.pending_diagnostics_targets.len(), 1);
    let task = state.pending_diagnostics_targets.get(&uri).unwrap();
    assert_eq!(task.generation, second);
    assert_eq!(task.mode, DiagnosticsAnalysisMode::Structure);
}

#[test]
fn diagnostics_lane_upgrades_coalesced_target_to_full_analysis() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_diagnostics_task_upgrade", "fn main() void {}\n");
    let first = state.begin_target_analysis(&uri);
    let second = state.begin_target_analysis(&uri);

    state.queue_target_diagnostics_task(uri.clone(), first, DiagnosticsAnalysisMode::Structure);
    state.queue_target_diagnostics_task(uri.clone(), second, DiagnosticsAnalysisMode::Full);

    assert_eq!(state.pending_diagnostics_targets.len(), 1);
    let task = state.pending_diagnostics_targets.get(&uri).unwrap();
    assert_eq!(task.generation, second);
    assert_eq!(task.mode, DiagnosticsAnalysisMode::Full);
}

#[test]
fn dirty_open_queues_structure_diagnostics() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_dirty_open_structure", "fn main() void {}\n");
    let mut output = Vec::new();
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
                    text: "fn main() void {\n}\n".to_string(),
                },
            })
        },
    )
    .unwrap();

    assert_eq!(
        state
            .pending_diagnostics_targets
            .get(&uri)
            .map(|task| task.mode),
        Some(DiagnosticsAnalysisMode::Structure)
    );
}

#[test]
fn dirty_save_keeps_structure_diagnostics() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_dirty_save_structure", "fn main() void {}\n");
    let mut output = Vec::new();
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
    state.pending_diagnostics_targets.clear();

    execute_document_diagnostics(
        &mut state,
        &mut writer,
        &uri,
        SchedulerLane::Diagnostics,
        |analysis| {
            analysis.change_document_state(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version: 2,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    text: "fn main() void {\n}\n".to_string(),
                }],
            })
        },
    )
    .unwrap();
    state.pending_diagnostics_targets.clear();

    execute_document_diagnostics(
        &mut state,
        &mut writer,
        &uri,
        SchedulerLane::Diagnostics,
        |analysis| analysis.save_document_state(uri.clone()),
    )
    .unwrap();

    assert_eq!(
        state
            .pending_diagnostics_targets
            .get(&uri)
            .map(|task| task.mode),
        Some(DiagnosticsAnalysisMode::Structure)
    );
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
    assert!(state.pending_diagnostics_targets.contains_key(&uri));

    {
        let mut writer = MessageWriter::new(&mut output);
        drain_scheduler(&mut state, &mut writer).unwrap();
        drain_scheduler_to_quiescence(&mut state, &mut writer);
    }

    let messages = read_all_messages(&output);
    assert!(!messages.is_empty());
    assert_eq!(messages[0]["method"], "textDocument/publishDiagnostics");
}

#[test]
fn stale_diagnostics_task_skips_analysis_work() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_stale_diagnostics_task", "fn main() void {}\n");
    let stale = state.begin_target_analysis(&uri);
    let _newer = state.begin_target_analysis(&uri);
    state.pending_diagnostics_targets.insert(
        uri.clone(),
        super::super::state::ScheduledDiagnosticsTask {
            generation: stale,
            mode: DiagnosticsAnalysisMode::Full,
        },
    );
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    flush_diagnostics_lane(&mut state, &mut writer).unwrap();

    assert!(output.is_empty());
    assert!(state.pending_diagnostics_targets.is_empty());
    assert!(state.pending_diagnostics.is_empty());
    assert_eq!(state.analysis.last_analysis_tier(), None);
}

#[test]
fn stale_diagnostics_task_is_not_queued() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_stale_diagnostics_queue", "fn main() void {}\n");
    let stale = state.begin_target_analysis(&uri);
    let _newer = state.begin_target_analysis(&uri);

    state.queue_target_diagnostics_task(uri, stale, DiagnosticsAnalysisMode::Full);

    assert!(state.pending_diagnostics_targets.is_empty());
}

#[test]
fn diagnostics_lane_yields_remaining_tasks_after_exceeded_budget() {
    let mut state = initialized_state();
    state.diagnostics_flush_policy.target_task_budget = 1;
    let uri_a = temp_file_uri("server_budget_yield_a", "fn main() void {}\n");
    let uri_b = temp_file_uri("server_budget_yield_b", "fn main() void {}\n");
    let generation_a = state.begin_target_analysis(&uri_a);
    let generation_b = state.begin_target_analysis(&uri_b);
    state.queue_target_diagnostics_task(
        uri_a.clone(),
        generation_a,
        DiagnosticsAnalysisMode::Structure,
    );
    state.queue_target_diagnostics_task(
        uri_b.clone(),
        generation_b,
        DiagnosticsAnalysisMode::Structure,
    );
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    flush_diagnostics_lane(&mut state, &mut writer).unwrap();

    assert!(output.is_empty());
    assert_eq!(state.pending_diagnostics_worker_tasks, 1);
    assert_eq!(state.pending_diagnostics_targets.len(), 1);
    assert!(state.has_pending_diagnostics_work());
}

#[test]
fn diagnostics_lane_prioritizes_active_document_within_target_budget() {
    let mut state = initialized_state();
    state.diagnostics_flush_policy.target_task_budget = 1;
    let uri_a = temp_file_uri("server_active_priority_a", "fn main() void {}\n");
    let uri_b = temp_file_uri("server_active_priority_b", "fn main() void {}\n");
    let generation_a = state.begin_target_analysis(&uri_a);
    let generation_b = state.begin_target_analysis(&uri_b);
    state.queue_target_diagnostics_task(
        uri_a.clone(),
        generation_a,
        DiagnosticsAnalysisMode::Structure,
    );
    state.queue_target_diagnostics_task(
        uri_b.clone(),
        generation_b,
        DiagnosticsAnalysisMode::Structure,
    );
    state.mark_active_document(&uri_b);
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    flush_diagnostics_lane(&mut state, &mut writer).unwrap();

    assert!(output.is_empty());
    assert_eq!(state.pending_diagnostics_worker_tasks, 1);
    assert_eq!(
        state
            .pending_diagnostics_targets
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec![uri_a]
    );
    assert!(state.has_pending_diagnostics_work());
}

#[test]
fn diagnostics_lane_respects_per_drain_target_budget() {
    let mut state = initialized_state();
    state.diagnostics_flush_policy.target_task_budget = 2;
    let uris = ["a", "b", "c"]
        .into_iter()
        .map(|suffix| {
            temp_file_uri(
                &format!("server_per_drain_budget_{suffix}"),
                "fn main() void {}\n",
            )
        })
        .collect::<Vec<_>>();
    for uri in &uris {
        let generation = state.begin_target_analysis(uri);
        state.queue_target_diagnostics_task(
            uri.clone(),
            generation,
            DiagnosticsAnalysisMode::Structure,
        );
    }
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    flush_diagnostics_lane(&mut state, &mut writer).unwrap();

    assert!(state.pending_diagnostics_worker_tasks <= 2);
    let submitted_or_completed = state.pending_diagnostics_worker_tasks
        + state.pending_diagnostics.len()
        + state.published_by_target.len();
    assert_eq!(submitted_or_completed, 2);
    assert_eq!(state.pending_diagnostics_targets.len(), 1);
    assert!(state.has_pending_diagnostics_work());
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
    let invalid_source = "fn main() i32 {\n    let value = 1i32\n    return value;\n}\n";
    let uri = temp_file_uri("server_deferred_diagnostics_interactive", invalid_source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri, invalid_source, 1));
    let _ = dispatch_messages(
        &mut state,
        did_change_message(
            &uri,
            "fn main() i32 {\n    let value = 2i32\n    return value;\n}\n",
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
fn interactive_requests_do_not_force_drain_when_diagnostics_budget_is_reached() {
    let mut state = initialized_state();
    let source = "fn main() i32 {\n    let value = 1i32;\n    return value;\n}\n";
    let uri_a = temp_file_uri("server_interactive_budget_a", source);
    let uri_b = temp_file_uri("server_interactive_budget_b", source);

    let _ = dispatch_messages(&mut state, did_open_message(&uri_a, source, 1));
    let _ = dispatch_messages(&mut state, did_open_message(&uri_b, source, 1));
    state.pending_diagnostics_targets.clear();
    let generation_a = state.begin_target_analysis(&uri_a);
    let generation_b = state.begin_target_analysis(&uri_b);
    state.queue_target_diagnostics_task(
        uri_a.clone(),
        generation_a,
        DiagnosticsAnalysisMode::Structure,
    );
    state.queue_target_diagnostics_task(
        uri_b.clone(),
        generation_b,
        DiagnosticsAnalysisMode::Structure,
    );
    assert_eq!(
        state.pending_diagnostics_targets.len(),
        state.diagnostics_flush_policy.target_task_budget
    );

    let messages = dispatch_messages(
        &mut state,
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(53)),
            method: Some("textDocument/hover".to_string()),
            params: Some(json!({
                "textDocument": { "uri": uri_a },
                "position": { "line": 0, "character": 3 }
            })),
        },
    );

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["id"], json!(53));
    assert_eq!(
        state.pending_diagnostics_targets.len(),
        state.diagnostics_flush_policy.target_task_budget
    );
}

#[test]
fn canceled_non_document_response_is_not_rewritten() {
    let mut state = initialized_state();
    state.cancel_request(json!(42));
    let request = state.request_context(json!(42));
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    write_success_response(&mut state, &mut writer, &request, json!({ "ok": true })).unwrap();

    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["id"], json!(42));
    assert_eq!(messages[0]["result"], json!({ "ok": true }));
    assert!(state.canceled_request_ids.is_empty());
}

#[test]
fn canceled_document_request_skips_analysis_work_and_returns_canceled_error() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_canceled_preflight", "fn main() void {}\n");
    state.cancel_request(json!(44));
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);
    let analyzed = std::sync::Arc::new(std::sync::Mutex::new(false));
    let observed = analyzed.clone();

    execute_document_request(
        &mut state,
        &mut writer,
        json!(44),
        &uri,
        SchedulerLane::Interactive,
        "textDocument/hover",
        move |_, _| {
            *observed.lock().unwrap() = true;
            Ok::<Value, String>(json!({ "ok": true }))
        },
    )
    .unwrap();

    flush_document_request_results(&mut state, &mut writer, true).unwrap();

    assert!(!*analyzed.lock().unwrap());
    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["id"], json!(44));
    assert_eq!(messages[0]["error"]["code"], json!(REQUEST_CANCELLED));
    assert_eq!(messages[0]["error"]["message"], "request was canceled");
    assert!(state.canceled_request_ids.is_empty());
}

#[test]
fn queued_document_request_cancel_skips_analysis_work() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_canceled_queued_request", "fn main() void {}\n");
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);
    let release = std::sync::Arc::new(std::sync::Barrier::new(5));
    let analyzed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    for id in 100..104 {
        let release = release.clone();
        execute_document_request(
            &mut state,
            &mut writer,
            json!(id),
            &uri,
            SchedulerLane::Interactive,
            "textDocument/hover",
            move |_, _| {
                release.wait();
                Ok::<Value, String>(json!({ "id": id }))
            },
        )
        .unwrap();
    }

    execute_document_request(
        &mut state,
        &mut writer,
        json!(104),
        &uri,
        SchedulerLane::Interactive,
        "textDocument/hover",
        {
            let analyzed = analyzed.clone();
            move |_, _| {
                analyzed.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok::<Value, String>(json!({ "id": 104 }))
            }
        },
    )
    .unwrap();

    state.cancel_request(json!(104));
    release.wait();
    while state.has_pending_document_request_work() {
        flush_document_request_results(&mut state, &mut writer, true).unwrap();
    }

    assert!(!analyzed.load(std::sync::atomic::Ordering::SeqCst));
    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 5);
    let canceled = messages
        .iter()
        .find(|message| message["id"] == json!(104))
        .unwrap();
    assert_eq!(canceled["error"]["code"], json!(REQUEST_CANCELLED));
    assert_eq!(canceled["error"]["message"], "request was canceled");
}

#[test]
fn running_document_request_cancel_returns_canceled_error() {
    let mut state = initialized_state();
    state.trace = super::super::lifecycle::TraceValue::Verbose;
    let uri = temp_file_uri("server_canceled_running_request", "fn main() void {}\n");
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);
    let started = std::sync::Arc::new(std::sync::Barrier::new(2));
    let release = std::sync::Arc::new(std::sync::Barrier::new(2));
    let analyzed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    execute_document_request(
        &mut state,
        &mut writer,
        json!(105),
        &uri,
        SchedulerLane::Interactive,
        "textDocument/hover",
        {
            let started = started.clone();
            let release = release.clone();
            let analyzed = analyzed.clone();
            move |_, _| {
                analyzed.store(true, std::sync::atomic::Ordering::SeqCst);
                started.wait();
                release.wait();
                Ok::<Value, String>(json!({ "id": 105 }))
            }
        },
    )
    .unwrap();

    started.wait();
    state.cancel_request(json!(105));
    release.wait();
    flush_document_request_results(&mut state, &mut writer, true).unwrap();

    assert!(analyzed.load(std::sync::atomic::Ordering::SeqCst));
    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["method"], "$/logTrace");
    assert_eq!(messages[0]["params"]["message"], "request canceled");
    let verbose = messages[0]["params"]["verbose"].as_str().unwrap();
    assert!(verbose.contains("queue_wait_ms="), "{verbose}");
    assert!(verbose.contains("elapsed_ms="), "{verbose}");
    assert!(verbose.contains("status=canceled"), "{verbose}");
    assert!(verbose.contains("method=textDocument/hover"), "{verbose}");
    assert_eq!(messages[1]["id"], json!(105));
    assert_eq!(messages[1]["error"]["code"], json!(REQUEST_CANCELLED));
    assert_eq!(messages[1]["error"]["message"], "request was canceled");
    assert!(state.canceled_request_ids.is_empty());
}

#[test]
fn running_document_request_cancel_reaches_analysis_snapshot() {
    let mut state = initialized_state();
    state.trace = super::super::lifecycle::TraceValue::Verbose;
    let uri = temp_file_uri("server_canceled_analysis_snapshot", "fn main() void {}\n");
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);
    let started = std::sync::Arc::new(std::sync::Barrier::new(2));
    let release = std::sync::Arc::new(std::sync::Barrier::new(2));

    execute_document_request(
        &mut state,
        &mut writer,
        json!(106),
        &uri,
        SchedulerLane::Interactive,
        "textDocument/documentSymbol",
        {
            let started = started.clone();
            let release = release.clone();
            let uri = uri.clone();
            move |analysis, snapshot| {
                started.wait();
                release.wait();
                analysis
                    .document_symbols_in_snapshot(snapshot, &uri)
                    .map(|_| json!({ "ok": true }))
            }
        },
    )
    .unwrap();

    started.wait();
    state.cancel_request(json!(106));
    release.wait();
    flush_document_request_results(&mut state, &mut writer, true).unwrap();

    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["method"], "$/logTrace");
    assert_eq!(messages[0]["params"]["message"], "request canceled");
    assert_eq!(messages[1]["id"], json!(106));
    assert_eq!(messages[1]["error"]["code"], json!(REQUEST_CANCELLED));
    assert_eq!(messages[1]["error"]["message"], "request was canceled");
    assert_eq!(state.analysis.last_analysis_tier(), None);
}

#[test]
fn analysis_cancellation_text_alone_is_not_reclassified_as_lsp_cancellation() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_analysis_cancel_error", "fn main() void {}\n");
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    execute_document_request::<Value, _>(
        &mut state,
        &mut writer,
        json!(107),
        &uri,
        SchedulerLane::Interactive,
        "textDocument/hover",
        |_, _| Err("request was canceled".to_string()),
    )
    .unwrap();
    flush_document_request_results(&mut state, &mut writer, true).unwrap();

    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["id"], json!(107));
    assert_eq!(messages[0]["error"]["code"], json!(INVALID_REQUEST));
    assert_eq!(messages[0]["error"]["message"], "request was canceled");
}

#[test]
fn panicking_document_request_returns_error_response() {
    let mut state = initialized_state();
    state.trace = super::super::lifecycle::TraceValue::Verbose;
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
        "textDocument/hover",
        |_, _| panic!("synthetic analysis panic"),
    )
    .unwrap();
    flush_document_request_results(&mut state, &mut writer, true).unwrap();
    std::panic::set_hook(previous_hook);

    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["id"], json!(47));
    assert!(
        messages[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("synthetic analysis panic")
    );
    assert_eq!(messages[1]["method"], "$/logTrace");
    assert_eq!(messages[1]["params"]["message"], "analysis tier selected");
    let verbose = messages[1]["params"]["verbose"].as_str().unwrap();
    assert!(verbose.contains("request_id=47"), "{verbose}");
    assert!(verbose.contains("document_generation=None"), "{verbose}");
    assert!(verbose.contains("document_version=None"), "{verbose}");
    assert!(verbose.contains("snapshot_generation="), "{verbose}");
    assert!(verbose.contains("cache=none"), "{verbose}");
    assert!(verbose.contains("error_class=InternalBug"), "{verbose}");
}

#[test]
fn document_request_runs_on_worker_thread() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_worker_request", "fn main() void {}\n");
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);
    let caller_thread = std::thread::current().id();
    let ran_on_worker = std::sync::Arc::new(std::sync::Mutex::new(None));
    let observed = ran_on_worker.clone();

    execute_document_request(
        &mut state,
        &mut writer,
        json!(48),
        &uri,
        SchedulerLane::Interactive,
        "textDocument/hover",
        move |_, _| {
            *observed.lock().unwrap() = Some(std::thread::current().id() != caller_thread);
            Ok::<Value, String>(json!({ "ok": true }))
        },
    )
    .unwrap();
    flush_document_request_results(&mut state, &mut writer, true).unwrap();

    assert_eq!(*ran_on_worker.lock().unwrap(), Some(true));
    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["id"], json!(48));
}

#[test]
fn document_requests_can_be_in_flight_together_before_flush() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_parallel_request", "fn main() void {}\n");
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);
    let started = std::sync::Arc::new(std::sync::Barrier::new(3));
    let release = std::sync::Arc::new(std::sync::Barrier::new(3));

    for id in [60, 61] {
        let started = started.clone();
        let release = release.clone();
        execute_document_request(
            &mut state,
            &mut writer,
            json!(id),
            &uri,
            SchedulerLane::Interactive,
            "textDocument/hover",
            move |_, _| {
                started.wait();
                release.wait();
                Ok::<Value, String>(json!({ "id": id }))
            },
        )
        .unwrap();
    }

    assert_eq!(state.pending_document_request_tasks, 2);
    started.wait();
    release.wait();
    flush_document_request_results(&mut state, &mut writer, true).unwrap();
    flush_document_request_results(&mut state, &mut writer, true).unwrap();

    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 2);
    assert_eq!(state.pending_document_request_tasks, 0);
    let mut ids = messages
        .iter()
        .map(|message| message["id"].as_i64().unwrap())
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, vec![60, 61]);
}

#[test]
fn document_request_worker_pool_limits_running_tasks() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_bounded_worker_request", "fn main() void {}\n");
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);
    let release = std::sync::Arc::new(std::sync::Barrier::new(5));
    let running = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let max_running = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    for id in 70..78 {
        let release = release.clone();
        let running = running.clone();
        let max_running = max_running.clone();
        execute_document_request(
            &mut state,
            &mut writer,
            json!(id),
            &uri,
            SchedulerLane::Interactive,
            "textDocument/hover",
            move |_, _| {
                let current = running.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                max_running.fetch_max(current, std::sync::atomic::Ordering::SeqCst);
                if id < 74 {
                    release.wait();
                }
                running.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                Ok::<Value, String>(json!({ "id": id }))
            },
        )
        .unwrap();
    }

    while max_running.load(std::sync::atomic::Ordering::SeqCst) < 4 {
        std::thread::yield_now();
    }
    assert_eq!(max_running.load(std::sync::atomic::Ordering::SeqCst), 4);
    release.wait();
    while state.has_pending_document_request_work() {
        flush_document_request_results(&mut state, &mut writer, true).unwrap();
    }

    assert_eq!(max_running.load(std::sync::atomic::Ordering::SeqCst), 4);
    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 8);
}

#[test]
fn configured_document_request_worker_pool_limits_running_tasks() {
    let mut state = ServerState::with_options(
        AnalysisEngine::default(),
        ServerOptions { worker_threads: 2 },
    );
    state.initialized = true;
    let uri = temp_file_uri("server_configured_worker_request", "fn main() void {}\n");
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);
    let release = std::sync::Arc::new(std::sync::Barrier::new(3));
    let running = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let max_running = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    for id in 80..84 {
        let release = release.clone();
        let running = running.clone();
        let max_running = max_running.clone();
        execute_document_request(
            &mut state,
            &mut writer,
            json!(id),
            &uri,
            SchedulerLane::Interactive,
            "textDocument/hover",
            move |_, _| {
                let current = running.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                max_running.fetch_max(current, std::sync::atomic::Ordering::SeqCst);
                if id < 82 {
                    release.wait();
                }
                running.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                Ok::<Value, String>(json!({ "id": id }))
            },
        )
        .unwrap();
    }

    while max_running.load(std::sync::atomic::Ordering::SeqCst) < 2 {
        std::thread::yield_now();
    }
    assert_eq!(max_running.load(std::sync::atomic::Ordering::SeqCst), 2);
    release.wait();
    while state.has_pending_document_request_work() {
        flush_document_request_results(&mut state, &mut writer, true).unwrap();
    }

    assert_eq!(max_running.load(std::sync::atomic::Ordering::SeqCst), 2);
    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 4);
}

#[test]
fn optional_document_request_none_returns_null_response() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_optional_null_request", "fn main() void {}\n");
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    execute_optional_document_request::<Value, _>(
        &mut state,
        &mut writer,
        json!(49),
        &uri,
        SchedulerLane::Interactive,
        "textDocument/hover",
        |_, _| Ok(None),
    )
    .unwrap();
    flush_document_request_results(&mut state, &mut writer, true).unwrap();

    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["id"], json!(49));
    assert!(messages[0]["result"].is_null());
}

#[test]
fn stale_document_request_generation_drops_response() {
    let mut state = initialized_state();
    state.trace = super::super::lifecycle::TraceValue::Verbose;
    let uri = temp_file_uri("server_stale_request", "fn main() void {}\n");
    let _current = state.begin_target_analysis(&uri);
    let request = state.request_context_for_document(json!(43), &uri);
    let _newer = state.begin_target_analysis(&uri);
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    write_success_response(&mut state, &mut writer, &request, json!({ "ok": true })).unwrap();

    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["method"], "$/logTrace");
    assert_eq!(messages[0]["params"]["message"], "stale response dropped");
    let verbose = messages[0]["params"]["verbose"].as_str().unwrap();
    assert!(verbose.contains("request_id=43"), "{verbose}");
    assert!(verbose.contains("document_generation=1"), "{verbose}");
    assert!(verbose.contains("document_version=None"), "{verbose}");
    assert!(verbose.contains("snapshot_generation=None"), "{verbose}");
    assert!(verbose.contains("status=stale"), "{verbose}");
    assert!(verbose.contains("cache=none"), "{verbose}");
}

#[test]
fn canceled_standalone_error_response_is_still_written() {
    let mut state = initialized_state();
    state.cancel_request(json!(45));
    let request = state.request_context(json!(45));
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    write_error_response(
        &mut state,
        &mut writer,
        &request,
        REQUEST_CANCELLED,
        "request was canceled",
    )
    .unwrap();

    let messages = read_all_messages(&output);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["id"], json!(45));
    assert_eq!(messages[0]["error"]["code"], json!(REQUEST_CANCELLED));
}

#[test]
fn stale_document_request_task_result_drops_response() {
    let mut state = initialized_state();
    let uri = temp_file_uri("server_stale_task_result", "fn main() void {}\n");
    let stale_generation = state.begin_target_analysis(&uri);
    let request = RequestContext {
        id: json!(48),
        target_uri: Some(uri.clone()),
        generation: Some(stale_generation),
        cancellation: None,
        work_done_token: None,
    };
    let _newer = state.begin_target_analysis(&uri);
    let mut output = Vec::new();
    let mut writer = MessageWriter::new(&mut output);

    submit_document_request_result(
        &mut state,
        &mut writer,
        DocumentRequestTaskResult {
            request,
            target_uri: uri,
            lane: SchedulerLane::Interactive,
            method: "textDocument/hover".to_string(),
            trace: TraceContext::default(),
            queue_wait_ms: 0,
            elapsed_ms: 0,
            analysis_tier: None,
            analysis_trace: Default::default(),
            canceled: false,
            error_class: None,
            response: DocumentRequestResponse::Success(json!({ "ok": true })),
        },
    )
    .unwrap();

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
        cancellation: None,
        work_done_token: None,
    };

    assert!(state.should_skip_request(&stale_request));
}
