use super::*;

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
    let source = "fn main() i32 {\n    le\n    return 0;\n}\n";
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
