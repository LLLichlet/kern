use super::lifecycle::{
    TraceValue, emit_initialize_followups, emit_trace, ensure_utf16_position_encoding,
    negotiate_capabilities, select_workspace_root,
};
use super::scheduler::{
    drain_scheduler, execute_document_diagnostics, execute_document_request,
    execute_optional_document_request, flush_document_request_results, schedule_workspace_refresh,
    write_error_response, write_null_response, write_success_response,
};
use super::{
    INVALID_REQUEST, METHOD_NOT_FOUND, SERVER_NOT_INITIALIZED, SchedulerLane, ServerError,
    ServerState,
};
use crate::protocol::{
    CancelRequestParams, CodeActionParams, CompletionParams, DefinitionParams,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentHighlightParams, DocumentSymbolParams, IncomingMessage,
    InitializeParams, InlayHintParams, ReferenceParams, RenameParams, SemanticTokensParams,
    SetTraceParams, SignatureHelpParams, error_response, initialize_result, log_message,
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
    handle_message_with_document_request_policy(state, writer, message, true)
}

pub(super) fn handle_message_nonblocking(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    message: IncomingMessage,
) -> Result<bool, ServerError> {
    handle_message_with_document_request_policy(state, writer, message, false)
}

fn handle_message_with_document_request_policy(
    state: &mut ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    message: IncomingMessage,
    wait_for_document_requests: bool,
) -> Result<bool, ServerError> {
    let Some(method) = message.method.as_deref() else {
        if state.is_pending_server_request(message.id.as_ref()) {
            return Ok(false);
        }
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
            let (workspace_root, ignored_workspace_folders) = select_workspace_root(&params);
            state.trace = TraceValue::from_raw(params.trace.as_deref());
            state.work_done_progress = capabilities.work_done_progress;
            state.workspace_root = workspace_root;
            state.ignored_workspace_folders = ignored_workspace_folders;
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
            let params = required_params::<DidSaveTextDocumentParams>(message.params)?;
            let target_uri = params.text_document.uri.clone();
            execute_document_diagnostics(
                state,
                writer,
                &target_uri,
                SchedulerLane::Diagnostics,
                |analysis| analysis.save_document_state(params.text_document.uri),
            )?;
        }
        "textDocument/documentSymbol" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/documentSymbol must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<DocumentSymbolParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            execute_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .document_symbols_in_snapshot(snapshot, &query_uri)
                        .map(|symbols| {
                            symbols
                                .into_iter()
                                .map(crate::analysis::ide::IdeDocumentSymbol::into_lsp)
                                .collect::<Vec<_>>()
                        })
                },
            )?;
        }
        "textDocument/definition" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/definition must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<DefinitionParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            let position = params.position;
            execute_optional_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .goto_definition_in_snapshot(snapshot, &query_uri, position)
                        .map(|location| location.map(crate::analysis::ide::IdeLocation::into_lsp))
                },
            )?;
        }
        "textDocument/documentHighlight" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/documentHighlight must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<DocumentHighlightParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            let position = params.position;
            execute_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .document_highlights_in_snapshot(snapshot, &query_uri, position)
                        .map(|highlights| {
                            highlights
                                .into_iter()
                                .map(crate::analysis::ide::IdeDocumentHighlight::into_lsp)
                                .collect::<Vec<_>>()
                        })
                },
            )?;
        }
        "textDocument/references" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/references must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<ReferenceParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            let position = params.position;
            let include_declaration = params.context.include_declaration;
            execute_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .references_in_snapshot(snapshot, &query_uri, position, include_declaration)
                        .map(|locations| {
                            locations
                                .into_iter()
                                .map(crate::analysis::ide::IdeLocation::into_lsp)
                                .collect::<Vec<_>>()
                        })
                },
            )?;
        }
        "textDocument/hover" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol("textDocument/hover must be sent as a request".to_string())
            })?;
            let params = required_params::<DefinitionParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            let position = params.position;
            #[cfg(test)]
            let barriers = super::TEST_DOCUMENT_REQUEST_BARRIERS
                .lock()
                .unwrap()
                .clone();
            execute_optional_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    #[cfg(test)]
                    if let Some((started, release)) = barriers {
                        started.wait();
                        release.wait();
                    }
                    analysis
                        .hover_in_snapshot(snapshot, &query_uri, position)
                        .map(|hover| hover.map(crate::analysis::ide::IdeHover::into_lsp))
                },
            )?;
        }
        "textDocument/signatureHelp" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/signatureHelp must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<SignatureHelpParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            let position = params.position;
            execute_optional_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .signature_help_in_snapshot(snapshot, &query_uri, position)
                        .map(|help| help.map(crate::analysis::ide::IdeSignatureHelp::into_lsp))
                },
            )?;
        }
        "textDocument/completion" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/completion must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<CompletionParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            let position = params.position;
            execute_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .completion_in_snapshot(snapshot, &query_uri, position)
                        .map(|items| {
                            items
                                .into_iter()
                                .map(crate::analysis::ide::IdeCompletionItem::into_lsp)
                                .collect::<Vec<_>>()
                        })
                },
            )?;
        }
        "textDocument/semanticTokens/full" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/semanticTokens/full must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<SemanticTokensParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            execute_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .semantic_tokens_in_snapshot(snapshot, &query_uri)
                        .map(crate::analysis::ide::IdeSemanticTokens::into_lsp)
                },
            )?;
        }
        "textDocument/inlayHint" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/inlayHint must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<InlayHintParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            let range = params.range;
            execute_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .inlay_hints_in_snapshot(snapshot, &query_uri, range)
                        .map(|hints| {
                            hints
                                .into_iter()
                                .map(crate::analysis::ide::IdeInlayHint::into_lsp)
                                .collect::<Vec<_>>()
                        })
                },
            )?;
        }
        "textDocument/prepareRename" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/prepareRename must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<DefinitionParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            let position = params.position;
            execute_optional_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .prepare_rename_in_snapshot(snapshot, &query_uri, position)
                        .map(|result| {
                            result.map(crate::analysis::ide::IdePrepareRenameResult::into_lsp)
                        })
                },
            )?;
        }
        "textDocument/rename" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol("textDocument/rename must be sent as a request".to_string())
            })?;
            let params = required_params::<RenameParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            let position = params.position;
            let new_name = params.new_name;
            execute_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .rename_in_snapshot(snapshot, &query_uri, position, &new_name)
                        .map(crate::analysis::ide::IdeWorkspaceEdit::into_lsp)
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
            let target_uri = params.text_document.uri;
            if !context_allows_quickfix(&params.context.only) {
                execute_document_request(
                    state,
                    writer,
                    id,
                    &target_uri,
                    SchedulerLane::Interactive,
                    method,
                    |_, _| Ok::<Value, String>(Value::Array(Vec::new())),
                )?;
            } else {
                let query_uri = target_uri.clone();
                let range = params.range;
                execute_document_request(
                    state,
                    writer,
                    id,
                    &target_uri,
                    SchedulerLane::Interactive,
                    method,
                    move |analysis, snapshot| {
                        analysis
                            .code_actions_in_snapshot(snapshot, &query_uri, range)
                            .map(|actions| {
                                actions
                                    .into_iter()
                                    .map(crate::analysis::ide::IdeCodeAction::into_lsp)
                                    .collect::<Vec<_>>()
                            })
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
    } else if wait_for_document_requests {
        flush_document_request_results(state, writer, true)?;
    } else {
        flush_document_request_results(state, writer, false)?;
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
