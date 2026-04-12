use crate::{build_from_mast, build_from_mast_unoptimized};
use kernc_ast::{AssignmentOperator, BinaryOperator, UnaryOperator};
use kernc_mast::{
    MastAsmBlock, MastBlock, MastCastKind, MastExpr, MastExprKind, MastFunction, MastGlobal,
    MastInlineHint, MastLinkage, MastModule, MastParam, MastStmt, MastSwitchCase,
};
use kernc_mir::{
    MirAggregateKind, MirBitIntrinsicKind, MirCallTarget, MirCastKind, MirConst, MirInlineHint,
    MirInstruction, MirLocalKind, MirMemoryIntrinsic, MirOperand, MirPlace, MirProjectionKind,
    MirRvalue, MirSimdBinaryIntrinsicKind, MirSliceBase, MirTerminator,
};
use kernc_mono::{MonoId, MonoModuleMetadata};
use kernc_sema::ty::TypeId;
use kernc_utils::{AtomicOrdering, AtomicRmwOp, Span, SymbolId};

fn expr(kind: MastExprKind) -> MastExpr {
    MastExpr::new(TypeId::I32, kind, Span::default())
}

fn void_expr(kind: MastExprKind) -> MastExpr {
    MastExpr::new(TypeId::VOID, kind, Span::default())
}

fn bool_expr(kind: MastExprKind) -> MastExpr {
    MastExpr::new(TypeId::BOOL, kind, Span::default())
}

fn module_with_function(function: MastFunction) -> MastModule {
    MastModule {
        name: "demo".to_string(),
        structs: vec![],
        globals: vec![],
        functions: vec![function],
        mono: MonoModuleMetadata::default(),
    }
}

#[test]
fn mir_builder_preserves_inline_hint() {
    let report = build_from_mast(&module_with_function(MastFunction {
        id: MonoId(77),
        name: "inline_demo".to_string(),
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
fn mir_builder_preserves_always_inline_hint() {
    let report = build_from_mast(&module_with_function(MastFunction {
        id: MonoId(78),
        name: "always_inline_demo".to_string(),
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
        inline_hint: MastInlineHint::Always,
        attributes: vec![],
    }));

    assert_eq!(
        report.module.functions[0].inline_hint,
        MirInlineHint::Always
    );
    assert_eq!(
        report.summary.functions[0].inline_hint,
        MirInlineHint::Always
    );
}

#[test]
fn mir_builder_extracts_cfg_from_if_statement() {
    let function = MastFunction {
        id: MonoId(1),
        name: "demo".to_string(),
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

    assert_eq!(body.locals.len(), 1);
    assert!(matches!(body.locals[0].kind, MirLocalKind::Param));
    assert_eq!(body.blocks.len(), 4);
    assert!(matches!(
        body.blocks[0].terminator,
        MirTerminator::Branch { .. }
    ));
    assert!(matches!(body.blocks[1].terminator, MirTerminator::Goto(_)));
    assert!(matches!(body.blocks[2].terminator, MirTerminator::Goto(_)));
    assert!(matches!(
        body.blocks[3].terminator,
        MirTerminator::Return(Some(_))
    ));
}

#[test]
fn mir_builder_records_defers_and_loop_edges() {
    let function = MastFunction {
        id: MonoId(2),
        name: "loop_demo".to_string(),
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
            .any(|instruction| matches!(instruction, MirInstruction::Breakpoint))
    }));
    assert!(body.blocks.iter().any(|block| {
        block
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, MirInstruction::Trap))
    }));
    assert!(
        body.blocks
            .iter()
            .any(|block| matches!(block.terminator, MirTerminator::Unreachable))
    );
    assert!(
        body.blocks
            .iter()
            .any(|block| matches!(block.terminator, MirTerminator::Goto(_)))
    );
}

#[test]
fn mir_builder_lowers_explicit_unreachable_to_unreachable_terminator() {
    let function = MastFunction {
        id: MonoId(92),
        name: "never".to_string(),
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
        MirTerminator::Unreachable
    ));
}

#[test]
fn mir_builder_resolves_param_and_let_uses_to_locals() {
    let seed = SymbolId(11);
    let value = SymbolId(12);
    let function = MastFunction {
        id: MonoId(3),
        name: "bindings".to_string(),
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

    assert!(matches!(
        &body.blocks[0].instructions[0],
        MirInstruction::Let {
            place: MirPlace::Local(place_local),
            init: MirRvalue::Use(MirOperand::Local(init_local)),
        } if place_local == &let_local && init_local == &param_local
    ));
    assert!(matches!(
        &body.blocks[0].terminator,
        MirTerminator::Return(Some(MirRvalue::Use(MirOperand::Local(return_local))))
            if return_local == &let_local
    ));
}

#[test]
fn mir_builder_let_initializer_uses_outer_binding_before_shadowing() {
    let align = SymbolId(19);
    let function = MastFunction {
        id: MonoId(19),
        name: "shadow_init".to_string(),
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

    assert!(matches!(
        &body.blocks[0].instructions[0],
        MirInstruction::Let {
            place: MirPlace::Local(place_local),
            init: MirRvalue::Use(MirOperand::Local(init_local)),
        } if place_local == &let_local && init_local == &param_local
    ));
    assert!(matches!(
        &body.blocks[0].terminator,
        MirTerminator::Return(Some(MirRvalue::Use(MirOperand::Local(return_local))))
            if return_local == &let_local
    ));
}

#[test]
fn mir_builder_extracts_direct_calls_from_mast_exprs() {
    let helper_id = MonoId(30);
    let seed = SymbolId(31);
    let value = SymbolId(32);
    let function = MastFunction {
        id: MonoId(4),
        name: "caller".to_string(),
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
        MirInstruction::Let {
            init: MirRvalue::Call {
                callee: MirCallTarget::Direct(id),
                args,
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
        MirInstruction::Assign {
            place: MirPlace::Global(id),
            op: AssignmentOperator::Assign,
            value: MirRvalue::Use(MirOperand::Const(_)),
        } if *id == global_id
    ));
}

#[test]
fn mir_builder_materializes_nested_operands_into_temps() {
    let helper_id = MonoId(35);
    let seed = SymbolId(36);
    let value = SymbolId(37);
    let function = MastFunction {
        id: MonoId(34),
        name: "nested_call_args".to_string(),
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
                    args: vec![expr(MastExprKind::Binary {
                        op: BinaryOperator::Add,
                        lhs: Box::new(expr(MastExprKind::Var(seed))),
                        rhs: Box::new(expr(MastExprKind::Integer(1))),
                    })],
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
    let value_local = body.locals[1].id;
    let temp_local = body.locals[2].id;

    assert!(matches!(
        &body.blocks[0].instructions[0],
        MirInstruction::Let {
            place: MirPlace::Local(place_local),
            init: MirRvalue::Binary {
                op: BinaryOperator::Add,
                lhs: MirOperand::Local(lhs),
                rhs: MirOperand::Const(_),
            },
        } if place_local == &temp_local && lhs == &param_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[1],
        MirInstruction::Let {
            place: MirPlace::Local(place_local),
            init: MirRvalue::Call {
                callee: MirCallTarget::Direct(id),
                args,
            },
        } if place_local == &value_local
            && id == &helper_id
            && matches!(args.as_slice(), [MirOperand::Local(local)] if local == &temp_local)
    ));
}

#[test]
fn mir_builder_extracts_structured_scalar_rvalues() {
    let seed = SymbolId(41);
    let neg = SymbolId(42);
    let sum = SymbolId(43);
    let casted = SymbolId(44);
    let function = MastFunction {
        id: MonoId(5),
        name: "scalar_ops".to_string(),
        linkage: MastLinkage::External,
        params: vec![MastParam {
            name: seed,
            ty: TypeId::I32,
            is_mut: false,
        }],
        ret_ty: TypeId::I64,
        body: Some(MastBlock {
            stmts: vec![
                MastStmt::Let {
                    name: neg,
                    ty: TypeId::I32,
                    is_mut: false,
                    init: expr(MastExprKind::Unary {
                        op: UnaryOperator::Negate,
                        operand: Box::new(expr(MastExprKind::Var(seed))),
                    }),
                },
                MastStmt::Let {
                    name: sum,
                    ty: TypeId::I32,
                    is_mut: false,
                    init: expr(MastExprKind::Binary {
                        op: BinaryOperator::Add,
                        lhs: Box::new(expr(MastExprKind::Var(seed))),
                        rhs: Box::new(expr(MastExprKind::Var(neg))),
                    }),
                },
                MastStmt::Let {
                    name: casted,
                    ty: TypeId::I64,
                    is_mut: false,
                    init: MastExpr::new(
                        TypeId::I64,
                        MastExprKind::Cast {
                            kind: MastCastKind::SignExt,
                            operand: Box::new(expr(MastExprKind::Var(sum))),
                        },
                        Span::default(),
                    ),
                },
            ],
            result: Some(Box::new(MastExpr::new(
                TypeId::I64,
                MastExprKind::Var(casted),
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
    let param_local = body.locals[0].id;
    let neg_local = body.locals[1].id;
    let sum_local = body.locals[2].id;

    assert!(matches!(
        &body.blocks[0].instructions[0],
        MirInstruction::Let {
            init: MirRvalue::Unary {
                op: UnaryOperator::Negate,
                operand: MirOperand::Local(local),
            },
            ..
        } if local == &param_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[1],
        MirInstruction::Let {
            init: MirRvalue::Binary {
                op: BinaryOperator::Add,
                lhs: MirOperand::Local(lhs),
                rhs: MirOperand::Local(rhs),
            },
            ..
        } if lhs == &param_local && rhs == &neg_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[2],
        MirInstruction::Let {
            init: MirRvalue::Cast {
                kind: MirCastKind::SignExt,
                operand: MirOperand::Local(local),
            },
            ..
        } if local == &sum_local
    ));
}

#[test]
fn mir_builder_extracts_address_of_and_load_places() {
    let ptr = SymbolId(51);
    let value = SymbolId(52);
    let addr = SymbolId(53);
    let loaded = SymbolId(54);
    let function = MastFunction {
        id: MonoId(6),
        name: "memory_ops".to_string(),
        linkage: MastLinkage::External,
        params: vec![
            MastParam {
                name: ptr,
                ty: TypeId::USIZE,
                is_mut: false,
            },
            MastParam {
                name: value,
                ty: TypeId::I32,
                is_mut: false,
            },
        ],
        ret_ty: TypeId::I32,
        body: Some(MastBlock {
            stmts: vec![
                MastStmt::Let {
                    name: addr,
                    ty: TypeId::USIZE,
                    is_mut: false,
                    init: MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::AddressOf(Box::new(expr(MastExprKind::Var(value)))),
                        Span::default(),
                    ),
                },
                MastStmt::Let {
                    name: loaded,
                    ty: TypeId::I32,
                    is_mut: false,
                    init: expr(MastExprKind::Deref(Box::new(MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::Var(ptr),
                        Span::default(),
                    )))),
                },
            ],
            result: Some(Box::new(expr(MastExprKind::Var(loaded)))),
            defers: vec![],
        }),
        is_extern: false,
        is_variadic: false,
        inline_hint: MastInlineHint::None,
        attributes: vec![],
    };

    let report = build_from_mast_unoptimized(&module_with_function(function));
    let body = report.module.functions[0].body.as_ref().unwrap();
    let ptr_local = body.locals[0].id;
    let value_local = body.locals[1].id;

    assert!(matches!(
        &body.blocks[0].instructions[0],
        MirInstruction::Let {
            init: MirRvalue::AddressOf(MirPlace::Local(local)),
            ..
        } if local == &value_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[1],
        MirInstruction::Let {
            init: MirRvalue::Load(MirPlace::Deref(MirOperand::Local(local))),
            ..
        } if local == &ptr_local
    ));
}

#[test]
fn mir_builder_extracts_assignment_instruction() {
    let seed = SymbolId(61);
    let value = SymbolId(62);
    let function = MastFunction {
        id: MonoId(7),
        name: "assign_ops".to_string(),
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
                    name: value,
                    ty: TypeId::I32,
                    is_mut: true,
                    init: expr(MastExprKind::Var(seed)),
                },
                MastStmt::Expr(MastExpr::new(
                    TypeId::I32,
                    MastExprKind::Assign {
                        op: AssignmentOperator::Assign,
                        lhs: Box::new(expr(MastExprKind::Var(value))),
                        rhs: Box::new(expr(MastExprKind::Binary {
                            op: BinaryOperator::Add,
                            lhs: Box::new(expr(MastExprKind::Var(seed))),
                            rhs: Box::new(expr(MastExprKind::Integer(1))),
                        })),
                    },
                    Span::default(),
                )),
            ],
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
    let value_local = body.locals[1].id;
    let seed_local = body.locals[0].id;

    assert!(matches!(
        &body.blocks[0].instructions[1],
        MirInstruction::Assign {
            place: MirPlace::Local(place_local),
            op: AssignmentOperator::Assign,
            value: MirRvalue::Binary {
                op: BinaryOperator::Add,
                lhs: MirOperand::Local(lhs),
                rhs: MirOperand::Const(_),
            },
        } if place_local == &value_local && lhs == &seed_local
    ));
}

#[test]
fn mir_builder_extracts_bit_and_atomic_operations() {
    let ptr = SymbolId(66);
    let value = SymbolId(67);
    let bits = SymbolId(68);
    let loaded = SymbolId(69);
    let swapped = SymbolId(70);
    let function = MastFunction {
        id: MonoId(71),
        name: "atomics".to_string(),
        linkage: MastLinkage::External,
        params: vec![
            MastParam {
                name: ptr,
                ty: TypeId::USIZE,
                is_mut: false,
            },
            MastParam {
                name: value,
                ty: TypeId::I32,
                is_mut: false,
            },
        ],
        ret_ty: TypeId::I32,
        body: Some(MastBlock {
            stmts: vec![
                MastStmt::Let {
                    name: bits,
                    ty: TypeId::I32,
                    is_mut: false,
                    init: expr(MastExprKind::BitIntrinsic {
                        kind: kernc_mast::BitIntrinsicKind::PopCount,
                        operand: Box::new(expr(MastExprKind::Var(value))),
                    }),
                },
                MastStmt::Let {
                    name: loaded,
                    ty: TypeId::I32,
                    is_mut: false,
                    init: expr(MastExprKind::AtomicLoad {
                        ptr: Box::new(MastExpr::new(
                            TypeId::USIZE,
                            MastExprKind::Var(ptr),
                            Span::default(),
                        )),
                        ordering: AtomicOrdering::Acquire,
                    }),
                },
                MastStmt::Expr(void_expr(MastExprKind::AtomicStore {
                    ptr: Box::new(MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::Var(ptr),
                        Span::default(),
                    )),
                    value: Box::new(expr(MastExprKind::Var(loaded))),
                    ordering: AtomicOrdering::Release,
                })),
                MastStmt::Expr(void_expr(MastExprKind::Fence {
                    ordering: AtomicOrdering::SeqCst,
                })),
                MastStmt::Let {
                    name: swapped,
                    ty: TypeId::I32,
                    is_mut: false,
                    init: expr(MastExprKind::AtomicRmw {
                        op: AtomicRmwOp::Xchg,
                        ptr: Box::new(MastExpr::new(
                            TypeId::USIZE,
                            MastExprKind::Var(ptr),
                            Span::default(),
                        )),
                        value: Box::new(expr(MastExprKind::Var(bits))),
                        ordering: AtomicOrdering::AcqRel,
                    }),
                },
            ],
            result: Some(Box::new(expr(MastExprKind::AtomicCas {
                weak: false,
                ptr: Box::new(MastExpr::new(
                    TypeId::USIZE,
                    MastExprKind::Var(ptr),
                    Span::default(),
                )),
                expected: Box::new(expr(MastExprKind::Var(swapped))),
                desired: Box::new(expr(MastExprKind::Var(value))),
                success: AtomicOrdering::AcqRel,
                failure: AtomicOrdering::Acquire,
            }))),
            defers: vec![],
        }),
        is_extern: false,
        is_variadic: false,
        inline_hint: MastInlineHint::None,
        attributes: vec![],
    };

    let report = build_from_mast_unoptimized(&module_with_function(function));
    let body = report.module.functions[0].body.as_ref().unwrap();
    let ptr_local = body.locals[0].id;
    let value_local = body.locals[1].id;
    let bits_local = body.locals[2].id;
    let loaded_local = body.locals[3].id;
    let swapped_local = body.locals[4].id;

    assert!(matches!(
        &body.blocks[0].instructions[0],
        MirInstruction::Let {
            place: MirPlace::Local(place_local),
            init: MirRvalue::BitIntrinsic {
                kind: MirBitIntrinsicKind::PopCount,
                operand: MirOperand::Local(local),
            },
        } if place_local == &bits_local && local == &value_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[1],
        MirInstruction::Let {
            place: MirPlace::Local(place_local),
            init: MirRvalue::AtomicLoad {
                ptr: MirOperand::Local(local),
                ordering: AtomicOrdering::Acquire,
            },
        } if place_local == &loaded_local && local == &ptr_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[2],
        MirInstruction::AtomicStore {
            ptr: MirOperand::Local(ptr_ref),
            value: MirOperand::Local(value_ref),
            ordering: AtomicOrdering::Release,
        } if ptr_ref == &ptr_local && value_ref == &loaded_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[3],
        MirInstruction::Fence {
            ordering: AtomicOrdering::SeqCst,
        }
    ));
    assert!(matches!(
        &body.blocks[0].instructions[4],
        MirInstruction::Let {
            place: MirPlace::Local(place_local),
            init: MirRvalue::AtomicRmw {
                op: AtomicRmwOp::Xchg,
                ptr: MirOperand::Local(ptr_ref),
                value: MirOperand::Local(value_ref),
                ordering: AtomicOrdering::AcqRel,
            },
        } if place_local == &swapped_local && ptr_ref == &ptr_local && value_ref == &bits_local
    ));
    assert!(matches!(
        &body.blocks[0].terminator,
        MirTerminator::Return(Some(MirRvalue::AtomicCas {
            weak: false,
            ptr: MirOperand::Local(ptr_ref),
            expected: MirOperand::Local(expected_ref),
            desired: MirOperand::Local(desired_ref),
            success: AtomicOrdering::AcqRel,
            failure: AtomicOrdering::Acquire,
        })) if ptr_ref == &ptr_local && expected_ref == &swapped_local && desired_ref == &value_local
    ));
}

#[test]
fn mir_builder_extracts_inline_asm_instruction() {
    let input = SymbolId(72);
    let output = SymbolId(73);
    let function = MastFunction {
        id: MonoId(74),
        name: "asm_ops".to_string(),
        linkage: MastLinkage::External,
        params: vec![
            MastParam {
                name: input,
                ty: TypeId::I32,
                is_mut: false,
            },
            MastParam {
                name: output,
                ty: TypeId::USIZE,
                is_mut: false,
            },
        ],
        ret_ty: TypeId::VOID,
        body: Some(MastBlock {
            stmts: vec![MastStmt::Expr(void_expr(MastExprKind::Asm(MastAsmBlock {
                asm_template: "mov eax, eax".to_string(),
                constraints: "={eax},{eax}".to_string(),
                input_args: vec![expr(MastExprKind::Var(input))],
                output_ptrs: vec![MastExpr::new(
                    TypeId::USIZE,
                    MastExprKind::Var(output),
                    Span::default(),
                )],
                output_tys: vec![TypeId::I32],
                is_volatile: true,
            })))],
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
    let input_local = body.locals[0].id;
    let output_local = body.locals[1].id;

    assert!(matches!(
        &body.blocks[0].instructions[0],
        MirInstruction::InlineAsm(asm)
            if asm.asm_template == "mov eax, eax"
                && asm.constraints == "={eax},{eax}"
                && asm.is_volatile
                && asm.output_tys.as_slice() == [TypeId::I32]
                && matches!(asm.input_args.as_slice(), [MirOperand::Local(local)] if local == &input_local)
                && matches!(asm.output_ptrs.as_slice(), [MirOperand::Local(local)] if local == &output_local)
    ));
    assert!(matches!(
        body.blocks[0].terminator,
        MirTerminator::Return(None)
    ));
}

#[test]
fn mir_builder_extracts_simd_memory_and_slice_operations() {
    let ptr = SymbolId(91);
    let indices = SymbolId(92);
    let mask = SymbolId(93);
    let fallback = SymbolId(94);
    let array = SymbolId(95);
    let loaded = SymbolId(96);
    let masked = SymbolId(97);
    let gathered = SymbolId(98);
    let masked_gathered = SymbolId(99);
    let slice = SymbolId(100);
    let function = MastFunction {
        id: MonoId(101),
        name: "simd_memory".to_string(),
        linkage: MastLinkage::External,
        params: vec![
            MastParam {
                name: ptr,
                ty: TypeId::USIZE,
                is_mut: false,
            },
            MastParam {
                name: indices,
                ty: TypeId::USIZE,
                is_mut: false,
            },
            MastParam {
                name: mask,
                ty: TypeId(201),
                is_mut: false,
            },
            MastParam {
                name: fallback,
                ty: TypeId(200),
                is_mut: false,
            },
            MastParam {
                name: array,
                ty: TypeId(202),
                is_mut: true,
            },
        ],
        ret_ty: TypeId(203),
        body: Some(MastBlock {
            stmts: vec![
                MastStmt::Let {
                    name: loaded,
                    ty: TypeId(200),
                    is_mut: false,
                    init: MastExpr::new(
                        TypeId(200),
                        MastExprKind::SimdLoad {
                            ptr: Box::new(MastExpr::new(
                                TypeId::USIZE,
                                MastExprKind::Var(ptr),
                                Span::default(),
                            )),
                            align: 16,
                        },
                        Span::default(),
                    ),
                },
                MastStmt::Let {
                    name: masked,
                    ty: TypeId(200),
                    is_mut: false,
                    init: MastExpr::new(
                        TypeId(200),
                        MastExprKind::SimdMaskedLoad {
                            ptr: Box::new(MastExpr::new(
                                TypeId::USIZE,
                                MastExprKind::Var(ptr),
                                Span::default(),
                            )),
                            mask: Box::new(MastExpr::new(
                                TypeId(201),
                                MastExprKind::Var(mask),
                                Span::default(),
                            )),
                            or_else: Box::new(MastExpr::new(
                                TypeId(200),
                                MastExprKind::Var(fallback),
                                Span::default(),
                            )),
                            align: 16,
                        },
                        Span::default(),
                    ),
                },
                MastStmt::Expr(void_expr(MastExprKind::SimdStore {
                    ptr: Box::new(MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::Var(ptr),
                        Span::default(),
                    )),
                    value: Box::new(MastExpr::new(
                        TypeId(200),
                        MastExprKind::Var(loaded),
                        Span::default(),
                    )),
                    align: 16,
                })),
                MastStmt::Expr(void_expr(MastExprKind::SimdMaskedStore {
                    ptr: Box::new(MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::Var(ptr),
                        Span::default(),
                    )),
                    mask: Box::new(MastExpr::new(
                        TypeId(201),
                        MastExprKind::Var(mask),
                        Span::default(),
                    )),
                    value: Box::new(MastExpr::new(
                        TypeId(200),
                        MastExprKind::Var(masked),
                        Span::default(),
                    )),
                    align: 16,
                })),
                MastStmt::Let {
                    name: gathered,
                    ty: TypeId(200),
                    is_mut: false,
                    init: MastExpr::new(
                        TypeId(200),
                        MastExprKind::SimdGather {
                            ptr: Box::new(MastExpr::new(
                                TypeId::USIZE,
                                MastExprKind::Var(ptr),
                                Span::default(),
                            )),
                            indices: Box::new(MastExpr::new(
                                TypeId::USIZE,
                                MastExprKind::Var(indices),
                                Span::default(),
                            )),
                        },
                        Span::default(),
                    ),
                },
                MastStmt::Expr(void_expr(MastExprKind::SimdScatter {
                    ptr: Box::new(MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::Var(ptr),
                        Span::default(),
                    )),
                    indices: Box::new(MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::Var(indices),
                        Span::default(),
                    )),
                    value: Box::new(MastExpr::new(
                        TypeId(200),
                        MastExprKind::Var(gathered),
                        Span::default(),
                    )),
                })),
                MastStmt::Let {
                    name: masked_gathered,
                    ty: TypeId(200),
                    is_mut: false,
                    init: MastExpr::new(
                        TypeId(200),
                        MastExprKind::SimdMaskedGather {
                            ptr: Box::new(MastExpr::new(
                                TypeId::USIZE,
                                MastExprKind::Var(ptr),
                                Span::default(),
                            )),
                            indices: Box::new(MastExpr::new(
                                TypeId::USIZE,
                                MastExprKind::Var(indices),
                                Span::default(),
                            )),
                            mask: Box::new(MastExpr::new(
                                TypeId(201),
                                MastExprKind::Var(mask),
                                Span::default(),
                            )),
                            or_else: Box::new(MastExpr::new(
                                TypeId(200),
                                MastExprKind::Var(fallback),
                                Span::default(),
                            )),
                        },
                        Span::default(),
                    ),
                },
                MastStmt::Expr(void_expr(MastExprKind::SimdMaskedScatter {
                    ptr: Box::new(MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::Var(ptr),
                        Span::default(),
                    )),
                    indices: Box::new(MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::Var(indices),
                        Span::default(),
                    )),
                    mask: Box::new(MastExpr::new(
                        TypeId(201),
                        MastExprKind::Var(mask),
                        Span::default(),
                    )),
                    value: Box::new(MastExpr::new(
                        TypeId(200),
                        MastExprKind::Var(masked_gathered),
                        Span::default(),
                    )),
                })),
                MastStmt::Let {
                    name: slice,
                    ty: TypeId(203),
                    is_mut: false,
                    init: MastExpr::new(
                        TypeId(203),
                        MastExprKind::SliceOp {
                            lhs: Box::new(MastExpr::new(
                                TypeId(202),
                                MastExprKind::Var(array),
                                Span::default(),
                            )),
                            start: Some(Box::new(expr(MastExprKind::Integer(1)))),
                            end: Some(Box::new(expr(MastExprKind::Integer(3)))),
                            is_inclusive: false,
                        },
                        Span::default(),
                    ),
                },
            ],
            result: Some(Box::new(MastExpr::new(
                TypeId(203),
                MastExprKind::Var(slice),
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
    let ptr_local = body.locals[0].id;
    let indices_local = body.locals[1].id;
    let mask_local = body.locals[2].id;
    let fallback_local = body.locals[3].id;
    let array_local = body.locals[4].id;
    let loaded_local = body.locals[5].id;
    let masked_local = body.locals[6].id;
    let gathered_local = body.locals[7].id;
    let masked_gathered_local = body.locals[8].id;
    let slice_local = body.locals[9].id;

    assert!(matches!(
        &body.blocks[0].instructions[0],
        MirInstruction::Let {
            place: MirPlace::Local(place_local),
            init: MirRvalue::SimdLoad {
                ptr: MirOperand::Local(ptr_ref),
                align: 16,
            },
        } if place_local == &loaded_local && ptr_ref == &ptr_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[1],
        MirInstruction::Let {
            place: MirPlace::Local(place_local),
            init: MirRvalue::SimdMaskedLoad {
                ptr: MirOperand::Local(ptr_ref),
                mask: MirOperand::Local(mask_ref),
                or_else: MirOperand::Local(fallback_ref),
                align: 16,
            },
        } if place_local == &masked_local
            && ptr_ref == &ptr_local
            && mask_ref == &mask_local
            && fallback_ref == &fallback_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[2],
        MirInstruction::SimdStore {
            ptr: MirOperand::Local(ptr_ref),
            value: MirOperand::Local(value_ref),
            align: 16,
        } if ptr_ref == &ptr_local && value_ref == &loaded_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[3],
        MirInstruction::SimdMaskedStore {
            ptr: MirOperand::Local(ptr_ref),
            mask: MirOperand::Local(mask_ref),
            value: MirOperand::Local(value_ref),
            align: 16,
        } if ptr_ref == &ptr_local && mask_ref == &mask_local && value_ref == &masked_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[4],
        MirInstruction::Let {
            place: MirPlace::Local(place_local),
            init: MirRvalue::SimdGather {
                ptr: MirOperand::Local(ptr_ref),
                indices: MirOperand::Local(indices_ref),
            },
        } if place_local == &gathered_local && ptr_ref == &ptr_local && indices_ref == &indices_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[5],
        MirInstruction::SimdScatter {
            ptr: MirOperand::Local(ptr_ref),
            indices: MirOperand::Local(indices_ref),
            value: MirOperand::Local(value_ref),
        } if ptr_ref == &ptr_local && indices_ref == &indices_local && value_ref == &gathered_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[6],
        MirInstruction::Let {
            place: MirPlace::Local(place_local),
            init: MirRvalue::SimdMaskedGather {
                ptr: MirOperand::Local(ptr_ref),
                indices: MirOperand::Local(indices_ref),
                mask: MirOperand::Local(mask_ref),
                or_else: MirOperand::Local(fallback_ref),
            },
        } if place_local == &masked_gathered_local
            && ptr_ref == &ptr_local
            && indices_ref == &indices_local
            && mask_ref == &mask_local
            && fallback_ref == &fallback_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[7],
        MirInstruction::SimdMaskedScatter {
            ptr: MirOperand::Local(ptr_ref),
            indices: MirOperand::Local(indices_ref),
            mask: MirOperand::Local(mask_ref),
            value: MirOperand::Local(value_ref),
        } if ptr_ref == &ptr_local
            && indices_ref == &indices_local
            && mask_ref == &mask_local
            && value_ref == &masked_gathered_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[8],
        MirInstruction::Let {
            place: MirPlace::Local(place_local),
            init: MirRvalue::SliceOp {
                lhs: MirSliceBase::Place(MirPlace::Local(base_local)),
                start: Some(MirOperand::Const(_)),
                end: Some(MirOperand::Const(_)),
                is_inclusive: false,
            },
        } if place_local == &slice_local && base_local == &array_local
    ));
}

#[test]
fn mir_builder_extracts_pure_simd_rvalues() {
    let lhs = SymbolId(81);
    let rhs = SymbolId(82);
    let sum = SymbolId(83);
    let mins = SymbolId(84);
    let mask = SymbolId(85);
    let mixed = SymbolId(86);
    let function = MastFunction {
        id: MonoId(87),
        name: "simd_ops".to_string(),
        linkage: MastLinkage::External,
        params: vec![
            MastParam {
                name: lhs,
                ty: TypeId(200),
                is_mut: false,
            },
            MastParam {
                name: rhs,
                ty: TypeId(200),
                is_mut: false,
            },
        ],
        ret_ty: TypeId(200),
        body: Some(MastBlock {
            stmts: vec![
                MastStmt::Let {
                    name: sum,
                    ty: TypeId(200),
                    is_mut: false,
                    init: MastExpr::new(
                        TypeId(200),
                        MastExprKind::Binary {
                            op: BinaryOperator::Add,
                            lhs: Box::new(MastExpr::new(
                                TypeId(200),
                                MastExprKind::Var(lhs),
                                Span::default(),
                            )),
                            rhs: Box::new(MastExpr::new(
                                TypeId(200),
                                MastExprKind::Var(rhs),
                                Span::default(),
                            )),
                        },
                        Span::default(),
                    ),
                },
                MastStmt::Let {
                    name: mins,
                    ty: TypeId(200),
                    is_mut: false,
                    init: MastExpr::new(
                        TypeId(200),
                        MastExprKind::SimdBinaryIntrinsic {
                            kind: kernc_mast::SimdBinaryIntrinsicKind::Min,
                            lhs: Box::new(MastExpr::new(
                                TypeId(200),
                                MastExprKind::Var(lhs),
                                Span::default(),
                            )),
                            rhs: Box::new(MastExpr::new(
                                TypeId(200),
                                MastExprKind::Var(rhs),
                                Span::default(),
                            )),
                        },
                        Span::default(),
                    ),
                },
                MastStmt::Let {
                    name: mask,
                    ty: TypeId(201),
                    is_mut: false,
                    init: MastExpr::new(
                        TypeId(201),
                        MastExprKind::Binary {
                            op: BinaryOperator::LessThan,
                            lhs: Box::new(MastExpr::new(
                                TypeId(200),
                                MastExprKind::Var(lhs),
                                Span::default(),
                            )),
                            rhs: Box::new(MastExpr::new(
                                TypeId(200),
                                MastExprKind::Var(rhs),
                                Span::default(),
                            )),
                        },
                        Span::default(),
                    ),
                },
                MastStmt::Let {
                    name: mixed,
                    ty: TypeId(200),
                    is_mut: false,
                    init: MastExpr::new(
                        TypeId(200),
                        MastExprKind::SimdSelect {
                            mask: Box::new(MastExpr::new(
                                TypeId(201),
                                MastExprKind::Var(mask),
                                Span::default(),
                            )),
                            on_true: Box::new(MastExpr::new(
                                TypeId(200),
                                MastExprKind::Var(sum),
                                Span::default(),
                            )),
                            on_false: Box::new(MastExpr::new(
                                TypeId(200),
                                MastExprKind::Var(mins),
                                Span::default(),
                            )),
                        },
                        Span::default(),
                    ),
                },
            ],
            result: Some(Box::new(MastExpr::new(
                TypeId(200),
                MastExprKind::Var(mixed),
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
    let lhs_local = body.locals[0].id;
    let rhs_local = body.locals[1].id;
    let sum_local = body.locals[2].id;
    let mins_local = body.locals[3].id;
    let mask_local = body.locals[4].id;

    assert!(matches!(
        &body.blocks[0].instructions[0],
        MirInstruction::Let {
            place: MirPlace::Local(place_local),
            init: MirRvalue::Binary {
                op: BinaryOperator::Add,
                lhs: MirOperand::Local(lhs_ref),
                rhs: MirOperand::Local(rhs_ref),
            },
        } if place_local == &sum_local && lhs_ref == &lhs_local && rhs_ref == &rhs_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[1],
        MirInstruction::Let {
            place: MirPlace::Local(place_local),
            init: MirRvalue::SimdBinaryIntrinsic {
                kind: MirSimdBinaryIntrinsicKind::Min,
                lhs: MirOperand::Local(lhs_ref),
                rhs: MirOperand::Local(rhs_ref),
            },
        } if place_local == &mins_local && lhs_ref == &lhs_local && rhs_ref == &rhs_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[2],
        MirInstruction::Let {
            place: MirPlace::Local(place_local),
            init: MirRvalue::Binary {
                op: BinaryOperator::LessThan,
                lhs: MirOperand::Local(lhs_ref),
                rhs: MirOperand::Local(rhs_ref),
            },
        } if place_local == &mask_local && lhs_ref == &lhs_local && rhs_ref == &rhs_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[3],
        MirInstruction::Let {
            init: MirRvalue::SimdSelect {
                mask: MirOperand::Local(mask_ref),
                on_true: MirOperand::Local(true_ref),
                on_false: MirOperand::Local(false_ref),
            },
            ..
        } if mask_ref == &mask_local && true_ref == &sum_local && false_ref == &mins_local
    ));
}

#[test]
fn mir_builder_extracts_aggregate_rvalues() {
    let lhs = SymbolId(71);
    let rhs = SymbolId(72);
    let pair = SymbolId(73);
    let array = SymbolId(74);
    let fat = SymbolId(75);
    let tagged = SymbolId(76);
    let struct_id = MonoId(81);
    let data_struct_id = MonoId(82);
    let function = MastFunction {
        id: MonoId(8),
        name: "aggregates".to_string(),
        linkage: MastLinkage::External,
        params: vec![
            MastParam {
                name: lhs,
                ty: TypeId::USIZE,
                is_mut: false,
            },
            MastParam {
                name: rhs,
                ty: TypeId::USIZE,
                is_mut: false,
            },
        ],
        ret_ty: TypeId::USIZE,
        body: Some(MastBlock {
            stmts: vec![
                MastStmt::Let {
                    name: pair,
                    ty: TypeId::USIZE,
                    is_mut: false,
                    init: MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::StructInit {
                            struct_id,
                            fields: vec![
                                MastExpr::new(
                                    TypeId::USIZE,
                                    MastExprKind::Var(lhs),
                                    Span::default(),
                                ),
                                MastExpr::new(
                                    TypeId::USIZE,
                                    MastExprKind::Var(rhs),
                                    Span::default(),
                                ),
                            ],
                        },
                        Span::default(),
                    ),
                },
                MastStmt::Let {
                    name: array,
                    ty: TypeId::USIZE,
                    is_mut: false,
                    init: MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::ArrayInit(vec![
                            MastExpr::new(TypeId::USIZE, MastExprKind::Var(lhs), Span::default()),
                            MastExpr::new(TypeId::USIZE, MastExprKind::Integer(7), Span::default()),
                        ]),
                        Span::default(),
                    ),
                },
                MastStmt::Let {
                    name: fat,
                    ty: TypeId::USIZE,
                    is_mut: false,
                    init: MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::ConstructFatPointer {
                            data_ptr: Box::new(MastExpr::new(
                                TypeId::USIZE,
                                MastExprKind::Var(lhs),
                                Span::default(),
                            )),
                            meta: Box::new(MastExpr::new(
                                TypeId::USIZE,
                                MastExprKind::Var(rhs),
                                Span::default(),
                            )),
                        },
                        Span::default(),
                    ),
                },
                MastStmt::Let {
                    name: tagged,
                    ty: TypeId::USIZE,
                    is_mut: false,
                    init: MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::DataInit {
                            data_struct_id,
                            tag_value: 3,
                            payload: Box::new(MastExpr::new(
                                TypeId::USIZE,
                                MastExprKind::Var(lhs),
                                Span::default(),
                            )),
                        },
                        Span::default(),
                    ),
                },
            ],
            result: Some(Box::new(MastExpr::new(
                TypeId::USIZE,
                MastExprKind::Var(lhs),
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
    let lhs_local = body.locals[0].id;
    let rhs_local = body.locals[1].id;

    assert!(matches!(
        &body.blocks[0].instructions[0],
        MirInstruction::Let {
            init: MirRvalue::Aggregate {
                kind: MirAggregateKind::Struct { struct_id: got },
                fields,
                ..
            },
            ..
        } if got == &struct_id
            && matches!(fields.as_slice(), [MirOperand::Local(a), MirOperand::Local(b)] if a == &lhs_local && b == &rhs_local)
    ));
    assert!(matches!(
        &body.blocks[0].instructions[1],
        MirInstruction::Let {
            init: MirRvalue::Aggregate {
                kind: MirAggregateKind::Array,
                fields,
                ..
            },
            ..
        } if matches!(fields.as_slice(), [MirOperand::Local(a), MirOperand::Const(_)] if a == &lhs_local)
    ));
    assert!(matches!(
        &body.blocks[0].instructions[2],
        MirInstruction::Let {
            init: MirRvalue::Aggregate {
                kind: MirAggregateKind::FatPointer,
                fields,
                ..
            },
            ..
        } if matches!(fields.as_slice(), [MirOperand::Local(a), MirOperand::Local(b)] if a == &lhs_local && b == &rhs_local)
    ));
    assert!(matches!(
        &body.blocks[0].instructions[3],
        MirInstruction::Let {
            init: MirRvalue::Aggregate {
                kind: MirAggregateKind::Data { data_struct_id: got, tag_value: 3 },
                fields,
                ..
            },
            ..
        } if got == &data_struct_id
            && matches!(fields.as_slice(), [MirOperand::Local(a)] if a == &lhs_local)
    ));
    assert!(report.workload.aggregate_rvalues >= 4);
}

#[test]
fn mir_builder_extracts_fat_pointer_projection_rvalues() {
    let fat = SymbolId(91);
    let data = SymbolId(92);
    let meta = SymbolId(93);
    let function = MastFunction {
        id: MonoId(10),
        name: "fat_proj".to_string(),
        linkage: MastLinkage::External,
        params: vec![MastParam {
            name: fat,
            ty: TypeId::USIZE,
            is_mut: false,
        }],
        ret_ty: TypeId::USIZE,
        body: Some(MastBlock {
            stmts: vec![
                MastStmt::Let {
                    name: data,
                    ty: TypeId::USIZE,
                    is_mut: false,
                    init: MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::ExtractFatPtrData(Box::new(MastExpr::new(
                            TypeId::USIZE,
                            MastExprKind::Var(fat),
                            Span::default(),
                        ))),
                        Span::default(),
                    ),
                },
                MastStmt::Let {
                    name: meta,
                    ty: TypeId::USIZE,
                    is_mut: false,
                    init: MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::ExtractFatPtrMeta(Box::new(MastExpr::new(
                            TypeId::USIZE,
                            MastExprKind::Var(fat),
                            Span::default(),
                        ))),
                        Span::default(),
                    ),
                },
            ],
            result: Some(Box::new(MastExpr::new(
                TypeId::USIZE,
                MastExprKind::Var(data),
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
    let fat_local = body.locals[0].id;

    assert!(matches!(
        &body.blocks[0].instructions[0],
        MirInstruction::Let {
            init: MirRvalue::Projection {
                kind: MirProjectionKind::FatPtrData,
                operand: MirOperand::Local(local),
            },
            ..
        } if local == &fat_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[1],
        MirInstruction::Let {
            init: MirRvalue::Projection {
                kind: MirProjectionKind::FatPtrMeta,
                operand: MirOperand::Local(local),
            },
            ..
        } if local == &fat_local
    ));
    assert!(report.workload.projection_rvalues >= 2);
}

#[test]
fn mir_builder_extracts_memory_intrinsics() {
    let dest = SymbolId(101);
    let src = SymbolId(102);
    let len = SymbolId(103);
    let val = SymbolId(104);
    let function = MastFunction {
        id: MonoId(11),
        name: "memory_intrinsics".to_string(),
        linkage: MastLinkage::External,
        params: vec![
            MastParam {
                name: dest,
                ty: TypeId::USIZE,
                is_mut: false,
            },
            MastParam {
                name: src,
                ty: TypeId::USIZE,
                is_mut: false,
            },
            MastParam {
                name: len,
                ty: TypeId::USIZE,
                is_mut: false,
            },
            MastParam {
                name: val,
                ty: TypeId::U8,
                is_mut: false,
            },
        ],
        ret_ty: TypeId::VOID,
        body: Some(MastBlock {
            stmts: vec![
                MastStmt::Expr(MastExpr::new(
                    TypeId::VOID,
                    MastExprKind::Memcpy {
                        dest: Box::new(MastExpr::new(
                            TypeId::USIZE,
                            MastExprKind::Var(dest),
                            Span::default(),
                        )),
                        src: Box::new(MastExpr::new(
                            TypeId::USIZE,
                            MastExprKind::Var(src),
                            Span::default(),
                        )),
                        len: Box::new(MastExpr::new(
                            TypeId::USIZE,
                            MastExprKind::Var(len),
                            Span::default(),
                        )),
                    },
                    Span::default(),
                )),
                MastStmt::Expr(MastExpr::new(
                    TypeId::VOID,
                    MastExprKind::Memmove {
                        dest: Box::new(MastExpr::new(
                            TypeId::USIZE,
                            MastExprKind::Var(dest),
                            Span::default(),
                        )),
                        src: Box::new(MastExpr::new(
                            TypeId::USIZE,
                            MastExprKind::Var(src),
                            Span::default(),
                        )),
                        len: Box::new(MastExpr::new(
                            TypeId::USIZE,
                            MastExprKind::Var(len),
                            Span::default(),
                        )),
                    },
                    Span::default(),
                )),
                MastStmt::Expr(MastExpr::new(
                    TypeId::VOID,
                    MastExprKind::Memset {
                        dest: Box::new(MastExpr::new(
                            TypeId::USIZE,
                            MastExprKind::Var(dest),
                            Span::default(),
                        )),
                        val: Box::new(MastExpr::new(
                            TypeId::U8,
                            MastExprKind::Var(val),
                            Span::default(),
                        )),
                        len: Box::new(MastExpr::new(
                            TypeId::USIZE,
                            MastExprKind::Var(len),
                            Span::default(),
                        )),
                    },
                    Span::default(),
                )),
            ],
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
    let dest_local = body.locals[0].id;
    let src_local = body.locals[1].id;
    let len_local = body.locals[2].id;
    let val_local = body.locals[3].id;

    assert!(matches!(
        &body.blocks[0].instructions[0],
        MirInstruction::Memory(MirMemoryIntrinsic::Copy {
            dest: MirOperand::Local(d),
            src: MirOperand::Local(s),
            len: MirOperand::Local(n),
        }) if d == &dest_local && s == &src_local && n == &len_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[1],
        MirInstruction::Memory(MirMemoryIntrinsic::Move {
            dest: MirOperand::Local(d),
            src: MirOperand::Local(s),
            len: MirOperand::Local(n),
        }) if d == &dest_local && s == &src_local && n == &len_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[2],
        MirInstruction::Memory(MirMemoryIntrinsic::Set {
            dest: MirOperand::Local(d),
            val: MirOperand::Local(v),
            len: MirOperand::Local(n),
        }) if d == &dest_local && v == &val_local && n == &len_local
    ));
    assert!(report.workload.memory_instructions >= 3);
}

#[test]
fn mir_pass_pipeline_forwards_trivial_local_copy_chains() {
    let seed = SymbolId(120);
    let first = SymbolId(121);
    let second = SymbolId(122);
    let function = MastFunction {
        id: MonoId(13),
        name: "copy_chain".to_string(),
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
    assert!(body.blocks[0].instructions.is_empty());
    assert!(matches!(
        &body.blocks[0].terminator,
        MirTerminator::Return(Some(MirRvalue::Use(MirOperand::Local(local))))
            if local == &param_local
    ));
}

#[test]
fn mir_pass_pipeline_folds_const_branch_after_copy_propagation() {
    let cond = SymbolId(130);
    let report = build_from_mast(&module_with_function(MastFunction {
        id: MonoId(14),
        name: "const_branch".to_string(),
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
    assert_eq!(body.blocks.len(), 2);
    let MirTerminator::Goto(target) = &body.blocks[0].terminator else {
        panic!("entry terminator should fold to goto");
    };
    assert!(matches!(
        &body.blocks[target.0 as usize].terminator,
        MirTerminator::Return(Some(MirRvalue::Use(MirOperand::Const(value))))
            if matches!(value, MirConst::Integer { value: 1, .. })
    ));
}

#[test]
fn mir_pass_pipeline_folds_const_switch_to_matching_case() {
    let report = build_from_mast(&module_with_function(MastFunction {
        id: MonoId(15),
        name: "const_switch".to_string(),
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
    assert_eq!(body.blocks.len(), 2);
    let MirTerminator::Goto(target) = &body.blocks[0].terminator else {
        panic!("entry terminator should fold to goto");
    };
    assert!(matches!(
        &body.blocks[target.0 as usize].terminator,
        MirTerminator::Return(Some(MirRvalue::Use(MirOperand::Const(value))))
            if matches!(value, MirConst::Integer { value: 20, .. })
    ));
}
