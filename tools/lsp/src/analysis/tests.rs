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
mod structure;

use super::cache::AnalysisCacheKey;
use super::semantic::{SemanticModifiers, SemanticTokenTypes};
use super::{
    AnalysisEngine, AnalysisSettings, AnalysisTier, CancellationToken, DiagnosticBundle,
    IdeChangeDocument, IdeCloseDocument, IdeOpenDocument, IdePosition, IdeRange,
    IdeTextDocumentChange, IntoIdeChangeDocument, IntoIdeCloseDocument, IntoIdeOpenDocument,
    IntoIdePosition, IntoIdeRange, byte_offset_to_position, cleared_uris, file_path_to_uri,
    hash_source_text, normalize_path, position_to_byte_offset, uri_to_analysis_path,
    uri_to_file_path,
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

impl IntoIdeOpenDocument for DidOpenTextDocumentParams {
    fn into_ide_open_document(self) -> IdeOpenDocument {
        IdeOpenDocument {
            uri: self.text_document.uri,
            version: self.text_document.version,
            text: self.text_document.text,
        }
    }
}

impl IntoIdeChangeDocument for DidChangeTextDocumentParams {
    fn into_ide_change_document(self) -> IdeChangeDocument {
        IdeChangeDocument {
            uri: self.text_document.uri,
            version: self.text_document.version,
            changes: self
                .content_changes
                .into_iter()
                .map(|change| IdeTextDocumentChange {
                    range: change.range.map(|range| IdeRange {
                        start: IdePosition {
                            line: range.start.line,
                            character: range.start.character,
                        },
                        end: IdePosition {
                            line: range.end.line,
                            character: range.end.character,
                        },
                    }),
                    text: change.text,
                })
                .collect(),
        }
    }
}

impl IntoIdeCloseDocument for DidCloseTextDocumentParams {
    fn into_ide_close_document(self) -> IdeCloseDocument {
        IdeCloseDocument {
            uri: self.text_document.uri,
        }
    }
}

impl IntoIdePosition for Position {
    fn into_ide_position(self) -> IdePosition {
        IdePosition {
            line: self.line,
            character: self.character,
        }
    }
}

impl IntoIdeRange for Range {
    fn into_ide_range(self) -> IdeRange {
        IdeRange {
            start: self.start.into_ide_position(),
            end: self.end.into_ide_position(),
        }
    }
}

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
    let snapshot = analysis.snapshot(Vec::new(), CancellationToken::new());
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
    let snapshot = analysis.snapshot(Vec::new(), token);

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

#[test]
fn non_test_analysis_modules_do_not_import_protocol_sync_payloads() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut paths = vec![manifest_dir.join("src/analysis.rs")];
    for entry in fs::read_dir(manifest_dir.join("src/analysis")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("rs")
            && path.file_name().and_then(|name| name.to_str()) != Some("tests.rs")
        {
            paths.push(path);
        }
    }
    let forbidden_payloads = [
        "DidChangeTextDocumentParams",
        "DidCloseTextDocumentParams",
        "DidOpenTextDocumentParams",
        "DidSaveTextDocumentParams",
        "TextDocumentContentChangeEvent",
        "TextDocumentIdentifier",
        "TextDocumentItem",
        "VersionedTextDocumentIdentifier",
    ];
    let mut violations = Vec::new();

    for path in paths {
        let source = fs::read_to_string(&path).unwrap();
        let file_name = path.file_name().and_then(|name| name.to_str()).unwrap();
        for payload in forbidden_payloads {
            if source.contains(payload) {
                violations.push(format!("{} mentions {payload}", path.display()));
            }
        }
        for forbidden in [
            "pub location: Location",
            "pub position: crate::protocol::Position",
        ] {
            if source.contains(forbidden) {
                violations.push(format!(
                    "{} exposes protocol field `{forbidden}`",
                    path.display()
                ));
            }
        }
        for forbidden in [
            "pub range: Range",
            "pub selection_range: Range",
            "pub range: crate::protocol::Range",
            "pub selection_range: crate::protocol::Range",
            "pub from_ranges: Vec<Range>",
            "pub from_ranges: Vec<crate::protocol::Range>",
            "pub range: Option<Range>",
            "pub range: Option<crate::protocol::Range>",
            "pub position: Position",
            "pub position: crate::protocol::Position",
        ] {
            if source.contains(forbidden) {
                violations.push(format!(
                    "{} exposes protocol IDE field `{forbidden}`",
                    path.display()
                ));
            }
        }
        for line in source.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("pub fn ")
                && (trimmed.contains("position: Position")
                    || trimmed.contains("range: Range")
                    || trimmed.contains("requested_range: Range")
                    || trimmed.contains("positions: Vec<Position>")
                    || trimmed.contains("item_range: &Range"))
            {
                violations.push(format!(
                    "{} exposes protocol coordinate input `{trimmed}`",
                    path.display()
                ));
            }
        }
        for line in source.lines() {
            let Some(imports) = line.trim().strip_prefix("use crate::protocol::{") else {
                continue;
            };
            let Some(imports) = imports.strip_suffix("};") else {
                continue;
            };
            for imported in imports
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                if !protocol_import_allowed(file_name, imported) {
                    violations.push(format!(
                        "{} imports unexpected protocol type `{imported}`",
                        path.display()
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "analysis protocol boundary violations:\n{}",
        violations.join("\n")
    );
}

fn protocol_import_allowed(file_name: &str, imported: &str) -> bool {
    match file_name {
        "analysis.rs" => matches!(
            imported,
            "CodeActionResolveData" | "CompletionResolveData" | "Position" | "Range"
        ),
        "ide.rs" => true,
        "diagnostics.rs" => imported == "DiagnosticTag",
        _ => false,
    }
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

fn assert_no_token_at(tokens: &[(Position, u32, u32, u32)], position: Position) {
    assert!(
        tokens
            .iter()
            .all(|(token_position, _, _, _)| *token_position != position),
        "unexpected semantic token at {:?}",
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
