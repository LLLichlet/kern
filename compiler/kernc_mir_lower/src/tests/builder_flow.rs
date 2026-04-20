use super::*;

#[test]
fn mir_builder_lowers_static_slice_literal_via_fat_pointer_init() {
    let backing = MastGlobal {
        id: MonoId(10),
        name: "backing".to_string(),
        span: Span::default(),
        linkage: MastLinkage::Internal,
        ty: TypeId::USIZE,
        is_mut: false,
        init: Some(MastExpr::new(
            TypeId::USIZE,
            MastExprKind::Integer(7),
            Span::default(),
        )),
        is_extern: false,
        attributes: vec![],
    };
    let slice = MastGlobal {
        id: MonoId(11),
        name: "slice".to_string(),
        span: Span::default(),
        linkage: MastLinkage::Internal,
        ty: TypeId::USIZE,
        is_mut: false,
        init: Some(MastExpr::new(
            TypeId::USIZE,
            MastExprKind::ConstructFatPointer {
                data_ptr: Box::new(MastExpr::new(
                    TypeId::USIZE,
                    MastExprKind::AddressOf(Box::new(MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::GlobalRef(MonoId(10)),
                        Span::default(),
                    ))),
                    Span::default(),
                )),
                meta: Box::new(MastExpr::new(
                    TypeId::USIZE,
                    MastExprKind::Integer(3),
                    Span::default(),
                )),
            },
            Span::default(),
        )),
        is_extern: false,
        attributes: vec![],
    };
    let module = MastModule {
        name: "demo".to_string(),
        structs: vec![],
        globals: vec![backing, slice],
        functions: vec![],
        mono: MonoModuleMetadata::default(),
    };

    let report = build_from_mast_unoptimized(&module);
    let init = report.module.globals[1]
        .init
        .as_ref()
        .expect("slice global should keep a static init");
    match init {
        MirStaticInit::FatPointer { data_ptr, meta, .. } => {
            assert!(matches!(
                data_ptr.as_ref(),
                MirStaticInit::Const(MirConst::GlobalRef { id, .. }) if *id == MonoId(10)
            ));
            assert!(matches!(
                meta.as_ref(),
                MirStaticInit::Const(MirConst::Integer { value: 3, .. })
            ));
        }
        other => panic!("expected fat-pointer static init, got {other:?}"),
    }
}

#[test]
fn mir_builder_preserves_inline_hint() {
    let report = build_from_mast(&module_with_function(MastFunction {
        id: MonoId(77),
        name: "inline_demo".to_string(),
        span: Span::default(),
        linkage: MastLinkage::External,
        params: vec![],
        ret_ty: TypeId::VOID,
        body: Some(MastBlock {
            stmts: vec![],
            result: None,
            defers: vec![],
        }),
        is_extern: false,
        is_variadic: false,
        inline_hint: MastInlineHint::Inline,
        attributes: vec![],
    }));

    assert_eq!(
        report.module.functions[0].inline_hint,
        MirInlineHint::Inline
    );
    assert_eq!(
        report.summary.functions[0].inline_hint,
        MirInlineHint::Inline
    );
}

#[test]
fn mir_builder_extracts_cfg_from_if_statement() {
    let function = MastFunction {
        id: MonoId(1),
        name: "demo".to_string(),
        span: Span::default(),
        linkage: MastLinkage::External,
        params: vec![MastParam {
            name: SymbolId(1),
            ty: TypeId::I32,
            is_mut: false,
        }],
        ret_ty: TypeId::I32,
        body: Some(MastBlock {
            stmts: vec![MastStmt::Expr(void_expr(MastExprKind::If {
                cond: Box::new(expr(MastExprKind::Bool(true))),
                then_branch: MastBlock {
                    stmts: vec![],
                    result: Some(Box::new(expr(MastExprKind::Integer(1)))),
                    defers: vec![],
                },
                else_branch: Some(MastBlock {
                    stmts: vec![],
                    result: Some(Box::new(expr(MastExprKind::Integer(2)))),
                    defers: vec![],
                }),
            }))],
            result: Some(Box::new(expr(MastExprKind::Integer(3)))),
            defers: vec![],
        }),
        is_extern: false,
        is_variadic: false,
        inline_hint: MastInlineHint::None,
        attributes: vec![],
    };

    let report = build_from_mast_unoptimized(&module_with_function(function));
    let body = report.module.functions[0].body.as_ref().unwrap();
    assert!(!body.locals.is_empty());
    assert!(matches!(body.locals[0].kind, MirLocalKind::Param));
    assert_eq!(body.blocks.len(), 4);
    assert!(matches!(
        body.blocks[0].terminator,
        MirTerminatorData {
            kind: MirTerminator::Branch { .. },
            ..
        }
    ));
    assert!(matches!(
        body.blocks[1].terminator,
        MirTerminatorData {
            kind: MirTerminator::Goto(_),
            ..
        }
    ));
    assert!(matches!(
        body.blocks[2].terminator,
        MirTerminatorData {
            kind: MirTerminator::Goto(_),
            ..
        }
    ));
    assert!(matches!(
        body.blocks[3].terminator,
        MirTerminatorData {
            kind: MirTerminator::Return(Some(_)),
            ..
        }
    ));
}

#[test]
fn mir_builder_records_defers_and_loop_edges() {
    let function = MastFunction {
        id: MonoId(2),
        name: "loop_demo".to_string(),
        span: Span::default(),
        linkage: MastLinkage::External,
        params: vec![],
        ret_ty: TypeId::VOID,
        body: Some(MastBlock {
            stmts: vec![MastStmt::Expr(void_expr(MastExprKind::Loop {
                body: MastBlock {
                    stmts: vec![MastStmt::Expr(void_expr(MastExprKind::Breakpoint))],
                    result: None,
                    defers: vec![void_expr(MastExprKind::Trap)],
                },
                latch: None,
            }))],
            result: None,
            defers: vec![],
        }),
        is_extern: false,
        is_variadic: false,
        inline_hint: MastInlineHint::None,
        attributes: vec![],
    };

    let report = build_from_mast_unoptimized(&module_with_function(function));
    let body = report.module.functions[0].body.as_ref().unwrap();

    assert!(body.locals.is_empty());
    assert!(body.blocks.iter().any(|block| {
        block
            .instructions
            .iter()
            .any(|instruction| matches!(instruction.kind, MirInstruction::Breakpoint))
    }));
    assert!(body.blocks.iter().any(|block| {
        block
            .instructions
            .iter()
            .any(|instruction| matches!(instruction.kind, MirInstruction::Trap))
    }));
    assert!(
        body.blocks
            .iter()
            .any(|block| matches!(block.terminator.kind, MirTerminator::Unreachable))
    );
    assert!(
        body.blocks
            .iter()
            .any(|block| matches!(block.terminator.kind, MirTerminator::Goto(_)))
    );
}

#[test]
fn mir_builder_accepts_nonvoid_loop_tail_as_diverging_control() {
    let function = MastFunction {
        id: MonoId(93),
        name: "loop_tail_value".to_string(),
        span: Span::default(),
        linkage: MastLinkage::External,
        params: vec![],
        ret_ty: TypeId::I32,
        body: Some(MastBlock {
            stmts: vec![],
            result: Some(Box::new(MastExpr::new(
                TypeId::I32,
                MastExprKind::Loop {
                    body: MastBlock {
                        stmts: vec![MastStmt::Expr(MastExpr::new(
                            TypeId::NEVER,
                            MastExprKind::Return(Some(Box::new(expr(MastExprKind::Integer(7))))),
                            Span::default(),
                        ))],
                        result: None,
                        defers: vec![],
                    },
                    latch: None,
                },
                Span::default(),
            ))),
            defers: vec![],
        }),
        is_extern: false,
        is_variadic: false,
        inline_hint: MastInlineHint::None,
        attributes: vec![],
    };

    let report = build_from_mast_unoptimized(&module_with_function(function));
    let body = report.module.functions[0].body.as_ref().unwrap();

    assert!(
        body.blocks
            .iter()
            .any(|block| matches!(block.terminator.kind, MirTerminator::Return(Some(_))))
    );
}

#[test]
fn mir_builder_accepts_diverging_block_rvalue_with_loop_tail() {
    let local = SymbolId(40);
    let function = MastFunction {
        id: MonoId(94),
        name: "loop_in_rvalue_block".to_string(),
        span: Span::default(),
        linkage: MastLinkage::External,
        params: vec![],
        ret_ty: TypeId::I32,
        body: Some(MastBlock {
            stmts: vec![MastStmt::Let {
                name: local,
                ty: TypeId::I32,
                is_mut: false,
                init: MastExpr::new(
                    TypeId::I32,
                    MastExprKind::Block(MastBlock {
                        stmts: vec![],
                        result: Some(Box::new(MastExpr::new(
                            TypeId::I32,
                            MastExprKind::Loop {
                                body: MastBlock {
                                    stmts: vec![MastStmt::Expr(MastExpr::new(
                                        TypeId::NEVER,
                                        MastExprKind::Return(Some(Box::new(expr(
                                            MastExprKind::Integer(9),
                                        )))),
                                        Span::default(),
                                    ))],
                                    result: None,
                                    defers: vec![],
                                },
                                latch: None,
                            },
                            Span::default(),
                        ))),
                        defers: vec![],
                    }),
                    Span::default(),
                ),
            }],
            result: None,
            defers: vec![],
        }),
        is_extern: false,
        is_variadic: false,
        inline_hint: MastInlineHint::None,
        attributes: vec![],
    };

    let report = build_from_mast_unoptimized(&module_with_function(function));
    let body = report.module.functions[0].body.as_ref().unwrap();

    assert!(
        body.blocks
            .iter()
            .any(|block| matches!(block.terminator.kind, MirTerminator::Return(Some(_))))
    );
}

#[test]
fn mir_builder_lowers_explicit_unreachable_to_unreachable_terminator() {
    let function = MastFunction {
        id: MonoId(92),
        name: "never".to_string(),
        span: Span::default(),
        linkage: MastLinkage::External,
        params: vec![],
        ret_ty: TypeId::VOID,
        body: Some(MastBlock {
            stmts: vec![MastStmt::Expr(void_expr(MastExprKind::Unreachable))],
            result: None,
            defers: vec![],
        }),
        is_extern: false,
        is_variadic: false,
        inline_hint: MastInlineHint::None,
        attributes: vec![],
    };

    let report = build_from_mast_unoptimized(&module_with_function(function));
    let body = report.module.functions[0].body.as_ref().unwrap();

    assert!(matches!(
        body.blocks[0].terminator,
        MirTerminatorData {
            kind: MirTerminator::Unreachable,
            ..
        }
    ));
}

#[test]
fn mir_builder_resolves_param_and_let_uses_to_locals() {
    let seed = SymbolId(11);
    let value = SymbolId(12);
    let function = MastFunction {
        id: MonoId(3),
        name: "bindings".to_string(),
        span: Span::default(),
        linkage: MastLinkage::External,
        params: vec![MastParam {
            name: seed,
            ty: TypeId::I32,
            is_mut: false,
        }],
        ret_ty: TypeId::I32,
        body: Some(MastBlock {
            stmts: vec![MastStmt::Let {
                name: value,
                ty: TypeId::I32,
                is_mut: false,
                init: expr(MastExprKind::Var(seed)),
            }],
            result: Some(Box::new(expr(MastExprKind::Var(value)))),
            defers: vec![],
        }),
        is_extern: false,
        is_variadic: false,
        inline_hint: MastInlineHint::None,
        attributes: vec![],
    };

    let report = build_from_mast_unoptimized(&module_with_function(function));
    let body = report.module.functions[0].body.as_ref().unwrap();
    let param_local = body.locals[0].id;
    let let_local = body.locals[1].id;
    let return_local = body.locals[2].id;

    assert!(body.blocks.iter().any(|block| matches!(
        block.instructions.first(),
        Some(MirInstructionData {
            kind: MirInstruction::Let {
                place: MirPlace::Local(place_local),
                init: MirRvalue::Use(MirOperand::Local(init_local)),
            },
            ..
        }) if *place_local == let_local && *init_local == param_local
    )));
    assert!(body.blocks.iter().any(|block| matches!(
        block.instructions.get(1),
        Some(MirInstructionData {
            kind: MirInstruction::Assign {
                place: MirPlace::Local(place_local),
                op: AssignmentOperator::Assign,
                value: MirRvalue::Use(MirOperand::Local(init_local)),
            },
            ..
        }) if place_local == &return_local && init_local == &let_local
    )));
    assert!(body.blocks.iter().any(|block| matches!(
        &block.terminator,
        MirTerminatorData {
            kind: MirTerminator::Return(Some(MirRvalue::Use(MirOperand::Local(local)))),
            ..
        } if local == &return_local
    )));
}

#[test]
fn mir_builder_let_initializer_uses_outer_binding_before_shadowing() {
    let align = SymbolId(19);
    let function = MastFunction {
        id: MonoId(19),
        name: "shadow_init".to_string(),
        span: Span::default(),
        linkage: MastLinkage::External,
        params: vec![MastParam {
            name: align,
            ty: TypeId::I64,
            is_mut: false,
        }],
        ret_ty: TypeId::I64,
        body: Some(MastBlock {
            stmts: vec![MastStmt::Let {
                name: align,
                ty: TypeId::I64,
                is_mut: false,
                init: expr(MastExprKind::Var(align)),
            }],
            result: Some(Box::new(expr(MastExprKind::Var(align)))),
            defers: vec![],
        }),
        is_extern: false,
        is_variadic: false,
        inline_hint: MastInlineHint::None,
        attributes: vec![],
    };

    let report = build_from_mast_unoptimized(&module_with_function(function));
    let body = report.module.functions[0].body.as_ref().unwrap();
    let param_local = body.locals[0].id;
    let let_local = body.locals[1].id;
    let return_local = body.locals[2].id;

    assert!(body.blocks.iter().any(|block| matches!(
        block.instructions.first(),
        Some(MirInstructionData {
            kind: MirInstruction::Let {
                place: MirPlace::Local(place_local),
                init: MirRvalue::Use(MirOperand::Local(init_local)),
            },
            ..
        }) if *place_local == let_local && *init_local == param_local
    )));
    assert!(body.blocks.iter().any(|block| matches!(
        block.instructions.get(1),
        Some(MirInstructionData {
            kind: MirInstruction::Assign {
                place: MirPlace::Local(place_local),
                op: AssignmentOperator::Assign,
                value: MirRvalue::Use(MirOperand::Local(init_local)),
            },
            ..
        }) if place_local == &return_local && init_local == &let_local
    )));
    assert!(body.blocks.iter().any(|block| matches!(
        &block.terminator,
        MirTerminatorData {
            kind: MirTerminator::Return(Some(MirRvalue::Use(MirOperand::Local(local)))),
            ..
        } if local == &return_local
    )));
}

#[test]
fn mir_builder_extracts_direct_calls_from_mast_exprs() {
    let helper_id = MonoId(30);
    let seed = SymbolId(31);
    let value = SymbolId(32);
    let function = MastFunction {
        id: MonoId(4),
        name: "caller".to_string(),
        span: Span::default(),
        linkage: MastLinkage::External,
        params: vec![MastParam {
            name: seed,
            ty: TypeId::I32,
            is_mut: false,
        }],
        ret_ty: TypeId::I32,
        body: Some(MastBlock {
            stmts: vec![MastStmt::Let {
                name: value,
                ty: TypeId::I32,
                is_mut: false,
                init: expr(MastExprKind::Call {
                    callee: Box::new(expr(MastExprKind::FuncRef(helper_id))),
                    args: vec![expr(MastExprKind::Var(seed))],
                }),
            }],
            result: Some(Box::new(expr(MastExprKind::Var(value)))),
            defers: vec![],
        }),
        is_extern: false,
        is_variadic: false,
        inline_hint: MastInlineHint::None,
        attributes: vec![],
    };

    let report = build_from_mast_unoptimized(&module_with_function(function));
    let body = report.module.functions[0].body.as_ref().unwrap();
    let param_local = body.locals[0].id;

    assert!(matches!(
        &body.blocks[0].instructions[0],
        MirInstructionData {
            kind: MirInstruction::Let {
                init: MirRvalue::Call {
                    callee: MirCallTarget::Direct(id),
                    args,
                },
                ..
            },
            ..
        } if id == &helper_id
            && matches!(args.as_slice(), [MirOperand::Local(local)] if local == &param_local)
    ));
}

#[test]
fn mir_builder_lowers_global_assignments_to_global_places() {
    let global_id = MonoId(90);
    let function = MastFunction {
        id: MonoId(91),
        name: "write_global".to_string(),
        span: Span::default(),
        linkage: MastLinkage::External,
        params: vec![],
        ret_ty: TypeId::VOID,
        body: Some(MastBlock {
            stmts: vec![MastStmt::Expr(void_expr(MastExprKind::Assign {
                op: AssignmentOperator::Assign,
                lhs: Box::new(expr(MastExprKind::GlobalRef(global_id))),
                rhs: Box::new(expr(MastExprKind::Integer(7))),
            }))],
            result: None,
            defers: vec![],
        }),
        is_extern: false,
        is_variadic: false,
        inline_hint: MastInlineHint::None,
        attributes: vec![],
    };
    let module = MastModule {
        name: "demo".to_string(),
        structs: vec![],
        globals: vec![MastGlobal {
            id: global_id,
            name: "answer".to_string(),
            span: Span::default(),
            linkage: MastLinkage::Internal,
            ty: TypeId::I32,
            is_mut: true,
            init: Some(expr(MastExprKind::Integer(0))),
            is_extern: false,
            attributes: vec![],
        }],
        functions: vec![function],
        mono: MonoModuleMetadata::default(),
    };

    let report = build_from_mast_unoptimized(&module);
    let body = report.module.functions[0].body.as_ref().unwrap();

    assert!(matches!(
        &body.blocks[0].instructions[0],
        MirInstructionData {
            kind: MirInstruction::Assign {
                place: MirPlace::Global(id),
                op: AssignmentOperator::Assign,
                value: MirRvalue::Use(MirOperand::Const(_)),
            },
            ..
        } if *id == global_id
    ));
}
