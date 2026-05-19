//! Structure-query analysis tests.

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
fn workspace_symbols_reuse_symbol_index_across_queries() {
    let mut analysis = AnalysisEngine::default();
    let source = "struct SearchTarget { value: i32 }\nfn helper() void {}\n";
    let uri = temp_file_uri("workspace_symbols_index", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let target_symbols = analysis.workspace_symbols("target").unwrap();
    assert_eq!(target_symbols.len(), 1);
    assert_eq!(analysis.cached_workspace_symbol_index_count(), 1);

    let helper_symbols = analysis.workspace_symbols("helper").unwrap();
    assert_eq!(helper_symbols.len(), 1);
    assert_eq!(helper_symbols[0].name, "helper");
    assert_eq!(analysis.cached_workspace_symbol_index_count(), 1);

    let _ = analysis.change_document(DidChangeTextDocumentParams {
        text_document: VersionedTextDocumentIdentifier { uri, version: 2 },
        content_changes: vec![TextDocumentContentChangeEvent {
            range: None,
            text: "struct ChangedTarget { value: i32 }\nfn helper() void {}\n".to_string(),
        }],
    });
    assert_eq!(analysis.cached_workspace_symbol_index_count(), 1);

    let changed_symbols = analysis.workspace_symbols("changed").unwrap();
    assert_eq!(changed_symbols.len(), 1);
    assert_eq!(changed_symbols[0].name, "ChangedTarget");
    assert_eq!(analysis.cached_workspace_symbol_index_count(), 2);

    let helper_symbols = analysis.workspace_symbols("helper").unwrap();
    assert_eq!(helper_symbols.len(), 1);
    assert_eq!(analysis.cached_workspace_symbol_index_count(), 2);
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

#[test]
fn document_links_return_resolved_import_targets() {
    let root = unique_temp_dir("document_links_import_targets");
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
    fs::write(dep_dir.join("lib.kn"), "pub mod child;\n").unwrap();
    fs::write(
        dep_dir.join("child.kn"),
        "pub fn helper() i32 { return 1; }\n",
    )
    .unwrap();
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
    fs::write(
        app_dir.join("lib.kn"),
        "use dep.child;\nuse dep.{child as imported_child};\nfn main() i32 { return imported_child.helper(); }\n",
    )
    .unwrap();

    let source = fs::read_to_string(app_dir.join("lib.kn")).unwrap();
    let uri = file_path_to_uri(&app_dir.join("lib.kn")).unwrap();
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

    assert_eq!(links.len(), 2, "{links:#?}");
    assert!(
        links.iter().all(|link| link.target.ends_with("/child.kn")),
        "{links:#?}"
    );
    assert_eq!(
        links[0].range,
        Range {
            start: Position {
                line: 0,
                character: 8,
            },
            end: Position {
                line: 0,
                character: 13,
            },
        }
    );
    assert_eq!(
        links[1].range,
        Range {
            start: Position {
                line: 1,
                character: 18,
            },
            end: Position {
                line: 1,
                character: 32,
            },
        }
    );
}

#[test]
fn document_links_skip_grouped_import_leaf_symbols() {
    let root = unique_temp_dir("document_links_grouped_import_leaf");
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
    let source = "pub mod types;\npub use .types.{ Answer, Widget };\n";
    fs::write(src_dir.join("lib.kn"), source).unwrap();
    fs::write(
        src_dir.join("types.kn"),
        "pub const Answer = 42i32;\npub type Widget = i32;\n",
    )
    .unwrap();

    let uri = file_path_to_uri(&src_dir.join("lib.kn")).unwrap();
    let mut analysis = AnalysisEngine::default();
    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let links = analysis.document_links(&uri).unwrap();

    assert_eq!(links.len(), 1, "{links:#?}");
    assert_eq!(
        links[0].range,
        Range {
            start: Position {
                line: 0,
                character: 8,
            },
            end: Position {
                line: 0,
                character: 13,
            },
        }
    );
}

#[test]
fn document_links_return_empty_when_structure_analysis_fails() {
    let root = unique_temp_dir("document_links_structure_failure");
    fs::write(root.join("mod.kn"), "mod missing;\n").unwrap();

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

    assert!(links.is_empty(), "{links:#?}");
}

#[test]
fn document_links_return_manifest_dependency_targets() {
    let root = unique_temp_dir("document_links_manifest_dependencies");
    fs::create_dir_all(root.join("dep/src")).unwrap();
    fs::create_dir_all(root.join("shared/src")).unwrap();
    fs::create_dir_all(root.join("app/src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        format!(
            "\
[workspace]
name = \"workspace\"
members = [\"dep\", \"shared\", \"app\"]

[workspace.dependencies]
shared = {{ path = \"shared\" }}
"
        ),
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
    fs::write(root.join("dep/src/lib.kn"), "pub fn dep() void {}\n").unwrap();
    fs::write(
        root.join("shared/Craft.toml"),
        format!(
            "\
[package]
name = \"shared\"
version = \"0.1.0\"
kern = \"{CURRENT_KERN_VERSION}\"

[lib]
root = \"src/lib.kn\"
"
        ),
    )
    .unwrap();
    fs::write(root.join("shared/src/lib.kn"), "pub fn shared() void {}\n").unwrap();
    let manifest_source = format!(
        "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"{CURRENT_KERN_VERSION}\"

[lib]
root = \"src/lib.kn\"

[dependencies]
dep = {{ path = \"../dep\" }}
shared = {{ workspace = true }}
"
    );
    fs::write(root.join("app/Craft.toml"), &manifest_source).unwrap();
    fs::write(root.join("app/src/lib.kn"), "pub fn app() void {}\n").unwrap();

    let uri = file_path_to_uri(&root.join("app/Craft.toml")).unwrap();
    let mut analysis = AnalysisEngine::default();
    let _ = analysis.open_document_state(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "toml".to_string(),
            version: 1,
            text: manifest_source,
        },
    });

    let links = analysis.document_links(&uri).unwrap();

    assert_eq!(links.len(), 2, "{links:#?}");
    assert!(
        links[0].target.ends_with("/dep/Craft.toml"),
        "{}",
        links[0].target
    );
    assert_eq!(
        links[0].range,
        Range {
            start: Position {
                line: 9,
                character: 0,
            },
            end: Position {
                line: 9,
                character: 3,
            },
        }
    );
    assert!(
        links[1].target.ends_with("/shared/Craft.toml"),
        "{}",
        links[1].target
    );
    assert_eq!(
        links[1].range,
        Range {
            start: Position {
                line: 10,
                character: 0,
            },
            end: Position {
                line: 10,
                character: 6,
            },
        }
    );
}
