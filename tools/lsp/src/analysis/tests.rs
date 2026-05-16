mod code_actions;
mod completion;
mod diagnostics;
mod dirty_cache;
mod document;
mod inlay_hints;
mod navigation;
mod project_resolution;
mod real_projects;
mod robustness;
mod semantic_tokens;

use super::cache::AnalysisCacheKey;
use super::semantic::{SemanticModifiers, SemanticTokenTypes};
use super::{
    AnalysisEngine, AnalysisSettings, AnalysisTier, CancellationToken, DiagnosticBundle,
    byte_offset_to_position, cleared_uris, file_path_to_uri, hash_source_text, normalize_path,
    position_to_byte_offset, uri_to_analysis_path, uri_to_file_path,
};
use crate::analysis::DocumentSyncAction;
use crate::analysis::ide::{
    IdeCompletionItem, IdeDiagnosticSeverity, IdeDiagnosticTag, IdeDocumentHighlightKind,
    IdeSemanticTokens,
};
use crate::protocol::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams, Position,
    Range, TextDocumentContentChangeEvent, TextDocumentItem, VersionedTextDocumentIdentifier,
};
use crate::server::DiagnosticsAnalysisMode;
use craft::analysis_context;
use kernc_utils::SourceFile;
use kernc_utils::config::{CompileOptions, LibraryBundle, RuntimeEntry};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);
const CURRENT_KERN_VERSION: &str = env!("CARGO_PKG_VERSION");

fn temp_file_uri(prefix: &str, initial_text: &str) -> String {
    let path = unique_temp_file_path(prefix);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, initial_text).unwrap();
    file_path_to_uri(&path).unwrap()
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let path = unique_temp_file_path(prefix);
    fs::create_dir_all(&path).unwrap();
    path
}

fn unique_temp_file_path(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let counter = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "{}_{}_{}_{}.kn",
        prefix,
        std::process::id(),
        nanos,
        counter
    ))
}

fn untitled_uri(name: &str) -> String {
    format!("untitled:{name}")
}

fn workspace_root() -> PathBuf {
    normalize_path(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap(),
    )
}

fn open_workspace_document(analysis: &mut AnalysisEngine, path: &PathBuf) -> (String, String) {
    let uri = file_path_to_uri(path).unwrap();
    let source = fs::read_to_string(path).unwrap();

    let _ = analysis.open_document_state(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.clone(),
        },
    });

    (uri, source)
}

fn open_document_for_full_diagnostics(
    analysis: &mut AnalysisEngine,
    uri: &str,
    source: &str,
) -> super::AnalysisOutcome {
    let _ = analysis.open_document_state(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.to_string(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });
    analysis.analyze_document_uri(uri)
}

fn change_document_for_full_diagnostics(
    analysis: &mut AnalysisEngine,
    uri: &str,
    version: i64,
    source: &str,
) -> super::AnalysisOutcome {
    let _ = analysis.change_document_state(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.to_string(),
            version,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: source.to_string(),
        }],
    });
    analysis.analyze_document_uri(uri)
}

fn warm_clean_semantic_artifact(analysis: &AnalysisEngine, uri: &str, _source: &str) {
    let snapshot = analysis.snapshot();
    let _ = analysis
        .analyze_interactive_artifact_for_snapshot(&snapshot, uri)
        .unwrap();
}

#[test]
fn canceled_snapshot_stops_interactive_analysis_before_semantic_work() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() void {}\n";
    let uri = temp_file_uri("analysis_canceled_snapshot", source);
    let _ = analysis.open_document_state(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });
    let token = CancellationToken::new();
    token.cancel();
    let snapshot = analysis.snapshot_with_cancellation(Some(token));

    let err = analysis
        .document_symbols_in_snapshot(&snapshot, &uri)
        .unwrap_err();

    assert_eq!(err, "request was canceled");
    assert_eq!(analysis.last_analysis_tier(), None);
}

fn position_of_nth(source: &str, needle: &str, occurrence: usize, char_offset: u32) -> Position {
    let byte_offset = nth_match_offset(source, needle, occurrence) + char_offset as usize;
    let prefix = &source[..byte_offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let line_start = prefix.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let character = source[line_start..byte_offset].encode_utf16().count() as u32;

    Position { line, character }
}

fn nth_match_offset(source: &str, needle: &str, occurrence: usize) -> usize {
    source
        .match_indices(needle)
        .nth(occurrence)
        .map(|(offset, _)| offset)
        .unwrap()
}

fn completion_labels(items: &[IdeCompletionItem]) -> Vec<String> {
    items.iter().map(|item| item.label.clone()).collect()
}

fn decode_semantic_tokens(tokens: &IdeSemanticTokens) -> Vec<(Position, u32, u32, u32)> {
    let mut decoded = Vec::new();
    let mut line = 0;
    let mut start = 0;

    for chunk in tokens.data.chunks_exact(5) {
        line += chunk[0];
        if chunk[0] == 0 {
            start += chunk[1];
        } else {
            start = chunk[1];
        }

        decoded.push((
            Position {
                line,
                character: start,
            },
            chunk[2],
            chunk[3],
            chunk[4],
        ));
    }

    decoded
}

fn assert_token_type(tokens: &[(Position, u32, u32, u32)], position: Position, expected_type: u32) {
    assert!(
        tokens.iter().any(
            |(token_position, _, token_type, _)| *token_position == position
                && *token_type == expected_type
        ),
        "missing semantic token {:?} at {:?}",
        expected_type,
        position
    );
}

fn assert_token(
    tokens: &[(Position, u32, u32, u32)],
    position: Position,
    expected_type: u32,
    expected_modifiers: u32,
) {
    assert!(
        tokens.iter().any(
            |(token_position, _, token_type, modifiers)| *token_position == position
                && *token_type == expected_type
                && *modifiers == expected_modifiers
        ),
        "missing semantic token {:?} with modifiers {:?} at {:?}",
        expected_type,
        expected_modifiers,
        position
    );
}

fn assert_token_with_length(
    tokens: &[(Position, u32, u32, u32)],
    position: Position,
    expected_length: u32,
    expected_type: u32,
) {
    assert!(
        tokens.iter().any(
            |(token_position, length, token_type, _)| *token_position == position
                && *length == expected_length
                && *token_type == expected_type
        ),
        "missing semantic token {:?} with length {:?} at {:?}",
        expected_type,
        expected_length,
        position
    );
}
