//! Request and notification dispatch for JSON-RPC messages.
//!
//! Dispatch validates protocol parameters, routes each method to analysis or
//! lifecycle handlers, and decides when pending scheduler work should be
//! flushed.

use super::configuration::{ConfigurationChange, handle_configuration_change};
use super::lifecycle::{
    TraceValue, emit_initialize_followups, emit_trace, ensure_utf16_position_encoding,
    negotiate_capabilities, normalize_workspace_roots, select_workspace_roots,
};
use super::scheduler::{
    drain_scheduler, execute_document_diagnostics, execute_document_request,
    execute_document_request_with_progress, execute_optional_document_request,
    execute_raw_document_request, execute_request_with_progress, flush_document_request_results,
    schedule_workspace_refresh, write_error_response, write_null_response, write_success_response,
};
use super::{
    INVALID_REQUEST, METHOD_NOT_FOUND, SERVER_NOT_INITIALIZED, SchedulerLane, ServerError,
    ServerState, WorkspaceRefreshKind,
};
use crate::analysis::{
    IdeChangeDocument, IdeCloseDocument, IdeOpenDocument, IdePosition, IdeRange,
    IdeTextDocumentChange,
};
use crate::protocol::{
    CallHierarchyIncomingCallsParams, CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams,
    CancelRequestParams, CodeAction, CodeActionParams, CodeActionResolveData, CodeLens,
    CodeLensParams, CodeLensResolveData, CompletionItem, CompletionParams, CompletionResolveData,
    DefinitionParams, DidChangeConfigurationParams, DidChangeTextDocumentParams,
    DidChangeWatchedFilesParams, DidChangeWorkspaceFoldersParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentHighlightParams, DocumentLink,
    DocumentLinkParams, DocumentLinkResolveData, DocumentSymbolParams, FoldingRangeParams,
    FormattingParams, IncomingMessage, InitializeParams, InlayHintParams, MarkupContent, Position,
    Range, RangeFormattingParams, ReferenceParams, RenameParams, SelectionRangeParams,
    SemanticTokensDeltaParams, SemanticTokensParams, SemanticTokensRangeParams, SetTraceParams,
    SignatureHelpParams, WorkspaceSymbolParams, error_response, initialize_result, log_message,
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
            let workspace_roots = select_workspace_roots(&params);
            state.trace = TraceValue::from_raw(params.trace.as_deref());
            state.work_done_progress = capabilities.work_done_progress;
            state.workspace_roots = workspace_roots;
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
            let params = message
                .params
                .map(serde_json::from_value::<DidChangeConfigurationParams>)
                .transpose()?
                .unwrap_or(DidChangeConfigurationParams {
                    settings: Value::Null,
                });
            if handle_configuration_change(state, writer, params)? == ConfigurationChange::Changed {
                schedule_workspace_refresh(
                    state,
                    writer,
                    "workspace configuration changed",
                    WorkspaceRefreshKind::ProjectMetadata,
                )?;
            }
        }
        "workspace/didChangeWatchedFiles" => {
            let params = message
                .params
                .map(serde_json::from_value::<DidChangeWatchedFilesParams>)
                .transpose()?
                .unwrap_or(DidChangeWatchedFilesParams {
                    changes: Vec::new(),
                });
            let changed_uris = params
                .changes
                .into_iter()
                .map(|change| change.uri)
                .collect::<Vec<_>>();
            let kind = if crate::analysis::AnalysisEngine::watched_files_require_project_reload(
                &changed_uris,
            ) {
                WorkspaceRefreshKind::ProjectMetadata
            } else {
                WorkspaceRefreshKind::Sources
            };
            let reason = match kind {
                WorkspaceRefreshKind::Sources => "workspace source files changed",
                WorkspaceRefreshKind::ProjectMetadata => "workspace project metadata changed",
            };
            schedule_workspace_refresh(state, writer, reason, kind)?;
        }
        "workspace/didChangeWorkspaceFolders" => {
            let params = required_params::<DidChangeWorkspaceFoldersParams>(message.params)?;
            let removed = params
                .event
                .removed
                .iter()
                .filter_map(|folder| crate::protocol::file_uri_to_path(&folder.uri))
                .collect::<Vec<_>>();
            let mut roots = state
                .workspace_roots
                .iter()
                .filter(|root| !removed.iter().any(|removed| removed == *root))
                .cloned()
                .collect::<Vec<_>>();
            roots.extend(
                params
                    .event
                    .added
                    .iter()
                    .filter_map(|folder| crate::protocol::file_uri_to_path(&folder.uri)),
            );
            state.workspace_roots = normalize_workspace_roots(roots);
            schedule_workspace_refresh(
                state,
                writer,
                "workspace folders changed",
                WorkspaceRefreshKind::ProjectMetadata,
            )?;
        }
        "workspace/symbol" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol("workspace/symbol must be sent as a request".to_string())
            })?;
            let params = required_params::<WorkspaceSymbolParams>(message.params)?;
            let query = params.query;
            execute_request_with_progress(
                state,
                writer,
                id,
                "<workspace>",
                SchedulerLane::Interactive,
                method,
                params.work_done_token,
                "Kern workspace symbols",
                "Searching workspace symbols",
                move |analysis, snapshot| {
                    analysis
                        .workspace_symbols_in_snapshot(snapshot, &query)
                        .map(|symbols| {
                            symbols
                                .into_iter()
                                .map(crate::analysis::ide::IdeWorkspaceSymbol::into_lsp)
                                .collect::<Vec<_>>()
                        })
                },
            )?;
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
            let document = ide_open_document_from_protocol(params);
            execute_document_diagnostics(
                state,
                writer,
                &target_uri,
                SchedulerLane::Diagnostics,
                |analysis| analysis.open_document_state(document),
            )?;
        }
        "textDocument/didChange" => {
            let params = required_params::<DidChangeTextDocumentParams>(message.params)?;
            let target_uri = params.text_document.uri.clone();
            state.clear_semantic_tokens_results_for_uri(&target_uri);
            let change = ide_change_document_from_protocol(params);
            execute_document_diagnostics(
                state,
                writer,
                &target_uri,
                SchedulerLane::Diagnostics,
                |analysis| analysis.change_document_state(change),
            )?;
        }
        "textDocument/didClose" => {
            let params = required_params::<DidCloseTextDocumentParams>(message.params)?;
            let target_uri = params.text_document.uri.clone();
            state.clear_semantic_tokens_results_for_uri(&target_uri);
            let document = ide_close_document_from_protocol(params);
            execute_document_diagnostics(
                state,
                writer,
                &target_uri,
                SchedulerLane::Diagnostics,
                |analysis| analysis.close_document_state(document),
            )?;
            state.clear_active_document(&target_uri);
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
            let position = ide_position_from_protocol(params.position);
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
        "textDocument/declaration" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/declaration must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<DefinitionParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            let position = ide_position_from_protocol(params.position);
            execute_optional_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .goto_declaration_in_snapshot(snapshot, &query_uri, position)
                        .map(|location| location.map(crate::analysis::ide::IdeLocation::into_lsp))
                },
            )?;
        }
        "textDocument/typeDefinition" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/typeDefinition must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<DefinitionParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            let position = ide_position_from_protocol(params.position);
            execute_optional_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .goto_type_definition_in_snapshot(snapshot, &query_uri, position)
                        .map(|location| location.map(crate::analysis::ide::IdeLocation::into_lsp))
                },
            )?;
        }
        "textDocument/implementation" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/implementation must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<DefinitionParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            let position = ide_position_from_protocol(params.position);
            execute_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .implementation_locations_in_snapshot(snapshot, &query_uri, position)
                        .map(|locations| {
                            locations
                                .into_iter()
                                .map(crate::analysis::ide::IdeLocation::into_lsp)
                                .collect::<Vec<_>>()
                        })
                },
            )?;
        }
        "textDocument/prepareCallHierarchy" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/prepareCallHierarchy must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<CallHierarchyPrepareParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            let position = ide_position_from_protocol(params.position);
            execute_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .prepare_call_hierarchy_in_snapshot(snapshot, &query_uri, position)
                        .map(|item| {
                            item.into_iter()
                                .map(crate::analysis::ide::IdeCallHierarchyItem::into_lsp)
                                .collect::<Vec<_>>()
                        })
                },
            )?;
        }
        "callHierarchy/incomingCalls" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "callHierarchy/incomingCalls must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<CallHierarchyIncomingCallsParams>(message.params)?;
            let target_uri = params.item.uri.clone();
            let query_uri = target_uri.clone();
            let target_range = ide_range_from_protocol(params.item.selection_range);
            execute_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .call_hierarchy_incoming_calls_in_snapshot(
                            snapshot,
                            &query_uri,
                            target_range,
                        )
                        .map(|calls| {
                            calls
                                .into_iter()
                                .map(crate::analysis::ide::IdeCallHierarchyIncomingCall::into_lsp)
                                .collect::<Vec<_>>()
                        })
                },
            )?;
        }
        "callHierarchy/outgoingCalls" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "callHierarchy/outgoingCalls must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<CallHierarchyOutgoingCallsParams>(message.params)?;
            let target_uri = params.item.uri.clone();
            let query_uri = target_uri.clone();
            let target_range = ide_range_from_protocol(params.item.selection_range);
            execute_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .call_hierarchy_outgoing_calls_in_snapshot(
                            snapshot,
                            &query_uri,
                            target_range,
                        )
                        .map(|calls| {
                            calls
                                .into_iter()
                                .map(crate::analysis::ide::IdeCallHierarchyOutgoingCall::into_lsp)
                                .collect::<Vec<_>>()
                        })
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
            let position = ide_position_from_protocol(params.position);
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
            let position = ide_position_from_protocol(params.position);
            let include_declaration = params.context.include_declaration;
            let work_done_token = params.work_done_token;
            execute_document_request_with_progress(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                work_done_token,
                "Kern workspace references",
                "Searching workspace references",
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
            let position = ide_position_from_protocol(params.position);
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
            let position = ide_position_from_protocol(params.position);
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
            let position = ide_position_from_protocol(params.position);
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
        "completionItem/resolve" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "completionItem/resolve must be sent as a request".to_string(),
                )
            })?;
            let mut item = required_params::<CompletionItem>(message.params)?;
            let resolve_data = item
                .data
                .clone()
                .and_then(|data| serde_json::from_value::<CompletionResolveData>(data).ok());
            if let Some(resolve_data) = resolve_data {
                let target_uri = resolve_data.uri.clone();
                execute_document_request(
                    state,
                    writer,
                    id,
                    &target_uri,
                    SchedulerLane::Interactive,
                    method,
                    move |analysis, snapshot| {
                        let mut resolved_item = item;
                        if resolved_item.documentation.is_none()
                            && let Some(resolved) = analysis
                                .resolve_completion_item_in_snapshot(snapshot, &resolve_data)?
                            && let Some(documentation) = resolved.documentation
                        {
                            resolved_item.documentation = Some(MarkupContent {
                                kind: "markdown".to_string(),
                                value: documentation,
                            });
                        }
                        resolved_item.data = None;
                        Ok(resolved_item)
                    },
                )?;
            } else {
                let request = state.request_context(id);
                item.data = None;
                write_success_response(state, writer, &request, serde_json::to_value(item)?)?;
            }
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
            execute_raw_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .semantic_tokens_in_snapshot(snapshot, &query_uri)
                        .map(
                            |tokens| super::DocumentRequestResponse::SemanticTokensFull {
                                uri: query_uri,
                                data: tokens.data,
                            },
                        )
                },
            )?;
        }
        "textDocument/semanticTokens/full/delta" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/semanticTokens/full/delta must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<SemanticTokensDeltaParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            let previous_result_id = params.previous_result_id;
            execute_raw_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .semantic_tokens_in_snapshot(snapshot, &query_uri)
                        .map(
                            |tokens| super::DocumentRequestResponse::SemanticTokensDelta {
                                uri: query_uri,
                                previous_result_id,
                                data: tokens.data,
                            },
                        )
                },
            )?;
        }
        "textDocument/semanticTokens/range" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/semanticTokens/range must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<SemanticTokensRangeParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            let range = ide_range_from_protocol(params.range);
            execute_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .semantic_tokens_range_in_snapshot(snapshot, &query_uri, range)
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
            let range = ide_range_from_protocol(params.range);
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
        "textDocument/foldingRange" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/foldingRange must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<FoldingRangeParams>(message.params)?;
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
                        .folding_ranges_in_snapshot(snapshot, &query_uri)
                        .map(|ranges| {
                            ranges
                                .into_iter()
                                .map(crate::analysis::ide::IdeFoldingRange::into_lsp)
                                .collect::<Vec<_>>()
                        })
                },
            )?;
        }
        "textDocument/selectionRange" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/selectionRange must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<SelectionRangeParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            let positions = params
                .positions
                .into_iter()
                .map(ide_position_from_protocol)
                .collect();
            execute_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .selection_ranges_in_snapshot(snapshot, &query_uri, positions)
                        .map(|ranges| {
                            ranges
                                .into_iter()
                                .map(crate::analysis::ide::IdeSelectionRange::into_lsp)
                                .collect::<Vec<_>>()
                        })
                },
            )?;
        }
        "textDocument/codeLens" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol("textDocument/codeLens must be sent as a request".to_string())
            })?;
            let params = required_params::<CodeLensParams>(message.params)?;
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
                        .code_lenses_in_snapshot(snapshot, &query_uri)
                        .map(|lenses| {
                            lenses
                                .into_iter()
                                .map(crate::analysis::ide::IdeCodeLens::into_lsp)
                                .collect::<Vec<_>>()
                        })
                },
            )?;
        }
        "codeLens/resolve" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol("codeLens/resolve must be sent as a request".to_string())
            })?;
            let mut lens = required_params::<CodeLens>(message.params)?;
            let resolve_data = lens
                .data
                .clone()
                .and_then(|data| serde_json::from_value::<CodeLensResolveData>(data).ok());
            if let Some(resolve_data) = resolve_data {
                lens.command = Some(crate::protocol::Command {
                    title: resolve_data.title,
                    command: resolve_data.command,
                    arguments: resolve_data.arguments,
                });
            }
            lens.data = None;
            let request = state.request_context(id);
            write_success_response(state, writer, &request, serde_json::to_value(lens)?)?;
        }
        "textDocument/documentLink" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/documentLink must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<DocumentLinkParams>(message.params)?;
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
                        .document_links_in_snapshot(snapshot, &query_uri)
                        .map(|links| {
                            links
                                .into_iter()
                                .map(crate::analysis::ide::IdeDocumentLink::into_lsp)
                                .collect::<Vec<_>>()
                        })
                },
            )?;
        }
        "documentLink/resolve" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol("documentLink/resolve must be sent as a request".to_string())
            })?;
            let mut link = required_params::<DocumentLink>(message.params)?;
            let resolve_data = link
                .data
                .clone()
                .and_then(|data| serde_json::from_value::<DocumentLinkResolveData>(data).ok());
            if let Some(resolve_data) = resolve_data {
                link.target = Some(resolve_data.target);
            }
            link.data = None;
            let request = state.request_context(id);
            write_success_response(state, writer, &request, serde_json::to_value(link)?)?;
        }
        "textDocument/formatting" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/formatting must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<FormattingParams>(message.params)?;
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
                        .formatting_edits_in_snapshot(snapshot, &query_uri)
                        .map(|edits| {
                            edits
                                .into_iter()
                                .map(crate::analysis::ide::IdeTextEdit::into_lsp)
                                .collect::<Vec<_>>()
                        })
                },
            )?;
        }
        "textDocument/rangeFormatting" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol(
                    "textDocument/rangeFormatting must be sent as a request".to_string(),
                )
            })?;
            let params = required_params::<RangeFormattingParams>(message.params)?;
            let target_uri = params.text_document.uri;
            let query_uri = target_uri.clone();
            let range = ide_range_from_protocol(params.range);
            execute_document_request(
                state,
                writer,
                id,
                &target_uri,
                SchedulerLane::Interactive,
                method,
                move |analysis, snapshot| {
                    analysis
                        .range_formatting_edits_in_snapshot(snapshot, &query_uri, range)
                        .map(|edits| {
                            edits
                                .into_iter()
                                .map(crate::analysis::ide::IdeTextEdit::into_lsp)
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
            let position = ide_position_from_protocol(params.position);
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
            let position = ide_position_from_protocol(params.position);
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
                let range = ide_range_from_protocol(params.range);
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
        "codeAction/resolve" => {
            let id = message.id.ok_or_else(|| {
                ServerError::Protocol("codeAction/resolve must be sent as a request".to_string())
            })?;
            let mut action = required_params::<CodeAction>(message.params)?;
            let resolve_data = action
                .data
                .clone()
                .and_then(|data| serde_json::from_value::<CodeActionResolveData>(data).ok());
            if let Some(resolve_data) = resolve_data {
                let target_uri = resolve_data.uri.clone();
                execute_document_request(
                    state,
                    writer,
                    id,
                    &target_uri,
                    SchedulerLane::Interactive,
                    method,
                    move |analysis, snapshot| {
                        if action.edit.is_none()
                            && let Some(resolved) =
                                analysis.resolve_code_action_in_snapshot(snapshot, &resolve_data)?
                            && let Some(edit) = resolved.edit
                        {
                            action.edit = Some(edit.into_lsp());
                        }
                        action.data = None;
                        Ok(action)
                    },
                )?;
            } else {
                let request = state.request_context(id);
                action.data = None;
                write_success_response(state, writer, &request, serde_json::to_value(action)?)?;
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

pub(crate) fn ide_open_document_from_protocol(
    params: DidOpenTextDocumentParams,
) -> IdeOpenDocument {
    IdeOpenDocument {
        uri: params.text_document.uri,
        version: params.text_document.version,
        text: params.text_document.text,
    }
}

pub(crate) fn ide_change_document_from_protocol(
    params: DidChangeTextDocumentParams,
) -> IdeChangeDocument {
    IdeChangeDocument {
        uri: params.text_document.uri,
        version: params.text_document.version,
        changes: params
            .content_changes
            .into_iter()
            .map(|change| IdeTextDocumentChange {
                range: change.range.map(|range| IdeRange {
                    start: IdePosition {
                        line: range.start.line,
                        character: range.start.character,
                    },
                    end: IdePosition {
                        line: range.end.line,
                        character: range.end.character,
                    },
                }),
                text: change.text,
            })
            .collect(),
    }
}

pub(crate) fn ide_close_document_from_protocol(
    params: DidCloseTextDocumentParams,
) -> IdeCloseDocument {
    IdeCloseDocument {
        uri: params.text_document.uri,
    }
}

pub(crate) fn ide_position_from_protocol(position: Position) -> IdePosition {
    IdePosition {
        line: position.line,
        character: position.character,
    }
}

pub(crate) fn ide_range_from_protocol(range: Range) -> IdeRange {
    IdeRange {
        start: ide_position_from_protocol(range.start),
        end: ide_position_from_protocol(range.end),
    }
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
