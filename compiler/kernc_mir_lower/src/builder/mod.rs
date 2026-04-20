mod control;
mod expr;
mod static_init;

use crate::MirBuildReport;
use kernc_mast::{
    BitIntrinsicKind, MastBlock, MastCastKind, MastExpr, MastExprKind, MastField, MastFunction,
    MastInlineHint, MastLinkage, MastModule, MastParam, MastStmt, MastStruct,
    SimdBinaryIntrinsicKind, SimdReduceKind, SimdUnaryIntrinsicKind,
};
use kernc_mir::{
    MirAggregateKind, MirBitIntrinsicKind, MirBlock, MirBlockId, MirBody, MirCallTarget,
    MirCastKind, MirConst, MirField, MirFunction, MirGlobal, MirInlineAsm, MirInlineHint,
    MirInstruction, MirInstructionData, MirLinkage, MirLocal, MirLocalId, MirLocalKind,
    MirMemoryIntrinsic, MirModule, MirOperand, MirParam, MirPlace, MirProjectionKind, MirRvalue,
    MirSimdBinaryIntrinsicKind, MirSimdReduceKind, MirSimdUnaryIntrinsicKind, MirSliceBase,
    MirStaticInit, MirStruct, MirSwitchTarget, MirTerminator, MirTerminatorData,
};
use kernc_sema::ty::TypeId;
use kernc_utils::{Span, SymbolId};
use std::collections::HashMap;

type LowerResult<T> = Result<T, MirLowerError>;

#[derive(Debug, Clone)]
struct MirLowerError {
    span: Span,
    message: String,
}

impl MirLowerError {
    fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }
}

fn lower_linkage(linkage: MastLinkage) -> MirLinkage {
    match linkage {
        MastLinkage::External => MirLinkage::External,
        MastLinkage::LinkOnceOdr => MirLinkage::LinkOnceOdr,
        MastLinkage::Internal => MirLinkage::Internal,
    }
}

fn lower_inline_hint(hint: MastInlineHint) -> MirInlineHint {
    match hint {
        MastInlineHint::None => MirInlineHint::None,
        MastInlineHint::Inline => MirInlineHint::Inline,
        MastInlineHint::NoInline => MirInlineHint::NoInline,
    }
}

fn lower_field(field: MastField) -> MirField {
    MirField {
        name: field.name,
        ty: field.ty,
    }
}

fn lower_struct(item: MastStruct) -> MirStruct {
    MirStruct {
        id: item.id,
        name: item.name,
        fields: item.fields.into_iter().map(lower_field).collect(),
        is_extern: item.is_extern,
        is_union: item.is_union,
        largest_field_idx: item.largest_field_idx,
        union_size: item.union_size,
        union_align: item.union_align,
        attributes: item.attributes,
    }
}

fn lower_param(param: MastParam) -> MirParam {
    MirParam {
        name: param.name,
        ty: param.ty,
        is_mut: param.is_mut,
    }
}

pub(super) fn lower_cast_kind(kind: MastCastKind) -> MirCastKind {
    match kind {
        MastCastKind::Bitcast => MirCastKind::Bitcast,
        MastCastKind::PtrToInt => MirCastKind::PtrToInt,
        MastCastKind::IntToPtr => MirCastKind::IntToPtr,
        MastCastKind::SignExt => MirCastKind::SignExt,
        MastCastKind::ZeroExt => MirCastKind::ZeroExt,
        MastCastKind::Trunc => MirCastKind::Trunc,
        MastCastKind::SIntToFloat => MirCastKind::SIntToFloat,
        MastCastKind::UIntToFloat => MirCastKind::UIntToFloat,
        MastCastKind::FloatToSInt => MirCastKind::FloatToSInt,
        MastCastKind::FloatToUInt => MirCastKind::FloatToUInt,
        MastCastKind::FloatCast => MirCastKind::FloatCast,
        MastCastKind::ArrayToSlice => MirCastKind::ArrayToSlice,
    }
}

pub(super) fn lower_bit_intrinsic_kind(kind: BitIntrinsicKind) -> MirBitIntrinsicKind {
    match kind {
        BitIntrinsicKind::PopCount => MirBitIntrinsicKind::PopCount,
        BitIntrinsicKind::Clz => MirBitIntrinsicKind::Clz,
        BitIntrinsicKind::Ctz => MirBitIntrinsicKind::Ctz,
        BitIntrinsicKind::Bswap => MirBitIntrinsicKind::Bswap,
    }
}

pub(super) fn lower_simd_unary_intrinsic_kind(
    kind: SimdUnaryIntrinsicKind,
) -> MirSimdUnaryIntrinsicKind {
    match kind {
        SimdUnaryIntrinsicKind::Abs => MirSimdUnaryIntrinsicKind::Abs,
        SimdUnaryIntrinsicKind::Sqrt => MirSimdUnaryIntrinsicKind::Sqrt,
        SimdUnaryIntrinsicKind::Floor => MirSimdUnaryIntrinsicKind::Floor,
        SimdUnaryIntrinsicKind::Ceil => MirSimdUnaryIntrinsicKind::Ceil,
        SimdUnaryIntrinsicKind::Trunc => MirSimdUnaryIntrinsicKind::Trunc,
        SimdUnaryIntrinsicKind::Round => MirSimdUnaryIntrinsicKind::Round,
    }
}

pub(super) fn lower_simd_binary_intrinsic_kind(
    kind: SimdBinaryIntrinsicKind,
) -> MirSimdBinaryIntrinsicKind {
    match kind {
        SimdBinaryIntrinsicKind::Min => MirSimdBinaryIntrinsicKind::Min,
        SimdBinaryIntrinsicKind::Max => MirSimdBinaryIntrinsicKind::Max,
    }
}

pub(super) fn lower_simd_reduce_kind(kind: SimdReduceKind) -> MirSimdReduceKind {
    match kind {
        SimdReduceKind::Add => MirSimdReduceKind::Add,
        SimdReduceKind::Mul => MirSimdReduceKind::Mul,
        SimdReduceKind::And => MirSimdReduceKind::And,
        SimdReduceKind::Or => MirSimdReduceKind::Or,
        SimdReduceKind::Xor => MirSimdReduceKind::Xor,
        SimdReduceKind::Min => MirSimdReduceKind::Min,
        SimdReduceKind::Max => MirSimdReduceKind::Max,
    }
}

pub(crate) fn build_from_mast_unoptimized(module: &MastModule) -> MirBuildReport {
    let globals = module
        .globals
        .iter()
        .map(|global| MirGlobal {
            id: global.id,
            name: global.name.clone(),
            span: global.span,
            linkage: lower_linkage(global.linkage),
            ty: global.ty,
            is_mut: global.is_mut,
            init: global
                .init
                .as_ref()
                .map(static_init::lower_static_init)
                .transpose()
                .unwrap_or_else(|error| {
                    panic!(
                        "Kern ICE (MIR Lower): failed to lower global `{}` initializer at {:?}: {}",
                        global.name, error.span, error.message
                    )
                }),
            is_extern: global.is_extern,
            attributes: global.attributes.clone(),
        })
        .collect::<Vec<_>>();
    let functions = module
        .functions
        .iter()
        .map(|function| {
            MirFunctionBuilder::build(function).unwrap_or_else(|error| {
                panic!(
                    "Kern ICE (MIR Lower): failed to lower function `{}` at {:?}: {}",
                    function.name, error.span, error.message
                )
            })
        })
        .collect::<Vec<_>>();

    let module = MirModule {
        name: module.name.clone(),
        structs: module.structs.iter().cloned().map(lower_struct).collect(),
        globals,
        functions,
        mono: module.mono.clone(),
    };
    kernc_mir::verify_module(&module).expect("Kern ICE (MIR): built invalid MIR.");
    let workload = module.workload_stats();
    let summary = module.summary_index();
    MirBuildReport {
        module,
        workload,
        summary,
        pass_pipeline: kernc_mir::MirPassPipelineReport::default(),
    }
}

#[derive(Debug, Clone, Copy)]
struct MirLoopTargets {
    break_block: MirBlockId,
    continue_block: MirBlockId,
}

#[derive(Debug, Clone, Default)]
struct MirBlockBuilder {
    instructions: Vec<MirInstructionData>,
    terminator: Option<MirTerminatorData>,
}

struct MirFunctionBuilder {
    locals: Vec<MirLocal>,
    next_local_id: u32,
    next_temp_name: usize,
    blocks: Vec<MirBlockBuilder>,
    loop_stack: Vec<MirLoopTargets>,
    local_scopes: Vec<HashMap<SymbolId, MirLocalId>>,
}

impl MirFunctionBuilder {
    fn build(function: &MastFunction) -> LowerResult<MirFunction> {
        let params = function
            .params
            .iter()
            .cloned()
            .map(lower_param)
            .collect::<Vec<_>>();
        let body = function
            .body
            .as_ref()
            .map(|body| Self::build_body(body, &params))
            .transpose()?;
        Ok(MirFunction {
            id: function.id,
            name: function.name.clone(),
            span: function.span,
            linkage: lower_linkage(function.linkage),
            params,
            ret_ty: function.ret_ty,
            body,
            is_extern: function.is_extern,
            is_variadic: function.is_variadic,
            inline_hint: lower_inline_hint(function.inline_hint),
            attributes: function.attributes.clone(),
        })
    }

    fn build_body(body: &MastBlock, params: &[MirParam]) -> LowerResult<MirBody> {
        let mut builder = Self {
            locals: vec![],
            next_local_id: 0,
            next_temp_name: 0,
            blocks: vec![],
            loop_stack: vec![],
            local_scopes: vec![],
        };
        builder.push_scope();
        for param in params {
            let local = builder.new_local(
                param.name,
                Span::default(),
                param.ty,
                param.is_mut,
                MirLocalKind::Param,
            );
            builder.bind_local(param.name, local);
        }
        let entry = builder.new_block();
        let _ = builder.lower_block(entry, body, None)?;
        builder.pop_scope();
        let locals = builder.locals;
        let blocks = builder
            .blocks
            .into_iter()
            .enumerate()
            .map(|(index, block)| MirBlock {
                id: MirBlockId(index as u32),
                instructions: block.instructions,
                terminator: block.terminator.unwrap_or(MirTerminatorData {
                    span: Span::default(),
                    kind: MirTerminator::Unreachable,
                }),
            })
            .collect::<Vec<_>>();
        Ok(MirBody {
            entry,
            locals,
            blocks,
        })
    }

    pub(super) fn new_block(&mut self) -> MirBlockId {
        let id = MirBlockId(self.blocks.len() as u32);
        self.blocks.push(MirBlockBuilder::default());
        id
    }

    pub(super) fn push_scope(&mut self) {
        self.local_scopes.push(HashMap::new());
    }

    pub(super) fn pop_scope(&mut self) {
        let _ = self.local_scopes.pop();
    }

    pub(super) fn bind_local(&mut self, name: SymbolId, local: MirLocalId) {
        if let Some(scope) = self.local_scopes.last_mut() {
            scope.insert(name, local);
        }
    }

    pub(super) fn lookup_local(&self, name: SymbolId) -> Option<MirLocalId> {
        self.local_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&name).copied())
    }

    pub(super) fn new_local(
        &mut self,
        name: SymbolId,
        span: Span,
        ty: TypeId,
        is_mut: bool,
        kind: MirLocalKind,
    ) -> MirLocalId {
        let id = MirLocalId(self.next_local_id);
        self.next_local_id += 1;
        self.locals.push(MirLocal {
            id,
            name,
            span,
            ty,
            is_mut,
            kind,
        });
        id
    }

    pub(super) fn new_temp_local(&mut self, ty: TypeId, span: Span) -> MirLocalId {
        let name = SymbolId(usize::MAX - self.next_temp_name);
        self.next_temp_name += 1;
        self.new_local(name, span, ty, false, MirLocalKind::Let)
    }

    fn current_block(&mut self, id: MirBlockId) -> &mut MirBlockBuilder {
        &mut self.blocks[id.0 as usize]
    }

    pub(super) fn emit_instruction(
        &mut self,
        id: MirBlockId,
        span: Span,
        instruction: MirInstruction,
    ) {
        self.current_block(id)
            .instructions
            .push(MirInstructionData {
                span,
                kind: instruction,
            });
    }

    pub(super) fn set_terminator(&mut self, id: MirBlockId, span: Span, terminator: MirTerminator) {
        self.current_block(id).terminator = Some(MirTerminatorData {
            span,
            kind: terminator,
        });
    }

    pub(super) fn unsupported_expr<T>(&self, expr: &MastExpr, context: &str) -> LowerResult<T> {
        Err(MirLowerError::new(
            expr.span,
            format!(
                "MAST expression `{}` is not lifted into MIR {}.",
                self.expr_kind_name(expr),
                context
            ),
        ))
    }

    fn expr_kind_name(&self, expr: &MastExpr) -> &'static str {
        match &expr.kind {
            MastExprKind::Undef => "undef",
            MastExprKind::Unreachable => "unreachable",
            MastExprKind::Trap => "trap",
            MastExprKind::Breakpoint => "breakpoint",
            MastExprKind::Integer(_) => "integer literal",
            MastExprKind::Float(_) => "float literal",
            MastExprKind::Bool(_) => "bool literal",
            MastExprKind::StringLiteral(_) => "string literal",
            MastExprKind::Var(_) => "variable reference",
            MastExprKind::GlobalRef(_) => "global reference",
            MastExprKind::FuncRef(_) => "function reference",
            MastExprKind::AddressOf(_) => "address-of",
            MastExprKind::Deref(_) => "deref",
            MastExprKind::StructInit { .. } => "struct init",
            MastExprKind::UnionInit { .. } => "union init",
            MastExprKind::ArrayInit(_) => "array init",
            MastExprKind::FieldAccess { .. } => "field access",
            MastExprKind::IndexAccess { .. } => "index access",
            MastExprKind::Call { .. } => "call",
            MastExprKind::If { .. } => "if expression",
            MastExprKind::Loop { .. } => "loop expression",
            MastExprKind::Switch { .. } => "switch expression",
            MastExprKind::Break => "break",
            MastExprKind::Continue => "continue",
            MastExprKind::Return(_) => "return",
            MastExprKind::Binary { .. } => "binary operator",
            MastExprKind::Unary { .. } => "unary operator",
            MastExprKind::Assign { .. } => "assignment",
            MastExprKind::Cast { .. } => "cast",
            MastExprKind::ConstructFatPointer { .. } => "fat-pointer construction",
            MastExprKind::ExtractFatPtrData(_) => "fat-pointer data projection",
            MastExprKind::ExtractFatPtrMeta(_) => "fat-pointer meta projection",
            MastExprKind::Block(_) => "block expression",
            MastExprKind::DataInit { .. } => "data init",
            MastExprKind::Asm(_) => "inline asm",
            MastExprKind::BitIntrinsic { .. } => "bit intrinsic",
            MastExprKind::SimdUnaryIntrinsic { .. } => "simd unary intrinsic",
            MastExprKind::SimdBinaryIntrinsic { .. } => "simd binary intrinsic",
            MastExprKind::SimdReduce { .. } => "simd reduce",
            MastExprKind::SimdAny { .. } => "simd any",
            MastExprKind::SimdAll { .. } => "simd all",
            MastExprKind::SimdBitmask { .. } => "simd bitmask",
            MastExprKind::SimdSplat { .. } => "simd splat",
            MastExprKind::SimdCast { .. } => "simd cast",
            MastExprKind::SimdBitcast { .. } => "simd bitcast",
            MastExprKind::SimdSelect { .. } => "simd select",
            MastExprKind::SimdShuffle { .. } => "simd shuffle",
            MastExprKind::SimdInsertHalf { .. } => "simd insert-half",
            MastExprKind::SimdLoad { .. } => "simd load",
            MastExprKind::SimdStore { .. } => "simd store",
            MastExprKind::SimdMaskedLoad { .. } => "simd masked load",
            MastExprKind::SimdMaskedStore { .. } => "simd masked store",
            MastExprKind::SimdGather { .. } => "simd gather",
            MastExprKind::SimdScatter { .. } => "simd scatter",
            MastExprKind::SimdMaskedGather { .. } => "simd masked gather",
            MastExprKind::SimdMaskedScatter { .. } => "simd masked scatter",
            MastExprKind::AtomicLoad { .. } => "atomic load",
            MastExprKind::AtomicStore { .. } => "atomic store",
            MastExprKind::AtomicCas { .. } => "atomic compare-exchange",
            MastExprKind::AtomicRmw { .. } => "atomic rmw",
            MastExprKind::Fence { .. } => "fence",
            MastExprKind::Memcpy { .. } => "memcpy",
            MastExprKind::Memmove { .. } => "memmove",
            MastExprKind::Memset { .. } => "memset",
            MastExprKind::SliceOp { .. } => "slice op",
        }
    }
}
