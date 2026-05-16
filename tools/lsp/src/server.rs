mod configuration;
mod dispatch;
mod lifecycle;
mod scheduler;
mod state;
#[cfg(test)]
mod tests;

use self::dispatch::{
    handle_message as dispatch_handle_message, handle_message_nonblocking,
    report_message_error as dispatch_report_message_error,
};
pub(crate) use self::state::DiagnosticsAnalysisMode;
pub use self::state::ServerOptions;
use self::state::{
    AnalysisGeneration, DiagnosticsTaskResult, DocumentRequestResponse, DocumentRequestTaskResult,
    LspWorkerTask, RequestContext, ScheduledDocumentRequestTask, SchedulerLane, ServerState,
    WorkspaceRefreshTaskResult,
};
use crate::analysis::AnalysisEngine;
use crate::protocol::{IncomingMessage, error_response};
use crate::transport::{MessageReader, MessageWriter};
use serde_json::Value;
use std::fmt;
use std::io::{self, BufReader, BufWriter};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[cfg(test)]
pub(super) use crate::protocol::initialize_result;

#[cfg(test)]
use std::sync::{Arc, Barrier};

const PARSE_ERROR: i64 = -32700;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_REQUEST: i64 = -32600;
const INVALID_PARAMS: i64 = -32602;
const SERVER_NOT_INITIALIZED: i64 = -32002;
const REQUEST_CANCELLED: i64 = -32800;

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

pub fn run_with_analysis_options(
    analysis: AnalysisEngine,
    options: ServerOptions,
) -> Result<(), ServerError> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let reader = MessageReader::new(BufReader::new(stdin));
    let mut writer = MessageWriter::new(BufWriter::new(stdout));
    let mut state = ServerState::with_options(analysis, options);

    run_message_loop(&mut state, reader, &mut writer)
}

enum ServerInputEvent {
    Message(IncomingMessage),
    ParseError(String),
    ReadError(io::Error),
    Eof,
}

#[cfg(test)]
static TEST_DOCUMENT_REQUEST_BARRIERS: std::sync::Mutex<Option<(Arc<Barrier>, Arc<Barrier>)>> =
    std::sync::Mutex::new(None);

fn run_message_loop<R, W>(
    state: &mut ServerState,
    mut reader: MessageReader<R>,
    writer: &mut MessageWriter<W>,
) -> Result<(), ServerError>
where
    R: io::BufRead + Send + 'static,
    W: io::Write,
{
    let (input_tx, input_rx) = mpsc::channel();
    thread::spawn(move || {
        loop {
            let event = match reader.read_message() {
                Ok(Some(payload)) => match serde_json::from_slice::<IncomingMessage>(&payload) {
                    Ok(message) => ServerInputEvent::Message(message),
                    Err(err) => {
                        ServerInputEvent::ParseError(format!("failed to parse LSP message: {err}"))
                    }
                },
                Ok(None) => ServerInputEvent::Eof,
                Err(err) => ServerInputEvent::ReadError(err),
            };
            let done = matches!(
                event,
                ServerInputEvent::Eof | ServerInputEvent::ReadError(_)
            );
            if input_tx.send(event).is_err() || done {
                break;
            }
        }
    });

    let mut input_closed = false;
    loop {
        scheduler::flush_document_request_results(state, writer, false)?;
        scheduler::flush_workspace_refresh_results(state, writer, false)?;
        scheduler::flush_diagnostics_results(state, writer, false)?;
        if state.pending_workspace_refresh_reason.is_some()
            || !state.pending_diagnostics_targets.is_empty()
            || !state.pending_diagnostics.is_empty()
        {
            scheduler::drain_scheduler(state, writer)?;
        }
        if input_closed {
            if state.has_pending_worker_work() {
                scheduler::flush_document_request_results(state, writer, true)?;
                scheduler::flush_workspace_refresh_results(state, writer, true)?;
                scheduler::flush_diagnostics_results(state, writer, true)?;
                continue;
            }
            break;
        }

        let event = if state.has_pending_worker_work() {
            match input_rx.recv_timeout(Duration::from_millis(5)) {
                Ok(event) => event,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    input_closed = true;
                    continue;
                }
            }
        } else {
            match input_rx.recv() {
                Ok(event) => event,
                Err(_) => break,
            }
        };

        let message = match event {
            ServerInputEvent::Message(message) => message,
            ServerInputEvent::ParseError(message) => {
                writer.write_json(&error_response(Value::Null, PARSE_ERROR, message))?;
                continue;
            }
            ServerInputEvent::ReadError(err) => return Err(ServerError::Io(err)),
            ServerInputEvent::Eof => {
                input_closed = true;
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

        match handle_message_nonblocking(state, writer, message) {
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
    dispatch_report_message_error(state, writer, id, code, message)
}

#[cfg_attr(not(test), allow(dead_code))]
fn handle_message(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    message: IncomingMessage,
) -> Result<bool, ServerError> {
    dispatch_handle_message(state, writer, message)
}
