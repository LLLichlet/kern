use super::state::RequestBudgetKind;
use super::{
    AnalysisEngine, AnalysisGeneration, DiagnosticsAnalysisMode, DiagnosticsTaskResult,
    DocumentRequestResponse, DocumentRequestTaskResult, INVALID_REQUEST, LspErrorClass,
    LspWorkerTask, REQUEST_CANCELLED, RequestContext, ScheduledDocumentRequestTask, SchedulerLane,
    ServerError, ServerState, TraceContext, WorkspaceRefreshKind, WorkspaceRefreshTaskResult,
    lifecycle::emit_trace,
};
use crate::analysis::{
    AnalysisOutcome, AnalysisSnapshot, CancellationToken, DocumentSyncAction, cleared_uris,
};
use crate::protocol::{
    WorkDoneProgressValue, error_response, null_response, progress, publish_diagnostics,
    success_response, work_done_progress_create,
};
use crate::transport::MessageWriter;
use serde_json::Value;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

pub(super) fn publish_analysis_outcome(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    target_uri: &str,
    generation: AnalysisGeneration,
    outcome: AnalysisOutcome,
) -> Result<(), ServerError> {
    if !state.is_current_generation(target_uri, generation) {
        return Ok(());
    }

    for bundle in &outcome.bundles {
        writer.write_json(&publish_diagnostics(
            bundle.uri.clone(),
            bundle
                .diagnostics
                .clone()
                .into_iter()
                .map(crate::analysis::ide::IdeDiagnostic::into_lsp)
                .collect(),
        ))?;
    }

    let previous = state
        .published_by_target
        .get(target_uri)
        .cloned()
        .unwrap_or_default();
    for uri in cleared_uris(&previous, &outcome.bundles) {
        writer.write_json(&publish_diagnostics(uri, Vec::new()))?;
    }

    let current = outcome
        .bundles
        .iter()
        .map(|bundle| bundle.uri.clone())
        .collect();
    state
        .published_by_target
        .insert(target_uri.to_string(), current);

    Ok(())
}

pub(super) fn flush_diagnostics_lane(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
) -> Result<(), ServerError> {
    flush_workspace_refresh_results(state, writer, false)?;
    flush_diagnostics_results(state, writer, false)?;

    if let Some(reason) = state.pending_workspace_refresh_reason.take() {
        let kind = state
            .pending_workspace_refresh_kind
            .take()
            .unwrap_or(WorkspaceRefreshKind::Sources);
        submit_workspace_refresh_task(state, writer, reason, kind)?;
    }
    if state.pending_workspace_refresh_reason.is_none() {
        let target_task_budget = state.diagnostics_flush_policy.target_task_budget.max(1);
        let mut submitted_targets = 0;
        while let Some((target_uri, task)) = state.pop_next_diagnostics_target() {
            let mode = task.mode;
            let generation = task.generation;
            if !state.is_current_generation(&target_uri, generation) {
                continue;
            }
            submit_diagnostics_task(state, target_uri, generation, mode);
            submitted_targets += 1;
            if submitted_targets >= target_task_budget {
                break;
            }
        }
    }

    flush_workspace_refresh_results(state, writer, false)?;
    flush_diagnostics_results(state, writer, false)?;

    let pending = std::mem::take(&mut state.pending_diagnostics);
    for (target_uri, publish) in pending {
        publish_analysis_outcome(
            state,
            writer,
            &target_uri,
            publish.generation,
            publish.outcome,
        )?;
    }
    Ok(())
}

fn submit_workspace_refresh_task(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    reason: String,
    kind: WorkspaceRefreshKind,
) -> Result<(), ServerError> {
    let mut analysis = state.analysis.clone();
    let workspace_root = state.workspace_root.clone();
    let queued_at = Instant::now();
    let progress_token = emit_workspace_refresh_progress_begin(state, writer, &reason)?;
    state.queue_workspace_refresh_worker_task();
    let task = LspWorkerTask::WorkspaceRefresh(Box::new(move || {
        analysis.clear_last_analysis_trace();
        let started_at = Instant::now();
        let refresh = catch_unwind(AssertUnwindSafe(|| match kind {
            WorkspaceRefreshKind::Sources => analysis
                .refresh_workspace_index_cancelable(workspace_root, CancellationToken::new()),
            WorkspaceRefreshKind::ProjectMetadata => analysis
                .reload_project_metadata_index_cancelable(workspace_root, CancellationToken::new()),
        }))
        .map_err(|payload| panic_message(payload.as_ref()))
        .and_then(|result| result);
        let error_class = refresh
            .as_ref()
            .err()
            .map(|message| classify_lsp_error(message));
        WorkspaceRefreshTaskResult {
            reason,
            progress_token,
            trace: TraceContext::default(),
            queue_wait_ms: started_at.duration_since(queued_at).as_millis(),
            elapsed_ms: started_at.elapsed().as_millis(),
            analysis_trace: analysis.last_analysis_trace(),
            error_class,
            refresh,
        }
    }));
    if state.worker_task_tx.send(task).is_err() {
        state.complete_workspace_refresh_worker_task();
    }
    Ok(())
}

fn submit_diagnostics_task(
    state: &mut ServerState,
    target_uri: String,
    generation: AnalysisGeneration,
    mode: DiagnosticsAnalysisMode,
) {
    let analysis = state.analysis.clone();
    let queued_at = Instant::now();
    state.queue_diagnostics_worker_task();
    let task = LspWorkerTask::Diagnostics(Box::new(move || {
        run_diagnostics_task(analysis, target_uri, generation, mode, queued_at)
    }));
    if state.worker_task_tx.send(task).is_err() {
        state.complete_diagnostics_worker_task();
    }
}

fn run_diagnostics_task(
    analysis: AnalysisEngine,
    target_uri: String,
    generation: AnalysisGeneration,
    mode: DiagnosticsAnalysisMode,
    queued_at: Instant,
) -> DiagnosticsTaskResult {
    analysis.clear_last_analysis_tier();
    analysis.clear_last_analysis_trace();
    analysis.start_analysis_trace();
    let started_at = Instant::now();
    let outcome = match catch_unwind(AssertUnwindSafe(|| match mode {
        DiagnosticsAnalysisMode::Structure => analysis.analyze_document_structure_uri(&target_uri),
        DiagnosticsAnalysisMode::Full => analysis.analyze_document_uri(&target_uri),
    })) {
        Ok(outcome) => outcome,
        Err(payload) => {
            let message = format!(
                "kern-lsp analysis panicked: {}",
                panic_message(payload.as_ref())
            );
            crate::analysis::single_server_diagnostic(target_uri.clone(), message)
        }
    };
    let error_class = diagnostic_outcome_error_class(&outcome);
    let document_version = analysis.document_version(&target_uri);
    DiagnosticsTaskResult {
        target_uri,
        generation,
        mode,
        trace: TraceContext {
            request_id: None,
            document_generation: Some(generation),
            document_version,
        },
        queue_wait_ms: started_at.duration_since(queued_at).as_millis(),
        elapsed_ms: started_at.elapsed().as_millis(),
        analysis_tier: analysis.last_analysis_tier(),
        analysis_trace: analysis.last_analysis_trace(),
        error_class,
        outcome,
    }
}

pub(super) fn flush_workspace_refresh_results(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    wait_for_ready: bool,
) -> Result<(), ServerError> {
    if wait_for_ready && state.pending_workspace_refresh_tasks > 0 {
        let result = state.workspace_refresh_results_rx.recv().map_err(|err| {
            ServerError::Protocol(format!("workspace refresh result channel closed: {err}"))
        })?;
        state.complete_workspace_refresh_worker_task();
        submit_workspace_refresh_result(state, writer, result)?;
    }

    while let Ok(result) = state.workspace_refresh_results_rx.try_recv() {
        state.complete_workspace_refresh_worker_task();
        submit_workspace_refresh_result(state, writer, result)?;
    }

    Ok(())
}

fn submit_workspace_refresh_result(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    result: WorkspaceRefreshTaskResult,
) -> Result<(), ServerError> {
    match result.refresh {
        Ok(refresh) => {
            let target_count = refresh.targets.len();
            let indexed_targets = refresh.indexed_targets;
            let failed_targets = refresh.failed_targets;
            let generation = refresh.generation;
            for (target_uri, mode) in refresh.targets {
                let generation = state.begin_target_analysis(&target_uri);
                state.queue_target_diagnostics_task(target_uri, generation, mode);
            }
            emit_workspace_refresh_queued_trace(
                state,
                writer,
                result.progress_token,
                &result.reason,
                target_count,
                indexed_targets,
                failed_targets,
                generation,
                result.queue_wait_ms,
                result.elapsed_ms,
                &result.trace,
                &result.analysis_trace,
                result.error_class,
            )
        }
        Err(message) => {
            let fallback_targets = state.analysis.document_uris();
            let target_count = fallback_targets.len();
            for target_uri in fallback_targets {
                let generation = state.begin_target_analysis(&target_uri);
                state.queue_diagnostics_publish(
                    target_uri.clone(),
                    generation,
                    crate::analysis::single_server_diagnostic(
                        target_uri,
                        format!("kern-lsp analysis panicked: {message}"),
                    ),
                );
            }
            emit_workspace_refresh_queued_trace(
                state,
                writer,
                result.progress_token,
                &result.reason,
                target_count,
                0,
                1,
                0,
                result.queue_wait_ms,
                result.elapsed_ms,
                &result.trace,
                &result.analysis_trace,
                result.error_class,
            )
        }
    }
}

pub(super) fn flush_diagnostics_results(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    wait_for_ready: bool,
) -> Result<(), ServerError> {
    if wait_for_ready && state.pending_diagnostics_worker_tasks > 0 {
        let result = state.diagnostics_results_rx.recv().map_err(|err| {
            ServerError::Protocol(format!("diagnostics result channel closed: {err}"))
        })?;
        state.complete_diagnostics_worker_task();
        submit_diagnostics_result(state, writer, result)?;
    }

    while let Ok(result) = state.diagnostics_results_rx.try_recv() {
        state.complete_diagnostics_worker_task();
        submit_diagnostics_result(state, writer, result)?;
    }

    Ok(())
}

fn submit_diagnostics_result(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    result: DiagnosticsTaskResult,
) -> Result<(), ServerError> {
    if !state.is_current_generation(&result.target_uri, result.generation) {
        return Ok(());
    }
    emit_diagnostics_analysis_trace(
        state,
        writer,
        &result.target_uri,
        result.mode,
        result.queue_wait_ms,
        result.elapsed_ms,
        result.analysis_tier,
        &result.trace,
        &result.analysis_trace,
        result.error_class,
    )?;
    state.queue_diagnostics_publish(result.target_uri, result.generation, result.outcome);
    Ok(())
}

pub(super) fn drain_scheduler(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
) -> Result<(), ServerError> {
    flush_document_request_results(state, writer, false)?;
    flush_diagnostics_lane(state, writer)
}

pub(super) fn execute_document_diagnostics<F>(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    target_uri: &str,
    _lane: SchedulerLane,
    action: F,
) -> Result<(), ServerError>
where
    F: FnOnce(&mut AnalysisEngine) -> DocumentSyncAction,
{
    state.mark_active_document(target_uri);
    let generation = state.begin_target_analysis(target_uri);
    let result = catch_unwind(AssertUnwindSafe(|| action(&mut state.analysis)));
    match result {
        Ok(DocumentSyncAction::ScheduleTarget { uri, mode }) => {
            state
                .latest_generation_by_target
                .insert(uri.clone(), generation);
            state.queue_target_diagnostics_task(uri, generation, mode);
        }
        Ok(DocumentSyncAction::Immediate(outcome)) => {
            state.queue_diagnostics_publish(target_uri.to_string(), generation, outcome);
        }
        Err(payload) => {
            let message = panic_message(payload.as_ref());
            state.queue_diagnostics_publish(
                target_uri.to_string(),
                generation,
                crate::analysis::single_server_diagnostic(
                    target_uri.to_string(),
                    format!("kern-lsp analysis panicked: {message}"),
                ),
            );
        }
    }
    let _ = writer;
    Ok(())
}

pub(super) fn execute_workspace_diagnostics_refresh(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    reason: &str,
    _lane: SchedulerLane,
    kind: WorkspaceRefreshKind,
) -> Result<(), ServerError> {
    state.queue_workspace_refresh_task(reason.to_string(), kind);
    let _ = writer;
    Ok(())
}

pub(super) fn execute_document_request<T, F>(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    id: Value,
    target_uri: &str,
    lane: SchedulerLane,
    method: &str,
    analysis: F,
) -> Result<(), ServerError>
where
    T: serde::Serialize,
    F: FnOnce(&AnalysisEngine, &AnalysisSnapshot) -> Result<T, String> + Send + 'static,
{
    state.mark_active_document(target_uri);
    let mut request = state.request_context_for_document(id, target_uri);
    if state.should_skip_request(&request) {
        return Ok(());
    }
    let was_canceled_before_registration = state.take_pending_cancel(&request.id);
    state.register_request_cancellation(&mut request);
    if was_canceled_before_registration {
        if let Some(cancellation) = &request.cancellation {
            cancellation.cancel();
        }
    }

    submit_document_request_task(
        state,
        ScheduledDocumentRequestTask {
            request,
            target_uri: target_uri.to_string(),
            lane,
            method: method.to_string(),
            queued_at: Instant::now(),
        },
        state.workspace_root.clone(),
        |engine, snapshot| {
            analysis(engine, snapshot)
                .and_then(|result| {
                    serde_json::to_value(result)
                        .map_err(|err| format!("failed to encode response: {err}"))
                })
                .map(DocumentRequestResponse::Success)
        },
    );
    let _ = writer;
    Ok(())
}

pub(super) fn execute_document_request_with_progress<T, F>(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    id: Value,
    target_uri: &str,
    lane: SchedulerLane,
    method: &str,
    work_done_token: Option<Value>,
    progress_title: &str,
    progress_message: &str,
    analysis: F,
) -> Result<(), ServerError>
where
    T: serde::Serialize,
    F: FnOnce(&AnalysisEngine, &AnalysisSnapshot) -> Result<T, String> + Send + 'static,
{
    state.mark_active_document(target_uri);
    let mut request = state.request_context_for_document(id, target_uri);
    if state.should_skip_request(&request) {
        return Ok(());
    }
    let was_canceled_before_registration = state.take_pending_cancel(&request.id);
    state.register_request_cancellation(&mut request);
    if was_canceled_before_registration {
        if let Some(cancellation) = &request.cancellation {
            cancellation.cancel();
        }
    }
    if let Some(token) = work_done_token
        && state.work_done_progress
    {
        writer.write_json(&progress(
            token.clone(),
            WorkDoneProgressValue::Begin {
                title: progress_title.to_string(),
                message: progress_message.to_string(),
                percentage: None,
            },
        ))?;
        request.work_done_token = Some(token);
    }

    submit_document_request_task(
        state,
        ScheduledDocumentRequestTask {
            request,
            target_uri: target_uri.to_string(),
            lane,
            method: method.to_string(),
            queued_at: Instant::now(),
        },
        state.workspace_root.clone(),
        |engine, snapshot| {
            analysis(engine, snapshot)
                .and_then(|result| {
                    serde_json::to_value(result)
                        .map_err(|err| format!("failed to encode response: {err}"))
                })
                .map(DocumentRequestResponse::Success)
        },
    );
    Ok(())
}

pub(super) fn execute_request_with_progress<T, F>(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    id: Value,
    target_label: &str,
    lane: SchedulerLane,
    method: &str,
    work_done_token: Option<Value>,
    progress_title: &str,
    progress_message: &str,
    analysis: F,
) -> Result<(), ServerError>
where
    T: serde::Serialize,
    F: FnOnce(&AnalysisEngine, &AnalysisSnapshot) -> Result<T, String> + Send + 'static,
{
    let mut request = state.request_context(id);
    let was_canceled_before_registration = state.take_pending_cancel(&request.id);
    state.register_request_cancellation(&mut request);
    if was_canceled_before_registration {
        if let Some(cancellation) = &request.cancellation {
            cancellation.cancel();
        }
    }
    if let Some(token) = work_done_token
        && state.work_done_progress
    {
        writer.write_json(&progress(
            token.clone(),
            WorkDoneProgressValue::Begin {
                title: progress_title.to_string(),
                message: progress_message.to_string(),
                percentage: None,
            },
        ))?;
        request.work_done_token = Some(token);
    }

    submit_document_request_task(
        state,
        ScheduledDocumentRequestTask {
            request,
            target_uri: target_label.to_string(),
            lane,
            method: method.to_string(),
            queued_at: Instant::now(),
        },
        state.workspace_root.clone(),
        |engine, snapshot| {
            analysis(engine, snapshot)
                .and_then(|result| {
                    serde_json::to_value(result)
                        .map_err(|err| format!("failed to encode response: {err}"))
                })
                .map(DocumentRequestResponse::Success)
        },
    );
    Ok(())
}

fn submit_document_request_task<F>(
    state: &mut ServerState,
    task_info: ScheduledDocumentRequestTask,
    workspace_root: Option<std::path::PathBuf>,
    task: F,
) where
    F: FnOnce(&AnalysisEngine, &AnalysisSnapshot) -> Result<DocumentRequestResponse, String>
        + Send
        + 'static,
{
    state.analysis.clear_last_analysis_tier();
    state.analysis.clear_last_analysis_trace();
    let analysis = state.analysis.clone();
    let cancellation = task_info
        .request
        .cancellation
        .as_ref()
        .map(|token| token.analysis_token())
        .unwrap_or_else(CancellationToken::new);
    let snapshot = analysis.snapshot(workspace_root, cancellation);
    let trace = TraceContext {
        request_id: Some(task_info.request.id.clone()),
        document_generation: task_info.request.generation,
        document_version: task_info
            .request
            .target_uri
            .as_deref()
            .and_then(|uri| snapshot.document_version(uri)),
    };
    state.queue_document_request_task();
    let task = LspWorkerTask::DocumentRequest(Box::new(move || {
        let result = run_document_request_task(analysis, snapshot, trace, task_info, task);
        result
    }));
    if state.worker_task_tx.send(task).is_err() {
        state.complete_document_request_task();
    }
}

fn run_document_request_task<F>(
    analysis: AnalysisEngine,
    snapshot: AnalysisSnapshot,
    trace: TraceContext,
    task_info: ScheduledDocumentRequestTask,
    task: F,
) -> DocumentRequestTaskResult
where
    F: FnOnce(&AnalysisEngine, &AnalysisSnapshot) -> Result<DocumentRequestResponse, String> + Send,
{
    let started_at = Instant::now();
    let canceled = task_info.request.is_canceled();
    let result = if canceled {
        Ok(Ok(DocumentRequestResponse::Null))
    } else {
        catch_unwind(AssertUnwindSafe(|| task(&analysis, &snapshot)))
    };
    let elapsed_ms = started_at.elapsed().as_millis();
    let analysis_tier = analysis.last_analysis_tier();
    let (response, error_class) = match result {
        Ok(Ok(response)) => (response, None),
        Ok(Err(message)) => {
            let error_class = classify_lsp_error(&message);
            (
                DocumentRequestResponse::Error {
                    code: INVALID_REQUEST,
                    message,
                },
                Some(error_class),
            )
        }
        Err(payload) => (
            DocumentRequestResponse::Error {
                code: INVALID_REQUEST,
                message: format!(
                    "kern-lsp analysis panicked: {}",
                    panic_message(payload.as_ref())
                ),
            },
            Some(LspErrorClass::InternalBug),
        ),
    };
    DocumentRequestTaskResult {
        request: task_info.request,
        target_uri: task_info.target_uri,
        lane: task_info.lane,
        method: task_info.method,
        trace,
        queue_wait_ms: started_at.duration_since(task_info.queued_at).as_millis(),
        elapsed_ms,
        analysis_tier,
        analysis_trace: analysis.last_analysis_trace(),
        canceled,
        error_class,
        response,
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "unknown panic payload".to_string()
}

fn classify_lsp_error(message: &str) -> LspErrorClass {
    if message.contains("request was canceled") {
        return LspErrorClass::RequestCanceled;
    }
    if message.contains("kern-lsp analysis panicked") {
        return LspErrorClass::InternalBug;
    }
    if message.contains("failed to parse `") || message.contains("invalid Craft") {
        return LspErrorClass::ProjectInvalid;
    }
    if message.contains("failed to resolve Craft project")
        || message.contains("no Craft project")
        || message.contains("project unavailable")
    {
        return LspErrorClass::ProjectUnavailable;
    }
    if message.contains("failed to encode response") {
        return LspErrorClass::ProtocolError;
    }
    LspErrorClass::AnalysisFailed
}

fn diagnostic_outcome_error_class(outcome: &AnalysisOutcome) -> Option<LspErrorClass> {
    outcome
        .bundles
        .iter()
        .flat_map(|bundle| bundle.diagnostics.iter())
        .find_map(|diagnostic| {
            diagnostic
                .message
                .starts_with("kern-lsp analysis panicked:")
                .then_some(LspErrorClass::InternalBug)
        })
}

pub(super) fn execute_optional_document_request<T, F>(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    id: Value,
    target_uri: &str,
    lane: SchedulerLane,
    method: &str,
    analysis: F,
) -> Result<(), ServerError>
where
    T: serde::Serialize,
    F: FnOnce(&AnalysisEngine, &AnalysisSnapshot) -> Result<Option<T>, String> + Send + 'static,
{
    state.mark_active_document(target_uri);
    let mut request = state.request_context_for_document(id, target_uri);
    if state.should_skip_request(&request) {
        return Ok(());
    }
    let was_canceled_before_registration = state.take_pending_cancel(&request.id);
    state.register_request_cancellation(&mut request);
    if was_canceled_before_registration {
        if let Some(cancellation) = &request.cancellation {
            cancellation.cancel();
        }
    }

    submit_document_request_task(
        state,
        ScheduledDocumentRequestTask {
            request,
            target_uri: target_uri.to_string(),
            lane,
            method: method.to_string(),
            queued_at: Instant::now(),
        },
        state.workspace_root.clone(),
        |engine, snapshot| match analysis(engine, snapshot)? {
            Some(result) => serde_json::to_value(result)
                .map(DocumentRequestResponse::Success)
                .map_err(|err| format!("failed to encode response: {err}")),
            None => Ok(DocumentRequestResponse::Null),
        },
    );
    let _ = writer;
    Ok(())
}

pub(super) fn flush_document_request_results(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    wait_for_ready: bool,
) -> Result<(), ServerError> {
    if wait_for_ready && state.has_pending_document_request_work() {
        let result = state
            .document_request_results_rx
            .recv()
            .map_err(|err| ServerError::Protocol(format!("worker result channel closed: {err}")))?;
        state.complete_document_request_task();
        submit_document_request_result(state, writer, result)?;
    }

    while let Ok(result) = state.document_request_results_rx.try_recv() {
        state.complete_document_request_task();
        submit_document_request_result(state, writer, result)?;
    }

    Ok(())
}

pub(super) fn submit_document_request_result(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    result: DocumentRequestTaskResult,
) -> Result<(), ServerError> {
    if result.request.is_canceled() || result.canceled {
        emit_request_canceled_trace(state, writer, &result)?;
        emit_request_progress_end(writer, &result.request, "Canceled")?;
        write_error_response(
            state,
            writer,
            &result.request,
            REQUEST_CANCELLED,
            "request was canceled",
        )?;
        return Ok(());
    }
    match result.response {
        DocumentRequestResponse::Success(value) => {
            emit_request_progress_end(writer, &result.request, "Complete")?;
            write_success_response(state, writer, &result.request, value)?
        }
        DocumentRequestResponse::Null => {
            emit_request_progress_end(writer, &result.request, "Complete")?;
            write_null_response(state, writer, &result.request)?
        }
        DocumentRequestResponse::Error { code, message } => {
            emit_request_progress_end(writer, &result.request, "Failed")?;
            write_error_response(state, writer, &result.request, code, message)?
        }
    }
    emit_analysis_tier_trace(
        state,
        writer,
        &result.target_uri,
        result.lane,
        &result.method,
        result.queue_wait_ms,
        result.elapsed_ms,
        result.analysis_tier,
        &result.trace,
        &result.analysis_trace,
        result.error_class,
    )
}

fn emit_request_progress_end(
    writer: &mut MessageWriter<impl io::Write>,
    request: &RequestContext,
    message: &str,
) -> Result<(), ServerError> {
    let Some(token) = &request.work_done_token else {
        return Ok(());
    };
    writer.write_json(&progress(
        token.clone(),
        WorkDoneProgressValue::End {
            message: message.to_string(),
        },
    ))?;
    Ok(())
}

fn emit_request_canceled_trace(
    state: &ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    result: &DocumentRequestTaskResult,
) -> Result<(), ServerError> {
    let trace_fields = format_trace_context(&result.trace, &result.analysis_trace);
    emit_trace(
        state,
        writer,
        "request canceled",
        Some(format!(
            "{} queue_wait_ms={} elapsed_ms={} status=canceled lane={:?} method={} target={} cache={} error_class={}",
            trace_fields,
            result.queue_wait_ms,
            result.elapsed_ms,
            result.lane,
            result.method,
            result.target_uri,
            result.analysis_trace.cache_summary(),
            LspErrorClass::RequestCanceled.as_str()
        )),
        true,
    )
}

fn emit_analysis_tier_trace(
    state: &ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    target_uri: &str,
    lane: SchedulerLane,
    method: &str,
    queue_wait_ms: u128,
    elapsed_ms: u128,
    analysis_tier: Option<crate::analysis::AnalysisTier>,
    trace: &TraceContext,
    analysis_trace: &crate::analysis::AnalysisTrace,
    error_class: Option<LspErrorClass>,
) -> Result<(), ServerError> {
    if analysis_tier.is_none() && error_class.is_none() {
        return Ok(());
    }
    let tier = analysis_tier.map(|tier| tier.as_str()).unwrap_or("none");
    let budget_status = state
        .request_budget_policy
        .status(RequestBudgetKind::Interactive, elapsed_ms)
        .as_str();
    let error_class = error_class.map(LspErrorClass::as_str).unwrap_or("None");
    let trace_fields = format_trace_context(trace, analysis_trace);
    emit_trace(
        state,
        writer,
        "analysis tier selected",
        Some(format!(
            "{} tier={} queue_wait_ms={} elapsed_ms={} status=completed budget={} lane={:?} method={} target={} cache={} error_class={}",
            trace_fields,
            tier,
            queue_wait_ms,
            elapsed_ms,
            budget_status,
            lane,
            method,
            target_uri,
            analysis_trace.cache_summary(),
            error_class
        )),
        true,
    )
}

fn emit_diagnostics_analysis_trace(
    state: &ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    target_uri: &str,
    mode: DiagnosticsAnalysisMode,
    queue_wait_ms: u128,
    elapsed_ms: u128,
    analysis_tier: Option<crate::analysis::AnalysisTier>,
    trace: &TraceContext,
    analysis_trace: &crate::analysis::AnalysisTrace,
    error_class: Option<LspErrorClass>,
) -> Result<(), ServerError> {
    let tier = analysis_tier.map(|tier| tier.as_str());
    let error_class = error_class.map(LspErrorClass::as_str).unwrap_or("None");
    let trace_fields = format_trace_context(trace, analysis_trace);
    let mut verbose = format!(
        "{} mode={:?} queue_wait_ms={} elapsed_ms={} status=completed budget={} lane={:?} target={} cache={} error_class={}",
        trace_fields,
        mode,
        queue_wait_ms,
        elapsed_ms,
        state
            .request_budget_policy
            .status(RequestBudgetKind::Diagnostics, elapsed_ms)
            .as_str(),
        SchedulerLane::Diagnostics,
        target_uri,
        analysis_trace.cache_summary(),
        error_class
    );
    if let Some(tier) = tier {
        verbose.insert_str(0, &format!("tier={} ", tier));
    }
    emit_trace(
        state,
        writer,
        "diagnostics analysis completed",
        Some(verbose),
        true,
    )
}

fn emit_workspace_refresh_queued_trace(
    state: &ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    progress_token: Option<Value>,
    reason: &str,
    target_count: usize,
    indexed_targets: usize,
    failed_targets: usize,
    generation: u64,
    queue_wait_ms: u128,
    elapsed_ms: u128,
    trace: &TraceContext,
    analysis_trace: &crate::analysis::AnalysisTrace,
    error_class: Option<LspErrorClass>,
) -> Result<(), ServerError> {
    emit_workspace_refresh_progress_end(
        state,
        writer,
        progress_token,
        target_count,
        indexed_targets,
        failed_targets,
        elapsed_ms,
    )?;
    let error_class = error_class.map(LspErrorClass::as_str).unwrap_or("None");
    let trace_fields = format_trace_context(trace, analysis_trace);
    emit_trace(
        state,
        writer,
        "workspace refresh queued",
        Some(format!(
            "{} reason={} targets={} indexed_targets={} failed_targets={} index_generation={} queue_wait_ms={} elapsed_ms={} status=completed budget={} lane={:?} cache={} error_class={}",
            trace_fields,
            reason,
            target_count,
            indexed_targets,
            failed_targets,
            generation,
            queue_wait_ms,
            elapsed_ms,
            state
                .request_budget_policy
                .status(RequestBudgetKind::WorkspaceRefresh, elapsed_ms)
                .as_str(),
            SchedulerLane::Diagnostics,
            analysis_trace.cache_summary(),
            error_class
        )),
        true,
    )
}

fn format_trace_context(
    trace: &TraceContext,
    analysis_trace: &crate::analysis::AnalysisTrace,
) -> String {
    let request_id = trace
        .request_id
        .as_ref()
        .map(Value::to_string)
        .unwrap_or_else(|| "None".to_string());
    let document_generation = trace
        .document_generation
        .map(|generation| generation.0.to_string())
        .unwrap_or_else(|| "None".to_string());
    let document_version = trace
        .document_version
        .map(|version| version.to_string())
        .unwrap_or_else(|| "None".to_string());
    let snapshot_generation = analysis_trace
        .snapshot_generation
        .map(|generation| generation.to_string())
        .unwrap_or_else(|| "None".to_string());
    format!(
        "request_id={} document_generation={} document_version={} snapshot_generation={}",
        request_id, document_generation, document_version, snapshot_generation
    )
}

fn emit_workspace_refresh_progress_begin(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    reason: &str,
) -> Result<Option<Value>, ServerError> {
    if !state.work_done_progress {
        return Ok(None);
    }

    let token = state.next_progress_token("workspace-refresh");
    let request_id = state.next_server_request_id();
    writer.write_json(&work_done_progress_create(request_id, token.clone()))?;
    writer.write_json(&progress(
        token.clone(),
        WorkDoneProgressValue::Begin {
            title: "Kern workspace refresh".to_string(),
            message: reason.to_string(),
            percentage: None,
        },
    ))?;
    Ok(Some(token))
}

fn emit_workspace_refresh_progress_end(
    state: &ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    progress_token: Option<Value>,
    target_count: usize,
    indexed_targets: usize,
    failed_targets: usize,
    elapsed_ms: u128,
) -> Result<(), ServerError> {
    let Some(token) = progress_token else {
        return Ok(());
    };
    if !state.work_done_progress {
        return Ok(());
    }

    writer.write_json(&progress(
        token,
        WorkDoneProgressValue::End {
            message: format!(
                "refreshed {target_count} workspace targets, indexed {indexed_targets} targets, {failed_targets} index failures in {elapsed_ms}ms"
            ),
        },
    ))?;
    Ok(())
}

pub(super) fn write_success_response(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    request: &RequestContext,
    result: Value,
) -> Result<(), ServerError> {
    if state.should_drop_response(request) {
        emit_stale_response_trace(state, writer, request)?;
        return Ok(());
    }
    writer.write_json(&success_response(request.id.clone(), result))?;
    Ok(())
}

pub(super) fn write_null_response(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    request: &RequestContext,
) -> Result<(), ServerError> {
    if state.should_drop_response(request) {
        emit_stale_response_trace(state, writer, request)?;
        return Ok(());
    }
    writer.write_json(&null_response(request.id.clone()))?;
    Ok(())
}

pub(super) fn write_error_response(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    request: &RequestContext,
    code: i64,
    message: impl Into<String>,
) -> Result<(), ServerError> {
    if state.should_drop_response(request) {
        emit_stale_response_trace(state, writer, request)?;
        return Ok(());
    }
    writer.write_json(&error_response(request.id.clone(), code, message))?;
    Ok(())
}

fn emit_stale_response_trace(
    state: &ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    request: &RequestContext,
) -> Result<(), ServerError> {
    let trace = TraceContext {
        request_id: Some(request.id.clone()),
        document_generation: request.generation,
        document_version: request
            .target_uri
            .as_deref()
            .and_then(|uri| state.analysis.document_version(uri)),
    };
    let analysis_trace = crate::analysis::AnalysisTrace::default();
    let target = request.target_uri.as_deref().unwrap_or("None");
    emit_trace(
        state,
        writer,
        "stale response dropped",
        Some(format!(
            "{} status=stale target={} cache={}",
            format_trace_context(&trace, &analysis_trace),
            target,
            analysis_trace.cache_summary()
        )),
        true,
    )
}

pub(super) fn schedule_workspace_refresh(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    reason: &str,
    kind: WorkspaceRefreshKind,
) -> Result<(), ServerError> {
    execute_workspace_diagnostics_refresh(state, writer, reason, SchedulerLane::Diagnostics, kind)
}
