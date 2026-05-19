//! Scalar MIR builder tests.
//!
//! These cases cover arithmetic, casts, loads/stores, memory intrinsics, calls,
//! and basic expression-to-rvalue lowering without relying on frontend parsing.

use super::*;

#[test]
fn mir_builder_materializes_nested_operands_into_temps() {
    let helper_id = MonoId(35);
    let seed = SymbolId(36);
    let value = SymbolId(37);
    let function = MastFunction {
        id: MonoId(34),
        name: "nested_call_args".to_string(),
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
        MirInstructionData {
            kind: MirInstruction::Let {
                place: MirPlace::Local(place_local),
                init: MirRvalue::Binary {
                    op: BinaryOperator::Add,
                    lhs: MirOperand::Local(lhs),
                    rhs: MirOperand::Const(_),
                },
            },
            ..
        } if place_local == &temp_local && lhs == &param_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[1],
        MirInstructionData {
            kind: MirInstruction::Let {
                place: MirPlace::Local(place_local),
                init: MirRvalue::Call {
                    callee: MirCallTarget::Direct(id),
                    args,
                },
            },
            ..
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
        span: Span::default(),
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
        MirInstructionData {
            kind: MirInstruction::Let {
                init: MirRvalue::Unary {
                    op: UnaryOperator::Negate,
                    operand: MirOperand::Local(local),
                },
                ..
            },
            ..
        } if local == &param_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[1],
        MirInstructionData {
            kind: MirInstruction::Let {
                init: MirRvalue::Binary {
                    op: BinaryOperator::Add,
                    lhs: MirOperand::Local(lhs),
                    rhs: MirOperand::Local(rhs),
                },
                ..
            },
            ..
        } if lhs == &param_local && rhs == &neg_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[2],
        MirInstructionData {
            kind: MirInstruction::Let {
                init: MirRvalue::Cast {
                    kind: MirCastKind::SignExt,
                    target_ty,
                    operand: MirOperand::Local(local),
                },
                ..
            },
            ..
        } if target_ty == &TypeId::I64 && local == &sum_local
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
        span: Span::default(),
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
        MirInstructionData {
            kind: MirInstruction::Let {
                init: MirRvalue::AddressOf(MirPlace::Local(local)),
                ..
            },
            ..
        } if local == &value_local
    ));
    assert!(matches!(
        &body.blocks[0].instructions[1],
        MirInstructionData {
            kind: MirInstruction::Let {
                init: MirRvalue::Load(MirPlace::Deref(MirOperand::Local(local))),
                ..
            },
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
        MirInstructionData {
            kind: MirInstruction::Assign {
                place: MirPlace::Local(place_local),
                op: AssignmentOperator::Assign,
                value: MirRvalue::Binary {
                    op: BinaryOperator::Add,
                    lhs: MirOperand::Local(lhs),
                    rhs: MirOperand::Const(_),
                },
            },
            ..
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
        span: Span::default(),
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
    let return_local = body.locals[5].id;
    let instructions = body
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter())
        .collect::<Vec<_>>();

    assert!(matches!(
        instructions[0],
        MirInstructionData {
            kind: MirInstruction::Let {
                place: MirPlace::Local(place_local),
                init: MirRvalue::BitIntrinsic {
                    kind: MirBitIntrinsicKind::PopCount,
                    operand: MirOperand::Local(local),
                },
            },
            ..
        } if place_local == &bits_local && local == &value_local
    ));
    assert!(matches!(
        instructions[1],
        MirInstructionData {
            kind: MirInstruction::Let {
                place: MirPlace::Local(place_local),
                init: MirRvalue::AtomicLoad {
                    ptr: MirOperand::Local(local),
                    ordering: AtomicOrdering::Acquire,
                },
            },
            ..
        } if place_local == &loaded_local && local == &ptr_local
    ));
    assert!(matches!(
        instructions[2],
        MirInstructionData {
            kind: MirInstruction::AtomicStore {
                ptr: MirOperand::Local(ptr_ref),
                value: MirOperand::Local(value_ref),
                ordering: AtomicOrdering::Release,
            },
            ..
        } if ptr_ref == &ptr_local && value_ref == &loaded_local
    ));
    assert!(matches!(
        instructions[3],
        MirInstructionData {
            kind: MirInstruction::Fence {
                ordering: AtomicOrdering::SeqCst,
            },
            ..
        }
    ));
    assert!(matches!(
        instructions[4],
        MirInstructionData {
            kind: MirInstruction::Let {
                place: MirPlace::Local(place_local),
                init: MirRvalue::AtomicRmw {
                    op: AtomicRmwOp::Xchg,
                    ptr: MirOperand::Local(ptr_ref),
                    value: MirOperand::Local(value_ref),
                    ordering: AtomicOrdering::AcqRel,
                },
            },
            ..
        } if place_local == &swapped_local && ptr_ref == &ptr_local && value_ref == &bits_local
    ));
    assert!(body.blocks.iter().any(|block| matches!(
        block.instructions.get(5),
        Some(MirInstructionData {
            kind: MirInstruction::Assign {
                place: MirPlace::Local(place_local),
                op: AssignmentOperator::Assign,
                value: MirRvalue::AtomicCas {
                    weak: false,
                    ptr: MirOperand::Local(ptr_ref),
                    expected: MirOperand::Local(expected_ref),
                    desired: MirOperand::Local(desired_ref),
                    success: AtomicOrdering::AcqRel,
                    failure: AtomicOrdering::Acquire,
                },
            },
            ..
        }) if place_local == &return_local
            && ptr_ref == &ptr_local
            && expected_ref == &swapped_local
            && desired_ref == &value_local
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
fn mir_builder_extracts_inline_asm_instruction() {
    let input = SymbolId(72);
    let output = SymbolId(73);
    let function = MastFunction {
        id: MonoId(74),
        name: "asm_ops".to_string(),
        span: Span::default(),
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
        MirInstructionData {
            kind: MirInstruction::InlineAsm(asm),
            ..
        }
            if asm.asm_template == "mov eax, eax"
                && asm.constraints == "={eax},{eax}"
                && asm.is_volatile
                && asm.output_tys.as_slice() == [TypeId::I32]
                && matches!(asm.input_args.as_slice(), [MirOperand::Local(local)] if local == &input_local)
                && matches!(asm.output_ptrs.as_slice(), [MirOperand::Local(local)] if local == &output_local)
    ));
    assert!(matches!(
        body.blocks[0].terminator,
        MirTerminatorData {
            kind: MirTerminator::Return(None),
            ..
        }
    ));
}
