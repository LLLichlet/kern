use crate::MirBuildReport;
use kernc_mast::{
    BitIntrinsicKind, MastBlock, MastCastKind, MastExpr, MastExprKind, MastField, MastFunction,
    MastInlineHint, MastLinkage, MastModule, MastParam, MastStmt, MastStruct,
    SimdBinaryIntrinsicKind, SimdReduceKind, SimdUnaryIntrinsicKind,
};
use kernc_mir::{
    MirAggregateKind, MirBitIntrinsicKind, MirBlock, MirBlockId, MirBody, MirCallTarget,
    MirCastKind, MirConst, MirField, MirFunction, MirGlobal, MirInlineAsm, MirInlineHint,
    MirInstruction, MirLinkage, MirLocal, MirLocalId, MirLocalKind, MirMemoryIntrinsic, MirModule,
    MirOperand, MirParam, MirPlace, MirProjectionKind, MirRvalue, MirSimdBinaryIntrinsicKind,
    MirSimdReduceKind, MirSimdUnaryIntrinsicKind, MirSliceBase, MirStaticInit, MirStruct,
    MirSwitchTarget, MirTerminator,
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

fn lower_cast_kind(kind: MastCastKind) -> MirCastKind {
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

fn lower_bit_intrinsic_kind(kind: BitIntrinsicKind) -> MirBitIntrinsicKind {
    match kind {
        BitIntrinsicKind::PopCount => MirBitIntrinsicKind::PopCount,
        BitIntrinsicKind::Clz => MirBitIntrinsicKind::Clz,
        BitIntrinsicKind::Ctz => MirBitIntrinsicKind::Ctz,
        BitIntrinsicKind::Bswap => MirBitIntrinsicKind::Bswap,
    }
}

fn lower_simd_unary_intrinsic_kind(kind: SimdUnaryIntrinsicKind) -> MirSimdUnaryIntrinsicKind {
    match kind {
        SimdUnaryIntrinsicKind::Abs => MirSimdUnaryIntrinsicKind::Abs,
        SimdUnaryIntrinsicKind::Sqrt => MirSimdUnaryIntrinsicKind::Sqrt,
        SimdUnaryIntrinsicKind::Floor => MirSimdUnaryIntrinsicKind::Floor,
        SimdUnaryIntrinsicKind::Ceil => MirSimdUnaryIntrinsicKind::Ceil,
        SimdUnaryIntrinsicKind::Trunc => MirSimdUnaryIntrinsicKind::Trunc,
        SimdUnaryIntrinsicKind::Round => MirSimdUnaryIntrinsicKind::Round,
    }
}

fn lower_simd_binary_intrinsic_kind(kind: SimdBinaryIntrinsicKind) -> MirSimdBinaryIntrinsicKind {
    match kind {
        SimdBinaryIntrinsicKind::Min => MirSimdBinaryIntrinsicKind::Min,
        SimdBinaryIntrinsicKind::Max => MirSimdBinaryIntrinsicKind::Max,
    }
}

fn lower_simd_reduce_kind(kind: SimdReduceKind) -> MirSimdReduceKind {
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
            linkage: lower_linkage(global.linkage),
            ty: global.ty,
            is_mut: global.is_mut,
            init: global
                .init
                .as_ref()
                .map(lower_static_init)
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

fn lower_static_init(expr: &MastExpr) -> LowerResult<MirStaticInit> {
    match &expr.kind {
        MastExprKind::Undef => Ok(MirStaticInit::Const(MirConst::Undef { ty: expr.ty })),
        MastExprKind::Integer(value) => Ok(MirStaticInit::Const(MirConst::Integer {
            ty: expr.ty,
            value: *value,
        })),
        MastExprKind::Float(value) => Ok(MirStaticInit::Const(MirConst::Float {
            ty: expr.ty,
            value: *value,
        })),
        MastExprKind::Bool(value) => Ok(MirStaticInit::Const(MirConst::Bool { value: *value })),
        MastExprKind::StringLiteral(value) => Ok(MirStaticInit::Const(MirConst::StringLiteral {
            ty: expr.ty,
            value: value.clone(),
        })),
        MastExprKind::GlobalRef(id) => Ok(MirStaticInit::Const(MirConst::GlobalRef {
            ty: expr.ty,
            id: *id,
        })),
        MastExprKind::FuncRef(id) => Ok(MirStaticInit::Const(MirConst::FuncRef {
            ty: expr.ty,
            id: *id,
        })),
        MastExprKind::ArrayInit(elems) => Ok(MirStaticInit::Array {
            ty: expr.ty,
            elems: elems
                .iter()
                .map(lower_static_init)
                .collect::<LowerResult<Vec<_>>>()?,
        }),
        MastExprKind::AddressOf(inner) => lower_static_address_of(expr.ty, inner, expr.span),
        MastExprKind::ConstructFatPointer { data_ptr, meta } => Ok(MirStaticInit::FatPointer {
            ty: expr.ty,
            data_ptr: Box::new(lower_static_init(data_ptr)?),
            meta: Box::new(lower_static_init(meta)?),
        }),
        MastExprKind::StructInit { struct_id, fields } => Ok(MirStaticInit::Struct {
            ty: expr.ty,
            struct_id: *struct_id,
            fields: fields
                .iter()
                .map(lower_static_init)
                .collect::<LowerResult<Vec<_>>>()?,
        }),
        MastExprKind::UnionInit {
            union_id,
            field_idx,
            value,
        } => Ok(MirStaticInit::Union {
            ty: expr.ty,
            union_id: *union_id,
            field_idx: *field_idx,
            value: Box::new(lower_static_init(value)?),
        }),
        MastExprKind::DataInit {
            data_struct_id,
            tag_value,
            payload,
        } => Ok(MirStaticInit::Data {
            ty: expr.ty,
            data_struct_id: *data_struct_id,
            tag_value: *tag_value,
            payload: (payload.ty != TypeId::VOID && payload.ty != TypeId::ERROR)
                .then(|| lower_static_init(payload))
                .transpose()?
                .map(Box::new),
        }),
        _ => Err(MirLowerError::new(
            expr.span,
            format!(
                "global initializer `{}` is not representable as MIR static init",
                match &expr.kind {
                    MastExprKind::AddressOf(_) => "address-of",
                    MastExprKind::Deref(_) => "deref",
                    MastExprKind::Var(_) => "variable reference",
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
                    MastExprKind::Unreachable => "unreachable",
                    MastExprKind::Trap => "trap",
                    MastExprKind::Breakpoint => "breakpoint",
                    MastExprKind::Undef
                    | MastExprKind::Integer(_)
                    | MastExprKind::Float(_)
                    | MastExprKind::Bool(_)
                    | MastExprKind::StringLiteral(_)
                    | MastExprKind::GlobalRef(_)
                    | MastExprKind::FuncRef(_)
                    | MastExprKind::ArrayInit(_)
                    | MastExprKind::StructInit { .. }
                    | MastExprKind::UnionInit { .. }
                    | MastExprKind::DataInit { .. } => unreachable!(),
                    MastExprKind::FieldAccess { .. } => "field access",
                    MastExprKind::IndexAccess { .. } => "index access",
                }
            ),
        )),
    }
}

fn lower_static_address_of(ty: TypeId, inner: &MastExpr, span: Span) -> LowerResult<MirStaticInit> {
    match &inner.kind {
        MastExprKind::GlobalRef(id) => {
            Ok(MirStaticInit::Const(MirConst::GlobalRef { ty, id: *id }))
        }
        MastExprKind::FuncRef(id) => Ok(MirStaticInit::Const(MirConst::FuncRef { ty, id: *id })),
        _ => Err(MirLowerError::new(
            span,
            format!(
                "global initializer `address-of {}` is not representable as MIR static init",
                match &inner.kind {
                    MastExprKind::Var(_) => "variable reference",
                    MastExprKind::Deref(_) => "deref",
                    MastExprKind::AddressOf(_) => "address-of",
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
                    MastExprKind::Unreachable => "unreachable",
                    MastExprKind::Trap => "trap",
                    MastExprKind::Breakpoint => "breakpoint",
                    MastExprKind::Undef => "undef",
                    MastExprKind::Integer(_) => "integer literal",
                    MastExprKind::Float(_) => "float literal",
                    MastExprKind::Bool(_) => "bool literal",
                    MastExprKind::StringLiteral(_) => "string literal",
                    MastExprKind::GlobalRef(_) | MastExprKind::FuncRef(_) => unreachable!(),
                    MastExprKind::ArrayInit(_) => "array init",
                    MastExprKind::StructInit { .. } => "struct init",
                    MastExprKind::UnionInit { .. } => "union init",
                    MastExprKind::DataInit { .. } => "data init",
                    MastExprKind::FieldAccess { .. } => "field access",
                    MastExprKind::IndexAccess { .. } => "index access",
                }
            ),
        )),
    }
}

#[derive(Debug, Clone, Copy)]
struct MirLoopTargets {
    break_block: MirBlockId,
    continue_block: MirBlockId,
}

#[derive(Debug, Clone, Default)]
struct MirBlockBuilder {
    instructions: Vec<MirInstruction>,
    terminator: Option<MirTerminator>,
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
            let local = builder.new_local(param.name, param.ty, param.is_mut, MirLocalKind::Param);
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
                terminator: block.terminator.unwrap_or(MirTerminator::Unreachable),
            })
            .collect::<Vec<_>>();
        Ok(MirBody {
            entry,
            locals,
            blocks,
        })
    }

    fn new_block(&mut self) -> MirBlockId {
        let id = MirBlockId(self.blocks.len() as u32);
        self.blocks.push(MirBlockBuilder::default());
        id
    }

    fn push_scope(&mut self) {
        self.local_scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        let _ = self.local_scopes.pop();
    }

    fn bind_local(&mut self, name: SymbolId, local: MirLocalId) {
        if let Some(scope) = self.local_scopes.last_mut() {
            scope.insert(name, local);
        }
    }

    fn lookup_local(&self, name: SymbolId) -> Option<MirLocalId> {
        self.local_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&name).copied())
    }

    fn new_local(
        &mut self,
        name: SymbolId,
        ty: TypeId,
        is_mut: bool,
        kind: MirLocalKind,
    ) -> MirLocalId {
        let id = MirLocalId(self.next_local_id);
        self.next_local_id += 1;
        self.locals.push(MirLocal {
            id,
            name,
            ty,
            is_mut,
            kind,
        });
        id
    }

    fn new_temp_local(&mut self, ty: TypeId) -> MirLocalId {
        let name = SymbolId(usize::MAX - self.next_temp_name);
        self.next_temp_name += 1;
        self.new_local(name, ty, false, MirLocalKind::Let)
    }

    fn current_block(&mut self, id: MirBlockId) -> &mut MirBlockBuilder {
        &mut self.blocks[id.0 as usize]
    }

    fn emit_instruction(&mut self, id: MirBlockId, instruction: MirInstruction) {
        self.current_block(id).instructions.push(instruction);
    }

    fn set_terminator(&mut self, id: MirBlockId, terminator: MirTerminator) {
        self.current_block(id).terminator = Some(terminator);
    }

    fn unsupported_expr<T>(&self, expr: &MastExpr, context: &str) -> LowerResult<T> {
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

    fn lower_rvalue(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<MirRvalue>> {
        match &expr.kind {
            MastExprKind::Block(_) | MastExprKind::If { .. } | MastExprKind::Switch { .. } => {
                let temp = self.new_temp_local(expr.ty);
                let Some(end_block) =
                    self.lower_expr_into_place(*block_id, expr, MirPlace::Local(temp))?
                else {
                    return Ok(None);
                };
                *block_id = end_block;
                Ok(Some(MirRvalue::Use(MirOperand::Local(temp))))
            }
            MastExprKind::Call { callee, args } => {
                let Some(callee) = self.lower_call_target(block_id, callee)? else {
                    return Ok(None);
                };
                let Some(args) = self.lower_operands(block_id, args)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Call { callee, args }))
            }
            MastExprKind::StructInit { struct_id, fields } => {
                let Some(fields) = self.lower_operands(block_id, fields)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Aggregate {
                    ty: expr.ty,
                    kind: MirAggregateKind::Struct {
                        struct_id: *struct_id,
                    },
                    fields,
                }))
            }
            MastExprKind::UnionInit {
                union_id,
                field_idx,
                value,
            } => {
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Aggregate {
                    ty: expr.ty,
                    kind: MirAggregateKind::Union {
                        union_id: *union_id,
                        field_idx: *field_idx,
                    },
                    fields: vec![value],
                }))
            }
            MastExprKind::ArrayInit(fields) => {
                let Some(fields) = self.lower_operands(block_id, fields)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Aggregate {
                    ty: expr.ty,
                    kind: MirAggregateKind::Array,
                    fields,
                }))
            }
            MastExprKind::ConstructFatPointer { data_ptr, meta } => {
                let Some(data_ptr) = self.lower_expr_to_operand(block_id, data_ptr)? else {
                    return Ok(None);
                };
                let Some(meta) = self.lower_expr_to_operand(block_id, meta)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Aggregate {
                    ty: expr.ty,
                    kind: MirAggregateKind::FatPointer,
                    fields: vec![data_ptr, meta],
                }))
            }
            MastExprKind::DataInit {
                data_struct_id,
                tag_value,
                payload,
            } => {
                let Some(payload) = self.lower_expr_to_operand(block_id, payload)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Aggregate {
                    ty: expr.ty,
                    kind: MirAggregateKind::Data {
                        data_struct_id: *data_struct_id,
                        tag_value: *tag_value,
                    },
                    fields: vec![payload],
                }))
            }
            MastExprKind::ExtractFatPtrData(inner) => {
                let Some(inner) = self.lower_expr_to_operand(block_id, inner)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Projection {
                    kind: MirProjectionKind::FatPtrData,
                    operand: inner,
                }))
            }
            MastExprKind::ExtractFatPtrMeta(inner) => {
                let Some(inner) = self.lower_expr_to_operand(block_id, inner)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Projection {
                    kind: MirProjectionKind::FatPtrMeta,
                    operand: inner,
                }))
            }
            MastExprKind::Unary { op, operand } => {
                let Some(operand) = self.lower_expr_to_operand(block_id, operand)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Unary { op: *op, operand }))
            }
            MastExprKind::Binary { op, lhs, rhs } => {
                let Some(lhs) = self.lower_expr_to_operand(block_id, lhs)? else {
                    return Ok(None);
                };
                let Some(rhs) = self.lower_expr_to_operand(block_id, rhs)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Binary { op: *op, lhs, rhs }))
            }
            MastExprKind::Cast { kind, operand } => {
                let Some(operand) = self.lower_expr_to_operand(block_id, operand)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Cast {
                    kind: lower_cast_kind(*kind),
                    operand,
                }))
            }
            MastExprKind::BitIntrinsic { kind, operand } => {
                let Some(operand) = self.lower_expr_to_operand(block_id, operand)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::BitIntrinsic {
                    kind: lower_bit_intrinsic_kind(*kind),
                    operand,
                }))
            }
            MastExprKind::AtomicLoad { ptr, ordering } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::AtomicLoad {
                    ptr,
                    ordering: *ordering,
                }))
            }
            MastExprKind::AtomicCas {
                weak,
                ptr,
                expected,
                desired,
                success,
                failure,
            } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(expected) = self.lower_expr_to_operand(block_id, expected)? else {
                    return Ok(None);
                };
                let Some(desired) = self.lower_expr_to_operand(block_id, desired)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::AtomicCas {
                    weak: *weak,
                    ptr,
                    expected,
                    desired,
                    success: *success,
                    failure: *failure,
                }))
            }
            MastExprKind::AtomicRmw {
                op,
                ptr,
                value,
                ordering,
            } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::AtomicRmw {
                    op: *op,
                    ptr,
                    value,
                    ordering: *ordering,
                }))
            }
            MastExprKind::SimdUnaryIntrinsic { kind, operand } => {
                let Some(operand) = self.lower_expr_to_operand(block_id, operand)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdUnaryIntrinsic {
                    kind: lower_simd_unary_intrinsic_kind(*kind),
                    operand,
                }))
            }
            MastExprKind::SimdBinaryIntrinsic { kind, lhs, rhs } => {
                let Some(lhs) = self.lower_expr_to_operand(block_id, lhs)? else {
                    return Ok(None);
                };
                let Some(rhs) = self.lower_expr_to_operand(block_id, rhs)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdBinaryIntrinsic {
                    kind: lower_simd_binary_intrinsic_kind(*kind),
                    lhs,
                    rhs,
                }))
            }
            MastExprKind::SimdReduce { kind, operand } => {
                let Some(operand) = self.lower_expr_to_operand(block_id, operand)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdReduce {
                    kind: lower_simd_reduce_kind(*kind),
                    operand,
                }))
            }
            MastExprKind::SimdAny { operand } => {
                let Some(operand) = self.lower_expr_to_operand(block_id, operand)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdAny { operand }))
            }
            MastExprKind::SimdAll { operand } => {
                let Some(operand) = self.lower_expr_to_operand(block_id, operand)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdAll { operand }))
            }
            MastExprKind::SimdBitmask { operand } => {
                let Some(operand) = self.lower_expr_to_operand(block_id, operand)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdBitmask { operand }))
            }
            MastExprKind::SimdSplat { value } => {
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdSplat { value }))
            }
            MastExprKind::SimdCast { value } => {
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdCast { value }))
            }
            MastExprKind::SimdBitcast { value } => {
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdBitcast { value }))
            }
            MastExprKind::SimdSelect {
                mask,
                on_true,
                on_false,
            } => {
                let Some(mask) = self.lower_expr_to_operand(block_id, mask)? else {
                    return Ok(None);
                };
                let Some(on_true) = self.lower_expr_to_operand(block_id, on_true)? else {
                    return Ok(None);
                };
                let Some(on_false) = self.lower_expr_to_operand(block_id, on_false)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdSelect {
                    mask,
                    on_true,
                    on_false,
                }))
            }
            MastExprKind::SimdShuffle { lhs, rhs, indices } => {
                let Some(lhs) = self.lower_expr_to_operand(block_id, lhs)? else {
                    return Ok(None);
                };
                let Some(rhs) = self.lower_expr_to_operand(block_id, rhs)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdShuffle {
                    lhs,
                    rhs,
                    indices: indices.clone(),
                }))
            }
            MastExprKind::SimdInsertHalf {
                base,
                half,
                high_half,
            } => {
                let Some(base) = self.lower_expr_to_operand(block_id, base)? else {
                    return Ok(None);
                };
                let Some(half) = self.lower_expr_to_operand(block_id, half)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdInsertHalf {
                    base,
                    half,
                    high_half: *high_half,
                }))
            }
            MastExprKind::SimdLoad { ptr, align } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdLoad { ptr, align: *align }))
            }
            MastExprKind::SimdMaskedLoad {
                ptr,
                mask,
                or_else,
                align,
            } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(mask) = self.lower_expr_to_operand(block_id, mask)? else {
                    return Ok(None);
                };
                let Some(or_else) = self.lower_expr_to_operand(block_id, or_else)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdMaskedLoad {
                    ptr,
                    mask,
                    or_else,
                    align: *align,
                }))
            }
            MastExprKind::SimdGather { ptr, indices } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(indices) = self.lower_expr_to_operand(block_id, indices)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdGather { ptr, indices }))
            }
            MastExprKind::SimdMaskedGather {
                ptr,
                indices,
                mask,
                or_else,
            } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(indices) = self.lower_expr_to_operand(block_id, indices)? else {
                    return Ok(None);
                };
                let Some(mask) = self.lower_expr_to_operand(block_id, mask)? else {
                    return Ok(None);
                };
                let Some(or_else) = self.lower_expr_to_operand(block_id, or_else)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::SimdMaskedGather {
                    ptr,
                    indices,
                    mask,
                    or_else,
                }))
            }
            MastExprKind::SliceOp {
                lhs,
                start,
                end,
                is_inclusive,
            } => {
                let Some(lhs) = self.lower_slice_base(block_id, lhs)? else {
                    return Ok(None);
                };
                let start = match start.as_deref() {
                    Some(start) => {
                        let Some(start) = self.lower_expr_to_operand(block_id, start)? else {
                            return Ok(None);
                        };
                        Some(start)
                    }
                    None => None,
                };
                let end = match end.as_deref() {
                    Some(end) => {
                        let Some(end) = self.lower_expr_to_operand(block_id, end)? else {
                            return Ok(None);
                        };
                        Some(end)
                    }
                    None => None,
                };
                Ok(Some(MirRvalue::SliceOp {
                    lhs,
                    start,
                    end,
                    is_inclusive: *is_inclusive,
                }))
            }
            MastExprKind::AddressOf(inner) => {
                let Some(place) = self.lower_place(block_id, inner)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::AddressOf(place)))
            }
            MastExprKind::Deref(_)
            | MastExprKind::FieldAccess { .. }
            | MastExprKind::IndexAccess { .. } => {
                let Some(place) = self.lower_place(block_id, expr)? else {
                    return Ok(None);
                };
                Ok(Some(MirRvalue::Load(place)))
            }
            MastExprKind::Return(_)
            | MastExprKind::Loop { .. }
            | MastExprKind::Break
            | MastExprKind::Continue
            | MastExprKind::Unreachable
            | MastExprKind::Trap => {
                let _ = self.lower_control_or_eval_stmt(*block_id, expr)?;
                Ok(None)
            }
            _ => {
                if let Some(operand) = self.lower_direct_operand(expr) {
                    Ok(Some(MirRvalue::Use(operand)))
                } else {
                    self.unsupported_expr(expr, "rvalue position")
                }
            }
        }
    }

    fn lower_call_target(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<MirCallTarget>> {
        match &expr.kind {
            MastExprKind::FuncRef(id) => Ok(Some(MirCallTarget::Direct(*id))),
            _ => {
                let Some(operand) = self.lower_expr_to_operand(block_id, expr)? else {
                    return Ok(None);
                };
                Ok(Some(MirCallTarget::Operand(operand)))
            }
        }
    }

    fn lower_direct_operand(&self, expr: &MastExpr) -> Option<MirOperand> {
        match &expr.kind {
            MastExprKind::Var(name) => self.lookup_local(*name).map(MirOperand::Local),
            MastExprKind::Undef => Some(MirOperand::Const(MirConst::Undef { ty: expr.ty })),
            MastExprKind::Integer(value) => Some(MirOperand::Const(MirConst::Integer {
                ty: expr.ty,
                value: *value,
            })),
            MastExprKind::Float(value) => Some(MirOperand::Const(MirConst::Float {
                ty: expr.ty,
                value: *value,
            })),
            MastExprKind::Bool(value) => Some(MirOperand::Const(MirConst::Bool { value: *value })),
            MastExprKind::StringLiteral(value) => {
                Some(MirOperand::Const(MirConst::StringLiteral {
                    ty: expr.ty,
                    value: value.clone(),
                }))
            }
            MastExprKind::GlobalRef(id) => Some(MirOperand::Const(MirConst::GlobalRef {
                ty: expr.ty,
                id: *id,
            })),
            MastExprKind::FuncRef(id) => Some(MirOperand::Const(MirConst::FuncRef {
                ty: expr.ty,
                id: *id,
            })),
            _ => None,
        }
    }

    fn lower_expr_to_operand(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<MirOperand>> {
        if let Some(operand) = self.lower_direct_operand(expr) {
            return Ok(Some(operand));
        }

        if matches!(
            expr.kind,
            MastExprKind::Block(_) | MastExprKind::If { .. } | MastExprKind::Switch { .. }
        ) {
            let temp = self.new_temp_local(expr.ty);
            let Some(end_block) =
                self.lower_expr_into_place(*block_id, expr, MirPlace::Local(temp))?
            else {
                return Ok(None);
            };
            *block_id = end_block;
            return Ok(Some(MirOperand::Local(temp)));
        }

        match &expr.kind {
            MastExprKind::Loop { .. }
            | MastExprKind::Break
            | MastExprKind::Continue
            | MastExprKind::Return(_)
            | MastExprKind::Unreachable
            | MastExprKind::Trap => {
                let _ = self.lower_control_or_eval_stmt(*block_id, expr)?;
                Ok(None)
            }
            MastExprKind::Assign { .. }
            | MastExprKind::Asm(_)
            | MastExprKind::SimdStore { .. }
            | MastExprKind::SimdMaskedStore { .. }
            | MastExprKind::SimdScatter { .. }
            | MastExprKind::SimdMaskedScatter { .. }
            | MastExprKind::AtomicStore { .. }
            | MastExprKind::Fence { .. }
            | MastExprKind::Memcpy { .. }
            | MastExprKind::Memmove { .. }
            | MastExprKind::Memset { .. } => self.unsupported_expr(expr, "operand position"),
            _ => {
                let temp = self.new_temp_local(expr.ty);
                let Some(init) = self.lower_rvalue(block_id, expr)? else {
                    return Ok(None);
                };
                self.emit_instruction(
                    *block_id,
                    MirInstruction::Let {
                        place: MirPlace::Local(temp),
                        init,
                    },
                );
                Ok(Some(MirOperand::Local(temp)))
            }
        }
    }

    fn lower_operands(
        &mut self,
        block_id: &mut MirBlockId,
        exprs: &[MastExpr],
    ) -> LowerResult<Option<Vec<MirOperand>>> {
        let mut operands = Vec::with_capacity(exprs.len());
        for expr in exprs {
            let Some(operand) = self.lower_expr_to_operand(block_id, expr)? else {
                return Ok(None);
            };
            operands.push(operand);
        }
        Ok(Some(operands))
    }

    fn expr_is_addressable(&self, expr: &MastExpr) -> bool {
        matches!(
            expr.kind,
            MastExprKind::Var(_)
                | MastExprKind::GlobalRef(_)
                | MastExprKind::FieldAccess { .. }
                | MastExprKind::IndexAccess { .. }
                | MastExprKind::Deref(_)
        )
    }

    fn lower_slice_base(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<MirSliceBase>> {
        if self.expr_is_addressable(expr) {
            let Some(place) = self.lower_place(block_id, expr)? else {
                return Ok(None);
            };
            return Ok(Some(MirSliceBase::Place(place)));
        }
        let Some(operand) = self.lower_expr_to_operand(block_id, expr)? else {
            return Ok(None);
        };
        Ok(Some(MirSliceBase::Operand(operand)))
    }

    fn lower_expr_into_place(
        &mut self,
        block_id: MirBlockId,
        expr: &MastExpr,
        place: MirPlace,
    ) -> LowerResult<Option<MirBlockId>> {
        match &expr.kind {
            MastExprKind::Block(block) => self.lower_value_block(block_id, block, place),
            MastExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.lower_value_if(block_id, cond, then_branch, else_branch.as_ref(), place),
            MastExprKind::Switch {
                target,
                cases,
                default_case,
            } => self.lower_value_switch(block_id, target, cases, default_case.as_ref(), place),
            _ => {
                let mut current = block_id;
                let Some(value) = self.lower_rvalue(&mut current, expr)? else {
                    return Ok(None);
                };
                self.emit_instruction(
                    current,
                    MirInstruction::Assign {
                        place,
                        op: kernc_ast::AssignmentOperator::Assign,
                        value,
                    },
                );
                Ok(Some(current))
            }
        }
    }

    fn lower_value_block(
        &mut self,
        start: MirBlockId,
        block: &MastBlock,
        place: MirPlace,
    ) -> LowerResult<Option<MirBlockId>> {
        self.push_scope();
        let mut current = Some(start);
        for stmt in &block.stmts {
            let Some(block_id) = current else {
                self.pop_scope();
                return Ok(None);
            };
            current = self.lower_stmt(block_id, stmt)?;
        }

        let Some(block_id) = current else {
            self.pop_scope();
            return Ok(None);
        };
        let mut after_defers = Some(block_id);
        for defer in &block.defers {
            let Some(defer_block) = after_defers else {
                self.pop_scope();
                return Ok(None);
            };
            after_defers = self.lower_defer_expr(defer_block, defer)?;
        }
        let Some(block_id) = after_defers else {
            self.pop_scope();
            return Ok(None);
        };
        let end = self.lower_value_tail(block_id, block.result.as_deref(), place)?;
        self.pop_scope();
        Ok(end)
    }

    fn lower_value_if(
        &mut self,
        block_id: MirBlockId,
        cond: &MastExpr,
        then_branch: &MastBlock,
        else_branch: Option<&MastBlock>,
        place: MirPlace,
    ) -> LowerResult<Option<MirBlockId>> {
        let then_block = self.new_block();
        let else_block = self.new_block();
        let join = self.new_block();
        let mut cond_block = block_id;
        let Some(cond) = self.lower_rvalue(&mut cond_block, cond)? else {
            return Ok(None);
        };
        self.set_terminator(
            cond_block,
            MirTerminator::Branch {
                cond,
                then_block,
                else_block,
            },
        );
        let then_end = self.lower_value_block(then_block, then_branch, place.clone())?;
        if let Some(then_end) = then_end {
            self.set_terminator(then_end, MirTerminator::Goto(join));
        }
        if let Some(else_branch) = else_branch {
            let else_end = self.lower_value_block(else_block, else_branch, place)?;
            if let Some(else_end) = else_end {
                self.set_terminator(else_end, MirTerminator::Goto(join));
            }
        } else {
            self.set_terminator(else_block, MirTerminator::Goto(join));
        }
        Ok(Some(join))
    }

    fn lower_value_switch(
        &mut self,
        block_id: MirBlockId,
        target: &MastExpr,
        cases: &[kernc_mast::MastSwitchCase],
        default_case: Option<&MastBlock>,
        place: MirPlace,
    ) -> LowerResult<Option<MirBlockId>> {
        let join = self.new_block();
        let mut mir_cases = Vec::with_capacity(cases.len());
        for case in cases {
            let case_block = self.new_block();
            mir_cases.push(MirSwitchTarget {
                values: case.values.clone(),
                block: case_block,
            });
        }
        let default_block = default_case.as_ref().map(|_| self.new_block());
        let mut target_block = block_id;
        let Some(target) = self.lower_rvalue(&mut target_block, target)? else {
            return Ok(None);
        };
        self.set_terminator(
            target_block,
            MirTerminator::Switch {
                target,
                cases: mir_cases.clone(),
                default_block,
            },
        );
        for (case, mir_case) in cases.iter().zip(mir_cases.iter()) {
            let end = self.lower_value_block(mir_case.block, &case.body, place.clone())?;
            if let Some(end) = end {
                self.set_terminator(end, MirTerminator::Goto(join));
            }
        }
        if let Some(default_case) = default_case {
            let default_id = default_block.expect("default block must exist");
            let end = self.lower_value_block(default_id, default_case, place)?;
            if let Some(end) = end {
                self.set_terminator(end, MirTerminator::Goto(join));
            }
        }
        Ok(Some(join))
    }

    fn lower_value_tail(
        &mut self,
        block_id: MirBlockId,
        result: Option<&MastExpr>,
        place: MirPlace,
    ) -> LowerResult<Option<MirBlockId>> {
        let Some(result) = result else {
            return Ok(Some(block_id));
        };
        match &result.kind {
            MastExprKind::Block(block) => self.lower_value_block(block_id, block, place),
            MastExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.lower_value_if(block_id, cond, then_branch, else_branch.as_ref(), place),
            MastExprKind::Switch {
                target,
                cases,
                default_case,
            } => self.lower_value_switch(block_id, target, cases, default_case.as_ref(), place),
            MastExprKind::Return(_) | MastExprKind::Break | MastExprKind::Continue => {
                self.lower_tail(block_id, Some(result), None)
            }
            MastExprKind::Loop { .. } => {
                let _ = self.lower_control_or_eval_stmt(block_id, result)?;
                Ok(None)
            }
            MastExprKind::Unreachable => {
                self.set_terminator(block_id, MirTerminator::Unreachable);
                Ok(None)
            }
            MastExprKind::Trap => self.lower_tail(block_id, Some(result), None),
            MastExprKind::Breakpoint => self.unsupported_expr(result, "value tail position"),
            _ => self.lower_expr_into_place(block_id, result, place),
        }
    }

    fn lower_place(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<MirPlace>> {
        match &expr.kind {
            MastExprKind::Var(name) => self
                .lookup_local(*name)
                .map(MirPlace::Local)
                .map(Some)
                .ok_or_else(|| {
                    MirLowerError::new(
                        expr.span,
                        format!("unresolved local binding {:?} in MIR place lowering", name),
                    )
                }),
            MastExprKind::GlobalRef(id) => Ok(Some(MirPlace::Global(*id))),
            MastExprKind::Deref(inner) => {
                let Some(inner) = self.lower_expr_to_operand(block_id, inner)? else {
                    return Ok(None);
                };
                Ok(Some(MirPlace::Deref(inner)))
            }
            MastExprKind::FieldAccess {
                lhs,
                struct_id,
                field_idx,
            } => {
                let Some(lhs) = self.lower_place(block_id, lhs)? else {
                    return Ok(None);
                };
                Ok(Some(MirPlace::Field {
                    base: Box::new(lhs),
                    struct_id: *struct_id,
                    field_idx: *field_idx,
                    field_ty: expr.ty,
                }))
            }
            MastExprKind::IndexAccess { lhs, index } => {
                let Some(lhs) = self.lower_place(block_id, lhs)? else {
                    return Ok(None);
                };
                let Some(index) = self.lower_expr_to_operand(block_id, index)? else {
                    return Ok(None);
                };
                Ok(Some(MirPlace::Index {
                    base: Box::new(lhs),
                    index,
                }))
            }
            _ => {
                let temp = self.new_temp_local(expr.ty);
                let Some(end_block) =
                    self.lower_expr_into_place(*block_id, expr, MirPlace::Local(temp))?
                else {
                    return if expr.ty == TypeId::NEVER {
                        Ok(None)
                    } else {
                        self.unsupported_expr(expr, "place position")
                    };
                };
                *block_id = end_block;
                Ok(Some(MirPlace::Local(temp)))
            }
        }
    }

    fn lower_assign_instruction(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<MirPlace>> {
        let MastExprKind::Assign { op, lhs, rhs } = &expr.kind else {
            return Ok(None);
        };
        let Some(place) = self.lower_place(block_id, lhs)? else {
            return Ok(None);
        };
        let Some(value) = self.lower_rvalue(block_id, rhs)? else {
            return Ok(None);
        };
        self.emit_instruction(
            *block_id,
            MirInstruction::Assign {
                place: place.clone(),
                op: *op,
                value,
            },
        );
        Ok(Some(place))
    }

    fn lower_memory_instruction(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<bool>> {
        let intrinsic = match &expr.kind {
            MastExprKind::Memcpy { dest, src, len } => {
                let Some(dest) = self.lower_expr_to_operand(block_id, dest)? else {
                    return Ok(None);
                };
                let Some(src) = self.lower_expr_to_operand(block_id, src)? else {
                    return Ok(None);
                };
                let Some(len) = self.lower_expr_to_operand(block_id, len)? else {
                    return Ok(None);
                };
                MirMemoryIntrinsic::Copy { dest, src, len }
            }
            MastExprKind::Memmove { dest, src, len } => {
                let Some(dest) = self.lower_expr_to_operand(block_id, dest)? else {
                    return Ok(None);
                };
                let Some(src) = self.lower_expr_to_operand(block_id, src)? else {
                    return Ok(None);
                };
                let Some(len) = self.lower_expr_to_operand(block_id, len)? else {
                    return Ok(None);
                };
                MirMemoryIntrinsic::Move { dest, src, len }
            }
            MastExprKind::Memset { dest, val, len } => {
                let Some(dest) = self.lower_expr_to_operand(block_id, dest)? else {
                    return Ok(None);
                };
                let Some(val) = self.lower_expr_to_operand(block_id, val)? else {
                    return Ok(None);
                };
                let Some(len) = self.lower_expr_to_operand(block_id, len)? else {
                    return Ok(None);
                };
                MirMemoryIntrinsic::Set { dest, val, len }
            }
            _ => return Ok(Some(false)),
        };
        self.emit_instruction(*block_id, MirInstruction::Memory(intrinsic));
        Ok(Some(true))
    }

    fn lower_atomic_instruction(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<bool>> {
        let instruction = match &expr.kind {
            MastExprKind::AtomicStore {
                ptr,
                value,
                ordering,
            } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                MirInstruction::AtomicStore {
                    ptr,
                    value,
                    ordering: *ordering,
                }
            }
            MastExprKind::Fence { ordering } => MirInstruction::Fence {
                ordering: *ordering,
            },
            _ => return Ok(Some(false)),
        };
        self.emit_instruction(*block_id, instruction);
        Ok(Some(true))
    }

    fn lower_inline_asm_instruction(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<bool>> {
        let MastExprKind::Asm(asm) = &expr.kind else {
            return Ok(Some(false));
        };
        let mut input_args = Vec::with_capacity(asm.input_args.len());
        for input in &asm.input_args {
            let Some(input) = self.lower_expr_to_operand(block_id, input)? else {
                return Ok(None);
            };
            input_args.push(input);
        }
        let mut output_ptrs = Vec::with_capacity(asm.output_ptrs.len());
        for output in &asm.output_ptrs {
            let Some(output) = self.lower_expr_to_operand(block_id, output)? else {
                return Ok(None);
            };
            output_ptrs.push(output);
        }
        self.emit_instruction(
            *block_id,
            MirInstruction::InlineAsm(MirInlineAsm {
                asm_template: asm.asm_template.clone(),
                constraints: asm.constraints.clone(),
                input_args,
                output_ptrs,
                output_tys: asm.output_tys.clone(),
                is_volatile: asm.is_volatile,
            }),
        );
        Ok(Some(true))
    }

    fn lower_simd_memory_instruction(
        &mut self,
        block_id: &mut MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<bool>> {
        let instruction = match &expr.kind {
            MastExprKind::SimdStore { ptr, value, align } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                MirInstruction::SimdStore {
                    ptr,
                    value,
                    align: *align,
                }
            }
            MastExprKind::SimdMaskedStore {
                ptr,
                mask,
                value,
                align,
            } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(mask) = self.lower_expr_to_operand(block_id, mask)? else {
                    return Ok(None);
                };
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                MirInstruction::SimdMaskedStore {
                    ptr,
                    mask,
                    value,
                    align: *align,
                }
            }
            MastExprKind::SimdScatter {
                ptr,
                indices,
                value,
            } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(indices) = self.lower_expr_to_operand(block_id, indices)? else {
                    return Ok(None);
                };
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                MirInstruction::SimdScatter {
                    ptr,
                    indices,
                    value,
                }
            }
            MastExprKind::SimdMaskedScatter {
                ptr,
                indices,
                mask,
                value,
            } => {
                let Some(ptr) = self.lower_expr_to_operand(block_id, ptr)? else {
                    return Ok(None);
                };
                let Some(indices) = self.lower_expr_to_operand(block_id, indices)? else {
                    return Ok(None);
                };
                let Some(mask) = self.lower_expr_to_operand(block_id, mask)? else {
                    return Ok(None);
                };
                let Some(value) = self.lower_expr_to_operand(block_id, value)? else {
                    return Ok(None);
                };
                MirInstruction::SimdMaskedScatter {
                    ptr,
                    indices,
                    mask,
                    value,
                }
            }
            _ => return Ok(Some(false)),
        };
        self.emit_instruction(*block_id, instruction);
        Ok(Some(true))
    }

    fn lower_block(
        &mut self,
        start: MirBlockId,
        block: &MastBlock,
        fallthrough: Option<MirBlockId>,
    ) -> LowerResult<Option<MirBlockId>> {
        self.push_scope();
        let mut current = Some(start);
        for stmt in &block.stmts {
            let Some(block_id) = current else {
                self.pop_scope();
                return Ok(None);
            };
            current = self.lower_stmt(block_id, stmt)?;
        }

        let Some(block_id) = current else {
            self.pop_scope();
            return Ok(None);
        };
        if let Some(result) = block.result.as_deref() {
            if result.ty == TypeId::NEVER {
                let end =
                    self.lower_never_block_tail(block_id, result, &block.defers, fallthrough)?;
                self.pop_scope();
                return Ok(end);
            }

            let mut block_id = block_id;
            if result.ty == TypeId::VOID || result.ty == TypeId::ERROR {
                let Some(end_block) = self.lower_control_or_eval_stmt(block_id, result)? else {
                    self.pop_scope();
                    return Ok(None);
                };
                block_id = end_block;
                let after_defers = self.lower_block_defers(block_id, &block.defers)?;
                let Some(block_id) = after_defers else {
                    self.pop_scope();
                    return Ok(None);
                };
                if let Some(next) = fallthrough {
                    self.set_terminator(block_id, MirTerminator::Goto(next));
                    self.pop_scope();
                    return Ok(Some(next));
                }
                self.set_terminator(block_id, MirTerminator::Return(None));
                self.pop_scope();
                return Ok(None);
            }

            let result_temp = self.new_temp_local(result.ty);
            let Some(end_block) =
                self.lower_expr_into_place(block_id, result, MirPlace::Local(result_temp))?
            else {
                self.pop_scope();
                return Ok(None);
            };
            let after_defers = self.lower_block_defers(end_block, &block.defers)?;
            let Some(block_id) = after_defers else {
                self.pop_scope();
                return Ok(None);
            };
            if let Some(next) = fallthrough {
                self.set_terminator(block_id, MirTerminator::Goto(next));
                self.pop_scope();
                return Ok(Some(next));
            }
            self.set_terminator(
                block_id,
                MirTerminator::Return(Some(MirRvalue::Use(MirOperand::Local(result_temp)))),
            );
            self.pop_scope();
            return Ok(None);
        }

        let Some(block_id) = self.lower_block_defers(block_id, &block.defers)? else {
            self.pop_scope();
            return Ok(None);
        };
        let end = self.lower_tail(block_id, None, fallthrough)?;
        self.pop_scope();
        Ok(end)
    }

    fn lower_block_defers(
        &mut self,
        block_id: MirBlockId,
        defers: &[MastExpr],
    ) -> LowerResult<Option<MirBlockId>> {
        let mut current = Some(block_id);
        for defer in defers {
            let Some(defer_block) = current else {
                return Ok(None);
            };
            current = self.lower_defer_expr(defer_block, defer)?;
        }
        Ok(current)
    }

    fn lower_never_block_tail(
        &mut self,
        block_id: MirBlockId,
        result: &MastExpr,
        defers: &[MastExpr],
        fallthrough: Option<MirBlockId>,
    ) -> LowerResult<Option<MirBlockId>> {
        match &result.kind {
            MastExprKind::Return(value) => {
                let mut block_id = block_id;
                let ret_value = match value.as_deref() {
                    Some(value) if value.ty != TypeId::VOID && value.ty != TypeId::ERROR => {
                        let ret_temp = self.new_temp_local(value.ty);
                        let Some(end_block) =
                            self.lower_expr_into_place(block_id, value, MirPlace::Local(ret_temp))?
                        else {
                            return Ok(None);
                        };
                        block_id = end_block;
                        Some(MirRvalue::Use(MirOperand::Local(ret_temp)))
                    }
                    Some(value) => {
                        let Some(end_block) = self.lower_control_or_eval_stmt(block_id, value)?
                        else {
                            return Ok(None);
                        };
                        block_id = end_block;
                        None
                    }
                    None => None,
                };
                let Some(block_id) = self.lower_block_defers(block_id, defers)? else {
                    return Ok(None);
                };
                self.set_terminator(block_id, MirTerminator::Return(ret_value));
                Ok(None)
            }
            MastExprKind::Break => {
                let Some(block_id) = self.lower_block_defers(block_id, defers)? else {
                    return Ok(None);
                };
                let break_block = self
                    .loop_stack
                    .last()
                    .map(|targets| targets.break_block)
                    .unwrap_or_else(|| self.new_block());
                self.set_terminator(block_id, MirTerminator::Goto(break_block));
                Ok(None)
            }
            MastExprKind::Continue => {
                let Some(block_id) = self.lower_block_defers(block_id, defers)? else {
                    return Ok(None);
                };
                let continue_block = self
                    .loop_stack
                    .last()
                    .map(|targets| targets.continue_block)
                    .unwrap_or_else(|| self.new_block());
                self.set_terminator(block_id, MirTerminator::Goto(continue_block));
                Ok(None)
            }
            _ => {
                let Some(block_id) = self.lower_block_defers(block_id, defers)? else {
                    return Ok(None);
                };
                self.lower_tail(block_id, Some(result), fallthrough)
            }
        }
    }

    fn lower_defer_expr(
        &mut self,
        block_id: MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<MirBlockId>> {
        match &expr.kind {
            MastExprKind::Trap => {
                self.emit_instruction(block_id, MirInstruction::Trap);
                self.set_terminator(block_id, MirTerminator::Unreachable);
                Ok(None)
            }
            MastExprKind::Breakpoint => {
                self.emit_instruction(block_id, MirInstruction::Breakpoint);
                Ok(Some(block_id))
            }
            MastExprKind::Unreachable => {
                self.set_terminator(block_id, MirTerminator::Unreachable);
                Ok(None)
            }
            _ => {
                let mut defer_block = block_id;
                let Some(defer_rvalue) = self.lower_rvalue(&mut defer_block, expr)? else {
                    return Ok(None);
                };
                self.emit_instruction(defer_block, MirInstruction::Defer(defer_rvalue));
                Ok(Some(defer_block))
            }
        }
    }

    fn lower_stmt(
        &mut self,
        block_id: MirBlockId,
        stmt: &MastStmt,
    ) -> LowerResult<Option<MirBlockId>> {
        match stmt {
            MastStmt::Let {
                name,
                ty,
                is_mut,
                init,
            } => {
                let mut block_id = block_id;
                let local = self.new_local(*name, *ty, *is_mut, MirLocalKind::Let);
                let Some(init) = self.lower_rvalue(&mut block_id, init)? else {
                    return Ok(None);
                };
                self.bind_local(*name, local);
                self.emit_instruction(
                    block_id,
                    MirInstruction::Let {
                        place: MirPlace::Local(local),
                        init,
                    },
                );
                Ok(Some(block_id))
            }
            MastStmt::Expr(expr) => self.lower_control_or_eval_stmt(block_id, expr),
        }
    }

    fn lower_control_or_eval_stmt(
        &mut self,
        block_id: MirBlockId,
        expr: &MastExpr,
    ) -> LowerResult<Option<MirBlockId>> {
        let mut block_id = block_id;
        match &expr.kind {
            MastExprKind::Block(block) => {
                let join = self.new_block();
                let end = self.lower_block(block_id, block, Some(join))?;
                Ok(if end.is_none() { None } else { Some(join) })
            }
            MastExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let then_block = self.new_block();
                let else_block = self.new_block();
                let join = self.new_block();
                let Some(cond) = self.lower_rvalue(&mut block_id, cond)? else {
                    return Ok(None);
                };
                self.set_terminator(
                    block_id,
                    MirTerminator::Branch {
                        cond,
                        then_block,
                        else_block,
                    },
                );
                let then_end = self.lower_block(then_block, then_branch, Some(join))?;
                if then_end.is_none() && else_branch.is_none() {
                    self.set_terminator(else_block, MirTerminator::Goto(join));
                } else if let Some(else_branch) = else_branch {
                    let _ = self.lower_block(else_block, else_branch, Some(join))?;
                } else {
                    self.set_terminator(else_block, MirTerminator::Goto(join));
                }
                Ok(Some(join))
            }
            MastExprKind::Switch {
                target,
                cases,
                default_case,
            } => {
                let join = self.new_block();
                let mut mir_cases = Vec::with_capacity(cases.len());
                for case in cases {
                    let case_block = self.new_block();
                    mir_cases.push(MirSwitchTarget {
                        values: case.values.clone(),
                        block: case_block,
                    });
                }
                let default_block = default_case.as_ref().map(|_| self.new_block());
                let Some(target) = self.lower_rvalue(&mut block_id, target)? else {
                    return Ok(None);
                };
                self.set_terminator(
                    block_id,
                    MirTerminator::Switch {
                        target,
                        cases: mir_cases.clone(),
                        default_block,
                    },
                );
                for (case, mir_case) in cases.iter().zip(mir_cases.iter()) {
                    let _ = self.lower_block(mir_case.block, &case.body, Some(join))?;
                }
                if let Some(default_case) = default_case {
                    let _ = self.lower_block(
                        default_block.expect("default block must exist"),
                        default_case,
                        Some(join),
                    )?;
                }
                Ok(Some(join))
            }
            MastExprKind::Loop { body, latch } => {
                let body_block = self.new_block();
                let continue_block = latch
                    .as_ref()
                    .map(|_| self.new_block())
                    .unwrap_or(body_block);
                let exit_block = self.new_block();
                self.set_terminator(block_id, MirTerminator::Goto(body_block));
                self.loop_stack.push(MirLoopTargets {
                    break_block: exit_block,
                    continue_block,
                });
                let _ = self.lower_block(body_block, body, Some(continue_block))?;
                if let Some(latch) = latch {
                    let _ = self.lower_block(continue_block, latch, Some(body_block))?;
                }
                self.loop_stack.pop();
                Ok(Some(exit_block))
            }
            MastExprKind::Return(value) => {
                let ret_value = match value.as_deref() {
                    Some(value) => match self.lower_rvalue(&mut block_id, value)? {
                        Some(value) => Some(value),
                        None => return Ok(None),
                    },
                    None => None,
                };
                self.set_terminator(block_id, MirTerminator::Return(ret_value));
                Ok(None)
            }
            MastExprKind::Break => {
                let break_block = self
                    .loop_stack
                    .last()
                    .map(|targets| targets.break_block)
                    .unwrap_or_else(|| self.new_block());
                self.set_terminator(block_id, MirTerminator::Goto(break_block));
                Ok(None)
            }
            MastExprKind::Continue => {
                let continue_block = self
                    .loop_stack
                    .last()
                    .map(|targets| targets.continue_block)
                    .unwrap_or_else(|| self.new_block());
                self.set_terminator(block_id, MirTerminator::Goto(continue_block));
                Ok(None)
            }
            MastExprKind::Assign { .. } => {
                if self
                    .lower_assign_instruction(&mut block_id, expr)?
                    .is_none()
                {
                    return Ok(None);
                }
                Ok(Some(block_id))
            }
            MastExprKind::Unreachable => {
                self.set_terminator(block_id, MirTerminator::Unreachable);
                Ok(None)
            }
            MastExprKind::Trap => {
                self.emit_instruction(block_id, MirInstruction::Trap);
                self.set_terminator(block_id, MirTerminator::Unreachable);
                Ok(None)
            }
            MastExprKind::Breakpoint => {
                self.emit_instruction(block_id, MirInstruction::Breakpoint);
                Ok(Some(block_id))
            }
            MastExprKind::Memcpy { .. }
            | MastExprKind::Memmove { .. }
            | MastExprKind::Memset { .. } => {
                if self
                    .lower_memory_instruction(&mut block_id, expr)?
                    .is_none()
                {
                    return Ok(None);
                }
                Ok(Some(block_id))
            }
            MastExprKind::AtomicStore { .. } | MastExprKind::Fence { .. } => {
                if self
                    .lower_atomic_instruction(&mut block_id, expr)?
                    .is_none()
                {
                    return Ok(None);
                }
                Ok(Some(block_id))
            }
            MastExprKind::SimdStore { .. }
            | MastExprKind::SimdMaskedStore { .. }
            | MastExprKind::SimdScatter { .. }
            | MastExprKind::SimdMaskedScatter { .. } => {
                if self
                    .lower_simd_memory_instruction(&mut block_id, expr)?
                    .is_none()
                {
                    return Ok(None);
                }
                Ok(Some(block_id))
            }
            MastExprKind::Asm(_) => {
                if self
                    .lower_inline_asm_instruction(&mut block_id, expr)?
                    .is_none()
                {
                    return Ok(None);
                }
                Ok(Some(block_id))
            }
            _ => {
                let Some(value) = self.lower_rvalue(&mut block_id, expr)? else {
                    return Ok(None);
                };
                self.emit_instruction(block_id, MirInstruction::Eval(value));
                Ok(Some(block_id))
            }
        }
    }

    fn lower_tail(
        &mut self,
        block_id: MirBlockId,
        result: Option<&MastExpr>,
        fallthrough: Option<MirBlockId>,
    ) -> LowerResult<Option<MirBlockId>> {
        let mut block_id = block_id;
        let Some(result) = result else {
            if let Some(next) = fallthrough {
                self.set_terminator(block_id, MirTerminator::Goto(next));
                return Ok(Some(next));
            }
            self.set_terminator(block_id, MirTerminator::Return(None));
            return Ok(None);
        };

        match &result.kind {
            MastExprKind::Block(block) => self.lower_block(block_id, block, fallthrough),
            MastExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let then_block = self.new_block();
                let else_block = self.new_block();
                let Some(cond) = self.lower_rvalue(&mut block_id, cond)? else {
                    return Ok(None);
                };
                self.set_terminator(
                    block_id,
                    MirTerminator::Branch {
                        cond,
                        then_block,
                        else_block,
                    },
                );
                let _ = self.lower_block(then_block, then_branch, fallthrough)?;
                if let Some(else_branch) = else_branch {
                    let _ = self.lower_block(else_block, else_branch, fallthrough)?;
                } else if let Some(next) = fallthrough {
                    self.set_terminator(else_block, MirTerminator::Goto(next));
                } else {
                    self.set_terminator(else_block, MirTerminator::Return(None));
                }
                Ok(fallthrough)
            }
            MastExprKind::Switch {
                target,
                cases,
                default_case,
            } => {
                let mut mir_cases = Vec::with_capacity(cases.len());
                for _ in cases {
                    mir_cases.push(MirSwitchTarget {
                        values: vec![],
                        block: self.new_block(),
                    });
                }
                for (mir_case, case) in mir_cases.iter_mut().zip(cases.iter()) {
                    mir_case.values = case.values.clone();
                    let _ = self.lower_block(mir_case.block, &case.body, fallthrough)?;
                }
                let default_block = if let Some(default_case) = default_case {
                    let id = self.new_block();
                    let _ = self.lower_block(id, default_case, fallthrough)?;
                    Some(id)
                } else {
                    None
                };
                let Some(target) = self.lower_rvalue(&mut block_id, target)? else {
                    return Ok(None);
                };
                self.set_terminator(
                    block_id,
                    MirTerminator::Switch {
                        target,
                        cases: mir_cases,
                        default_block,
                    },
                );
                Ok(fallthrough)
            }
            MastExprKind::Loop { .. }
            | MastExprKind::Return(_)
            | MastExprKind::Break
            | MastExprKind::Continue => self.lower_control_or_eval_stmt(block_id, result),
            MastExprKind::Unreachable => {
                self.set_terminator(block_id, MirTerminator::Unreachable);
                Ok(None)
            }
            MastExprKind::Trap => {
                self.emit_instruction(block_id, MirInstruction::Trap);
                self.set_terminator(block_id, MirTerminator::Unreachable);
                Ok(None)
            }
            MastExprKind::Breakpoint => {
                self.emit_instruction(block_id, MirInstruction::Breakpoint);
                if let Some(next) = fallthrough {
                    self.set_terminator(block_id, MirTerminator::Goto(next));
                    Ok(Some(next))
                } else {
                    Ok(Some(block_id))
                }
            }
            MastExprKind::Assign { .. } => {
                let Some(place) = self.lower_assign_instruction(&mut block_id, result)? else {
                    return Ok(None);
                };
                if let Some(next) = fallthrough {
                    self.set_terminator(block_id, MirTerminator::Goto(next));
                    Ok(Some(next))
                } else {
                    self.set_terminator(
                        block_id,
                        MirTerminator::Return(Some(MirRvalue::Load(place))),
                    );
                    Ok(None)
                }
            }
            MastExprKind::Memcpy { .. }
            | MastExprKind::Memmove { .. }
            | MastExprKind::Memset { .. } => {
                if self
                    .lower_memory_instruction(&mut block_id, result)?
                    .is_none()
                {
                    return Ok(None);
                }
                if let Some(next) = fallthrough {
                    self.set_terminator(block_id, MirTerminator::Goto(next));
                    Ok(Some(next))
                } else {
                    self.set_terminator(block_id, MirTerminator::Return(None));
                    Ok(None)
                }
            }
            MastExprKind::AtomicStore { .. } | MastExprKind::Fence { .. } => {
                if self
                    .lower_atomic_instruction(&mut block_id, result)?
                    .is_none()
                {
                    return Ok(None);
                }
                if let Some(next) = fallthrough {
                    self.set_terminator(block_id, MirTerminator::Goto(next));
                    Ok(Some(next))
                } else {
                    self.set_terminator(block_id, MirTerminator::Return(None));
                    Ok(None)
                }
            }
            MastExprKind::SimdStore { .. }
            | MastExprKind::SimdMaskedStore { .. }
            | MastExprKind::SimdScatter { .. }
            | MastExprKind::SimdMaskedScatter { .. } => {
                if self
                    .lower_simd_memory_instruction(&mut block_id, result)?
                    .is_none()
                {
                    return Ok(None);
                }
                if let Some(next) = fallthrough {
                    self.set_terminator(block_id, MirTerminator::Goto(next));
                    Ok(Some(next))
                } else {
                    self.set_terminator(block_id, MirTerminator::Return(None));
                    Ok(None)
                }
            }
            MastExprKind::Asm(_) => {
                if self
                    .lower_inline_asm_instruction(&mut block_id, result)?
                    .is_none()
                {
                    return Ok(None);
                }
                if let Some(next) = fallthrough {
                    self.set_terminator(block_id, MirTerminator::Goto(next));
                    Ok(Some(next))
                } else {
                    self.set_terminator(block_id, MirTerminator::Return(None));
                    Ok(None)
                }
            }
            _ => {
                let Some(lowered) = self.lower_rvalue(&mut block_id, result)? else {
                    return Ok(None);
                };
                if let Some(next) = fallthrough {
                    self.emit_instruction(block_id, MirInstruction::Eval(lowered));
                    self.set_terminator(block_id, MirTerminator::Goto(next));
                    Ok(Some(next))
                } else {
                    self.set_terminator(block_id, MirTerminator::Return(Some(lowered)));
                    Ok(None)
                }
            }
        }
    }
}
