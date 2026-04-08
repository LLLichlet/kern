use super::{
    AnalysisDeadStoreKind, AnalysisFlowBindingKind, AnalysisFlowCfgEdgeKind,
    AnalysisFlowCfgNodeKind, AnalysisFlowDefinitionKind, AnalysisFlowDefinitionRef,
    AnalysisFlowOwnerKind, AnalysisFlowRegionKind, AnalysisFlowResolvedUseKind,
    AnalysisUnusedBindingKind, AnalysisUnusedItemKind, CompilerDriver, SourceOverrides,
};
use kernc_mast::{MastBlock, MastExpr, MastExprKind, MastStmt};
use kernc_utils::Session;
use kernc_utils::config::{CompileOptions, DriverMode};
use std::fs;

fn count_assignments_in_block(block: &MastBlock) -> usize {
    let stmt_count: usize = block.stmts.iter().map(count_assignments_in_stmt).sum();
    let result_count = block
        .result
        .as_deref()
        .map(count_assignments_in_expr)
        .unwrap_or(0);
    let defer_count: usize = block.defers.iter().map(count_assignments_in_expr).sum();
    stmt_count + result_count + defer_count
}

fn count_assignments_in_stmt(stmt: &MastStmt) -> usize {
    match stmt {
        MastStmt::Let { init, .. } => count_assignments_in_expr(init),
        MastStmt::Expr(expr) => count_assignments_in_expr(expr),
    }
}

fn count_assignments_in_expr(expr: &MastExpr) -> usize {
    let self_count = usize::from(matches!(expr.kind, MastExprKind::Assign { .. }));
    let child_count = match &expr.kind {
        MastExprKind::AddressOf(operand)
        | MastExprKind::Deref(operand)
        | MastExprKind::ExtractFatPtrData(operand)
        | MastExprKind::ExtractFatPtrMeta(operand)
        | MastExprKind::BitIntrinsic { operand, .. }
        | MastExprKind::Cast { operand, .. }
        | MastExprKind::Unary { operand, .. } => count_assignments_in_expr(operand),
        MastExprKind::StructInit { fields, .. } | MastExprKind::ArrayInit(fields) => {
            fields.iter().map(count_assignments_in_expr).sum()
        }
        MastExprKind::UnionInit { value, .. } | MastExprKind::DataInit { payload: value, .. } => {
            count_assignments_in_expr(value)
        }
        MastExprKind::FieldAccess { lhs, .. } => count_assignments_in_expr(lhs),
        MastExprKind::IndexAccess { lhs, index }
        | MastExprKind::Binary {
            lhs, rhs: index, ..
        }
        | MastExprKind::Assign {
            lhs, rhs: index, ..
        } => count_assignments_in_expr(lhs) + count_assignments_in_expr(index),
        MastExprKind::Call { callee, args } => {
            count_assignments_in_expr(callee)
                + args.iter().map(count_assignments_in_expr).sum::<usize>()
        }
        MastExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            count_assignments_in_expr(cond)
                + count_assignments_in_block(then_branch)
                + else_branch
                    .as_ref()
                    .map(count_assignments_in_block)
                    .unwrap_or(0)
        }
        MastExprKind::Loop { body, latch } => {
            count_assignments_in_block(body)
                + latch.as_ref().map(count_assignments_in_block).unwrap_or(0)
        }
        MastExprKind::Switch {
            target,
            cases,
            default_case,
        } => {
            count_assignments_in_expr(target)
                + cases
                    .iter()
                    .map(|case| count_assignments_in_block(&case.body))
                    .sum::<usize>()
                + default_case
                    .as_ref()
                    .map(count_assignments_in_block)
                    .unwrap_or(0)
        }
        MastExprKind::Return(value) => value.as_deref().map(count_assignments_in_expr).unwrap_or(0),
        MastExprKind::AtomicLoad { ptr, .. } => count_assignments_in_expr(ptr),
        MastExprKind::AtomicStore { ptr, value, .. }
        | MastExprKind::AtomicRmw { ptr, value, .. } => {
            count_assignments_in_expr(ptr) + count_assignments_in_expr(value)
        }
        MastExprKind::AtomicCas {
            ptr,
            expected,
            desired,
            ..
        } => {
            count_assignments_in_expr(ptr)
                + count_assignments_in_expr(expected)
                + count_assignments_in_expr(desired)
        }
        MastExprKind::Fence { .. } => 0,
        MastExprKind::Memcpy { dest, src, len } => {
            count_assignments_in_expr(dest)
                + count_assignments_in_expr(src)
                + count_assignments_in_expr(len)
        }
        MastExprKind::Memmove { dest, src, len } => {
            count_assignments_in_expr(dest)
                + count_assignments_in_expr(src)
                + count_assignments_in_expr(len)
        }
        MastExprKind::Memset { dest, val, len } => {
            count_assignments_in_expr(dest)
                + count_assignments_in_expr(val)
                + count_assignments_in_expr(len)
        }
        MastExprKind::ConstructFatPointer { data_ptr, meta } => {
            count_assignments_in_expr(data_ptr) + count_assignments_in_expr(meta)
        }
        MastExprKind::Block(block) => count_assignments_in_block(block),
        MastExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            count_assignments_in_expr(lhs)
                + start.as_deref().map(count_assignments_in_expr).unwrap_or(0)
                + end.as_deref().map(count_assignments_in_expr).unwrap_or(0)
        }
        _ => 0,
    };

    self_count + child_count
}

#[test]
fn structure_cache_reuses_loaded_frontend_modules_until_input_changes() {
    let root = std::env::temp_dir().join(format!(
        "kern_structure_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let mut dirty = SourceOverrides::new();
    dirty.insert(main.clone(), "fn main() i32 { return 2; }".to_string());
    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &dirty)
            .is_some()
    );
    assert!(driver.uncached_parse_count() > parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn parse_modules_reuses_cached_structure_without_extra_frontend_parse() {
    let root = std::env::temp_dir().join(format!(
        "kern_parse_modules_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    let parsed = driver
        .parse_modules(main.to_str().unwrap(), &overrides)
        .expect("parsed modules should be available from cached structure");
    assert!(!parsed.modules.is_empty());
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn shared_incremental_state_reuses_frontend_cache_across_output_variants() {
    let root = std::env::temp_dir().join(format!(
        "kern_shared_incremental_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let options = CompileOptions {
        input_file: Some(main.to_string_lossy().to_string()),
        output_file: root.join("main.o").to_string_lossy().to_string(),
        ..CompileOptions::default()
    };
    let driver = CompilerDriver::new(options.clone());
    let overrides = SourceOverrides::new();

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    let shared = driver
        .share_incremental_state(CompileOptions {
            output_file: root.join("other.o").to_string_lossy().to_string(),
            ..options
        })
        .expect("output-only changes should preserve incremental compatibility");
    assert!(
        shared
            .analyze_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    assert_eq!(shared.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn shared_incremental_state_rejects_semantic_option_changes() {
    let driver = CompilerDriver::new(CompileOptions::default());
    let mut changed = CompileOptions::default();
    changed
        .custom_defines
        .insert("feature".to_string(), "enabled".to_string());

    assert!(driver.share_incremental_state(changed).is_none());
}

#[test]
fn compile_report_exposes_cache_hits_and_frontend_parse_deltas() {
    let root = std::env::temp_dir().join(format!(
        "kern_compile_cache_stats_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let object = root.join("main.o");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions {
        input_file: Some(main.to_string_lossy().to_string()),
        output_file: object.to_string_lossy().to_string(),
        driver_mode: DriverMode::CompileOnly,
        report_progress: false,
        ..CompileOptions::default()
    });

    let first = driver
        .compile_with_report()
        .expect("first compile should succeed");
    assert!(first.cache_stats.structure_misses > 0);
    assert!(first.cache_stats.fresh_frontend_parses > 0);

    let second = driver
        .compile_with_report()
        .expect("second compile should succeed");
    assert!(second.cache_stats.compile_structure_hits > 0);
    assert_eq!(second.cache_stats.fresh_frontend_parses, 0);

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &SourceOverrides::new())
            .is_some()
    );
    let third = driver
        .compile_with_report()
        .expect("structure-warmed compile should succeed");
    assert!(third.cache_stats.compile_structure_hits > 0);
    assert_eq!(third.cache_stats.fresh_frontend_parses, 0);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_outline_reuses_collected_cache_without_extra_frontend_parse() {
    let root = std::env::temp_dir().join(format!(
        "kern_outline_collected_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    let parsed = driver
        .parse_modules(main.to_str().unwrap(), &overrides)
        .expect("parsed modules should be available");
    assert!(!parsed.modules.is_empty());
    let parse_count = driver.uncached_parse_count();

    let outline = driver.analyze_outline(main.to_str().unwrap(), &overrides);
    assert!(!outline.symbols.is_empty());
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn imported_structure_cache_reuses_loaded_frontend_modules_without_type_stage() {
    let root = std::env::temp_dir().join(format!(
        "kern_imported_structure_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    let imported = driver
        .analyze_imported_structure(main.to_str().unwrap(), &overrides)
        .expect("imported structure should be available");
    let parse_count = driver.uncached_parse_count();

    let items = imported.completion_items(main.as_path(), 0);
    assert!(!items.is_empty());

    let imported_again = driver
        .analyze_imported_structure(main.to_str().unwrap(), &overrides)
        .expect("imported structure should stay cached");
    assert!(
        !imported_again
            .completion_items(main.as_path(), 0)
            .is_empty()
    );
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn surface_artifact_reuses_cached_imported_stage_without_extra_frontend_parse() {
    let root = std::env::temp_dir().join(format!(
        "kern_surface_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    assert!(
        driver
            .analyze_imported_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    let surface = driver
        .analyze_surface(main.to_str().unwrap(), &overrides)
        .expect("surface artifact should be derivable from imported cache");
    assert!(!surface.symbols.is_empty());
    assert!(!surface.completion_items(main.as_path(), 0).is_empty());
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_structure_reuses_cached_imported_stage_without_extra_frontend_parse() {
    let root = std::env::temp_dir().join(format!(
        "kern_structure_from_imported_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    assert!(
        driver
            .analyze_imported_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    let structure = driver
        .analyze_structure(main.to_str().unwrap(), &overrides)
        .expect("typed structure should be derivable from imported cache");
    assert!(!structure.completion_items(main.as_path(), 0).is_empty());
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn parse_modules_reuses_cached_imported_stage_without_extra_frontend_parse() {
    let root = std::env::temp_dir().join(format!(
        "kern_parse_from_imported_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    assert!(
        driver
            .analyze_imported_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    let parsed = driver
        .parse_modules(main.to_str().unwrap(), &overrides)
        .expect("parsed modules should be derivable from imported cache");
    assert!(!parsed.modules.is_empty());
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn parsed_modules_cache_preserves_body_completion_regions() {
    let root = std::env::temp_dir().join(format!(
        "kern_parse_body_regions_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = "fn main() i32 {\n    return 1;\n}\n";
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    assert!(
        driver
            .analyze_imported_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    let parsed = driver
        .parse_modules(main.to_str().unwrap(), &overrides)
        .expect("parsed modules should be derivable from imported cache");
    let body_offset = source.find("return").unwrap();
    let top_level_offset = source.find("fn").unwrap();

    assert!(parsed.requires_body_completion(main.as_path(), body_offset));
    assert!(!parsed.requires_body_completion(main.as_path(), top_level_offset));
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

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
        "    let .{ Ok: ok } = value else .{ Err: err } => {\n",
        "        return err;\n",
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
        "    let .{ Ok: ok } = value else .{ Err: err } => {\n",
        "        return err;\n",
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
    let main = root.join("main.rn");
    let source = "const helper = i32.{1};\nfn main() i32 { return helper; }\n";
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let artifact = driver.analyze_artifact(main.to_str().unwrap(), &SourceOverrides::new());
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
    let main = root.join("main.rn");
    let source = concat!(
        "fn main(value: i32) i32 {\n",
        "    let local = value;\n",
        "    static cache = local;\n",
        "    return cache;\n",
        "}\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let artifact = driver.analyze_artifact(main.to_str().unwrap(), &SourceOverrides::new());
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
    let main = root.join("main.rn");
    let source = concat!(
        "fn main(value: i32) i32 {\n",
        "    let local = value;\n",
        "    return local;\n",
        "}\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let artifact = driver.analyze_artifact(main.to_str().unwrap(), &SourceOverrides::new());
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
    let main = root.join("main.rn");
    let source = concat!(
        "fn main(value: i32) i32 {\n",
        "    let local = value;\n",
        "    return local;\n",
        "}\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let artifact = driver.analyze_artifact(main.to_str().unwrap(), &SourceOverrides::new());
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
    let main = root.join("main.rn");
    let source = concat!(
        "fn main(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    return value;\n",
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
    let main = root.join("main.rn");
    let source = concat!(
        "fn main(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    return value;\n",
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
    let main = root.join("main.rn");
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
    let artifact = driver.analyze_artifact(main.to_str().unwrap(), &SourceOverrides::new());
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
    let main = root.join("main.rn");
    let source = concat!(
        "fn main(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    return value;\n",
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
    let main = root.join("main.rn");
    let source = concat!(
        "fn main(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    return value;\n",
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
    let main = root.join("main.rn");
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
    let artifact = driver.analyze_artifact(main.to_str().unwrap(), &SourceOverrides::new());
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

    let unique = root.join("unique.rn");
    let unique_source = concat!(
        "fn main(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    return value;\n",
        "}\n",
    );
    fs::write(&unique, unique_source).unwrap();

    let unique_artifact =
        driver.analyze_artifact(unique.to_str().unwrap(), &SourceOverrides::new());
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
    let main = root.join("main.rn");
    let source = concat!(
        "fn main(seed: i32) i32 {\n",
        "    let local = seed;\n",
        "    let value = local;\n",
        "    return value;\n",
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
    let main = root.join("main.rn");
    let source = concat!(
        "fn main(flag: bool) i32 {\n",
        "    defer trace(flag);\n",
        "    if (flag) {\n",
        "        return 1;\n",
        "    }\n",
        "    for (; flag; flag = false) {\n",
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
    let artifact = driver.analyze_artifact(main.to_str().unwrap(), &SourceOverrides::new());
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

#[test]
fn analysis_artifact_exposes_unused_private_items() {
    let root = std::env::temp_dir().join(format!(
        "kern_unused_items_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = concat!(
        "const dead_const = 1;\n",
        "fn dead_fn() i32 { return dead_const; }\n",
        "extern fn main() i32 { return 0; }\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let artifact = driver.analyze_artifact(main.to_str().unwrap(), &SourceOverrides::new());
    let unused = artifact.unused_private_items();

    assert_eq!(unused.len(), 2);
    assert!(unused.iter().any(|item| {
        item.kind == AnalysisUnusedItemKind::Constant && item.name == "dead_const"
    }));
    assert!(
        unused
            .iter()
            .any(|item| item.kind == AnalysisUnusedItemKind::Function && item.name == "dead_fn")
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_exposes_unused_bindings() {
    let root = std::env::temp_dir().join(format!(
        "kern_unused_bindings_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = concat!(
        "fn helper(_: i32, unused_param: i32, used_param: i32) i32 {\n",
        "    let unused_local = used_param;\n",
        "    return used_param;\n",
        "}\n",
        "extern fn main() i32 { return helper(1, 2, 3); }\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let artifact = driver.analyze_artifact(main.to_str().unwrap(), &SourceOverrides::new());
    let unused = artifact.unused_bindings();

    assert_eq!(unused.len(), 2);
    assert!(unused.iter().any(|binding| {
        binding.kind == AnalysisUnusedBindingKind::Parameter && binding.name == "unused_param"
    }));
    assert!(unused.iter().any(|binding| {
        binding.kind == AnalysisUnusedBindingKind::Variable && binding.name == "unused_local"
    }));
    assert!(!unused.iter().any(|binding| binding.name == "_"));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_exposes_dead_stores() {
    let root = std::env::temp_dir().join(format!(
        "kern_dead_store_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    value = seed + 2;\n",
        "    return value;\n",
        "}\n",
        "extern fn main() i32 { return helper(1); }\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let artifact = driver.analyze_artifact(main.to_str().unwrap(), &SourceOverrides::new());
    let dead_stores = artifact.dead_stores();

    assert_eq!(dead_stores.len(), 2);
    assert!(dead_stores.iter().all(|store| store.name == "value"));
    assert!(
        dead_stores
            .iter()
            .any(|store| { store.kind == AnalysisDeadStoreKind::Initializer })
    );
    assert!(
        dead_stores
            .iter()
            .any(|store| { store.kind == AnalysisDeadStoreKind::Assignment })
    );
    for dead_store in &dead_stores {
        assert!(artifact.flow_owners().iter().any(|owner| {
            owner
                .bindings
                .iter()
                .any(|binding| binding.id == dead_store.binding_id)
        }));
    }

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn lowering_replaces_dead_pure_initializer_with_undef() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_dead_init_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    return value;\n",
        "}\n",
        "extern fn main() i32 { return helper(1); }\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let mut session = Session::new();
    let mut ctx = driver
        .analyze(&mut session, main.to_str().unwrap())
        .expect("expected sema context");
    let module = driver
        .lower_module(&mut ctx)
        .expect("expected lowered module");
    let helper = module
        .functions
        .iter()
        .find(|function| function.name.contains("helper"))
        .expect("expected helper function");
    let body = helper.body.as_ref().expect("expected helper body");
    let value_let = body
        .stmts
        .iter()
        .find_map(|stmt| match stmt {
            MastStmt::Let { name, init, .. } if ctx.resolve(*name) == "value" => Some(init),
            _ => None,
        })
        .expect("expected lowered value binding");

    assert!(matches!(value_let.kind, MastExprKind::Undef));
    assert!(body.stmts.iter().all(|stmt| match stmt {
        MastStmt::Let { name, .. } => !ctx.resolve(*name).starts_with("__match_target_"),
        _ => true,
    }));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn lowering_prunes_dead_pure_assignment_statement() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_dead_assign_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    value = seed + 2;\n",
        "    return value;\n",
        "}\n",
        "extern fn main() i32 { return helper(1); }\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let mut session = Session::new();
    let mut ctx = driver
        .analyze(&mut session, main.to_str().unwrap())
        .expect("expected sema context");
    let module = driver
        .lower_module(&mut ctx)
        .expect("expected lowered module");
    let helper = module
        .functions
        .iter()
        .find(|function| function.name.contains("helper"))
        .expect("expected helper function");
    let body = helper.body.as_ref().expect("expected helper body");
    let assignment_count = body
        .stmts
        .iter()
        .filter(|stmt| {
            matches!(
                stmt,
                MastStmt::Expr(expr)
                    if matches!(expr.kind, MastExprKind::Assign { .. })
            )
        })
        .count();

    assert_eq!(assignment_count, 1);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn lowering_prunes_dead_pure_assignment_in_for_init_clause() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_dead_for_init_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    for (value = seed + 1; false; ) {\n",
        "    }\n",
        "    value = seed + 2;\n",
        "    return value;\n",
        "}\n",
        "extern fn main() i32 { return helper(1); }\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let mut session = Session::new();
    let mut ctx = driver
        .analyze(&mut session, main.to_str().unwrap())
        .expect("expected sema context");
    let module = driver
        .lower_module(&mut ctx)
        .expect("expected lowered module");
    let helper = module
        .functions
        .iter()
        .find(|function| function.name.contains("helper"))
        .expect("expected helper function");
    let body = helper.body.as_ref().expect("expected helper body");

    assert_eq!(count_assignments_in_block(body), 1);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn lowering_prunes_dead_pure_assignment_in_for_post_clause() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_dead_for_post_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = concat!(
        "fn helper(limit: usize, seed: i32) i32 {\n",
        "    let mut i = usize.{0};\n",
        "    let mut value = seed;\n",
        "    for (; i < limit; value = seed + 1) {\n",
        "        i += usize.{1};\n",
        "    }\n",
        "    value = seed + 2;\n",
        "    return value;\n",
        "}\n",
        "extern fn main() i32 { return helper(usize.{3}, 1); }\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let mut session = Session::new();
    let mut ctx = driver
        .analyze(&mut session, main.to_str().unwrap())
        .expect("expected sema context");
    let module = driver
        .lower_module(&mut ctx)
        .expect("expected lowered module");
    let helper = module
        .functions
        .iter()
        .find(|function| function.name.contains("helper"))
        .expect("expected helper function");
    let body = helper.body.as_ref().expect("expected helper body");

    assert_eq!(count_assignments_in_block(body), 2);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn lowering_prunes_dead_pure_assignment_in_ignored_let_initializer() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_dead_ignore_init_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    let _ = value = seed + 1;\n",
        "    value = seed + 2;\n",
        "    return value;\n",
        "}\n",
        "extern fn main() i32 { return helper(1); }\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let mut session = Session::new();
    let mut ctx = driver
        .analyze(&mut session, main.to_str().unwrap())
        .expect("expected sema context");
    let module = driver
        .lower_module(&mut ctx)
        .expect("expected lowered module");
    let helper = module
        .functions
        .iter()
        .find(|function| function.name.contains("helper"))
        .expect("expected helper function");
    let body = helper.body.as_ref().expect("expected helper body");

    assert_eq!(count_assignments_in_block(body), 1);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn lowering_copy_propagates_identifier_use_from_immutable_source_chain() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_copy_prop_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let local = seed;\n",
        "    let value = local;\n",
        "    return value;\n",
        "}\n",
        "extern fn main() i32 { return helper(1); }\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let mut session = Session::new();
    let mut ctx = driver
        .analyze(&mut session, main.to_str().unwrap())
        .expect("expected sema context");
    let module = driver
        .lower_module(&mut ctx)
        .expect("expected lowered module");
    let helper = module
        .functions
        .iter()
        .find(|function| function.name.contains("helper"))
        .expect("expected helper function");
    let body = helper.body.as_ref().expect("expected helper body");
    assert!(body.stmts.iter().all(|stmt| match stmt {
        MastStmt::Let { name, .. } => {
            let name = ctx.resolve(*name);
            name != "local" && name != "value"
        }
        _ => true,
    }));
    let return_expr = body
        .stmts
        .iter()
        .find_map(|stmt| match stmt {
            MastStmt::Expr(expr) if matches!(expr.kind, MastExprKind::Return(_)) => Some(expr),
            _ => None,
        })
        .expect("expected return expression");

    let returned = match &return_expr.kind {
        MastExprKind::Return(Some(value)) => value,
        _ => panic!("expected return with value"),
    };
    match &returned.kind {
        MastExprKind::Var(name) => assert_eq!(ctx.resolve(*name), "seed"),
        other => panic!("expected propagated variable return, got {:?}", other),
    }

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn lowering_forwards_pure_value_binding_without_emitting_local_let() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_value_forward_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let value = seed + 1;\n",
        "    return value;\n",
        "}\n",
        "extern fn main() i32 { return helper(1); }\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let mut session = Session::new();
    let mut ctx = driver
        .analyze(&mut session, main.to_str().unwrap())
        .expect("expected sema context");
    let module = driver
        .lower_module(&mut ctx)
        .expect("expected lowered module");
    let helper = module
        .functions
        .iter()
        .find(|function| function.name.contains("helper"))
        .expect("expected helper function");
    let body = helper.body.as_ref().expect("expected helper body");

    assert!(
        body.stmts.iter().all(|stmt| match stmt {
            MastStmt::Let { name, .. } => ctx.resolve(*name) != "value",
            _ => true,
        }),
        "unexpected helper body stmts: {:?}",
        body.stmts
    );

    let return_expr = body
        .stmts
        .iter()
        .find_map(|stmt| match stmt {
            MastStmt::Expr(expr) if matches!(expr.kind, MastExprKind::Return(_)) => Some(expr),
            _ => None,
        })
        .expect("expected return expression");
    let returned = match &return_expr.kind {
        MastExprKind::Return(Some(value)) => value,
        _ => panic!("expected return with value"),
    };
    match &returned.kind {
        MastExprKind::Binary { lhs, rhs, .. } => {
            match &lhs.kind {
                MastExprKind::Var(name) => assert_eq!(ctx.resolve(*name), "seed"),
                other => panic!("expected seed lhs, got {:?}", other),
            }
            assert!(matches!(rhs.kind, MastExprKind::Integer(1)));
        }
        other => panic!("expected forwarded binary value, got {:?}", other),
    }

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn lowering_does_not_forward_value_binding_that_depends_on_mutable_local() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_value_forward_mut_dep_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut current = seed;\n",
        "    let value = current + 1;\n",
        "    current = seed + 2;\n",
        "    return value;\n",
        "}\n",
        "extern fn main() i32 { return helper(1); }\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let mut session = Session::new();
    let mut ctx = driver
        .analyze(&mut session, main.to_str().unwrap())
        .expect("expected sema context");
    let module = driver
        .lower_module(&mut ctx)
        .expect("expected lowered module");
    let helper = module
        .functions
        .iter()
        .find(|function| function.name.contains("helper"))
        .expect("expected helper function");
    let body = helper.body.as_ref().expect("expected helper body");

    assert!(body.stmts.iter().any(|stmt| match stmt {
        MastStmt::Let { name, .. } => ctx.resolve(*name) == "value",
        _ => false,
    }));

    let return_expr = body
        .stmts
        .iter()
        .find_map(|stmt| match stmt {
            MastStmt::Expr(expr) if matches!(expr.kind, MastExprKind::Return(_)) => Some(expr),
            _ => None,
        })
        .expect("expected return expression");
    let returned = match &return_expr.kind {
        MastExprKind::Return(Some(value)) => value,
        _ => panic!("expected return with value"),
    };
    match &returned.kind {
        MastExprKind::Var(name) => assert_eq!(ctx.resolve(*name), "value"),
        other => panic!("expected preserved local binding return, got {:?}", other),
    }

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn lowering_elides_unused_immutable_pure_binding() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_elide_unused_binding_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let unused = seed + 1;\n",
        "    return seed;\n",
        "}\n",
        "extern fn main() i32 { return helper(1); }\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let mut session = Session::new();
    let mut ctx = driver
        .analyze(&mut session, main.to_str().unwrap())
        .expect("expected sema context");
    let module = driver
        .lower_module(&mut ctx)
        .expect("expected lowered module");
    let helper = module
        .functions
        .iter()
        .find(|function| function.name.contains("helper"))
        .expect("expected helper function");
    let body = helper.body.as_ref().expect("expected helper body");

    assert!(body.stmts.iter().all(|stmt| match stmt {
        MastStmt::Let { name, .. } => ctx.resolve(*name) != "unused",
        _ => true,
    }));

    let return_expr = body
        .stmts
        .iter()
        .find_map(|stmt| match stmt {
            MastStmt::Expr(expr) if matches!(expr.kind, MastExprKind::Return(_)) => Some(expr),
            _ => None,
        })
        .expect("expected return expression");
    let returned = match &return_expr.kind {
        MastExprKind::Return(Some(value)) => value,
        _ => panic!("expected return with value"),
    };
    match &returned.kind {
        MastExprKind::Var(name) => assert_eq!(ctx.resolve(*name), "seed"),
        other => panic!("expected seed return, got {:?}", other),
    }

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn lowering_std_hello_world_prunes_unreachable_file_methods() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_std_hello_world_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(
        &main,
        concat!(
            "use std.io;\n",
            "\n",
            "extern fn main(argc: i32, argv: **u8) i32 {\n",
            "    let _ = argc;\n",
            "    let _ = argv;\n",
            "    io.println(\"hello, {}!\", .{\"world\",});\n",
            "    return 0;\n",
            "}\n",
        ),
    )
    .unwrap();

    let mut options = CompileOptions {
        library_bundle: kernc_utils::config::LibraryBundle::Std,
        ..CompileOptions::default()
    };
    kernc_utils::config::inject_default_library_aliases(&mut options);
    kernc_utils::config::inject_driver_condition_defines(&mut options);
    let driver = CompilerDriver::new(options);
    let structure = driver
        .analyze_compile_structure(main.to_str().unwrap(), &SourceOverrides::new())
        .expect("expected compile structure");
    let mut session = structure.session.clone();
    let mut ctx = driver.build_sema_context(&mut session);
    ctx.restore_structure(structure.snapshot);
    let body_pipeline = driver
        .run_body_pipeline_with_report(&mut ctx)
        .expect("expected body pipeline");
    let lowered_roots = body_pipeline
        .lowered_module_items
        .iter()
        .map(|&def_id| ctx.get_export_name(def_id, &[]))
        .collect::<std::collections::BTreeSet<_>>();
    let module = driver
        .lower_module_with_flow(
            &mut ctx,
            &body_pipeline.flow_lowering_hints,
            &body_pipeline.lowered_module_items,
        )
        .expect("expected lowered module");
    let lowered_functions = module
        .functions
        .iter()
        .filter(|function| function.body.is_some())
        .map(|function| function.name.clone())
        .collect::<std::collections::BTreeSet<_>>();

    assert!(
        !lowered_roots.contains("_K3std2fs4file8PmutFile4read"),
        "unexpected lowered roots: {lowered_roots:#?}"
    );
    assert!(
        !lowered_roots.contains("_K3std2fs4file8PmutFile5close"),
        "unexpected lowered roots: {lowered_roots:#?}"
    );
    if !cfg!(windows) {
        assert!(
            !lowered_functions.contains("_K3std2fs4file8PmutFile4read"),
            "unexpected lowered functions: {lowered_functions:#?}"
        );
        assert!(
            !lowered_functions.contains("_K3std2fs4file8PmutFile5close"),
            "unexpected lowered functions: {lowered_functions:#?}"
        );
    }

    let _ = fs::remove_dir_all(&root);
}
