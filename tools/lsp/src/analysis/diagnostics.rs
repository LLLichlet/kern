use super::OpenDocument;
use crate::protocol::{Diagnostic, DiagnosticRelatedInformation, Location};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub(super) fn diagnostics_from_session(
    session: &kernc_utils::Session,
    open_documents: &BTreeMap<String, OpenDocument>,
) -> BTreeMap<String, Vec<Diagnostic>> {
    let uri_by_path: BTreeMap<_, _> = open_documents
        .iter()
        .map(|(uri, doc)| (super::normalize_path(&doc.path), uri.clone()))
        .collect();

    let mut bundles = BTreeMap::<String, Vec<Diagnostic>>::new();

    for diag in &session.diagnostics {
        let uri = diagnostic_uri(session, diag.primary_span.file, &uri_by_path)
            .unwrap_or_else(|| "kern-lsp:/unknown".to_string());

        bundles
            .entry(uri)
            .or_default()
            .push(convert_diagnostic(session, diag));
    }

    bundles
}

fn diagnostic_uri(
    session: &kernc_utils::Session,
    file_id: kernc_utils::FileId,
    uri_by_path: &BTreeMap<PathBuf, String>,
) -> Option<String> {
    let path = session.source_manager.get_file_path(file_id)?;
    let normalized = super::normalize_path(path);
    if let Some(uri) = uri_by_path.get(&normalized) {
        return Some(uri.clone());
    }

    super::file_path_to_uri(path).ok()
}

pub(super) fn convert_diagnostic(
    session: &kernc_utils::Session,
    diagnostic: &kernc_utils::Diagnostic,
) -> Diagnostic {
    Diagnostic {
        range: super::span_to_range(session, diagnostic.primary_span),
        severity: diagnostic_severity(diagnostic.level),
        source: "kernc",
        message: diagnostic_message(diagnostic),
        related_information: diagnostic_related_information(session, diagnostic),
    }
}

fn diagnostic_severity(level: kernc_utils::DiagnosticLevel) -> u8 {
    match level {
        kernc_utils::DiagnosticLevel::Error | kernc_utils::DiagnosticLevel::Ice => 1,
        kernc_utils::DiagnosticLevel::Warning => 2,
        kernc_utils::DiagnosticLevel::Note => 3,
    }
}

fn diagnostic_message(diagnostic: &kernc_utils::Diagnostic) -> String {
    let mut message = diagnostic.message.clone();
    for hint in &diagnostic.hints {
        message.push_str("\n\nHint: ");
        message.push_str(hint);
    }
    message
}

fn diagnostic_related_information(
    session: &kernc_utils::Session,
    diagnostic: &kernc_utils::Diagnostic,
) -> Option<Vec<DiagnosticRelatedInformation>> {
    let related_information = diagnostic
        .related_spans
        .iter()
        .filter_map(|(span, message)| {
            let path = session.source_manager.get_file_path(span.file)?;
            let uri = super::file_path_to_uri(path).ok()?;
            Some(DiagnosticRelatedInformation {
                location: Location {
                    uri,
                    range: super::span_to_range(session, *span),
                },
                message: message.clone(),
            })
        })
        .collect::<Vec<_>>();

    (!related_information.is_empty()).then_some(related_information)
}

#[cfg(test)]
mod tests {
    use super::convert_diagnostic;

    #[test]
    fn convert_diagnostic_includes_hints_and_related_information() {
        let mut session = kernc_utils::Session::new();
        let file_id = session.source_manager.add_file(
            "diag_test.rn".to_string(),
            "fn main() void {}\n".to_string(),
        );
        let primary_span = kernc_utils::Span {
            file: file_id,
            start: 3,
            end: 7,
        };
        let related_span = kernc_utils::Span {
            file: file_id,
            start: 0,
            end: 2,
        };
        let diagnostic = kernc_utils::Diagnostic::new(
            kernc_utils::DiagnosticLevel::Error,
            primary_span,
            "sample error",
        )
        .with_hint("first hint");
        let mut diagnostic = diagnostic;
        diagnostic
            .related_spans
            .push((related_span, "related here".to_string()));

        let converted = convert_diagnostic(&session, &diagnostic);

        assert!(converted.message.contains("sample error"));
        assert!(converted.message.contains("Hint: first hint"));
        let related = converted.related_information.unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].message, "related here");
        assert_eq!(related[0].location.range.start.line, 0);
    }
}
