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
            && span_text(source, call.callee_definition_span) == "leaf"
            && span_text(source, call.callee_span) == "leaf"
    }));
    assert!(artifact.calls.iter().any(|call| {
        span_text(source, call.caller_definition_span) == "main"
            && span_text(source, call.callee_definition_span) == "helper"
            && span_text(source, call.callee_span) == "helper"
    }));

    let _ = fs::remove_dir_all(&root);
}

fn span_text(source: &str, span: kernc_utils::Span) -> &str {
    &source[span.start..span.end]
}
