mod lifecycle;
mod scheduler;
#[cfg(test)]
mod tests;

use self::lifecycle::{
    TraceValue, emit_initialize_followups, emit_trace, ensure_utf16_position_encoding,
    negotiate_capabilities,
};
use self::scheduler::{
    drain_scheduler, execute_document_diagnostics, execute_document_request,
    execute_optional_document_request, schedule_workspace_refresh, write_error_response,
    write_null_response, write_success_response,
};
use crate::analysis::{AnalysisEngine, AnalysisOutcome};
use crate::protocol::{
    CancelRequestParams, CodeActionParams, CompletionParams, DefinitionParams,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentHighlightParams, DocumentSymbolParams, IncomingMessage,
    InitializeParams, ReferenceParams, RenameParams, SemanticTokensParams, SetTraceParams,
    SignatureHelpParams, error_response, initialize_result, log_message,
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
