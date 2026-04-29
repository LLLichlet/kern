use super::*;
use kernc_mast::{MastBlock, MastExpr, MastExprKind, MastStmt};

pub(super) fn build_item_refs(module: &MastModule) -> HashMap<ItemKey, ItemRefs> {
    let mut refs = HashMap::new();

    for function in &module.functions {
        if function.body.is_some() {
            refs.insert(ItemKey::Function(function.id), refs_for_function(function));
        }
    }

    for global in &module.globals {
        if global.init.is_some() {
            refs.insert(ItemKey::Global(global.id), refs_for_global(global));
        }
    }

    refs
}

fn refs_for_function(function: &MastFunction) -> ItemRefs {
    let mut refs = ItemRefs::default();
    if let Some(body) = &function.body {
        collect_block_refs(body, &mut refs);
    }
    refs
}

fn refs_for_global(global: &MastGlobal) -> ItemRefs {
    let mut refs = ItemRefs::default();
    if let Some(init) = &global.init {
        collect_expr_refs(init, &mut refs);
    }
    refs
}

fn collect_block_refs(block: &MastBlock, refs: &mut ItemRefs) {
    for stmt in &block.stmts {
        match stmt {
            MastStmt::Let { init, .. } => collect_expr_refs(init, refs),
            MastStmt::Expr(expr) => collect_expr_refs(expr, refs),
        }
    }
    if let Some(result) = &block.result {
        collect_expr_refs(result, refs);
    }
    for defer in &block.defers {
        collect_expr_refs(defer, refs);
    }
}

fn collect_expr_refs(expr: &MastExpr, refs: &mut ItemRefs) {
    match &expr.kind {
        MastExprKind::GlobalRef(id) => {
            refs.globals.insert(*id);
        }
        MastExprKind::FuncRef(id) => {
            refs.functions.insert(*id);
        }
        MastExprKind::AddressOf(inner)
        | MastExprKind::Deref(inner)
        | MastExprKind::Discard(inner)
        | MastExprKind::ExtractFatPtrData(inner)
        | MastExprKind::ExtractFatPtrMeta(inner)
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
        | MastExprKind::Unary { operand: inner, .. } => collect_expr_refs(inner, refs),
        MastExprKind::StructInit { fields, .. } | MastExprKind::ArrayInit(fields) => {
            for field in fields {
                collect_expr_refs(field, refs);
            }
        }
        MastExprKind::UnionInit { value, .. } | MastExprKind::DataInit { payload: value, .. } => {
            collect_expr_refs(value, refs)
        }
        MastExprKind::FieldAccess { lhs, .. } => collect_expr_refs(lhs, refs),
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
            collect_expr_refs(lhs, refs);
            collect_expr_refs(index, refs);
        }
        MastExprKind::Call { callee, args } => {
            collect_expr_refs(callee, refs);
            for arg in args {
                collect_expr_refs(arg, refs);
            }
        }
        MastExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_refs(cond, refs);
            collect_block_refs(then_branch, refs);
            if let Some(else_branch) = else_branch {
                collect_block_refs(else_branch, refs);
            }
        }
        MastExprKind::Loop { body, latch } => {
            collect_block_refs(body, refs);
            if let Some(latch) = latch {
                collect_block_refs(latch, refs);
            }
        }
        MastExprKind::Switch {
            target,
            cases,
            default_case,
        } => {
            collect_expr_refs(target, refs);
            for case in cases {
                collect_block_refs(&case.body, refs);
            }
            if let Some(default_case) = default_case {
                collect_block_refs(default_case, refs);
            }
        }
        MastExprKind::Return(value) => {
            if let Some(value) = value {
                collect_expr_refs(value, refs);
            }
        }
        MastExprKind::AtomicLoad { ptr, .. } => collect_expr_refs(ptr, refs),
        MastExprKind::AtomicStore { ptr, value, .. }
        | MastExprKind::AtomicRmw { ptr, value, .. } => {
            collect_expr_refs(ptr, refs);
            collect_expr_refs(value, refs);
        }
        MastExprKind::AtomicCas {
            ptr,
            expected,
            desired,
            ..
        } => {
            collect_expr_refs(ptr, refs);
            collect_expr_refs(expected, refs);
            collect_expr_refs(desired, refs);
        }
        MastExprKind::Memcpy { dest, src, len } | MastExprKind::Memmove { dest, src, len } => {
            collect_expr_refs(dest, refs);
            collect_expr_refs(src, refs);
            collect_expr_refs(len, refs);
        }
        MastExprKind::Memset { dest, val, len } => {
            collect_expr_refs(dest, refs);
            collect_expr_refs(val, refs);
            collect_expr_refs(len, refs);
        }
        MastExprKind::ConstructFatPointer { data_ptr, meta } => {
            collect_expr_refs(data_ptr, refs);
            collect_expr_refs(meta, refs);
        }
        MastExprKind::SimdSelect {
            mask,
            on_true,
            on_false,
        } => {
            collect_expr_refs(mask, refs);
            collect_expr_refs(on_true, refs);
            collect_expr_refs(on_false, refs);
        }
        MastExprKind::SimdShuffle { lhs, rhs, .. } => {
            collect_expr_refs(lhs, refs);
            collect_expr_refs(rhs, refs);
        }
        MastExprKind::SimdInsertHalf { base, half, .. } => {
            collect_expr_refs(base, refs);
            collect_expr_refs(half, refs);
        }
        MastExprKind::SimdLoad { ptr, .. } => collect_expr_refs(ptr, refs),
        MastExprKind::SimdStore { ptr, value, .. } => {
            collect_expr_refs(ptr, refs);
            collect_expr_refs(value, refs);
        }
        MastExprKind::SimdMaskedLoad {
            ptr, mask, or_else, ..
        } => {
            collect_expr_refs(ptr, refs);
            collect_expr_refs(mask, refs);
            collect_expr_refs(or_else, refs);
        }
        MastExprKind::SimdMaskedStore {
            ptr, mask, value, ..
        } => {
            collect_expr_refs(ptr, refs);
            collect_expr_refs(mask, refs);
            collect_expr_refs(value, refs);
        }
        MastExprKind::SimdGather { ptr, indices } => {
            collect_expr_refs(ptr, refs);
            collect_expr_refs(indices, refs);
        }
        MastExprKind::SimdScatter {
            ptr,
            indices,
            value,
        } => {
            collect_expr_refs(ptr, refs);
            collect_expr_refs(indices, refs);
            collect_expr_refs(value, refs);
        }
        MastExprKind::SimdMaskedGather {
            ptr,
            indices,
            mask,
            or_else,
        } => {
            collect_expr_refs(ptr, refs);
            collect_expr_refs(indices, refs);
            collect_expr_refs(mask, refs);
            collect_expr_refs(or_else, refs);
        }
        MastExprKind::SimdMaskedScatter {
            ptr,
            indices,
            mask,
            value,
        } => {
            collect_expr_refs(ptr, refs);
            collect_expr_refs(indices, refs);
            collect_expr_refs(mask, refs);
            collect_expr_refs(value, refs);
        }
        MastExprKind::Block(block) => collect_block_refs(block, refs),
        MastExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_expr_refs(lhs, refs);
            if let Some(start) = start {
                collect_expr_refs(start, refs);
            }
            if let Some(end) = end {
                collect_expr_refs(end, refs);
            }
        }
        MastExprKind::Asm(asm) => {
            for arg in &asm.input_args {
                collect_expr_refs(arg, refs);
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
        | MastExprKind::Var(_) => {}
    }
}
