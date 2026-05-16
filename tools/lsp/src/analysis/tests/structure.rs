use super::*;

#[test]
fn folding_ranges_return_multiline_blocks_and_comments() {
    let mut analysis = AnalysisEngine::default();
    let source =
        "/* head\n   body */\nfn main() void {\n    if true {\n        return;\n    }\n}\n";
    let uri = temp_file_uri("folding_ranges", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let ranges = analysis.folding_ranges(&uri).unwrap();
    assert_eq!(ranges.len(), 3);
    assert_eq!(
        ranges[0].kind,
        Some(crate::analysis::ide::IdeFoldingRangeKind::Comment)
    );
    assert_eq!(ranges[0].start_line, 0);
    assert_eq!(ranges[0].end_line, 1);
    assert_eq!(ranges[1].start_line, 2);
    assert_eq!(ranges[1].end_line, 6);
    assert_eq!(ranges[2].start_line, 3);
    assert_eq!(ranges[2].end_line, 5);
}

#[test]
fn folding_ranges_ignore_braces_inside_strings() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() void {\n    let value = \"{\";\n}\n";
    let uri = temp_file_uri("folding_ranges_ignore_string", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let ranges = analysis.folding_ranges(&uri).unwrap();
    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0].start_line, 0);
    assert_eq!(ranges[0].end_line, 2);
}

#[test]
fn selection_ranges_build_nested_parent_chain() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() void {\n    let value = helper(1);\n}\n";
    let uri = temp_file_uri("selection_ranges", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let ranges = analysis
        .selection_ranges(
            &uri,
            vec![Position {
                line: 1,
                character: 23,
            }],
        )
        .unwrap();
    assert_eq!(ranges.len(), 1);
    assert_eq!(
        ranges[0].range,
        Range {
            start: Position {
                line: 1,
                character: 23,
            },
            end: Position {
                line: 1,
                character: 24,
            },
        }
    );

    let call_range = ranges[0].parent.as_ref().unwrap();
    assert_eq!(call_range.range.start.line, 1);
    assert_eq!(call_range.range.start.character, 22);
    assert_eq!(call_range.range.end.character, 25);

    let line_range = call_range.parent.as_ref().unwrap();
    assert_eq!(line_range.range.start.line, 1);
    assert_eq!(line_range.range.end.line, 1);

    let block_range = line_range.parent.as_ref().unwrap();
    assert_eq!(block_range.range.start.line, 0);
    assert_eq!(block_range.range.end.line, 2);
}

#[test]
fn workspace_symbols_filter_open_document_symbols() {
    let mut analysis = AnalysisEngine::default();
    let source = "struct SearchTarget { value: i32 }\nfn helper() void {}\n";
    let uri = temp_file_uri("workspace_symbols", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let symbols = analysis.workspace_symbols("target").unwrap();
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "SearchTarget");
    assert_eq!(symbols[0].location.uri, uri);
    assert_eq!(symbols[0].location.range.start.line, 0);
    assert_eq!(symbols[0].location.range.start.character, 7);
}

#[test]
fn document_links_return_external_module_links() {
    let root = unique_temp_dir("document_links_external_module");
    fs::write(root.join("mod.kn"), "mod child;\nmod inline {}\n").unwrap();
    fs::write(root.join("child.kn"), "pub fn child() void {}\n").unwrap();

    let source = fs::read_to_string(root.join("mod.kn")).unwrap();
    let uri = file_path_to_uri(&root.join("mod.kn")).unwrap();
    let mut analysis = AnalysisEngine::default();
    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source,
        },
    });

    let links = analysis.document_links(&uri).unwrap();

    assert_eq!(links.len(), 1);
    assert_eq!(
        links[0].range,
        Range {
            start: Position {
                line: 0,
                character: 4,
            },
            end: Position {
                line: 0,
                character: 9,
            },
        }
    );
    assert!(
        links[0].target.ends_with("/child.kn"),
        "{}",
        links[0].target
    );
}
