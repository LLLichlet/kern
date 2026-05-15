use crate::protocol::{
    CodeAction, Diagnostic, DiagnosticRelatedInformation, DiagnosticTag, Location, Range, TextEdit,
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

#[derive(Debug, Clone)]
pub(crate) struct IdeTextEdit {
    pub range: Range,
    pub new_text: String,
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
    pub(super) fn into_lsp(self) -> WorkspaceEdit {
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
