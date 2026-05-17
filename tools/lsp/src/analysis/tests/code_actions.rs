use super::*;

fn resolve_deferred_action(
    analysis: &AnalysisEngine,
    action: super::super::ide::IdeCodeAction,
) -> super::super::ide::IdeCodeAction {
    let data = action.resolve_data.clone().unwrap();
    let snapshot = analysis.snapshot(Vec::new(), CancellationToken::new());
    analysis
        .resolve_code_action_in_snapshot(&snapshot, &data)
        .unwrap()
        .unwrap()
}

fn assert_deferred_action(
    action: &super::super::ide::IdeCodeAction,
    uri: &str,
    fix_id: &str,
    diagnostic_code: &str,
) {
    assert!(action.edit.is_none());
    let data = action.resolve_data.as_ref().unwrap();
    assert_eq!(data.uri, uri);
    assert_eq!(data.version, 1);
    assert_eq!(data.fix_id, fix_id);
    assert_eq!(data.diagnostic_code.as_deref(), Some(diagnostic_code));
}

#[test]
fn code_actions_offer_missing_semicolon_fix() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() i32 {\n    let value = 1\n    return value;\n}\n";
    let uri = temp_file_uri("code_action_semicolon", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 2,
                    character: 0,
                },
                end: Position {
                    line: 2,
                    character: 20,
                },
            },
        )
        .unwrap();

    let action = actions
        .iter()
        .find(|action| action.title == "Insert `;`")
        .unwrap();
    let edit = action.edit.as_ref().unwrap();
    let text_edit = edit.changes.get(&uri).unwrap().first().unwrap();

    assert_eq!(
        text_edit.range.start,
        Position {
            line: 2,
            character: 4,
        }
    );
    assert_eq!(text_edit.new_text, ";");
    assert_eq!(action.kind, Some("quickfix"));
    assert_eq!(action.is_preferred, Some(true));
}

#[test]
fn code_actions_offer_missing_closing_delimiter_fix() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() i32 {\n    return (1 + 2;\n}\n";
    let uri = temp_file_uri("code_action_paren", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 1,
                    character: 0,
                },
                end: Position {
                    line: 1,
                    character: 18,
                },
            },
        )
        .unwrap();

    let action = actions
        .iter()
        .find(|action| action.title == "Insert `)`")
        .unwrap();
    let edit = action.edit.as_ref().unwrap();
    let text_edit = edit.changes.get(&uri).unwrap().first().unwrap();

    assert_eq!(
        text_edit.range.start,
        Position {
            line: 1,
            character: 17,
        }
    );
    assert_eq!(text_edit.new_text, ")");
}

#[test]
fn code_actions_offer_discard_non_void_value_fix() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn helper() i32 { return 1; }\nfn main() void {\n    helper();\n}\n";
    let uri = temp_file_uri("code_action_discard_value", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 2,
                    character: 4,
                },
                end: Position {
                    line: 2,
                    character: 12,
                },
            },
        )
        .unwrap();

    let action = actions
        .iter()
        .find(|action| action.title == "Discard value with `let _ =`")
        .unwrap();
    let edit = action.edit.as_ref().unwrap();
    let text_edit = edit.changes.get(&uri).unwrap().first().unwrap();

    assert_eq!(
        text_edit.range.start,
        Position {
            line: 2,
            character: 4,
        }
    );
    assert_eq!(text_edit.new_text, "let _ = ");
}

#[test]
fn code_actions_offer_let_mut_fix() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() void {\n    let value = 1;\n    value = 2;\n}\n";
    let uri = temp_file_uri("code_action_let_mut", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 2,
                    character: 4,
                },
                end: Position {
                    line: 2,
                    character: 13,
                },
            },
        )
        .unwrap();

    let action = actions
        .iter()
        .find(|action| action.title == "Change to `let mut`")
        .unwrap();
    let diagnostic = action.diagnostics.first().unwrap();
    assert_deferred_action(action, &uri, "change-let-mut", "requires-let-mut");
    let resolved = resolve_deferred_action(&analysis, action.clone());
    let edit = resolved.edit.as_ref().unwrap();
    let text_edit = edit.changes.get(&uri).unwrap().first().unwrap();

    assert_eq!(diagnostic.code.as_deref(), Some("requires-let-mut"));
    assert_eq!(
        text_edit.range.start,
        Position {
            line: 1,
            character: 8,
        }
    );
    assert_eq!(text_edit.new_text, "mut ");
}

#[test]
fn code_actions_keep_untitled_uri_for_same_file_fix() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() i32 {\n    let value = 1\n    return value;\n}\n";
    let uri = untitled_uri("Untitled-CodeAction");

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 2,
                    character: 0,
                },
                end: Position {
                    line: 2,
                    character: 20,
                },
            },
        )
        .unwrap();

    let action = actions
        .iter()
        .find(|action| action.title == "Insert `;`")
        .unwrap();
    let edit = action.edit.as_ref().unwrap();
    assert!(edit.changes.contains_key(&uri));
}

#[test]
fn code_actions_on_dirty_documents_use_lightweight_fixes_without_full_analysis() {
    let mut analysis = AnalysisEngine::default();
    let clean_source = "fn main() i32 {\n    let value = 1;\n    return value;\n}\n";
    let dirty_source = "fn main() i32 {\n    let value = 1\n    return value;\n}\n";
    let uri = temp_file_uri("code_action_dirty_semicolon", clean_source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: clean_source.to_string(),
        },
    });
    analysis.artifact_cache.lock().unwrap().clear();

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

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 2,
                    character: 0,
                },
                end: Position {
                    line: 2,
                    character: 20,
                },
            },
        )
        .unwrap();

    assert_eq!(analysis.artifact_cache.lock().unwrap().len(), 0);
    assert_eq!(analysis.last_analysis_tier(), Some(AnalysisTier::ParseOnly));
    assert!(actions.iter().any(|action| action.title == "Insert `;`"));
}

#[test]
fn code_actions_offer_unused_binding_rename_fix() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main(unused_param: i32) i32 {\n    return 0;\n}\n";
    let uri = temp_file_uri("code_action_unused_binding", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 0,
                    character: 8,
                },
                end: Position {
                    line: 0,
                    character: 28,
                },
            },
        )
        .unwrap();

    let action = actions
        .iter()
        .find(|action| action.title == "Rename binding to `_`")
        .unwrap();
    assert_deferred_action(
        action,
        &uri,
        "rename-unused-binding-to-underscore",
        "unused-binding",
    );
    let resolved = resolve_deferred_action(&analysis, action.clone());
    let edit = resolved.edit.as_ref().unwrap();
    let text_edit = edit.changes.get(&uri).unwrap().first().unwrap();

    assert_eq!(
        text_edit.range.start,
        Position {
            line: 0,
            character: 8,
        }
    );
    assert_eq!(
        text_edit.range.end,
        Position {
            line: 0,
            character: 20,
        }
    );
    assert_eq!(text_edit.new_text, "_");
    assert_eq!(action.kind, Some("quickfix"));
    assert_eq!(action.is_preferred, Some(true));
}

#[test]
fn deferred_code_actions_keep_same_title_distinct_by_resolve_data() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main(first: i32, second: i32) i32 {\n    return 0;\n}\n";
    let uri = temp_file_uri("code_action_duplicate_unused_bindings", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: Position {
                    line: 0,
                    character: 34,
                },
            },
        )
        .unwrap();

    let rename_actions = actions
        .into_iter()
        .filter(|action| action.title == "Rename binding to `_`")
        .collect::<Vec<_>>();
    assert_eq!(rename_actions.len(), 2, "{rename_actions:#?}");
    let resolve_keys = rename_actions
        .iter()
        .map(|action| {
            let data = action.resolve_data.as_ref().unwrap();
            (
                data.diagnostic_range.start.character,
                data.diagnostic_range.end.character,
            )
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        resolve_keys,
        std::collections::BTreeSet::from([(8, 13), (20, 26)])
    );
}

#[test]
fn code_actions_offer_dead_store_assignment_removal_fix() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    value = seed + 2;\n",
        "    return value;\n",
        "}\n",
        "fn main() i32 { return helper(1); }\n",
    );
    let uri = temp_file_uri("code_action_dead_store_assignment", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 2,
                    character: 4,
                },
                end: Position {
                    line: 2,
                    character: 21,
                },
            },
        )
        .unwrap();

    let action = actions
        .iter()
        .find(|action| action.title == "Remove dead assignment")
        .unwrap();
    assert_deferred_action(action, &uri, "remove-dead-assignment", "dead-store");
    let resolved = resolve_deferred_action(&analysis, action.clone());
    let edit = resolved.edit.as_ref().unwrap();
    let text_edit = edit.changes.get(&uri).unwrap().first().unwrap();

    assert_eq!(
        text_edit.range.start,
        Position {
            line: 2,
            character: 0,
        }
    );
    assert_eq!(
        text_edit.range.end,
        Position {
            line: 3,
            character: 0,
        }
    );
    assert_eq!(text_edit.new_text, "");
    assert_eq!(action.kind, Some("quickfix"));
    assert_eq!(action.is_preferred, Some(true));
}

#[test]
fn code_actions_do_not_offer_dead_store_removal_for_initializer() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    return value;\n",
        "}\n",
        "fn main() i32 { return helper(1); }\n",
    );
    let uri = temp_file_uri("code_action_dead_store_initializer", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 1,
                    character: 4,
                },
                end: Position {
                    line: 1,
                    character: 25,
                },
            },
        )
        .unwrap();

    assert!(
        !actions
            .iter()
            .any(|action| action.title == "Remove dead assignment"),
        "unexpected actions: {actions:?}"
    );
}

#[test]
fn code_actions_offer_make_item_public_fix_for_unused_private_function() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper() i32 { return 1; }\n",
        "fn main() i32 { return 0; }\n",
    );
    let uri = temp_file_uri("code_action_unused_private_function", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: Position {
                    line: 0,
                    character: 20,
                },
            },
        )
        .unwrap();

    let action = actions
        .iter()
        .find(|action| action.title == "Make item public")
        .unwrap();
    assert_deferred_action(
        action,
        &uri,
        "make-private-item-public",
        "unused-private-item",
    );
    let resolved = resolve_deferred_action(&analysis, action.clone());
    let edit = resolved.edit.as_ref().unwrap();
    let text_edit = edit.changes.get(&uri).unwrap().first().unwrap();

    assert_eq!(
        text_edit.range.start,
        Position {
            line: 0,
            character: 0,
        }
    );
    assert_eq!(text_edit.new_text, "pub ");
    assert_eq!(action.kind, Some("quickfix"));
    assert_eq!(action.is_preferred, Some(false));
}

#[test]
fn code_actions_offer_match_catch_all_fix() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() i32 {\n    return match (1) {\n        1 => 1,\n    };\n}\n";
    let uri = temp_file_uri("code_action_match_catch_all", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 1,
                    character: 4,
                },
                end: Position {
                    line: 3,
                    character: 5,
                },
            },
        )
        .unwrap();

    let action = actions
        .iter()
        .find(|action| action.title == "Add `_ => @unreachable()` arm")
        .unwrap();
    assert_deferred_action(action, &uri, "add-match-catch-all", "nonexhaustive-match");
    assert_eq!(action.is_preferred, Some(false));
}

#[test]
fn code_actions_remove_irrefutable_let_else_branch() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn main() void {\n    let value = 1 else return;\n}\n";
    let uri = temp_file_uri("code_action_remove_let_else", source);

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let actions = analysis
        .code_actions(
            &uri,
            Range {
                start: Position {
                    line: 1,
                    character: 4,
                },
                end: Position {
                    line: 1,
                    character: 29,
                },
            },
        )
        .unwrap();

    let action = actions
        .iter()
        .find(|action| action.title == "Remove invalid `else` branch")
        .unwrap();
    assert_deferred_action(
        action,
        &uri,
        "remove-irrefutable-let-else",
        "irrefutable-let-else",
    );
    let resolved = resolve_deferred_action(&analysis, action.clone());
    let edit = resolved.edit.as_ref().unwrap();
    let text_edit = edit.changes.get(&uri).unwrap().first().unwrap();

    assert_eq!(
        text_edit.range.start,
        Position {
            line: 1,
            character: 18,
        }
    );
    assert_eq!(
        text_edit.range.end,
        Position {
            line: 1,
            character: 29,
        }
    );
    assert_eq!(text_edit.new_text, "");
}
