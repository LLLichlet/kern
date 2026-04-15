use super::*;

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
