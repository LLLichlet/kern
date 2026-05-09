use super::*;

#[test]
fn diagnostics_include_native_doc_lints_as_warnings() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "///\n",
        "/// Args:\n",
        "/// - y: does not exist.\n",
        "fn helper(x: i32) i32 { return x; }\n",
        "fn main() i32 { return helper(1); }\n",
    );
    let uri = temp_file_uri("doc_lints", source);

    let outcome = open_document_for_full_diagnostics(&mut analysis, &uri, source);

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
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
            .any(|diagnostic| { diagnostic.message.contains("missing a summary paragraph") })
    );
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("unknown documented argument `y`")
    }));
}

#[test]
fn diagnostics_include_native_doc_lints_for_impl_methods() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "struct Counter { value: i32 }\n",
        "impl Counter {\n",
        "    /// Read the counter.\n",
        "    /// Args:\n",
        "    /// - missing: not a real parameter.\n",
        "    fn get() i32 { return self.value; }\n",
        "}\n",
    );
    let uri = temp_file_uri("doc_lints_impl_method", source);

    let outcome = open_document_for_full_diagnostics(&mut analysis, &uri, source);

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(
        bundle
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity == 2)
    );
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("unknown documented argument `missing`")
    }));
}

#[test]
fn diagnostics_warn_for_unreachable_private_function_chain() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn leaf() i32 { return 1; }\n",
        "fn helper() i32 { return leaf(); }\n",
        "fn main() i32 { return 0; }\n",
    );
    let uri = temp_file_uri("unused_private_chain", source);

    let outcome = open_document_for_full_diagnostics(&mut analysis, &uri, source);

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("private function `helper` is never used")
    }));
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("private function `leaf` is never used")
    }));
}

#[test]
fn diagnostics_warn_for_unreachable_private_constant() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!("const helper = 1;\n", "fn main() i32 { return 0; }\n",);
    let uri = temp_file_uri("unused_private_const", source);

    let outcome = open_document_for_full_diagnostics(&mut analysis, &uri, source);

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("private constant `helper` is never used")
    }));
}

#[test]
fn diagnostics_warn_for_unreachable_private_static() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!("static helper = 1;\n", "fn main() i32 { return 0; }\n",);
    let uri = temp_file_uri("unused_private_static", source);

    let outcome = open_document_for_full_diagnostics(&mut analysis, &uri, source);

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("private static `helper` is never used")
    }));
}

#[test]
fn diagnostics_warn_for_unused_parameter_and_local_binding() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper(_: i32, unused_param: i32, used_param: i32) i32 {\n",
        "    let unused_local = used_param;\n",
        "    return used_param;\n",
        "}\n",
        "fn main() i32 { return helper(1, 2, 3); }\n",
    );
    let uri = temp_file_uri("unused_bindings", source);

    let outcome = open_document_for_full_diagnostics(&mut analysis, &uri, source);

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("parameter `unused_param` is never used")
    }));
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("local variable `unused_local` is never used")
    }));
    assert!(
        !bundle
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.message.contains("parameter `_` is never used") })
    );
}

#[test]
fn diagnostics_warn_for_dead_store_assignment() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    if (seed == 0) { return value; }\n",
        "    value = seed + 1;\n",
        "    value = seed + 2;\n",
        "    return value;\n",
        "}\n",
        "fn main() i32 { return helper(1); }\n",
    );
    let uri = temp_file_uri("dead_store_assignment", source);

    let outcome = open_document_for_full_diagnostics(&mut analysis, &uri, source);

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("value assigned to `value` is never read")
    }));
}

#[test]
fn diagnostics_warn_for_dead_initializer() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    return value;\n",
        "}\n",
        "fn main() i32 { return helper(1); }\n",
    );
    let uri = temp_file_uri("dead_initializer", source);

    let outcome = open_document_for_full_diagnostics(&mut analysis, &uri, source);

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("initial value assigned to `value` is never read")
    }));
}

#[test]
fn diagnostics_mark_flow_and_reachability_warnings_as_unnecessary() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn leaf() i32 { return 1; }\n",
        "fn helper(unused_param: i32, seed: i32) i32 {\n",
        "    let unused_local = seed;\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    value = seed + 2;\n",
        "    return value;\n",
        "}\n",
        "fn main() i32 { return 0; }\n",
    );
    let uri = temp_file_uri("unnecessary_warning_tags", source);

    let outcome = open_document_for_full_diagnostics(&mut analysis, &uri, source);

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");

    for needle in [
        (
            "private function `leaf` is never used",
            "unused-private-item",
        ),
        (
            "private function `helper` is never used",
            "unused-private-item",
        ),
        ("parameter `unused_param` is never used", "unused-binding"),
        (
            "local variable `unused_local` is never used",
            "unused-binding",
        ),
        ("value assigned to `value` is never read", "dead-store"),
    ] {
        let (needle, code) = needle;
        let diagnostic = bundle
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.message.contains(needle))
            .unwrap_or_else(|| panic!("missing diagnostic: {needle}"));
        assert_eq!(diagnostic.code.as_deref(), Some(code));
        assert_eq!(diagnostic.tags, Some(vec![DiagnosticTag::Unnecessary]));
    }
}

#[test]
fn diagnostics_expose_structured_code_for_missing_semicolon() {
    let mut analysis = AnalysisEngine::default();
    let source = "fn helper() i32 {\n    let value = 1\n    return value;\n}\n";
    let uri = temp_file_uri("diagnostic_code_missing_semicolon", source);

    let outcome = open_document_for_full_diagnostics(&mut analysis, &uri, source);

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");

    let missing_semicolon = bundle
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("Expected semicolon"))
        .expect("missing semicolon diagnostic");
    assert_eq!(
        missing_semicolon.code.as_deref(),
        Some("expected-semicolon")
    );
}

#[test]
fn diagnostics_expose_structured_code_for_nonexhaustive_match() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper(value: i32) i32 {\n",
        "    return match (value) {\n",
        "        1 => 1,\n",
        "    };\n",
        "}\n",
    );
    let uri = temp_file_uri("diagnostic_code_nonexhaustive_match", source);

    let outcome = open_document_for_full_diagnostics(&mut analysis, &uri, source);

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");

    let nonexhaustive_match = bundle
        .diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic
                .message
                .contains("match expression is not exhaustive")
                || diagnostic
                    .message
                    .contains("match expression must be exhaustive")
        })
        .expect("missing nonexhaustive match diagnostic");
    assert_eq!(
        nonexhaustive_match.code.as_deref(),
        Some("nonexhaustive-match")
    );
}

#[test]
fn diagnostics_warn_for_unreachable_private_item_chain() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "const leaf = i32.{1};\n",
        "fn helper() i32 { return leaf; }\n",
        "fn main() i32 { return 0; }\n",
    );
    let uri = temp_file_uri("unused_private_item_chain", source);

    let outcome = open_document_for_full_diagnostics(&mut analysis, &uri, source);

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("private function `helper` is never used")
    }));
    assert!(bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("private constant `leaf` is never used")
    }));
}

#[test]
fn public_reexport_marks_private_function_as_reachable_root() {
    let mut analysis = AnalysisEngine::default();
    let source = concat!(
        "fn helper() i32 { return 1; }\n",
        "pub use .helper as exported;\n",
        "fn main() i32 { return 0; }\n",
    );
    let uri = temp_file_uri("unused_private_reexport_root", source);

    let outcome = open_document_for_full_diagnostics(&mut analysis, &uri, source);

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(
        !bundle.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("private function `helper` is never used")),
        "unexpected diagnostics: {:?}",
        bundle.diagnostics
    );
}

#[test]
fn body_only_change_recomputes_unused_private_function_warning() {
    let mut analysis = AnalysisEngine::default();
    let initial = concat!(
        "fn helper() i32 { return 1; }\n",
        "fn main() i32 { return 0; }\n",
    );
    let updated = concat!(
        "fn helper() i32 { return 1; }\n",
        "fn main() i32 { return helper(); }\n",
    );
    let uri = temp_file_uri("unused_private_incremental", initial);

    let open_outcome = open_document_for_full_diagnostics(&mut analysis, &uri, initial);
    let open_bundle = open_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(open_bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("private function `helper` is never used")
    }));

    let change_outcome = change_document_for_full_diagnostics(&mut analysis, &uri, 2, updated);
    let change_bundle = change_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(
        !change_bundle.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("private function `helper` is never used")),
        "unexpected diagnostics: {:?}",
        change_bundle.diagnostics
    );
}

#[test]
fn body_only_change_recomputes_unused_private_constant_warning() {
    let mut analysis = AnalysisEngine::default();
    let initial = concat!("const helper = 1;\n", "fn main() i32 { return 0; }\n",);
    let updated = concat!("const helper = 1;\n", "fn main() i32 { return helper; }\n",);
    let uri = temp_file_uri("unused_private_const_incremental", initial);

    let open_outcome = open_document_for_full_diagnostics(&mut analysis, &uri, initial);
    let open_bundle = open_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(open_bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("private constant `helper` is never used")
    }));

    let change_outcome = change_document_for_full_diagnostics(&mut analysis, &uri, 2, updated);
    let change_bundle = change_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(
        !change_bundle.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("private constant `helper` is never used")),
        "unexpected diagnostics: {:?}",
        change_bundle.diagnostics
    );
}

#[test]
fn body_only_change_recomputes_unused_private_static_warning() {
    let mut analysis = AnalysisEngine::default();
    let initial = concat!("static helper = 1;\n", "fn main() i32 { return 0; }\n",);
    let updated = concat!("static helper = 1;\n", "fn main() i32 { return helper; }\n",);
    let uri = temp_file_uri("unused_private_static_incremental", initial);

    let open_outcome = open_document_for_full_diagnostics(&mut analysis, &uri, initial);
    let open_bundle = open_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(open_bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("private static `helper` is never used")
    }));

    let change_outcome = change_document_for_full_diagnostics(&mut analysis, &uri, 2, updated);
    let change_bundle = change_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(
        !change_bundle.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("private static `helper` is never used")),
        "unexpected diagnostics: {:?}",
        change_bundle.diagnostics
    );
}

#[test]
fn body_only_change_recomputes_unused_binding_warnings() {
    let mut analysis = AnalysisEngine::default();
    let initial = concat!(
        "fn helper(unused_param: i32, used_param: i32) i32 {\n",
        "    let unused_local = used_param;\n",
        "    return used_param;\n",
        "}\n",
        "fn main() i32 { return helper(1, 2); }\n",
    );
    let updated = concat!(
        "fn helper(unused_param: i32, used_param: i32) i32 {\n",
        "    let unused_local = used_param;\n",
        "    if (unused_param == 0) { return unused_local; }\n",
        "    return used_param;\n",
        "}\n",
        "fn main() i32 { return helper(1, 2); }\n",
    );
    let uri = temp_file_uri("unused_bindings_incremental", initial);

    let open_outcome = open_document_for_full_diagnostics(&mut analysis, &uri, initial);
    let open_bundle = open_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(open_bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("parameter `unused_param` is never used")
    }));
    assert!(open_bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("local variable `unused_local` is never used")
    }));

    let change_outcome = change_document_for_full_diagnostics(&mut analysis, &uri, 2, updated);
    let change_bundle = change_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(
        !change_bundle.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("parameter `unused_param` is never used")),
        "unexpected diagnostics: {:?}",
        change_bundle.diagnostics
    );
    assert!(
        !change_bundle.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("local variable `unused_local` is never used")),
        "unexpected diagnostics: {:?}",
        change_bundle.diagnostics
    );
}

#[test]
fn body_only_change_recomputes_dead_store_warnings() {
    let mut analysis = AnalysisEngine::default();
    let initial = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    value = seed + 2;\n",
        "    return value;\n",
        "}\n",
        "fn main() i32 { return helper(1); }\n",
    );
    let updated = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    if (seed == 0) { return value; }\n",
        "    value = seed + 2;\n",
        "    return value;\n",
        "}\n",
        "fn main() i32 { return helper(1); }\n",
    );
    let uri = temp_file_uri("dead_store_incremental", initial);

    let open_outcome = open_document_for_full_diagnostics(&mut analysis, &uri, initial);
    let open_bundle = open_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(open_bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("value assigned to `value` is never read")
    }));

    let change_outcome = change_document_for_full_diagnostics(&mut analysis, &uri, 2, updated);
    let change_bundle = change_outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("expected diagnostics bundle");
    assert!(
        !change_bundle.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .starts_with("value assigned to `value` is never read")
        }),
        "unexpected diagnostics: {:?}",
        change_bundle.diagnostics
    );
    assert!(change_bundle.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("initial value assigned to `value` is never read")
    }));
}

#[test]
fn computes_cleared_uris() {
    let previous = BTreeSet::from(["file:///one.rn".to_string(), "file:///two.rn".to_string()]);
    let current = vec![super::DiagnosticBundle {
        uri: "file:///one.rn".to_string(),
        diagnostics: Vec::new(),
    }];

    let cleared = cleared_uris(&previous, &current);
    assert_eq!(cleared, vec!["file:///two.rn".to_string()]);
}
