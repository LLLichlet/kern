use super::{
    ConstExprBinaryOp, ConstExprId, ConstExprKind, ConstExprUnaryOp, ConstGeneric,
    ConstGenericValue, ConstGenericValueKind, PrimitiveType, TypeId, TypeKind,
};
use kernc_utils::FastHashMap;

/// Interning table for semantic types.
#[derive(Clone)]
pub struct TypeRegistry {
    /// Dense storage for type structures.
    types: Vec<TypeKind>,

    /// Deduplication map that guarantees identical types share one `TypeId`.
    interner: FastHashMap<TypeKind, TypeId>,

    /// Dense storage for interned const-generic expression nodes.
    const_exprs: Vec<ConstExprKind>,

    /// Deduplication map for const-generic expression nodes.
    const_expr_interner: FastHashMap<ConstExprKind, ConstExprId>,
}

impl Default for TypeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeRegistry {
    pub fn new() -> Self {
        let mut reg = Self {
            types: Vec::new(),
            interner: FastHashMap::default(),
            const_exprs: Vec::new(),
            const_expr_interner: FastHashMap::default(),
        };
        reg.init_primitives();
        reg
    }

    fn init_primitives(&mut self) {
        // Keep this order in sync with the reserved `TypeId` constants.
        self.add_primitive(PrimitiveType::Void); // 0
        self.add_primitive(PrimitiveType::Bool); // 1
        self.add_primitive(PrimitiveType::I8); // 2
        self.add_primitive(PrimitiveType::I16); // 3
        self.add_primitive(PrimitiveType::I32); // 4
        self.add_primitive(PrimitiveType::I64); // 5
        self.add_primitive(PrimitiveType::I128); // 6
        self.add_primitive(PrimitiveType::U8); // 7
        self.add_primitive(PrimitiveType::U16); // 8
        self.add_primitive(PrimitiveType::U32); // 9
        self.add_primitive(PrimitiveType::U64); // 10
        self.add_primitive(PrimitiveType::U128); // 11
        self.add_primitive(PrimitiveType::F32); // 12
        self.add_primitive(PrimitiveType::F64); // 13
        self.add_primitive(PrimitiveType::ISize); // 14
        self.add_primitive(PrimitiveType::USize); // 15
        self.add_primitive(PrimitiveType::Never); // 16

        // 17: Error
        self.types.push(TypeKind::Error);
    }

    fn add_primitive(&mut self, p: PrimitiveType) {
        let kind = TypeKind::Primitive(p);
        let id = TypeId(self.types.len() as u32);
        self.types.push(kind.clone());
        self.interner.insert(kind, id);
    }

    /// Return the canonical ID for a type, creating it if needed.
    pub fn intern(&mut self, kind: TypeKind) -> TypeId {
        if let Some(&id) = self.interner.get(&kind) {
            return id;
        }

        let id = TypeId(self.types.len() as u32);
        self.types.push(kind.clone());
        self.interner.insert(kind, id);
        id
    }

    /// Borrow the type structure referenced by an ID.
    pub fn get(&self, id: TypeId) -> &TypeKind {
        &self.types[id.0 as usize]
    }

    pub fn intern_const_expr(&mut self, kind: ConstExprKind) -> ConstExprId {
        if let Some(&id) = self.const_expr_interner.get(&kind) {
            return id;
        }

        let id = ConstExprId(self.const_exprs.len() as u32);
        self.const_exprs.push(kind);
        self.const_expr_interner.insert(kind, id);
        id
    }

    pub fn const_expr(&self, id: ConstExprId) -> &ConstExprKind {
        &self.const_exprs[id.0 as usize]
    }

    pub fn const_generic_ty(&self, value: ConstGeneric) -> Option<TypeId> {
        match value {
            ConstGeneric::Value(value) => Some(value.ty),
            ConstGeneric::Param(_, ty) => Some(ty),
            ConstGeneric::Expr(id) => match self.const_expr(id) {
                ConstExprKind::Unary { ty, .. }
                | ConstExprKind::Binary { ty, .. }
                | ConstExprKind::Cast { ty, .. } => Some(*ty),
            },
            ConstGeneric::Error => None,
        }
    }

    pub fn const_generic_contains_params(&self, value: ConstGeneric) -> bool {
        match value {
            ConstGeneric::Value(_) => false,
            ConstGeneric::Param(_, _) | ConstGeneric::Error => true,
            ConstGeneric::Expr(id) => match *self.const_expr(id) {
                ConstExprKind::Unary { expr, .. } | ConstExprKind::Cast { expr, .. } => {
                    self.const_generic_contains_params(expr)
                }
                ConstExprKind::Binary { lhs, rhs, .. } => {
                    self.const_generic_contains_params(lhs)
                        || self.const_generic_contains_params(rhs)
                }
            },
        }
    }

    pub fn fold_const_generic(&mut self, value: ConstGeneric) -> ConstGeneric {
        match value {
            ConstGeneric::Expr(id) => match *self.const_expr(id) {
                ConstExprKind::Unary { op, expr, ty } => {
                    let expr = self.fold_const_generic(expr);
                    if let Some(value) = self.eval_const_expr(ConstExprKind::Unary { op, expr, ty })
                    {
                        ConstGeneric::Value(value)
                    } else {
                        ConstGeneric::Expr(self.intern_const_expr(ConstExprKind::Unary {
                            op,
                            expr,
                            ty,
                        }))
                    }
                }
                ConstExprKind::Binary { op, lhs, rhs, ty } => {
                    let lhs = self.fold_const_generic(lhs);
                    let rhs = self.fold_const_generic(rhs);
                    if let Some(value) =
                        self.eval_const_expr(ConstExprKind::Binary { op, lhs, rhs, ty })
                    {
                        ConstGeneric::Value(value)
                    } else {
                        ConstGeneric::Expr(self.intern_const_expr(ConstExprKind::Binary {
                            op,
                            lhs,
                            rhs,
                            ty,
                        }))
                    }
                }
                ConstExprKind::Cast { expr, ty } => {
                    let expr = self.fold_const_generic(expr);
                    if let Some(value) = self.eval_const_expr(ConstExprKind::Cast { expr, ty }) {
                        ConstGeneric::Value(value)
                    } else {
                        ConstGeneric::Expr(self.intern_const_expr(ConstExprKind::Cast { expr, ty }))
                    }
                }
            },
            other => other,
        }
    }

    fn eval_const_expr(&self, expr: ConstExprKind) -> Option<ConstGenericValue> {
        match expr {
            ConstExprKind::Unary { op, expr, ty } => {
                let value = self.const_generic_scalar(expr)?;
                let result = match op {
                    ConstExprUnaryOp::Negate => value.wrapping_neg(),
                    ConstExprUnaryOp::BitwiseNot => !value,
                };
                self.coerce_const_scalar(result, ty)
            }
            ConstExprKind::Binary { op, lhs, rhs, ty } => {
                let lhs = self.const_generic_scalar(lhs)?;
                let rhs = self.const_generic_scalar(rhs)?;
                let result = match op {
                    ConstExprBinaryOp::Add => lhs.wrapping_add(rhs),
                    ConstExprBinaryOp::Subtract => lhs.wrapping_sub(rhs),
                    ConstExprBinaryOp::Multiply => lhs.wrapping_mul(rhs),
                    ConstExprBinaryOp::Divide => {
                        if rhs == 0 {
                            return None;
                        }
                        lhs.wrapping_div(rhs)
                    }
                    ConstExprBinaryOp::Modulo => {
                        if rhs == 0 {
                            return None;
                        }
                        lhs.wrapping_rem(rhs)
                    }
                    ConstExprBinaryOp::BitwiseAnd => lhs & rhs,
                    ConstExprBinaryOp::BitwiseOr => lhs | rhs,
                    ConstExprBinaryOp::BitwiseXor => lhs ^ rhs,
                    ConstExprBinaryOp::ShiftLeft => {
                        let shift = u32::try_from(rhs).ok()?;
                        lhs.checked_shl(shift).unwrap_or(0)
                    }
                    ConstExprBinaryOp::ShiftRight => {
                        let shift = u32::try_from(rhs).ok()?;
                        lhs.checked_shr(shift).unwrap_or(0)
                    }
                };
                self.coerce_const_scalar(result, ty)
            }
            ConstExprKind::Cast { expr, ty } => {
                let value = self.const_generic_scalar(expr)?;
                self.coerce_const_scalar(value, ty)
            }
        }
    }

    fn const_generic_scalar(&self, value: ConstGeneric) -> Option<i128> {
        match value {
            ConstGeneric::Value(value) => value.as_int(),
            ConstGeneric::Expr(id) => self
                .eval_const_expr(*self.const_expr(id))
                .and_then(|value| value.as_int()),
            ConstGeneric::Param(_, _) | ConstGeneric::Error => None,
        }
    }

    fn coerce_const_scalar(&self, value: i128, ty: TypeId) -> Option<ConstGenericValue> {
        let norm = self.normalize(ty);
        let bit_width = match self.get(norm) {
            TypeKind::Primitive(PrimitiveType::I8 | PrimitiveType::U8) => 8,
            TypeKind::Primitive(PrimitiveType::I16 | PrimitiveType::U16) => 16,
            TypeKind::Primitive(PrimitiveType::I32 | PrimitiveType::U32) => 32,
            TypeKind::Primitive(PrimitiveType::I64 | PrimitiveType::U64) => 64,
            TypeKind::Primitive(PrimitiveType::I128 | PrimitiveType::U128) => 128,
            TypeKind::Primitive(PrimitiveType::ISize | PrimitiveType::USize) => 64,
            _ => return None,
        };

        let coerced = match self.get(norm) {
            TypeKind::Primitive(
                PrimitiveType::U8
                | PrimitiveType::U16
                | PrimitiveType::U32
                | PrimitiveType::U64
                | PrimitiveType::U128
                | PrimitiveType::USize,
            ) => {
                if value < 0 {
                    return None;
                }
                if bit_width >= 128 {
                    value
                } else {
                    let max = (1u128 << bit_width) - 1;
                    if (value as u128) > max {
                        return None;
                    }
                    value
                }
            }
            TypeKind::Primitive(
                PrimitiveType::I8
                | PrimitiveType::I16
                | PrimitiveType::I32
                | PrimitiveType::I64
                | PrimitiveType::I128
                | PrimitiveType::ISize,
            ) => {
                if bit_width >= 128 {
                    value
                } else {
                    let max = (1i128 << (bit_width - 1)) - 1;
                    let min = -(1i128 << (bit_width - 1));
                    if value < min || value > max {
                        return None;
                    }
                    value
                }
            }
            _ => return None,
        };

        Some(ConstGenericValue {
            ty: norm,
            kind: ConstGenericValueKind::Int(coerced),
        })
    }

    pub fn supports_const_generic_value_type(&self, id: TypeId) -> bool {
        let norm = self.normalize(id);
        self.is_integer(norm) || norm == TypeId::BOOL
    }

    /// Normalize a type by following aliases to their final target.
    pub fn normalize(&self, mut id: TypeId) -> TypeId {
        loop {
            match self.get(id) {
                TypeKind::Alias(_, target) => {
                    id = *target;
                }
                _ => return id,
            }
        }
    }

    /// Return whether a type is an integer after normalization.
    pub fn is_integer(&self, id: TypeId) -> bool {
        match self.get(self.normalize(id)) {
            TypeKind::Primitive(p) => matches!(
                p,
                PrimitiveType::I8
                    | PrimitiveType::I16
                    | PrimitiveType::I32
                    | PrimitiveType::I64
                    | PrimitiveType::I128
                    | PrimitiveType::ISize
                    | PrimitiveType::U8
                    | PrimitiveType::U16
                    | PrimitiveType::U32
                    | PrimitiveType::U64
                    | PrimitiveType::U128
                    | PrimitiveType::USize
            ),
            _ => false,
        }
    }

    pub fn is_float(&self, id: TypeId) -> bool {
        match self.get(self.normalize(id)) {
            TypeKind::Primitive(p) => matches!(p, PrimitiveType::F32 | PrimitiveType::F64),
            _ => false,
        }
    }

    pub fn is_simd(&self, id: TypeId) -> bool {
        matches!(self.get(self.normalize(id)), TypeKind::Simd { .. })
    }

    pub fn simd_info(&self, id: TypeId) -> Option<(TypeId, u16)> {
        match self.get(self.normalize(id)) {
            TypeKind::Simd { elem, lanes } => Some((*elem, *lanes)),
            _ => None,
        }
    }

    pub fn is_simd_mask(&self, id: TypeId) -> bool {
        matches!(self.simd_info(id), Some((TypeId::BOOL, _)))
    }

    /// Return whether the normalized type carries mutable reference semantics.
    pub fn is_mut_reference(&self, id: TypeId) -> bool {
        match self.get(self.normalize(id)) {
            TypeKind::Pointer { is_mut, .. }
            | TypeKind::VolatilePtr { is_mut, .. }
            | TypeKind::Slice { is_mut, .. } => *is_mut,
            _ => false,
        }
    }

    pub fn get_elem_type(&self, id: TypeId) -> Option<TypeId> {
        match self.get(self.normalize(id)) {
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::Array { elem, .. }
            | TypeKind::ArrayInfer { elem, .. }
            | TypeKind::Simd { elem, .. } => Some(*elem),
            _ => None,
        }
    }

    pub fn is_closure_interface(&self, id: TypeId) -> bool {
        matches!(
            self.get(self.normalize(id)),
            TypeKind::ClosureInterface { .. }
        )
    }

    /// Return whether the normalized type is exactly `void`.
    pub fn is_void(&self, id: TypeId) -> bool {
        let norm = self.normalize(id);
        norm == TypeId::VOID
    }

    /// Return whether the normalized type is any raw pointer.
    pub fn is_any_pointer(&self, id: TypeId) -> bool {
        matches!(self.get(self.normalize(id)), TypeKind::Pointer { .. })
    }

    /// Return whether the normalized type is `&void` or `&mut void`.
    pub fn is_pointer_to_void(&self, id: TypeId) -> bool {
        match self.get(self.normalize(id)) {
            TypeKind::Pointer { elem, .. } => self.is_void(*elem),
            _ => false,
        }
    }

    /// Return whether the normalized type is specifically `&mut void`.
    pub fn is_mut_pointer_to_void(&self, id: TypeId) -> bool {
        match self.get(self.normalize(id)) {
            TypeKind::Pointer { is_mut: true, elem } => self.is_void(*elem),
            _ => false,
        }
    }
}
