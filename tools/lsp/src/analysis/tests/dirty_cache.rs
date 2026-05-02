use super::*;

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
    assert_eq!(analysis.structure_cache.borrow().len(), 0);
    assert_eq!(analysis.artifact_cache.borrow().len(), 0);

    let _ = analysis
        .hover(&result_uri, position_of_nth(result_source, "Result", 0, 1))
        .unwrap();
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

    let _ = analysis
        .hover(&result_uri, position_of_nth(result_source, "Result", 0, 1))
        .unwrap();
    assert_eq!(analysis.structure_cache.borrow().len(), 1);
    assert!(
        analysis
            .artifact_cache
            .borrow()
            .keys()
            .all(AnalysisCacheKey::is_clean)
    );
}

#[test]
fn dirty_semantic_tokens_use_lexical_fallback_without_full_analysis() {
    let clean = "fn main() void {\n    let value = 1;\n}\n";
    let dirty = "fn main() void {\n    _ = value;\n}\n";
    let root = unique_temp_dir("dirty_semantic_tokens_lexical_project");
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        root.join("Craft.toml"),
        format!(
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "{CURRENT_KERN_VERSION}"

[[bin]]
name = "app"
root = "src/main.rn"
"#
        ),
    )
    .unwrap();
    let path = src.join("main.rn");
    fs::write(&path, clean).unwrap();
    let uri = file_path_to_uri(&path).unwrap();
    let mut analysis = AnalysisEngine::default();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: clean.to_string(),
        },
    });
    let _ = analysis.semantic_tokens(&uri).unwrap();
    assert_eq!(analysis.artifact_cache.borrow().len(), 0);

    let _ = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: dirty.to_string(),
        }],
    });
    analysis.artifact_cache.borrow_mut().clear();

    let tokens = analysis.semantic_tokens(&uri).unwrap();

    assert!(!tokens.data.is_empty());
    assert_eq!(analysis.artifact_cache.borrow().len(), 0);
}

#[test]
fn dirty_interactive_requests_after_complex_error_avoid_full_dirty_analysis() {
    let (clean, dirty) = dirty_complex_sources();
    let uri = open_dirty_complex_document(clean);
    let mut analysis = AnalysisEngine::default();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: clean.to_string(),
        },
    });
    let _ = analysis.semantic_tokens(&uri).unwrap();
    assert_eq!(analysis.artifact_cache.borrow().len(), 0);

    let _ = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: dirty.to_string(),
        }],
    });
    analysis.artifact_cache.borrow_mut().clear();

    let _ = analysis.semantic_tokens(&uri).unwrap();
    let _ = analysis
        .completion(&uri, position_of_nth(dirty, "Shape.Dot", 0, 9))
        .unwrap();
    let _ = analysis
        .hover(&uri, position_of_nth(dirty, "make_point", 1, 1))
        .unwrap();
    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 7,
                    character: 0,
                },
                end: Position {
                    line: 8,
                    character: 16,
                },
            },
        )
        .unwrap();

    assert!(!actions.is_empty());
    assert_eq!(analysis.artifact_cache.borrow().len(), 1);
}

#[test]
fn dirty_complex_change_only_stays_lightweight() {
    let (clean, dirty) = dirty_complex_sources();
    let (_uri, analysis) = dirty_complex_analysis_after_change(clean, dirty);

    assert_eq!(analysis.artifact_cache.borrow().len(), 0);
}

#[test]
fn dirty_complex_open_only_finishes() {
    let (clean, _) = dirty_complex_sources();
    let uri = open_dirty_complex_document(clean);
    let mut analysis = AnalysisEngine::default();

    let action = analysis.open_document_state(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri,
            _language_id: "kern".to_string(),
            version: 1,
            text: clean.to_string(),
        },
    });
    assert!(matches!(action, DocumentSyncAction::ScheduleTarget { .. }));
}

#[test]
fn dirty_complex_clean_semantic_tokens_finish() {
    let (clean, _) = dirty_complex_sources();
    let uri = open_dirty_complex_document(clean);
    let mut analysis = AnalysisEngine::default();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: clean.to_string(),
        },
    });
    let _ = analysis.semantic_tokens(&uri).unwrap();
}

#[test]
fn dirty_complex_change_state_only_finishes() {
    let (clean, dirty) = dirty_complex_sources();
    let uri = open_dirty_complex_document(clean);
    let mut analysis = AnalysisEngine::default();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: clean.to_string(),
        },
    });
    let _ = analysis.semantic_tokens(&uri).unwrap();
    let action = analysis.change_document_state(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: dirty.to_string(),
        }],
    });

    assert!(matches!(
        action,
        DocumentSyncAction::ScheduleTarget {
            mode: DiagnosticsAnalysisMode::Structure,
            ..
        }
    ));
}

#[test]
fn dirty_complex_semantic_tokens_stay_lexical() {
    let (clean, dirty) = dirty_complex_sources();
    let (uri, analysis) = dirty_complex_analysis_after_change(clean, dirty);

    let tokens = analysis.semantic_tokens(&uri).unwrap();

    assert!(!tokens.data.is_empty());
    assert_eq!(analysis.artifact_cache.borrow().len(), 0);
}

#[test]
fn dirty_complex_completion_uses_clean_analysis() {
    let (clean, dirty) = dirty_complex_sources();
    let (uri, analysis) = dirty_complex_analysis_after_change(clean, dirty);

    let _ = analysis
        .completion(&uri, position_of_nth(dirty, "Shape.Dot", 0, 9))
        .unwrap();

    assert_eq!(analysis.artifact_cache.borrow().len(), 1);
}

#[test]
fn dirty_complex_hover_uses_clean_analysis() {
    let (clean, dirty) = dirty_complex_sources();
    let (uri, analysis) = dirty_complex_analysis_after_change(clean, dirty);

    let _ = analysis
        .hover(&uri, position_of_nth(dirty, "make_point", 1, 1))
        .unwrap();

    assert_eq!(analysis.artifact_cache.borrow().len(), 1);
}

#[test]
fn dirty_complex_code_actions_use_lightweight_diagnostics() {
    let (clean, dirty) = dirty_complex_sources();
    let (uri, analysis) = dirty_complex_analysis_after_change(clean, dirty);

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 7,
                    character: 0,
                },
                end: Position {
                    line: 8,
                    character: 16,
                },
            },
        )
        .unwrap();

    assert!(!actions.is_empty());
    assert_eq!(analysis.artifact_cache.borrow().len(), 0);
}

fn dirty_complex_sources() -> (&'static str, &'static str) {
    let clean = concat!(
        "type Point = struct { x: i32, y: i32 };\n",
        "type Shape = enum { Dot: Point, Empty };\n",
        "fn make_point(x: i32, y: i32) Point {\n",
        "    return Point.{ x: x, y: y };\n",
        "}\n",
        "fn main(flag: bool) i32 {\n",
        "    let point = make_point(1, 2);\n",
        "    let shape = Shape.Dot(point);\n",
        "    match shape {\n",
        "        Shape.Dot(p) => return p.x;\n",
        "        Shape.Empty => return 0;\n",
        "    }\n",
        "}\n",
    );
    let dirty = concat!(
        "type Point = struct { x: i32, y: i32 };\n",
        "type Shape = enum { Dot: Point, Empty };\n",
        "fn make_point(x: i32, y: i32) Point {\n",
        "    return Point.{ x: x, y: y };\n",
        "}\n",
        "fn main(flag: bool) i32 {\n",
        "    let point = make_point(1, 2)\n",
        "    let shape = Shape.Dot(point;\n",
        "    match shape {\n",
        "        Shape.Dot(p) => return p.x;\n",
        "        Shape.Empty => return 0;\n",
        "    }\n",
        "}\n",
    );

    (clean, dirty)
}

fn open_dirty_complex_document(clean: &str) -> String {
    temp_file_uri("dirty_complex_interactive_requests", clean)
}

fn dirty_complex_analysis_after_change(
    clean: &'static str,
    dirty: &'static str,
) -> (String, AnalysisEngine) {
    let uri = open_dirty_complex_document(clean);
    let mut analysis = AnalysisEngine::default();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: clean.to_string(),
        },
    });
    let _ = analysis.semantic_tokens(&uri).unwrap();
    assert_eq!(analysis.artifact_cache.borrow().len(), 0);

    let _ = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: dirty.to_string(),
        }],
    });

    (uri, analysis)
}

#[test]
fn unrelated_dirty_package_file_does_not_force_lexical_fallback_for_library() {
    let root = unique_temp_dir("dirty_unrelated_example_project");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("examples")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        format!(
            r#"
[package]
name = "raylike"
version = "0.1.0"
kern = "{CURRENT_KERN_VERSION}"

[lib]
root = "src/lib.rn"
"#
        ),
    )
    .unwrap();
    let lib_source = "pub fn helper() i32 { return 1; }\n";
    let lib_path = root.join("src/lib.rn");
    fs::write(&lib_path, lib_source).unwrap();
    let example_path = root.join("examples/new_window.rn");
    let example_source = "fn main() i32 { return 0; }\n";

    let mut analysis = AnalysisEngine::default();
    let lib_uri = file_path_to_uri(&lib_path).unwrap();
    let example_uri = file_path_to_uri(&example_path).unwrap();
    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: lib_uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: lib_source.to_string(),
        },
    });
    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: example_uri,
            _language_id: "kern".to_string(),
            version: 1,
            text: example_source.to_string(),
        },
    });
    analysis.artifact_cache.borrow_mut().clear();

    let tokens = analysis.semantic_tokens(&lib_uri).unwrap();
    let hover = analysis
        .hover(&lib_uri, position_of_nth(lib_source, "helper", 0, 1))
        .unwrap()
        .unwrap();

    assert!(!tokens.data.is_empty());
    assert_eq!(analysis.artifact_cache.borrow().len(), 1);
    assert!(
        hover.contents.value.contains("fn helper"),
        "{}",
        hover.contents.value
    );
}

#[test]
fn dirty_bin_root_does_not_force_lexical_fallback_for_library_tokens() {
    let root = unique_temp_dir("dirty_bin_root_not_library");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        format!(
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "{CURRENT_KERN_VERSION}"

[lib]
root = "src/lib.rn"

[[bin]]
name = "app"
root = "src/main.rn"
"#
        ),
    )
    .unwrap();
    let lib_source = "pub type Bitmap = struct {};\npub fn make() Bitmap { return Bitmap.{}; }\n";
    let main_source = "fn main() i32 { return 0; }\n";
    let dirty_main_source = "fn main() i32 {\n    return 0;\n}\n";
    let lib_path = root.join("src/lib.rn");
    let main_path = root.join("src/main.rn");
    fs::write(&lib_path, lib_source).unwrap();
    fs::write(&main_path, main_source).unwrap();

    let mut analysis = AnalysisEngine::default();
    let lib_uri = file_path_to_uri(&lib_path).unwrap();
    let main_uri = file_path_to_uri(&main_path).unwrap();
    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: lib_uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: lib_source.to_string(),
        },
    });
    warm_clean_semantic_artifact(&analysis, &lib_uri, lib_source);
    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: main_uri,
            _language_id: "kern".to_string(),
            version: 1,
            text: dirty_main_source.to_string(),
        },
    });

    let decoded = decode_semantic_tokens(&analysis.semantic_tokens(&lib_uri).unwrap());

    assert_token_with_length(
        &decoded,
        position_of_nth(lib_source, "Bitmap", 0, 0),
        6,
        SemanticTokenTypes::STRUCT,
    );
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

    let _ = analysis
        .hover(&result_uri, position_of_nth(result_source, "Result", 0, 1))
        .unwrap();
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

    let _ = analysis
        .hover(&uri, position_of_nth(&original, "main", 0, 1))
        .unwrap();
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

    let _ = analysis
        .hover(
            &uri,
            position_of_nth("fn main() i32 { return i32.{1}; }\n", "main", 0, 1),
        )
        .unwrap();
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
            .all(|diagnostic| diagnostic.source == "kernc")
    );
    assert_eq!(analysis.structure_cache.borrow().len(), 0);
    assert_eq!(analysis.artifact_cache.borrow().len(), 0);
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
    let _ = analysis
        .hover(
            &bad_uri,
            position_of_nth("fn broken() i32 { return missing; }\n", "broken", 0, 1),
        )
        .unwrap();
    let _ = analysis
        .hover(
            &good_uri,
            position_of_nth("fn main() i32 { return i32.{1}; }\n", "main", 0, 1),
        )
        .unwrap();

    let _ = analysis.change_document_state(DidChangeTextDocumentParams {
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
    let outcome = analysis.analyze_document(&good_uri);

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
    let _ = analysis
        .hover(&uri, position_of_nth(original, "main", 0, 1))
        .unwrap();

    let _ = analysis.change_document_state(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier {
            uri: uri.clone(),
            version: 2,
        },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: dirty.to_string(),
        }],
    });
    let outcome = analysis.analyze_document(&uri);

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
    let _ = analysis
        .hover(&uri, position_of_nth(original, "missing", 0, 1))
        .unwrap();

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
