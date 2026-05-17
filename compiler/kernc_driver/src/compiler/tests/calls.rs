use super::*;
use crate::AnalysisCallTargetCompleteness;

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
    assert_eq!(indirect_calls[0].indirect_targets.len(), 1);
    assert_eq!(
        indirect_calls[0].indirect_target_completeness,
        AnalysisCallTargetCompleteness::Partial
    );
    assert_eq!(
        span_text(source, indirect_calls[0].indirect_targets[0]),
        "leaf"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_resolves_local_function_value_call_targets() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_local_indirect_calls_{}_{}",
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
        "fn main() i32 {\n",
        "    let cb = leaf;\n",
        "    return cb();\n",
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

    let indirect_calls = artifact
        .calls
        .iter()
        .filter(|call| call.kind == AnalysisCallKind::Indirect)
        .collect::<Vec<_>>();
    assert_eq!(indirect_calls.len(), 1, "{:#?}", artifact.calls);
    assert_eq!(
        span_text(source, indirect_calls[0].caller_definition_span),
        "main"
    );
    assert_eq!(span_text(source, indirect_calls[0].callee_span), "cb");
    assert!(indirect_calls[0].callee_definition_span.is_none());
    assert!(indirect_calls[0].dynamic_dispatch_targets.is_empty());
    assert_eq!(indirect_calls[0].indirect_targets.len(), 1);
    assert_eq!(
        span_text(source, indirect_calls[0].indirect_targets[0]),
        "leaf"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_resolves_forwarded_local_function_value_call_targets() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_forwarded_local_indirect_calls_{}_{}",
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
        "fn main() i32 {\n",
        "    let first = leaf;\n",
        "    let second = first;\n",
        "    return second();\n",
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

    let indirect_calls = artifact
        .calls
        .iter()
        .filter(|call| call.kind == AnalysisCallKind::Indirect)
        .collect::<Vec<_>>();
    assert_eq!(indirect_calls.len(), 1, "{:#?}", artifact.calls);
    assert_eq!(
        span_text(source, indirect_calls[0].caller_definition_span),
        "main"
    );
    assert_eq!(span_text(source, indirect_calls[0].callee_span), "second");
    assert!(indirect_calls[0].callee_definition_span.is_none());
    assert!(indirect_calls[0].dynamic_dispatch_targets.is_empty());
    assert_eq!(indirect_calls[0].indirect_targets.len(), 1);
    assert_eq!(
        span_text(source, indirect_calls[0].indirect_targets[0]),
        "leaf"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_resolves_mutable_function_value_assignment_targets() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_mut_function_value_assignment_calls_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn first() i32 { return 1; }\n",
        "fn second() i32 { return 2; }\n",
        "fn main() i32 {\n",
        "    let mut cb = first;\n",
        "    cb = second;\n",
        "    return cb();\n",
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

    let indirect_call = artifact
        .calls
        .iter()
        .find(|call| {
            call.kind == AnalysisCallKind::Indirect && span_text(source, call.callee_span) == "cb"
        })
        .unwrap_or_else(|| {
            panic!(
                "expected mutable function-value indirect call, got {:#?}",
                artifact.calls
            )
        });
    assert_eq!(
        indirect_call.indirect_target_completeness,
        AnalysisCallTargetCompleteness::Exact
    );
    assert_eq!(indirect_call.indirect_targets.len(), 1);
    assert_eq!(
        span_text(source, indirect_call.indirect_targets[0]),
        "second"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_resolves_multi_source_function_value_call_targets() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_multi_source_function_value_calls_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn first() i32 { return 1; }\n",
        "fn second() i32 { return 2; }\n",
        "fn main(flag: bool) i32 {\n",
        "    let mut cb = first;\n",
        "    if (flag) {\n",
        "        cb = second;\n",
        "    }\n",
        "    return cb();\n",
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

    let indirect_call = artifact
        .calls
        .iter()
        .find(|call| {
            call.kind == AnalysisCallKind::Indirect && span_text(source, call.callee_span) == "cb"
        })
        .unwrap_or_else(|| {
            panic!(
                "expected multi-source function-value indirect call, got {:#?}",
                artifact.calls
            )
        });
    assert_eq!(
        indirect_call.indirect_target_completeness,
        AnalysisCallTargetCompleteness::Partial
    );
    assert_eq!(indirect_call.indirect_targets.len(), 2);
    assert!(
        indirect_call
            .indirect_targets
            .iter()
            .any(|span| span_text(source, *span) == "first")
    );
    assert!(
        indirect_call
            .indirect_targets
            .iter()
            .any(|span| span_text(source, *span) == "second")
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_propagates_multi_source_function_value_parameters() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_multi_source_parameter_function_value_calls_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn first() i32 { return 1; }\n",
        "fn second() i32 { return 2; }\n",
        "fn apply(cb: &fn() i32) i32 { return cb(); }\n",
        "fn main(flag: bool) i32 {\n",
        "    let mut cb = first;\n",
        "    if (flag) {\n",
        "        cb = second;\n",
        "    }\n",
        "    return apply(cb);\n",
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

    let parameter_call = artifact
        .calls
        .iter()
        .find(|call| {
            call.kind == AnalysisCallKind::Indirect && span_text(source, call.callee_span) == "cb"
        })
        .unwrap_or_else(|| {
            panic!(
                "expected multi-source parameter indirect call, got {:#?}",
                artifact.calls
            )
        });
    assert_eq!(
        parameter_call.indirect_target_completeness,
        AnalysisCallTargetCompleteness::Partial
    );
    assert_eq!(parameter_call.indirect_targets.len(), 2);
    assert!(
        parameter_call
            .indirect_targets
            .iter()
            .any(|span| span_text(source, *span) == "first")
    );
    assert!(
        parameter_call
            .indirect_targets
            .iter()
            .any(|span| span_text(source, *span) == "second")
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_preserves_forwarded_multi_source_function_value_completeness() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_forwarded_multi_source_function_value_calls_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn first() i32 { return 1; }\n",
        "fn second() i32 { return 2; }\n",
        "fn main(flag: bool) i32 {\n",
        "    let mut cb = first;\n",
        "    if (flag) {\n",
        "        cb = second;\n",
        "    }\n",
        "    let alias = cb;\n",
        "    return alias();\n",
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

    let indirect_call = artifact
        .calls
        .iter()
        .find(|call| {
            call.kind == AnalysisCallKind::Indirect
                && span_text(source, call.callee_span) == "alias"
        })
        .unwrap_or_else(|| {
            panic!(
                "expected forwarded multi-source function-value indirect call, got {:#?}",
                artifact.calls
            )
        });
    assert_eq!(
        indirect_call.indirect_target_completeness,
        AnalysisCallTargetCompleteness::Partial
    );
    assert_eq!(indirect_call.indirect_targets.len(), 2);
    assert!(
        indirect_call
            .indirect_targets
            .iter()
            .any(|span| span_text(source, *span) == "first")
    );
    assert!(
        indirect_call
            .indirect_targets
            .iter()
            .any(|span| span_text(source, *span) == "second")
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_keeps_known_targets_for_partially_unknown_function_values() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_partially_unknown_function_value_calls_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn known() i32 { return 1; }\n",
        "fn apply(flag: bool, incoming: &fn() i32) i32 {\n",
        "    let mut cb = known;\n",
        "    if (flag) {\n",
        "        cb = incoming;\n",
        "    }\n",
        "    return cb();\n",
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

    let indirect_call = artifact
        .calls
        .iter()
        .find(|call| {
            call.kind == AnalysisCallKind::Indirect && span_text(source, call.callee_span) == "cb"
        })
        .unwrap_or_else(|| {
            panic!(
                "expected partially unknown function-value indirect call, got {:#?}",
                artifact.calls
            )
        });
    assert_eq!(
        indirect_call.indirect_target_completeness,
        AnalysisCallTargetCompleteness::Partial
    );
    assert_eq!(indirect_call.indirect_targets.len(), 1);
    assert_eq!(
        span_text(source, indirect_call.indirect_targets[0]),
        "known"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_marks_parameters_partial_when_any_argument_source_is_unknown() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_partially_unknown_parameter_calls_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn known() i32 { return 1; }\n",
        "fn apply(cb: &fn() i32) i32 { return cb(); }\n",
        "fn main(flag: bool, incoming: &fn() i32) i32 {\n",
        "    if (flag) {\n",
        "        return apply(known);\n",
        "    }\n",
        "    return apply(incoming);\n",
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

    let parameter_call = artifact
        .calls
        .iter()
        .find(|call| {
            call.kind == AnalysisCallKind::Indirect && span_text(source, call.callee_span) == "cb"
        })
        .unwrap_or_else(|| {
            panic!(
                "expected partially unknown parameter indirect call, got {:#?}",
                artifact.calls
            )
        });
    assert_eq!(
        parameter_call.indirect_target_completeness,
        AnalysisCallTargetCompleteness::Partial
    );
    assert_eq!(parameter_call.indirect_targets.len(), 1);
    assert_eq!(
        span_text(source, parameter_call.indirect_targets[0]),
        "known"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_resolves_grouped_function_value_call_targets() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_grouped_indirect_calls_{}_{}",
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
        "fn main() i32 {\n",
        "    let first = (leaf);\n",
        "    let second = (first);\n",
        "    return second() + apply((leaf));\n",
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

    let local_call = artifact
        .calls
        .iter()
        .find(|call| {
            call.kind == AnalysisCallKind::Indirect
                && span_text(source, call.callee_span) == "second"
        })
        .unwrap_or_else(|| {
            panic!(
                "expected grouped local indirect call, got {:#?}",
                artifact.calls
            )
        });
    assert_eq!(
        local_call.indirect_target_completeness,
        AnalysisCallTargetCompleteness::Exact
    );
    assert_eq!(local_call.indirect_targets.len(), 1);
    assert_eq!(span_text(source, local_call.indirect_targets[0]), "leaf");

    let parameter_call = artifact
        .calls
        .iter()
        .find(|call| {
            call.kind == AnalysisCallKind::Indirect && span_text(source, call.callee_span) == "cb"
        })
        .unwrap_or_else(|| {
            panic!(
                "expected grouped parameter indirect call, got {:#?}",
                artifact.calls
            )
        });
    assert_eq!(
        parameter_call.indirect_target_completeness,
        AnalysisCallTargetCompleteness::Partial
    );
    assert_eq!(parameter_call.indirect_targets.len(), 1);
    assert_eq!(
        span_text(source, parameter_call.indirect_targets[0]),
        "leaf"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_does_not_treat_parameters_as_single_function_value_targets() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_parameter_indirect_calls_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn first() i32 { return 1; }\n",
        "fn second() i32 { return 2; }\n",
        "fn apply(cb: &fn() i32) i32 { return cb(); }\n",
        "fn main() i32 { return apply(first) + apply(second); }\n",
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

    let parameter_call = artifact
        .calls
        .iter()
        .find(|call| {
            call.kind == AnalysisCallKind::Indirect && span_text(source, call.callee_span) == "cb"
        })
        .unwrap_or_else(|| {
            panic!(
                "expected parameter indirect call, got {:#?}",
                artifact.calls
            )
        });
    assert!(parameter_call.callee_definition_span.is_none());
    assert!(parameter_call.dynamic_dispatch_targets.is_empty());
    assert_eq!(parameter_call.indirect_targets.len(), 2);
    assert_eq!(
        parameter_call.indirect_target_completeness,
        AnalysisCallTargetCompleteness::Partial
    );
    assert!(
        parameter_call
            .indirect_targets
            .iter()
            .any(|span| span_text(source, *span) == "first")
    );
    assert!(
        parameter_call
            .indirect_targets
            .iter()
            .any(|span| span_text(source, *span) == "second")
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_propagates_function_value_parameters_across_direct_calls() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_parameter_forwarding_indirect_calls_{}_{}",
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
        "fn forward(cb: &fn() i32) i32 { return apply(cb); }\n",
        "fn main() i32 { return forward(leaf); }\n",
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

    let parameter_call = artifact
        .calls
        .iter()
        .find(|call| {
            call.kind == AnalysisCallKind::Indirect && span_text(source, call.callee_span) == "cb"
        })
        .unwrap_or_else(|| {
            panic!(
                "expected parameter indirect call, got {:#?}",
                artifact.calls
            )
        });
    assert_eq!(parameter_call.indirect_targets.len(), 1);
    assert_eq!(
        parameter_call.indirect_target_completeness,
        AnalysisCallTargetCompleteness::Partial
    );
    assert_eq!(
        span_text(source, parameter_call.indirect_targets[0]),
        "leaf"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_resolves_local_closure_object_call_targets() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_local_closure_indirect_calls_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn main() i32 {\n",
        "    let base = 2i32;\n",
        "    let cb = [base]() i32 { return base; };\n",
        "    return cb();\n",
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

    let indirect_calls = artifact
        .calls
        .iter()
        .filter(|call| call.kind == AnalysisCallKind::Indirect)
        .collect::<Vec<_>>();
    assert_eq!(indirect_calls.len(), 1, "{:#?}", artifact.calls);
    assert_eq!(
        span_text(source, indirect_calls[0].caller_definition_span),
        "main"
    );
    assert_eq!(span_text(source, indirect_calls[0].callee_span), "cb");
    assert_eq!(indirect_calls[0].indirect_targets.len(), 1);
    assert_eq!(
        indirect_calls[0].indirect_target_completeness,
        AnalysisCallTargetCompleteness::Exact
    );
    assert_eq!(
        span_text(source, indirect_calls[0].indirect_targets[0]),
        "cb"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_records_closure_body_call_edges() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_closure_body_calls_{}_{}",
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
        "fn seed() i32 { return 2; }\n",
        "fn main() i32 {\n",
        "    let cb = [base = seed()]() i32 { return base + leaf(); };\n",
        "    return cb();\n",
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

    let closure_body_call = artifact
        .calls
        .iter()
        .find(|call| {
            call.kind == AnalysisCallKind::Direct
                && span_text(source, call.caller_definition_span) == "cb"
                && span_text(source, call.callee_span) == "leaf"
        })
        .unwrap_or_else(|| {
            panic!(
                "expected closure-body direct call edge, got {:#?}",
                artifact.calls
            )
        });
    assert_eq!(
        closure_body_call
            .callee_definition_span
            .map(|span| span_text(source, span)),
        Some("leaf")
    );
    assert!(!artifact.calls.iter().any(|call| {
        call.kind == AnalysisCallKind::Direct
            && span_text(source, call.caller_definition_span) == "main"
            && span_text(source, call.callee_span) == "leaf"
    }));

    let capture_call = artifact
        .calls
        .iter()
        .find(|call| {
            call.kind == AnalysisCallKind::Direct
                && span_text(source, call.caller_definition_span) == "main"
                && span_text(source, call.callee_span) == "seed"
        })
        .expect("expected capture initializer call to remain attributed to main");
    assert_eq!(
        capture_call
            .callee_definition_span
            .map(|span| span_text(source, span)),
        Some("seed")
    );
    assert!(!artifact.calls.iter().any(|call| {
        call.kind == AnalysisCallKind::Direct
            && span_text(source, call.caller_definition_span) == "cb"
            && span_text(source, call.callee_span) == "seed"
    }));

    let indirect_call = artifact
        .calls
        .iter()
        .find(|call| {
            call.kind == AnalysisCallKind::Indirect && span_text(source, call.callee_span) == "cb"
        })
        .expect("expected closure object indirect call");
    assert_eq!(
        span_text(source, indirect_call.caller_definition_span),
        "main"
    );
    assert_eq!(indirect_call.indirect_targets.len(), 1);
    assert_eq!(span_text(source, indirect_call.indirect_targets[0]), "cb");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_propagates_closure_object_parameters_across_direct_calls() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_parameter_closure_indirect_calls_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn apply(cb: &Fn() i32) i32 { return cb(); }\n",
        "fn main() i32 {\n",
        "    let base = 2i32;\n",
        "    let local = [base]() i32 { return base; };\n",
        "    return apply(local);\n",
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

    let parameter_call = artifact
        .calls
        .iter()
        .find(|call| {
            call.kind == AnalysisCallKind::Indirect && span_text(source, call.callee_span) == "cb"
        })
        .unwrap_or_else(|| {
            panic!(
                "expected parameter indirect call, got {:#?}",
                artifact.calls
            )
        });
    assert_eq!(parameter_call.indirect_targets.len(), 1);
    assert_eq!(
        parameter_call.indirect_target_completeness,
        AnalysisCallTargetCompleteness::Partial
    );
    assert_eq!(
        span_text(source, parameter_call.indirect_targets[0]),
        "local"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_propagates_cast_closure_object_parameters_across_direct_calls() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_cast_closure_parameter_indirect_calls_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn apply(cb: &Fn() i32) i32 { return cb(); }\n",
        "fn main() i32 {\n",
        "    let base = 2i32;\n",
        "    let local = [base]() i32 { return base; };\n",
        "    return apply((local.& as &Fn() i32));\n",
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

    let parameter_call = artifact
        .calls
        .iter()
        .find(|call| {
            call.kind == AnalysisCallKind::Indirect && span_text(source, call.callee_span) == "cb"
        })
        .unwrap_or_else(|| {
            panic!(
                "expected cast closure parameter indirect call, got {:#?}",
                artifact.calls
            )
        });
    assert_eq!(parameter_call.indirect_targets.len(), 1);
    assert_eq!(
        parameter_call.indirect_target_completeness,
        AnalysisCallTargetCompleteness::Partial
    );
    assert_eq!(
        span_text(source, parameter_call.indirect_targets[0]),
        "local"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_resolves_forwarded_cast_closure_object_call_targets() {
    let root = std::env::temp_dir().join(format!(
        "kern_analysis_forwarded_cast_closure_indirect_calls_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn apply(cb: &Fn() i32) i32 { return cb(); }\n",
        "fn main() i32 {\n",
        "    let base = 2i32;\n",
        "    let local = [base]() i32 { return base; };\n",
        "    let erased = (local.& as &Fn() i32);\n",
        "    return erased() + apply(erased);\n",
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

    let local_call = artifact
        .calls
        .iter()
        .find(|call| {
            call.kind == AnalysisCallKind::Indirect
                && span_text(source, call.callee_span) == "erased"
        })
        .unwrap_or_else(|| {
            panic!(
                "expected forwarded cast closure indirect call, got {:#?}",
                artifact.calls
            )
        });
    assert_eq!(local_call.indirect_targets.len(), 1);
    assert_eq!(
        local_call.indirect_target_completeness,
        AnalysisCallTargetCompleteness::Exact
    );
    assert_eq!(span_text(source, local_call.indirect_targets[0]), "local");

    let parameter_call = artifact
        .calls
        .iter()
        .find(|call| {
            call.kind == AnalysisCallKind::Indirect && span_text(source, call.callee_span) == "cb"
        })
        .unwrap_or_else(|| {
            panic!(
                "expected forwarded cast closure parameter call, got {:#?}",
                artifact.calls
            )
        });
    assert_eq!(parameter_call.indirect_targets.len(), 1);
    assert_eq!(
        parameter_call.indirect_target_completeness,
        AnalysisCallTargetCompleteness::Partial
    );
    assert_eq!(
        span_text(source, parameter_call.indirect_targets[0]),
        "local"
    );

    let _ = fs::remove_dir_all(&root);
}

fn span_text(source: &str, span: kernc_utils::Span) -> &str {
    &source[span.start..span.end]
}
