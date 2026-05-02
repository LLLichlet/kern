mod dispatch;
mod lifecycle;
mod scheduler;
mod state;
#[cfg(test)]
mod tests;

use self::dispatch::{
    handle_message as dispatch_handle_message,
    report_message_error as dispatch_report_message_error,
};
pub(crate) use self::state::DiagnosticsAnalysisMode;
use self::state::{AnalysisGeneration, RequestContext, SchedulerLane, ServerState};
use crate::analysis::AnalysisEngine;
use crate::protocol::{IncomingMessage, error_response};
use crate::transport::{MessageReader, MessageWriter};
use serde_json::Value;
use std::fmt;
use std::io::{self, BufReader, BufWriter};

#[cfg(test)]
pub(super) use crate::protocol::initialize_result;

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
