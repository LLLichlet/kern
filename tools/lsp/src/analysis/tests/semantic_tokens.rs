use super::*;

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
    assert_eq!(analysis.last_analysis_tier(), Some(AnalysisTier::Lexical));
    assert!(
        decoded
            .iter()
            .any(|token| token.2 == SemanticTokenTypes::KEYWORD)
    );
}

#[test]
fn semantic_tokens_for_valid_dirty_documents_use_lexical_fallback() {
    let mut analysis = AnalysisEngine::default();
    let clean_source = "fn main() i32 {\n    let value = 1;\n    return value;\n}\n";
    let dirty_source = "fn main() i32 {\n\n    let value = 1;\n    return value;\n}\n";
    let uri = temp_file_uri("semantic_tokens_valid_dirty", clean_source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: clean_source.to_string(),
        },
    });

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

    let cached_artifacts = analysis.artifact_cache.borrow().len();
    let decoded = decode_semantic_tokens(&analysis.semantic_tokens(&uri).unwrap());
    assert!(!decoded.is_empty());
    assert_eq!(analysis.last_analysis_tier(), Some(AnalysisTier::Lexical));
    assert_eq!(analysis.artifact_cache.borrow().len(), cached_artifacts);
    assert!(
        decoded
            .iter()
            .any(|token| token.2 == SemanticTokenTypes::KEYWORD)
    );
}

#[test]
fn semantic_tokens_classify_keywords_types_and_functions() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Point { x: i32 }\n",
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
    warm_clean_semantic_artifact(&analysis, &uri, source);

    let decoded = decode_semantic_tokens(&analysis.semantic_tokens(&uri).unwrap());

    assert_token_type(
        &decoded,
        position_of_nth(source, "struct", 0, 0),
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
fn semantic_tokens_classify_prefixed_and_qualified_type_contexts() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Allocator {}\n",
        "fn z_string_layout(bytes: &[u8]) ?base.mem.Layout {\n",
        "    return .None;\n",
        "}\n",
        "pub fn owned(alloc: &mut Allocator, bytes: &[u8]) ?Owned {\n",
        "    return .None;\n",
        "}\n",
    );
    let uri = temp_file_uri("semantic_tokens_prefixed_type_contexts", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });
    warm_clean_semantic_artifact(&analysis, &uri, source);

    let decoded = decode_semantic_tokens(&analysis.semantic_tokens(&uri).unwrap());

    assert_token_type(
        &decoded,
        position_of_nth(source, "base", 0, 0),
        SemanticTokenTypes::TYPE,
    );
    assert_token_type(
        &decoded,
        position_of_nth(source, "mem", 0, 0),
        SemanticTokenTypes::TYPE,
    );
    assert_token_type(
        &decoded,
        position_of_nth(source, "Layout", 0, 0),
        SemanticTokenTypes::TYPE,
    );
    assert_token_type(
        &decoded,
        position_of_nth(source, "Allocator", 1, 0),
        SemanticTokenTypes::TYPE,
    );
}

#[test]
fn lexical_semantic_tokens_classify_full_impl_target_name() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "impl Bitmap {\n",
        "    pub fn get(index: usize) bool {\n",
        "        return false;\n",
        "    }\n",
        "}\n",
    );
    let uri = temp_file_uri("semantic_tokens_impl_target", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let decoded = decode_semantic_tokens(&analysis.semantic_tokens(&uri).unwrap());

    assert_token_with_length(
        &decoded,
        position_of_nth(source, "Bitmap", 0, 0),
        6,
        SemanticTokenTypes::TYPE,
    );
}

#[test]
fn semantic_tokens_prefer_symbol_kinds_and_modifiers_for_references() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "const LIMIT = 5i32;\n",
        "static mut TOTAL = 0i32;\n",
        "struct Counter { value: i32 }\n",
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
    warm_clean_semantic_artifact(&analysis, &uri, source);

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
        "enum Result { Ok: i32, Err }\n",
        "fn main() i32 {\n",
        "    let value = Result.{ Ok: 1i32 };\n",
        "    let other = Result.Err;\n",
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
    warm_clean_semantic_artifact(&analysis, &uri, source);

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
    assert_eq!(analysis.navigation_cache.borrow().len(), 1);
    assert_eq!(analysis.artifact_cache.borrow().len(), 0);

    analysis.navigation_cache.borrow_mut().clear();
    analysis.artifact_cache.borrow_mut().clear();
    let second = analysis.semantic_tokens(&uri).unwrap();

    assert_eq!(first.data, second.data);
    assert!(analysis.navigation_cache.borrow().is_empty());
    assert!(analysis.artifact_cache.borrow().is_empty());
    assert_eq!(analysis.semantic_tokens_cache.borrow().len(), 1);
}

#[test]
fn semantic_tokens_use_navigation_artifact_without_full_analysis() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Point { x: i32 }\n",
        "fn helper(point: Point) i32 {\n",
        "    return point.x;\n",
        "}\n",
    );
    let uri = temp_file_uri("semantic_tokens_navigation_artifact", source);

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
    analysis.navigation_cache.borrow_mut().clear();
    analysis.artifact_cache.borrow_mut().clear();

    let decoded = decode_semantic_tokens(&analysis.semantic_tokens(&uri).unwrap());

    assert!(!decoded.is_empty());
    assert_eq!(
        analysis.last_analysis_tier(),
        Some(AnalysisTier::CleanSemantic)
    );
    assert_eq!(analysis.navigation_cache.borrow().len(), 1);
    assert_eq!(analysis.artifact_cache.borrow().len(), 0);
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
    warm_clean_semantic_artifact(&analysis, &uri, source);

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
    warm_clean_semantic_artifact(&analysis, &uri, source);

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
        dbg_dir.join("mod.kn"),
        "pub mod helper;\nmod use_helper;\n",
    )
    .unwrap();
    fs::write(
        dbg_dir.join("helper.kn"),
        "pub fn helper() i32 { return 1; }\n",
    )
    .unwrap();
    let use_helper_source = "use ..helper.helper;\npub fn run() i32 { return helper(); }\n";
    fs::write(dbg_dir.join("use_helper.kn"), use_helper_source).unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&dbg_dir.join("use_helper.kn")).unwrap();
    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: use_helper_source.to_string(),
        },
    });
    warm_clean_semantic_artifact(&analysis, &uri, use_helper_source);

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
        "enum Option[T] {\n",
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
    warm_clean_semantic_artifact(&analysis, &uri, source);

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
