use super::*;

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
