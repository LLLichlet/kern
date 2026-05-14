use super::OpenDocument;
use crate::protocol::{Diagnostic, DiagnosticRelatedInformation, DiagnosticTag, Location, Range};
use kernc_driver::{AnalysisArtifact, TargetedAnalysisReport};
use kernc_utils::{Session, SourceFile, Span};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const MAX_DIAGNOSTICS_PER_URI: usize = 200;

#[derive(Debug, Clone, Copy)]
struct OffsetReplacement {
    clean_start: usize,
    clean_end: usize,
    dirty_start: usize,
    dirty_end: usize,
}

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
        let fallback_file = open_documents
            .get(&uri)
            .map(|document| SourceFile::new(document.path.clone(), document.text.clone()));

        push_bounded_diagnostic(
            bundles.entry(uri).or_default(),
            convert_diagnostic_with_fallback(
                session,
                diag,
                fallback_file.as_ref(),
                Some(&uri_by_path),
            ),
        );
    }

    bundles
}

fn push_bounded_diagnostic(bundle: &mut Vec<Diagnostic>, diagnostic: Diagnostic) {
    if bundle.len() < MAX_DIAGNOSTICS_PER_URI {
        bundle.push(diagnostic);
        return;
    }

    if bundle.len() == MAX_DIAGNOSTICS_PER_URI {
        bundle.push(Diagnostic {
            range: crate::analysis::text::empty_range(),
            severity: 2,
            source: "kern-lsp",
            message: format!(
                "diagnostic output truncated after {MAX_DIAGNOSTICS_PER_URI} entries to keep the editor responsive"
            ),
            code: None,
            tags: None,
            related_information: None,
        });
    }
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
    convert_diagnostic_with_fallback(session, diagnostic, None, None)
}

pub(super) fn convert_diagnostic_for_document(
    session: &kernc_utils::Session,
    diagnostic: &kernc_utils::Diagnostic,
    document: &OpenDocument,
) -> Diagnostic {
    let fallback = SourceFile::new(document.path.clone(), document.text.clone());
    let uri_by_path = BTreeMap::from([(super::normalize_path(&document.path), String::new())]);
    let mut converted =
        convert_diagnostic_with_fallback(session, diagnostic, Some(&fallback), Some(&uri_by_path));
    if let Some(related_information) = converted.related_information.as_mut() {
        related_information.retain(|related| !related.location.uri.is_empty());
    }
    converted
}

fn convert_diagnostic_with_fallback(
    session: &kernc_utils::Session,
    diagnostic: &kernc_utils::Diagnostic,
    fallback_file: Option<&SourceFile>,
    uri_by_path: Option<&BTreeMap<PathBuf, String>>,
) -> Diagnostic {
    Diagnostic {
        range: diagnostic_range(session, diagnostic.primary_span, fallback_file),
        severity: diagnostic_severity(diagnostic.level),
        source: "kernc",
        message: diagnostic_message(diagnostic),
        code: diagnostic.code.map(|code| code.as_str().to_string()),
        tags: diagnostic_tags(diagnostic),
        related_information: diagnostic_related_information(session, diagnostic, uri_by_path),
    }
}

fn diagnostic_range(
    session: &kernc_utils::Session,
    span: kernc_utils::Span,
    fallback_file: Option<&SourceFile>,
) -> crate::protocol::Range {
    if let Some(file) = session.source_manager.get_file(span.file) {
        return crate::protocol::Range {
            start: super::byte_offset_to_position(file, span.start),
            end: super::byte_offset_to_position(file, span.end),
        };
    }

    let Some(file) = fallback_file else {
        return super::text::empty_range();
    };
    crate::protocol::Range {
        start: super::byte_offset_to_position(file, span.start),
        end: super::byte_offset_to_position(file, span.end),
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

fn diagnostic_tags(diagnostic: &kernc_utils::Diagnostic) -> Option<Vec<DiagnosticTag>> {
    let mut tags = diagnostic
        .tags
        .iter()
        .map(|tag| match tag {
            kernc_utils::DiagnosticTag::Unnecessary => DiagnosticTag::Unnecessary,
            kernc_utils::DiagnosticTag::Deprecated => DiagnosticTag::Deprecated,
        })
        .collect::<Vec<_>>();
    tags.sort_by_key(|tag| match tag {
        DiagnosticTag::Unnecessary => 1u8,
        DiagnosticTag::Deprecated => 2u8,
    });
    tags.dedup();
    (!tags.is_empty()).then_some(tags)
}

fn diagnostic_related_information(
    session: &kernc_utils::Session,
    diagnostic: &kernc_utils::Diagnostic,
    uri_by_path: Option<&BTreeMap<PathBuf, String>>,
) -> Option<Vec<DiagnosticRelatedInformation>> {
    let related_information = diagnostic
        .related_spans
        .iter()
        .filter_map(|(span, message)| {
            let path = session.source_manager.get_file_path(span.file)?;
            let normalized = super::normalize_path(path);
            let uri = uri_by_path
                .and_then(|map| map.get(&normalized))
                .cloned()
                .or_else(|| super::file_path_to_uri(path).ok())?;
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

pub(super) fn preserve_target_diagnostics(
    clean_artifact: &AnalysisArtifact,
    clean_file: &SourceFile,
    dirty_file: &SourceFile,
    target_uri: &str,
    report: &TargetedAnalysisReport,
) -> Vec<Diagnostic> {
    let target_path = super::normalize_path(&dirty_file.path);
    let mut replacements = report
        .replaced_spans
        .iter()
        .map(|replacement| OffsetReplacement {
            clean_start: replacement.clean.start,
            clean_end: replacement.clean.end,
            dirty_start: replacement.dirty.start,
            dirty_end: replacement.dirty.end,
        })
        .collect::<Vec<_>>();
    replacements.sort_by_key(|replacement| replacement.clean_start);

    clean_artifact
        .session
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.level == kernc_utils::DiagnosticLevel::Error)
        .filter(|diagnostic| {
            super::span_in_path(
                &clean_artifact.session,
                diagnostic.primary_span,
                &target_path,
            )
        })
        .filter_map(|diagnostic| {
            remap_clean_diagnostic(
                &clean_artifact.session,
                diagnostic,
                clean_file,
                dirty_file,
                target_uri,
                &target_path,
                &replacements,
            )
        })
        .collect()
}

fn remap_clean_diagnostic(
    session: &Session,
    diagnostic: &kernc_utils::Diagnostic,
    clean_file: &SourceFile,
    dirty_file: &SourceFile,
    target_uri: &str,
    target_path: &Path,
    replacements: &[OffsetReplacement],
) -> Option<Diagnostic> {
    let mut converted = convert_diagnostic(session, diagnostic);
    converted.range = remap_span_to_range(
        clean_file,
        dirty_file,
        diagnostic.primary_span,
        replacements,
    )?;

    if let Some(related_information) = converted.related_information.as_mut() {
        for (related, (span, _)) in related_information
            .iter_mut()
            .zip(&diagnostic.related_spans)
        {
            if !super::span_in_path(session, *span, target_path) {
                continue;
            }
            related.location.uri = target_uri.to_string();
            related.location.range =
                remap_span_to_range(clean_file, dirty_file, *span, replacements)?;
        }
    }

    Some(converted)
}

fn remap_span_to_range(
    clean_file: &SourceFile,
    dirty_file: &SourceFile,
    span: Span,
    replacements: &[OffsetReplacement],
) -> Option<Range> {
    if span.end > clean_file.src.len() {
        return None;
    }

    let start = remap_offset(span.start, replacements)?;
    let end = remap_offset(span.end, replacements)?;
    Some(Range {
        start: super::byte_offset_to_position(dirty_file, start),
        end: super::byte_offset_to_position(dirty_file, end),
    })
}

fn remap_offset(offset: usize, replacements: &[OffsetReplacement]) -> Option<usize> {
    let mut delta = 0_i64;

    for replacement in replacements {
        if offset < replacement.clean_start {
            break;
        }
        if offset > replacement.clean_end {
            delta += replacement.dirty_end as i64 - replacement.dirty_start as i64;
            delta -= replacement.clean_end as i64 - replacement.clean_start as i64;
            continue;
        }
        if offset == replacement.clean_start {
            return Some(replacement.dirty_start);
        }
        if offset == replacement.clean_end {
            return Some(replacement.dirty_end);
        }
        return None;
    }

    offset.checked_add_signed(delta as isize)
}

pub fn cleared_uris(
    previous: &BTreeSet<String>,
    current: &[super::DiagnosticBundle],
) -> Vec<String> {
    let current_uris: BTreeSet<_> = current.iter().map(|bundle| bundle.uri.clone()).collect();
    previous
        .iter()
        .filter(|uri| !current_uris.contains(*uri))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::convert_diagnostic;
    use crate::protocol::DiagnosticTag;

    #[test]
    fn convert_diagnostic_includes_hints_and_related_information() {
        let mut session = kernc_utils::Session::new();
        let file_id = session.source_manager.add_file(
            "diag_test.kn".to_string(),
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
        .with_code(kernc_utils::DiagnosticCode::ExpectedSemicolon)
        .with_hint("first hint")
        .with_tag(kernc_utils::DiagnosticTag::Unnecessary);
        let mut diagnostic = diagnostic;
        diagnostic
            .related_spans
            .push((related_span, "related here".to_string()));

        let converted = convert_diagnostic(&session, &diagnostic);

        assert!(converted.message.contains("sample error"));
        assert!(converted.message.contains("Hint: first hint"));
        assert_eq!(converted.code.as_deref(), Some("expected-semicolon"));
        assert_eq!(converted.tags, Some(vec![DiagnosticTag::Unnecessary]));
        let related = converted.related_information.unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].message, "related here");
        assert_eq!(related[0].location.range.start.line, 0);
    }
}
