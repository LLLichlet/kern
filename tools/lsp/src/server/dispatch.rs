use super::lifecycle::{
    TraceValue, emit_initialize_followups, emit_trace, ensure_utf16_position_encoding,
    negotiate_capabilities,
};
use super::scheduler::{
    drain_scheduler, execute_document_diagnostics, execute_document_request,
    execute_optional_document_request, schedule_workspace_refresh, write_error_response,
    write_null_response, write_success_response,
};
use super::{
    INVALID_REQUEST, METHOD_NOT_FOUND, SERVER_NOT_INITIALIZED, SchedulerLane, ServerError,
    ServerState,
};
use crate::protocol::{
    CancelRequestParams, CodeActionParams, CompletionParams, DefinitionParams,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentHighlightParams, DocumentSymbolParams, IncomingMessage,
    InitializeParams, ReferenceParams, RenameParams, SemanticTokensParams, SetTraceParams,
    SignatureHelpParams, error_response, initialize_result, log_message,
};
use crate::transport::MessageWriter;
use serde_json::Value;
use std::io;

pub(super) fn report_message_error(
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

pub(super) fn handle_message(
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
