use super::*;

fn inlay_position_of_nth(
    source: &str,
    needle: &str,
    occurrence: usize,
    char_offset: u32,
) -> IdePosition {
    position_of_nth(source, needle, occurrence, char_offset).into()
}

#[test]
fn inlay_hints_include_inferred_let_binding_types() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper() usize { return 1usize; }\n",
        "fn main() i32 {\n",
        "    let value = helper();\n",
        "    return 0;\n",
        "}\n",
    );
    let uri = temp_file_uri("inlay_hints_inferred_let", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hints = analysis.inlay_hints(&uri, whole_document_range()).unwrap();

    assert!(hints.iter().any(|hint| {
        hint.label == ": usize"
            && hint.position == inlay_position_of_nth(source, "value", 0, "value".len() as u32)
    }));
}

#[test]
fn inlay_hints_build_semantic_classification_artifact_after_tokens() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper() usize { return 1usize; }\n",
        "fn main() i32 {\n",
        "    let value = helper();\n",
        "    return 0;\n",
        "}\n",
    );
    let uri = temp_file_uri("inlay_hints_reuse_semantic_classification", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });
    let _ = analysis.semantic_tokens(&uri).unwrap();
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

    let hints = analysis.inlay_hints(&uri, whole_document_range()).unwrap();

    assert!(hints.iter().any(|hint| hint.label == ": usize"));
    assert_eq!(analysis.navigation_cache.lock().unwrap().len(), 0);
    assert_eq!(analysis.artifact_cache.lock().unwrap().len(), 0);
    assert_eq!(
        analysis.semantic_classification_cache.lock().unwrap().len(),
        1
    );
}

#[test]
fn inlay_hints_skip_explicit_let_binding_types() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn main() i32 {\n",
        "    let value: usize = 10usize;\n",
        "    return 0;\n",
        "}\n",
    );
    let uri = temp_file_uri("inlay_hints_explicit_let", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hints = analysis.inlay_hints(&uri, whole_document_range()).unwrap();

    assert!(!hints.iter().any(|hint| {
        hint.label == ": usize"
            && hint.position == inlay_position_of_nth(source, "value", 0, "value".len() as u32)
    }));
}

#[test]
fn inlay_hints_include_inferred_static_binding_types() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn main() i32 {\n",
        "    static mut total = 0usize;\n",
        "    return 0;\n",
        "}\n",
    );
    let uri = temp_file_uri("inlay_hints_static_binding", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hints = analysis.inlay_hints(&uri, whole_document_range()).unwrap();

    assert!(hints.iter().any(|hint| {
        hint.label == ": usize"
            && hint.position == inlay_position_of_nth(source, "total", 0, "total".len() as u32)
    }));
}

#[test]
fn inlay_hints_include_inferred_global_const_and_static_types() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct SpinLock {}\n",
        "const SPIN_UNLOCKED = SpinLock.{};\n",
        "static mut frame_op_lock = SPIN_UNLOCKED;\n",
        "fn main() i32 {\n",
        "    return 0;\n",
        "}\n",
    );
    let uri = temp_file_uri("inlay_hints_global_const_static", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hints = analysis.inlay_hints(&uri, whole_document_range()).unwrap();

    assert!(hints.iter().any(|hint| {
        hint.label == ": SpinLock"
            && hint.position
                == inlay_position_of_nth(source, "SPIN_UNLOCKED", 0, "SPIN_UNLOCKED".len() as u32)
    }));
    assert!(hints.iter().any(|hint| {
        hint.label == ": SpinLock"
            && hint.position
                == inlay_position_of_nth(source, "frame_op_lock", 0, "frame_op_lock".len() as u32)
    }));
}

#[test]
fn inlay_hints_skip_calls_fields_and_function_values() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "const ATOMIC_RELAXED = 0i32;\n",
        "const LOCK_STATE_LOCKED = 1u8;\n",
        "struct Counter { value: i32 }\n",
        "impl Counter {\n",
        "    fn get() i32 { return self.value; }\n",
        "}\n",
        "fn make_counter() Counter { return Counter.{ value: 1i32 }; }\n",
        "fn helper() i32 { return 1; }\n",
        "fn main() i32 {\n",
        "    static mut state: u8 = 0u8;\n",
        "    let value = make_counter().get();\n",
        "    while (@atomicLoad[u8](state..&, ATOMIC_RELAXED) == LOCK_STATE_LOCKED) {}\n",
        "    let callback = helper;\n",
        "    let result = helper();\n",
        "    return value;\n",
        "}\n",
    );
    let uri = temp_file_uri("inlay_hints_skip_expression_noise", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hints = analysis.inlay_hints(&uri, whole_document_range()).unwrap();

    assert!(hints.iter().any(|hint| {
        hint.label == ": i32"
            && hint.position == inlay_position_of_nth(source, "result", 0, "result".len() as u32)
    }));
    assert!(!hints.iter().any(|hint| hint.label.contains("fn(")));
    assert!(!hints.iter().any(|hint| {
        hint.position
            == inlay_position_of_nth(source, "make_counter()", 1, "make_counter()".len() as u32)
    }));
    assert!(!hints.iter().any(|hint| {
        hint.position
            == inlay_position_of_nth(
                source,
                "make_counter().get()",
                0,
                "make_counter().get()".len() as u32,
            )
    }));
    assert!(!hints.iter().any(|hint| {
        hint.position == inlay_position_of_nth(source, "state", 1, "state".len() as u32)
    }));
    assert!(!hints.iter().any(|hint| {
        hint.position
            == inlay_position_of_nth(
                source,
                "@atomicLoad[u8](state..&, ATOMIC_RELAXED)",
                0,
                "@atomicLoad[u8](state..&, ATOMIC_RELAXED)".len() as u32,
            )
    }));
}

#[test]
fn inlay_hints_include_only_multiline_chain_expression_types() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Counter { value: i32 }\n",
        "impl Counter {\n",
        "    fn get() i32 { return self.value; }\n",
        "    fn bump(amount: i32) Counter { return Counter.{ value: self.value + amount }; }\n",
        "}\n",
        "fn make_counter() Counter { return Counter.{ value: 1i32 }; }\n",
        "fn main() i32 {\n",
        "    let chained = make_counter()\n",
        "        .bump(1i32)\n",
        "        .get();\n",
        "    let single_line = make_counter().bump(2i32).get();\n",
        "    let argument_wrapped = make_counter().bump(\n",
        "        3i32,\n",
        "    );\n",
        "    return chained + single_line + argument_wrapped.get();\n",
        "}\n",
    );
    let uri = temp_file_uri("inlay_hints_multiline_chain", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hints = analysis.inlay_hints(&uri, whole_document_range()).unwrap();

    assert!(hints.iter().any(|hint| {
        hint.label == ": Counter"
            && hint.position
                == inlay_position_of_nth(
                    source,
                    "make_counter()\n        .bump(1i32)",
                    0,
                    "make_counter()\n        .bump(1i32)".len() as u32,
                )
    }));
    assert!(hints.iter().any(|hint| {
        hint.label == ": i32"
            && hint.position
                == inlay_position_of_nth(
                    source,
                    "make_counter()\n        .bump(1i32)\n        .get()",
                    0,
                    "make_counter()\n        .bump(1i32)\n        .get()".len() as u32,
                )
    }));
    assert!(!hints.iter().any(|hint| {
        hint.position
            == inlay_position_of_nth(
                source,
                "make_counter().bump(2i32).get()",
                0,
                "make_counter().bump(2i32).get()".len() as u32,
            )
    }));
    assert!(!hints.iter().any(|hint| {
        hint.position
            == inlay_position_of_nth(
                source,
                "make_counter().bump(\n        3i32,\n    )",
                0,
                "make_counter().bump(\n        3i32,\n    )".len() as u32,
            )
    }));
}

#[test]
fn inlay_hints_include_contextual_data_init_type_prefixes() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Point { x: i32, y: i32 }\n",
        "fn make_point() Point {\n",
        "    return .{ x: 1i32, y: 2i32 };\n",
        "}\n",
        "fn main() i32 {\n",
        "    let point: Point = .{ x: 10i32, y: 20i32 };\n",
        "    let explicit = Point.{ x: 30i32, y: 40i32 };\n",
        "    return point.x + explicit.y;\n",
        "}\n",
    );
    let uri = temp_file_uri("inlay_hints_contextual_data_init_prefix", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hints = analysis.inlay_hints(&uri, whole_document_range()).unwrap();

    assert!(hints.iter().any(|hint| {
        hint.label == "Point" && hint.position == inlay_position_of_nth(source, ".{ x: 1i32", 0, 0)
    }));
    assert!(hints.iter().any(|hint| {
        hint.label == "Point" && hint.position == inlay_position_of_nth(source, ".{ x: 10i32", 0, 0)
    }));
    assert!(!hints.iter().any(|hint| {
        hint.label == "Point"
            && hint.position == inlay_position_of_nth(source, "Point.{", 0, "Point".len() as u32)
    }));
}

#[test]
fn inlay_hints_include_contextual_enum_literal_type_prefixes() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "enum Result { Ok: i32, Err }\n",
        "fn make_result() Result {\n",
        "    return .Err;\n",
        "}\n",
        "fn main() i32 {\n",
        "    let value: Result = .Err;\n",
        "    let explicit = Result.Err;\n",
        "    return 0;\n",
        "}\n",
    );
    let uri = temp_file_uri("inlay_hints_contextual_enum_literal_prefix", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hints = analysis.inlay_hints(&uri, whole_document_range()).unwrap();

    assert!(hints.iter().any(|hint| {
        hint.label == "Result" && hint.position == inlay_position_of_nth(source, ".Err", 0, 0)
    }));
    assert!(hints.iter().any(|hint| {
        hint.label == "Result" && hint.position == inlay_position_of_nth(source, ".Err", 1, 0)
    }));
    assert!(!hints.iter().any(|hint| {
        hint.label == "Result"
            && hint.position
                == inlay_position_of_nth(source, "Result.Err", 0, "Result".len() as u32)
    }));
}

#[test]
fn inlay_hints_include_builtin_contextual_shorthand_type_prefixes() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn make_optional() ?i32 {\n",
        "    return .None;\n",
        "}\n",
        "fn make_result() i32!i32 {\n",
        "    return .{ Ok: 7i32 };\n",
        "}\n",
        "fn main() i32 {\n",
        "    let array: [_]u8 = .{ 1u8, 2u8, 3u8 };\n",
        "    return array[0] as i32;\n",
        "}\n",
    );
    let uri = temp_file_uri("inlay_hints_builtin_shorthand_prefix", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let hints = analysis.inlay_hints(&uri, whole_document_range()).unwrap();

    assert!(hints.iter().any(|hint| {
        hint.label == "?i32" && hint.position == inlay_position_of_nth(source, ".None", 0, 0)
    }));
    assert!(hints.iter().any(|hint| {
        hint.label == "i32!i32" && hint.position == inlay_position_of_nth(source, ".{ Ok", 0, 0)
    }));
    assert!(hints.iter().any(|hint| {
        hint.label == "[3]u8" && hint.position == inlay_position_of_nth(source, ".{ 1u8", 0, 0)
    }));
}

fn whole_document_range() -> Range {
    Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: u32::MAX,
            character: u32::MAX,
        },
    }
}
