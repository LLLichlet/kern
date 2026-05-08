use crate::{
    MirBlock, MirBlockId, MirBody, MirCallTarget, MirConst, MirDirectCalleeCallsiteCount,
    MirFunction, MirGlobal, MirInlineHint, MirInstruction, MirInstructionData, MirItemBodyRole,
    MirLinkage, MirLocal, MirLocalId, MirLocalKind, MirMemoryIntrinsic, MirModule, MirOperand,
    MirRvalue, MirStaticInit, MirTerminator, MirTerminatorData, run_default_pass_pipeline,
    verify_module,
};
use kernc_mono::{MonoId, MonoModuleMetadata};
use kernc_ty::TypeId;
use kernc_utils::{Span, SymbolId};

fn instr(kind: MirInstruction) -> MirInstructionData {
    MirInstructionData {
        span: Span::default(),
        kind,
    }
}

fn term(kind: MirTerminator) -> MirTerminatorData {
    MirTerminatorData {
        span: Span::default(),
        kind,
    }
}

#[test]
fn mir_verifier_rejects_out_of_range_local_refs() {
    let module = MirModule {
        name: "bad".to_string(),
        structs: vec![],
        globals: vec![],
        functions: vec![MirFunction {
            id: MonoId(9),
            name: "bad_fn".to_string(),
            span: Span::default(),
            linkage: MirLinkage::External,
            params: vec![],
            ret_ty: TypeId::VOID,
            body: Some(MirBody {
                entry: MirBlockId(0),
                locals: vec![MirLocal {
                    id: MirLocalId(0),
                    name: SymbolId(90),
                    span: Span::default(),
                    ty: TypeId::I32,
                    is_mut: false,
                    kind: MirLocalKind::Let,
                }],
                blocks: vec![MirBlock {
                    id: MirBlockId(0),
                    instructions: vec![instr(MirInstruction::Eval(MirRvalue::Use(
                        MirOperand::Local(MirLocalId(1)),
                    )))],
                    terminator: term(MirTerminator::Return(None)),
                }],
            }),
            is_extern: false,
            is_variadic: false,
            inline_hint: MirInlineHint::None,
            attributes: vec![],
        }],
        mono: MonoModuleMetadata::default(),
    };

    let error = verify_module(&module).expect_err("verifier should reject invalid local refs");
    assert!(error.function.contains("bad_fn"));
    assert!(error.message.contains("out of range"));
}

#[test]
fn mir_verifier_rejects_out_of_range_memory_refs() {
    let module = MirModule {
        name: "bad_mem".to_string(),
        structs: vec![],
        globals: vec![],
        functions: vec![MirFunction {
            id: MonoId(12),
            name: "bad_mem_fn".to_string(),
            span: Span::default(),
            linkage: MirLinkage::External,
            params: vec![],
            ret_ty: TypeId::VOID,
            body: Some(MirBody {
                entry: MirBlockId(0),
                locals: vec![MirLocal {
                    id: MirLocalId(0),
                    name: SymbolId(110),
                    span: Span::default(),
                    ty: TypeId::USIZE,
                    is_mut: false,
                    kind: MirLocalKind::Let,
                }],
                blocks: vec![MirBlock {
                    id: MirBlockId(0),
                    instructions: vec![instr(MirInstruction::Memory(MirMemoryIntrinsic::Copy {
                        dest: MirOperand::Local(MirLocalId(0)),
                        src: MirOperand::Local(MirLocalId(1)),
                        len: MirOperand::Const(MirConst::Integer {
                            ty: TypeId::USIZE,
                            value: 4,
                        }),
                    }))],
                    terminator: term(MirTerminator::Return(None)),
                }],
            }),
            is_extern: false,
            is_variadic: false,
            inline_hint: MirInlineHint::None,
            attributes: vec![],
        }],
        mono: MonoModuleMetadata::default(),
    };

    let error = verify_module(&module).expect_err("verifier should reject invalid memory refs");
    assert!(error.function.contains("bad_mem_fn"));
    assert!(error.message.contains("out of range"));
}

#[test]
fn mir_pass_pipeline_folds_degenerate_branch_targets() {
    let join = MirBlockId(1);
    let mut module = MirModule {
        name: "demo".to_string(),
        structs: vec![],
        globals: vec![],
        functions: vec![MirFunction {
            id: MonoId(16),
            name: "degenerate_branch".to_string(),
            span: Span::default(),
            linkage: MirLinkage::External,
            params: vec![],
            ret_ty: TypeId::VOID,
            body: Some(MirBody {
                entry: MirBlockId(0),
                locals: vec![],
                blocks: vec![
                    MirBlock {
                        id: MirBlockId(0),
                        instructions: vec![],
                        terminator: term(MirTerminator::Branch {
                            cond: MirRvalue::Use(MirOperand::Const(MirConst::Bool {
                                value: false,
                            })),
                            then_block: join,
                            else_block: join,
                        }),
                    },
                    MirBlock {
                        id: join,
                        instructions: vec![],
                        terminator: term(MirTerminator::Return(None)),
                    },
                ],
            }),
            is_extern: false,
            is_variadic: false,
            inline_hint: MirInlineHint::None,
            attributes: vec![],
        }],
        mono: MonoModuleMetadata::default(),
    };

    let pipeline = run_default_pass_pipeline(&mut module);
    verify_module(&module).expect("pass pipeline should preserve valid MIR");

    let thread_pass = &pipeline.passes[1];
    let branch_pass = &pipeline.passes[2];
    let cfg_pass = &pipeline.passes[3];
    assert_eq!(thread_pass.name, "cfg_thread_jumps");
    assert!(!thread_pass.changed());
    assert_eq!(branch_pass.name, "branch_folding");
    assert_eq!(branch_pass.terminator_rewrites, 1);
    assert_eq!(cfg_pass.name, "cfg_prune_unreachable_blocks");
    assert_eq!(cfg_pass.removed_blocks, 0);
    let body = module.functions[0].body.as_ref().unwrap();
    assert!(
        matches!(body.blocks[0].terminator.kind, MirTerminator::Goto(target) if target == join)
    );
}

#[test]
fn mir_pass_pipeline_threads_trivial_goto_chains_before_pruning() {
    let mut module = MirModule {
        name: "demo".to_string(),
        structs: vec![],
        globals: vec![],
        functions: vec![MirFunction {
            id: MonoId(17),
            name: "goto_chain".to_string(),
            span: Span::default(),
            linkage: MirLinkage::External,
            params: vec![],
            ret_ty: TypeId::VOID,
            body: Some(MirBody {
                entry: MirBlockId(0),
                locals: vec![],
                blocks: vec![
                    MirBlock {
                        id: MirBlockId(0),
                        instructions: vec![],
                        terminator: term(MirTerminator::Goto(MirBlockId(1))),
                    },
                    MirBlock {
                        id: MirBlockId(1),
                        instructions: vec![],
                        terminator: term(MirTerminator::Goto(MirBlockId(2))),
                    },
                    MirBlock {
                        id: MirBlockId(2),
                        instructions: vec![],
                        terminator: term(MirTerminator::Return(None)),
                    },
                ],
            }),
            is_extern: false,
            is_variadic: false,
            inline_hint: MirInlineHint::None,
            attributes: vec![],
        }],
        mono: MonoModuleMetadata::default(),
    };

    let pipeline = run_default_pass_pipeline(&mut module);
    verify_module(&module).expect("pass pipeline should preserve valid MIR");

    let thread_pass = &pipeline.passes[1];
    let branch_pass = &pipeline.passes[2];
    let cfg_pass = &pipeline.passes[3];
    assert_eq!(thread_pass.name, "cfg_thread_jumps");
    assert_eq!(thread_pass.terminator_rewrites, 1);
    assert_eq!(branch_pass.name, "branch_folding");
    assert!(!branch_pass.changed());
    assert_eq!(cfg_pass.name, "cfg_prune_unreachable_blocks");
    assert_eq!(cfg_pass.removed_blocks, 1);

    let body = module.functions[0].body.as_ref().unwrap();
    assert_eq!(body.blocks.len(), 2);
    assert!(matches!(
        body.blocks[0].terminator.kind,
        MirTerminator::Goto(target) if target == MirBlockId(1)
    ));
    assert!(matches!(
        body.blocks[1].terminator.kind,
        MirTerminator::Return(None)
    ));
}

#[test]
fn mir_summary_tracks_calls_refs_and_body_roles() {
    let module = MirModule {
        name: "summary_demo".to_string(),
        structs: vec![],
        globals: vec![MirGlobal {
            id: MonoId(10),
            name: "GLOBAL".to_string(),
            span: Span::default(),
            linkage: MirLinkage::Internal,
            ty: TypeId::USIZE,
            is_mut: false,
            init: Some(MirStaticInit::Const(MirConst::FuncRef {
                ty: TypeId::USIZE,
                id: MonoId(2),
            })),
            is_extern: false,
            attributes: vec![],
        }],
        functions: vec![
            MirFunction {
                id: MonoId(1),
                name: "root".to_string(),
                span: Span::default(),
                linkage: MirLinkage::External,
                params: vec![],
                ret_ty: TypeId::VOID,
                body: Some(MirBody {
                    entry: MirBlockId(0),
                    locals: vec![],
                    blocks: vec![MirBlock {
                        id: MirBlockId(0),
                        instructions: vec![
                            instr(MirInstruction::Eval(MirRvalue::Use(MirOperand::Const(
                                MirConst::GlobalRef {
                                    ty: TypeId::USIZE,
                                    id: MonoId(10),
                                },
                            )))),
                            instr(MirInstruction::Eval(MirRvalue::Use(MirOperand::Const(
                                MirConst::FuncRef {
                                    ty: TypeId::USIZE,
                                    id: MonoId(3),
                                },
                            )))),
                            instr(MirInstruction::Eval(MirRvalue::Call {
                                callee: MirCallTarget::Direct(MonoId(2)),
                                args: vec![MirOperand::Const(MirConst::GlobalRef {
                                    ty: TypeId::USIZE,
                                    id: MonoId(10),
                                })],
                            })),
                            instr(MirInstruction::Eval(MirRvalue::Call {
                                callee: MirCallTarget::Direct(MonoId(2)),
                                args: vec![],
                            })),
                            instr(MirInstruction::Eval(MirRvalue::Call {
                                callee: MirCallTarget::Operand(MirOperand::Const(
                                    MirConst::FuncRef {
                                        ty: TypeId::USIZE,
                                        id: MonoId(4),
                                    },
                                )),
                                args: vec![],
                            })),
                        ],
                        terminator: term(MirTerminator::Return(None)),
                    }],
                }),
                is_extern: false,
                is_variadic: false,
                inline_hint: MirInlineHint::Inline,
                attributes: vec![],
            },
            MirFunction {
                id: MonoId(2),
                name: "helper".to_string(),
                span: Span::default(),
                linkage: MirLinkage::Internal,
                params: vec![],
                ret_ty: TypeId::VOID,
                body: Some(MirBody {
                    entry: MirBlockId(0),
                    locals: vec![],
                    blocks: vec![MirBlock {
                        id: MirBlockId(0),
                        instructions: vec![],
                        terminator: term(MirTerminator::Return(None)),
                    }],
                }),
                is_extern: false,
                is_variadic: false,
                inline_hint: MirInlineHint::NoInline,
                attributes: vec![],
            },
            MirFunction {
                id: MonoId(3),
                name: "decl".to_string(),
                span: Span::default(),
                linkage: MirLinkage::External,
                params: vec![],
                ret_ty: TypeId::VOID,
                body: None,
                is_extern: true,
                is_variadic: false,
                inline_hint: MirInlineHint::None,
                attributes: vec![],
            },
        ],
        mono: MonoModuleMetadata::default(),
    };

    let summary = module.summary_index();
    let root = summary.function(MonoId(1)).expect("missing root summary");
    assert_eq!(root.inline_hint, MirInlineHint::Inline);
    assert_eq!(root.body_role, MirItemBodyRole::ExportRoot);
    assert!(root.can_import_body);
    assert_eq!(root.direct_call_count, 2);
    assert_eq!(root.indirect_call_count, 1);
    assert_eq!(root.refs.direct_callee_ids, vec![MonoId(2)]);
    assert_eq!(
        root.refs.direct_callee_callsite_counts,
        vec![MirDirectCalleeCallsiteCount {
            callee_id: MonoId(2),
            callsite_count: 2,
        }]
    );
    assert_eq!(root.refs.direct_callsite_count(MonoId(2)), 2);
    assert_eq!(
        root.refs.function_ids,
        vec![MonoId(2), MonoId(3), MonoId(4)]
    );
    assert_eq!(root.refs.global_ids, vec![MonoId(10)]);

    let helper = summary.function(MonoId(2)).expect("missing helper summary");
    assert_eq!(helper.inline_hint, MirInlineHint::NoInline);
    assert_eq!(helper.body_role, MirItemBodyRole::InternalBody);

    let decl = summary
        .function(MonoId(3))
        .expect("missing declaration summary");
    assert_eq!(decl.body_role, MirItemBodyRole::DeclarationOnly);
    assert!(!decl.can_import_body);

    let global = summary.global(MonoId(10)).expect("missing global summary");
    assert_eq!(global.body_role, MirItemBodyRole::InternalBody);
    assert_eq!(global.refs.function_ids, vec![MonoId(2)]);

    assert_eq!(
        summary
            .callers_by_callee
            .get(&MonoId(2))
            .cloned()
            .unwrap_or_default(),
        vec![MonoId(1)]
    );
    assert_eq!(
        summary
            .direct_callsites_by_callee
            .get(&MonoId(2))
            .copied()
            .unwrap_or_default(),
        2
    );
}
