use crate::protocol::{
    CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, CodeAction,
    CodeActionResolveData, CodeLens, Command, CompletionItem, CompletionResolveData, Diagnostic,
    DiagnosticRelatedInformation, DiagnosticTag, DocumentHighlight, DocumentLink, DocumentSymbol,
    FoldingRange, InlayHint, Location, ParameterInformation, PrepareRenameResult, SelectionRange,
    SemanticTokens, SignatureHelp, SignatureInformation, TextEdit, WorkspaceEdit, WorkspaceSymbol,
};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub(crate) struct IdeCodeAction {
    pub title: String,
    pub kind: Option<&'static str>,
    pub diagnostics: Vec<IdeDiagnostic>,
    pub edit: Option<IdeWorkspaceEdit>,
    pub is_preferred: Option<bool>,
    pub fix_id: Option<&'static str>,
    pub resolve_data: Option<CodeActionResolveData>,
}

#[derive(Debug, Clone)]
pub(crate) struct IdeWorkspaceEdit {
    pub changes: BTreeMap<String, Vec<IdeTextEdit>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdeTextEdit {
    pub range: super::IdeRange,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdeLocation {
    pub uri: String,
    pub range: super::IdeRange,
}

#[derive(Debug, Clone)]
pub(crate) struct IdeDocumentSymbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: IdeSymbolKind,
    pub range: super::IdeRange,
    pub selection_range: super::IdeRange,
    pub children: Vec<IdeDocumentSymbol>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdeWorkspaceSymbol {
    pub name: String,
    pub kind: IdeSymbolKind,
    pub location: IdeLocation,
    pub container_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdeCallHierarchyItem {
    pub name: String,
    pub kind: IdeSymbolKind,
    pub uri: String,
    pub range: super::IdeRange,
    pub selection_range: super::IdeRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdeCallHierarchyIncomingCall {
    pub from: IdeCallHierarchyItem,
    pub from_ranges: Vec<super::IdeRange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdeCallHierarchyOutgoingCall {
    pub to: IdeCallHierarchyItem,
    pub from_ranges: Vec<super::IdeRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdeSymbolKind {
    Module,
    Namespace,
    Struct,
    Trait,
    Method,
    Function,
    Enum,
    TypeAlias,
    Constant,
    Static,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdeDocumentHighlight {
    pub range: super::IdeRange,
    pub kind: Option<IdeDocumentHighlightKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdeFoldingRange {
    pub start_line: u32,
    pub start_character: Option<u32>,
    pub end_line: u32,
    pub end_character: Option<u32>,
    pub kind: Option<IdeFoldingRangeKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdeFoldingRangeKind {
    Comment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdeSelectionRange {
    pub range: super::IdeRange,
    pub parent: Option<Box<IdeSelectionRange>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdeDocumentLink {
    pub range: super::IdeRange,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdeCodeLens {
    pub range: super::IdeRange,
    pub title: String,
    pub command: String,
    pub arguments: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdeDocumentHighlightKind {
    Text,
}

#[derive(Debug, Clone)]
pub(crate) struct IdeInlayHint {
    pub position: super::IdePosition,
    pub label: String,
    pub kind: Option<IdeInlayHintKind>,
    pub padding_left: Option<bool>,
    pub padding_right: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdeInlayHintKind {
    Type,
}

#[derive(Debug, Clone)]
pub(crate) struct IdeSignatureHelp {
    pub signatures: Vec<IdeSignatureInformation>,
    pub active_signature: u32,
    pub active_parameter: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct IdeSignatureInformation {
    pub label: String,
    pub parameters: Vec<IdeParameterInformation>,
}

#[derive(Debug, Clone)]
pub(crate) struct IdeParameterInformation {
    pub label: String,
}

#[derive(Debug, Clone)]
pub(crate) struct IdePrepareRenameResult {
    pub range: super::IdeRange,
    pub placeholder: String,
}

#[derive(Debug, Clone)]
pub(crate) struct IdeSemanticTokens {
    pub data: Vec<u32>,
}

#[derive(Debug, Clone)]
pub(crate) struct IdeCompletionItem {
    pub label: String,
    pub kind: IdeCompletionKind,
    pub detail: Option<String>,
    pub insert_text: Option<String>,
    pub documentation: Option<String>,
    pub resolve_data: Option<CompletionResolveData>,
}

#[derive(Debug, Clone)]
pub(crate) struct IdeHover {
    pub contents: String,
    pub range: Option<super::IdeRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdeCompletionKind {
    Variable,
    Function,
    Module,
    Struct,
    Union,
    Enum,
    Trait,
    TypeAlias,
    Constant,
    Static,
    TypeParameter,
    Keyword,
}

#[derive(Debug, Clone)]
pub(crate) struct IdeDiagnostic {
    pub range: super::IdeRange,
    pub severity: IdeDiagnosticSeverity,
    pub source: &'static str,
    pub message: String,
    pub code: Option<String>,
    pub tags: Vec<IdeDiagnosticTag>,
    pub related_information: Vec<IdeDiagnosticRelatedInformation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdeDiagnosticSeverity {
    Error,
    Warning,
    Information,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum IdeDiagnosticTag {
    Unnecessary,
    Deprecated,
}

#[derive(Debug, Clone)]
pub(crate) struct IdeDiagnosticRelatedInformation {
    pub location: IdeLocation,
    pub message: String,
}

impl IdeCodeAction {
    pub(crate) fn into_lsp(self) -> CodeAction {
        CodeAction {
            title: self.title,
            kind: self.kind.map(str::to_string),
            diagnostics: (!self.diagnostics.is_empty()).then(|| {
                self.diagnostics
                    .into_iter()
                    .map(IdeDiagnostic::into_lsp)
                    .collect()
            }),
            edit: self.edit.map(IdeWorkspaceEdit::into_lsp),
            is_preferred: self.is_preferred,
            data: self
                .resolve_data
                .map(|data| serde_json::to_value(data).expect("code action resolve data encodes")),
        }
    }
}

impl IdeWorkspaceEdit {
    pub(crate) fn into_lsp(self) -> WorkspaceEdit {
        WorkspaceEdit {
            changes: self
                .changes
                .into_iter()
                .map(|(uri, edits)| (uri, edits.into_iter().map(IdeTextEdit::into_lsp).collect()))
                .collect(),
        }
    }
}

impl IdeTextEdit {
    pub(crate) fn into_lsp(self) -> TextEdit {
        TextEdit {
            range: self.range.into(),
            new_text: self.new_text,
        }
    }
}

impl IdeLocation {
    pub(crate) fn into_lsp(self) -> Location {
        Location {
            uri: self.uri,
            range: self.range.into(),
        }
    }
}

impl IdeDocumentSymbol {
    pub(crate) fn into_lsp(self) -> DocumentSymbol {
        DocumentSymbol {
            name: self.name,
            detail: self.detail,
            kind: self.kind.into_lsp(),
            range: self.range.into(),
            selection_range: self.selection_range.into(),
            children: self
                .children
                .into_iter()
                .map(IdeDocumentSymbol::into_lsp)
                .collect(),
        }
    }
}

impl IdeWorkspaceSymbol {
    pub(crate) fn into_lsp(self) -> WorkspaceSymbol {
        WorkspaceSymbol {
            name: self.name,
            kind: self.kind.into_lsp(),
            location: self.location.into_lsp(),
            container_name: self.container_name,
        }
    }
}

impl IdeCodeLens {
    pub(crate) fn into_lsp(self) -> CodeLens {
        CodeLens {
            range: self.range.into(),
            command: Command {
                title: self.title,
                command: self.command,
                arguments: self.arguments,
            },
        }
    }
}

impl IdeCallHierarchyItem {
    pub(crate) fn into_lsp(self) -> CallHierarchyItem {
        CallHierarchyItem {
            name: self.name,
            kind: self.kind.into_lsp(),
            uri: self.uri,
            range: self.range.into(),
            selection_range: self.selection_range.into(),
        }
    }
}

impl IdeCallHierarchyIncomingCall {
    pub(crate) fn into_lsp(self) -> CallHierarchyIncomingCall {
        CallHierarchyIncomingCall {
            from: self.from.into_lsp(),
            from_ranges: self.from_ranges.into_iter().map(Into::into).collect(),
        }
    }
}

impl IdeCallHierarchyOutgoingCall {
    pub(crate) fn into_lsp(self) -> CallHierarchyOutgoingCall {
        CallHierarchyOutgoingCall {
            to: self.to.into_lsp(),
            from_ranges: self.from_ranges.into_iter().map(Into::into).collect(),
        }
    }
}

impl IdeSymbolKind {
    fn into_lsp(self) -> u8 {
        match self {
            Self::Module => 2,
            Self::Namespace => 3,
            Self::Struct => 23,
            Self::Trait => 11,
            Self::Method => 6,
            Self::Function => 12,
            Self::Enum => 10,
            Self::TypeAlias => 13,
            Self::Constant => 14,
            Self::Static => 13,
        }
    }
}

impl IdeDocumentHighlight {
    pub(crate) fn into_lsp(self) -> DocumentHighlight {
        DocumentHighlight {
            range: self.range.into(),
            kind: self.kind.map(IdeDocumentHighlightKind::into_lsp),
        }
    }
}

impl IdeDocumentHighlightKind {
    fn into_lsp(self) -> u8 {
        match self {
            Self::Text => 1,
        }
    }
}

impl IdeFoldingRange {
    pub(crate) fn into_lsp(self) -> FoldingRange {
        FoldingRange {
            start_line: self.start_line,
            start_character: self.start_character,
            end_line: self.end_line,
            end_character: self.end_character,
            kind: self.kind.map(IdeFoldingRangeKind::into_lsp),
        }
    }
}

impl IdeFoldingRangeKind {
    fn into_lsp(self) -> &'static str {
        match self {
            Self::Comment => "comment",
        }
    }
}

impl IdeSelectionRange {
    pub(crate) fn into_lsp(self) -> SelectionRange {
        SelectionRange {
            range: self.range.into(),
            parent: self.parent.map(|parent| Box::new(parent.into_lsp())),
        }
    }
}

impl IdeDocumentLink {
    pub(crate) fn into_lsp(self) -> DocumentLink {
        DocumentLink {
            range: self.range.into(),
            target: self.target,
        }
    }
}

impl IdeInlayHint {
    pub(crate) fn into_lsp(self) -> InlayHint {
        InlayHint {
            position: self.position.into(),
            label: self.label,
            kind: self.kind.map(IdeInlayHintKind::into_lsp),
            padding_left: self.padding_left,
            padding_right: self.padding_right,
        }
    }
}

impl IdeInlayHintKind {
    fn into_lsp(self) -> u8 {
        match self {
            Self::Type => 1,
        }
    }
}

impl IdeSignatureHelp {
    pub(crate) fn into_lsp(self) -> SignatureHelp {
        SignatureHelp {
            signatures: self
                .signatures
                .into_iter()
                .map(IdeSignatureInformation::into_lsp)
                .collect(),
            active_signature: self.active_signature,
            active_parameter: self.active_parameter,
        }
    }
}

impl IdeSignatureInformation {
    fn into_lsp(self) -> SignatureInformation {
        SignatureInformation {
            label: self.label,
            parameters: self
                .parameters
                .into_iter()
                .map(IdeParameterInformation::into_lsp)
                .collect(),
        }
    }
}

impl IdeParameterInformation {
    fn into_lsp(self) -> ParameterInformation {
        ParameterInformation { label: self.label }
    }
}

impl IdePrepareRenameResult {
    pub(crate) fn into_lsp(self) -> PrepareRenameResult {
        PrepareRenameResult {
            range: self.range.into(),
            placeholder: self.placeholder,
        }
    }
}

impl IdeSemanticTokens {
    pub(crate) fn into_lsp(self) -> SemanticTokens {
        SemanticTokens {
            result_id: None,
            data: self.data,
        }
    }
}

impl IdeCompletionItem {
    pub(crate) fn into_lsp(self) -> CompletionItem {
        let insert_text_format = self.insert_text.as_ref().map(|text| {
            if completion_insert_uses_snippet(text) {
                2
            } else {
                1
            }
        });
        CompletionItem {
            label: self.label,
            kind: Some(self.kind.into_lsp()),
            detail: self.detail,
            insert_text: self.insert_text,
            insert_text_format,
            documentation: None,
            data: self
                .resolve_data
                .map(|data| serde_json::to_value(data).expect("completion resolve data encodes")),
        }
    }
}

impl IdeCompletionKind {
    fn into_lsp(self) -> u8 {
        match self {
            Self::Variable => 6,
            Self::Function => 3,
            Self::Module => 9,
            Self::Struct => 22,
            Self::Union => 22,
            Self::Enum => 13,
            Self::Trait => 8,
            Self::TypeAlias => 25,
            Self::Constant => 21,
            Self::Static => 6,
            Self::TypeParameter => 25,
            Self::Keyword => 14,
        }
    }
}

impl IdeHover {
    pub(crate) fn into_lsp(self) -> crate::protocol::Hover {
        crate::protocol::Hover {
            contents: crate::protocol::MarkupContent {
                kind: "markdown".to_string(),
                value: self.contents,
            },
            range: self.range.map(Into::into),
        }
    }
}

fn completion_insert_uses_snippet(text: &str) -> bool {
    text.contains('$')
}

impl IdeDiagnostic {
    pub(crate) fn into_lsp(self) -> Diagnostic {
        Diagnostic {
            range: self.range.into(),
            severity: self.severity.into_lsp(),
            source: self.source.to_string(),
            message: self.message,
            code: self.code,
            tags: (!self.tags.is_empty()).then(|| {
                self.tags
                    .into_iter()
                    .map(IdeDiagnosticTag::into_lsp)
                    .collect()
            }),
            related_information: (!self.related_information.is_empty()).then(|| {
                self.related_information
                    .into_iter()
                    .map(IdeDiagnosticRelatedInformation::into_lsp)
                    .collect()
            }),
        }
    }

    pub(super) fn related_information_mut(
        &mut self,
    ) -> Option<&mut Vec<IdeDiagnosticRelatedInformation>> {
        (!self.related_information.is_empty()).then_some(&mut self.related_information)
    }
}

impl IdeDiagnosticSeverity {
    pub(super) fn into_lsp(self) -> u8 {
        match self {
            Self::Error => 1,
            Self::Warning => 2,
            Self::Information => 3,
        }
    }
}

impl IdeDiagnosticTag {
    pub(super) fn into_lsp(self) -> DiagnosticTag {
        match self {
            Self::Unnecessary => DiagnosticTag::Unnecessary,
            Self::Deprecated => DiagnosticTag::Deprecated,
        }
    }
}

impl IdeDiagnosticRelatedInformation {
    fn into_lsp(self) -> DiagnosticRelatedInformation {
        DiagnosticRelatedInformation {
            location: self.location.into_lsp(),
            message: self.message,
        }
    }
}
