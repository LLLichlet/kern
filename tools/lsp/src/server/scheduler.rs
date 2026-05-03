use super::state::{RequestBudgetKind, RequestBudgetStatus};
use super::{
    AnalysisEngine, AnalysisGeneration, DiagnosticsAnalysisMode, INVALID_REQUEST, RequestContext,
    SchedulerLane, ServerError, ServerState, lifecycle::emit_trace,
};
use crate::analysis::{AnalysisOutcome, DocumentSyncAction, cleared_uris};
use crate::protocol::{error_response, null_response, publish_diagnostics, success_response};
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
            bundle.diagnostics.clone(),
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
    if let Some(reason) = state.pending_workspace_refresh_reason.take() {
        let started_at = Instant::now();
        let refresh = catch_unwind(AssertUnwindSafe(|| {
            state.analysis.refresh_workspace_targets()
        }));
        let elapsed_ms = started_at.elapsed().as_millis();
        match refresh {
            Ok(targets) => {
                let target_count = targets.len();
                for (target_uri, mode) in targets {
                    let generation = state.begin_target_analysis(&target_uri);
                    state.queue_target_diagnostics_task(target_uri, generation, mode);
                }
                emit_workspace_refresh_trace(state, writer, &reason, target_count, elapsed_ms)?;
            }
            Err(payload) => {
                let message = panic_message(payload.as_ref());
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
                emit_workspace_refresh_trace(state, writer, &reason, target_count, elapsed_ms)?;
            }
        }
    }
    if state.pending_workspace_refresh_reason.is_none() {
        let mut targets = std::mem::take(&mut state.pending_diagnostics_targets);
        while let Some((target_uri, task)) = targets.pop_first() {
            let mode = task.mode;
            let generation = task.generation;
            if !state.is_current_generation(&target_uri, generation) {
                continue;
            }
            state.analysis.clear_last_analysis_tier();
            let started_at = Instant::now();
            let outcome = match catch_unwind(AssertUnwindSafe(|| match mode {
                DiagnosticsAnalysisMode::Structure => {
                    state.analysis.analyze_document_structure_uri(&target_uri)
                }
                DiagnosticsAnalysisMode::Full => state.analysis.analyze_document_uri(&target_uri),
            })) {
                Ok(outcome) => outcome,
                Err(payload) => crate::analysis::single_server_diagnostic(
                    target_uri.clone(),
                    format!(
                        "kern-lsp analysis panicked: {}",
                        panic_message(payload.as_ref())
                    ),
                ),
            };
            let elapsed_ms = started_at.elapsed().as_millis();
            let budget_status = state
                .request_budget_policy
                .status(RequestBudgetKind::Diagnostics, elapsed_ms);
            emit_diagnostics_analysis_trace(state, writer, &target_uri, mode, elapsed_ms)?;
            state.queue_diagnostics_publish(target_uri, generation, outcome);
            if budget_status == RequestBudgetStatus::Exceeded {
                state.pending_diagnostics_targets.extend(targets);
                break;
            }
        }
    }

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

pub(super) fn drain_scheduler(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
) -> Result<(), ServerError> {
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
) -> Result<(), ServerError> {
    state.queue_workspace_refresh_task(reason.to_string());
    let _ = writer;
    Ok(())
}

pub(super) fn execute_document_request<T, F>(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    id: Value,
    target_uri: &str,
    lane: SchedulerLane,
    analysis: F,
) -> Result<(), ServerError>
where
    T: serde::Serialize,
    F: FnOnce(&AnalysisEngine) -> Result<T, String>,
{
    let request = state.request_context_for_document(id, target_uri);
    if state.should_skip_request(&request) {
        return Ok(());
    }

    state.analysis.clear_last_analysis_tier();
    let started_at = Instant::now();
    let result = catch_unwind(AssertUnwindSafe(|| analysis(&state.analysis)));
    let elapsed_ms = started_at.elapsed().as_millis();
    match result {
        Ok(Ok(result)) => {
            write_success_response(state, writer, &request, serde_json::to_value(result)?)
        }
        Ok(Err(message)) => write_error_response(state, writer, &request, INVALID_REQUEST, message),
        Err(payload) => write_error_response(
            state,
            writer,
            &request,
            INVALID_REQUEST,
            format!(
                "kern-lsp analysis panicked: {}",
                panic_message(payload.as_ref())
            ),
        ),
    }?;
    emit_analysis_tier_trace(state, writer, target_uri, lane, elapsed_ms)
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

pub(super) fn execute_optional_document_request<T, F>(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    id: Value,
    target_uri: &str,
    lane: SchedulerLane,
    analysis: F,
) -> Result<(), ServerError>
where
    T: serde::Serialize,
    F: FnOnce(&AnalysisEngine) -> Result<Option<T>, String>,
{
    let request = state.request_context_for_document(id, target_uri);
    if state.should_skip_request(&request) {
        return Ok(());
    }

    state.analysis.clear_last_analysis_tier();
    let started_at = Instant::now();
    let result = catch_unwind(AssertUnwindSafe(|| analysis(&state.analysis)));
    let elapsed_ms = started_at.elapsed().as_millis();
    match result {
        Ok(Ok(Some(result))) => {
            write_success_response(state, writer, &request, serde_json::to_value(result)?)
        }
        Ok(Ok(None)) => write_null_response(state, writer, &request),
        Ok(Err(message)) => write_error_response(state, writer, &request, INVALID_REQUEST, message),
        Err(payload) => write_error_response(
            state,
            writer,
            &request,
            INVALID_REQUEST,
            format!(
                "kern-lsp analysis panicked: {}",
                panic_message(payload.as_ref())
            ),
        ),
    }?;
    emit_analysis_tier_trace(state, writer, target_uri, lane, elapsed_ms)
}

fn emit_analysis_tier_trace(
    state: &ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    target_uri: &str,
    lane: SchedulerLane,
    elapsed_ms: u128,
) -> Result<(), ServerError> {
    let Some(tier) = state.analysis.last_analysis_tier() else {
        return Ok(());
    };
    let budget_status = state
        .request_budget_policy
        .status(RequestBudgetKind::Interactive, elapsed_ms)
        .as_str();
    emit_trace(
        state,
        writer,
        "analysis tier selected",
        Some(format!(
            "tier={} elapsed_ms={} budget={} lane={:?} target={}",
            tier.as_str(),
            elapsed_ms,
            budget_status,
            lane,
            target_uri
        )),
        true,
    )
}

fn emit_diagnostics_analysis_trace(
    state: &ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    target_uri: &str,
    mode: DiagnosticsAnalysisMode,
    elapsed_ms: u128,
) -> Result<(), ServerError> {
    let tier = state
        .analysis
        .last_analysis_tier()
        .map(|tier| tier.as_str());
    let mut verbose = format!(
        "mode={:?} elapsed_ms={} budget={} lane={:?} target={}",
        mode,
        elapsed_ms,
        state
            .request_budget_policy
            .status(RequestBudgetKind::Diagnostics, elapsed_ms)
            .as_str(),
        SchedulerLane::Diagnostics,
        target_uri
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

fn emit_workspace_refresh_trace(
    state: &ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    reason: &str,
    target_count: usize,
    elapsed_ms: u128,
) -> Result<(), ServerError> {
    emit_trace(
        state,
        writer,
        "workspace refresh completed",
        Some(format!(
            "reason={} targets={} elapsed_ms={} budget={} lane={:?}",
            reason,
            target_count,
            elapsed_ms,
            state
                .request_budget_policy
                .status(RequestBudgetKind::WorkspaceRefresh, elapsed_ms)
                .as_str(),
            SchedulerLane::Diagnostics
        )),
        true,
    )
}

pub(super) fn write_success_response(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    request: &RequestContext,
    result: Value,
) -> Result<(), ServerError> {
    if state.should_drop_response(request) {
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
        return Ok(());
    }
    writer.write_json(&error_response(request.id.clone(), code, message))?;
    Ok(())
}

pub(super) fn schedule_workspace_refresh(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    reason: &str,
) -> Result<(), ServerError> {
    execute_workspace_diagnostics_refresh(state, writer, reason, SchedulerLane::Diagnostics)
}
