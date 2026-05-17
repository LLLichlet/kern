use super::*;

#[test]
fn analysis_artifact_exposes_flow_owners() {
    let root = std::env::temp_dir().join(format!(
        "kern_flow_owners_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = "const helper = 1i32;\nfn main() i32 { return helper; }\n";
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let artifact = driver
        .analyze_artifact(
            main.to_str().unwrap(),
            &SourceOverrides::new(),
            &CancellationToken::new(),
        )
        .unwrap();
    let owners = artifact.flow_owners();

    assert_eq!(owners.len(), 2);
    assert!(owners.iter().any(|owner| {
        owner.kind == AnalysisFlowOwnerKind::Constant
            && owner.referenced_definition_spans.is_empty()
    }));
    assert!(owners.iter().any(|owner| {
        owner.kind == AnalysisFlowOwnerKind::Function
            && owner.referenced_definition_spans.len() == 1
    }));
    let function_owner = owners
        .iter()
        .find(|owner| owner.kind == AnalysisFlowOwnerKind::Function)
        .expect("expected function owner");
    assert_eq!(function_owner.bindings.len(), 0);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_exposes_flow_local_bindings() {
    let root = std::env::temp_dir().join(format!(
        "kern_flow_bindings_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn main(value: i32) i32 {\n",
        "    let local = value;\n",
        "    static cache = local;\n",
        "    return cache;\n",
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
    let owners = artifact.flow_owners();
    let function_owner = owners
        .iter()
        .find(|owner| owner.kind == AnalysisFlowOwnerKind::Function)
        .expect("expected function owner");

    assert_eq!(function_owner.bindings.len(), 3);
    assert!(function_owner.bindings.iter().any(|binding| {
        binding.kind == AnalysisFlowBindingKind::Parameter && binding.reference_spans.len() == 1
    }));
    assert!(function_owner.bindings.iter().any(|binding| {
        binding.kind == AnalysisFlowBindingKind::Variable && binding.reference_spans.len() == 1
    }));
    assert!(function_owner.bindings.iter().any(|binding| {
        binding.kind == AnalysisFlowBindingKind::Static && binding.reference_spans.len() == 1
    }));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_exposes_flow_liveness() {
    let root = std::env::temp_dir().join(format!(
        "kern_flow_liveness_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn main(value: i32) i32 {\n",
        "    let local = value;\n",
        "    return local;\n",
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
    let owners = artifact.flow_owners();
    let function_owner = owners
        .iter()
        .find(|owner| owner.kind == AnalysisFlowOwnerKind::Function)
        .expect("expected function owner");

    let parameter_binding = function_owner
        .bindings
        .iter()
        .find(|binding| binding.kind == AnalysisFlowBindingKind::Parameter)
        .expect("expected parameter binding");
    let local_binding = function_owner
        .bindings
        .iter()
        .find(|binding| binding.kind == AnalysisFlowBindingKind::Variable)
        .expect("expected local binding");
    let local_use_span = local_binding
        .reference_spans
        .first()
        .copied()
        .expect("expected local use span");

    assert_eq!(
        function_owner.liveness.len(),
        function_owner.cfg.nodes.len()
    );
    let entry_liveness = function_owner
        .liveness
        .iter()
        .find(|state| state.node_id == function_owner.cfg.entry)
        .expect("expected entry liveness");
    assert!(entry_liveness.live_out.contains(&parameter_binding.id));

    let local_eval_node_id = function_owner
        .cfg
        .nodes
        .iter()
        .find(|node| node.kind == AnalysisFlowCfgNodeKind::Eval && node.span == local_use_span)
        .expect("expected local eval node")
        .id;
    let local_eval_liveness = function_owner
        .liveness
        .iter()
        .find(|state| state.node_id == local_eval_node_id)
        .expect("expected local eval liveness");
    assert!(local_eval_liveness.live_in.contains(&local_binding.id));

    let return_node_id = function_owner
        .cfg
        .nodes
        .iter()
        .find(|node| node.kind == AnalysisFlowCfgNodeKind::Return)
        .expect("expected return node")
        .id;
    let return_liveness = function_owner
        .liveness
        .iter()
        .find(|state| state.node_id == return_node_id)
        .expect("expected return liveness");
    assert!(return_liveness.live_out.is_empty());

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_exposes_flow_binding_summaries() {
    let root = std::env::temp_dir().join(format!(
        "kern_flow_binding_summary_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn main(value: i32) i32 {\n",
        "    let local = value;\n",
        "    return local;\n",
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
    let owners = artifact.flow_owners();
    let function_owner = owners
        .iter()
        .find(|owner| owner.kind == AnalysisFlowOwnerKind::Function)
        .expect("expected function owner");
    let local_binding = function_owner
        .bindings
        .iter()
        .find(|binding| binding.kind == AnalysisFlowBindingKind::Variable)
        .expect("expected local binding");
    let local_use_span = local_binding
        .reference_spans
        .first()
        .copied()
        .expect("expected local use span");
    let local_use_node_id = function_owner
        .cfg
        .nodes
        .iter()
        .find(|node| node.kind == AnalysisFlowCfgNodeKind::Eval && node.span == local_use_span)
        .expect("expected local use node")
        .id;
    let local_summary = function_owner
        .binding_summaries
        .iter()
        .find(|summary| summary.binding_id == local_binding.id)
        .expect("expected local binding summary");

    assert_eq!(local_summary.definition_node_ids.len(), 1);
    assert!(local_summary.use_node_ids.contains(&local_use_node_id));
    assert!(local_summary.live_node_ids.contains(&local_use_node_id));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_exposes_flow_reaching_definitions() {
    let root = std::env::temp_dir().join(format!(
        "kern_flow_reaching_defs_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn main(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    return value;\n",
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
    let function_owner = artifact
        .flow_owners()
        .into_iter()
        .find(|owner| owner.kind == AnalysisFlowOwnerKind::Function)
        .expect("expected function owner");
    let value_binding = function_owner
        .bindings
        .iter()
        .find(|binding| binding.kind == AnalysisFlowBindingKind::Variable)
        .expect("expected local binding");
    let value_summary = function_owner
        .binding_summaries
        .iter()
        .find(|summary| summary.binding_id == value_binding.id)
        .expect("expected value binding summary");
    let value_use_node_id = *value_summary
        .use_node_ids
        .last()
        .expect("expected value use node");
    let reaching = function_owner
        .reaching_definitions
        .iter()
        .find(|state| state.node_id == value_use_node_id)
        .expect("expected reaching definition state");
    let value_reaching_in = reaching
        .reaching_in
        .iter()
        .filter(|definition| definition.binding_id == value_binding.id)
        .collect::<Vec<_>>();
    let value_reaching_out = reaching
        .reaching_out
        .iter()
        .filter(|definition| definition.binding_id == value_binding.id)
        .collect::<Vec<_>>();

    assert_eq!(value_summary.definition_node_ids.len(), 2);
    assert_eq!(value_reaching_in.len(), 1);
    assert_eq!(
        value_reaching_in[0].node_id,
        value_summary.definition_node_ids[1]
    );
    assert_eq!(value_reaching_out.len(), 1);
    assert_eq!(
        value_reaching_out[0].node_id,
        value_summary.definition_node_ids[1]
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_exposes_flow_node_facts_and_transfers() {
    let root = std::env::temp_dir().join(format!(
        "kern_flow_node_facts_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn main(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    return value;\n",
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
    let function_owner = artifact
        .flow_owners()
        .into_iter()
        .find(|owner| owner.kind == AnalysisFlowOwnerKind::Function)
        .expect("expected function owner");
    let value_binding = function_owner
        .bindings
        .iter()
        .find(|binding| binding.kind == AnalysisFlowBindingKind::Variable)
        .expect("expected local binding");
    let value_summary = function_owner
        .binding_summaries
        .iter()
        .find(|summary| summary.binding_id == value_binding.id)
        .expect("expected binding summary");

    let assignment_node_id = value_summary.definition_node_ids[1];
    let use_node_id = *value_summary
        .use_node_ids
        .last()
        .expect("expected value use node");

    let assignment_facts = function_owner
        .node_facts
        .iter()
        .find(|facts| facts.node_id == assignment_node_id)
        .expect("expected assignment node facts");
    assert_eq!(
        assignment_facts.definition_kind,
        Some(AnalysisFlowDefinitionKind::Assignment)
    );
    assert!(
        assignment_facts
            .define_binding_ids
            .contains(&value_binding.id)
    );

    let assignment_transfer = function_owner
        .node_transfers
        .iter()
        .find(|transfer| transfer.node_id == assignment_node_id)
        .expect("expected assignment node transfer");
    assert!(
        assignment_transfer
            .kill_binding_ids
            .contains(&value_binding.id)
    );
    assert!(
        assignment_transfer
            .generate_definitions
            .iter()
            .any(|definition| {
                definition.binding_id == value_binding.id
                    && definition.node_id == assignment_node_id
            })
    );

    let use_facts = function_owner
        .node_facts
        .iter()
        .find(|facts| facts.node_id == use_node_id)
        .expect("expected use node facts");
    assert!(use_facts.use_binding_ids.contains(&value_binding.id));
    assert!(use_facts.define_binding_ids.is_empty());

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_exposes_flow_definition_facts() {
    let root = std::env::temp_dir().join(format!(
        "kern_flow_definition_facts_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn main(seed: i32) i32 {\n",
        "    let local = seed;\n",
        "    let mut value = local;\n",
        "    value = seed + 1;\n",
        "    return value;\n",
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
    let function_owner = artifact
        .flow_owners()
        .into_iter()
        .find(|owner| owner.kind == AnalysisFlowOwnerKind::Function)
        .expect("expected function owner");
    let mut variable_bindings = function_owner
        .bindings
        .iter()
        .filter(|binding| binding.kind == AnalysisFlowBindingKind::Variable);
    let local_binding = variable_bindings.next().expect("expected local binding");
    let value_binding = variable_bindings.next().expect("expected value binding");

    let local_summary = function_owner
        .binding_summaries
        .iter()
        .find(|summary| summary.binding_id == local_binding.id)
        .expect("expected local summary");
    let value_summary = function_owner
        .binding_summaries
        .iter()
        .find(|summary| summary.binding_id == value_binding.id)
        .expect("expected value summary");

    let local_def = function_owner
        .definition_facts
        .iter()
        .find(|facts| facts.definition.node_id == local_summary.definition_node_ids[0])
        .expect("expected local definition facts");
    assert_eq!(local_def.kind, AnalysisFlowDefinitionKind::Initializer);
    let seed_binding = function_owner
        .bindings
        .iter()
        .find(|binding| binding.kind == AnalysisFlowBindingKind::Parameter)
        .expect("expected seed binding");
    assert_eq!(local_def.copy_source_binding_id, Some(seed_binding.id));

    let value_init_def = function_owner
        .definition_facts
        .iter()
        .find(|facts| facts.definition.node_id == value_summary.definition_node_ids[0])
        .expect("expected value initializer facts");
    assert_eq!(value_init_def.kind, AnalysisFlowDefinitionKind::Initializer);
    assert_eq!(
        value_init_def.copy_source_binding_id,
        Some(local_binding.id)
    );

    let value_assignment_def = function_owner
        .definition_facts
        .iter()
        .find(|facts| facts.definition.node_id == value_summary.definition_node_ids[1])
        .expect("expected value assignment facts");
    assert_eq!(
        value_assignment_def.kind,
        AnalysisFlowDefinitionKind::Assignment
    );
    assert_eq!(value_assignment_def.copy_source_binding_id, None);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_does_not_treat_casts_as_flow_copy_sources() {
    let root = std::env::temp_dir().join(format!(
        "kern_flow_cast_copy_sources_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn main(value: i32, ptr: &u8) usize {\n",
        "    let borrowed = value.&;\n",
        "    let addr = ptr as usize;\n",
        "    return addr + (borrowed as usize);\n",
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
    let function_owner = artifact
        .flow_owners()
        .into_iter()
        .find(|owner| owner.kind == AnalysisFlowOwnerKind::Function)
        .expect("expected function owner");
    let ptr_binding = function_owner
        .bindings
        .iter()
        .find(|binding| span_text(source, binding.definition_span) == "ptr")
        .expect("expected ptr binding");
    let addr_binding = function_owner
        .bindings
        .iter()
        .find(|binding| span_text(source, binding.definition_span) == "addr")
        .expect("expected addr binding");
    let value_binding = function_owner
        .bindings
        .iter()
        .find(|binding| span_text(source, binding.definition_span) == "value")
        .expect("expected value binding");
    let borrowed_binding = function_owner
        .bindings
        .iter()
        .find(|binding| span_text(source, binding.definition_span) == "borrowed")
        .expect("expected borrowed binding");
    let borrowed_def = function_owner
        .definition_facts
        .iter()
        .find(|facts| facts.definition.binding_id == borrowed_binding.id)
        .expect("expected borrowed definition facts");
    let addr_def = function_owner
        .definition_facts
        .iter()
        .find(|facts| facts.definition.binding_id == addr_binding.id)
        .expect("expected addr definition facts");

    assert_eq!(borrowed_def.kind, AnalysisFlowDefinitionKind::Initializer);
    assert_eq!(borrowed_def.copy_source_binding_id, None);
    assert_eq!(borrowed_def.use_binding_ids, vec![value_binding.id]);
    assert_eq!(addr_def.kind, AnalysisFlowDefinitionKind::Initializer);
    assert_eq!(addr_def.copy_source_binding_id, None);
    assert_eq!(addr_def.use_binding_ids, vec![ptr_binding.id]);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_exposes_flow_use_defs() {
    let root = std::env::temp_dir().join(format!(
        "kern_flow_use_defs_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn main(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    return value;\n",
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
    let function_owner = artifact
        .flow_owners()
        .into_iter()
        .find(|owner| owner.kind == AnalysisFlowOwnerKind::Function)
        .expect("expected function owner");
    let value_binding = function_owner
        .bindings
        .iter()
        .find(|binding| binding.kind == AnalysisFlowBindingKind::Variable)
        .expect("expected local binding");
    let value_summary = function_owner
        .binding_summaries
        .iter()
        .find(|summary| summary.binding_id == value_binding.id)
        .expect("expected binding summary");
    let use_node_id = *value_summary
        .use_node_ids
        .last()
        .expect("expected value use node");
    let use_def = function_owner
        .use_defs
        .iter()
        .find(|use_def| use_def.node_id == use_node_id && use_def.binding_id == value_binding.id)
        .expect("expected use-def entry");

    assert_eq!(use_def.reaching_definitions.len(), 1);
    assert_eq!(
        use_def.reaching_definitions[0].node_id,
        value_summary.definition_node_ids[1]
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_exposes_flow_def_uses() {
    let root = std::env::temp_dir().join(format!(
        "kern_flow_def_uses_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn main(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    return value;\n",
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
    let function_owner = artifact
        .flow_owners()
        .into_iter()
        .find(|owner| owner.kind == AnalysisFlowOwnerKind::Function)
        .expect("expected function owner");
    let value_binding = function_owner
        .bindings
        .iter()
        .find(|binding| binding.kind == AnalysisFlowBindingKind::Variable)
        .expect("expected local binding");
    let value_summary = function_owner
        .binding_summaries
        .iter()
        .find(|summary| summary.binding_id == value_binding.id)
        .expect("expected binding summary");
    let final_use_node_id = *value_summary
        .use_node_ids
        .last()
        .expect("expected final value use");

    let initializer_def_use = function_owner
        .def_uses
        .iter()
        .find(|def_use| def_use.definition.node_id == value_summary.definition_node_ids[0])
        .expect("expected initializer def-use");
    assert!(initializer_def_use.use_node_ids.is_empty());

    let assignment_def_use = function_owner
        .def_uses
        .iter()
        .find(|def_use| def_use.definition.node_id == value_summary.definition_node_ids[1])
        .expect("expected assignment def-use");
    assert_eq!(assignment_def_use.use_node_ids, vec![final_use_node_id]);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_exposes_flow_resolved_uses() {
    let root = std::env::temp_dir().join(format!(
        "kern_flow_resolved_uses_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn main(flag: bool, seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    if (flag) {\n",
        "        value = seed + 1;\n",
        "    } else {\n",
        "        value = seed + 2;\n",
        "    }\n",
        "    return value;\n",
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
    let function_owner = artifact
        .flow_owners()
        .into_iter()
        .find(|owner| owner.kind == AnalysisFlowOwnerKind::Function)
        .expect("expected function owner");
    let value_binding = function_owner
        .bindings
        .iter()
        .find(|binding| binding.kind == AnalysisFlowBindingKind::Variable)
        .expect("expected value binding");
    let seed_binding = function_owner
        .bindings
        .iter()
        .find(|binding| binding.kind == AnalysisFlowBindingKind::Parameter)
        .expect("expected seed binding");

    let value_summary = function_owner
        .binding_summaries
        .iter()
        .find(|summary| summary.binding_id == value_binding.id)
        .expect("expected value summary");
    let final_value_use = function_owner
        .resolved_uses
        .iter()
        .find(|resolved| {
            resolved.node_id
                == *value_summary
                    .use_node_ids
                    .last()
                    .expect("expected final value use")
                && resolved.binding_id == value_binding.id
        })
        .expect("expected resolved value use");
    assert_eq!(final_value_use.kind, AnalysisFlowResolvedUseKind::Ambiguous);
    assert_eq!(final_value_use.candidate_definitions.len(), 2);

    let missing_seed_use = function_owner
        .resolved_uses
        .iter()
        .find(|resolved| {
            resolved.binding_id == seed_binding.id
                && resolved.kind == AnalysisFlowResolvedUseKind::Missing
        })
        .expect("expected missing seed use");
    assert!(missing_seed_use.candidate_definitions.is_empty());

    let unique = root.join("unique.kn");
    let unique_source = concat!(
        "fn main(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    return value;\n",
        "}\n",
    );
    fs::write(&unique, unique_source).unwrap();

    let unique_artifact = driver
        .analyze_artifact(
            unique.to_str().unwrap(),
            &SourceOverrides::new(),
            &CancellationToken::new(),
        )
        .unwrap();
    let unique_owner = unique_artifact
        .flow_owners()
        .into_iter()
        .find(|owner| owner.kind == AnalysisFlowOwnerKind::Function)
        .expect("expected unique function owner");
    let unique_value_binding = unique_owner
        .bindings
        .iter()
        .find(|binding| binding.kind == AnalysisFlowBindingKind::Variable)
        .expect("expected unique value binding");
    let unique_value_summary = unique_owner
        .binding_summaries
        .iter()
        .find(|summary| summary.binding_id == unique_value_binding.id)
        .expect("expected unique value summary");
    let unique_value_use = unique_owner
        .resolved_uses
        .iter()
        .find(|resolved| {
            resolved.node_id
                == *unique_value_summary
                    .use_node_ids
                    .last()
                    .expect("expected unique final use")
                && resolved.binding_id == unique_value_binding.id
        })
        .expect("expected unique resolved use");
    assert_eq!(unique_value_use.kind, AnalysisFlowResolvedUseKind::Unique);
    assert_eq!(unique_value_use.candidate_definitions.len(), 1);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_exposes_flow_single_source_uses() {
    let root = std::env::temp_dir().join(format!(
        "kern_flow_single_source_uses_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn main(seed: i32) i32 {\n",
        "    let local = seed;\n",
        "    let value = local;\n",
        "    return value;\n",
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
    let function_owner = artifact
        .flow_owners()
        .into_iter()
        .find(|owner| owner.kind == AnalysisFlowOwnerKind::Function)
        .expect("expected function owner");

    let mut variable_bindings = function_owner
        .bindings
        .iter()
        .filter(|binding| binding.kind == AnalysisFlowBindingKind::Variable);
    let local_binding = variable_bindings.next().expect("expected local binding");
    let value_binding = variable_bindings.next().expect("expected value binding");
    let value_summary = function_owner
        .binding_summaries
        .iter()
        .find(|summary| summary.binding_id == value_binding.id)
        .expect("expected value summary");
    let final_value_use = *value_summary
        .use_node_ids
        .last()
        .expect("expected final value use");

    let single_source = function_owner
        .single_source_uses
        .iter()
        .find(|single| single.node_id == final_value_use && single.binding_id == value_binding.id)
        .expect("expected single-source use");
    assert_eq!(
        single_source.definition,
        AnalysisFlowDefinitionRef {
            binding_id: value_binding.id,
            node_id: value_summary.definition_node_ids[0],
        }
    );
    assert_eq!(
        single_source.definition_kind,
        AnalysisFlowDefinitionKind::Initializer
    );
    assert_eq!(single_source.copy_source_binding_id, Some(local_binding.id));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_exposes_flow_control_summary() {
    let root = std::env::temp_dir().join(format!(
        "kern_flow_summary_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn main(flag: bool) i32 {\n",
        "    defer trace(flag);\n",
        "    if (flag) {\n",
        "        return 1;\n",
        "    }\n",
        "    while (flag) {\n",
        "        if (flag) {\n",
        "            break;\n",
        "        }\n",
        "        trace(flag);\n",
        "    }\n",
        "    return match (1) {\n",
        "        1 => { continue_label(); 2 },\n",
        "        _ => 3,\n",
        "    };\n",
        "}\n",
        "fn trace(_: bool) void {}\n",
        "fn continue_label() void {}\n",
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
    let owners = artifact.flow_owners();
    let function_owner = owners
        .iter()
        .find(|owner| owner.kind == AnalysisFlowOwnerKind::Function)
        .expect("expected function owner");

    assert_eq!(function_owner.summary.block_count, 5);
    assert_eq!(function_owner.summary.branch_count, 3);
    assert_eq!(function_owner.summary.loop_count, 1);
    assert_eq!(function_owner.summary.defer_count, 1);
    assert_eq!(function_owner.summary.return_count, 2);
    assert_eq!(function_owner.summary.break_count, 1);
    assert_eq!(
        function_owner.cfg.nodes[function_owner.cfg.entry.index()].kind,
        AnalysisFlowCfgNodeKind::Entry
    );
    assert_eq!(
        function_owner.cfg.nodes[function_owner.cfg.exit.index()].kind,
        AnalysisFlowCfgNodeKind::Exit
    );
    assert!(
        function_owner
            .cfg
            .nodes
            .iter()
            .any(|node| { node.kind == AnalysisFlowCfgNodeKind::Branch })
    );
    assert!(
        function_owner
            .cfg
            .nodes
            .iter()
            .any(|node| { node.kind == AnalysisFlowCfgNodeKind::LoopHead })
    );
    assert!(
        function_owner
            .cfg
            .nodes
            .iter()
            .any(|node| { node.kind == AnalysisFlowCfgNodeKind::Match })
    );
    assert!(
        function_owner
            .cfg
            .nodes
            .iter()
            .any(|node| { node.kind == AnalysisFlowCfgNodeKind::Return })
    );
    assert!(
        function_owner
            .cfg
            .edges
            .iter()
            .any(|edge| { edge.kind == AnalysisFlowCfgEdgeKind::TrueBranch })
    );
    assert!(
        function_owner
            .cfg
            .edges
            .iter()
            .any(|edge| { edge.kind == AnalysisFlowCfgEdgeKind::FalseBranch })
    );
    assert!(
        function_owner
            .cfg
            .edges
            .iter()
            .any(|edge| { edge.kind == AnalysisFlowCfgEdgeKind::LoopBack })
    );
    assert!(
        function_owner
            .cfg
            .edges
            .iter()
            .any(|edge| { edge.kind == AnalysisFlowCfgEdgeKind::BreakFlow })
    );
    assert!(
        function_owner
            .cfg
            .edges
            .iter()
            .any(|edge| { edge.kind == AnalysisFlowCfgEdgeKind::ReturnFlow })
    );
    assert!(
        function_owner
            .control_regions
            .iter()
            .any(|region| { region.kind == AnalysisFlowRegionKind::If })
    );
    assert!(
        function_owner
            .control_regions
            .iter()
            .any(|region| { region.kind == AnalysisFlowRegionKind::Match })
    );
    assert!(
        function_owner
            .control_regions
            .iter()
            .any(|region| { region.kind == AnalysisFlowRegionKind::Loop })
    );

    let _ = fs::remove_dir_all(&root);
}

fn span_text(source: &str, span: kernc_utils::Span) -> &str {
    &source[span.start..span.end]
}
