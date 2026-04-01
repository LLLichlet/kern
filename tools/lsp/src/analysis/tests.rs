use super::semantic::{SemanticModifiers, SemanticTokenTypes};
use super::{
    AnalysisEngine, byte_offset_to_position, cleared_uris, file_path_to_uri,
    position_to_byte_offset, uri_to_file_path,
};
use crate::protocol::{
    DidChangeTextDocumentParams, DidOpenTextDocumentParams, Position, Range, SemanticTokens,
    TextDocumentContentChangeEvent, TextDocumentItem, VersionedTextDocumentIdentifier,
};
use kernc_utils::SourceFile;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

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
fn semantic_tokens_classify_keywords_types_and_functions() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Point = struct { x: i32 };\n",
        "fn helper(point: Point) i32 {\n",
        "    return point.x;\n",
        "}\n",
    );
    let uri = temp_file_uri("semantic_tokens", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let decoded = decode_semantic_tokens(&analysis.semantic_tokens(&uri).unwrap());

    assert_token_type(
        &decoded,
        position_of_nth(source, "type", 0, 0),
        SemanticTokenTypes::KEYWORD,
    );
    assert_token_type(
        &decoded,
        position_of_nth(source, "Point", 0, 0),
        SemanticTokenTypes::STRUCT,
    );
    assert_token_type(
        &decoded,
        position_of_nth(source, "helper", 0, 0),
        SemanticTokenTypes::FUNCTION,
    );
    assert_token_type(
        &decoded,
        position_of_nth(source, "point", 0, 0),
        SemanticTokenTypes::PARAMETER,
    );
    assert_token_type(
        &decoded,
        position_of_nth(source, "Point", 1, 0),
        SemanticTokenTypes::TYPE,
    );
    assert_token_type(
        &decoded,
        position_of_nth(source, "x", 1, 0),
        SemanticTokenTypes::PROPERTY,
    );
    assert_token_type(
        &decoded,
        position_of_nth(source, "struct", 0, 0),
        SemanticTokenTypes::KEYWORD,
    );
    assert_token_type(
        &decoded,
        position_of_nth(source, "return", 0, 0),
        SemanticTokenTypes::KEYWORD,
    );
}

#[test]
fn semantic_tokens_prefer_symbol_kinds_and_modifiers_for_references() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "const LIMIT = i32.{5};\n",
        "static mut TOTAL = i32.{0};\n",
        "type Counter = struct { value: i32 };\n",
        "impl Counter {\n",
        "    fn get() i32 {\n",
        "        return self.value;\n",
        "    }\n",
        "}\n",
        "fn main() i32 {\n",
        "    let counter = Counter.{ value: LIMIT };\n",
        "    return counter.get() + LIMIT + TOTAL;\n",
        "}\n",
    );
    let uri = temp_file_uri("semantic_tokens_symbols", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let decoded = decode_semantic_tokens(&analysis.semantic_tokens(&uri).unwrap());

    assert_token(
        &decoded,
        position_of_nth(source, "get", 1, 0),
        SemanticTokenTypes::METHOD,
        0,
    );
    assert_token(
        &decoded,
        position_of_nth(source, "LIMIT", 1, 0),
        SemanticTokenTypes::VARIABLE,
        SemanticModifiers::READONLY,
    );
    assert_token(
        &decoded,
        position_of_nth(source, "TOTAL", 1, 0),
        SemanticTokenTypes::VARIABLE,
        SemanticModifiers::STATIC,
    );
    assert_token(
        &decoded,
        position_of_nth(source, "value", 1, 0),
        SemanticTokenTypes::PROPERTY,
        0,
    );
}

#[test]
fn code_actions_offer_missing_semicolon_fix() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() i32 {\n    let value = 1\n    return value;\n}\n";
    let uri = temp_file_uri("code_action_semicolon", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 2,
                    character: 0,
                },
                end: Position {
                    line: 2,
                    character: 20,
                },
            },
        )
        .unwrap();

    let action = actions
        .iter()
        .find(|action| action.title == "Insert `;`")
        .unwrap();
    let edit = action.edit.as_ref().unwrap();
    let text_edit = edit.changes.get(&uri).unwrap().first().unwrap();

    assert_eq!(
        text_edit.range.start,
        Position {
            line: 2,
            character: 4,
        }
    );
    assert_eq!(text_edit.new_text, ";");
    assert_eq!(action.kind, Some("quickfix"));
    assert_eq!(action.is_preferred, Some(true));
}

#[test]
fn code_actions_offer_missing_closing_delimiter_fix() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() i32 {\n    return (1 + 2;\n}\n";
    let uri = temp_file_uri("code_action_paren", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 1,
                    character: 0,
                },
                end: Position {
                    line: 1,
                    character: 18,
                },
            },
        )
        .unwrap();

    let action = actions
        .iter()
        .find(|action| action.title == "Insert `)`")
        .unwrap();
    let edit = action.edit.as_ref().unwrap();
    let text_edit = edit.changes.get(&uri).unwrap().first().unwrap();

    assert_eq!(
        text_edit.range.start,
        Position {
            line: 1,
            character: 17,
        }
    );
    assert_eq!(text_edit.new_text, ")");
}

#[test]
fn code_actions_offer_discard_non_void_value_fix() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn helper() i32 { return 1; }\nfn main() void {\n    helper();\n}\n";
    let uri = temp_file_uri("code_action_discard_value", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 2,
                    character: 4,
                },
                end: Position {
                    line: 2,
                    character: 12,
                },
            },
        )
        .unwrap();

    let action = actions
        .iter()
        .find(|action| action.title == "Discard value with `let _ =`")
        .unwrap();
    let edit = action.edit.as_ref().unwrap();
    let text_edit = edit.changes.get(&uri).unwrap().first().unwrap();

    assert_eq!(
        text_edit.range.start,
        Position {
            line: 2,
            character: 4,
        }
    );
    assert_eq!(text_edit.new_text, "let _ = ");
}

#[test]
fn code_actions_offer_let_mut_fix() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() void {\n    let value = 1;\n    value = 2;\n}\n";
    let uri = temp_file_uri("code_action_let_mut", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 2,
                    character: 4,
                },
                end: Position {
                    line: 2,
                    character: 13,
                },
            },
        )
        .unwrap();

    let action = actions
        .iter()
        .find(|action| action.title == "Change to `let mut`")
        .unwrap();
    let edit = action.edit.as_ref().unwrap();
    let text_edit = edit.changes.get(&uri).unwrap().first().unwrap();

    assert_eq!(
        text_edit.range.start,
        Position {
            line: 1,
            character: 8,
        }
    );
    assert_eq!(text_edit.new_text, "mut ");
}

#[test]
fn code_actions_offer_match_catch_all_fix() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() i32 {\n    return match (1) {\n        1 => 1,\n    };\n}\n";
    let uri = temp_file_uri("code_action_match_catch_all", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 1,
                    character: 4,
                },
                end: Position {
                    line: 3,
                    character: 5,
                },
            },
        )
        .unwrap();

    let action = actions
        .iter()
        .find(|action| action.title == "Add `_ => @unreachable()` arm")
        .unwrap();
    let edit = action.edit.as_ref().unwrap();
    let text_edit = edit.changes.get(&uri).unwrap().first().unwrap();

    assert_eq!(
        text_edit.range.start,
        Position {
            line: 3,
            character: 4,
        }
    );
    assert_eq!(text_edit.new_text, "        _ => @unreachable(),\n");
    assert_eq!(action.is_preferred, Some(false));
}

#[test]
fn code_actions_remove_irrefutable_let_else_branch() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() void {\n    let value = 1 else return;\n}\n";
    let uri = temp_file_uri("code_action_remove_let_else", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 1,
                    character: 4,
                },
                end: Position {
                    line: 1,
                    character: 29,
                },
            },
        )
        .unwrap();

    let action = actions
        .iter()
        .find(|action| action.title == "Remove invalid `else` branch")
        .unwrap();
    let edit = action.edit.as_ref().unwrap();
    let text_edit = edit.changes.get(&uri).unwrap().first().unwrap();

    assert_eq!(
        text_edit.range.start,
        Position {
            line: 1,
            character: 18,
        }
    );
    assert_eq!(
        text_edit.range.end,
        Position {
            line: 1,
            character: 29,
        }
    );
    assert_eq!(text_edit.new_text, "");
}

#[test]
fn overlay_text_is_used_for_compiler_diagnostics() {
    let mut analysis = AnalysisEngine::default();
    let uri = temp_file_uri("overlay_diag", "extern fn main() i32 { 0 }");

    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: "extern fn main( ".to_string(),
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
fn computes_cleared_uris() {
    let previous = BTreeSet::from(["file:///one.rn".to_string(), "file:///two.rn".to_string()]);
    let current = vec![super::DiagnosticBundle {
        uri: "file:///one.rn".to_string(),
        diagnostics: Vec::new(),
    }];

    let cleared = cleared_uris(&previous, &current);
    assert_eq!(cleared, vec!["file:///two.rn".to_string()]);
}

#[test]
fn extracts_document_symbols_from_compiler_artifact() {
    let mut analysis = AnalysisEngine::default();
    let uri = temp_file_uri(
        "document_symbols",
        "type Point = struct { x: i32, y: i32 };\nfn helper() i32 { return 1; }\n",
    );

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: "type Point = struct { x: i32, y: i32 };\nfn helper() i32 { return 1; }\n"
                .to_string(),
        },
    });

    let symbols = analysis.document_symbols(&uri).unwrap();
    let names = symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"Point"));
    assert!(names.contains(&"helper"));
}

#[test]
fn goto_definition_resolves_local_identifier_references() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn helper() i32 {\n    let value = i32.{1};\n    return value;\n}\n";
    let uri = temp_file_uri("goto_definition_local", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let query_position = position_of_nth(source, "value", 1, 2);
    let definition = analysis
        .goto_definition(&uri, query_position)
        .unwrap()
        .unwrap();

    assert_eq!(definition.uri, uri);
    assert_eq!(
        definition.range.start,
        position_of_nth(source, "value", 0, 0)
    );
}

#[test]
fn goto_definition_resolves_function_identifier_references() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper(); }\n";
    let uri = temp_file_uri("goto_definition_function", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let query_position = position_of_nth(source, "helper", 1, 1);
    let definition = analysis
        .goto_definition(&uri, query_position)
        .unwrap()
        .unwrap();

    assert_eq!(definition.uri, uri);
    assert_eq!(
        definition.range.start,
        position_of_nth(source, "helper", 0, 0)
    );
}

#[test]
fn finds_references_from_identifier_reference_position() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper() + helper(); }\n";
    let uri = temp_file_uri("references_from_ref", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let query_position = position_of_nth(source, "helper", 1, 1);
    let locations = analysis.references(&uri, query_position, false).unwrap();

    assert_eq!(locations.len(), 2);
    assert_eq!(
        locations[0].range.start,
        position_of_nth(source, "helper", 1, 0)
    );
    assert_eq!(
        locations[1].range.start,
        position_of_nth(source, "helper", 2, 0)
    );
}

#[test]
fn finds_references_from_definition_position_including_declaration() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper(); }\n";
    let uri = temp_file_uri("references_from_def", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let query_position = position_of_nth(source, "helper", 0, 1);
    let locations = analysis.references(&uri, query_position, true).unwrap();

    assert_eq!(locations.len(), 2);
    assert_eq!(
        locations[0].range.start,
        position_of_nth(source, "helper", 0, 0)
    );
    assert_eq!(
        locations[1].range.start,
        position_of_nth(source, "helper", 1, 0)
    );
}

#[test]
fn document_highlights_include_definition_and_same_file_references() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper() + helper(); }\n";
    let uri = temp_file_uri("document_highlights", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let highlights = analysis
        .document_highlights(&uri, position_of_nth(source, "helper", 1, 1))
        .unwrap();

    assert_eq!(highlights.len(), 3);
    assert_eq!(
        highlights[0].range.start,
        position_of_nth(source, "helper", 0, 0)
    );
    assert_eq!(
        highlights[1].range.start,
        position_of_nth(source, "helper", 1, 0)
    );
    assert_eq!(
        highlights[2].range.start,
        position_of_nth(source, "helper", 2, 0)
    );
    assert!(highlights.iter().all(|highlight| highlight.kind == Some(1)));
}

#[test]
fn hover_resolves_function_signature_from_reference() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn helper(x: i32) i32 { return x; }\nfn main() i32 { return helper(1); }\n";
    let uri = temp_file_uri("hover_function", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hover = analysis
        .hover(&uri, position_of_nth(source, "helper", 1, 1))
        .unwrap()
        .unwrap();

    assert!(hover.contents.value.contains("fn helper: fn(i32) i32"));
}

#[test]
fn signature_help_resolves_function_parameters_and_active_argument() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper(first: i32, second: i32) i32 {\n",
        "    return first + second;\n",
        "}\n",
        "fn main() i32 {\n",
        "    let value = i32.{2};\n",
        "    return helper(1, value);\n",
        "}\n",
    );
    let uri = temp_file_uri("signature_help", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let help = analysis
        .signature_help(&uri, position_of_nth(source, "value", 1, 1))
        .unwrap()
        .unwrap();

    assert_eq!(help.active_signature, 0);
    assert_eq!(help.active_parameter, 1);
    assert_eq!(help.signatures.len(), 1);
    assert_eq!(
        help.signatures[0].label,
        "helper(first: i32, second: i32) i32"
    );
    assert_eq!(help.signatures[0].parameters.len(), 2);
    assert_eq!(help.signatures[0].parameters[0].label, "first: i32");
    assert_eq!(help.signatures[0].parameters[1].label, "second: i32");
}

#[test]
fn hover_resolves_local_definition_without_references() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() i32 {\n    let value = i32.{1};\n    return 0;\n}\n";
    let uri = temp_file_uri("hover_local_definition", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hover = analysis
        .hover(&uri, position_of_nth(source, "value", 0, 1))
        .unwrap()
        .unwrap();

    assert!(hover.contents.value.contains("var value: i32"));
}

#[test]
fn prepare_rename_returns_placeholder_for_reference() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper(); }\n";
    let uri = temp_file_uri("prepare_rename", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let result = analysis
        .prepare_rename(&uri, position_of_nth(source, "helper", 1, 1))
        .unwrap()
        .unwrap();

    assert_eq!(result.placeholder, "helper");
    assert_eq!(result.range.start, position_of_nth(source, "helper", 1, 0));
}

#[test]
fn rename_updates_definition_and_references() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper() + helper(); }\n";
    let uri = temp_file_uri("rename_function", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let edit = analysis
        .rename(&uri, position_of_nth(source, "helper", 1, 1), "assist")
        .unwrap();
    let edits = edit.changes.get(&uri).unwrap();

    assert_eq!(edits.len(), 3);
    assert!(edits.iter().all(|edit| edit.new_text == "assist"));
    assert_eq!(
        edits[0].range.start,
        position_of_nth(source, "helper", 0, 0)
    );
    assert_eq!(
        edits[1].range.start,
        position_of_nth(source, "helper", 1, 0)
    );
    assert_eq!(
        edits[2].range.start,
        position_of_nth(source, "helper", 2, 0)
    );
}

#[test]
fn rename_rejects_invalid_identifiers() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn helper() i32 { return 1; }\n";
    let uri = temp_file_uri("rename_invalid", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let error = analysis
        .rename(&uri, position_of_nth(source, "helper", 0, 1), "fn")
        .unwrap_err();

    assert!(error.contains("not a valid Kern identifier"));
}

#[test]
fn byte_offsets_roundtrip_through_utf16_positions() {
    let file = SourceFile::new(PathBuf::from("utf16.rn"), "a😀b\n".to_string());
    let offset = "a😀".len();
    let position = byte_offset_to_position(&file, offset);

    assert_eq!(
        position,
        Position {
            line: 0,
            character: 3,
        }
    );
    assert_eq!(position_to_byte_offset(&file, &position), Some(offset));
}

#[test]
fn completion_in_function_body_includes_visible_symbols() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Point = struct { x: i32 };\n",
        "fn helper(param: i32) i32 {\n",
        "    let value = param;\n",
        "    return value;\n",
        "}\n",
    );
    let uri = temp_file_uri("completion_function", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let items = analysis
        .completion(&uri, position_of_nth(source, "return", 0, 0))
        .unwrap();
    let labels = completion_labels(&items);

    assert!(labels.contains(&"Point".to_string()));
    assert!(labels.contains(&"helper".to_string()));
    assert!(labels.contains(&"param".to_string()));
    assert!(labels.contains(&"value".to_string()));

    let helper = items.iter().find(|item| item.label == "helper").unwrap();
    assert_eq!(helper.insert_text.as_deref(), Some("helper($0)"));
    assert_eq!(helper.insert_text_format, Some(2));
}

#[test]
fn completion_filters_and_sorts_items_by_typed_prefix() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper() void {}\n",
        "fn help() void {}\n",
        "fn main() void {\n",
        "    hel\n",
        "}\n",
    );
    let uri = temp_file_uri("completion_prefix", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let items = analysis
        .completion(&uri, position_of_nth(source, "hel", 1, 3))
        .unwrap();
    let labels = completion_labels(&items);

    assert_eq!(labels, vec!["help".to_string(), "helper".to_string()]);
}

#[test]
fn completion_avoids_duplicate_call_parentheses_when_already_present() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 {\n    return hel();\n}\n";
    let uri = temp_file_uri("completion_existing_paren", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let items = analysis
        .completion(&uri, position_of_nth(source, "hel", 1, 3))
        .unwrap();
    let helper = items.iter().find(|item| item.label == "helper").unwrap();

    assert_eq!(helper.insert_text, None);
    assert_eq!(helper.insert_text_format, None);
}

#[test]
fn completion_prefers_types_in_type_annotations() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type MarkerType = struct {};\n",
        "fn Mark() MarkerType { return MarkerType.{}; }\n",
        "fn main() void {\n",
        "    let value = Mark() as Mar;\n",
        "}\n",
    );
    let uri = temp_file_uri("completion_type_context", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let items = analysis
        .completion(&uri, position_of_nth(source, " as Mar", 0, 7))
        .unwrap();
    let labels = completion_labels(&items);

    assert!(labels.starts_with(&["MarkerType".to_string(), "Mark".to_string()]));
}

#[test]
fn completion_in_method_body_includes_self() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Counter = struct { value: i32 };\n",
        "impl Counter {\n",
        "    fn get() i32 {\n",
        "        return self.value;\n",
        "    }\n",
        "}\n",
    );
    let uri = temp_file_uri("completion_method", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let items = analysis
        .completion(&uri, position_of_nth(source, "self", 0, 0))
        .unwrap();
    let labels = completion_labels(&items);

    assert!(labels.contains(&"self".to_string()));
    assert!(labels.contains(&"Counter".to_string()));
}

#[test]
fn completion_on_field_access_returns_member_items() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Point = struct { x: i32, y: i32 };\n",
        "fn main() i32 {\n",
        "    let point = Point.{ x: 1, y: 2 };\n",
        "    return point.x;\n",
        "}\n",
    );
    let uri = temp_file_uri("completion_field_access", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let items = analysis
        .completion(&uri, position_of_nth(source, "point", 1, 5))
        .unwrap();
    let labels = completion_labels(&items);

    assert_eq!(labels, vec!["x".to_string(), "y".to_string()]);
}

#[test]
fn completion_on_generic_bound_receiver_includes_trait_methods() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type HasLen = trait { len: fn() i32, };\n",
        "impl *i32 : HasLen {\n",
        "    pub fn len() i32 { return self.*; }\n",
        "}\n",
        "fn use_it[T](x: *T) i32\n",
        "    where *T: HasLen,\n",
        "{\n",
        "    return x.len();\n",
        "}\n",
    );
    let uri = temp_file_uri("completion_generic_bound", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let items = analysis
        .completion(&uri, position_of_nth(source, "x", 1, 1))
        .unwrap();
    let labels = completion_labels(&items);

    assert!(labels.contains(&"len".to_string()));
}

#[test]
fn resolve_analysis_uses_workspace_package_root_and_local_aliases() {
    let root = unique_temp_dir("analysis_workspace");
    let app_dir = root.join("app");
    let util_dir = root.join("util");
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::create_dir_all(util_dir.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        "[workspace]\nmembers = [\"app\", \"util\"]\n",
    )
    .unwrap();
    fs::write(
        app_dir.join("Craft.toml"),
        "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.7\"

[lib]
root = \"src/lib.rn\"

[dependencies]
util = { path = \"../util\" }
",
    )
    .unwrap();
    fs::write(app_dir.join("src/lib.rn"), "mod sub;\n").unwrap();
    fs::write(app_dir.join("src/sub.rn"), "fn local() i32 { return 1; }\n").unwrap();
    fs::write(
        util_dir.join("Craft.toml"),
        "\
[package]
name = \"util\"
version = \"0.1.0\"
kern = \"0.7\"

[lib]
root = \"src/lib.rn\"
",
    )
    .unwrap();
    fs::write(
        util_dir.join("src/lib.rn"),
        "fn helper() i32 { return 1; }\n",
    )
    .unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&app_dir.join("src/sub.rn")).unwrap();
    let source = fs::read_to_string(app_dir.join("src/sub.rn")).unwrap();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source,
        },
    });

    let resolved = analysis.resolve_analysis(&uri).unwrap();

    assert_eq!(
        super::normalize_path(&resolved.input_file),
        super::normalize_path(&app_dir.join("src/lib.rn"))
    );
    assert_eq!(
        resolved.compile_options.root_module_name,
        Some("app".to_string())
    );
    assert_eq!(
        resolved
            .compile_options
            .module_aliases
            .get("util")
            .map(PathBuf::from),
        Some(super::normalize_path(&util_dir.join("src/lib.rn")))
    );
    assert!(resolved.compile_options.module_aliases.contains_key("std"));
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
        "{}_{}_{}_{}.rn",
        prefix,
        std::process::id(),
        nanos,
        counter
    ))
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

fn completion_labels(items: &[crate::protocol::CompletionItem]) -> Vec<String> {
    items.iter().map(|item| item.label.clone()).collect()
}

fn decode_semantic_tokens(tokens: &SemanticTokens) -> Vec<(Position, u32, u32, u32)> {
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
