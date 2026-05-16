use super::*;
use std::sync::Arc;

#[test]
fn full_sync_replaces_document_text() {
    let mut analysis = AnalysisEngine::default();
    let uri = temp_file_uri("full_sync", "let x = 1;");

    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: "let x = 1;".to_string(),
        },
    });

    assert!(!outcome.bundles.is_empty());

    let outcome = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: "let x = 2;".to_string(),
        }],
    });

    assert!(outcome.bundles.iter().any(|bundle| bundle.uri == uri));
    let doc = analysis.documents.get(&uri).unwrap();
    assert_eq!(doc.version, 2);
    assert_eq!(doc.text, "let x = 2;");
}

#[test]
fn close_document_clears_open_state_and_returns_empty_bundle() {
    let mut analysis = AnalysisEngine::default();
    let uri = temp_file_uri("close_document", "let x = 1;");

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: "let x = 1;".to_string(),
        },
    });

    let outcome = analysis.close_document(DidCloseTextDocumentParams {
        text_document: crate::protocol::TextDocumentIdentifier { uri: uri.clone() },
    });

    assert!(!analysis.documents.contains_key(&uri));
    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    assert!(bundle.diagnostics.is_empty());
}

#[test]
fn snapshot_preserves_open_document_view_after_later_changes() {
    let mut analysis = AnalysisEngine::default();
    let uri = temp_file_uri("snapshot_open_document", "let value = 1;");

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: "let value = 1;".to_string(),
        },
    });

    let snapshot = analysis.snapshot(None, CancellationToken::new());

    let _ = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: "let value = 2;".to_string(),
        }],
    });

    assert_eq!(snapshot.document(&uri).unwrap().version, 1);
    assert_eq!(snapshot.document(&uri).unwrap().text, "let value = 1;");
    assert_eq!(analysis.documents.get(&uri).unwrap().version, 2);
    assert_eq!(analysis.documents.get(&uri).unwrap().text, "let value = 2;");
}

#[test]
fn analysis_reuses_driver_for_repeated_requests_on_same_document() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() i32 { return 1; }\n";
    let uri = temp_file_uri("driver_reuse", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });
    assert_eq!(analysis.cached_driver_count(), 0);

    let _ = analysis.analyze_document_uri(&uri);
    assert_eq!(analysis.cached_driver_count(), 1);

    let _ = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: "fn main() i32 { return 2; }\n".to_string(),
        }],
    });
    assert_eq!(analysis.cached_driver_count(), 1);
}

#[test]
fn open_path_index_reuses_on_text_changes_and_invalidates_on_open_close() {
    let mut analysis = AnalysisEngine::default();
    let uri = temp_file_uri("open_path_index", "fn main() void {}\n");
    let path = uri_to_file_path(&uri).unwrap();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: "fn main() void {}\n".to_string(),
        },
    });

    let initial_index = analysis.open_uri_by_normalized_path();
    assert_eq!(initial_index.get(&normalize_path(&path)), Some(&uri));
    assert!(analysis.analysis_path_exists(&path));

    let _ = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: "fn main() void {}\nfn helper() void {}\n".to_string(),
        }],
    });

    let changed_index = analysis.open_uri_by_normalized_path();
    assert!(Arc::ptr_eq(&initial_index, &changed_index));

    let sibling_uri = temp_file_uri("open_path_index_sibling", "fn sibling() void {}\n");
    let sibling_path = uri_to_file_path(&sibling_uri).unwrap();
    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: sibling_uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: "fn sibling() void {}\n".to_string(),
        },
    });

    let sibling_index = analysis.open_uri_by_normalized_path();
    assert!(!Arc::ptr_eq(&changed_index, &sibling_index));
    assert_eq!(
        sibling_index.get(&normalize_path(&sibling_path)),
        Some(&sibling_uri)
    );

    let _ = analysis.close_document(DidCloseTextDocumentParams {
        text_document: crate::protocol::TextDocumentIdentifier { uri: uri.clone() },
    });

    let closed_index = analysis.open_uri_by_normalized_path();
    assert!(!Arc::ptr_eq(&sibling_index, &closed_index));
    assert!(!closed_index.contains_key(&normalize_path(&path)));
    assert_eq!(
        closed_index.get(&normalize_path(&sibling_path)),
        Some(&sibling_uri)
    );
}

#[test]
fn incremental_sync_inserts_text() {
    let mut analysis = AnalysisEngine::default();
    let uri = temp_file_uri("incremental_insert", "let value = 1;");

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: "let value = 1;".to_string(),
        },
    });

    let outcome = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position {
                    line: 0,
                    character: 13,
                },
                end: Position {
                    line: 0,
                    character: 13,
                },
            }),
            text: " + 1".to_string(),
        }],
    });

    assert!(outcome.bundles.iter().any(|bundle| bundle.uri == uri));
    assert_eq!(
        analysis.documents.get(&uri).unwrap().text,
        "let value = 1 + 1;"
    );
}

#[test]
fn incremental_sync_replaces_text() {
    let mut analysis = AnalysisEngine::default();
    let uri = temp_file_uri("incremental_replace", "let value = 1;");

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: "let value = 1;".to_string(),
        },
    });

    let outcome = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position {
                    line: 0,
                    character: 12,
                },
                end: Position {
                    line: 0,
                    character: 13,
                },
            }),
            text: "42".to_string(),
        }],
    });

    assert!(outcome.bundles.iter().any(|bundle| bundle.uri == uri));
    assert_eq!(
        analysis.documents.get(&uri).unwrap().text,
        "let value = 42;"
    );
}

#[test]
fn incremental_sync_deletes_text() {
    let mut analysis = AnalysisEngine::default();
    let uri = temp_file_uri("incremental_delete", "let value = 123;");

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: "let value = 123;".to_string(),
        },
    });

    let outcome = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position {
                    line: 0,
                    character: 12,
                },
                end: Position {
                    line: 0,
                    character: 14,
                },
            }),
            text: String::new(),
        }],
    });

    assert!(outcome.bundles.iter().any(|bundle| bundle.uri == uri));
    assert_eq!(analysis.documents.get(&uri).unwrap().text, "let value = 3;");
}

#[test]
fn incremental_sync_respects_utf16_positions() {
    let mut analysis = AnalysisEngine::default();
    let uri = temp_file_uri("incremental_utf16", "let face = \"😀x\";");

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: "let face = \"😀x\";".to_string(),
        },
    });

    let outcome = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position {
                    line: 0,
                    character: 14,
                },
                end: Position {
                    line: 0,
                    character: 15,
                },
            }),
            text: "!".to_string(),
        }],
    });

    assert!(outcome.bundles.iter().any(|bundle| bundle.uri == uri));
    assert_eq!(
        analysis.documents.get(&uri).unwrap().text,
        "let face = \"😀!\";"
    );
}

#[test]
fn invalid_incremental_sync_range_keeps_previous_text() {
    let mut analysis = AnalysisEngine::default();
    let uri = temp_file_uri("incremental_invalid", "let value = 1;");

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: "let value = 1;".to_string(),
        },
    });

    let outcome = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position {
                    line: 1,
                    character: 0,
                },
                end: Position {
                    line: 1,
                    character: 1,
                },
            }),
            text: "x".to_string(),
        }],
    });

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    assert_eq!(analysis.documents.get(&uri).unwrap().text, "let value = 1;");
    assert!(
        bundle
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("invalid start position"))
    );
}

#[test]
fn overlay_text_is_used_for_compiler_diagnostics() {
    let mut analysis = AnalysisEngine::default();
    let uri = temp_file_uri("overlay_diag", "fn main() i32 { 0 }");

    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: "fn main( ".to_string(),
        },
    });

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    assert!(
        !bundle.diagnostics.is_empty(),
        "expected diagnostics from in-memory overlay"
    );
}

#[test]
fn file_uri_roundtrips() {
    let path = unique_temp_file_path("uri_roundtrip");
    let uri = file_path_to_uri(&path).unwrap();
    let parsed = uri_to_file_path(&uri).unwrap();
    assert_eq!(parsed, path);
}

#[test]
fn untitled_uri_maps_to_stable_virtual_path() {
    let uri = untitled_uri("Untitled-1");
    let first = uri_to_analysis_path(&uri).unwrap();
    let second = uri_to_analysis_path(&uri).unwrap();

    assert_eq!(first, second);
    assert!(first.to_string_lossy().contains("Untitled-1"));
}

#[test]
fn untitled_document_is_analyzed_without_file_uri_error() {
    let mut analysis = AnalysisEngine::default();
    let uri = untitled_uri("Untitled-2");

    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: "fn main() i32 { return 1; }\n".to_string(),
        },
    });

    assert!(outcome.bundles.iter().any(|bundle| bundle.uri == uri));
    assert!(
        outcome
            .bundles
            .iter()
            .flat_map(|bundle| &bundle.diagnostics)
            .all(|diagnostic| !diagnostic.message.contains("only file://"))
    );
    assert_eq!(
        analysis.documents.get(&uri).unwrap().path,
        uri_to_analysis_path(&uri).unwrap()
    );
}
