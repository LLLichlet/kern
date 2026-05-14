use super::*;

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
            && hint.position == position_of_nth(source, "value", 0, "value".len() as u32)
    }));
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
            && hint.position == position_of_nth(source, "value", 0, "value".len() as u32)
    }));
}

#[test]
fn inlay_hints_include_call_and_chain_expression_types() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Counter { value: i32 }\n",
        "impl Counter {\n",
        "    fn get() i32 { return self.value; }\n",
        "}\n",
        "fn make_counter() Counter { return Counter.{ value: 1i32 }; }\n",
        "fn main() i32 {\n",
        "    let value = make_counter().get();\n",
        "    return value;\n",
        "}\n",
    );
    let uri = temp_file_uri("inlay_hints_chain", source);

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
                == position_of_nth(source, "make_counter()", 1, "make_counter()".len() as u32)
    }));
    assert!(hints.iter().any(|hint| {
        hint.label == ": i32"
            && hint.position
                == position_of_nth(
                    source,
                    "make_counter().get()",
                    0,
                    "make_counter().get()".len() as u32,
                )
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
