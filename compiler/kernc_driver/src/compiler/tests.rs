use super::{
    AnalysisCallKind, AnalysisDeadStoreKind, AnalysisFlowBindingKind, AnalysisFlowCfgEdgeKind,
    AnalysisFlowCfgNodeKind, AnalysisFlowDefinitionKind, AnalysisFlowDefinitionRef,
    AnalysisFlowOwnerKind, AnalysisFlowRegionKind, AnalysisFlowResolvedUseKind,
    AnalysisUnusedBindingKind, AnalysisUnusedItemKind, CancellationToken, CompilerDriver,
    SourceOverrides,
};
use kernc_mast::{MastBlock, MastExpr, MastExprKind, MastStmt};
use kernc_utils::Session;
use kernc_utils::config::{CompileOptions, DriverMode, LtoMode};
use std::fs;
use std::process::Command;

mod cache;
mod calls;
mod completion;
mod diagnostics;
mod flow;
mod lowering;

#[test]
fn canceled_analysis_artifact_stops_before_driver_work() {
    let root = std::env::temp_dir().join(format!(
        "kern_driver_canceled_artifact_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    fs::write(&main, "fn main() i32 { return 1; }\n").unwrap();
    let driver = CompilerDriver::new(CompileOptions::default());
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let result = driver.analyze_artifact(
        main.to_str().unwrap(),
        &SourceOverrides::new(),
        &cancellation,
    );

    assert!(result.is_err());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_cancellation_reaches_typeck_body_worklist() {
    let root = std::env::temp_dir().join(format!(
        "kern_driver_typeck_canceled_artifact_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let mut source = String::new();
    source.push_str("fn main() i32 { return f0(); }\n");
    for index in 0..64 {
        source.push_str(&format!("fn f{index}() i32 {{ return {index}; }}\n"));
    }
    fs::write(&main, source).unwrap();
    let driver = CompilerDriver::new(CompileOptions::default());
    let structure = driver
        .analyze_structure(main.to_str().unwrap(), &SourceOverrides::new())
        .expect("large test source should structure-check");
    let cancellation = CancellationToken::with_check_budget_for_testing(6);

    let result = driver.analyze_artifact_from_structure(&structure, &cancellation);

    assert!(result.is_err());
    assert!(cancellation.is_canceled());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_cancellation_reaches_typeck_expression_traversal() {
    let root = temp_test_dir("kern_driver_typeck_expr_canceled_artifact");
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let mut source = String::from("fn main() i32 {\n");
    for index in 0..128 {
        source.push_str(&format!("    let value{index} = {index};\n"));
    }
    source.push_str("    return value127;\n}\n");
    fs::write(&main, source).unwrap();
    let driver = CompilerDriver::new(CompileOptions::default());
    let structure = driver
        .analyze_structure(main.to_str().unwrap(), &SourceOverrides::new())
        .expect("nested expression source should structure-check");
    let cancellation = CancellationToken::with_check_budget_for_testing(10);

    let result = driver.analyze_artifact_from_structure(&structure, &cancellation);

    assert!(result.is_err());
    assert!(cancellation.is_canceled());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_cancellation_reaches_flow_collection() {
    let root = temp_test_dir("kern_driver_flow_canceled_artifact");
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let mut source = String::new();
    source.push_str("fn main() i32 { return f0(); }\n");
    for index in 0..64 {
        source.push_str(&format!(
            "fn f{index}() i32 {{ let value = {index}; return value; }}\n"
        ));
    }
    fs::write(&main, source).unwrap();
    let driver = CompilerDriver::new(CompileOptions::default());
    let structure = driver
        .analyze_structure(main.to_str().unwrap(), &SourceOverrides::new())
        .expect("large test source should structure-check");
    let cancellation = CancellationToken::with_check_budget_for_testing(140);

    let result = driver.analyze_artifact_from_structure(&structure, &cancellation);

    assert!(result.is_err());
    assert!(cancellation.is_canceled());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_cancellation_reaches_body_diagnostics_after_flow() {
    let root = temp_test_dir("kern_driver_body_diagnostics_canceled_artifact");
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let mut source = String::new();
    source.push_str("fn main() i32 { return f0(); }\n");
    for index in 0..128 {
        source.push_str(&format!(
            "fn f{index}() i32 {{ let unused_{index} = {index}; return {index}; }}\n"
        ));
    }
    fs::write(&main, source).unwrap();
    let driver = CompilerDriver::new(CompileOptions::default());
    let structure = driver
        .analyze_structure(main.to_str().unwrap(), &SourceOverrides::new())
        .expect("large test source should structure-check");
    let cancellation = CancellationToken::with_check_budget_for_testing(260);

    let result = driver.analyze_artifact_from_structure(&structure, &cancellation);

    assert!(result.is_err());
    assert!(cancellation.is_canceled());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn navigation_artifact_cancellation_reaches_typeck_body_worklist() {
    let root = std::env::temp_dir().join(format!(
        "kern_driver_typeck_canceled_navigation_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let mut source = String::new();
    source.push_str("fn main() i32 { return f0(); }\n");
    for index in 0..64 {
        source.push_str(&format!("fn f{index}() i32 {{ return {index}; }}\n"));
    }
    fs::write(&main, source).unwrap();
    let driver = CompilerDriver::new(CompileOptions::default());
    let structure = driver
        .analyze_structure(main.to_str().unwrap(), &SourceOverrides::new())
        .expect("large test source should structure-check");
    let cancellation = CancellationToken::with_check_budget_for_testing(6);

    let result = driver.analyze_navigation_artifact_from_structure(&structure, &cancellation);

    assert!(result.is_err());
    assert!(cancellation.is_canceled());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn parse_modules_cancellation_reaches_module_loader() {
    let root = std::env::temp_dir().join(format!(
        "kern_driver_module_loader_canceled_parse_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let mut source = String::new();
    for index in 0..64 {
        source.push_str(&format!("mod m{index};\n"));
        fs::write(
            root.join(format!("m{index}.kn")),
            format!("fn f{index}() i32 {{ return {index}; }}\n"),
        )
        .unwrap();
    }
    fs::write(&main, source).unwrap();
    let driver = CompilerDriver::new(CompileOptions::default());
    let cancellation = CancellationToken::with_check_budget_for_testing(8);

    let result = driver.parse_modules(
        main.to_str().unwrap(),
        &SourceOverrides::new(),
        &cancellation,
    );

    assert!(result.is_err());
    assert!(cancellation.is_canceled());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_cancellation_reaches_structure_module_loader() {
    let root = std::env::temp_dir().join(format!(
        "kern_driver_module_loader_canceled_analysis_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let mut source = String::new();
    for index in 0..64 {
        source.push_str(&format!("mod m{index};\n"));
        fs::write(
            root.join(format!("m{index}.kn")),
            format!("fn f{index}() i32 {{ return {index}; }}\n"),
        )
        .unwrap();
    }
    fs::write(&main, source).unwrap();
    let driver = CompilerDriver::new(CompileOptions::default());
    let cancellation = CancellationToken::with_check_budget_for_testing(8);

    let result = driver.analyze_artifact(
        main.to_str().unwrap(),
        &SourceOverrides::new(),
        &cancellation,
    );

    assert!(result.is_err());
    assert!(cancellation.is_canceled());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn parse_modules_cancellation_reaches_parser_loop_without_poisoning_cache() {
    let root = temp_test_dir("kern_driver_parser_canceled");
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let mut source = String::new();
    for index in 0..128 {
        source.push_str(&format!("fn f{index}() i32 {{ return {index}; }}\n"));
    }
    fs::write(&main, source).unwrap();
    let driver = CompilerDriver::new(CompileOptions::default());
    let cancellation = CancellationToken::with_check_budget_for_testing(4);

    let canceled = driver.parse_modules(
        main.to_str().unwrap(),
        &SourceOverrides::new(),
        &cancellation,
    );

    assert!(canceled.is_err());
    assert!(cancellation.is_canceled());

    let parsed = driver
        .parse_modules(
            main.to_str().unwrap(),
            &SourceOverrides::new(),
            &CancellationToken::new(),
        )
        .expect("fresh cancellation token cannot be canceled");

    assert!(parsed.is_some());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn collected_structure_cancellation_reaches_collector_loop() {
    let root = temp_test_dir("kern_driver_collector_canceled");
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let mut source = String::new();
    for index in 0..64 {
        source.push_str(&format!("fn f{index}() i32 {{ return {index}; }}\n"));
    }
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let mut session = Session::new();
    session.apply_options(&CompileOptions::default());
    let mut ctx = driver.build_sema_context(&mut session);
    let loaded = driver
        .load_asts(&mut ctx, main.to_str().unwrap(), true)
        .expect("large test source should load");
    let cancellation = CancellationToken::with_check_budget_for_testing(3);

    let result = driver.build_collected_structure_from_context_cancelable(
        &mut ctx,
        loaded.asts,
        &cancellation,
    );

    assert!(result.is_err());
    assert!(cancellation.is_canceled());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn imported_structure_cancellation_reaches_import_resolver_loop() {
    let root = temp_test_dir("kern_driver_import_resolver_canceled");
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let mut source = String::new();
    for index in 0..64 {
        source.push_str(&format!("use helper.f{index};\n"));
    }
    source.push_str("mod helper;\n");
    fs::write(&main, source).unwrap();
    let mut helper = String::new();
    for index in 0..64 {
        helper.push_str(&format!("fn f{index}() i32 {{ return {index}; }}\n"));
    }
    fs::write(root.join("helper.kn"), helper).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let mut session = Session::new();
    session.apply_options(&CompileOptions::default());
    let mut ctx = driver.build_sema_context(&mut session);
    let loaded = driver
        .load_asts(&mut ctx, main.to_str().unwrap(), true)
        .expect("large test source should load");
    let collected = driver
        .build_collected_structure_from_context_cancelable(
            &mut ctx,
            loaded.asts,
            &CancellationToken::new(),
        )
        .expect("fresh cancellation token cannot be canceled")
        .expect("large test source should collect");
    let cancellation = CancellationToken::with_check_budget_for_testing(3);

    let result = driver.build_imported_structure_cancelable(&collected, &cancellation);

    assert!(result.is_err());
    assert!(cancellation.is_canceled());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn structure_cancellation_reaches_type_resolver_loop_from_imported_cache() {
    let root = temp_test_dir("kern_driver_type_resolver_canceled");
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let mut source = String::new();
    for index in 0..64 {
        source.push_str(&format!("type Alias{index} = i32;\n"));
        source.push_str(&format!(
            "fn f{index}() Alias{index} {{ return {index}; }}\n"
        ));
    }
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let imported = driver
        .analyze_imported_structure(
            main.to_str().unwrap(),
            &SourceOverrides::new(),
            &CancellationToken::new(),
        )
        .expect("fresh cancellation token cannot be canceled")
        .expect("large test source should import-check");
    let cancellation = CancellationToken::with_check_budget_for_testing(3);

    let result = driver.analyze_structure_cancelable(
        main.to_str().unwrap(),
        &SourceOverrides::new(),
        &cancellation,
    );

    assert!(result.is_err());
    assert!(cancellation.is_canceled());
    drop(imported);
    let _ = fs::remove_dir_all(&root);
}

fn temp_test_dir(prefix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "{prefix}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

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
