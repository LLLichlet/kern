use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const JSONRPC_VERSION: &str = "2.0";
pub const TEXT_DOCUMENT_SYNC_INCREMENTAL: u8 = 2;
pub const SEMANTIC_TOKEN_TYPES: &[&str] = &[
    "namespace",
    "type",
    "struct",
    "enum",
    "interface",
    "typeParameter",
    "parameter",
    "variable",
    "property",
    "function",
    "method",
    "keyword",
    "string",
    "number",
    "operator",
];
pub const SEMANTIC_TOKEN_MODIFIERS: &[&str] = &["declaration", "readonly", "static"];

#[derive(Debug, Deserialize)]
pub struct IncomingMessage {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    #[serde(default)]
    pub capabilities: ClientCapabilities,
    #[serde(default)]
    pub trace: Option<String>,
    #[serde(default)]
    pub client_info: Option<ClientInfo>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    #[serde(default)]
    pub general: GeneralClientCapabilities,
    #[serde(default)]
    pub text_document: TextDocumentClientCapabilities,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneralClientCapabilities {
    #[serde(default)]
    pub position_encodings: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentClientCapabilities {
    #[serde(default)]
    pub code_action: Option<CodeActionClientCapabilities>,
    #[serde(default)]
    pub rename: Option<RenameClientCapabilities>,
    #[serde(default)]
    pub semantic_tokens: Option<SemanticTokensClientCapabilities>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeActionClientCapabilities {
    #[serde(default)]
    pub code_action_literal_support: Option<CodeActionLiteralSupport>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeActionLiteralSupport {
    #[serde(rename = "codeActionKind")]
    pub _code_action_kind: Value,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameClientCapabilities {
    #[serde(default)]
    pub prepare_support: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticTokensClientCapabilities {
    #[serde(default)]
    pub _requests: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetTraceParams {
    pub value: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidOpenTextDocumentParams {
    pub text_document: TextDocumentItem,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentItem {
    pub uri: String,
    #[serde(rename = "languageId")]
    pub _language_id: String,
    pub version: i64,
    pub text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidChangeTextDocumentParams {
    pub text_document: VersionedTextDocumentIdentifier,
    pub content_changes: Vec<TextDocumentContentChangeEvent>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionedTextDocumentIdentifier {
    pub uri: String,
    pub version: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentContentChangeEvent {
    #[serde(default)]
    pub range: Option<Range>,
    pub text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidCloseTextDocumentParams {
    pub text_document: TextDocumentIdentifier,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidSaveTextDocumentParams {
    #[serde(rename = "textDocument")]
    pub _text_document: TextDocumentIdentifier,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSymbolParams {
    pub text_document: TextDocumentIdentifier,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentHighlightParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
    pub context: ReferenceContext,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceContext {
    pub include_declaration: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
    pub new_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticTokensParams {
    pub text_document: TextDocumentIdentifier,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeActionParams {
    pub text_document: TextDocumentIdentifier,
    pub range: Range,
    pub context: CodeActionContext,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeActionContext {
    #[serde(default)]
    #[serde(rename = "diagnostics")]
    pub _diagnostics: Vec<CodeActionDiagnostic>,
    #[serde(default)]
    pub only: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeActionDiagnostic {
    #[serde(rename = "range")]
    pub _range: Range,
    #[serde(rename = "message")]
    pub _message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hover {
    pub contents: MarkupContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
}

#[derive(Debug, Serialize)]
pub struct MarkupContent {
    pub kind: &'static str,
    pub value: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareRenameResult {
    pub range: Range,
    pub placeholder: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceEdit {
    pub changes: BTreeMap<String, Vec<TextEdit>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeAction {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<Diagnostic>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit: Option<WorkspaceEdit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_preferred: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    pub label: String,
    pub kind: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ResponseMessage {
    pub jsonrpc: &'static str,
    pub id: Value,
    pub result: Value,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponseMessage {
    pub jsonrpc: &'static str,
    pub id: Value,
    pub error: ResponseError,
}

#[derive(Debug, Serialize)]
pub struct ResponseError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct NotificationMessage<T> {
    pub jsonrpc: &'static str,
    pub method: &'static str,
    pub params: T,
}

#[derive(Debug, Serialize)]
pub struct LogTraceParams {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verbose: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogMessageParams {
    #[serde(rename = "type")]
    pub typ: u8,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishDiagnosticsParams {
    pub uri: String,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub range: Range,
    pub severity: u8,
    pub source: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_information: Option<Vec<DiagnosticRelatedInformation>>,
}

#[derive(Debug, Serialize)]
pub struct SemanticTokens {
    pub data: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRelatedInformation {
    pub location: Location,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentHighlight {
    pub range: Range,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<u8>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSymbol {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub kind: u8,
    pub range: Range,
    pub selection_range: Range,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<DocumentSymbol>,
}

#[derive(Debug, Clone, Copy)]
pub struct InitializeResultOptions {
    pub code_action_literals: bool,
    pub rename_prepare_support: bool,
    pub semantic_tokens: bool,
}

impl Default for InitializeResultOptions {
    fn default() -> Self {
        Self {
            code_action_literals: true,
            rename_prepare_support: true,
            semantic_tokens: true,
        }
    }
}

pub fn initialize_result(options: InitializeResultOptions) -> Value {
    let mut capabilities = serde_json::Map::new();
    capabilities.insert(
        "textDocumentSync".to_string(),
        json!({
            "openClose": true,
            "change": TEXT_DOCUMENT_SYNC_INCREMENTAL,
            "save": {
                "includeText": false
            }
        }),
    );
    capabilities.insert("documentSymbolProvider".to_string(), Value::Bool(true));
    capabilities.insert("definitionProvider".to_string(), Value::Bool(true));
    capabilities.insert("documentHighlightProvider".to_string(), Value::Bool(true));
    capabilities.insert("referencesProvider".to_string(), Value::Bool(true));
    capabilities.insert("hoverProvider".to_string(), Value::Bool(true));
    capabilities.insert(
        "completionProvider".to_string(),
        json!({
            "resolveProvider": false,
            "triggerCharacters": ["."]
        }),
    );
    if options.semantic_tokens {
        capabilities.insert(
            "semanticTokensProvider".to_string(),
            json!({
                "legend": {
                    "tokenTypes": SEMANTIC_TOKEN_TYPES,
                    "tokenModifiers": SEMANTIC_TOKEN_MODIFIERS
                },
                "range": false,
                "full": {
                    "delta": false
                }
            }),
        );
    }
    capabilities.insert(
        "codeActionProvider".to_string(),
        if options.code_action_literals {
            json!({
                "codeActionKinds": ["quickfix"],
                "resolveProvider": false
            })
        } else {
            Value::Bool(false)
        },
    );
    capabilities.insert(
        "renameProvider".to_string(),
        if options.rename_prepare_support {
            json!({
                "prepareProvider": true
            })
        } else {
            Value::Bool(true)
        },
    );

    Value::Object(serde_json::Map::from_iter([
        (
            "positionEncoding".to_string(),
            Value::String("utf-16".to_string()),
        ),
        ("capabilities".to_string(), Value::Object(capabilities)),
        (
            "serverInfo".to_string(),
            json!({
                "name": "kern-lsp",
                "version": env!("CARGO_PKG_VERSION")
            }),
        ),
    ]))
}

pub fn success_response(id: Value, result: Value) -> ResponseMessage {
    ResponseMessage {
        jsonrpc: JSONRPC_VERSION,
        id,
        result,
    }
}

pub fn null_response(id: Value) -> ResponseMessage {
    success_response(id, Value::Null)
}

pub fn error_response(id: Value, code: i64, message: impl Into<String>) -> ErrorResponseMessage {
    ErrorResponseMessage {
        jsonrpc: JSONRPC_VERSION,
        id,
        error: ResponseError {
            code,
            message: message.into(),
        },
    }
}

pub fn publish_diagnostics(
    uri: String,
    diagnostics: Vec<Diagnostic>,
) -> NotificationMessage<PublishDiagnosticsParams> {
    NotificationMessage {
        jsonrpc: JSONRPC_VERSION,
        method: "textDocument/publishDiagnostics",
        params: PublishDiagnosticsParams { uri, diagnostics },
    }
}

pub fn log_trace(
    message: impl Into<String>,
    verbose: Option<String>,
) -> NotificationMessage<LogTraceParams> {
    NotificationMessage {
        jsonrpc: JSONRPC_VERSION,
        method: "$/logTrace",
        params: LogTraceParams {
            message: message.into(),
            verbose,
        },
    }
}

pub fn log_message(typ: u8, message: impl Into<String>) -> NotificationMessage<LogMessageParams> {
    NotificationMessage {
        jsonrpc: JSONRPC_VERSION,
        method: "window/logMessage",
        params: LogMessageParams {
            typ,
            message: message.into(),
        },
    }
}
