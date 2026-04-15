mod builder_data;
mod builder_flow;
mod builder_scalar;
mod builder_simd;
mod pipeline;

use crate::{build_from_mast, build_from_mast_unoptimized};
use kernc_ast::{AssignmentOperator, BinaryOperator, UnaryOperator};
use kernc_mast::{
    MastAsmBlock, MastBlock, MastCastKind, MastExpr, MastExprKind, MastFunction, MastGlobal,
    MastInlineHint, MastLinkage, MastModule, MastParam, MastStmt, MastSwitchCase,
};
use kernc_mir::{
    MirAggregateKind, MirBitIntrinsicKind, MirCallTarget, MirCastKind, MirConst, MirInlineHint,
    MirInstruction, MirLocalKind, MirMemoryIntrinsic, MirOperand, MirPlace, MirProjectionKind,
    MirRvalue, MirSimdBinaryIntrinsicKind, MirSliceBase, MirStaticInit, MirTerminator,
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
