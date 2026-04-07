use super::{INVALID_REQUEST, ServerError, ServerState};
use crate::protocol::{
    ClientCapabilities, InitializeParams, InitializeResultOptions, error_response, log_message,
    log_trace,
};
use crate::transport::MessageWriter;
use serde_json::Value;
use std::io;

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
    let semantic_tokens = capabilities.text_document.semantic_tokens.is_some();

    InitializeResultOptions {
        code_action_literals,
        rename_prepare_support,
        semantic_tokens,
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

pub(super) fn emit_trace(
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
