//! Lowering integration tests.
//!
//! These tests exercise driver-level lowering output and ensure semantic
//! analysis artifacts feed the lower/MIR/codegen stages correctly.

use super::*;
use kernc_mono::MonoId;

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
    let main = root.join("main.kn");
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
fn lowering_cancellation_reaches_root_expression_traversal() {
    let root = temp_test_dir("kern_lower_expr_canceled");
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let mut source = String::from("fn helper() i32 {\n    let mut value = 0;\n");
    for index in 0..128 {
        source.push_str(&format!("    value = value + {index};\n"));
    }
    source.push_str("    return value;\n}\n");
    source.push_str("extern fn main() i32 { return helper(); }\n");
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let mut session = Session::new();
    let mut ctx = driver
        .analyze(&mut session, main.to_str().unwrap())
        .expect("expected sema context");
    let body_pipeline = driver
        .run_body_pipeline_with_report(&mut ctx)
        .expect("expected body pipeline");
    let cancellation = CancellationToken::with_check_budget_for_testing(6);

    let result = driver.lower_module_with_flow_report_cancelable(
        &mut ctx,
        &body_pipeline.flow_lowering_hints,
        &body_pipeline.lowered_module_items,
        &cancellation,
    );

    assert!(result.is_err());
    assert!(cancellation.is_canceled());
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
    let main = root.join("main.kn");
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
fn lowering_prunes_dead_pure_assignment_before_never_entered_while() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_dead_while_preassign_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    while (false) {}\n",
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
fn lowering_keeps_assignment_in_while_body() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_keep_while_body_assign_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn helper(limit: usize, seed: i32) i32 {\n",
        "    let mut i = 0usize;\n",
        "    let mut value = seed;\n",
        "    while (i < limit) {\n",
        "        i += 1usize;\n",
        "        value = seed + 1;\n",
        "    }\n",
        "    value = seed + 2;\n",
        "    return value;\n",
        "}\n",
        "extern fn main() i32 { return helper(3usize, 1); }\n",
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
    let main = root.join("main.kn");
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
    let main = root.join("main.kn");
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
    let main = root.join("main.kn");
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
    let main = root.join("main.kn");
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
    let main = root.join("main.kn");
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
fn lowering_expands_optional_propagate_into_match_like_early_return() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_propagate_option_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn helper(input: ?i32) ?i32 {\n",
        "    let value = input.?;\n",
        "    return ?i32.{ Some: value + 1 };\n",
        "}\n",
        "extern fn main() i32 {\n",
        "    let _ = helper(?i32.{ Some: 1 });\n",
        "    return 0;\n",
        "}\n",
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
    let propagate_init = body
        .stmts
        .iter()
        .find_map(|stmt| match stmt {
            MastStmt::Let { name, init, .. } if ctx.resolve(*name) == "value" => Some(init),
            _ => None,
        })
        .expect("expected lowered propagate binding");

    let MastExprKind::Block(block) = &propagate_init.kind else {
        panic!(
            "expected propagate lowering block, got {:?}",
            propagate_init.kind
        );
    };
    assert_eq!(
        block.stmts.len(),
        1,
        "unexpected propagate block stmts: {:?}",
        block.stmts
    );
    let MastStmt::Let { name, .. } = &block.stmts[0] else {
        panic!("expected hidden target binding in propagate block");
    };
    assert!(
        ctx.resolve(*name).starts_with("__match_target_"),
        "unexpected propagate temp binding name: {}",
        ctx.resolve(*name)
    );

    let Some(result_expr) = &block.result else {
        panic!("expected propagate block result");
    };
    let MastExprKind::If {
        then_branch,
        else_branch,
        ..
    } = &result_expr.kind
    else {
        panic!(
            "expected propagate block to end in if, got {:?}",
            result_expr.kind
        );
    };
    assert!(
        matches!(
            then_branch.result.as_deref().map(|expr| &expr.kind),
            Some(MastExprKind::FieldAccess { .. })
        ),
        "expected propagate success branch to extract enum payload, got {:?}",
        then_branch.result
    );

    let else_branch = else_branch
        .as_ref()
        .expect("expected propagate failure branch");
    let else_result = else_branch
        .result
        .as_deref()
        .expect("expected propagate failure result");
    match &else_result.kind {
        MastExprKind::Return(Some(returned)) => {
            assert!(
                matches!(returned.kind, MastExprKind::DataInit { .. }),
                "expected optional propagate failure to return builtin optional constructor, got {:?}",
                returned.kind
            );
        }
        other => panic!(
            "expected early return in propagate failure branch, got {:?}",
            other
        ),
    }

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn lowering_preserves_return_temp_when_scope_has_defer() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_return_defer_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "struct Guard {\n",
        "    ptr: &mut i32,\n",
        "};\n",
        "impl &mut Guard {\n",
        "    fn deinit() void {\n",
        "        self.ptr.* = 2;\n",
        "    }\n",
        "}\n",
        "fn helper() i32 {\n",
        "    let mut state = 1i32;\n",
        "    let mut guard = Guard.{ ptr: state..& };\n",
        "    defer guard..&.deinit();\n",
        "    return state;\n",
        "}\n",
        "extern fn main() i32 { return helper(); }\n",
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

    let return_expr = body
        .stmts
        .iter()
        .find_map(|stmt| match stmt {
            MastStmt::Expr(expr) if matches!(expr.kind, MastExprKind::Return(_)) => Some(expr),
            MastStmt::Expr(expr) => match &expr.kind {
                MastExprKind::Block(block) => block.stmts.iter().find_map(|stmt| match stmt {
                    MastStmt::Expr(expr) if matches!(expr.kind, MastExprKind::Return(_)) => {
                        Some(expr)
                    }
                    _ => None,
                }),
                _ => None,
            },
            _ => None,
        })
        .expect("expected lowered return expression");

    let returned = match &return_expr.kind {
        MastExprKind::Return(Some(value)) => value,
        _ => panic!("expected return with value, got {:?}", return_expr),
    };
    match &returned.kind {
        MastExprKind::Var(name) => {
            assert!(
                ctx.resolve(*name).starts_with("__ret_tmp_"),
                "expected deferred return to use temp, got {:?} in {:?}",
                ctx.resolve(*name),
                body
            );
        }
        other => panic!(
            "expected deferred return temp, got {:?} in {:?}",
            other, body
        ),
    }

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn mir_lower_preserves_deferred_return_value_snapshot() {
    let root = std::env::temp_dir().join(format!(
        "kern_mir_lower_return_defer_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "struct Guard {\n",
        "    ptr: &mut i32,\n",
        "};\n",
        "impl &mut Guard {\n",
        "    fn deinit() void {\n",
        "        self.ptr.* = 2;\n",
        "    }\n",
        "}\n",
        "fn helper() i32 {\n",
        "    let mut state = 1i32;\n",
        "    let mut guard = Guard.{ ptr: state..& };\n",
        "    defer guard..&.deinit();\n",
        "    return state;\n",
        "}\n",
        "extern fn main() i32 { return helper(); }\n",
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
    let report = kernc_mir_lower::build_from_mast_unoptimized(&module);
    let helper = report
        .module
        .functions
        .iter()
        .find(|function| function.name.contains("helper"))
        .expect("expected helper function");
    let body = helper.body.as_ref().expect("expected helper MIR body");
    let ret_local = body
        .locals
        .iter()
        .find(|local| ctx.resolve(local.name).starts_with("__ret_tmp_"))
        .expect("expected deferred return temp local");
    let ret_temp_initialized = body.blocks.iter().any(|block| {
        block.instructions.iter().any(|instruction| {
            matches!(
                &instruction.kind,
                kernc_mir::MirInstruction::Let {
                    place: kernc_mir::MirPlace::Local(local),
                    init: kernc_mir::MirRvalue::Use(kernc_mir::MirOperand::Local(_)),
                } if *local == ret_local.id
            )
        })
    });
    assert!(
        ret_temp_initialized,
        "expected deferred return temp to be initialized before defers, got {:?}",
        body
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn mir_passes_preserve_deferred_return_value_snapshot() {
    let root = std::env::temp_dir().join(format!(
        "kern_mir_pass_return_defer_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "struct Guard {\n",
        "    ptr: &mut i32,\n",
        "};\n",
        "impl &mut Guard {\n",
        "    fn deinit() void {\n",
        "        self.ptr.* = 2;\n",
        "    }\n",
        "}\n",
        "fn helper() i32 {\n",
        "    let mut state = 1i32;\n",
        "    let mut guard = Guard.{ ptr: state..& };\n",
        "    defer guard..&.deinit();\n",
        "    return state;\n",
        "}\n",
        "extern fn main() i32 { return helper(); }\n",
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
    let report = kernc_mir_lower::build_from_mast(&module);
    let helper = report
        .module
        .functions
        .iter()
        .find(|function| function.name.contains("helper"))
        .expect("expected helper function");
    let body = helper.body.as_ref().expect("expected helper MIR body");
    let ret_local = body
        .locals
        .iter()
        .find(|local| ctx.resolve(local.name).starts_with("__ret_tmp_"))
        .expect("expected deferred return temp local");
    let returned_temp = body.blocks.iter().any(|block| {
        matches!(
            &block.terminator.kind,
            kernc_mir::MirTerminator::Return(Some(kernc_mir::MirRvalue::Use(
                kernc_mir::MirOperand::Local(local),
            ))) if *local == ret_local.id
        )
    });
    assert!(
        returned_temp,
        "expected MIR pass pipeline to preserve deferred return temp, got {:?}",
        body
    );

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
    let main = root.join("main.kn");
    fs::write(
        &main,
        concat!(
            "use std.io;\n",
            "\n",
            "extern fn main(argc: i32, argv: &&u8) i32 {\n",
            "    let _ = argc;\n",
            "    let _ = argv;\n",
            "    \"hello, {}!\".fmt(.{\"world\"}).println();\n",
            "    return 0;\n",
            "}\n",
        ),
    )
    .unwrap();

    let mut options = CompileOptions {
        library_bundle: kernc_utils::config::LibraryBundle::Std,
        ..CompileOptions::default()
    };
    kernc_utils::config::apply_configured_library_aliases(&mut options);
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

#[test]
fn lowering_inlines_simple_inline_helper() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_inline_simple_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "#[inline]\n",
        "fn add_one(x: i32) i32 {\n",
        "    let y = x + 1;\n",
        "    return y;\n",
        "}\n",
        "fn helper(seed: i32) i32 {\n",
        "    return add_one(seed);\n",
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
    let add_one = module
        .functions
        .iter()
        .find(|function| function.name.contains("add_one"))
        .expect("expected add_one function");
    let helper = module
        .functions
        .iter()
        .find(|function| function.name.contains("helper"))
        .expect("expected helper function");
    let helper_body = helper.body.as_ref().expect("expected helper body");

    assert_eq!(count_calls_to_block(helper_body, add_one.id), 0);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn lowering_inlines_guard_return_inline_helper() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_inline_guard_return_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "#[inline]\n",
        "fn pick_positive(x: i32) i32 {\n",
        "    if (x > 0) {\n",
        "        return x;\n",
        "    }\n",
        "    return 0;\n",
        "}\n",
        "fn helper(seed: i32) i32 {\n",
        "    return pick_positive(seed);\n",
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
    let pick_positive = module
        .functions
        .iter()
        .find(|function| function.name.contains("pick_positive"))
        .expect("expected pick_positive function");
    let helper = module
        .functions
        .iter()
        .find(|function| function.name.contains("helper"))
        .expect("expected helper function");
    let helper_body = helper.body.as_ref().expect("expected helper body");

    assert_eq!(count_calls_to_block(helper_body, pick_positive.id), 0);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn lowering_keeps_fallthrough_inline_helper_as_call() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_inline_fallthrough_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "#[inline]\n",
        "fn pick_positive(x: i32) i32 {\n",
        "    if (x > 0) {\n",
        "        return x;\n",
        "    }\n",
        "    if (x < 0) {\n",
        "        return -x;\n",
        "    }\n",
        "    return 0;\n",
        "}\n",
        "fn helper(seed: i32) i32 {\n",
        "    return pick_positive(seed);\n",
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
    let pick_positive = module
        .functions
        .iter()
        .find(|function| function.name.contains("pick_positive"))
        .expect("expected pick_positive function");
    let helper = module
        .functions
        .iter()
        .find(|function| function.name.contains("helper"))
        .expect("expected helper function");
    let helper_body = helper.body.as_ref().expect("expected helper body");

    assert_eq!(count_calls_to_block(helper_body, pick_positive.id), 1);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn lowering_inlines_plain_inline_helper() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_plain_inline_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "#[inline]\n",
        "fn add_one(x: i32) i32 {\n",
        "    let y = x + 1;\n",
        "    return y;\n",
        "}\n",
        "fn helper(seed: i32) i32 {\n",
        "    return add_one(seed);\n",
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
    let add_one = module
        .functions
        .iter()
        .find(|function| function.name.contains("add_one"))
        .expect("expected add_one function");
    let helper = module
        .functions
        .iter()
        .find(|function| function.name.contains("helper"))
        .expect("expected helper function");
    let helper_body = helper.body.as_ref().expect("expected helper body");

    assert_eq!(count_calls_to_block(helper_body, add_one.id), 0);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn lowering_respects_visibility_for_linkage() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_linkage_visibility_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = concat!(
        "fn private_helper() i32 { return 1; }\n",
        "pub.. fn parent_helper() i32 { return 3; }\n",
        "pub fn public_helper() i32 { return private_helper(); }\n",
        "#[export_name(\"bridge\")]\n",
        "fn named_export() i32 { return 2; }\n",
        "extern fn main() i32 { return public_helper() + named_export() + parent_helper(); }\n",
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

    let private_helper = module
        .functions
        .iter()
        .find(|function| function.name.contains("private_helper"))
        .expect("expected private helper");
    let public_helper = module
        .functions
        .iter()
        .find(|function| function.name.contains("public_helper"))
        .expect("expected public helper");
    let parent_helper = module
        .functions
        .iter()
        .find(|function| function.name.contains("parent_helper"))
        .expect("expected pub.. helper");
    let named_export = module
        .functions
        .iter()
        .find(|function| function.name == "bridge")
        .expect("expected export_name helper");

    assert_eq!(private_helper.linkage, kernc_mast::MastLinkage::Internal);
    assert_eq!(public_helper.linkage, kernc_mast::MastLinkage::External);
    assert_eq!(parent_helper.linkage, kernc_mast::MastLinkage::External);
    assert_eq!(named_export.linkage, kernc_mast::MastLinkage::External);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn lowering_keeps_pub_super_imported_helpers_reachable() {
    let root = std::env::temp_dir().join(format!(
        "kern_lower_pub_super_import_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join("left")).unwrap();
    fs::create_dir_all(root.join("right")).unwrap();
    let main = root.join("main.kn");
    fs::write(
        &main,
        concat!(
            "mod left;\n",
            "mod right;\n",
            "extern fn main() i32 { return right.value(); }\n",
        ),
    )
    .unwrap();
    fs::write(
        root.join("left").join("mod.kn"),
        "pub.. fn helper() i32 { return 7; }\n",
    )
    .unwrap();
    fs::write(
        root.join("right").join("mod.kn"),
        concat!(
            "use ..left.helper;\n",
            "pub fn value() i32 { return helper(); }\n",
        ),
    )
    .unwrap();

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
        .expect("expected imported pub.. helper to be lowered");
    assert_eq!(helper.linkage, kernc_mast::MastLinkage::External);

    let _ = fs::remove_dir_all(&root);
}

fn count_calls_to_block(block: &MastBlock, target: MonoId) -> usize {
    let mut total = 0;
    for stmt in &block.stmts {
        total += match stmt {
            MastStmt::Let { init, .. } => count_calls_to_expr(init, target),
            MastStmt::Expr(expr) => count_calls_to_expr(expr, target),
        };
    }
    if let Some(result) = &block.result {
        total += count_calls_to_expr(result, target);
    }
    for defer in &block.defers {
        total += count_calls_to_expr(defer, target);
    }
    total
}

fn count_calls_to_expr(expr: &MastExpr, target: MonoId) -> usize {
    match &expr.kind {
        MastExprKind::Call { callee, args } => {
            let mut total = count_calls_to_expr(callee, target);
            if matches!(callee.kind, MastExprKind::FuncRef(id) if id == target) {
                total += 1;
            }
            for arg in args {
                total += count_calls_to_expr(arg, target);
            }
            total
        }
        MastExprKind::AddressOf(inner)
        | MastExprKind::Deref(inner)
        | MastExprKind::Discard(inner)
        | MastExprKind::ExtractFatPtrData(inner)
        | MastExprKind::ExtractFatPtrMeta(inner)
        | MastExprKind::ExtractElementPtr(inner)
        | MastExprKind::Unary { operand: inner, .. }
        | MastExprKind::Cast { operand: inner, .. }
        | MastExprKind::BitIntrinsic { operand: inner, .. }
        | MastExprKind::SimdUnaryIntrinsic { operand: inner, .. }
        | MastExprKind::SimdReduce { operand: inner, .. }
        | MastExprKind::SimdAny { operand: inner, .. }
        | MastExprKind::SimdAll { operand: inner, .. }
        | MastExprKind::SimdBitmask { operand: inner, .. }
        | MastExprKind::SimdSplat { value: inner, .. }
        | MastExprKind::SimdCast { value: inner, .. }
        | MastExprKind::SimdBitcast { value: inner, .. } => count_calls_to_expr(inner, target),
        MastExprKind::StructInit { fields, .. } | MastExprKind::ArrayInit(fields) => fields
            .iter()
            .map(|field| count_calls_to_expr(field, target))
            .sum(),
        MastExprKind::UnionInit { value, .. } => count_calls_to_expr(value, target),
        MastExprKind::FieldAccess { lhs, .. } => count_calls_to_expr(lhs, target),
        MastExprKind::IndexAccess { lhs, index } => {
            count_calls_to_expr(lhs, target) + count_calls_to_expr(index, target)
        }
        MastExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            count_calls_to_expr(cond, target)
                + count_calls_to_block(then_branch, target)
                + else_branch
                    .as_ref()
                    .map_or(0, |else_branch| count_calls_to_block(else_branch, target))
        }
        MastExprKind::Loop { body, latch } => {
            count_calls_to_block(body, target)
                + latch
                    .as_ref()
                    .map_or(0, |latch| count_calls_to_block(latch, target))
        }
        MastExprKind::Switch {
            target: switch_target,
            cases,
            default_case,
        } => {
            count_calls_to_expr(switch_target, target)
                + cases
                    .iter()
                    .map(|case| count_calls_to_block(&case.body, target))
                    .sum::<usize>()
                + default_case
                    .as_ref()
                    .map_or(0, |default_case| count_calls_to_block(default_case, target))
        }
        MastExprKind::Return(value) => value
            .as_ref()
            .map_or(0, |value| count_calls_to_expr(value, target)),
        MastExprKind::Binary { lhs, rhs, .. }
        | MastExprKind::Assign { lhs, rhs, .. }
        | MastExprKind::SimdBinaryIntrinsic { lhs, rhs, .. } => {
            count_calls_to_expr(lhs, target) + count_calls_to_expr(rhs, target)
        }
        MastExprKind::ConstructFatPointer { data_ptr, meta } => {
            count_calls_to_expr(data_ptr, target) + count_calls_to_expr(meta, target)
        }
        MastExprKind::Block(block) => count_calls_to_block(block, target),
        MastExprKind::DataInit { payload, .. } => count_calls_to_expr(payload, target),
        MastExprKind::Asm(asm) => {
            asm.input_args
                .iter()
                .map(|input| count_calls_to_expr(input, target))
                .sum::<usize>()
                + asm
                    .output_ptrs
                    .iter()
                    .map(|output| count_calls_to_expr(output, target))
                    .sum::<usize>()
        }
        MastExprKind::SimdSelect {
            mask,
            on_true,
            on_false,
        } => {
            count_calls_to_expr(mask, target)
                + count_calls_to_expr(on_true, target)
                + count_calls_to_expr(on_false, target)
        }
        MastExprKind::SimdShuffle { lhs, rhs, .. } => {
            count_calls_to_expr(lhs, target) + count_calls_to_expr(rhs, target)
        }
        MastExprKind::SimdInsertHalf { base, half, .. } => {
            count_calls_to_expr(base, target) + count_calls_to_expr(half, target)
        }
        MastExprKind::SimdLoad { ptr, .. } => count_calls_to_expr(ptr, target),
        MastExprKind::SimdStore { ptr, value, .. } => {
            count_calls_to_expr(ptr, target) + count_calls_to_expr(value, target)
        }
        MastExprKind::SimdMaskedLoad {
            ptr, mask, or_else, ..
        } => {
            count_calls_to_expr(ptr, target)
                + count_calls_to_expr(mask, target)
                + count_calls_to_expr(or_else, target)
        }
        MastExprKind::SimdMaskedStore {
            ptr, mask, value, ..
        } => {
            count_calls_to_expr(ptr, target)
                + count_calls_to_expr(mask, target)
                + count_calls_to_expr(value, target)
        }
        MastExprKind::SimdGather { ptr, indices } => {
            count_calls_to_expr(ptr, target) + count_calls_to_expr(indices, target)
        }
        MastExprKind::SimdScatter {
            ptr,
            indices,
            value,
        } => {
            count_calls_to_expr(ptr, target)
                + count_calls_to_expr(indices, target)
                + count_calls_to_expr(value, target)
        }
        MastExprKind::SimdMaskedGather {
            ptr,
            indices,
            mask,
            or_else,
        } => {
            count_calls_to_expr(ptr, target)
                + count_calls_to_expr(indices, target)
                + count_calls_to_expr(mask, target)
                + count_calls_to_expr(or_else, target)
        }
        MastExprKind::SimdMaskedScatter {
            ptr,
            indices,
            mask,
            value,
        } => {
            count_calls_to_expr(ptr, target)
                + count_calls_to_expr(indices, target)
                + count_calls_to_expr(mask, target)
                + count_calls_to_expr(value, target)
        }
        MastExprKind::AtomicLoad { ptr, .. } => count_calls_to_expr(ptr, target),
        MastExprKind::AtomicStore { ptr, value, .. } => {
            count_calls_to_expr(ptr, target) + count_calls_to_expr(value, target)
        }
        MastExprKind::AtomicCas {
            ptr,
            expected,
            desired,
            ..
        } => {
            count_calls_to_expr(ptr, target)
                + count_calls_to_expr(expected, target)
                + count_calls_to_expr(desired, target)
        }
        MastExprKind::AtomicRmw { ptr, value, .. } => {
            count_calls_to_expr(ptr, target) + count_calls_to_expr(value, target)
        }
        MastExprKind::Memcpy { dest, src, len } | MastExprKind::Memmove { dest, src, len } => {
            count_calls_to_expr(dest, target)
                + count_calls_to_expr(src, target)
                + count_calls_to_expr(len, target)
        }
        MastExprKind::Memset { dest, val, len } => {
            count_calls_to_expr(dest, target)
                + count_calls_to_expr(val, target)
                + count_calls_to_expr(len, target)
        }
        MastExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            count_calls_to_expr(lhs, target)
                + start
                    .as_ref()
                    .map_or(0, |start| count_calls_to_expr(start, target))
                + end
                    .as_ref()
                    .map_or(0, |end| count_calls_to_expr(end, target))
        }
        MastExprKind::Undef
        | MastExprKind::Unreachable
        | MastExprKind::Trap
        | MastExprKind::Breakpoint
        | MastExprKind::Integer(_)
        | MastExprKind::Float(_)
        | MastExprKind::Bool(_)
        | MastExprKind::StringLiteral(_)
        | MastExprKind::Var(_)
        | MastExprKind::GlobalRef(_)
        | MastExprKind::FuncRef(_)
        | MastExprKind::Break
        | MastExprKind::Continue
        | MastExprKind::Fence { .. } => 0,
    }
}
