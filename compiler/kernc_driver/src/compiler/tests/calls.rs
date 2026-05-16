use super::*;

#[test]
fn analysis_artifact_exposes_direct_call_edges() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_calls_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn leaf() i32 { return 1; }\n",
        "fn helper() i32 { return leaf(); }\n",
        "fn main() i32 { return helper() + leaf(); }\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let artifact = driver
        .analyze_artifact(
            main.to_str().unwrap(),
            &SourceOverrides::new(),
            &CancellationToken::new(),
        )
        .unwrap();

    assert_eq!(artifact.calls.len(), 3, "{:#?}", artifact.calls);
    assert!(
        artifact
            .calls
            .iter()
            .all(|call| call.kind == AnalysisCallKind::Direct)
    );
    assert!(artifact.calls.iter().any(|call| {
        span_text(source, call.caller_definition_span) == "helper"
            && call
                .callee_definition_span
                .is_some_and(|span| span_text(source, span) == "leaf")
            && span_text(source, call.callee_span) == "leaf"
    }));
    assert!(artifact.calls.iter().any(|call| {
        span_text(source, call.caller_definition_span) == "main"
            && call
                .callee_definition_span
                .is_some_and(|span| span_text(source, span) == "helper")
            && span_text(source, call.callee_span) == "helper"
    }));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_classifies_trait_object_method_calls_as_dynamic_dispatch() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_dynamic_calls_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "trait Base { fn foo() i32; }\n",
        "impl &i32 : Base { pub fn foo() i32 { return self.*; } }\n",
        "impl &bool : Base { pub fn foo() i32 { return 7; } }\n",
        "fn run(base: &Base) i32 {\n",
        "    return base.foo();\n",
        "}\n",
        "fn main() i32 {\n",
        "    let value = 3i32;\n",
        "    let base = (value.& as &Base);\n",
        "    return run(base);\n",
        "}\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let artifact = driver
        .analyze_artifact(
            main.to_str().unwrap(),
            &SourceOverrides::new(),
            &CancellationToken::new(),
        )
        .unwrap();

    let foo_calls = artifact
        .calls
        .iter()
        .filter(|call| span_text(source, call.callee_span).ends_with(".foo"))
        .collect::<Vec<_>>();
    assert_eq!(foo_calls.len(), 1, "{:#?}", artifact.calls);
    assert_eq!(foo_calls[0].kind, AnalysisCallKind::DynamicDispatch);
    assert_eq!(
        span_text(source, foo_calls[0].caller_definition_span),
        "run"
    );
    assert_eq!(
        foo_calls[0]
            .callee_definition_span
            .map(|span| span_text(source, span)),
        Some("foo")
    );
    assert_eq!(span_text(source, foo_calls[0].callee_span), "base.foo");
    assert_eq!(foo_calls[0].dynamic_dispatch_targets.len(), 2);
    assert_ne!(
        foo_calls[0].dynamic_dispatch_targets[0],
        foo_calls[0].dynamic_dispatch_targets[1]
    );
    assert!(
        foo_calls[0]
            .dynamic_dispatch_targets
            .iter()
            .all(|span| span_text(source, *span) == "foo")
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_classifies_function_value_calls_as_indirect() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_indirect_calls_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn leaf() i32 { return 1; }\n",
        "fn apply(cb: &fn() i32) i32 { return cb(); }\n",
        "fn main() i32 { return apply(leaf); }\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let artifact = driver
        .analyze_artifact(
            main.to_str().unwrap(),
            &SourceOverrides::new(),
            &CancellationToken::new(),
        )
        .unwrap();

    let indirect_calls = artifact
        .calls
        .iter()
        .filter(|call| call.kind == AnalysisCallKind::Indirect)
        .collect::<Vec<_>>();
    assert_eq!(indirect_calls.len(), 1, "{:#?}", artifact.calls);
    assert_eq!(
        span_text(source, indirect_calls[0].caller_definition_span),
        "apply"
    );
    assert_eq!(span_text(source, indirect_calls[0].callee_span), "cb");
    assert!(indirect_calls[0].callee_definition_span.is_none());
    assert!(indirect_calls[0].dynamic_dispatch_targets.is_empty());

    let _ = fs::remove_dir_all(&root);
}

fn span_text(source: &str, span: kernc_utils::Span) -> &str {
    &source[span.start..span.end]
}
