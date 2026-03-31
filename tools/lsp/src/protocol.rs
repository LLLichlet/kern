use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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
    pub changes: std::collections::BTreeMap<String, Vec<TextEdit>>,
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

pub fn initialize_result() -> Value {
    json!({
        "positionEncoding": "utf-16",
        "capabilities": {
            "textDocumentSync": {
                "openClose": true,
                "change": TEXT_DOCUMENT_SYNC_INCREMENTAL,
                "save": {
                    "includeText": false
                }
            },
            "documentSymbolProvider": true,
            "definitionProvider": true,
            "referencesProvider": true,
            "hoverProvider": true,
            "completionProvider": {
                "resolveProvider": false,
                "triggerCharacters": ["."]
            },
            "semanticTokensProvider": {
                "legend": {
                    "tokenTypes": SEMANTIC_TOKEN_TYPES,
                    "tokenModifiers": SEMANTIC_TOKEN_MODIFIERS
                },
                "range": false,
                "full": {
                    "delta": false
                }
            },
            "codeActionProvider": {
                "codeActionKinds": ["quickfix"],
                "resolveProvider": false
            },
            "renameProvider": {
                "prepareProvider": true
            }
        },
        "serverInfo": {
            "name": "kern-lsp",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
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
