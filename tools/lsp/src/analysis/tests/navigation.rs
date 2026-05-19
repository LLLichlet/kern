//! Navigation, hover, references, rename, and symbol analysis tests.

use super::*;

#[test]
fn extracts_document_symbols_from_compiler_artifact() {
    let mut analysis = AnalysisEngine::default();
    let uri = temp_file_uri(
        "document_symbols",
        "struct Point { x: i32, y: i32 }\nfn helper() i32 { return 1; }\n",
    );

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: "struct Point { x: i32, y: i32 }\nfn helper() i32 { return 1; }\n".to_string(),
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
fn document_symbol_container_selection_uses_keyword_span() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Counter {}\n",
        "impl Counter {\n",
        "    fn get() i32 { return 1; }\n",
        "}\n",
        "extern \"C\" {\n",
        "    fn puts(text: &[u8]) i32;\n",
        "}\n",
    );
    let uri = temp_file_uri("document_symbol_container_selection", source);

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
        .find(|symbol| symbol.name.starts_with("impl Counter"))
        .unwrap();
    let extern_symbol = symbols
        .iter()
        .find(|symbol| symbol.name == "extern")
        .unwrap();

    assert_eq!(
        impl_symbol.selection_range.start,
        position_of_nth(source, "impl", 0, 0)
    );
    assert_eq!(
        impl_symbol.selection_range.end,
        position_of_nth(source, "impl", 0, 4)
    );
    assert_eq!(
        extern_symbol.selection_range.start,
        position_of_nth(source, "extern", 0, 0)
    );
    assert_eq!(
        extern_symbol.selection_range.end,
        position_of_nth(source, "extern", 0, 6)
    );
}

#[test]
fn document_symbols_use_clean_surface_when_dirty_body_is_incomplete() {
    let mut analysis = AnalysisEngine::default();
    let clean = concat!(
        "fn helper() void {}\n",
        "fn main() void {\n",
        "    helper();\n",
        "}\n",
    );
    let dirty = concat!(
        "fn helper() void {}\n",
        "fn main() void {\n",
        "    hel\n",
        "}\n",
    );
    let uri = temp_file_uri("document_symbols_dirty_fallback", clean);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: clean.to_string(),
        },
    });
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

    let symbols = analysis.document_symbols(&uri).unwrap();
    let names = symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect::<Vec<_>>();

    assert!(names.contains(&"helper"));
    assert!(names.contains(&"main"));
    assert_eq!(analysis.last_analysis_tier(), Some(AnalysisTier::Surface));
}

#[test]
fn dirty_document_symbols_do_not_create_dirty_surface_cache_entries() {
    let mut analysis = AnalysisEngine::default();
    let clean = concat!(
        "struct Point { x: i32 }\n",
        "fn helper(point: Point) i32 {\n",
        "    return point.x;\n",
        "}\n",
    );
    let dirty = concat!(
        "struct Point { x: i32 }\n",
        "fn helper(point: Point) i32 {\n",
        "    return point.x\n",
        "}\n",
    );
    let uri = temp_file_uri("document_symbols_dirty_clean_surface_only", clean);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: clean.to_string(),
        },
    });
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
    analysis.parse_cache.lock().unwrap().clear();
    analysis.surface_cache.lock().unwrap().clear();
    analysis.structure_cache.lock().unwrap().clear();
    analysis.artifact_cache.lock().unwrap().clear();

    let symbols = analysis.document_symbols(&uri).unwrap();
    let names = symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect::<Vec<_>>();

    assert!(names.contains(&"Point"));
    assert!(names.contains(&"helper"));
    assert_eq!(analysis.last_analysis_tier(), Some(AnalysisTier::Surface));
    assert_eq!(analysis.surface_cache.lock().unwrap().len(), 1);
    assert!(
        analysis
            .surface_cache
            .lock()
            .unwrap()
            .keys()
            .all(AnalysisCacheKey::is_clean)
    );
    assert_eq!(analysis.parse_cache.lock().unwrap().len(), 0);
    assert_eq!(analysis.structure_cache.lock().unwrap().len(), 0);
    assert_eq!(analysis.artifact_cache.lock().unwrap().len(), 0);
    assert_eq!(analysis.cached_document_symbol_index_count(), 1);
}

#[test]
fn document_symbols_use_surface_cache_without_body_artifact() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Point { x: i32 }\n",
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

    analysis.parse_cache.lock().unwrap().clear();
    analysis.surface_cache.lock().unwrap().clear();
    analysis.structure_cache.lock().unwrap().clear();
    analysis.artifact_cache.lock().unwrap().clear();
    assert_eq!(analysis.parse_cache.lock().unwrap().len(), 0);
    assert_eq!(analysis.surface_cache.lock().unwrap().len(), 0);
    assert_eq!(analysis.structure_cache.lock().unwrap().len(), 0);
    assert_eq!(analysis.artifact_cache.lock().unwrap().len(), 0);

    let symbols = analysis.document_symbols(&uri).unwrap();
    let names = symbols
        .iter()
        .map(|symbol| symbol.name.clone())
        .collect::<Vec<_>>();

    assert!(names.contains(&"Point".to_string()));
    assert!(names.contains(&"helper".to_string()));
    assert_eq!(analysis.parse_cache.lock().unwrap().len(), 0);
    assert_eq!(analysis.surface_cache.lock().unwrap().len(), 1);
    assert_eq!(analysis.structure_cache.lock().unwrap().len(), 0);
    assert_eq!(analysis.artifact_cache.lock().unwrap().len(), 0);
    assert_eq!(analysis.cached_document_symbol_index_count(), 1);

    let symbols = analysis.document_symbols(&uri).unwrap();
    let names = symbols
        .iter()
        .map(|symbol| symbol.name.clone())
        .collect::<Vec<_>>();

    assert!(names.contains(&"Point".to_string()));
    assert!(names.contains(&"helper".to_string()));
    assert_eq!(analysis.cached_document_symbol_index_count(), 1);
}

#[test]
fn document_symbols_use_collected_outline_names_for_impl_blocks() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Point { x: i32 }\n",
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
    let source = "fn helper() i32 {\n    let value = 1i32;\n    return value;\n}\n";
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
fn goto_definition_resolves_type_identifier_references() {
    let mut analysis = AnalysisEngine::default();
    let source = "struct Point { x: i32 }\nfn main(point: Point) i32 { return point.x; }\n";
    let uri = temp_file_uri("goto_definition_type", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let definition = analysis
        .goto_definition(&uri, position_of_nth(source, "Point", 1, 0))
        .unwrap()
        .unwrap();

    assert_eq!(definition.uri, uri);
    assert_eq!(
        definition.range.start,
        position_of_nth(source, "Point", 0, 0)
    );
}

#[test]
fn goto_definition_resolves_grouped_reexport_leaf_definitions() {
    let root = unique_temp_dir("goto_definition_grouped_reexport");
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(
        root.join("Craft.toml"),
        format!(
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"{CURRENT_KERN_VERSION}\"

[lib]
root = \"src/lib.kn\"
"
        ),
    )
    .unwrap();
    let lib_source = "pub mod types;\npub use .types.{\n    Answer,\n    Widget,\n};\n";
    let types_source = "pub const Answer = 42i32;\npub type Widget = i32;\n";
    fs::write(src_dir.join("lib.kn"), lib_source).unwrap();
    fs::write(src_dir.join("types.kn"), types_source).unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&src_dir.join("lib.kn")).unwrap();
    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: lib_source.to_string(),
        },
    });

    let answer_definition = analysis
        .goto_definition(&uri, position_of_nth(lib_source, "Answer", 0, 1))
        .unwrap()
        .unwrap();
    assert_eq!(
        normalize_path(&uri_to_file_path(&answer_definition.uri).unwrap()),
        normalize_path(&src_dir.join("types.kn"))
    );
    assert_eq!(
        answer_definition.range.start,
        position_of_nth(types_source, "Answer", 0, 0)
    );

    let widget_definition = analysis
        .goto_definition(&uri, position_of_nth(lib_source, "Widget", 0, 1))
        .unwrap()
        .unwrap();
    assert_eq!(
        normalize_path(&uri_to_file_path(&widget_definition.uri).unwrap()),
        normalize_path(&src_dir.join("types.kn"))
    );
    assert_eq!(
        widget_definition.range.start,
        position_of_nth(types_source, "Widget", 0, 0)
    );
}

#[test]
fn navigation_queries_use_navigation_cache_without_full_artifact() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper(); }\n";
    let uri = temp_file_uri("navigation_light_artifact", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    analysis.parse_cache.lock().unwrap().clear();
    analysis.surface_cache.lock().unwrap().clear();
    analysis.structure_cache.lock().unwrap().clear();
    analysis.artifact_cache.lock().unwrap().clear();
    analysis.navigation_cache.lock().unwrap().clear();

    let definition = analysis
        .goto_definition(&uri, position_of_nth(source, "helper", 1, 1))
        .unwrap()
        .unwrap();

    assert_eq!(
        definition.range.start,
        position_of_nth(source, "helper", 0, 0)
    );
    assert_eq!(
        analysis.last_analysis_tier(),
        Some(AnalysisTier::CleanSemantic)
    );
    assert_eq!(analysis.artifact_cache.lock().unwrap().len(), 0);
    assert_eq!(analysis.navigation_cache.lock().unwrap().len(), 1);

    let _hover = analysis
        .hover(&uri, position_of_nth(source, "helper", 1, 1))
        .unwrap()
        .unwrap();
    assert_eq!(analysis.artifact_cache.lock().unwrap().len(), 0);
    assert_eq!(analysis.navigation_cache.lock().unwrap().len(), 1);
}

#[test]
fn goto_definition_resolves_impl_method_references() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Counter { value: i32 }\n",
        "impl Counter {\n",
        "    fn get() i32 { return self.value; }\n",
        "}\n",
        "fn main() i32 {\n",
        "    let counter = Counter.{ value: 1i32 };\n",
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
        "struct Counter { value: i32 }\n",
        "fn main() i32 {\n",
        "    let counter = Counter.{ value: 1i32 };\n",
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
        "enum Result { Ok: i32, Err }\n",
        "fn main() i32 {\n",
        "    let value = Result.{ Ok: 1i32 };\n",
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
fn references_include_workspace_package_uses() {
    let root = unique_temp_dir("references_workspace_packages");
    let dep_dir = root.join("dep/src");
    let app_dir = root.join("app/src");
    fs::create_dir_all(&dep_dir).unwrap();
    fs::create_dir_all(&app_dir).unwrap();
    fs::write(
        root.join("Craft.toml"),
        "[workspace]\nname = \"workspace\"\nmembers = [\"dep\", \"app\"]\n",
    )
    .unwrap();
    fs::write(
        root.join("dep/Craft.toml"),
        format!(
            "\
[package]
name = \"dep\"
version = \"0.1.0\"
kern = \"{CURRENT_KERN_VERSION}\"

[lib]
root = \"src/lib.kn\"
"
        ),
    )
    .unwrap();
    let dep_source = "pub fn helper() i32 { return 1; }\n";
    fs::write(dep_dir.join("lib.kn"), dep_source).unwrap();
    fs::write(
        root.join("app/Craft.toml"),
        format!(
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"{CURRENT_KERN_VERSION}\"

[lib]
root = \"src/lib.kn\"

[dependencies]
dep = {{ path = \"../dep\" }}
"
        ),
    )
    .unwrap();
    let app_source = "use dep.helper;\npub fn run() i32 { return helper(); }\n";
    fs::write(app_dir.join("lib.kn"), app_source).unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&dep_dir.join("lib.kn")).unwrap();
    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: dep_source.to_string(),
        },
    });

    let references = analysis
        .references(&uri, position_of_nth(dep_source, "helper", 0, 1), true)
        .unwrap();

    assert_eq!(references.len(), 3, "{references:#?}");
    assert!(
        references.iter().any(|location| location.uri == uri
            && location.range.start == position_of_nth(dep_source, "helper", 0, 0)),
        "{references:#?}"
    );
    let app_references = references
        .iter()
        .filter(|location| location.uri.ends_with("/app/src/lib.kn"))
        .collect::<Vec<_>>();
    assert_eq!(app_references.len(), 2, "{references:#?}");
    assert!(
        app_references
            .iter()
            .any(|location| location.range.start == position_of_nth(app_source, "helper", 0, 0)),
        "{references:#?}"
    );
    assert!(
        app_references
            .iter()
            .any(|location| location.range.start == position_of_nth(app_source, "helper", 1, 0)),
        "{references:#?}"
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
    assert!(
        highlights
            .iter()
            .all(|highlight| highlight.kind == Some(IdeDocumentHighlightKind::Text))
    );
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

    assert!(hover.contents.contains("fn helper(x: i32) i32"));
    let range = hover.range.unwrap();
    assert_eq!(range.start, position_of_nth(source, "helper", 1, 0));
    assert_eq!(range.end, position_of_nth(source, "helper", 1, 6));
}

#[test]
fn hover_uses_token_artifact_without_navigation_or_full_analysis() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn helper(x: i32) i32 { return x; }\nfn main() i32 { return helper(1); }\n";
    let uri = temp_file_uri("hover_classification_artifact", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });
    analysis.parse_cache.lock().unwrap().clear();
    analysis.surface_cache.lock().unwrap().clear();
    analysis.structure_cache.lock().unwrap().clear();
    analysis.navigation_cache.lock().unwrap().clear();
    analysis
        .semantic_classification_cache
        .lock()
        .unwrap()
        .clear();
    analysis
        .semantic_token_classification_cache
        .lock()
        .unwrap()
        .clear();
    analysis.artifact_cache.lock().unwrap().clear();

    let hover = analysis
        .hover(&uri, position_of_nth(source, "helper", 1, 1))
        .unwrap()
        .unwrap();

    assert!(hover.contents.contains("fn helper(x: i32) i32"));
    assert_eq!(
        analysis.last_analysis_tier(),
        Some(AnalysisTier::CleanSemantic)
    );
    assert_eq!(
        analysis
            .semantic_token_classification_cache
            .lock()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        analysis.semantic_classification_cache.lock().unwrap().len(),
        0
    );
    assert_eq!(analysis.navigation_cache.lock().unwrap().len(), 0);
    assert_eq!(analysis.artifact_cache.lock().unwrap().len(), 0);
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

    assert!(hover.contents.contains("fn helper(x: i32) i32"));
    assert!(
        hover
            .contents
            .contains("Read one byte from the receiver register.")
    );
    assert!(hover.contents.contains("**Safety**"));
    assert!(
        hover
            .contents
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
            "entry_module_path = \"src/mod.kn\"\n",
        ),
    )
    .unwrap();
    fs::write(
        dep_meta.join("src/mod.kn"),
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
    let app_path = root.join("app.kn");
    fs::write(&app_path, app_source).unwrap();

    let mut options = CompileOptions {
        library_bundle: LibraryBundle::Std,
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

    assert!(hover.contents.contains("fn helper() i32"));
    assert!(
        hover
            .contents
            .contains("Imported helper from a kmeta package.")
    );
    assert!(hover.contents.contains("**Safety**"));
    assert!(
        hover
            .contents
            .contains("Pure helper with no hidden runtime policy.")
    );
}

#[test]
fn hover_renders_variable_types_without_internal_fn_pointer_shape() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper(x: i32) i32 { return x; }\n",
        "fn main() i32 {\n",
        "    let value = helper;\n",
        "    return 0;\n",
        "}\n",
    );
    let uri = temp_file_uri("hover_variable_fn_type", source);

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

    assert!(hover.contents.contains("let value:"));
    assert!(!hover.contents.contains("&fn("));
}

#[test]
fn hover_resolves_std_module_docs_from_use_alias() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "use std.io;\n",
        "\n",
        "fn main() i32 {\n",
        "    \"hello\".println();\n",
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
        .hover(&uri, position_of_nth(source, "io", 0, 1))
        .unwrap()
        .unwrap();

    assert!(hover.contents.contains("module io"));
    assert!(
        hover
            .contents
            .contains("Text and byte-oriented output helpers.")
    );
}

#[test]
fn goto_definition_in_untitled_document_preserves_untitled_uri() {
    let mut analysis = AnalysisEngine::default();
    let uri = untitled_uri("Untitled-Definition");
    let source = "fn helper() i32 { return 1; }\nfn main() i32 { return helper(); }\n";

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let definition = analysis
        .goto_definition(&uri, position_of_nth(source, "helper()", 1, 2))
        .unwrap()
        .unwrap();

    assert_eq!(definition.uri, uri);
    assert_eq!(
        definition.range.start,
        position_of_nth(source, "helper", 0, 0)
    );
}

#[test]
fn hover_resolves_std_reexported_function_docs_from_member_access() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "use std.io;\n",
        "\n",
        "fn main() i32 {\n",
        "    \"hello\".println();\n",
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

    assert!(
        hover
            .contents
            .contains("fn println[N: usize](self: [N]u8) void"),
        "{}",
        hover.contents
    );
    assert!(
        hover
            .contents
            .contains("Writes this byte string to standard output followed by a newline.")
    );
}

#[test]
fn hover_resolves_impl_method_signature_from_reference() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Counter { value: i32 }\n",
        "impl Counter {\n",
        "    fn get() i32 { return self.value; }\n",
        "}\n",
        "fn main() i32 {\n",
        "    let counter = Counter.{ value: 1i32 };\n",
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

    assert!(hover.contents.contains("fn get(self: Counter) i32"));
    let range = hover.range.unwrap();
    assert_eq!(range.start, position_of_nth(source, "get", 1, 0));
    assert_eq!(range.end, position_of_nth(source, "get", 1, 3));
}

#[test]
fn hover_renders_doc_comments_for_impl_method_reference() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Counter { value: i32 }\n",
        "impl Counter {\n",
        "    /// Read the current counter value.\n",
        "    ///\n",
        "    /// Safety:\n",
        "    /// - keep `self` bound to a live counter object.\n",
        "    fn get() i32 { return self.value; }\n",
        "}\n",
        "fn main() i32 {\n",
        "    let counter = Counter.{ value: 1i32 };\n",
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

    assert!(hover.contents.contains("fn get(self: Counter) i32"));
    assert!(hover.contents.contains("Read the current counter value."));
    assert!(hover.contents.contains("**Safety**"));
    assert!(
        hover
            .contents
            .contains("keep `self` bound to a live counter object.")
    );
}

#[test]
fn hover_resolves_struct_field_from_reference() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Counter { value: i32 }\n",
        "fn main() i32 {\n",
        "    let counter = Counter.{ value: 1i32 };\n",
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

    assert!(hover.contents.contains("field value: i32"));
}

#[test]
fn hover_resolves_struct_field_from_literal_initializer() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Counter { value: i32 }\n",
        "fn main() i32 {\n",
        "    let counter = Counter.{ value: 1i32 };\n",
        "    return counter.value;\n",
        "}\n",
    );
    let uri = temp_file_uri("hover_struct_field_literal_initializer", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hover = analysis
        .hover(&uri, position_of_nth(source, "value", 1, 1))
        .unwrap()
        .unwrap();

    assert!(hover.contents.contains("field value: i32"));
}

#[test]
fn hover_renders_complex_nested_pointer_field_types() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Payload { x: &&i32, y: &mut &mut f64 }\n",
        "struct Complex { ptr: &mut &[&[4]Payload] }\n",
        "fn main() i32 {\n",
        "    let complex = Complex.{ ptr: @trap() };\n",
        "    let _ = complex.ptr;\n",
        "    return 0;\n",
        "}\n",
    );
    let uri = temp_file_uri("hover_complex_nested_pointer_field", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hover = analysis
        .hover(&uri, position_of_nth(source, "ptr", 2, 1))
        .unwrap()
        .unwrap();

    assert!(
        hover.contents.contains("field ptr: &mut &[&[4]Payload]"),
        "{}",
        hover.contents
    );
}

#[test]
fn definition_resolves_struct_field_from_literal_initializer() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Counter { value: i32 }\n",
        "fn main() i32 {\n",
        "    let counter = Counter.{ value: 1i32 };\n",
        "    return counter.value;\n",
        "}\n",
    );
    let uri = temp_file_uri("definition_struct_field_literal_initializer", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let definition = analysis
        .goto_definition(&uri, position_of_nth(source, "value", 1, 1))
        .unwrap()
        .unwrap();

    assert_eq!(definition.uri, uri);
    assert_eq!(
        definition.range.start,
        position_of_nth(source, "value", 0, 0)
    );
}

#[test]
fn document_symbols_render_complex_impl_target_types() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Payload { x: &&i32, y: &mut &mut f64 }\n",
        "impl &mut &[&[4]Payload] {\n",
        "    fn depth() i32 { return 0; }\n",
        "}\n",
    );
    let uri = temp_file_uri("document_symbols_complex_impl_target", source);

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
        .find(|symbol| symbol.detail.as_deref() == Some("impl"))
        .expect("expected impl symbol");

    assert_eq!(impl_symbol.name, "impl &mut &[&[4]Payload]");
}

#[test]
fn document_symbols_render_anonymous_struct_impl_target_types() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "impl &mut [4]struct { x: &&i32, y: &mut &mut f64 } {\n",
        "    fn depth() i32 { return 0; }\n",
        "}\n",
    );
    let uri = temp_file_uri("document_symbols_anon_struct_impl_target", source);

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
        .find(|symbol| symbol.detail.as_deref() == Some("impl"))
        .expect("expected impl symbol");

    assert_eq!(
        impl_symbol.name,
        "impl &mut [4]struct { x: &&i32, y: &mut &mut f64 }"
    );
}

#[test]
fn hover_resolves_enum_variant_from_reference() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "enum Result { Ok: i32, Err }\n",
        "fn main() i32 {\n",
        "    let value = Result.{ Ok: 1i32 };\n",
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

    assert!(hover.contents.contains("variant Ok: i32"));
}

#[test]
fn hover_resolves_match_variant_pattern_from_reference() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "enum Result { Ok: i32, Err }\n",
        "fn main() i32 {\n",
        "    let value = Result.Err;\n",
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

    assert!(hover.contents.contains("variant Err"));
}

#[test]
fn hover_resolves_typed_match_variant_path_from_reference() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "enum Result { Ok: i32, Err }\n",
        "fn main() i32 {\n",
        "    let value = Result.{ Ok: 1i32 };\n",
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

    assert!(hover.contents.contains("variant Ok: i32"));
}

#[test]
fn signature_help_resolves_function_parameters_and_active_argument() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper(first: i32, second: i32) i32 {\n",
        "    return first + second;\n",
        "}\n",
        "fn main() i32 {\n",
        "    let value = 2i32;\n",
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
fn semantic_position_queries_skip_comments_and_literals() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper(first: i32) i32 { return first; }\n",
        "/// helper(1)\n",
        "fn main() i32 {\n",
        "    // helper(1)\n",
        "    let text = \"helper(1)\";\n",
        "    let bad = \"\\qhelper(1)\";\n",
        "    let multi = \\\\ helper(1)\n",
        "    let ch = 'h';\n",
        "    let byte = b'h';\n",
        "    /* outer /* inner helper(1) */ still helper(1) */\n",
        "    return helper(1) + h as i32;\n",
        "}\n",
    );
    let uri = temp_file_uri("semantic_queries_skip_text_contexts", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    analysis.parse_cache.lock().unwrap().clear();
    analysis.surface_cache.lock().unwrap().clear();
    analysis.structure_cache.lock().unwrap().clear();
    analysis.artifact_cache.lock().unwrap().clear();

    let comment_position = position_of_nth(source, "helper(1)", 1, 1);
    assert!(
        analysis
            .hover(&uri, comment_position.clone())
            .unwrap()
            .is_none()
    );
    assert_eq!(analysis.last_analysis_tier(), Some(AnalysisTier::Lexical));
    assert!(analysis.artifact_cache.lock().unwrap().is_empty());

    analysis.clear_last_analysis_tier();
    assert!(
        analysis
            .goto_definition(&uri, comment_position.clone())
            .unwrap()
            .is_none()
    );
    assert_eq!(analysis.last_analysis_tier(), Some(AnalysisTier::Lexical));
    assert!(analysis.artifact_cache.lock().unwrap().is_empty());

    analysis.clear_last_analysis_tier();
    assert!(
        analysis
            .references(&uri, comment_position.clone(), true)
            .unwrap()
            .is_empty()
    );
    assert_eq!(analysis.last_analysis_tier(), Some(AnalysisTier::Lexical));
    assert!(analysis.artifact_cache.lock().unwrap().is_empty());

    analysis.clear_last_analysis_tier();
    assert!(
        analysis
            .document_highlights(&uri, comment_position.clone())
            .unwrap()
            .is_empty()
    );
    assert_eq!(analysis.last_analysis_tier(), Some(AnalysisTier::Lexical));
    assert!(analysis.artifact_cache.lock().unwrap().is_empty());

    analysis.clear_last_analysis_tier();
    assert!(
        analysis
            .prepare_rename(&uri, comment_position.clone())
            .unwrap()
            .is_none()
    );
    assert_eq!(analysis.last_analysis_tier(), Some(AnalysisTier::Lexical));
    assert!(analysis.artifact_cache.lock().unwrap().is_empty());

    analysis.clear_last_analysis_tier();
    assert_eq!(
        analysis
            .rename(&uri, comment_position, "assist")
            .unwrap_err(),
        "rename target is not a supported identifier"
    );
    assert_eq!(analysis.last_analysis_tier(), Some(AnalysisTier::Lexical));
    assert!(analysis.artifact_cache.lock().unwrap().is_empty());

    analysis.clear_last_analysis_tier();
    let literal_position = position_of_nth(source, "helper(1)", 2, 8);
    assert!(
        analysis
            .signature_help(&uri, literal_position)
            .unwrap()
            .is_none()
    );
    assert_eq!(analysis.last_analysis_tier(), Some(AnalysisTier::Lexical));
    assert!(analysis.artifact_cache.lock().unwrap().is_empty());

    for (description, position) in [
        ("doc comment", position_of_nth(source, "helper(1)", 0, 1)),
        (
            "multiline string",
            position_of_nth(source, "helper(1)", 4, 1),
        ),
        ("invalid string", position_of_nth(source, "helper(1)", 3, 1)),
        ("char literal", position_of_nth(source, "'h'", 0, 1)),
        ("byte char literal", position_of_nth(source, "b'h'", 0, 2)),
        (
            "nested block comment",
            position_of_nth(source, "helper(1)", 5, 1),
        ),
        (
            "outer block comment after nested close",
            position_of_nth(source, "helper(1)", 6, 1),
        ),
    ] {
        analysis.clear_last_analysis_tier();
        assert!(
            analysis.hover(&uri, position).unwrap().is_none(),
            "hover should be empty inside {description}"
        );
        assert_eq!(
            analysis.last_analysis_tier(),
            Some(AnalysisTier::Lexical),
            "{description}"
        );
        assert!(
            analysis.artifact_cache.lock().unwrap().is_empty(),
            "{description}"
        );
    }

    assert_eq!(analysis.lexical_cache.lock().unwrap().len(), 1);
}

#[test]
fn hover_resolves_local_definition_without_references() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() i32 {\n    let value = 1i32;\n    return 0;\n}\n";
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

    assert!(
        hover.contents.contains("let value: i32"),
        "{}",
        hover.contents
    );
}

#[test]
fn hover_on_impl_method_definition_prefers_method_span() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Counter {}\n",
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

    assert!(hover.contents.contains("fn get(self: Counter) i32"));
    assert_eq!(range.start, position_of_nth(source, "get", 0, 0));
    assert_eq!(range.end, position_of_nth(source, "get", 0, 3));
}

#[test]
fn hover_in_method_body_does_not_use_synthetic_self_span() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Counter { value: i32 }\n",
        "impl Counter {\n",
        "    fn get() i32 { return self.value; }\n",
        "}\n",
    );
    let uri = temp_file_uri("hover_method_body_synthetic_self", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let stray_hover = analysis
        .hover(&uri, position_of_nth(source, "return", 0, 1))
        .unwrap();
    assert!(
        stray_hover.is_none(),
        "unexpected hover: {:?}",
        stray_hover.map(|hover| (hover.range, hover.contents))
    );

    let self_hover = analysis
        .hover(&uri, position_of_nth(source, "self", 0, 1))
        .unwrap();
    if let Some(hover) = self_hover {
        let range = hover.range.unwrap();
        assert_eq!(range.start, position_of_nth(source, "self", 0, 0));
        assert_eq!(range.end, position_of_nth(source, "self", 0, 4));
    }
}

#[test]
fn navigation_tracks_impl_methods_spread_across_modules() {
    let root = unique_temp_dir("navigation_spread_impl_methods");
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();

    fs::write(
        src.join("mod.kn"),
        concat!(
            "mod storage;\n",
            "mod view;\n",
            "pub struct Editor { value: i32 }\n",
        ),
    )
    .unwrap();

    let storage_source = concat!(
        "use ..Editor;\n",
        "impl &mut Editor {\n",
        "    fn buffer_slot_mut() i32 { return self.value; }\n",
        "    pub fn local_use() i32 { return self.buffer_slot_mut(); }\n",
        "    pub fn local_use_again() i32 { return self.buffer_slot_mut(); }\n",
        "}\n",
    );
    let storage_path = src.join("storage.kn");
    fs::write(&storage_path, storage_source).unwrap();

    let view_source = concat!(
        "use ..Editor;\n",
        "impl &mut Editor {\n",
        "    pub fn view_use() i32 { return self.buffer_slot_mut(); }\n",
        "}\n",
    );
    let view_path = src.join("view.kn");
    fs::write(&view_path, view_source).unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&storage_path).unwrap();
    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: storage_source.to_string(),
        },
    });

    let definition = analysis
        .goto_definition(
            &uri,
            position_of_nth(storage_source, "buffer_slot_mut", 1, 2),
        )
        .unwrap()
        .unwrap();
    assert_eq!(definition.uri, uri);
    assert_eq!(
        definition.range.start,
        position_of_nth(storage_source, "buffer_slot_mut", 0, 0)
    );

    let references = analysis
        .references(
            &uri,
            position_of_nth(storage_source, "buffer_slot_mut", 1, 2),
            true,
        )
        .unwrap();
    let view_uri = file_path_to_uri(&view_path).unwrap();
    assert_eq!(references.len(), 4, "{references:#?}");
    assert!(references[..3].iter().all(|location| location.uri == uri));
    assert_eq!(references[3].uri, view_uri);

    let highlights = analysis
        .document_highlights(
            &uri,
            position_of_nth(storage_source, "buffer_slot_mut", 1, 2),
        )
        .unwrap();
    assert_eq!(highlights.len(), 3);

    let hover = analysis
        .hover(
            &uri,
            position_of_nth(storage_source, "buffer_slot_mut", 1, 2),
        )
        .unwrap()
        .unwrap();
    assert!(
        hover
            .contents
            .contains("fn buffer_slot_mut(self: &mut Editor) i32"),
        "{}",
        hover.contents
    );

    let edit = analysis
        .rename(
            &uri,
            position_of_nth(storage_source, "buffer_slot_mut", 1, 2),
            "shared_buffer_slot_mut",
        )
        .unwrap();
    assert_eq!(edit.changes.get(&uri).unwrap().len(), 3);
    assert_eq!(edit.changes.get(&view_uri).unwrap().len(), 1);
    assert_eq!(
        edit.changes.get(&view_uri).unwrap()[0].range.start,
        position_of_nth(view_source, "buffer_slot_mut", 0, 0)
    );
}

#[test]
fn hover_on_destructure_pun_prefers_local_binding() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Counter { value: i32 }\n",
        "fn main() i32 {\n",
        "    let counter = Counter.{ value: 1i32 };\n",
        "    let Counter.{ value } = counter;\n",
        "    return value;\n",
        "}\n",
    );
    let uri = temp_file_uri("hover_destructure_pun", source);

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

    assert!(hover.contents.contains("let value: i32"));
}

#[test]
fn hover_on_destructure_payload_binding_prefers_local_binding() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "pub enum Option[T] { Some: T, None }\n",
        "fn main(value: Option[i32]) i32 {\n",
        "    let .{ Some: inner } = value else return 0;\n",
        "    return inner;\n",
        "}\n",
    );
    let uri = temp_file_uri("hover_destructure_payload_binding", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hover = analysis
        .hover(&uri, position_of_nth(source, "inner", 0, 1))
        .unwrap()
        .unwrap();

    assert!(hover.contents.contains("let inner: i32"));
}

#[test]
fn definition_from_destructure_payload_binding_reference_resolves_local_binding() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "pub enum Option[T] { Some: T, None }\n",
        "fn main(value: Option[i32]) i32 {\n",
        "    let .{ Some: inner } = value else return 0;\n",
        "    return inner;\n",
        "}\n",
    );
    let uri = temp_file_uri("definition_destructure_payload_binding", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let definition = analysis
        .goto_definition(&uri, position_of_nth(source, "inner", 1, 1))
        .unwrap()
        .unwrap();

    assert_eq!(definition.uri, uri);
    assert_eq!(
        definition.range.start,
        position_of_nth(source, "inner", 0, 0)
    );
}

#[test]
fn goto_definition_on_destructure_pun_definition_prefers_local_binding() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Counter { value: i32 }\n",
        "fn main() i32 {\n",
        "    let counter = Counter.{ value: 1i32 };\n",
        "    let Counter.{ value } = counter;\n",
        "    return value;\n",
        "}\n",
    );
    let uri = temp_file_uri("definition_destructure_pun_definition", source);

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
        position_of_nth(source, "value", 2, 0)
    );
}

#[test]
fn references_from_destructure_pun_definition_follow_local_binding() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Counter { value: i32 }\n",
        "fn main() i32 {\n",
        "    let counter = Counter.{ value: 1i32 };\n",
        "    let Counter.{ value } = counter;\n",
        "    return value;\n",
        "}\n",
    );
    let uri = temp_file_uri("references_destructure_pun_definition", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let locations = analysis
        .references(&uri, position_of_nth(source, "value", 2, 1), true)
        .unwrap();

    assert_eq!(locations.len(), 2);
    assert_eq!(
        locations[0].range.start,
        position_of_nth(source, "value", 2, 0)
    );
    assert_eq!(
        locations[1].range.start,
        position_of_nth(source, "value", 3, 0)
    );
}

#[test]
fn document_highlights_on_destructure_pun_definition_follow_local_binding() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Counter { value: i32 }\n",
        "fn main() i32 {\n",
        "    let counter = Counter.{ value: 1i32 };\n",
        "    let Counter.{ value } = counter;\n",
        "    return value;\n",
        "}\n",
    );
    let uri = temp_file_uri("highlights_destructure_pun_definition", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let highlights = analysis
        .document_highlights(&uri, position_of_nth(source, "value", 2, 1))
        .unwrap();

    assert_eq!(highlights.len(), 2);
    assert_eq!(
        highlights[0].range.start,
        position_of_nth(source, "value", 2, 0)
    );
    assert_eq!(
        highlights[1].range.start,
        position_of_nth(source, "value", 3, 0)
    );
}

#[test]
fn rename_destructure_payload_binding_updates_definition_and_references() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "pub enum Option[T] { Some: T, None }\n",
        "fn main(value: Option[i32]) i32 {\n",
        "    let .{ Some: inner } = value else return 0;\n",
        "    return inner;\n",
        "}\n",
    );
    let uri = temp_file_uri("rename_destructure_payload_binding", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let edit = analysis
        .rename(&uri, position_of_nth(source, "inner", 0, 1), "payload")
        .unwrap();
    let edits = edit.changes.get(&uri).unwrap();

    assert_eq!(edits.len(), 2);
    assert!(edits.iter().all(|edit| edit.new_text == "payload"));
    assert_eq!(edits[0].range.start, position_of_nth(source, "inner", 0, 0));
    assert_eq!(edits[1].range.start, position_of_nth(source, "inner", 1, 0));
}

#[test]
fn hover_on_match_payload_binding_prefers_local_binding() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "pub enum Option[T] { Some: T, None }\n",
        "fn main(value: Option[i32]) i32 {\n",
        "    return match (value) {\n",
        "        .{ Some: payload } => payload,\n",
        "        .None => 0,\n",
        "    };\n",
        "}\n",
    );
    let uri = temp_file_uri("hover_match_payload_binding", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hover = analysis
        .hover(&uri, position_of_nth(source, "payload", 0, 1))
        .unwrap()
        .unwrap();

    assert!(hover.contents.contains("let payload: i32"));
}

#[test]
fn goto_definition_resolves_trait_object_method_references_to_trait_method() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "trait Base { fn foo() i32; }\n",
        "impl &i32 : Base { pub fn foo() i32 { return self.*; } }\n",
        "fn main() i32 {\n",
        "    let value = 3i32;\n",
        "    let base = (value.& as &Base);\n",
        "    return base.foo();\n",
        "}\n",
    );
    let uri = temp_file_uri("definition_trait_object_method", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let definition = analysis
        .goto_definition(&uri, position_of_nth(source, "foo", 2, 1))
        .unwrap()
        .unwrap();

    assert_eq!(definition.uri, uri);
    assert_eq!(definition.range.start, position_of_nth(source, "foo", 0, 0));
}

#[test]
fn hover_resolves_trait_object_method_references_to_trait_method() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "trait Base { fn foo() i32; }\n",
        "impl &i32 : Base { pub fn foo() i32 { return self.*; } }\n",
        "fn main() i32 {\n",
        "    let value = 3i32;\n",
        "    let base = (value.& as &Base);\n",
        "    return base.foo();\n",
        "}\n",
    );
    let uri = temp_file_uri("hover_trait_object_method", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hover = analysis
        .hover(&uri, position_of_nth(source, "foo", 2, 1))
        .unwrap()
        .unwrap();

    assert!(
        hover.contents.contains("fn foo(Base) i32"),
        "{}",
        hover.contents
    );
}

#[test]
fn references_for_trait_method_include_impl_definition_and_call_sites() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "trait Base { fn foo() i32; }\n",
        "impl &i32 : Base { pub fn foo() i32 { return self.*; } }\n",
        "fn main() i32 {\n",
        "    let value = 3i32;\n",
        "    let base = (value.& as &Base);\n",
        "    return base.foo();\n",
        "}\n",
    );
    let uri = temp_file_uri("references_trait_method_group", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let locations = analysis
        .references(&uri, position_of_nth(source, "foo", 2, 1), true)
        .unwrap();

    assert_eq!(locations.len(), 3);
    assert_eq!(
        locations[0].range.start,
        position_of_nth(source, "foo", 0, 0)
    );
    assert_eq!(
        locations[1].range.start,
        position_of_nth(source, "foo", 1, 0)
    );
    assert_eq!(
        locations[2].range.start,
        position_of_nth(source, "foo", 2, 0)
    );
}

#[test]
fn document_highlights_for_trait_impl_method_include_trait_and_call_sites() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "trait Base { fn foo() i32; }\n",
        "impl &i32 : Base { pub fn foo() i32 { return self.*; } }\n",
        "fn main() i32 {\n",
        "    let value = 3i32;\n",
        "    let base = (value.& as &Base);\n",
        "    return base.foo();\n",
        "}\n",
    );
    let uri = temp_file_uri("highlights_trait_method_group", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let highlights = analysis
        .document_highlights(&uri, position_of_nth(source, "foo", 1, 1))
        .unwrap();

    assert_eq!(highlights.len(), 3);
    assert_eq!(
        highlights[0].range.start,
        position_of_nth(source, "foo", 0, 0)
    );
    assert_eq!(
        highlights[1].range.start,
        position_of_nth(source, "foo", 1, 0)
    );
    assert_eq!(
        highlights[2].range.start,
        position_of_nth(source, "foo", 2, 0)
    );
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
    let source = "fn main() i32 {\n    let value = 1i32;\n    return value + value;\n}\n";
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
fn rename_destructure_pun_expands_pattern_and_updates_uses() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Counter { value: i32 }\n",
        "fn main() i32 {\n",
        "    let counter = Counter.{ value: 1i32 };\n",
        "    let Counter.{ value } = counter;\n",
        "    return value;\n",
        "}\n",
    );
    let uri = temp_file_uri("rename_destructure_pun", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let edit = analysis
        .rename(&uri, position_of_nth(source, "value", 2, 1), "answer")
        .unwrap_or_else(|err| panic!("{err}"));
    let edits = edit.changes.get(&uri).unwrap();

    assert_eq!(edits.len(), 2);
    assert_eq!(edits[0].range.start, position_of_nth(source, "value", 2, 0));
    assert_eq!(edits[0].new_text, "value: answer");
    assert_eq!(edits[1].range.start, position_of_nth(source, "value", 3, 0));
    assert_eq!(edits[1].new_text, "answer");
}

#[test]
fn rename_trait_method_reference_updates_trait_impl_and_call_sites() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "trait Base { fn foo() i32; }\n",
        "impl &i32 : Base { pub fn foo() i32 { return self.*; } }\n",
        "fn main() i32 {\n",
        "    let value = 3i32;\n",
        "    let base = (value.& as &Base);\n",
        "    return base.foo();\n",
        "}\n",
    );
    let uri = temp_file_uri("rename_trait_method_reference", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let edit = analysis
        .rename(&uri, position_of_nth(source, "foo", 2, 1), "read")
        .unwrap();
    let edits = edit.changes.get(&uri).unwrap();

    assert_eq!(edits.len(), 3);
    assert!(edits.iter().all(|edit| edit.new_text == "read"));
    assert_eq!(edits[0].range.start, position_of_nth(source, "foo", 0, 0));
    assert_eq!(edits[1].range.start, position_of_nth(source, "foo", 1, 0));
    assert_eq!(edits[2].range.start, position_of_nth(source, "foo", 2, 0));
}

#[test]
fn rename_trait_impl_method_updates_trait_and_call_sites() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "trait Base { fn foo() i32; }\n",
        "impl &i32 : Base { pub fn foo() i32 { return self.*; } }\n",
        "fn main() i32 {\n",
        "    let value = 3i32;\n",
        "    let base = (value.& as &Base);\n",
        "    return base.foo();\n",
        "}\n",
    );
    let uri = temp_file_uri("rename_trait_impl_method", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let edit = analysis
        .rename(&uri, position_of_nth(source, "foo", 1, 1), "read")
        .unwrap();
    let edits = edit.changes.get(&uri).unwrap();

    assert_eq!(edits.len(), 3);
    assert!(edits.iter().all(|edit| edit.new_text == "read"));
    assert_eq!(edits[0].range.start, position_of_nth(source, "foo", 0, 0));
    assert_eq!(edits[1].range.start, position_of_nth(source, "foo", 1, 0));
    assert_eq!(edits[2].range.start, position_of_nth(source, "foo", 2, 0));
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
    let file = SourceFile::new(PathBuf::from("utf16.kn"), "a😀b\n".to_string());
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
