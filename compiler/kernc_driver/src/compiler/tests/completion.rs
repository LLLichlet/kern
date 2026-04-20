use super::*;

#[test]
fn analysis_artifact_exposes_explicit_let_else_bindings_to_completion() {
    let root = std::env::temp_dir().join(format!(
        "kern_let_else_completion_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = concat!(
        "type Result[T, E] = enum {\n",
        "    Ok: T,\n",
        "    Err: E,\n",
        "};\n",
        "\n",
        "fn main(value: Result[i32, i32]) i32 {\n",
        "    let .{ Ok: ok } = value else {\n",
        "        .{ Err: err } => {\n",
        "            return err;\n",
        "        },\n",
        "    };\n",
        "    return ok;\n",
        "}\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let artifact = driver.analyze_artifact(main.to_str().unwrap(), &SourceOverrides::new());
    let err_offset = source.rfind("err;").expect("expected err use");
    let ok_offset = source.rfind("ok;").expect("expected ok use");

    let else_items = artifact.completion_items(main.as_path(), err_offset);
    assert!(else_items.iter().any(|item| item.label == "err"));
    assert!(!else_items.iter().any(|item| item.label == "ok"));

    let tail_items = artifact.completion_items(main.as_path(), ok_offset);
    assert!(tail_items.iter().any(|item| item.label == "ok"));
    assert!(!tail_items.iter().any(|item| item.label == "err"));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_exposes_flow_definitions_for_explicit_let_else_bindings() {
    let root = std::env::temp_dir().join(format!(
        "kern_let_else_flow_defs_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = concat!(
        "type Result[T, E] = enum {\n",
        "    Ok: T,\n",
        "    Err: E,\n",
        "};\n",
        "\n",
        "fn main(value: Result[i32, i32]) i32 {\n",
        "    let .{ Ok: ok } = value else {\n",
        "        .{ Err: err } => {\n",
        "            return err;\n",
        "        },\n",
        "    };\n",
        "    return ok;\n",
        "}\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let artifact = driver.analyze_artifact(main.to_str().unwrap(), &SourceOverrides::new());
    let function_owner = artifact
        .flow_owners()
        .into_iter()
        .find(|owner| owner.kind == AnalysisFlowOwnerKind::Function)
        .expect("expected function owner");
    let err_binding_span = source.find("err }").expect("expected err binding");
    let err_use_span = source.rfind("err;").expect("expected err use");
    let err_binding = function_owner
        .bindings
        .iter()
        .find(|binding| binding.definition_span.start == err_binding_span)
        .expect("expected err binding");
    let err_summary = function_owner
        .binding_summaries
        .iter()
        .find(|summary| summary.binding_id == err_binding.id)
        .expect("expected err summary");
    let err_use_node_id = *err_summary
        .use_node_ids
        .first()
        .expect("expected err use node");
    let err_use_facts = function_owner
        .node_facts
        .iter()
        .find(|facts| facts.node_id == err_use_node_id)
        .expect("expected err use facts");
    let err_def_node_id = err_summary.definition_node_ids[0];
    let err_def_facts = function_owner
        .node_facts
        .iter()
        .find(|facts| facts.node_id == err_def_node_id)
        .expect("expected err definition facts");

    assert_eq!(err_summary.definition_node_ids.len(), 1);
    assert!(
        err_binding
            .reference_spans
            .iter()
            .any(|span| span.start == err_use_span)
    );
    assert_eq!(err_use_facts.definition_kind, None);
    assert!(err_use_facts.use_binding_ids.contains(&err_binding.id));
    assert_eq!(
        err_def_facts.definition_kind,
        Some(AnalysisFlowDefinitionKind::Initializer)
    );
    assert!(err_def_facts.define_binding_ids.contains(&err_binding.id));

    let _ = fs::remove_dir_all(&root);
}
