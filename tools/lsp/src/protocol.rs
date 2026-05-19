//! Minimal LSP protocol data model used by the server.
//!
//! Types in this module mirror the JSON-RPC/LSP payloads that `kern-lsp`
//! decodes and encodes without depending on a large protocol crate.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;

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
    "enumMember",
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
    pub root_uri: Option<String>,
    #[serde(default)]
    pub workspace_folders: Option<Vec<WorkspaceFolder>>,
    #[serde(default)]
    pub trace: Option<String>,
    #[serde(default)]
    pub client_info: Option<ClientInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceFolder {
    pub uri: String,
    #[serde(rename = "name")]
    pub _name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidChangeWorkspaceFoldersParams {
    pub event: WorkspaceFoldersChangeEvent,
}

#[derive(Debug, Deserialize)]
pub struct WorkspaceFoldersChangeEvent {
    #[serde(default)]
    pub added: Vec<WorkspaceFolder>,
    #[serde(default)]
    pub removed: Vec<WorkspaceFolder>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    #[serde(default)]
    pub general: GeneralClientCapabilities,
    #[serde(default)]
    pub window: WindowClientCapabilities,
    #[serde(default)]
    pub text_document: TextDocumentClientCapabilities,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowClientCapabilities {
    #[serde(default)]
    pub work_done_progress: bool,
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
    pub inlay_hint: Option<InlayHintClientCapabilities>,
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
pub struct InlayHintClientCapabilities {
    #[serde(default)]
    pub _dynamic_registration: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticTokensClientCapabilities {
    #[serde(default)]
    pub requests: Option<SemanticTokensClientRequests>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticTokensClientRequests {
    #[serde(default)]
    pub full: Option<SemanticTokensFullClientRequest>,
    #[serde(default)]
    pub _range: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(untagged)]
pub enum SemanticTokensFullClientRequest {
    Bool(bool),
    Options(SemanticTokensFullClientRequestOptions),
    #[default]
    Missing,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticTokensFullClientRequestOptions {
    #[serde(default)]
    pub delta: bool,
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
pub struct CancelRequestParams {
    pub id: Value,
}

#[derive(Debug, Deserialize)]
pub struct DidChangeConfigurationParams {
    #[serde(default)]
    pub settings: Value,
}

#[derive(Debug, Deserialize)]
pub struct DidChangeWatchedFilesParams {
    #[serde(default)]
    pub changes: Vec<FileEvent>,
}

#[derive(Debug, Deserialize)]
pub struct FileEvent {
    pub uri: String,
    #[serde(rename = "type")]
    pub _type: u8,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSymbolParams {
    pub query: String,
    #[serde(default, rename = "workDoneToken")]
    pub work_done_token: Option<Value>,
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
    pub text_document: TextDocumentIdentifier,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionResolveData {
    pub uri: String,
    pub version: i64,
    pub position: Position,
    pub label: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeActionResolveData {
    pub uri: String,
    pub version: i64,
    pub range: Range,
    pub diagnostic_range: Range,
    pub diagnostic_code: Option<String>,
    pub action_kind: String,
    pub fix_id: String,
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
pub struct CallHierarchyPrepareParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallHierarchyIncomingCallsParams {
    pub item: CallHierarchyItem,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallHierarchyOutgoingCallsParams {
    pub item: CallHierarchyItem,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentHighlightParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeLensParams {
    pub text_document: TextDocumentIdentifier,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureHelpParams {
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
    #[serde(default, rename = "workDoneToken")]
    pub work_done_token: Option<Value>,
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
pub struct SemanticTokensDeltaParams {
    pub text_document: TextDocumentIdentifier,
    pub previous_result_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticTokensRangeParams {
    pub text_document: TextDocumentIdentifier,
    pub range: Range,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InlayHintParams {
    pub text_document: TextDocumentIdentifier,
    pub range: Range,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FoldingRangeParams {
    pub text_document: TextDocumentIdentifier,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionRangeParams {
    pub text_document: TextDocumentIdentifier,
    pub positions: Vec<Position>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentLinkParams {
    pub text_document: TextDocumentIdentifier,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeLensResolveData {
    pub title: String,
    pub command: String,
    pub arguments: Vec<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentLinkResolveData {
    pub target: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormattingParams {
    pub text_document: TextDocumentIdentifier,
    #[serde(rename = "options")]
    pub _options: FormattingOptions,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RangeFormattingParams {
    pub text_document: TextDocumentIdentifier,
    pub range: Range,
    #[serde(rename = "options")]
    pub _options: FormattingOptions,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormattingOptions {
    #[serde(rename = "tabSize")]
    pub _tab_size: u32,
    #[serde(rename = "insertSpaces")]
    pub _insert_spaces: bool,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureHelp {
    pub signatures: Vec<SignatureInformation>,
    pub active_signature: u32,
    pub active_parameter: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureInformation {
    pub label: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<ParameterInformation>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterInformation {
    pub label: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarkupContent {
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareRenameResult {
    pub range: Range,
    pub placeholder: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkspaceEdit {
    pub changes: BTreeMap<String, Vec<TextEdit>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeAction {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<Diagnostic>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit: Option<WorkspaceEdit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_preferred: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeLens {
    pub range: Range,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Command>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Command {
    pub title: String,
    pub command: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub insert_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub insert_text_format: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<MarkupContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct ResponseMessage {
    pub jsonrpc: &'static str,
    pub id: Value,
    pub result: Value,
}

#[derive(Debug, Serialize)]
pub struct RequestMessage<T> {
    pub jsonrpc: &'static str,
    pub id: Value,
    pub method: &'static str,
    pub params: T,
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
pub struct WorkDoneProgressCreateParams {
    pub token: Value,
}

#[derive(Debug, Serialize)]
pub struct ProgressParams<T> {
    pub token: Value,
    pub value: T,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WorkDoneProgressValue {
    Begin {
        title: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        percentage: Option<u32>,
    },
    End {
        message: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishDiagnosticsParams {
    pub uri: String,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub range: Range,
    pub severity: u8,
    pub source: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<DiagnosticTag>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_information: Option<Vec<DiagnosticRelatedInformation>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(from = "u8", into = "u8")]
pub enum DiagnosticTag {
    Unnecessary,
    Deprecated,
}

impl From<u8> for DiagnosticTag {
    fn from(value: u8) -> Self {
        match value {
            2 => Self::Deprecated,
            _ => Self::Unnecessary,
        }
    }
}

impl From<DiagnosticTag> for u8 {
    fn from(value: DiagnosticTag) -> Self {
        match value {
            DiagnosticTag::Unnecessary => 1,
            DiagnosticTag::Deprecated => 2,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticTokens {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_id: Option<String>,
    pub data: Vec<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticTokensDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_id: Option<String>,
    pub edits: Vec<SemanticTokensEdit>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticTokensEdit {
    pub start: u32,
    pub delete_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<u32>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum SemanticTokensDeltaResult {
    Tokens(SemanticTokens),
    Delta(SemanticTokensDelta),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InlayHint {
    pub position: Position,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub padding_left: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub padding_right: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FoldingRange {
    pub start_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_character: Option<u32>,
    pub end_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_character: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionRange {
    pub range: Range,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Box<SelectionRange>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentLink {
    pub range: Range,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSymbol {
    pub name: String,
    pub kind: u8,
    pub location: Location,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallHierarchyItem {
    pub name: String,
    pub kind: u8,
    pub uri: String,
    pub range: Range,
    pub selection_range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallHierarchyIncomingCall {
    pub from: CallHierarchyItem,
    pub from_ranges: Vec<Range>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallHierarchyOutgoingCall {
    pub to: CallHierarchyItem,
    pub from_ranges: Vec<Range>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
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
    pub inlay_hint: bool,
    pub rename_prepare_support: bool,
    pub semantic_tokens: bool,
    pub semantic_tokens_delta: bool,
    pub work_done_progress: bool,
}

impl Default for InitializeResultOptions {
    fn default() -> Self {
        Self {
            code_action_literals: true,
            inlay_hint: true,
            rename_prepare_support: true,
            semantic_tokens: true,
            semantic_tokens_delta: true,
            work_done_progress: true,
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
    capabilities.insert("declarationProvider".to_string(), Value::Bool(true));
    capabilities.insert("typeDefinitionProvider".to_string(), Value::Bool(true));
    capabilities.insert("implementationProvider".to_string(), Value::Bool(true));
    capabilities.insert(
        "callHierarchyProvider".to_string(),
        json!({ "workDoneProgress": false }),
    );
    capabilities.insert("documentHighlightProvider".to_string(), Value::Bool(true));
    capabilities.insert(
        "codeLensProvider".to_string(),
        json!({ "resolveProvider": true }),
    );
    capabilities.insert(
        "referencesProvider".to_string(),
        json!({ "workDoneProgress": true }),
    );
    capabilities.insert("hoverProvider".to_string(), Value::Bool(true));
    capabilities.insert("foldingRangeProvider".to_string(), Value::Bool(true));
    capabilities.insert("selectionRangeProvider".to_string(), Value::Bool(true));
    capabilities.insert(
        "documentLinkProvider".to_string(),
        json!({ "resolveProvider": true }),
    );
    capabilities.insert("documentFormattingProvider".to_string(), Value::Bool(true));
    capabilities.insert(
        "documentRangeFormattingProvider".to_string(),
        Value::Bool(true),
    );
    capabilities.insert("workspaceSymbolProvider".to_string(), Value::Bool(true));
    capabilities.insert(
        "workspace".to_string(),
        json!({
            "workspaceFolders": {
                "supported": true,
                "changeNotifications": true
            }
        }),
    );
    capabilities.insert(
        "signatureHelpProvider".to_string(),
        json!({
            "triggerCharacters": ["(", ","],
            "retriggerCharacters": [","]
        }),
    );
    capabilities.insert(
        "completionProvider".to_string(),
        json!({
            "resolveProvider": true,
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
                "range": true,
                "full": {
                    "delta": options.semantic_tokens_delta
                }
            }),
        );
    }
    if options.inlay_hint {
        capabilities.insert("inlayHintProvider".to_string(), Value::Bool(true));
    }
    capabilities.insert(
        "codeActionProvider".to_string(),
        if options.code_action_literals {
            json!({
                "codeActionKinds": ["quickfix"],
                "resolveProvider": true
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

pub fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file://")?;
    let decoded = percent_decode(raw).ok()?;

    #[cfg(windows)]
    {
        let trimmed = normalize_windows_file_uri_path(&decoded);
        let with_separators = trimmed.replace('/', "\\");
        Some(PathBuf::from(with_separators))
    }

    #[cfg(not(windows))]
    {
        Some(PathBuf::from(decoded))
    }
}

pub fn work_done_progress_create(
    id: Value,
    token: Value,
) -> RequestMessage<WorkDoneProgressCreateParams> {
    RequestMessage {
        jsonrpc: JSONRPC_VERSION,
        id,
        method: "window/workDoneProgress/create",
        params: WorkDoneProgressCreateParams { token },
    }
}

#[cfg(windows)]
fn normalize_windows_file_uri_path(decoded: &str) -> &str {
    let trimmed = decoded.trim_start_matches('/');
    trimmed
        .strip_prefix("?/UNC/")
        .or_else(|| trimmed.strip_prefix("?/"))
        .unwrap_or(trimmed)
}

fn percent_decode(input: &str) -> Result<String, ()> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;

    while idx < bytes.len() {
        match bytes[idx] {
            b'%' => {
                if idx + 2 >= bytes.len() {
                    return Err(());
                }
                let hi = hex_value(bytes[idx + 1]).ok_or(())?;
                let lo = hex_value(bytes[idx + 2]).ok_or(())?;
                out.push((hi << 4) | lo);
                idx += 3;
            }
            b => {
                out.push(b);
                idx += 1;
            }
        }
    }

    String::from_utf8(out).map_err(|_| ())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub fn progress(
    token: Value,
    value: WorkDoneProgressValue,
) -> NotificationMessage<ProgressParams<WorkDoneProgressValue>> {
    NotificationMessage {
        jsonrpc: JSONRPC_VERSION,
        method: "$/progress",
        params: ProgressParams { token, value },
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
