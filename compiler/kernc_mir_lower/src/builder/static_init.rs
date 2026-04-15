use super::*;

pub(super) fn lower_static_init(expr: &MastExpr) -> LowerResult<MirStaticInit> {
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
