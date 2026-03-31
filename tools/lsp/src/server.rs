use crate::analysis::{AnalysisEngine, AnalysisOutcome, cleared_uris};
use crate::protocol::{
    CodeActionParams, CompletionParams, DefinitionParams, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DocumentSymbolParams, IncomingMessage,
    ReferenceParams, RenameParams, SemanticTokensParams, error_response, initialize_result,
    null_response, publish_diagnostics, success_response,
};
use crate::transport::{MessageReader, MessageWriter};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{self, BufReader, BufWriter};

const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_REQUEST: i64 = -32600;

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
    shutdown_requested: bool,
    analysis: AnalysisEngine,
    published_by_target: BTreeMap<String, BTreeSet<String>>,
}

impl ServerState {
    fn new() -> Self {
        Self {
            shutdown_requested: false,
            analysis: AnalysisEngine::default(),
            published_by_target: BTreeMap::new(),
        }
    }
}

pub fn run() -> Result<(), ServerError> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = MessageReader::new(BufReader::new(stdin.lock()));
    let mut writer = MessageWriter::new(BufWriter::new(stdout.lock()));
    let mut state = ServerState::new();

    while let Some(payload) = reader.read_message()? {
        let message = serde_json::from_slice::<IncomingMessage>(&payload)?;
        if message.jsonrpc != crate::protocol::JSONRPC_VERSION {
            return Err(ServerError::Protocol(format!(
                "unsupported jsonrpc version `{}`",
                message.jsonrpc
            )));
        }

        let should_exit = handle_message(&mut state, &mut writer, message)?;
        if should_exit {
            break;
        }
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

    match method {
        "initialize" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol("initialize must be sent as a request".to_string())
            })?;
            writer.write_json(&success_response(id, initialize_result()))?;
        }
        "initialized" => {}
        "shutdown" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol("shutdown must be sent as a request".to_string())
            })?;
            state.shutdown_requested = true;
            writer.write_json(&null_response(id))?;
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
            let outcome = state.analysis.open_document(params);
            publish_analysis_outcome(state, writer, &target_uri, outcome)?;
        }
        "textDocument/didChange" => {
            let params = required_params::<DidChangeTextDocumentParams>(message.params)?;
            let target_uri = params.text_document.uri.clone();
            let outcome = state.analysis.change_document(params);
            publish_analysis_outcome(state, writer, &target_uri, outcome)?;
        }
        "textDocument/didClose" => {
            let params = required_params::<DidCloseTextDocumentParams>(message.params)?;
            let target_uri = params.text_document.uri.clone();
            let outcome = state.analysis.close_document(params);
            publish_analysis_outcome(state, writer, &target_uri, outcome)?;
        }
        "textDocument/documentSymbol" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/documentSymbol must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<DocumentSymbolParams>(message.params)?;
            match state.analysis.document_symbols(&params.text_document.uri) {
                Ok(symbols) => {
                    let result = serde_json::to_value(symbols)?;
                    writer.write_json(&success_response(id, result))?;
                }
                Err(message) => {
                    writer.write_json(&error_response(id, INVALID_REQUEST, message))?;
                }
            }
        }
        "textDocument/definition" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/definition must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<DefinitionParams>(message.params)?;
            match state
                .analysis
                .goto_definition(&params.text_document.uri, params.position)
            {
                Ok(Some(location)) => {
                    let result = serde_json::to_value(location)?;
                    writer.write_json(&success_response(id, result))?;
                }
                Ok(None) => {
                    writer.write_json(&null_response(id))?;
                }
                Err(message) => {
                    writer.write_json(&error_response(id, INVALID_REQUEST, message))?;
                }
            }
        }
        "textDocument/references" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/references must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<ReferenceParams>(message.params)?;
            match state.analysis.references(
                &params.text_document.uri,
                params.position,
                params.context.include_declaration,
            ) {
                Ok(locations) => {
                    let result = serde_json::to_value(locations)?;
                    writer.write_json(&success_response(id, result))?;
                }
                Err(message) => {
                    writer.write_json(&error_response(id, INVALID_REQUEST, message))?;
                }
            }
        }
        "textDocument/hover" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol("textDocument/hover must be sent as a request".to_string())
            })?;
            let params = required_params::<DefinitionParams>(message.params)?;
            match state
                .analysis
                .hover(&params.text_document.uri, params.position)
            {
                Ok(Some(hover)) => {
                    let result = serde_json::to_value(hover)?;
                    writer.write_json(&success_response(id, result))?;
                }
                Ok(None) => {
                    writer.write_json(&null_response(id))?;
                }
                Err(message) => {
                    writer.write_json(&error_response(id, INVALID_REQUEST, message))?;
                }
            }
        }
        "textDocument/completion" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/completion must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<CompletionParams>(message.params)?;
            match state
                .analysis
                .completion(&params.text_document.uri, params.position)
            {
                Ok(items) => {
                    let result = serde_json::to_value(items)?;
                    writer.write_json(&success_response(id, result))?;
                }
                Err(message) => {
                    writer.write_json(&error_response(id, INVALID_REQUEST, message))?;
                }
            }
        }
        "textDocument/semanticTokens/full" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/semanticTokens/full must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<SemanticTokensParams>(message.params)?;
            match state.analysis.semantic_tokens(&params.text_document.uri) {
                Ok(tokens) => {
                    let result = serde_json::to_value(tokens)?;
                    writer.write_json(&success_response(id, result))?;
                }
                Err(message) => {
                    writer.write_json(&error_response(id, INVALID_REQUEST, message))?;
                }
            }
        }
        "textDocument/prepareRename" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/prepareRename must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<DefinitionParams>(message.params)?;
            match state
                .analysis
                .prepare_rename(&params.text_document.uri, params.position)
            {
                Ok(Some(result)) => {
                    let result = serde_json::to_value(result)?;
                    writer.write_json(&success_response(id, result))?;
                }
                Ok(None) => {
                    writer.write_json(&null_response(id))?;
                }
                Err(message) => {
                    writer.write_json(&error_response(id, INVALID_REQUEST, message))?;
                }
            }
        }
        "textDocument/rename" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol("textDocument/rename must be sent as a request".to_string())
            })?;
            let params = required_params::<RenameParams>(message.params)?;
            match state.analysis.rename(
                &params.text_document.uri,
                params.position,
                &params.new_name,
            ) {
                Ok(workspace_edit) => {
                    let result = serde_json::to_value(workspace_edit)?;
                    writer.write_json(&success_response(id, result))?;
                }
                Err(message) => {
                    writer.write_json(&error_response(id, INVALID_REQUEST, message))?;
                }
            }
        }
        "textDocument/codeAction" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/codeAction must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<CodeActionParams>(message.params)?;
            if !context_allows_quickfix(&params.context.only) {
                writer.write_json(&success_response(id, Value::Array(Vec::new())))?;
            } else {
                match state
                    .analysis
                    .code_actions(&params.text_document.uri, params.range)
                {
                    Ok(actions) => {
                        let result = serde_json::to_value(actions)?;
                        writer.write_json(&success_response(id, result))?;
                    }
                    Err(message) => {
                        writer.write_json(&error_response(id, INVALID_REQUEST, message))?;
                    }
                }
            }
        }
        _ => {
            if let Some(id) = message.id {
                writer.write_json(&error_response(
                    id,
                    METHOD_NOT_FOUND,
                    format!("method `{method}` is not implemented"),
                ))?;
            }
        }
    }

    Ok(false)
}

fn publish_analysis_outcome(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    target_uri: &str,
    outcome: AnalysisOutcome,
) -> Result<(), ServerError> {
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
