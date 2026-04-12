use super::{
    AnalysisDeadStoreKind, AnalysisFlowBindingKind, AnalysisFlowCfgEdgeKind,
    AnalysisFlowCfgNodeKind, AnalysisFlowDefinitionKind, AnalysisFlowDefinitionRef,
    AnalysisFlowOwnerKind, AnalysisFlowRegionKind, AnalysisFlowResolvedUseKind,
    AnalysisUnusedBindingKind, AnalysisUnusedItemKind, CompilerDriver, SourceOverrides,
};
use kernc_mast::{MastBlock, MastExpr, MastExprKind, MastStmt};
use kernc_utils::Session;
use kernc_utils::config::{CompileOptions, DriverMode, LtoMode};
use std::fs;
use std::process::Command;

mod cache;
mod completion;
mod diagnostics;
mod flow;
mod lowering;

fn count_assignments_in_block(block: &MastBlock) -> usize {
    let stmt_count: usize = block.stmts.iter().map(count_assignments_in_stmt).sum();
    let result_count = block
        .result
        .as_deref()
        .map(count_assignments_in_expr)
        .unwrap_or(0);
    let defer_count: usize = block.defers.iter().map(count_assignments_in_expr).sum();
    stmt_count + result_count + defer_count
}

fn count_assignments_in_stmt(stmt: &MastStmt) -> usize {
    match stmt {
        MastStmt::Let { init, .. } => count_assignments_in_expr(init),
        MastStmt::Expr(expr) => count_assignments_in_expr(expr),
    }
}

fn count_assignments_in_expr(expr: &MastExpr) -> usize {
    let self_count = usize::from(matches!(expr.kind, MastExprKind::Assign { .. }));
    let child_count = match &expr.kind {
        MastExprKind::AddressOf(operand)
        | MastExprKind::Deref(operand)
        | MastExprKind::ExtractFatPtrData(operand)
        | MastExprKind::ExtractFatPtrMeta(operand)
        | MastExprKind::BitIntrinsic { operand, .. }
        | MastExprKind::Cast { operand, .. }
        | MastExprKind::Unary { operand, .. } => count_assignments_in_expr(operand),
        MastExprKind::StructInit { fields, .. } | MastExprKind::ArrayInit(fields) => {
            fields.iter().map(count_assignments_in_expr).sum()
        }
        MastExprKind::UnionInit { value, .. } | MastExprKind::DataInit { payload: value, .. } => {
            count_assignments_in_expr(value)
        }
        MastExprKind::FieldAccess { lhs, .. } => count_assignments_in_expr(lhs),
        MastExprKind::IndexAccess { lhs, index }
        | MastExprKind::Binary {
            lhs, rhs: index, ..
        }
        | MastExprKind::Assign {
            lhs, rhs: index, ..
        } => count_assignments_in_expr(lhs) + count_assignments_in_expr(index),
        MastExprKind::Call { callee, args } => {
            count_assignments_in_expr(callee)
                + args.iter().map(count_assignments_in_expr).sum::<usize>()
        }
        MastExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            count_assignments_in_expr(cond)
                + count_assignments_in_block(then_branch)
                + else_branch
                    .as_ref()
                    .map(count_assignments_in_block)
                    .unwrap_or(0)
        }
        MastExprKind::Loop { body, latch } => {
            count_assignments_in_block(body)
                + latch.as_ref().map(count_assignments_in_block).unwrap_or(0)
        }
        MastExprKind::Switch {
            target,
            cases,
            default_case,
        } => {
            count_assignments_in_expr(target)
                + cases
                    .iter()
                    .map(|case| count_assignments_in_block(&case.body))
                    .sum::<usize>()
                + default_case
                    .as_ref()
                    .map(count_assignments_in_block)
                    .unwrap_or(0)
        }
        MastExprKind::Return(value) => value.as_deref().map(count_assignments_in_expr).unwrap_or(0),
        MastExprKind::AtomicLoad { ptr, .. } => count_assignments_in_expr(ptr),
        MastExprKind::AtomicStore { ptr, value, .. }
        | MastExprKind::AtomicRmw { ptr, value, .. } => {
            count_assignments_in_expr(ptr) + count_assignments_in_expr(value)
        }
        MastExprKind::AtomicCas {
            ptr,
            expected,
            desired,
            ..
        } => {
            count_assignments_in_expr(ptr)
                + count_assignments_in_expr(expected)
                + count_assignments_in_expr(desired)
        }
        MastExprKind::Fence { .. } => 0,
        MastExprKind::Memcpy { dest, src, len } => {
            count_assignments_in_expr(dest)
                + count_assignments_in_expr(src)
                + count_assignments_in_expr(len)
        }
        MastExprKind::Memmove { dest, src, len } => {
            count_assignments_in_expr(dest)
                + count_assignments_in_expr(src)
                + count_assignments_in_expr(len)
        }
        MastExprKind::Memset { dest, val, len } => {
            count_assignments_in_expr(dest)
                + count_assignments_in_expr(val)
                + count_assignments_in_expr(len)
        }
        MastExprKind::ConstructFatPointer { data_ptr, meta } => {
            count_assignments_in_expr(data_ptr) + count_assignments_in_expr(meta)
        }
        MastExprKind::Block(block) => count_assignments_in_block(block),
        MastExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            count_assignments_in_expr(lhs)
                + start.as_deref().map(count_assignments_in_expr).unwrap_or(0)
                + end.as_deref().map(count_assignments_in_expr).unwrap_or(0)
        }
        _ => 0,
    };

    self_count + child_count
}
