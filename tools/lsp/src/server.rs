use crate::analysis::{AnalysisEngine, AnalysisOutcome, cleared_uris};
use crate::protocol::{
    ClientCapabilities, CodeActionParams, CompletionParams, DefinitionParams,
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

const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_REQUEST: i64 = -32600;
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
    analysis: AnalysisEngine,
    published_by_target: BTreeMap<String, BTreeSet<String>>,
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
            analysis,
            published_by_target: BTreeMap::new(),
        }
    }
}

pub fn run_with_analysis(analysis: AnalysisEngine) -> Result<(), ServerError> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = MessageReader::new(BufReader::new(stdin.lock()));
    let mut writer = MessageWriter::new(BufWriter::new(stdout.lock()));
    let mut state = ServerState::with_analysis(analysis);

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
                let id = message.id.unwrap_or(Value::Null);
                writer.write_json(&error_response(
                    id,
                    INVALID_REQUEST,
                    "server is already initialized",
                ))?;
                return Ok(false);
            }
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol("initialize must be sent as a request".to_string())
            })?;
            let params = required_params::<InitializeParams>(message.params)?;
            ensure_utf16_position_encoding(&params.capabilities, id.clone(), writer)?;
            let capabilities = negotiate_capabilities(&params.capabilities);
            state.trace = TraceValue::from_raw(params.trace.as_deref());
            state.initialized = true;
            writer.write_json(&success_response(id, initialize_result(capabilities)))?;
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
        "$/cancelRequest" => {}
        "workspace/didChangeConfiguration" => {
            refresh_workspace(state, writer, "workspace configuration changed")?;
        }
        "workspace/didChangeWatchedFiles" => {
            refresh_workspace(state, writer, "workspace files changed")?;
        }
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
        "textDocument/documentHighlight" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/documentHighlight must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<DocumentHighlightParams>(message.params)?;
            match state
                .analysis
                .document_highlights(&params.text_document.uri, params.position)
            {
                Ok(highlights) => {
                    let result = serde_json::to_value(highlights)?;
                    writer.write_json(&success_response(id, result))?;
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
        "textDocument/signatureHelp" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/signatureHelp must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<SignatureHelpParams>(message.params)?;
            match state
                .analysis
                .signature_help(&params.text_document.uri, params.position)
            {
                Ok(Some(help)) => {
                    let result = serde_json::to_value(help)?;
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

fn refresh_workspace(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    reason: &str,
) -> Result<(), ServerError> {
    for (target_uri, outcome) in state.analysis.refresh_workspace() {
        publish_analysis_outcome(state, writer, &target_uri, outcome)?;
    }
    emit_trace(state, writer, reason, None, true)
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

        assert_eq!(change_messages.len(), 1);
        assert_eq!(
            change_messages[0]["method"],
            "textDocument/publishDiagnostics"
        );
        assert_eq!(change_messages[0]["params"]["uri"], uri);
        assert!(
            change_messages[0]["params"]["diagnostics"]
                .as_array()
                .unwrap()
                .is_empty()
        );
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
