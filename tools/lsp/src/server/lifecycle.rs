use super::{INVALID_REQUEST, ServerError, ServerState};
use crate::protocol::{
    ClientCapabilities, InitializeParams, InitializeResultOptions, WorkspaceFolder, error_response,
    file_uri_to_path, log_message, log_trace,
};
use crate::transport::MessageWriter;
use serde::Serialize;
use serde_json::Value;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TraceValue {
    Off,
    Messages,
    Verbose,
}

impl TraceValue {
    pub(super) fn from_raw(raw: Option<&str>) -> Self {
        match raw.unwrap_or("off") {
            "messages" => Self::Messages,
            "verbose" => Self::Verbose,
            _ => Self::Off,
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Messages => "messages",
            Self::Verbose => "verbose",
        }
    }
}

pub(super) fn ensure_utf16_position_encoding(
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

pub(super) fn negotiate_capabilities(capabilities: &ClientCapabilities) -> InitializeResultOptions {
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
    let inlay_hint = capabilities.text_document.inlay_hint.is_some();
    let semantic_tokens = capabilities.text_document.semantic_tokens.is_some();
    let work_done_progress = capabilities.window.work_done_progress;

    InitializeResultOptions {
        code_action_literals,
        inlay_hint,
        rename_prepare_support,
        semantic_tokens,
        work_done_progress,
    }
}

pub(super) fn emit_initialize_followups(
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
    if !state.ignored_workspace_folders.is_empty() {
        writer.write_json(&log_message(
            2,
            format!(
                "Kern LSP uses a single primary workspace folder; ignoring {} additional workspace folder(s).",
                state.ignored_workspace_folders.len()
            ),
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
    if let Some(root) = &state.workspace_root {
        verbose.push(format!("workspaceRoot={}", root.display()));
    }
    if !state.ignored_workspace_folders.is_empty() {
        verbose.push(format!(
            "ignoredWorkspaceFolders={}",
            state.ignored_workspace_folders.join(",")
        ));
    }

    emit_trace(
        state,
        writer,
        "initialize completed",
        (!verbose.is_empty()).then(|| verbose.join(" | ")),
        false,
    )
}

pub(super) fn select_workspace_root(params: &InitializeParams) -> (Option<PathBuf>, Vec<String>) {
    let folders = params.workspace_folders.as_deref().unwrap_or(&[]);
    if let Some(first) = folders.first() {
        let root = workspace_folder_path(first);
        let ignored = folders
            .iter()
            .skip(1)
            .map(|folder| folder.uri.clone())
            .collect();
        return (root, ignored);
    }

    let root = params.root_uri.as_deref().and_then(file_uri_to_path);
    (root, Vec::new())
}

fn workspace_folder_path(folder: &WorkspaceFolder) -> Option<PathBuf> {
    file_uri_to_path(&folder.uri)
}

pub(super) fn emit_trace(
    state: &ServerState,
    writer: &mut MessageWriter<impl io::Write>,
    message: impl Into<String>,
    verbose: Option<String>,
    verbose_only: bool,
) -> Result<(), ServerError> {
    let message = message.into();
    let _ = emit_trace_log_sink(state, &message, verbose.as_deref(), verbose_only);
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

#[derive(Serialize)]
struct TraceLogRecord<'a> {
    message: &'a str,
    verbose: Option<&'a str>,
    verbose_only: bool,
}

fn emit_trace_log_sink(
    state: &ServerState,
    message: &str,
    verbose: Option<&str>,
    verbose_only: bool,
) -> Result<(), ServerError> {
    let Some(path) = &state.trace_log_path else {
        return Ok(());
    };
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(
        &mut file,
        &TraceLogRecord {
            message,
            verbose,
            verbose_only,
        },
    )?;
    file.write_all(b"\n")?;
    Ok(())
}
