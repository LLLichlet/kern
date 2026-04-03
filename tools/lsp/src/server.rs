use crate::analysis::{AnalysisEngine, AnalysisOutcome, DocumentSyncAction, cleared_uris};
use crate::protocol::{
    CancelRequestParams, ClientCapabilities, CodeActionParams, CompletionParams, DefinitionParams,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentHighlightParams, DocumentSymbolParams, IncomingMessage,
    InitializeParams, InitializeResultOptions, ReferenceParams, RenameParams, SemanticTokensParams,
    SetTraceParams, SignatureHelpParams, error_response, initialize_result, log_message, log_trace,
    null_response, publish_diagnostics, success_response,
};
use crate::transport::{MessageReader, MessageWriter};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{self, BufReader, BufWriter};

const PARSE_ERROR: i64 = -32700;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_REQUEST: i64 = -32600;
const INVALID_PARAMS: i64 = -32602;
const SERVER_NOT_INITIALIZED: i64 = -32002;

#[derive(Debug)]
pub enum ServerError {
    Io(io::Error),
    Json(serde_json::Error),
    Protocol(String),
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServerError::Io(err) => write!(f, "{err}"),
            ServerError::Json(err) => write!(f, "{err}"),
            ServerError::Protocol(message) => write!(f, "{message}"),
        }
    }
}

impl From<io::Error> for ServerError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ServerError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

struct ServerState {
    initialized: bool,
    shutdown_requested: bool,
    trace: TraceValue,
    diagnostics_flush_policy: DiagnosticsFlushPolicy,
    analysis: AnalysisEngine,
    next_analysis_generation: u64,
    latest_generation_by_target: BTreeMap<String, AnalysisGeneration>,
    canceled_request_ids: Vec<Value>,
    pending_diagnostics_targets: BTreeSet<String>,
    pending_workspace_refresh_reason: Option<String>,
    pending_diagnostics: BTreeMap<String, ScheduledDiagnosticsPublish>,
    published_by_target: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct AnalysisGeneration(u64);

#[derive(Debug, Clone)]
struct RequestContext {
    id: Value,
    target_uri: Option<String>,
    generation: Option<AnalysisGeneration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchedulerLane {
    Interactive,
    Diagnostics,
}

struct ScheduledDiagnosticsPublish {
    generation: AnalysisGeneration,
    outcome: AnalysisOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiagnosticsFlushPolicy {
    target_task_budget: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchedulerDrainDecision {
    Drain,
    Defer,
}

impl DiagnosticsFlushPolicy {
    fn new() -> Self {
        Self {
            target_task_budget: 2,
        }
    }

    fn decide_after_message(self, method: &str) -> SchedulerDrainDecision {
        match method {
            "textDocument/didOpen" | "textDocument/didClose" | "textDocument/didSave" => {
                SchedulerDrainDecision::Drain
            }
            "textDocument/didChange"
            | "workspace/didChangeConfiguration"
            | "workspace/didChangeWatchedFiles" => SchedulerDrainDecision::Defer,
            _ => SchedulerDrainDecision::Defer,
        }
    }

    fn should_force_drain_for_pending_work(self, state: &ServerState) -> bool {
        state.pending_workspace_refresh_reason.is_some()
            || state.pending_diagnostics_targets.len() >= self.target_task_budget
    }
}

impl ServerState {
    #[cfg(test)]
    fn new() -> Self {
        Self::with_analysis(AnalysisEngine::default())
    }

    fn with_analysis(analysis: AnalysisEngine) -> Self {
        Self {
            initialized: false,
            shutdown_requested: false,
            trace: TraceValue::Off,
            diagnostics_flush_policy: DiagnosticsFlushPolicy::new(),
            analysis,
            next_analysis_generation: 0,
            latest_generation_by_target: BTreeMap::new(),
            canceled_request_ids: Vec::new(),
            pending_diagnostics_targets: BTreeSet::new(),
            pending_workspace_refresh_reason: None,
            pending_diagnostics: BTreeMap::new(),
            published_by_target: BTreeMap::new(),
        }
    }

    fn begin_target_analysis(&mut self, target_uri: &str) -> AnalysisGeneration {
        self.next_analysis_generation += 1;
        let generation = AnalysisGeneration(self.next_analysis_generation);
        self.latest_generation_by_target
            .insert(target_uri.to_string(), generation);
        generation
    }

    fn begin_workspace_refresh(&mut self) -> BTreeMap<String, AnalysisGeneration> {
        self.analysis
            .document_uris()
            .into_iter()
            .map(|target_uri| {
                let generation = self.begin_target_analysis(&target_uri);
                (target_uri, generation)
            })
            .collect()
    }

    fn is_current_generation(&self, target_uri: &str, generation: AnalysisGeneration) -> bool {
        self.latest_generation_by_target.get(target_uri).copied() == Some(generation)
    }

    fn request_context(&self, id: Value) -> RequestContext {
        RequestContext {
            id,
            target_uri: None,
            generation: None,
        }
    }

    fn request_context_for_document(&self, id: Value, target_uri: &str) -> RequestContext {
        RequestContext {
            id,
            target_uri: Some(target_uri.to_string()),
            generation: self.latest_generation_by_target.get(target_uri).copied(),
        }
    }

    fn cancel_request(&mut self, id: Value) {
        if self
            .canceled_request_ids
            .iter()
            .any(|canceled| canceled == &id)
        {
            return;
        }
        self.canceled_request_ids.push(id);
    }

    fn should_skip_request(&mut self, request: &RequestContext) -> bool {
        self.should_drop_response(request)
    }

    fn should_drop_response(&mut self, request: &RequestContext) -> bool {
        if let Some(index) = self
            .canceled_request_ids
            .iter()
            .position(|canceled| canceled == &request.id)
        {
            self.canceled_request_ids.swap_remove(index);
            return true;
        }

        match (&request.target_uri, request.generation) {
            (Some(target_uri), Some(generation)) => {
                !self.is_current_generation(target_uri, generation)
            }
            _ => false,
        }
    }

    fn queue_diagnostics_publish(
        &mut self,
        target_uri: String,
        generation: AnalysisGeneration,
        outcome: AnalysisOutcome,
    ) {
        self.pending_diagnostics.insert(
            target_uri,
            ScheduledDiagnosticsPublish {
                generation,
                outcome,
            },
        );
    }

    fn queue_target_diagnostics_task(&mut self, target_uri: String) {
        if self.pending_workspace_refresh_reason.is_some() {
            return;
        }
        self.pending_diagnostics_targets.insert(target_uri);
    }

    fn queue_workspace_refresh_task(&mut self, reason: String) {
        self.pending_workspace_refresh_reason = Some(reason);
        self.pending_diagnostics_targets.clear();
    }

    fn has_pending_diagnostics_work(&self) -> bool {
        !self.pending_diagnostics_targets.is_empty()
            || self.pending_workspace_refresh_reason.is_some()
            || !self.pending_diagnostics.is_empty()
    }

    fn should_drain_scheduler_after(&self, method: &str) -> bool {
        self.has_pending_diagnostics_work()
            && (self.diagnostics_flush_policy.decide_after_message(method)
                == SchedulerDrainDecision::Drain
                || self
                    .diagnostics_flush_policy
                    .should_force_drain_for_pending_work(self))
    }
}

pub fn run_with_analysis(analysis: AnalysisEngine) -> Result<(), ServerError> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = MessageReader::new(BufReader::new(stdin.lock()));
    let mut writer = MessageWriter::new(BufWriter::new(stdout.lock()));
    let mut state = ServerState::with_analysis(analysis);

    run_message_loop(&mut state, &mut reader, &mut writer)
}

fn run_message_loop<R, W>(
    state: &mut ServerState,
    reader: &mut MessageReader<R>,
    writer: &mut MessageWriter<W>,
) -> Result<(), ServerError>
where
    R: io::BufRead,
    W: io::Write,
{
    while let Some(payload) = reader.read_message()? {
        let message = match serde_json::from_slice::<IncomingMessage>(&payload) {
            Ok(message) => message,
            Err(err) => {
                writer.write_json(&error_response(
                    Value::Null,
                    PARSE_ERROR,
                    format!("failed to parse LSP message: {err}"),
                ))?;
                continue;
            }
        };
        let request_id = message.id.clone();
        if message.jsonrpc != crate::protocol::JSONRPC_VERSION {
            report_message_error(
                state,
                writer,
                request_id,
                INVALID_REQUEST,
                format!("unsupported jsonrpc version `{}`", message.jsonrpc),
            )?;
            continue;
        }

        match handle_message(state, writer, message) {
            Ok(true) => break,
            Ok(false) => {}
            Err(ServerError::Io(err)) => return Err(ServerError::Io(err)),
            Err(ServerError::Json(err)) => {
                report_message_error(
                    state,
                    writer,
                    request_id,
                    INVALID_PARAMS,
                    format!("invalid request params: {err}"),
                )?;
            }
            Err(ServerError::Protocol(message)) => {
                report_message_error(state, writer, request_id, INVALID_REQUEST, message)?;
            }
        }
    }

    Ok(())
}

fn report_message_error(
    state: &ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    id: Option<Value>,
    code: i64,
    message: impl Into<String>,
) -> Result<(), ServerError> {
    let message = message.into();
    if let Some(id) = id {
        writer.write_json(&error_response(id, code, message))?;
        return Ok(());
    }

    if state.initialized {
        writer.write_json(&log_message(2, message))?;
    }
    Ok(())
}

fn handle_message(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    message: IncomingMessage,
) -> Result<bool, ServerError> {
    let Some(method) = message.method.as_deref() else {
        if let Some(id) = message.id {
            writer.write_json(&error_response(
                id,
                INVALID_REQUEST,
                "message did not contain a method",
            ))?;
        }
        return Ok(false);
    };

    if !state.initialized && requires_initialization(method) {
        if let Some(id) = message.id {
            writer.write_json(&error_response(
                id,
                SERVER_NOT_INITIALIZED,
                format!("method `{method}` requires a completed initialize request"),
            ))?;
        }
        return Ok(false);
    }

    if state.shutdown_requested && rejects_after_shutdown(method) {
        if let Some(id) = message.id {
            writer.write_json(&error_response(
                id,
                INVALID_REQUEST,
                format!("method `{method}` is not allowed after shutdown"),
            ))?;
        }
        return Ok(false);
    }

    match method {
        "initialize" => {
            if state.initialized {
                let request = state.request_context(message.id.unwrap_or(Value::Null));
                write_error_response(
                    state,
                    writer,
                    &request,
                    INVALID_REQUEST,
                    "server is already initialized",
                )?;
                return Ok(false);
            }
            let request = state.request_context(message.id.ok_or_else(|| {
                ServerError::Protocol("initialize must be sent as a request".to_string())
            })?);
            let params = required_params::<InitializeParams>(message.params)?;
            ensure_utf16_position_encoding(&params.capabilities, request.id.clone(), writer)?;
            let capabilities = negotiate_capabilities(&params.capabilities);
            state.trace = TraceValue::from_raw(params.trace.as_deref());
            state.initialized = true;
            write_success_response(state, writer, &request, initialize_result(capabilities))?;
            emit_initialize_followups(state, writer, &params, capabilities)?;
        }
        "initialized" => {}
        "$/setTrace" => {
            let next_trace = message
                .params
                .and_then(|params| serde_json::from_value::<SetTraceParams>(params).ok())
                .map(|params| params.value);
            state.trace = TraceValue::from_raw(next_trace.as_deref());
            emit_trace(
                state,
                writer,
                format!("trace level set to `{}`", state.trace.as_str()),
                None,
                false,
            )?;
        }
        "$/cancelRequest" => {
            if let Some(params) = message.params
                && let Ok(params) = serde_json::from_value::<CancelRequestParams>(params)
            {
                state.cancel_request(params.id);
            }
        }
        "workspace/didChangeConfiguration" => {
            schedule_workspace_refresh(state, writer, "workspace configuration changed")?;
        }
        "workspace/didChangeWatchedFiles" => {
            schedule_workspace_refresh(state, writer, "workspace files changed")?;
        }
        "shutdown" => {
            let request = state.request_context(message.id.ok_or_else(|| {
                ServerError::Protocol("shutdown must be sent as a request".to_string())
            })?);
            state.shutdown_requested = true;
            write_null_response(state, writer, &request)?;
        }
        "exit" => {
            if !state.shutdown_requested {
                return Err(ServerError::Protocol(
                    "received `exit` before `shutdown`".to_string(),
                ));
            }
            return Ok(true);
        }
        "textDocument/didOpen" => {
            let params = required_params::<DidOpenTextDocumentParams>(message.params)?;
            let target_uri = params.text_document.uri.clone();
            execute_document_diagnostics(
                state,
                writer,
                &target_uri,
                SchedulerLane::Diagnostics,
                |analysis| analysis.open_document_state(params),
            )?;
        }
        "textDocument/didChange" => {
            let params = required_params::<DidChangeTextDocumentParams>(message.params)?;
            let target_uri = params.text_document.uri.clone();
            execute_document_diagnostics(
                state,
                writer,
                &target_uri,
                SchedulerLane::Diagnostics,
                |analysis| analysis.change_document_state(params),
            )?;
        }
        "textDocument/didClose" => {
            let params = required_params::<DidCloseTextDocumentParams>(message.params)?;
            let target_uri = params.text_document.uri.clone();
            execute_document_diagnostics(
                state,
                writer,
                &target_uri,
                SchedulerLane::Diagnostics,
                |analysis| analysis.close_document_state(params),
            )?;
        }
        "textDocument/didSave" => {
            let _ = required_params::<DidSaveTextDocumentParams>(message.params)?;
        }
        "textDocument/documentSymbol" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/documentSymbol must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<DocumentSymbolParams>(message.params)?;
            execute_document_request(
                state,
                writer,
                id,
                &params.text_document.uri,
                SchedulerLane::Interactive,
                |analysis| analysis.document_symbols(&params.text_document.uri),
            )?;
        }
        "textDocument/definition" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/definition must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<DefinitionParams>(message.params)?;
            execute_optional_document_request(
                state,
                writer,
                id,
                &params.text_document.uri,
                SchedulerLane::Interactive,
                |analysis| analysis.goto_definition(&params.text_document.uri, params.position),
            )?;
        }
        "textDocument/documentHighlight" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/documentHighlight must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<DocumentHighlightParams>(message.params)?;
            execute_document_request(
                state,
                writer,
                id,
                &params.text_document.uri,
                SchedulerLane::Interactive,
                |analysis| analysis.document_highlights(&params.text_document.uri, params.position),
            )?;
        }
        "textDocument/references" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/references must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<ReferenceParams>(message.params)?;
            execute_document_request(
                state,
                writer,
                id,
                &params.text_document.uri,
                SchedulerLane::Interactive,
                |analysis| {
                    analysis.references(
                        &params.text_document.uri,
                        params.position,
                        params.context.include_declaration,
                    )
                },
            )?;
        }
        "textDocument/hover" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol("textDocument/hover must be sent as a request".to_string())
            })?;
            let params = required_params::<DefinitionParams>(message.params)?;
            execute_optional_document_request(
                state,
                writer,
                id,
                &params.text_document.uri,
                SchedulerLane::Interactive,
                |analysis| analysis.hover(&params.text_document.uri, params.position),
            )?;
        }
        "textDocument/signatureHelp" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/signatureHelp must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<SignatureHelpParams>(message.params)?;
            execute_optional_document_request(
                state,
                writer,
                id,
                &params.text_document.uri,
                SchedulerLane::Interactive,
                |analysis| analysis.signature_help(&params.text_document.uri, params.position),
            )?;
        }
        "textDocument/completion" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/completion must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<CompletionParams>(message.params)?;
            execute_document_request(
                state,
                writer,
                id,
                &params.text_document.uri,
                SchedulerLane::Interactive,
                |analysis| analysis.completion(&params.text_document.uri, params.position),
            )?;
        }
        "textDocument/semanticTokens/full" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/semanticTokens/full must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<SemanticTokensParams>(message.params)?;
            execute_document_request(
                state,
                writer,
                id,
                &params.text_document.uri,
                SchedulerLane::Interactive,
                |analysis| analysis.semantic_tokens(&params.text_document.uri),
            )?;
        }
        "textDocument/prepareRename" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/prepareRename must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<DefinitionParams>(message.params)?;
            execute_optional_document_request(
                state,
                writer,
                id,
                &params.text_document.uri,
                SchedulerLane::Interactive,
                |analysis| analysis.prepare_rename(&params.text_document.uri, params.position),
            )?;
        }
        "textDocument/rename" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol("textDocument/rename must be sent as a request".to_string())
            })?;
            let params = required_params::<RenameParams>(message.params)?;
            execute_document_request(
                state,
                writer,
                id,
                &params.text_document.uri,
                SchedulerLane::Interactive,
                |analysis| {
                    analysis.rename(&params.text_document.uri, params.position, &params.new_name)
                },
            )?;
        }
        "textDocument/codeAction" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/codeAction must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<CodeActionParams>(message.params)?;
            if !context_allows_quickfix(&params.context.only) {
                execute_document_request(
                    state,
                    writer,
                    id,
                    &params.text_document.uri,
                    SchedulerLane::Interactive,
                    |_| Ok::<Value, String>(Value::Array(Vec::new())),
                )?;
            } else {
                execute_document_request(
                    state,
                    writer,
                    id,
                    &params.text_document.uri,
                    SchedulerLane::Interactive,
                    |analysis| {
                        analysis.code_actions(&params.text_document.uri, params.range.clone())
                    },
                )?;
            }
        }
        _ => {
            if let Some(id) = message.id {
                let request = state.request_context(id);
                write_error_response(
                    state,
                    writer,
                    &request,
                    METHOD_NOT_FOUND,
                    format!("method `{method}` is not implemented"),
                )?;
            }
        }
    }

    if state.should_drain_scheduler_after(method) {
        drain_scheduler(state, writer)?;
    }
    Ok(false)
}

fn publish_analysis_outcome(
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

fn flush_diagnostics_lane(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
) -> Result<(), ServerError> {
    if let Some(reason) = state.pending_workspace_refresh_reason.take() {
        let generations = state.begin_workspace_refresh();
        for (target_uri, outcome) in state.analysis.refresh_workspace() {
            let generation = generations
                .get(&target_uri)
                .copied()
                .unwrap_or_else(|| state.begin_target_analysis(&target_uri));
            state.queue_diagnostics_publish(target_uri, generation, outcome);
        }
        state.pending_diagnostics_targets.clear();
        emit_trace(state, writer, reason, None, true)?;
    } else {
        let targets = std::mem::take(&mut state.pending_diagnostics_targets);
        for target_uri in targets {
            let generation = state
                .latest_generation_by_target
                .get(&target_uri)
                .copied()
                .unwrap_or_else(|| state.begin_target_analysis(&target_uri));
            let outcome = state.analysis.analyze_document_uri(&target_uri);
            state.queue_diagnostics_publish(target_uri, generation, outcome);
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

fn drain_scheduler(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
) -> Result<(), ServerError> {
    flush_diagnostics_lane(state, writer)
}

fn execute_document_diagnostics<F>(
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
    match action(&mut state.analysis) {
        DocumentSyncAction::ScheduleTarget(uri) => {
            state
                .latest_generation_by_target
                .insert(uri.clone(), generation);
            state.queue_target_diagnostics_task(uri);
        }
        DocumentSyncAction::Immediate(outcome) => {
            state.queue_diagnostics_publish(target_uri.to_string(), generation, outcome);
        }
    }
    let _ = writer;
    Ok(())
}

fn execute_workspace_diagnostics_refresh(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    reason: &str,
    _lane: SchedulerLane,
) -> Result<(), ServerError> {
    state.queue_workspace_refresh_task(reason.to_string());
    let _ = writer;
    Ok(())
}

fn execute_document_request<T, F>(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    id: Value,
    target_uri: &str,
    _lane: SchedulerLane,
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

    match analysis(&state.analysis) {
        Ok(result) => {
            write_success_response(state, writer, &request, serde_json::to_value(result)?)
        }
        Err(message) => write_error_response(state, writer, &request, INVALID_REQUEST, message),
    }
}

fn execute_optional_document_request<T, F>(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    id: Value,
    target_uri: &str,
    _lane: SchedulerLane,
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

    match analysis(&state.analysis) {
        Ok(Some(result)) => {
            write_success_response(state, writer, &request, serde_json::to_value(result)?)
        }
        Ok(None) => write_null_response(state, writer, &request),
        Err(message) => write_error_response(state, writer, &request, INVALID_REQUEST, message),
    }
}

fn write_success_response(
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

fn write_null_response(
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

fn write_error_response(
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

fn schedule_workspace_refresh(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    reason: &str,
) -> Result<(), ServerError> {
    execute_workspace_diagnostics_refresh(state, writer, reason, SchedulerLane::Diagnostics)
}

fn required_params<T>(params: Option<Value>) -> Result<T, ServerError>
where
    T: serde::de::DeserializeOwned,
{
    let params = params.ok_or_else(|| {
        ServerError::Protocol("expected request params, but message omitted them".to_string())
    })?;
    Ok(serde_json::from_value(params)?)
}

fn context_allows_quickfix(only: &Option<Vec<String>>) -> bool {
    let Some(only) = only else {
        return true;
    };

    only.iter()
        .any(|kind| kind == "quickfix" || kind.starts_with("quickfix."))
}

fn requires_initialization(method: &str) -> bool {
    !matches!(
        method,
        "initialize" | "initialized" | "exit" | "$/setTrace" | "$/cancelRequest"
    )
}

fn rejects_after_shutdown(method: &str) -> bool {
    !matches!(method, "exit" | "$/cancelRequest" | "$/setTrace")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TraceValue {
    Off,
    Messages,
    Verbose,
}

impl TraceValue {
    fn from_raw(raw: Option<&str>) -> Self {
        match raw.unwrap_or("off") {
            "messages" => Self::Messages,
            "verbose" => Self::Verbose,
            _ => Self::Off,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Messages => "messages",
            Self::Verbose => "verbose",
        }
    }
}

fn ensure_utf16_position_encoding(
    capabilities: &ClientCapabilities,
    id: Value,
    writer: &mut MessageWriter<impl io::Write>,
) -> Result<(), ServerError> {
    let Some(encodings) = capabilities.general.position_encodings.as_ref() else {
        return Ok(());
    };
    if encodings
        .iter()
        .any(|encoding| encoding.eq_ignore_ascii_case("utf-16"))
    {
        return Ok(());
    }

    writer.write_json(&error_response(
        id,
        INVALID_REQUEST,
        "kern-lsp requires UTF-16 position encoding support from the client",
    ))?;
    Err(ServerError::Protocol(
        "client does not advertise UTF-16 position encoding support".to_string(),
    ))
}

fn negotiate_capabilities(capabilities: &ClientCapabilities) -> InitializeResultOptions {
    let code_action_literals = capabilities
        .text_document
        .code_action
        .as_ref()
        .and_then(|capabilities| capabilities.code_action_literal_support.as_ref())
        .is_some();
    let rename_prepare_support = capabilities
        .text_document
        .rename
        .as_ref()
        .map(|capabilities| capabilities.prepare_support)
        .unwrap_or(false);
    let semantic_tokens = capabilities.text_document.semantic_tokens.is_some();

    InitializeResultOptions {
        code_action_literals,
        rename_prepare_support,
        semantic_tokens,
    }
}

fn emit_initialize_followups(
    state: &ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    params: &InitializeParams,
    capabilities: InitializeResultOptions,
) -> Result<(), ServerError> {
    if !capabilities.code_action_literals {
        writer.write_json(&log_message(
            2,
            "Client does not advertise code action literal support; kern-lsp will disable quick-fix code actions.",
        ))?;
    }
    if !capabilities.rename_prepare_support {
        writer.write_json(&log_message(
            3,
            "Client does not advertise prepareRename support; kern-lsp will serve basic rename only.",
        ))?;
    }

    let mut verbose = Vec::new();
    if let Some(client_info) = &params.client_info {
        verbose.push(match &client_info.version {
            Some(version) => format!("client={} {}", client_info.name, version),
            None => format!("client={}", client_info.name),
        });
    }
    if let Some(encodings) = &params.capabilities.general.position_encodings {
        verbose.push(format!("positionEncodings={}", encodings.join(",")));
    }

    emit_trace(
        state,
        writer,
        "initialize completed",
        (!verbose.is_empty()).then(|| verbose.join(" | ")),
        false,
    )
}

fn emit_trace(
    state: &ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    message: impl Into<String>,
    verbose: Option<String>,
    verbose_only: bool,
) -> Result<(), ServerError> {
    match state.trace {
        TraceValue::Off => Ok(()),
        TraceValue::Messages if verbose_only => Ok(()),
        TraceValue::Messages | TraceValue::Verbose => {
            writer.write_json(&log_trace(
                message,
                if state.trace == TraceValue::Verbose {
                    verbose
                } else {
                    None
                },
            ))?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::JSONRPC_VERSION;
    use crate::transport::MessageReader;
    use serde_json::json;
    use std::fs;
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn initialize_result_advertises_precise_capabilities() {
        let result = initialize_result(InitializeResultOptions::default());

        assert_eq!(result["positionEncoding"], "utf-16");
        assert_eq!(
            result["capabilities"]["completionProvider"]["resolveProvider"],
            false
        );
        assert_eq!(
            result["capabilities"]["completionProvider"]["triggerCharacters"],
            json!(["."])
        );
        assert_eq!(result["capabilities"]["documentHighlightProvider"], true);
        assert_eq!(
            result["capabilities"]["signatureHelpProvider"]["triggerCharacters"],
            json!(["(", ","])
        );
        assert_eq!(
            result["capabilities"]["codeActionProvider"]["codeActionKinds"],
            json!(["quickfix"])
        );
        assert_eq!(
            result["capabilities"]["semanticTokensProvider"]["range"],
            false
        );
        assert_eq!(
            result["capabilities"]["semanticTokensProvider"]["full"]["delta"],
            false
        );
    }

    #[test]
    fn rejects_requests_before_initialize() {
        let mut state = ServerState::new();
        let mut output = Vec::new();
        let mut writer = MessageWriter::new(&mut output);

        let should_exit = handle_message(
            &mut state,
            &mut writer,
            IncomingMessage {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: Some(json!(1)),
                method: Some("textDocument/hover".to_string()),
                params: Some(json!({
                    "textDocument": { "uri": "file:///main.rn" },
                    "position": { "line": 0, "character": 0 }
                })),
            },
        )
        .unwrap();

        assert!(!should_exit);
        let response = read_single_response(&output);
        assert_eq!(response["id"], json!(1));
        assert_eq!(response["error"]["code"], json!(SERVER_NOT_INITIALIZED));
    }

    #[test]
    fn accepts_common_post_initialize_notifications() {
        let mut state = ServerState::new();
        state.initialized = true;
        let mut output = Vec::new();
        let mut writer = MessageWriter::new(&mut output);

        for method in [
            "$/setTrace",
            "$/cancelRequest",
            "workspace/didChangeConfiguration",
            "workspace/didChangeWatchedFiles",
        ] {
            let should_exit = handle_message(
                &mut state,
                &mut writer,
                IncomingMessage {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: None,
                    method: Some(method.to_string()),
                    params: Some(json!({})),
                },
            )
            .unwrap();

            assert!(!should_exit);
        }

        assert!(output.is_empty());
    }

    #[test]
    fn rejects_requests_after_shutdown() {
        let mut state = ServerState::new();
        state.initialized = true;
        state.shutdown_requested = true;
        let mut output = Vec::new();
        let mut writer = MessageWriter::new(&mut output);

        let should_exit = handle_message(
            &mut state,
            &mut writer,
            IncomingMessage {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: Some(json!(7)),
                method: Some("textDocument/documentSymbol".to_string()),
                params: Some(json!({
                    "textDocument": { "uri": "file:///main.rn" }
                })),
            },
        )
        .unwrap();

        assert!(!should_exit);
        let response = read_single_response(&output);
        assert_eq!(response["error"]["code"], json!(INVALID_REQUEST));
    }

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
                bundles: vec![crate::analysis::DiagnosticBundle {
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
                bundles: vec![crate::analysis::DiagnosticBundle {
                    uri: uri.clone(),
                    diagnostics: Vec::new(),
                }],
            },
        );
        state.queue_diagnostics_publish(
            uri.clone(),
            second,
            AnalysisOutcome {
                bundles: vec![crate::analysis::DiagnosticBundle {
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
    fn cancel_notification_registers_request_id() {
        let mut state = initialized_state();
        let mut output = Vec::new();
        let mut writer = MessageWriter::new(&mut output);

        let should_exit = handle_message(
            &mut state,
            &mut writer,
            IncomingMessage {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: None,
                method: Some("$/cancelRequest".to_string()),
                params: Some(json!({ "id": 41 })),
            },
        )
        .unwrap();

        assert!(!should_exit);
        assert_eq!(state.canceled_request_ids, vec![json!(41)]);
        assert!(output.is_empty());
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

    #[test]
    fn initialize_negotiates_capabilities_from_client_support() {
        let mut state = ServerState::new();
        let mut output = Vec::new();
        let mut writer = MessageWriter::new(&mut output);

        handle_message(
            &mut state,
            &mut writer,
            IncomingMessage {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: Some(json!(11)),
                method: Some("initialize".to_string()),
                params: Some(json!({
                    "capabilities": {
                        "general": {
                            "positionEncodings": ["utf-16", "utf-8"]
                        },
                        "textDocument": {
                            "semanticTokens": {
                                "requests": {
                                    "range": false,
                                    "full": { "delta": false }
                                }
                            }
                        }
                    }
                })),
            },
        )
        .unwrap();

        let messages = read_all_messages(&output);
        assert_eq!(
            messages[0]["result"]["capabilities"]["codeActionProvider"],
            false
        );
        assert_eq!(
            messages[0]["result"]["capabilities"]["renameProvider"],
            true
        );
        assert!(
            messages[0]["result"]["capabilities"]
                .get("semanticTokensProvider")
                .is_some()
        );
        assert_eq!(messages[1]["method"], "window/logMessage");
        assert_eq!(messages[2]["method"], "window/logMessage");
    }

    #[test]
    fn initialize_rejects_clients_without_utf16_support() {
        let mut state = ServerState::new();
        let mut output = Vec::new();
        let mut writer = MessageWriter::new(&mut output);

        let err = handle_message(
            &mut state,
            &mut writer,
            IncomingMessage {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: Some(json!(12)),
                method: Some("initialize".to_string()),
                params: Some(json!({
                    "capabilities": {
                        "general": {
                            "positionEncodings": ["utf-8"]
                        }
                    }
                })),
            },
        )
        .unwrap_err();

        assert!(matches!(err, ServerError::Protocol(_)));
        let response = read_single_response(&output);
        assert_eq!(response["error"]["code"], json!(INVALID_REQUEST));
    }

    #[test]
    fn initialize_trace_emits_log_trace_notification() {
        let mut state = ServerState::new();
        let mut output = Vec::new();
        let mut writer = MessageWriter::new(&mut output);

        handle_message(
            &mut state,
            &mut writer,
            IncomingMessage {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: Some(json!(13)),
                method: Some("initialize".to_string()),
                params: Some(json!({
                    "trace": "verbose",
                    "clientInfo": { "name": "Example", "version": "1.0" },
                    "capabilities": {
                        "general": { "positionEncodings": ["utf-16"] },
                        "textDocument": {
                            "codeAction": {
                                "codeActionLiteralSupport": {
                                    "codeActionKind": { "valueSet": ["quickfix"] }
                                }
                            },
                            "rename": { "prepareSupport": true },
                            "semanticTokens": {
                                "requests": { "full": true }
                            }
                        }
                    }
                })),
            },
        )
        .unwrap();

        let messages = read_all_messages(&output);
        assert_eq!(messages[1]["method"], "$/logTrace");
        assert_eq!(messages[1]["params"]["message"], "initialize completed");
        assert_eq!(
            messages[1]["params"]["verbose"],
            "client=Example 1.0 | positionEncodings=utf-16"
        );
    }

    #[test]
    fn set_trace_updates_state_and_emits_trace_notification() {
        let mut state = ServerState::new();
        state.trace = TraceValue::Messages;
        let mut output = Vec::new();
        let mut writer = MessageWriter::new(&mut output);

        handle_message(
            &mut state,
            &mut writer,
            IncomingMessage {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: None,
                method: Some("$/setTrace".to_string()),
                params: Some(json!({
                    "value": "verbose"
                })),
            },
        )
        .unwrap();

        assert_eq!(state.trace, TraceValue::Verbose);
        let response = read_single_response(&output);
        assert_eq!(response["method"], "$/logTrace");
        assert_eq!(
            response["params"]["message"],
            "trace level set to `verbose`"
        );
    }

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

        let open_messages =
            dispatch_messages(&mut state, did_open_message(&uri, invalid_source, 1));
        assert_eq!(open_messages.len(), 1);
        assert!(
            !open_messages[0]["params"]["diagnostics"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        let change_messages =
            dispatch_messages(&mut state, did_change_message(&uri, valid_source, 2));

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
    fn multiple_did_change_notifications_coalesce_until_save() {
        let mut state = initialized_state();
        let invalid_source = "fn main() i32 {\n    let value = i32.{1}\n    return value;\n}\n";
        let still_invalid = "fn main() i32 {\n    let value = i32.{2}\n    return value;\n}\n";
        let valid_source = "fn main() i32 {\n    let value = i32.{2};\n    return value;\n}\n";
        let uri = temp_file_uri("server_diagnostic_coalesce", invalid_source);

        let _ = dispatch_messages(&mut state, did_open_message(&uri, invalid_source, 1));
        assert!(
            dispatch_messages(&mut state, did_change_message(&uri, still_invalid, 2)).is_empty()
        );
        assert!(
            dispatch_messages(&mut state, did_change_message(&uri, valid_source, 3)).is_empty()
        );

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
        let change_messages =
            dispatch_messages(&mut state, did_change_message(&uri_b, changed_b, 2));

        assert_eq!(change_messages.len(), 2);
        assert!(
            change_messages
                .iter()
                .all(|message| message["method"] == "textDocument/publishDiagnostics")
        );
        assert!(!state.has_pending_diagnostics_work());
    }

    #[test]
    fn document_highlight_request_returns_same_file_spans() {
        let mut state = initialized_state();
        let source =
            "fn helper() i32 { return 1; }\nfn main() i32 { return helper() + helper(); }\n";
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
        let source = "fn main() i32 {\n    let value = i32.{1}\n    return value;\n}\n";
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
        let source = "fn main() i32 {\n    let value = i32.{1}\n    return value;\n}\n";
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
    fn rename_request_returns_workspace_edit() {
        let mut state = initialized_state();
        let source =
            "fn helper() i32 { return 1; }\nfn main() i32 { return helper() + helper(); }\n";
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
        let source =
            "fn helper() i32 { return 1; }\nfn main() i32 { return helper() + helper(); }\n";
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
    fn references_request_returns_sorted_locations() {
        let mut state = initialized_state();
        let source =
            "fn helper() i32 { return 1; }\nfn main() i32 { return helper() + helper(); }\n";
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
        assert!(contents.contains("fn helper: fn(i32) i32"));
    }

    #[test]
    fn signature_help_request_returns_active_parameter_information() {
        let mut state = initialized_state();
        let source = concat!(
            "fn helper(first: i32, second: i32) i32 {\n",
            "    return first + second;\n",
            "}\n",
            "fn main() i32 {\n",
            "    let value = i32.{2};\n",
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
    fn document_symbol_request_returns_top_level_symbols() {
        let mut state = initialized_state();
        let source = concat!(
            "type Point = struct { x: i32 };\n",
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

    #[test]
    fn semantic_tokens_request_returns_encoded_token_data() {
        let mut state = initialized_state();
        let source = concat!(
            "type Point = struct { x: i32 };\n",
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
    }

    #[test]
    fn run_loop_reports_parse_errors_and_keeps_processing_messages() {
        let invalid = "{\"oops\":1";
        let valid = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"shutdown\",\"params\":{}}";
        let payload = format!(
            "Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}",
            invalid.len(),
            invalid,
            valid.len(),
            valid
        );
        let mut reader = MessageReader::new(Cursor::new(payload.as_bytes()));
        let mut output = Vec::new();
        let mut writer = MessageWriter::new(&mut output);
        let mut state = initialized_state();

        run_message_loop(&mut state, &mut reader, &mut writer).unwrap();

        let messages = read_all_messages(&output);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["error"]["code"], json!(PARSE_ERROR));
        assert_eq!(messages[0]["id"], json!(null));
        assert_eq!(messages[1]["id"], json!(1));
        assert_eq!(messages[1]["result"], json!(null));
    }

    fn initialized_state() -> ServerState {
        let mut state = ServerState::new();
        state.initialized = true;
        state
    }

    fn dispatch_single_response(state: &mut ServerState, message: IncomingMessage) -> Value {
        let messages = dispatch_messages(state, message);
        assert_eq!(messages.len(), 1);
        messages.into_iter().next().unwrap()
    }

    fn dispatch_messages(state: &mut ServerState, message: IncomingMessage) -> Vec<Value> {
        let mut output = Vec::new();
        let mut writer = MessageWriter::new(&mut output);
        let should_exit = handle_message(state, &mut writer, message).unwrap();
        assert!(!should_exit);
        read_all_messages(&output)
    }

    fn did_open_message(uri: &str, text: &str, version: i64) -> IncomingMessage {
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: None,
            method: Some("textDocument/didOpen".to_string()),
            params: Some(json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "kern",
                    "version": version,
                    "text": text
                }
            })),
        }
    }

    fn did_change_message(uri: &str, text: &str, version: i64) -> IncomingMessage {
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: None,
            method: Some("textDocument/didChange".to_string()),
            params: Some(json!({
                "textDocument": {
                    "uri": uri,
                    "version": version
                },
                "contentChanges": [
                    {
                        "text": text
                    }
                ]
            })),
        }
    }

    fn did_save_message(uri: &str) -> IncomingMessage {
        IncomingMessage {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: None,
            method: Some("textDocument/didSave".to_string()),
            params: Some(json!({
                "textDocument": {
                    "uri": uri
                }
            })),
        }
    }

    fn temp_file_uri(prefix: &str, initial_text: &str) -> String {
        let path = unique_temp_file_path(prefix);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, initial_text).unwrap();
        format!("file://{}", path.to_string_lossy())
    }

    fn unique_temp_file_path(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let counter = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "{}_{}_{}_{}.rn",
            prefix,
            std::process::id(),
            nanos,
            counter
        ))
    }

    fn read_single_response(output: &[u8]) -> Value {
        let mut reader = MessageReader::new(Cursor::new(output));
        let payload = reader.read_message().unwrap().unwrap();
        serde_json::from_slice(&payload).unwrap()
    }

    fn read_all_messages(output: &[u8]) -> Vec<Value> {
        let mut reader = MessageReader::new(Cursor::new(output));
        let mut messages = Vec::new();
        while let Some(payload) = reader.read_message().unwrap() {
            messages.push(serde_json::from_slice(&payload).unwrap());
        }
        messages
    }
}
