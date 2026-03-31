use super::OpenDocument;
use crate::protocol::Diagnostic;
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
        message: diagnostic.message.clone(),
    }
}

fn diagnostic_severity(level: kernc_utils::DiagnosticLevel) -> u8 {
    match level {
        kernc_utils::DiagnosticLevel::Error | kernc_utils::DiagnosticLevel::Ice => 1,
        kernc_utils::DiagnosticLevel::Warning => 2,
        kernc_utils::DiagnosticLevel::Note => 3,
    }
}
