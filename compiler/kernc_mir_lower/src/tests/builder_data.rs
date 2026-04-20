use super::*;

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
        span: Span::default(),
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
        span: Span::default(),
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
        span: Span::default(),
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
