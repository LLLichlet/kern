use super::semantic::{SemanticModifiers, SemanticTokenTypes};
use super::{
    AnalysisEngine, AnalysisSettings, byte_offset_to_position, cleared_uris, file_path_to_uri,
    position_to_byte_offset, uri_to_file_path,
};
use crate::protocol::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams, Position,
    Range, SemanticTokens, TextDocumentContentChangeEvent, TextDocumentItem,
    VersionedTextDocumentIdentifier,
};
use craft::analysis_context;
use kernc_utils::SourceFile;
use kernc_utils::config::CompileOptions;
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
fn diagnostics_include_native_doc_lints_as_warnings() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "///\n",
        "/// Strange:\n",
        "/// - x: described in an unknown section.\n",
        "/// Args:\n",
        "/// - y: does not exist.\n",
        "fn helper(x: i32) i32 { return x; }\n",
        "fn main() i32 { return helper(1); }\n",
    );
    let uri = temp_file_uri("doc_lints", source);

    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(
        bundle
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity == 2)
    );
    assert!(
        bundle
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.message.contains("missing a summary paragraph") })
    );
    assert!(
        bundle
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.message.contains("unknown doc section `Strange`") })
    );
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("unknown documented argument `y`")
    }));
}

#[test]
fn diagnostics_include_native_doc_lints_for_impl_methods() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Counter = struct { value: i32 };\n",
        "impl Counter {\n",
        "    /// Read the counter.\n",
        "    /// Args:\n",
        "    /// - missing: not a real parameter.\n",
        "    fn get() i32 { return self.value; }\n",
        "}\n",
    );
    let uri = temp_file_uri("doc_lints_impl_method", source);

    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(
        bundle
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity == 2)
    );
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("unknown documented argument `missing`")
    }));
}

#[test]
fn diagnostics_warn_for_unreachable_private_function_chain() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn leaf() i32 { return 1; }\n",
        "fn helper() i32 { return leaf(); }\n",
        "fn main() i32 { return 0; }\n",
    );
    let uri = temp_file_uri("unused_private_chain", source);

    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("private function `helper` is never used")
    }));
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("private function `leaf` is never used")
    }));
}

#[test]
fn diagnostics_warn_for_unreachable_private_constant() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!("const helper = 1;\n", "fn main() i32 { return 0; }\n",);
    let uri = temp_file_uri("unused_private_const", source);

    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("private constant `helper` is never used")
    }));
}

#[test]
fn diagnostics_warn_for_unreachable_private_static() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!("static helper = 1;\n", "fn main() i32 { return 0; }\n",);
    let uri = temp_file_uri("unused_private_static", source);

    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("private static `helper` is never used")
    }));
}

#[test]
fn diagnostics_warn_for_unused_parameter_and_local_binding() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper(_: i32, unused_param: i32, used_param: i32) i32 {\n",
        "    let unused_local = used_param;\n",
        "    return used_param;\n",
        "}\n",
        "fn main() i32 { return helper(1, 2, 3); }\n",
    );
    let uri = temp_file_uri("unused_bindings", source);

    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("parameter `unused_param` is never used")
    }));
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("local variable `unused_local` is never used")
    }));
    assert!(
        !bundle
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.message.contains("parameter `_` is never used") })
    );
}

#[test]
fn diagnostics_warn_for_dead_store_assignment() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    if (seed == 0) { return value; }\n",
        "    value = seed + 1;\n",
        "    value = seed + 2;\n",
        "    return value;\n",
        "}\n",
        "fn main() i32 { return helper(1); }\n",
    );
    let uri = temp_file_uri("dead_store_assignment", source);

    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("value assigned to `value` is never read")
    }));
}

#[test]
fn diagnostics_warn_for_dead_initializer() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    return value;\n",
        "}\n",
        "fn main() i32 { return helper(1); }\n",
    );
    let uri = temp_file_uri("dead_initializer", source);

    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("initial value assigned to `value` is never read")
    }));
}

#[test]
fn diagnostics_warn_for_unreachable_private_item_chain() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "const leaf = i32.{1};\n",
        "fn helper() i32 { return leaf; }\n",
        "fn main() i32 { return 0; }\n",
    );
    let uri = temp_file_uri("unused_private_item_chain", source);

    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("private function `helper` is never used")
    }));
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("private constant `leaf` is never used")
    }));
}

#[test]
fn public_reexport_marks_private_function_as_reachable_root() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper() i32 { return 1; }\n",
        "pub use .helper as exported;\n",
        "fn main() i32 { return 0; }\n",
    );
    let uri = temp_file_uri("unused_private_reexport_root", source);

    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(
        !bundle.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("private function `helper` is never used")),
        "unexpected diagnostics: {:?}",
        bundle.diagnostics
    );
}

#[test]
fn body_only_change_recomputes_unused_private_function_warning() {
    let mut analysis = AnalysisEngine::default();
    let initial = concat!(
        "fn helper() i32 { return 1; }\n",
        "fn main() i32 { return 0; }\n",
    );
    let updated = concat!(
        "fn helper() i32 { return 1; }\n",
        "fn main() i32 { return helper(); }\n",
    );
    let uri = temp_file_uri("unused_private_incremental", initial);

    let open_outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: initial.to_string(),
        },
    });
    let open_bundle = open_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(open_bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("private function `helper` is never used")
    }));

    let change_outcome = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: updated.to_string(),
        }],
    });
    let change_bundle = change_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(
        !change_bundle.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("private function `helper` is never used")),
        "unexpected diagnostics: {:?}",
        change_bundle.diagnostics
    );
}

#[test]
fn body_only_change_recomputes_unused_private_constant_warning() {
    let mut analysis = AnalysisEngine::default();
    let initial = concat!("const helper = 1;\n", "fn main() i32 { return 0; }\n",);
    let updated = concat!("const helper = 1;\n", "fn main() i32 { return helper; }\n",);
    let uri = temp_file_uri("unused_private_const_incremental", initial);

    let open_outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: initial.to_string(),
        },
    });
    let open_bundle = open_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(open_bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("private constant `helper` is never used")
    }));

    let change_outcome = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: updated.to_string(),
        }],
    });
    let change_bundle = change_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(
        !change_bundle.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("private constant `helper` is never used")),
        "unexpected diagnostics: {:?}",
        change_bundle.diagnostics
    );
}

#[test]
fn body_only_change_recomputes_unused_private_static_warning() {
    let mut analysis = AnalysisEngine::default();
    let initial = concat!("static helper = 1;\n", "fn main() i32 { return 0; }\n",);
    let updated = concat!("static helper = 1;\n", "fn main() i32 { return helper; }\n",);
    let uri = temp_file_uri("unused_private_static_incremental", initial);

    let open_outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: initial.to_string(),
        },
    });
    let open_bundle = open_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(open_bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("private static `helper` is never used")
    }));

    let change_outcome = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: updated.to_string(),
        }],
    });
    let change_bundle = change_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(
        !change_bundle.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("private static `helper` is never used")),
        "unexpected diagnostics: {:?}",
        change_bundle.diagnostics
    );
}

#[test]
fn body_only_change_recomputes_unused_binding_warnings() {
    let mut analysis = AnalysisEngine::default();
    let initial = concat!(
        "fn helper(unused_param: i32, used_param: i32) i32 {\n",
        "    let unused_local = used_param;\n",
        "    return used_param;\n",
        "}\n",
        "fn main() i32 { return helper(1, 2); }\n",
    );
    let updated = concat!(
        "fn helper(unused_param: i32, used_param: i32) i32 {\n",
        "    let unused_local = used_param;\n",
        "    if (unused_param == 0) { return unused_local; }\n",
        "    return used_param;\n",
        "}\n",
        "fn main() i32 { return helper(1, 2); }\n",
    );
    let uri = temp_file_uri("unused_bindings_incremental", initial);

    let open_outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: initial.to_string(),
        },
    });
    let open_bundle = open_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(open_bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("parameter `unused_param` is never used")
    }));
    assert!(open_bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("local variable `unused_local` is never used")
    }));

    let change_outcome = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: updated.to_string(),
        }],
    });
    let change_bundle = change_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(
        !change_bundle.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("parameter `unused_param` is never used")),
        "unexpected diagnostics: {:?}",
        change_bundle.diagnostics
    );
    assert!(
        !change_bundle.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("local variable `unused_local` is never used")),
        "unexpected diagnostics: {:?}",
        change_bundle.diagnostics
    );
}

#[test]
fn body_only_change_recomputes_dead_store_warnings() {
    let mut analysis = AnalysisEngine::default();
    let initial = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    value = seed + 2;\n",
        "    return value;\n",
        "}\n",
        "fn main() i32 { return helper(1); }\n",
    );
    let updated = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    if (seed == 0) { return value; }\n",
        "    value = seed + 2;\n",
        "    return value;\n",
        "}\n",
        "fn main() i32 { return helper(1); }\n",
    );
    let uri = temp_file_uri("dead_store_incremental", initial);

    let open_outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: initial.to_string(),
        },
    });
    let open_bundle = open_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(open_bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("value assigned to `value` is never read")
    }));

    let change_outcome = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: updated.to_string(),
        }],
    });
    let change_bundle = change_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(
        !change_bundle.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("value assigned to `value` is never read")),
        "unexpected diagnostics: {:?}",
        change_bundle.diagnostics
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
fn semantic_tokens_for_dirty_documents_fall_back_to_lexical_tokens() {
    let mut analysis = AnalysisEngine::default();
    let uri = temp_file_uri(
        "semantic_tokens_dirty_fallback",
        "fn main() i32 { return 1; }\n",
    );

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: "fn main() i32 { return 1; }\n".to_string(),
        },
    });

    let _ = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: "fn main() i32 { return \n".to_string(),
        }],
    });

    let decoded = decode_semantic_tokens(&analysis.semantic_tokens(&uri).unwrap());
    assert!(!decoded.is_empty());
    assert!(
        decoded
            .iter()
            .any(|token| token.2 == SemanticTokenTypes::KEYWORD)
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
fn semantic_tokens_classify_enum_variant_references() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Result = enum { Ok: i32, Err };\n",
        "fn main() i32 {\n",
        "    let value = Result.{ Ok: i32.{1} };\n",
        "    let other = Result.{ Err };\n",
        "    let _ = value;\n",
        "    let _ = other;\n",
        "    return 0;\n",
        "}\n",
    );
    let uri = temp_file_uri("semantic_tokens_enum_variant", source);

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
        position_of_nth(source, "Ok", 1, 0),
        SemanticTokenTypes::ENUM,
        0,
    );
    assert_token(
        &decoded,
        position_of_nth(source, "Err", 1, 0),
        SemanticTokenTypes::ENUM,
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
fn document_symbols_use_surface_cache_without_body_artifact() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Point = struct { x: i32 };\n",
        "fn helper(point: Point) i32 {\n",
        "    return point.x;\n",
        "}\n",
    );
    let uri = temp_file_uri("document_symbols_structure_only", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    analysis.parse_cache.borrow_mut().clear();
    analysis.surface_cache.borrow_mut().clear();
    analysis.structure_cache.borrow_mut().clear();
    analysis.artifact_cache.borrow_mut().clear();
    assert_eq!(analysis.parse_cache.borrow().len(), 0);
    assert_eq!(analysis.surface_cache.borrow().len(), 0);
    assert_eq!(analysis.structure_cache.borrow().len(), 0);
    assert_eq!(analysis.artifact_cache.borrow().len(), 0);

    let symbols = analysis.document_symbols(&uri).unwrap();
    let names = symbols
        .iter()
        .map(|symbol| symbol.name.clone())
        .collect::<Vec<_>>();

    assert!(names.contains(&"Point".to_string()));
    assert!(names.contains(&"helper".to_string()));
    assert_eq!(analysis.parse_cache.borrow().len(), 0);
    assert_eq!(analysis.surface_cache.borrow().len(), 1);
    assert_eq!(analysis.structure_cache.borrow().len(), 0);
    assert_eq!(analysis.artifact_cache.borrow().len(), 0);
}

#[test]
fn document_symbols_use_collected_outline_names_for_impl_blocks() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Point = struct { x: i32 };\n",
        "impl Point {\n",
        "    fn magnitude() i32 { return self.x; }\n",
        "}\n",
    );
    let uri = temp_file_uri("document_symbols_impl_outline", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let symbols = analysis.document_symbols(&uri).unwrap();
    let impl_symbol = symbols
        .iter()
        .find(|symbol| symbol.name == "impl Point")
        .expect("impl block should use collected outline naming");

    assert_eq!(impl_symbol.detail.as_deref(), Some("impl"));
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
fn goto_definition_resolves_impl_method_references() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Counter = struct { value: i32 };\n",
        "impl Counter {\n",
        "    fn get() i32 { return self.value; }\n",
        "}\n",
        "fn main() i32 {\n",
        "    let counter = Counter.{ value: i32.{1} };\n",
        "    return counter.get();\n",
        "}\n",
    );
    let uri = temp_file_uri("goto_definition_impl_method", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let definition = analysis
        .goto_definition(&uri, position_of_nth(source, "get", 1, 1))
        .unwrap()
        .unwrap();

    assert_eq!(definition.uri, uri);
    assert_eq!(definition.range.start, position_of_nth(source, "get", 0, 0));
}

#[test]
fn goto_definition_resolves_struct_field_references() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Counter = struct { value: i32 };\n",
        "fn main() i32 {\n",
        "    let counter = Counter.{ value: i32.{1} };\n",
        "    return counter.value;\n",
        "}\n",
    );
    let uri = temp_file_uri("goto_definition_struct_field", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let definition = analysis
        .goto_definition(&uri, position_of_nth(source, "value", 2, 1))
        .unwrap()
        .unwrap();

    assert_eq!(definition.uri, uri);
    assert_eq!(
        definition.range.start,
        position_of_nth(source, "value", 0, 0)
    );
}

#[test]
fn goto_definition_resolves_enum_variant_references() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Result = enum { Ok: i32, Err };\n",
        "fn main() i32 {\n",
        "    let value = Result.{ Ok: i32.{1} };\n",
        "    return 0;\n",
        "}\n",
    );
    let uri = temp_file_uri("goto_definition_enum_variant", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let definition = analysis
        .goto_definition(&uri, position_of_nth(source, "Ok", 1, 1))
        .unwrap()
        .unwrap();

    assert_eq!(definition.uri, uri);
    assert_eq!(definition.range.start, position_of_nth(source, "Ok", 0, 0));
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
fn hover_renders_native_docs_after_signature() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "/// Read one byte from the receiver register.\n",
        "///\n",
        "/// Safety:\n",
        "/// - `self` must point to a mapped UART object.\n",
        "fn helper(x: i32) i32 { return x; }\n",
        "fn main() i32 { return helper(1); }\n",
    );
    let uri = temp_file_uri("hover_docs", source);

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
    assert!(
        hover
            .contents
            .value
            .contains("Read one byte from the receiver register.")
    );
    assert!(hover.contents.value.contains("**Safety**"));
    assert!(
        hover
            .contents
            .value
            .contains("`self` must point to a mapped UART object.")
    );
}

#[test]
fn hover_reuses_docs_from_imported_kmeta_packages() {
    let root = unique_temp_dir("hover_imported_kmeta_docs");
    let dep_meta = root.join("dep-meta");
    fs::create_dir_all(dep_meta.join("src")).unwrap();

    fs::write(
        dep_meta.join("Kmeta.toml"),
        concat!(
            "format_version = 2\n",
            "kind = \"source_snapshot\"\n",
            "package_name = \"dep\"\n",
            "package_version = \"0.1.0\"\n",
            "root_module_name = \"dep\"\n",
            "entry_module_path = \"src/init.rn\"\n",
        ),
    )
    .unwrap();
    fs::write(
        dep_meta.join("src/init.rn"),
        concat!(
            "/// Imported helper from a kmeta package.\n",
            "///\n",
            "/// Safety:\n",
            "/// - Pure helper with no hidden runtime policy.\n",
            "pub fn helper() i32 { return 1; }\n",
        ),
    )
    .unwrap();

    let app_source = "use dep.{helper};\nfn main() i32 { return helper(); }\n";
    let app_path = root.join("app.rn");
    fs::write(&app_path, app_source).unwrap();

    let mut options = CompileOptions {
        use_std: true,
        ..CompileOptions::default()
    };
    options
        .module_interface_aliases
        .insert("dep".to_string(), dep_meta.to_string_lossy().to_string());

    let mut analysis = AnalysisEngine::new(AnalysisSettings {
        compile_options: options,
    });
    let uri = file_path_to_uri(&app_path).unwrap();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: app_source.to_string(),
        },
    });

    let hover = analysis
        .hover(&uri, position_of_nth(app_source, "helper", 1, 1))
        .unwrap()
        .unwrap();

    assert!(hover.contents.value.contains("fn helper: fn() i32"));
    assert!(
        hover
            .contents
            .value
            .contains("Imported helper from a kmeta package.")
    );
    assert!(hover.contents.value.contains("**Safety**"));
    assert!(
        hover
            .contents
            .value
            .contains("Pure helper with no hidden runtime policy.")
    );
}

#[test]
fn hover_resolves_std_module_docs_from_use_alias() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "use std.io;\n",
        "\n",
        "extern fn main(args: [][]u8) i32 {\n",
        "    io.println(\"hello\", .{});\n",
        "    return 0;\n",
        "}\n",
    );
    let uri = temp_file_uri("hover_std_module_alias", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hover = analysis
        .hover(&uri, position_of_nth(source, "io", 1, 1))
        .unwrap()
        .unwrap();

    assert!(hover.contents.value.contains("module io"));
    assert!(
        hover
            .contents
            .value
            .contains("Text and byte-oriented output helpers.")
    );
}

#[test]
fn hover_resolves_std_reexported_function_docs_from_member_access() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "use std.io;\n",
        "\n",
        "extern fn main(args: [][]u8) i32 {\n",
        "    io.println(\"hello\", .{});\n",
        "    return 0;\n",
        "}\n",
    );
    let uri = temp_file_uri("hover_std_member_function", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hover = analysis
        .hover(&uri, position_of_nth(source, "println", 0, 1))
        .unwrap()
        .unwrap();

    assert!(hover.contents.value.contains("fn println:"));
    assert!(
        hover
            .contents
            .value
            .contains("Formats into standard output and appends a newline.")
    );
}

#[test]
fn hover_resolves_impl_method_signature_from_reference() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Counter = struct { value: i32 };\n",
        "impl Counter {\n",
        "    fn get() i32 { return self.value; }\n",
        "}\n",
        "fn main() i32 {\n",
        "    let counter = Counter.{ value: i32.{1} };\n",
        "    return counter.get();\n",
        "}\n",
    );
    let uri = temp_file_uri("hover_impl_method_reference", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hover = analysis
        .hover(&uri, position_of_nth(source, "get", 1, 1))
        .unwrap()
        .unwrap();

    assert!(hover.contents.value.contains("fn get: fn(Counter) i32"));
}

#[test]
fn hover_renders_doc_comments_for_impl_method_reference() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Counter = struct { value: i32 };\n",
        "impl Counter {\n",
        "    /// Read the current counter value.\n",
        "    ///\n",
        "    /// Safety:\n",
        "    /// - keep `self` bound to a live counter object.\n",
        "    fn get() i32 { return self.value; }\n",
        "}\n",
        "fn main() i32 {\n",
        "    let counter = Counter.{ value: i32.{1} };\n",
        "    return counter.get();\n",
        "}\n",
    );
    let uri = temp_file_uri("hover_impl_method_docs", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hover = analysis
        .hover(&uri, position_of_nth(source, "get", 1, 1))
        .unwrap()
        .unwrap();

    assert!(hover.contents.value.contains("fn get: fn(Counter) i32"));
    assert!(
        hover
            .contents
            .value
            .contains("Read the current counter value.")
    );
    assert!(hover.contents.value.contains("**Safety**"));
    assert!(
        hover
            .contents
            .value
            .contains("keep `self` bound to a live counter object.")
    );
}

#[test]
fn hover_resolves_struct_field_from_reference() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Counter = struct { value: i32 };\n",
        "fn main() i32 {\n",
        "    let counter = Counter.{ value: i32.{1} };\n",
        "    return counter.value;\n",
        "}\n",
    );
    let uri = temp_file_uri("hover_struct_field_reference", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hover = analysis
        .hover(&uri, position_of_nth(source, "value", 2, 1))
        .unwrap()
        .unwrap();

    assert!(hover.contents.value.contains("field value: i32"));
}

#[test]
fn hover_resolves_enum_variant_from_reference() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Result = enum { Ok: i32, Err };\n",
        "fn main() i32 {\n",
        "    let value = Result.{ Ok: i32.{1} };\n",
        "    let _ = value;\n",
        "    return 0;\n",
        "}\n",
    );
    let uri = temp_file_uri("hover_enum_variant_reference", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hover = analysis
        .hover(&uri, position_of_nth(source, "Ok", 1, 1))
        .unwrap()
        .unwrap();

    assert!(hover.contents.value.contains("variant Ok: i32"));
}

#[test]
fn hover_resolves_match_variant_pattern_from_reference() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Result = enum { Ok: i32, Err };\n",
        "fn main() i32 {\n",
        "    let value = Result.{ Err };\n",
        "    return match (value) {\n",
        "        .Err => 0,\n",
        "        .{ Ok: payload } => payload,\n",
        "    };\n",
        "}\n",
    );
    let uri = temp_file_uri("hover_match_variant_pattern_reference", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hover = analysis
        .hover(&uri, position_of_nth(source, "Err", 2, 1))
        .unwrap()
        .unwrap();

    assert!(hover.contents.value.contains("variant Err"));
}

#[test]
fn hover_resolves_typed_match_variant_path_from_reference() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Result = enum { Ok: i32, Err };\n",
        "fn main() i32 {\n",
        "    let value = Result.{ Ok: i32.{1} };\n",
        "    return match (value) {\n",
        "        Result.{ Ok: payload } => payload,\n",
        "        .Err => 0,\n",
        "    };\n",
        "}\n",
    );
    let uri = temp_file_uri("hover_typed_match_variant_path_reference", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hover = analysis
        .hover(&uri, position_of_nth(source, "Ok", 2, 1))
        .unwrap()
        .unwrap();

    assert!(hover.contents.value.contains("variant Ok: i32"));
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
fn hover_on_impl_method_definition_prefers_method_span() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Counter = struct {};\n",
        "impl Counter {\n",
        "    fn get() i32 { return 1; }\n",
        "}\n",
    );
    let uri = temp_file_uri("hover_impl_method_definition", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hover = analysis
        .hover(&uri, position_of_nth(source, "get", 0, 1))
        .unwrap()
        .unwrap();
    let range = hover.range.unwrap();

    assert!(hover.contents.value.contains("fn get:"));
    assert_eq!(range.start, position_of_nth(source, "get", 0, 0));
    assert_eq!(range.end, position_of_nth(source, "get", 0, 3));
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
fn rename_updates_local_binding_definition_and_references() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() i32 {\n    let value = i32.{1};\n    return value + value;\n}\n";
    let uri = temp_file_uri("rename_local_binding", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let edit = analysis
        .rename(&uri, position_of_nth(source, "value", 1, 1), "answer")
        .unwrap();
    let edits = edit.changes.get(&uri).unwrap();

    assert_eq!(edits.len(), 3);
    assert!(edits.iter().all(|edit| edit.new_text == "answer"));
    assert_eq!(edits[0].range.start, position_of_nth(source, "value", 0, 0));
    assert_eq!(edits[1].range.start, position_of_nth(source, "value", 1, 0));
    assert_eq!(edits[2].range.start, position_of_nth(source, "value", 2, 0));
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
fn completion_after_block_statements_includes_prior_bindings() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn main(flag: i32) void {\n",
        "    {\n",
        "        let first = flag;\n",
        "        let second = first;\n",
        "        \n",
        "    }\n",
        "}\n",
    );
    let uri = temp_file_uri("completion_block_tail_bindings", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let items = analysis
        .completion(
            &uri,
            Position {
                line: 4,
                character: 8,
            },
        )
        .unwrap();
    let labels = completion_labels(&items);

    assert!(labels.contains(&"flag".to_string()));
    assert!(labels.contains(&"first".to_string()));
    assert!(labels.contains(&"second".to_string()));
}

#[test]
fn completion_in_for_body_includes_init_bindings() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn main(limit: i32) void {\n",
        "    for (let index = 0; index < limit; index += 1) {\n",
        "        \n",
        "    }\n",
        "}\n",
    );
    let uri = temp_file_uri("completion_for_body_bindings", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let items = analysis
        .completion(
            &uri,
            Position {
                line: 2,
                character: 8,
            },
        )
        .unwrap();
    let labels = completion_labels(&items);

    assert!(labels.contains(&"limit".to_string()));
    assert!(labels.contains(&"index".to_string()));
}

#[test]
fn completion_in_match_arm_body_includes_pattern_bindings() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Result = enum { Ok: i32, Err };\n",
        "fn main(value: Result) void {\n",
        "    match (value) {\n",
        "        .{ Ok: payload } => {\n",
        "            \n",
        "        },\n",
        "        .Err => {},\n",
        "    };\n",
        "}\n",
    );
    let uri = temp_file_uri("completion_match_arm_bindings", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let items = analysis
        .completion(
            &uri,
            Position {
                line: 4,
                character: 12,
            },
        )
        .unwrap();
    let labels = completion_labels(&items);

    assert!(labels.contains(&"value".to_string()));
    assert!(labels.contains(&"payload".to_string()));
}

#[test]
fn completion_in_closure_body_includes_capture_and_param_bindings() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn main(seed: i32) void {\n",
        "    let visit = .[seed](value: i32) bool {\n",
        "        \n",
        "        return true;\n",
        "    };\n",
        "    let _ = visit;\n",
        "}\n",
    );
    let uri = temp_file_uri("completion_closure_bindings", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let items = analysis
        .completion(
            &uri,
            Position {
                line: 2,
                character: 8,
            },
        )
        .unwrap();
    let labels = completion_labels(&items);

    assert!(labels.contains(&"seed".to_string()));
    assert!(labels.contains(&"value".to_string()));
}

#[test]
fn completion_in_if_branches_includes_outer_bindings() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn main(seed: i32) void {\n",
        "    let value = seed;\n",
        "    if (seed > 0) {\n",
        "        \n",
        "    } else {\n",
        "        \n",
        "    }\n",
        "}\n",
    );
    let uri = temp_file_uri("completion_if_branch_bindings", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let then_items = analysis
        .completion(
            &uri,
            Position {
                line: 3,
                character: 8,
            },
        )
        .unwrap();
    let then_labels = completion_labels(&then_items);
    assert!(then_labels.contains(&"seed".to_string()));
    assert!(then_labels.contains(&"value".to_string()));

    let else_items = analysis
        .completion(
            &uri,
            Position {
                line: 5,
                character: 8,
            },
        )
        .unwrap();
    let else_labels = completion_labels(&else_items);
    assert!(else_labels.contains(&"seed".to_string()));
    assert!(else_labels.contains(&"value".to_string()));
}

#[test]
fn completion_in_function_signature_uses_surface_cache_without_parse_cache() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Helper = struct {};\n",
        "fn make() Helper {\n",
        "    return Helper.{};\n",
        "}\n",
    );
    let uri = temp_file_uri("completion_signature_cache", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    analysis.parse_cache.borrow_mut().clear();
    analysis.surface_cache.borrow_mut().clear();
    analysis.structure_cache.borrow_mut().clear();
    analysis.artifact_cache.borrow_mut().clear();

    let items = analysis
        .completion(&uri, position_of_nth(source, "Helper", 1, 3))
        .unwrap();
    let labels = completion_labels(&items);

    assert!(labels.contains(&"Helper".to_string()));
    assert_eq!(analysis.parse_cache.borrow().len(), 0);
    assert_eq!(analysis.surface_cache.borrow().len(), 1);
    assert!(analysis.structure_cache.borrow().is_empty());
    assert!(analysis.artifact_cache.borrow().is_empty());
}

#[test]
fn completion_in_function_body_still_uses_full_artifact_cache() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper() i32 { return 1; }\n",
        "fn main() i32 {\n",
        "    return hel;\n",
        "}\n",
    );
    let uri = temp_file_uri("completion_body_cache", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    analysis.parse_cache.borrow_mut().clear();
    analysis.surface_cache.borrow_mut().clear();
    analysis.structure_cache.borrow_mut().clear();
    analysis.artifact_cache.borrow_mut().clear();

    let items = analysis
        .completion(&uri, position_of_nth(source, "hel", 1, 3))
        .unwrap();
    let labels = completion_labels(&items);

    assert!(labels.contains(&"helper".to_string()));
    assert_eq!(analysis.parse_cache.borrow().len(), 0);
    assert_eq!(analysis.surface_cache.borrow().len(), 1);
    assert_eq!(analysis.structure_cache.borrow().len(), 1);
    assert_eq!(analysis.artifact_cache.borrow().len(), 1);
}

#[test]
fn semantic_tokens_cache_reuses_rendered_tokens_for_stable_document() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() i32 {\n    let mut value = 1;\n    return value;\n}\n";
    let uri = temp_file_uri("semantic_tokens_cache", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let first = analysis.semantic_tokens(&uri).unwrap();
    assert_eq!(analysis.semantic_tokens_cache.borrow().len(), 1);

    analysis.artifact_cache.borrow_mut().clear();
    let second = analysis.semantic_tokens(&uri).unwrap();

    assert_eq!(first.data, second.data);
    assert!(analysis.artifact_cache.borrow().is_empty());
    assert_eq!(analysis.semantic_tokens_cache.borrow().len(), 1);
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
fn completion_includes_keyword_suggestions_for_prefixes() {
    let mut analysis = AnalysisEngine::default();
    let source = "extern fn main(args: [][]u8) i32 {\n    le\n    return 0;\n}\n";
    let uri = temp_file_uri("completion_keywords", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let items = analysis
        .completion(&uri, position_of_nth(source, "le", 0, 2))
        .unwrap();
    let labels = completion_labels(&items);
    let let_item = items.iter().find(|item| item.label == "let").unwrap();

    assert!(labels.contains(&"let".to_string()));
    assert_eq!(
        let_item.insert_text.as_deref(),
        Some("let ${1:name} = ${0};")
    );
    assert_eq!(let_item.insert_text_format, Some(2));
}

#[test]
fn completion_includes_top_level_keyword_suggestions() {
    let mut analysis = AnalysisEngine::default();
    let source = "ex\n";
    let uri = temp_file_uri("completion_top_level_keywords", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let items = analysis
        .completion(&uri, position_of_nth(source, "ex", 0, 2))
        .unwrap();
    let labels = completion_labels(&items);
    let extern_item = items.iter().find(|item| item.label == "extern").unwrap();

    assert!(labels.contains(&"extern".to_string()));
    assert_eq!(
        extern_item.insert_text.as_deref(),
        Some("extern fn ${1:name}(${2:args}) ${3:i32} {\n    $0\n}")
    );
    assert_eq!(extern_item.insert_text_format, Some(2));
}

#[test]
fn completion_includes_top_level_type_keyword_snippet() {
    let mut analysis = AnalysisEngine::default();
    let source = "ty\n";
    let uri = temp_file_uri("completion_top_level_type_keyword", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let items = analysis
        .completion(&uri, position_of_nth(source, "ty", 0, 2))
        .unwrap();
    let labels = completion_labels(&items);
    let type_item = items.iter().find(|item| item.label == "type").unwrap();

    assert!(labels.contains(&"type".to_string()));
    assert_eq!(
        type_item.insert_text.as_deref(),
        Some("type ${1:Name} = ${0};")
    );
    assert_eq!(type_item.insert_text_format, Some(2));
}

#[test]
fn completion_in_type_context_includes_struct_keyword_snippet() {
    let mut analysis = AnalysisEngine::default();
    let source = "type Packet = st\n";
    let uri = temp_file_uri("completion_type_context_struct_keyword", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let items = analysis
        .completion(&uri, position_of_nth(source, "st", 0, 2))
        .unwrap();
    let labels = completion_labels(&items);
    let struct_item = items.iter().find(|item| item.label == "struct").unwrap();

    assert!(labels.contains(&"struct".to_string()));
    assert_eq!(
        struct_item.insert_text.as_deref(),
        Some("struct {\n    $0\n}")
    );
    assert_eq!(struct_item.insert_text_format, Some(2));
}

#[test]
fn completion_does_not_offer_keywords_after_member_access() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Console = struct { len: i32 };\n",
        "fn main() i32 {\n",
        "    let console = Console.{ len: i32.{1} };\n",
        "    return console.le;\n",
        "}\n",
    );
    let uri = temp_file_uri("completion_member_keywords", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let items = analysis
        .completion(&uri, position_of_nth(source, "console.le", 0, 10))
        .unwrap();
    let labels = completion_labels(&items);

    assert!(!labels.contains(&"let".to_string()));
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

#[test]
fn resolve_analysis_uses_craft_sdk_for_package_script_roots() {
    let root = unique_temp_dir("analysis_craft_script");
    let app_dir = root.join("app");
    fs::create_dir_all(app_dir.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        "[workspace]\nmembers = [\"app\"]\n",
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
",
    )
    .unwrap();
    fs::write(app_dir.join("src/lib.rn"), "pub fn helper() void {}\n").unwrap();
    fs::write(
        app_dir.join("craft.rn"),
        "use craft.plan;\npub fn craft(p: *mut plan.Plan) void { let _ = p; }\n",
    )
    .unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&app_dir.join("craft.rn")).unwrap();
    let source = fs::read_to_string(app_dir.join("craft.rn")).unwrap();

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
        super::normalize_path(&app_dir.join("craft.rn"))
    );
    assert!(
        resolved
            .compile_options
            .module_aliases
            .contains_key("craft")
    );
}

#[test]
fn bin_only_package_with_std_import_has_no_lsp_diagnostics() {
    let root = unique_temp_dir("analysis_bin_only_std");
    fs::create_dir_all(root.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        "\
[package]
name = \"my_app\"
version = \"0.1.0\"
kern = \"0.7\"

[[bin]]
name = \"my_app\"
root = \"src/main.rn\"
",
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        "use std.io;\n\nextern fn main(args: [][]u8) i32 {\n    io.println(\"Hello Kern!\", .{});\n    return 0;\n}\n",
    )
    .unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&root.join("src/main.rn")).unwrap();
    let source = fs::read_to_string(root.join("src/main.rn")).unwrap();

    let outcome = analysis.open_document(DidOpenTextDocumentParams {
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
        super::normalize_path(&root.join("src/main.rn"))
    );
    assert!(resolved.compile_options.module_aliases.contains_key("std"));

    let target_bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    assert!(
        target_bundle.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        target_bundle.diagnostics
    );
}

#[test]
fn resolve_analysis_applies_craft_cfg_and_define_values() {
    let root = unique_temp_dir("analysis_craft_cfg_define");
    fs::create_dir_all(root.join("src")).unwrap();
    let env_name = format!(
        "KERN_LSP_ANALYSIS_{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    fs::write(
        root.join("Craft.toml"),
        format!(
            "\
[package]
name = \"my_app\"
version = \"0.1.0\"
kern = \"0.7\"

[features]
experimental = []

[craft]
env = [\"{env_name}\"]

[[bin]]
name = \"my_app\"
root = \"src/main.rn\"
"
        ),
    )
    .unwrap();
    fs::write(
        root.join("craft.rn"),
        format!(
            "\
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {{
    if (p.feature_enabled(\"experimental\")) {{
        p.cfg_bool(\"enable_telemetry\", true);
        p.define_string(\"GREETING_MSG\", \"Hello from the experimental future!\");
    }}

    if (p.env(\"{env_name}\") != .None) {{
        p.cfg_bool(\"is_dev_env\", true);
    }}
}}
"
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        "\
use std.io;

#[if(enable_telemetry)]
fn init_telemetry() void {
    io.println(\"[Telemetry] Enabled\", .{});
}

#[if(is_dev_env)]
fn print_env_mode() void {
    io.println(\"[Mode] Development\", .{});
}

extern fn main(args: [][]u8) i32 {
    let _ = args;
    print_env_mode();
    init_telemetry();
    let _ = GREETING_MSG;
    return 0;
}
",
    )
    .unwrap();

    let mut options = CompileOptions {
        use_std: true,
        ..CompileOptions::default()
    };
    options.craft_features.push("experimental".to_string());
    let mut analysis = AnalysisEngine::new(AnalysisSettings {
        compile_options: options,
    });
    let uri = file_path_to_uri(&root.join("src/main.rn")).unwrap();
    let source = fs::read_to_string(root.join("src/main.rn")).unwrap();

    let outcome = with_env_var(&env_name, "1", || {
        analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: source,
            },
        })
    });
    let resolved = with_env_var(&env_name, "1", || analysis.resolve_analysis(&uri)).unwrap();

    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("enable_telemetry")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("is_dev_env")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("GREETING_MSG")
            .map(String::as_str),
        Some("Hello from the experimental future!")
    );

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    assert!(
        bundle.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        bundle.diagnostics
    );
}

#[test]
fn resolve_analysis_uses_persisted_craft_analysis_context_by_default() {
    let root = unique_temp_dir("analysis_persisted_craft_ctx");
    fs::create_dir_all(root.join("src")).unwrap();
    let env_name = format!(
        "KERN_LSP_PERSISTED_{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    fs::write(
        root.join("Craft.toml"),
        format!(
            "\
[package]
name = \"my_app\"
version = \"0.1.0\"
kern = \"0.7\"

[features]
experimental = []

[craft]
env = [\"{env_name}\"]

[[bin]]
name = \"my_app\"
root = \"src/main.rn\"
"
        ),
    )
    .unwrap();
    fs::write(
        root.join("craft.rn"),
        format!(
            "\
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {{
    if (p.feature_enabled(\"experimental\")) {{
        p.cfg_bool(\"enable_telemetry\", true);
        p.define_string(\"GREETING_MSG\", \"Hello from the experimental future!\");
    }}

    if (p.env(\"{env_name}\") != .None) {{
        p.cfg_bool(\"is_dev_env\", true);
    }}
}}
"
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        "\
use std.io;

#[if(enable_telemetry)]
fn init_telemetry() void {
    io.println(\"[Telemetry] Enabled\", .{});
}

#[if(!enable_telemetry)]
fn init_telemetry() void {
    io.println(\"[Telemetry] Disabled\", .{});
}

#[if(is_dev_env)]
fn print_env_mode() void {
    io.println(\"[Mode] Development\", .{});
}

#[if(!is_dev_env)]
fn print_env_mode() void {
}

extern fn main(args: [][]u8) i32 {
    let _ = args;
    print_env_mode();
    init_telemetry();
    let _ = GREETING_MSG;
    return 0;
}
",
    )
    .unwrap();

    with_env_var(&env_name, "1", || {
        analysis_context::sync_project_analysis_context(
            &root.join("Craft.toml"),
            true,
            &[String::from("experimental")],
        )
        .unwrap()
    });

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&root.join("src/main.rn")).unwrap();
    let source = fs::read_to_string(root.join("src/main.rn")).unwrap();

    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source,
        },
    });
    let resolved = analysis.resolve_analysis(&uri).unwrap();

    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("enable_telemetry")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("is_dev_env")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("GREETING_MSG")
            .map(String::as_str),
        Some("Hello from the experimental future!")
    );

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    assert!(
        bundle.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        bundle.diagnostics
    );
}

#[test]
fn resolve_analysis_matches_generated_source_root_from_persisted_context() {
    let root = unique_temp_dir("analysis_persisted_generated_root");

    fs::write(
        root.join("Craft.toml"),
        "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.7\"

[[bin]]
name = \"app\"
root = \"src/placeholder.rn\"
",
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        "\
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    let generated = b.emit_generated(
        \"src/main.rn\",
        \"#[if(generated)]\\nextern fn main(args: [][]u8) i32 { let _ = args; let _ = ENTRY_KIND; return 0; }\\n\"
    );
    b.set_source_root(generated);
    b.cfg_bool(\"generated\", true);
    b.define_string(\"ENTRY_KIND\", \"generated\");
}
",
    )
    .unwrap();

    analysis_context::sync_project_analysis_context(&root.join("Craft.toml"), true, &[]).unwrap();

    let generated_path = root
        .join(".craft")
        .join("build")
        .join("dev")
        .join("target")
        .join("gen")
        .join("app-0.1.0")
        .join("bin")
        .join("app")
        .join("src")
        .join("main.rn");
    let project =
        craft::project::AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
    let direct = project.resolve_for_file(&generated_path, &CompileOptions::default());
    assert_eq!(
        direct
            .compile_options
            .custom_defines
            .get("generated")
            .map(String::as_str),
        Some("true")
    );
    let uri = file_path_to_uri(&generated_path).unwrap();
    let source = "#[if(generated)]\nextern fn main(args: [][]u8) i32 { let _ = args; let _ = ENTRY_KIND; return 0; }\n";

    let mut analysis = AnalysisEngine::default();
    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });
    let resolved = analysis.resolve_analysis(&uri).unwrap();

    assert_eq!(
        super::normalize_path(&resolved.input_file),
        super::normalize_path(&generated_path)
    );
    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("generated")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("ENTRY_KIND")
            .map(String::as_str),
        Some("generated")
    );

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    assert!(
        bundle.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        bundle.diagnostics
    );
}

#[test]
fn resolve_analysis_maps_copied_template_sources_into_generated_root() {
    let root = unique_temp_dir("analysis_persisted_generated_alias");
    fs::create_dir_all(root.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.7\"

[[bin]]
name = \"app\"
root = \"src/placeholder.rn\"
",
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        "\
mod build_info;

#[if(generated)]
extern fn main(args: [][]u8) i32 {
    let _ = args;
    let _ = build_info.MAGIC_NUMBER;
    return 0;
}
",
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        "\
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    let main = b.stage_copy_package_file(\"src/main.rn\", \"src/main.rn\");
    let _ = b.stage_generated(
        \"src/build_info.rn\",
        \"pub const MAGIC_NUMBER = i32.{42};\\n\"
    );
    b.set_source_root_from(main);
    b.cfg_bool(\"generated\", true);
}
",
    )
    .unwrap();

    analysis_context::sync_project_analysis_context(&root.join("Craft.toml"), true, &[]).unwrap();

    let generated_main = root
        .join(".craft")
        .join("build")
        .join("dev")
        .join("target")
        .join("gen")
        .join("app-0.1.0")
        .join("bin")
        .join("app")
        .join("src")
        .join("main.rn");
    assert!(generated_main.is_file());
    assert!(
        generated_main
            .parent()
            .unwrap()
            .join("build_info.rn")
            .is_file()
    );

    let uri = file_path_to_uri(&root.join("src/main.rn")).unwrap();
    let source = fs::read_to_string(root.join("src/main.rn")).unwrap();

    let mut analysis = AnalysisEngine::default();
    let outcome = analysis.open_document(DidOpenTextDocumentParams {
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
        super::normalize_path(&generated_main)
    );
    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("generated")
            .map(String::as_str),
        Some("true")
    );

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    assert!(
        bundle.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        bundle.diagnostics
    );
}

#[test]
fn resolve_analysis_prefers_nearest_init_module_root_without_manifest() {
    let root = unique_temp_dir("analysis_init_root");
    let dbg_dir = root.join("dbg");
    fs::create_dir_all(&dbg_dir).unwrap();

    fs::write(
        dbg_dir.join("init.rn"),
        "mod option;\nmod result;\npub use .option.Option;\npub use .result.Result;\n",
    )
    .unwrap();
    fs::write(
        dbg_dir.join("option.rn"),
        "pub type Option[T] = enum { Some: T, None };\n",
    )
    .unwrap();
    fs::write(
        dbg_dir.join("result.rn"),
        "use ..Option;\npub type Result[T] = enum { Ok: T, Err: Option[T] };\n",
    )
    .unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&dbg_dir.join("result.rn")).unwrap();
    let source = fs::read_to_string(dbg_dir.join("result.rn")).unwrap();

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
        super::normalize_path(&dbg_dir.join("init.rn"))
    );
}

#[test]
fn standalone_submodule_analysis_does_not_treat_parent_import_as_root_error() {
    let root = unique_temp_dir("analysis_parent_import");
    let dbg_dir = root.join("dbg");
    fs::create_dir_all(&dbg_dir).unwrap();

    fs::write(
        dbg_dir.join("init.rn"),
        "mod option;\nmod result;\npub use .option.Option;\npub use .result.Result;\n",
    )
    .unwrap();
    fs::write(
        dbg_dir.join("option.rn"),
        "pub type Option[T] = enum { Some: T, None };\n",
    )
    .unwrap();
    let result_source = "use ..Option;\npub type Result[T] = enum { Ok: T, Err: Option[T] };\n";
    fs::write(dbg_dir.join("result.rn"), result_source).unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&dbg_dir.join("result.rn")).unwrap();
    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: result_source.to_string(),
        },
    });

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    assert!(bundle.diagnostics.iter().all(|diagnostic| {
        !diagnostic
            .message
            .contains("Cannot use `..` (Parent) from the root module")
    }));
}

#[test]
fn analysis_cache_reuses_shared_module_root_between_requests() {
    let root = unique_temp_dir("analysis_cache_shared_root");
    let dbg_dir = root.join("dbg");
    fs::create_dir_all(&dbg_dir).unwrap();

    fs::write(
        dbg_dir.join("init.rn"),
        "mod option;\nmod result;\npub use .option.Option;\npub use .result.Result;\n",
    )
    .unwrap();
    let option_source = "pub type Option[T] = enum { Some: T, None };\n";
    fs::write(dbg_dir.join("option.rn"), option_source).unwrap();
    let result_source = "use ..Option;\npub type Result[T] = enum { Ok: T, Err: Option[T] };\n";
    fs::write(dbg_dir.join("result.rn"), result_source).unwrap();

    let mut analysis = AnalysisEngine::default();
    let result_uri = file_path_to_uri(&dbg_dir.join("result.rn")).unwrap();
    let option_uri = file_path_to_uri(&dbg_dir.join("option.rn")).unwrap();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: result_uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: result_source.to_string(),
        },
    });
    assert_eq!(analysis.structure_cache.borrow().len(), 1);
    assert_eq!(analysis.artifact_cache.borrow().len(), 1);

    let _ = analysis.semantic_tokens(&result_uri).unwrap();
    assert_eq!(analysis.structure_cache.borrow().len(), 1);
    assert_eq!(analysis.artifact_cache.borrow().len(), 1);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: option_uri,
            _language_id: "kern".to_string(),
            version: 1,
            text: option_source.to_string(),
        },
    });
    assert_eq!(analysis.structure_cache.borrow().len(), 1);
    assert_eq!(analysis.artifact_cache.borrow().len(), 1);

    let _ = analysis.semantic_tokens(&result_uri).unwrap();
    assert_eq!(analysis.structure_cache.borrow().len(), 1);
    assert_eq!(analysis.artifact_cache.borrow().len(), 1);
}

#[test]
fn source_overrides_only_include_dirty_documents() {
    let uri = temp_file_uri("analysis_dirty_overrides", "fn main() void {}\n");
    let path = uri_to_file_path(&uri).unwrap();
    let source = fs::read_to_string(&path).unwrap();
    let mut analysis = AnalysisEngine::default();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.clone(),
        },
    });
    assert!(analysis.source_overrides().is_empty());

    let _ = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position {
                    line: 0,
                    character: 15,
                },
                end: Position {
                    line: 0,
                    character: 15,
                },
            }),
            text: "\nfn helper() void {}".to_string(),
        }],
    });
    assert_eq!(analysis.source_overrides().len(), 1);

    let _ = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 3,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: source,
        }],
    });
    assert!(analysis.source_overrides().is_empty());
}

#[test]
fn dirty_document_snapshot_reuses_cached_overrides_until_documents_change() {
    let uri = temp_file_uri("analysis_dirty_snapshot_cache", "fn main() void {}\n");
    let path = uri_to_file_path(&uri).unwrap();
    let source = fs::read_to_string(&path).unwrap();
    let mut analysis = AnalysisEngine::default();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source,
        },
    });

    let clean_snapshot = analysis.dirty_documents_snapshot();
    let clean_snapshot_again = analysis.dirty_documents_snapshot();
    assert!(std::rc::Rc::ptr_eq(&clean_snapshot, &clean_snapshot_again));
    assert!(clean_snapshot.is_clean());

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

    let dirty_snapshot = analysis.dirty_documents_snapshot();
    let dirty_snapshot_again = analysis.dirty_documents_snapshot();
    assert!(!std::rc::Rc::ptr_eq(&clean_snapshot, &dirty_snapshot));
    assert!(std::rc::Rc::ptr_eq(&dirty_snapshot, &dirty_snapshot_again));
    assert_eq!(dirty_snapshot.len(), 1);
}

#[test]
fn opening_clean_sibling_document_keeps_cached_artifact() {
    let root = unique_temp_dir("analysis_cache_clean_open");
    let dbg_dir = root.join("dbg");
    fs::create_dir_all(&dbg_dir).unwrap();

    fs::write(
        dbg_dir.join("init.rn"),
        "mod option;\nmod result;\npub use .option.Option;\npub use .result.Result;\n",
    )
    .unwrap();
    let option_source = "pub type Option[T] = enum { Some: T, None };\n";
    fs::write(dbg_dir.join("option.rn"), option_source).unwrap();
    let result_source = "use ..Option;\npub type Result[T] = enum { Ok: T, Err: Option[T] };\n";
    fs::write(dbg_dir.join("result.rn"), result_source).unwrap();

    let mut analysis = AnalysisEngine::default();
    let result_uri = file_path_to_uri(&dbg_dir.join("result.rn")).unwrap();
    let option_uri = file_path_to_uri(&dbg_dir.join("option.rn")).unwrap();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: result_uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: result_source.to_string(),
        },
    });

    let resolved = analysis.resolve_analysis(&result_uri).unwrap();
    let cache_key = super::AnalysisCacheKey::from_resolved(&resolved, &analysis.source_overrides());
    let cached_before = analysis
        .artifact_cache
        .borrow()
        .get(&cache_key)
        .cloned()
        .unwrap();
    let structure_before = analysis
        .structure_cache
        .borrow()
        .get(&cache_key)
        .cloned()
        .unwrap();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: option_uri,
            _language_id: "kern".to_string(),
            version: 1,
            text: option_source.to_string(),
        },
    });

    let cached_after = analysis
        .artifact_cache
        .borrow()
        .get(&cache_key)
        .cloned()
        .unwrap();
    let structure_after = analysis
        .structure_cache
        .borrow()
        .get(&cache_key)
        .cloned()
        .unwrap();
    assert!(std::rc::Rc::ptr_eq(&cached_before, &cached_after));
    assert!(std::rc::Rc::ptr_eq(&structure_before, &structure_after));
}

#[test]
fn reverting_dirty_document_reuses_clean_caches() {
    let uri = temp_file_uri(
        "analysis_cache_revert_clean",
        "fn main() i32 { return 1; }\n",
    );
    let path = uri_to_file_path(&uri).unwrap();
    let original = fs::read_to_string(&path).unwrap();
    let mut analysis = AnalysisEngine::default();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: original.clone(),
        },
    });

    let resolved = analysis.resolve_analysis(&uri).unwrap();
    let clean_key = super::AnalysisCacheKey::from_resolved(&resolved, &analysis.source_overrides());
    let clean_artifact = analysis
        .artifact_cache
        .borrow()
        .get(&clean_key)
        .cloned()
        .unwrap();
    let clean_structure = analysis
        .structure_cache
        .borrow()
        .get(&clean_key)
        .cloned()
        .unwrap();

    let dirty_source = "fn main() i32 {\n    let value = 41;\n    return value + 1;\n}\n";
    let _ = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: dirty_source.to_string(),
        }],
    });

    assert!(analysis.artifact_cache.borrow().contains_key(&clean_key));
    assert!(analysis.structure_cache.borrow().contains_key(&clean_key));

    let _ = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 3,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: original,
        }],
    });

    let reused_artifact = analysis
        .artifact_cache
        .borrow()
        .get(&clean_key)
        .cloned()
        .unwrap();
    let reused_structure = analysis
        .structure_cache
        .borrow()
        .get(&clean_key)
        .cloned()
        .unwrap();

    assert!(std::rc::Rc::ptr_eq(&clean_artifact, &reused_artifact));
    assert!(std::rc::Rc::ptr_eq(&clean_structure, &reused_structure));
}

#[test]
fn body_only_dirty_diagnostics_reuse_clean_structure_cache() {
    let uri = temp_file_uri(
        "analysis_dirty_body_reuse",
        "fn main() i32 { return i32.{1}; }\n",
    );
    let mut analysis = AnalysisEngine::default();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: "fn main() i32 { return i32.{1}; }\n".to_string(),
        },
    });

    let resolved = analysis.resolve_analysis(&uri).unwrap();
    let clean_key = super::AnalysisCacheKey::from_resolved(&resolved, &analysis.source_overrides());
    let clean_structure = analysis
        .structure_cache
        .borrow()
        .get(&clean_key)
        .cloned()
        .unwrap();

    analysis.parse_cache.borrow_mut().clear();
    let dirty_text = "fn main() i32 {\n    let value = i32.{41};\n    return value + i32.{1};\n}\n";
    let doc = analysis.documents.get_mut(&uri).unwrap();
    doc.text = dirty_text.to_string();
    doc.version = 2;
    doc.is_dirty = true;
    doc.text_hash = super::hash_source_text(&doc.text);
    analysis.invalidate_dirty_document_snapshot();

    let fast_report = analysis.analyze_dirty_report(&uri).unwrap();
    assert!(fast_report.is_some());

    let outcome = analysis.analyze_document(&uri);

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    assert!(bundle.diagnostics.is_empty());
    assert_eq!(analysis.structure_cache.borrow().len(), 1);
    assert_eq!(analysis.artifact_cache.borrow().len(), 1);
    assert_eq!(analysis.parse_cache.borrow().len(), 1);

    let reused_structure = analysis
        .structure_cache
        .borrow()
        .get(&clean_key)
        .cloned()
        .unwrap();
    assert!(std::rc::Rc::ptr_eq(&clean_structure, &reused_structure));
}

#[test]
fn structural_dirty_edit_falls_back_to_dirty_structure_analysis() {
    let uri = temp_file_uri(
        "analysis_dirty_structure_fallback",
        "fn main() i32 { return i32.{1}; }\n",
    );
    let mut analysis = AnalysisEngine::default();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: "fn main() i32 { return i32.{1}; }\n".to_string(),
        },
    });

    let outcome = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: "fn main() i32 { return i32.{1}; }\nfn helper() i32 { return i32.{2}; }\n"
                .to_string(),
        }],
    });

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    assert!(bundle.diagnostics.is_empty());
    assert_eq!(analysis.structure_cache.borrow().len(), 2);
    assert_eq!(analysis.artifact_cache.borrow().len(), 2);
}

#[test]
fn function_body_fast_path_preserves_clean_sibling_diagnostics() {
    let root = unique_temp_dir("analysis_fast_path_preserve_sibling");
    fs::write(root.join("init.rn"), "mod good;\nmod bad;\n").unwrap();
    fs::write(root.join("good.rn"), "fn main() i32 { return i32.{1}; }\n").unwrap();
    fs::write(root.join("bad.rn"), "fn broken() i32 { return missing; }\n").unwrap();

    let mut analysis = AnalysisEngine::default();
    let good_uri = file_path_to_uri(&root.join("good.rn")).unwrap();
    let bad_uri = file_path_to_uri(&root.join("bad.rn")).unwrap();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: bad_uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: fs::read_to_string(root.join("bad.rn")).unwrap(),
        },
    });
    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: good_uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: fs::read_to_string(root.join("good.rn")).unwrap(),
        },
    });

    let outcome = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: good_uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: "fn main() i32 {\n    let value = i32.{41};\n    return value + i32.{1};\n}\n"
                .to_string(),
        }],
    });

    let good_bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == good_uri)
        .unwrap();
    let bad_bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == bad_uri)
        .unwrap();

    assert!(good_bundle.diagnostics.is_empty());
    assert!(
        bad_bundle
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("undeclared identifier"))
    );
}

#[test]
fn function_body_fast_path_preserves_clean_target_diagnostics_outside_changed_owner() {
    let original = "fn main() i32 { return i32.{1}; }\nfn helper() i32 { return missing; }\n";
    let dirty = concat!(
        "fn main() i32 {\n",
        "    let value = i32.{41};\n",
        "    return value + i32.{1};\n",
        "}\n",
        "fn helper() i32 { return missing; }\n",
    );
    let uri = temp_file_uri("analysis_fast_path_preserve_target", original);
    let mut analysis = AnalysisEngine::default();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: original.to_string(),
        },
    });

    let outcome = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: dirty.to_string(),
        }],
    });

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    let diagnostic = bundle
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("undeclared identifier"))
        .unwrap();

    assert_eq!(
        diagnostic.range.start,
        position_of_nth(dirty, "missing", 0, 0)
    );
    assert_eq!(analysis.structure_cache.borrow().len(), 1);
    assert_eq!(analysis.artifact_cache.borrow().len(), 1);
}

#[test]
fn function_body_fast_path_replaces_overlapping_clean_target_diagnostics() {
    let original = "fn main() i32 { return missing; }\n";
    let dirty = concat!(
        "fn main() i32 {\n",
        "    let value = i32.{41};\n",
        "    return value + i32.{1};\n",
        "}\n",
    );
    let uri = temp_file_uri("analysis_fast_path_drop_target_overlap", original);
    let mut analysis = AnalysisEngine::default();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: original.to_string(),
        },
    });

    let outcome = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: dirty.to_string(),
        }],
    });

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();

    assert!(bundle.diagnostics.is_empty());
    assert_eq!(analysis.structure_cache.borrow().len(), 1);
    assert_eq!(analysis.artifact_cache.borrow().len(), 1);
}

#[test]
fn semantic_tokens_classify_local_let_bindings() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper() i32 { return 1; }\n",
        "fn main() i32 {\n",
        "    let value = helper();\n",
        "    return value;\n",
        "}\n",
    );
    let uri = temp_file_uri("semantic_tokens_local_let", source);

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
        position_of_nth(source, "value", 0, 0),
        SemanticTokenTypes::VARIABLE,
        SemanticModifiers::DECLARATION | SemanticModifiers::READONLY,
    );
    assert_token(
        &decoded,
        position_of_nth(source, "value", 1, 0),
        SemanticTokenTypes::VARIABLE,
        SemanticModifiers::READONLY,
    );
}

#[test]
fn semantic_tokens_classify_mutable_bindings_and_params() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn bump(mut value: i32) i32 {\n",
        "    let mut total = value;\n",
        "    return total;\n",
        "}\n",
    );
    let uri = temp_file_uri("semantic_tokens_mut_bindings", source);

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
        position_of_nth(source, "value", 0, 0),
        SemanticTokenTypes::PARAMETER,
        SemanticModifiers::DECLARATION,
    );
    assert_token(
        &decoded,
        position_of_nth(source, "value", 1, 0),
        SemanticTokenTypes::PARAMETER,
        0,
    );
    assert_token(
        &decoded,
        position_of_nth(source, "total", 0, 0),
        SemanticTokenTypes::VARIABLE,
        SemanticModifiers::DECLARATION,
    );
    assert_token(
        &decoded,
        position_of_nth(source, "total", 1, 0),
        SemanticTokenTypes::VARIABLE,
        0,
    );
}

#[test]
fn semantic_tokens_classify_imported_function_references_in_submodules() {
    let root = unique_temp_dir("semantic_tokens_imported_function");
    let dbg_dir = root.join("dbg");
    fs::create_dir_all(&dbg_dir).unwrap();

    fs::write(
        dbg_dir.join("init.rn"),
        "pub mod helper;\nmod use_helper;\n",
    )
    .unwrap();
    fs::write(
        dbg_dir.join("helper.rn"),
        "pub fn helper() i32 { return 1; }\n",
    )
    .unwrap();
    let use_helper_source = "use ..helper.helper;\npub fn run() i32 { return helper(); }\n";
    fs::write(dbg_dir.join("use_helper.rn"), use_helper_source).unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&dbg_dir.join("use_helper.rn")).unwrap();
    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: use_helper_source.to_string(),
        },
    });

    let decoded = decode_semantic_tokens(&analysis.semantic_tokens(&uri).unwrap());

    assert_token(
        &decoded,
        position_of_nth(use_helper_source, "helper", 2, 0),
        SemanticTokenTypes::FUNCTION,
        0,
    );
}

#[test]
fn semantic_tokens_classify_variant_let_else_payload_bindings() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "type Option[T] = enum {\n",
        "    None,\n",
        "    Some: T,\n",
        "};\n",
        "\n",
        "fn unwrap_or_zero(value: Option[i32]) i32 {\n",
        "    let .{ Some: inner } = value else return 0;\n",
        "    return inner;\n",
        "}\n",
    );
    let uri = temp_file_uri("semantic_tokens_let_else_binding", source);

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
        position_of_nth(source, "inner", 0, 0),
        SemanticTokenTypes::VARIABLE,
        SemanticModifiers::DECLARATION | SemanticModifiers::READONLY,
    );
    assert_token(
        &decoded,
        position_of_nth(source, "inner", 1, 0),
        SemanticTokenTypes::VARIABLE,
        SemanticModifiers::READONLY,
    );
}

fn temp_file_uri(prefix: &str, initial_text: &str) -> String {
    let path = unique_temp_file_path(prefix);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, initial_text).unwrap();
    file_path_to_uri(&path).unwrap()
}

fn with_env_var<T>(name: &str, value: &str, f: impl FnOnce() -> T) -> T {
    let previous = std::env::var_os(name);
    // SAFETY: tests use unique variable names and restore the previous value.
    unsafe {
        std::env::set_var(name, value);
    }
    let result = f();
    // SAFETY: restores the process environment to its previous state.
    unsafe {
        if let Some(previous) = previous {
            std::env::set_var(name, previous);
        } else {
            std::env::remove_var(name);
        }
    }
    result
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
