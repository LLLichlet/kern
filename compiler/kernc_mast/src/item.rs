use crate::{MastBlock, MastExpr};
use kernc_ast::MetaItem;
use kernc_mono::{MonoId, MonoModuleMetadata};
use kernc_sema::ty::TypeId;
use kernc_utils::{Span, SymbolId};

/// Final flattened compilation unit produced by lowering.
/// At this stage there are no nested modules, impl blocks, or unresolved generics.
#[derive(Debug, Clone)]
pub struct MastModule {
    pub name: String,
    pub structs: Vec<MastStruct>,
    /// All statics, including lowered local statics.
    pub globals: Vec<MastGlobal>,
    pub functions: Vec<MastFunction>,
    pub mono: MonoModuleMetadata,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MastWorkloadStats {
    pub structs: usize,
    pub globals: usize,
    pub globals_with_init: usize,
    pub functions: usize,
    pub function_bodies: usize,
    pub extern_functions: usize,
    pub blocks: usize,
    pub statements: usize,
    pub let_statements: usize,
    pub expr_statements: usize,
    pub defers: usize,
    pub expressions: usize,
    pub calls: usize,
    pub branches: usize,
    pub loops: usize,
    pub switches: usize,
    pub returns: usize,
    pub assignments: usize,
}

impl MastModule {
    pub fn workload_stats(&self) -> MastWorkloadStats {
        let mut stats = MastWorkloadStats {
            structs: self.structs.len(),
            globals: self.globals.len(),
            globals_with_init: self
                .globals
                .iter()
                .filter(|global| global.init.is_some())
                .count(),
            functions: self.functions.len(),
            function_bodies: self
                .functions
                .iter()
                .filter(|function| function.body.is_some())
                .count(),
            extern_functions: self
                .functions
                .iter()
                .filter(|function| function.is_extern)
                .count(),
            ..MastWorkloadStats::default()
        };

        for global in &self.globals {
            if let Some(init) = &global.init {
                visit_expr(init, &mut stats);
            }
        }

        for function in &self.functions {
            if let Some(body) = &function.body {
                visit_block(body, &mut stats);
            }
        }

        stats
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MastLinkage {
    External,
    LinkOnceOdr,
    Internal,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MastInlineHint {
    #[default]
    None,
    Inline,
    NoInline,
}

#[derive(Debug, Clone)]
pub struct MastStruct {
    pub id: MonoId,
    /// Flattened fully qualified name such as `std_collections_ArrayList_i32`.
    pub name: String,
    pub fields: Vec<MastField>,
    /// Preserves source layout for ABI-facing structs.
    pub is_extern: bool,
    pub is_union: bool,
    pub largest_field_idx: usize,
    pub union_size: usize,
    pub union_align: usize,
    pub attributes: Vec<MetaItem>,
}

#[derive(Debug, Clone)]
pub struct MastField {
    pub name: SymbolId,
    /// Always fully concrete and never contains `Param`.
    pub ty: TypeId,
}

#[derive(Debug, Clone)]
pub struct MastGlobal {
    pub id: MonoId,
    /// Flattened global symbol name.
    pub name: String,
    pub span: Span,
    pub linkage: MastLinkage,
    pub ty: TypeId,
    /// Mirrors `static mut`.
    pub is_mut: bool,
    /// `None` for extern declarations. Initializers must be constant expressions.
    pub init: Option<MastExpr>,
    pub is_extern: bool,
    pub attributes: Vec<MetaItem>,
}

#[derive(Debug, Clone)]
pub struct MastFunction {
    pub id: MonoId,
    /// Flattened symbol name, for example `Point_i32_move_by`.
    pub name: String,
    pub span: Span,
    pub linkage: MastLinkage,
    pub params: Vec<MastParam>,
    pub ret_ty: TypeId,
    /// `None` for extern declarations.
    pub body: Option<MastBlock>,
    pub is_extern: bool,
    pub is_variadic: bool,
    pub inline_hint: MastInlineHint,
    pub attributes: Vec<MetaItem>,
}

#[derive(Debug, Clone)]
pub struct MastParam {
    pub name: SymbolId,
    pub ty: TypeId,
    pub is_mut: bool,
}

fn visit_block(block: &MastBlock, stats: &mut MastWorkloadStats) {
    stats.blocks += 1;
    stats.statements += block.stmts.len();
    stats.defers += block.defers.len();

    for stmt in &block.stmts {
        match stmt {
            crate::MastStmt::Let { init, .. } => {
                stats.let_statements += 1;
                visit_expr(init, stats);
            }
            crate::MastStmt::Expr(expr) => {
                stats.expr_statements += 1;
                visit_expr(expr, stats);
            }
        }
    }

    if let Some(result) = &block.result {
        visit_expr(result, stats);
    }

    for defer in &block.defers {
        visit_expr(defer, stats);
    }
}

fn visit_expr(expr: &MastExpr, stats: &mut MastWorkloadStats) {
    stats.expressions += 1;

    match &expr.kind {
        crate::MastExprKind::Undef
        | crate::MastExprKind::Unreachable
        | crate::MastExprKind::Trap
        | crate::MastExprKind::Breakpoint
        | crate::MastExprKind::Integer(_)
        | crate::MastExprKind::Float(_)
        | crate::MastExprKind::Bool(_)
        | crate::MastExprKind::StringLiteral(_)
        | crate::MastExprKind::Var(_)
        | crate::MastExprKind::GlobalRef(_)
        | crate::MastExprKind::FuncRef(_)
        | crate::MastExprKind::Break
        | crate::MastExprKind::Continue
        | crate::MastExprKind::Fence { .. } => {}

        crate::MastExprKind::AddressOf(inner)
        | crate::MastExprKind::Deref(inner)
        | crate::MastExprKind::Discard(inner)
        | crate::MastExprKind::ExtractFatPtrData(inner)
        | crate::MastExprKind::ExtractFatPtrMeta(inner)
        | crate::MastExprKind::Unary { operand: inner, .. }
        | crate::MastExprKind::Cast { operand: inner, .. }
        | crate::MastExprKind::BitIntrinsic { operand: inner, .. }
        | crate::MastExprKind::SimdUnaryIntrinsic { operand: inner, .. }
        | crate::MastExprKind::SimdReduce { operand: inner, .. }
        | crate::MastExprKind::SimdAny { operand: inner, .. }
        | crate::MastExprKind::SimdAll { operand: inner, .. }
        | crate::MastExprKind::SimdBitmask { operand: inner, .. }
        | crate::MastExprKind::SimdSplat { value: inner, .. }
        | crate::MastExprKind::SimdCast { value: inner, .. }
        | crate::MastExprKind::SimdBitcast { value: inner, .. } => visit_expr(inner, stats),

        crate::MastExprKind::StructInit { fields, .. } | crate::MastExprKind::ArrayInit(fields) => {
            for field in fields {
                visit_expr(field, stats);
            }
        }

        crate::MastExprKind::UnionInit { value, .. } => visit_expr(value, stats),

        crate::MastExprKind::FieldAccess { lhs, .. } => visit_expr(lhs, stats),

        crate::MastExprKind::IndexAccess { lhs, index } => {
            visit_expr(lhs, stats);
            visit_expr(index, stats);
        }

        crate::MastExprKind::Call { callee, args } => {
            stats.calls += 1;
            visit_expr(callee, stats);
            for arg in args {
                visit_expr(arg, stats);
            }
        }

        crate::MastExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            stats.branches += 1;
            visit_expr(cond, stats);
            visit_block(then_branch, stats);
            if let Some(else_branch) = else_branch {
                visit_block(else_branch, stats);
            }
        }

        crate::MastExprKind::Loop { body, latch } => {
            stats.loops += 1;
            visit_block(body, stats);
            if let Some(latch) = latch {
                visit_block(latch, stats);
            }
        }

        crate::MastExprKind::Switch {
            target,
            cases,
            default_case,
        } => {
            stats.switches += 1;
            visit_expr(target, stats);
            for case in cases {
                visit_block(&case.body, stats);
            }
            if let Some(default_case) = default_case {
                visit_block(default_case, stats);
            }
        }

        crate::MastExprKind::Return(value) => {
            stats.returns += 1;
            if let Some(value) = value {
                visit_expr(value, stats);
            }
        }

        crate::MastExprKind::Binary { lhs, rhs, .. }
        | crate::MastExprKind::Assign { lhs, rhs, .. }
        | crate::MastExprKind::SimdBinaryIntrinsic { lhs, rhs, .. } => {
            if matches!(&expr.kind, crate::MastExprKind::Assign { .. }) {
                stats.assignments += 1;
            }
            visit_expr(lhs, stats);
            visit_expr(rhs, stats);
        }

        crate::MastExprKind::ConstructFatPointer { data_ptr, meta } => {
            visit_expr(data_ptr, stats);
            visit_expr(meta, stats);
        }

        crate::MastExprKind::SimdSelect {
            mask,
            on_true,
            on_false,
        } => {
            visit_expr(mask, stats);
            visit_expr(on_true, stats);
            visit_expr(on_false, stats);
        }

        crate::MastExprKind::SimdShuffle { lhs, rhs, .. } => {
            visit_expr(lhs, stats);
            visit_expr(rhs, stats);
        }

        crate::MastExprKind::SimdInsertHalf { base, half, .. } => {
            visit_expr(base, stats);
            visit_expr(half, stats);
        }

        crate::MastExprKind::SimdLoad { ptr, .. } => visit_expr(ptr, stats),

        crate::MastExprKind::SimdStore { ptr, value, .. } => {
            visit_expr(ptr, stats);
            visit_expr(value, stats);
        }

        crate::MastExprKind::SimdMaskedLoad {
            ptr, mask, or_else, ..
        } => {
            visit_expr(ptr, stats);
            visit_expr(mask, stats);
            visit_expr(or_else, stats);
        }

        crate::MastExprKind::SimdMaskedStore {
            ptr, mask, value, ..
        } => {
            visit_expr(ptr, stats);
            visit_expr(mask, stats);
            visit_expr(value, stats);
        }

        crate::MastExprKind::SimdGather { ptr, indices } => {
            visit_expr(ptr, stats);
            visit_expr(indices, stats);
        }

        crate::MastExprKind::SimdScatter {
            ptr,
            indices,
            value,
        } => {
            visit_expr(ptr, stats);
            visit_expr(indices, stats);
            visit_expr(value, stats);
        }

        crate::MastExprKind::SimdMaskedGather {
            ptr,
            indices,
            mask,
            or_else,
        } => {
            visit_expr(ptr, stats);
            visit_expr(indices, stats);
            visit_expr(mask, stats);
            visit_expr(or_else, stats);
        }

        crate::MastExprKind::SimdMaskedScatter {
            ptr,
            indices,
            mask,
            value,
        } => {
            visit_expr(ptr, stats);
            visit_expr(indices, stats);
            visit_expr(mask, stats);
            visit_expr(value, stats);
        }

        crate::MastExprKind::Block(block) => visit_block(block, stats),

        crate::MastExprKind::DataInit { payload, .. } => visit_expr(payload, stats),

        crate::MastExprKind::Asm(asm) => {
            for input in &asm.input_args {
                visit_expr(input, stats);
            }
            for output in &asm.output_ptrs {
                visit_expr(output, stats);
            }
        }

        crate::MastExprKind::AtomicLoad { ptr, .. } => visit_expr(ptr, stats),

        crate::MastExprKind::AtomicStore { ptr, value, .. } => {
            visit_expr(ptr, stats);
            visit_expr(value, stats);
        }

        crate::MastExprKind::AtomicCas {
            ptr,
            expected,
            desired,
            ..
        } => {
            visit_expr(ptr, stats);
            visit_expr(expected, stats);
            visit_expr(desired, stats);
        }

        crate::MastExprKind::AtomicRmw { ptr, value, .. } => {
            visit_expr(ptr, stats);
            visit_expr(value, stats);
        }

        crate::MastExprKind::Memcpy { dest, src, len }
        | crate::MastExprKind::Memmove { dest, src, len } => {
            visit_expr(dest, stats);
            visit_expr(src, stats);
            visit_expr(len, stats);
        }

        crate::MastExprKind::Memset { dest, val, len } => {
            visit_expr(dest, stats);
            visit_expr(val, stats);
            visit_expr(len, stats);
        }

        crate::MastExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            visit_expr(lhs, stats);
            if let Some(start) = start {
                visit_expr(start, stats);
            }
            if let Some(end) = end {
                visit_expr(end, stats);
            }
        }
    }
}
