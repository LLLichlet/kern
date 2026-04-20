use super::*;

#[test]
fn mir_pass_pipeline_forwards_trivial_local_copy_chains() {
    let seed = SymbolId(120);
    let first = SymbolId(121);
    let second = SymbolId(122);
    let function = MastFunction {
        id: MonoId(13),
        name: "copy_chain".to_string(),
        span: Span::default(),
        linkage: MastLinkage::External,
        params: vec![MastParam {
            name: seed,
            ty: TypeId::I32,
            is_mut: false,
        }],
        ret_ty: TypeId::I32,
        body: Some(MastBlock {
            stmts: vec![
                MastStmt::Let {
                    name: first,
                    ty: TypeId::I32,
                    is_mut: false,
                    init: MastExpr::new(TypeId::I32, MastExprKind::Var(seed), Span::default()),
                },
                MastStmt::Let {
                    name: second,
                    ty: TypeId::I32,
                    is_mut: false,
                    init: MastExpr::new(TypeId::I32, MastExprKind::Var(first), Span::default()),
                },
            ],
            result: Some(Box::new(MastExpr::new(
                TypeId::I32,
                MastExprKind::Var(second),
                Span::default(),
            ))),
            defers: vec![],
        }),
        is_extern: false,
        is_variadic: false,
        inline_hint: MastInlineHint::None,
        attributes: vec![],
    };

    let report = build_from_mast(&module_with_function(function));
    let body = report.module.functions[0].body.as_ref().unwrap();
    let pass = &report.pass_pipeline.passes[0];
    let param_local = body.locals[0].id;

    assert_eq!(pass.name, "local_copy_propagation");
    assert!(pass.changed());
    assert!(pass.removed_let_instructions >= 2);
    assert!(matches!(
        body.blocks[0].instructions.as_slice(),
        [MirInstruction::Assign {
            op: AssignmentOperator::Assign,
            value: MirRvalue::Use(MirOperand::Local(source_local)),
            ..
        }] if source_local == &param_local
    ));
    assert!(matches!(
        &body.blocks[0].terminator,
        MirTerminator::Return(Some(MirRvalue::Use(MirOperand::Local(_))))
    ));
}

#[test]
fn mir_pass_pipeline_folds_const_branch_after_copy_propagation() {
    let cond = SymbolId(130);
    let report = build_from_mast(&module_with_function(MastFunction {
        id: MonoId(14),
        name: "const_branch".to_string(),
        span: Span::default(),
        linkage: MastLinkage::External,
        params: vec![],
        ret_ty: TypeId::I32,
        body: Some(MastBlock {
            stmts: vec![MastStmt::Let {
                name: cond,
                ty: TypeId::BOOL,
                is_mut: false,
                init: bool_expr(MastExprKind::Bool(true)),
            }],
            result: Some(Box::new(MastExpr::new(
                TypeId::I32,
                MastExprKind::If {
                    cond: Box::new(bool_expr(MastExprKind::Var(cond))),
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
                },
                Span::default(),
            ))),
            defers: vec![],
        }),
        is_extern: false,
        is_variadic: false,
        inline_hint: MastInlineHint::None,
        attributes: vec![],
    }));
    let body = report.module.functions[0].body.as_ref().unwrap();
    let copy_pass = &report.pass_pipeline.passes[0];
    let thread_pass = &report.pass_pipeline.passes[1];
    let branch_pass = &report.pass_pipeline.passes[2];
    let cfg_pass = &report.pass_pipeline.passes[3];

    assert_eq!(copy_pass.name, "local_copy_propagation");
    assert!(copy_pass.changed());
    assert_eq!(thread_pass.name, "cfg_thread_jumps");
    assert!(!thread_pass.changed());
    assert_eq!(branch_pass.name, "branch_folding");
    assert!(branch_pass.changed());
    assert!(branch_pass.terminator_rewrites >= 1);
    assert_eq!(cfg_pass.name, "cfg_prune_unreachable_blocks");
    assert_eq!(cfg_pass.removed_blocks, 1);
    assert_eq!(body.blocks.len(), 3);
    assert!(matches!(&body.blocks[0].terminator, MirTerminator::Goto(_)));
    assert!(body.blocks.iter().any(|block| matches!(
        block.instructions.as_slice(),
        [MirInstruction::Assign {
            value: MirRvalue::Use(MirOperand::Const(MirConst::Integer { value: 1, .. })),
            ..
        }]
    )));
    assert!(body.blocks.iter().any(|block| matches!(
        &block.terminator,
        MirTerminator::Return(Some(MirRvalue::Use(MirOperand::Local(_))))
    )));
}

#[test]
fn mir_pass_pipeline_folds_const_switch_to_matching_case() {
    let report = build_from_mast(&module_with_function(MastFunction {
        id: MonoId(15),
        name: "const_switch".to_string(),
        span: Span::default(),
        linkage: MastLinkage::External,
        params: vec![],
        ret_ty: TypeId::I32,
        body: Some(MastBlock {
            stmts: vec![],
            result: Some(Box::new(MastExpr::new(
                TypeId::I32,
                MastExprKind::Switch {
                    target: Box::new(expr(MastExprKind::Integer(2))),
                    cases: vec![
                        MastSwitchCase {
                            values: vec![1],
                            body: MastBlock {
                                stmts: vec![],
                                result: Some(Box::new(expr(MastExprKind::Integer(10)))),
                                defers: vec![],
                            },
                        },
                        MastSwitchCase {
                            values: vec![2],
                            body: MastBlock {
                                stmts: vec![],
                                result: Some(Box::new(expr(MastExprKind::Integer(20)))),
                                defers: vec![],
                            },
                        },
                    ],
                    default_case: Some(MastBlock {
                        stmts: vec![],
                        result: Some(Box::new(expr(MastExprKind::Integer(30)))),
                        defers: vec![],
                    }),
                },
                Span::default(),
            ))),
            defers: vec![],
        }),
        is_extern: false,
        is_variadic: false,
        inline_hint: MastInlineHint::None,
        attributes: vec![],
    }));
    let body = report.module.functions[0].body.as_ref().unwrap();
    let thread_pass = &report.pass_pipeline.passes[1];
    let branch_pass = &report.pass_pipeline.passes[2];
    let cfg_pass = &report.pass_pipeline.passes[3];

    assert_eq!(thread_pass.name, "cfg_thread_jumps");
    assert!(!thread_pass.changed());
    assert_eq!(branch_pass.name, "branch_folding");
    assert!(branch_pass.changed());
    assert!(branch_pass.terminator_rewrites >= 1);
    assert_eq!(cfg_pass.name, "cfg_prune_unreachable_blocks");
    assert_eq!(cfg_pass.removed_blocks, 2);
    assert_eq!(body.blocks.len(), 3);
    assert!(matches!(&body.blocks[0].terminator, MirTerminator::Goto(_)));
    assert!(body.blocks.iter().any(|block| matches!(
        block.instructions.as_slice(),
        [MirInstruction::Assign {
            value: MirRvalue::Use(MirOperand::Const(MirConst::Integer { value: 20, .. })),
            ..
        }]
    )));
    assert!(body.blocks.iter().any(|block| matches!(
        &block.terminator,
        MirTerminator::Return(Some(MirRvalue::Use(MirOperand::Local(_))))
    )));
}
