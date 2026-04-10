use super::*;

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
    assert!(hover
        .contents
        .value
        .contains("Read one byte from the receiver register."));
    assert!(hover.contents.value.contains("**Safety**"));
    assert!(hover
        .contents
        .value
        .contains("`self` must point to a mapped UART object."));
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

    assert!(hover.contents.value.contains("fn helper: fn() i32"));
    assert!(hover
        .contents
        .value
        .contains("Imported helper from a kmeta package."));
    assert!(hover.contents.value.contains("**Safety**"));
    assert!(hover
        .contents
        .value
        .contains("Pure helper with no hidden runtime policy."));
}

#[test]
fn hover_resolves_std_module_docs_from_use_alias() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "use std.io;\n",
        "\n",
        "fn main() i32 {\n",
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
    assert!(hover
        .contents
        .value
        .contains("Text and byte-oriented output helpers."));
}

#[test]
fn hover_resolves_std_reexported_function_docs_from_member_access() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "use std.io;\n",
        "\n",
        "fn main() i32 {\n",
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
    assert!(hover
        .contents
        .value
        .contains("Formats into standard output and appends a newline."));
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
    assert!(hover
        .contents
        .value
        .contains("Read the current counter value."));
    assert!(hover.contents.value.contains("**Safety**"));
    assert!(hover
        .contents
        .value
        .contains("keep `self` bound to a live counter object."));
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
