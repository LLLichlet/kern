//! MAST workload estimates for codegen planning.
//!
//! These rough recursive counts let the fallback planner balance codegen units
//! when MIR summary information is unavailable.

use kernc_mast::{MastBlock, MastExpr, MastExprKind, MastFunction, MastGlobal, MastStmt};

pub(super) fn workload_for_function(function: &MastFunction) -> usize {
    function
        .body
        .as_ref()
        .map(block_workload)
        .unwrap_or(1)
        .max(1)
}

pub(super) fn workload_for_global(global: &MastGlobal) -> usize {
    global.init.as_ref().map(expr_workload).unwrap_or(1).max(1)
}

fn block_workload(block: &MastBlock) -> usize {
    let mut weight = 1 + block.stmts.len() + block.defers.len();
    for stmt in &block.stmts {
        weight += match stmt {
            MastStmt::Let { init, .. } => expr_workload(init),
            MastStmt::Expr(expr) => expr_workload(expr),
        };
    }
    if let Some(result) = &block.result {
        weight += expr_workload(result);
    }
    for defer in &block.defers {
        weight += expr_workload(defer);
    }
    weight
}

fn expr_workload(expr: &MastExpr) -> usize {
    let mut weight = 1;
    match &expr.kind {
        MastExprKind::AddressOf(inner)
        | MastExprKind::Deref(inner)
        | MastExprKind::Discard(inner)
        | MastExprKind::ExtractFatPtrData(inner)
        | MastExprKind::ExtractFatPtrMeta(inner)
        | MastExprKind::ExtractElementPtr(inner)
        | MastExprKind::BitIntrinsic { operand: inner, .. }
        | MastExprKind::SimdUnaryIntrinsic { operand: inner, .. }
        | MastExprKind::SimdReduce { operand: inner, .. }
        | MastExprKind::SimdAny { operand: inner, .. }
        | MastExprKind::SimdAll { operand: inner, .. }
        | MastExprKind::SimdBitmask { operand: inner, .. }
        | MastExprKind::SimdSplat { value: inner, .. }
        | MastExprKind::SimdCast { value: inner, .. }
        | MastExprKind::SimdBitcast { value: inner, .. }
        | MastExprKind::Cast { operand: inner, .. }
        | MastExprKind::Unary { operand: inner, .. } => weight += expr_workload(inner),
        MastExprKind::StructInit { fields, .. } | MastExprKind::ArrayInit(fields) => {
            for field in fields {
                weight += expr_workload(field);
            }
        }
        MastExprKind::UnionInit { value, .. } | MastExprKind::DataInit { payload: value, .. } => {
            weight += expr_workload(value);
        }
        MastExprKind::FieldAccess { lhs, .. } => weight += expr_workload(lhs),
        MastExprKind::IndexAccess { lhs, index }
        | MastExprKind::Binary {
            lhs, rhs: index, ..
        }
        | MastExprKind::SimdBinaryIntrinsic {
            lhs, rhs: index, ..
        }
        | MastExprKind::Assign {
            lhs, rhs: index, ..
        } => {
            weight += expr_workload(lhs);
            weight += expr_workload(index);
        }
        MastExprKind::Call { callee, args } => {
            weight += 2 + expr_workload(callee);
            for arg in args {
                weight += expr_workload(arg);
            }
        }
        MastExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            weight += 3 + expr_workload(cond) + block_workload(then_branch);
            if let Some(else_branch) = else_branch {
                weight += block_workload(else_branch);
            }
        }
        MastExprKind::Loop { body, latch } => {
            weight += 3 + block_workload(body);
            if let Some(latch) = latch {
                weight += block_workload(latch);
            }
        }
        MastExprKind::Switch {
            target,
            cases,
            default_case,
        } => {
            weight += 4 + expr_workload(target);
            for case in cases {
                weight += block_workload(&case.body);
            }
            if let Some(default_case) = default_case {
                weight += block_workload(default_case);
            }
        }
        MastExprKind::Return(value) => {
            if let Some(value) = value {
                weight += expr_workload(value);
            }
        }
        MastExprKind::AtomicLoad { ptr, .. } => weight += expr_workload(ptr),
        MastExprKind::AtomicStore { ptr, value, .. }
        | MastExprKind::AtomicRmw { ptr, value, .. } => {
            weight += expr_workload(ptr) + expr_workload(value);
        }
        MastExprKind::AtomicCas {
            ptr,
            expected,
            desired,
            ..
        } => {
            weight += expr_workload(ptr) + expr_workload(expected) + expr_workload(desired);
        }
        MastExprKind::Memcpy { dest, src, len } | MastExprKind::Memmove { dest, src, len } => {
            weight += expr_workload(dest) + expr_workload(src) + expr_workload(len);
        }
        MastExprKind::Memset { dest, val, len } => {
            weight += expr_workload(dest) + expr_workload(val) + expr_workload(len);
        }
        MastExprKind::ConstructFatPointer { data_ptr, meta } => {
            weight += expr_workload(data_ptr) + expr_workload(meta);
        }
        MastExprKind::SimdSelect {
            mask,
            on_true,
            on_false,
        } => {
            weight += expr_workload(mask) + expr_workload(on_true) + expr_workload(on_false);
        }
        MastExprKind::SimdShuffle { lhs, rhs, .. } => {
            weight += expr_workload(lhs) + expr_workload(rhs);
        }
        MastExprKind::SimdInsertHalf { base, half, .. } => {
            weight += expr_workload(base) + expr_workload(half);
        }
        MastExprKind::SimdLoad { ptr, .. } => weight += expr_workload(ptr),
        MastExprKind::SimdStore { ptr, value, .. } => {
            weight += expr_workload(ptr) + expr_workload(value);
        }
        MastExprKind::SimdMaskedLoad {
            ptr, mask, or_else, ..
        } => {
            weight += expr_workload(ptr) + expr_workload(mask) + expr_workload(or_else);
        }
        MastExprKind::SimdMaskedStore {
            ptr, mask, value, ..
        } => {
            weight += expr_workload(ptr) + expr_workload(mask) + expr_workload(value);
        }
        MastExprKind::SimdGather { ptr, indices } => {
            weight += expr_workload(ptr) + expr_workload(indices);
        }
        MastExprKind::SimdScatter {
            ptr,
            indices,
            value,
        } => {
            weight += expr_workload(ptr) + expr_workload(indices) + expr_workload(value);
        }
        MastExprKind::SimdMaskedGather {
            ptr,
            indices,
            mask,
            or_else,
        } => {
            weight += expr_workload(ptr)
                + expr_workload(indices)
                + expr_workload(mask)
                + expr_workload(or_else);
        }
        MastExprKind::SimdMaskedScatter {
            ptr,
            indices,
            mask,
            value,
        } => {
            weight += expr_workload(ptr)
                + expr_workload(indices)
                + expr_workload(mask)
                + expr_workload(value);
        }
        MastExprKind::Block(block) => weight += block_workload(block),
        MastExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            weight += expr_workload(lhs);
            if let Some(start) = start {
                weight += expr_workload(start);
            }
            if let Some(end) = end {
                weight += expr_workload(end);
            }
        }
        MastExprKind::Asm(asm) => {
            weight += 2;
            for arg in &asm.input_args {
                weight += expr_workload(arg);
            }
        }
        MastExprKind::Fence { .. }
        | MastExprKind::Undef
        | MastExprKind::Unreachable
        | MastExprKind::Trap
        | MastExprKind::Breakpoint
        | MastExprKind::Break
        | MastExprKind::Continue
        | MastExprKind::Integer(_)
        | MastExprKind::Float(_)
        | MastExprKind::Bool(_)
        | MastExprKind::StringLiteral(_)
        | MastExprKind::Var(_)
        | MastExprKind::GlobalRef(_)
        | MastExprKind::FuncRef(_) => {}
    }
    weight
}
