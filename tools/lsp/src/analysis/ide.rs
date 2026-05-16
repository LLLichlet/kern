use crate::protocol::{
    CodeAction, CompletionItem, Diagnostic, DiagnosticRelatedInformation, DiagnosticTag,
    DocumentHighlight, DocumentSymbol, InlayHint, Location, ParameterInformation,
    PrepareRenameResult, Range, SemanticTokens, SignatureHelp, SignatureInformation, TextEdit,
    WorkspaceEdit,
};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub(crate) struct IdeCodeAction {
    pub title: String,
    pub kind: Option<&'static str>,
    pub diagnostics: Vec<IdeDiagnostic>,
    pub edit: Option<IdeWorkspaceEdit>,
    pub is_preferred: Option<bool>,
}

#[derive(Debug, Clone)]
pub(crate) struct IdeWorkspaceEdit {
    pub changes: BTreeMap<String, Vec<IdeTextEdit>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdeTextEdit {
    pub range: Range,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdeLocation {
    pub uri: String,
    pub range: Range,
}

#[derive(Debug, Clone)]
pub(crate) struct IdeDocumentSymbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: IdeSymbolKind,
    pub range: Range,
    pub selection_range: Range,
    pub children: Vec<IdeDocumentSymbol>,
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
    pub range: Range,
    pub kind: Option<IdeDocumentHighlightKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdeDocumentHighlightKind {
    Text,
}

#[derive(Debug, Clone)]
pub(crate) struct IdeInlayHint {
    pub position: crate::protocol::Position,
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
    pub range: Range,
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
}

#[derive(Debug, Clone)]
pub(crate) struct IdeHover {
    pub contents: String,
    pub range: Option<Range>,
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
    pub range: Range,
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
    pub location: Location,
    pub message: String,
}

impl IdeCodeAction {
    pub(crate) fn into_lsp(self) -> CodeAction {
        CodeAction {
            title: self.title,
            kind: self.kind,
            diagnostics: (!self.diagnostics.is_empty()).then(|| {
                self.diagnostics
                    .into_iter()
                    .map(IdeDiagnostic::into_lsp)
                    .collect()
            }),
            edit: self.edit.map(IdeWorkspaceEdit::into_lsp),
            is_preferred: self.is_preferred,
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
    fn into_lsp(self) -> TextEdit {
        TextEdit {
            range: self.range,
            new_text: self.new_text,
        }
    }
}

impl IdeLocation {
    pub(crate) fn into_lsp(self) -> Location {
        Location {
            uri: self.uri,
            range: self.range,
        }
    }
}

impl IdeDocumentSymbol {
    pub(crate) fn into_lsp(self) -> DocumentSymbol {
        DocumentSymbol {
            name: self.name,
            detail: self.detail,
            kind: self.kind.into_lsp(),
            range: self.range,
            selection_range: self.selection_range,
            children: self
                .children
                .into_iter()
                .map(IdeDocumentSymbol::into_lsp)
                .collect(),
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
            range: self.range,
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

impl IdeInlayHint {
    pub(crate) fn into_lsp(self) -> InlayHint {
        InlayHint {
            position: self.position,
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
            range: self.range,
            placeholder: self.placeholder,
        }
    }
}

impl IdeSemanticTokens {
    pub(crate) fn into_lsp(self) -> SemanticTokens {
        SemanticTokens { data: self.data }
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
            kind: self.kind.into_lsp(),
            detail: self.detail,
            insert_text: self.insert_text,
            insert_text_format,
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
                kind: "markdown",
                value: self.contents,
            },
            range: self.range,
        }
    }
}

fn completion_insert_uses_snippet(text: &str) -> bool {
    text.contains('$')
}

impl IdeDiagnostic {
    pub(crate) fn into_lsp(self) -> Diagnostic {
        Diagnostic {
            range: self.range,
            severity: self.severity.into_lsp(),
            source: self.source,
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
            location: self.location,
            message: self.message,
        }
    }
}
