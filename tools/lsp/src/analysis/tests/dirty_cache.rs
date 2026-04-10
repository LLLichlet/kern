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
            .any(|diagnostic| diagnostic.code.as_deref() == Some("unused-private-item"))
    );
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
