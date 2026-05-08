use kernc_utils::FastHashMap;
use kernc_utils::{NodeId, Span, SymbolId};
use std::fmt;
use std::hash::{Hash, Hasher};

/// Identifier for a semantic definition collected from the AST.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DefId(pub u32);

/// Compact handle for an interned semantic type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub u32);

impl TypeId {
    // Reserve the lowest IDs for builtin primitive types.
    pub const VOID: Self = Self(0);
    pub const BOOL: Self = Self(1);
    pub const I8: Self = Self(2);
    pub const I16: Self = Self(3);
    pub const I32: Self = Self(4);
    pub const I64: Self = Self(5);
    pub const I128: Self = Self(6);
    pub const U8: Self = Self(7);
    pub const U16: Self = Self(8);
    pub const U32: Self = Self(9);
    pub const U64: Self = Self(10);
    pub const U128: Self = Self(11);
    pub const F32: Self = Self(12);
    pub const F64: Self = Self(13);
    pub const ISIZE: Self = Self(14);
    pub const USIZE: Self = Self(15);
    pub const NEVER: Self = Self(16);
    // Error placeholder used to suppress cascaded diagnostics.
    pub const ERROR: Self = Self(17);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConstGenericValueKind {
    Int(i128),
    Bool(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConstGenericValue {
    pub ty: TypeId,
    pub kind: ConstGenericValueKind,
}

impl ConstGenericValue {
    pub fn as_int(self) -> Option<i128> {
        match self.kind {
            ConstGenericValueKind::Int(value) => Some(value),
            ConstGenericValueKind::Bool(_) => None,
        }
    }

    pub fn as_bool(self) -> Option<bool> {
        match self.kind {
            ConstGenericValueKind::Bool(value) => Some(value),
            ConstGenericValueKind::Int(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConstExprId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConstExprUnaryOp {
    Negate,
    BitwiseNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConstExprBinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    BitwiseAnd,
    BitwiseOr,
    BitwiseXor,
    ShiftLeft,
    ShiftRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConstExprKind {
    Unary {
        op: ConstExprUnaryOp,
        expr: ConstGeneric,
        ty: TypeId,
    },
    Binary {
        op: ConstExprBinaryOp,
        lhs: ConstGeneric,
        rhs: ConstGeneric,
        ty: TypeId,
    },
    Cast {
        expr: ConstGeneric,
        ty: TypeId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConstGeneric {
    Value(ConstGenericValue),
    Param(SymbolId, TypeId),
    Expr(ConstExprId),
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GenericArg {
    Type(TypeId),
    Const(ConstGeneric),
}

impl GenericArg {
    pub fn as_type(self) -> Option<TypeId> {
        match self {
            Self::Type(ty) => Some(ty),
            Self::Const(_) => None,
        }
    }
}

pub fn wrap_type_arg(ty: TypeId) -> GenericArg {
    GenericArg::Type(ty)
}

pub fn wrap_type_args(args: impl IntoIterator<Item = TypeId>) -> Vec<GenericArg> {
    args.into_iter().map(GenericArg::Type).collect()
}

pub fn erase_non_type_generic_args(args: &[GenericArg]) -> Vec<TypeId> {
    args.iter()
        .map(|arg| arg.as_type().unwrap_or(TypeId::ERROR))
        .collect()
}

impl fmt::Display for ConstGeneric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(value) => write!(f, "{}", value),
            Self::Param(symbol, _) => write!(f, "{}", symbol.0),
            Self::Expr(id) => write!(f, "<const-expr:{}>", id.0),
            Self::Error => write!(f, "<const-error>"),
        }
    }
}

impl fmt::Display for ConstGenericValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ConstGenericValueKind::Int(value) => write!(f, "{}", value),
            ConstGenericValueKind::Bool(value) => write!(f, "{}", value),
        }
    }
}

/// Canonical semantic type representation.
/// Rich field and variant data lives in the definition tables; this enum stores shape and identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeKind {
    /// Primitive builtin type such as `i32`, `bool`, or `void`.
    Primitive(PrimitiveType),

    /// Fixed-width SIMD vector such as `f32x4` or `boolx8`.
    Simd {
        elem: TypeId,
        lanes: u16,
    },

    /// Raw pointer, `&T` or `&mut T`.
    Pointer {
        is_mut: bool,
        elem: TypeId,
    },

    /// Volatile pointer, `^T` or `^mut T`.
    VolatilePtr {
        is_mut: bool,
        elem: TypeId,
    },

    /// Fixed-size array, `[N]T`.
    Array {
        elem: TypeId,
        len: ConstGeneric,
    },

    /// Array whose length is inferred later, `[_]T`.
    ArrayInfer {
        elem: TypeId,
    },

    /// Slice type, `&[T]` or `&mut [T]`.
    Slice {
        is_mut: bool,
        elem: TypeId,
    },

    /// Reference to a named struct or union definition.
    /// Only the `DefId` is stored here so recursive types remain representable.
    Def(DefId, Vec<GenericArg>),

    /// Algebraic data type backed by an enum definition.
    Enum(DefId, Vec<GenericArg>),

    /// Physical payload union used by a lowered enum representation.
    EnumPayload(DefId, Vec<GenericArg>),

    /// Trait object fat pointer `{ data_ptr, vtable }`.
    TraitObject(DefId, Vec<GenericArg>, Vec<(DefId, TypeId)>),

    /// Associated type projection such as `T.Add[U].Out`.
    Projection {
        target: TypeId,
        trait_def_id: DefId,
        trait_args: Vec<GenericArg>,
        assoc_def_id: DefId,
        assoc_args: Vec<GenericArg>,
    },

    /// Closure call interface, `Fn(Args) Ret`.
    ClosureInterface {
        params: Vec<TypeId>,
        ret: TypeId,
    },

    /// Physical state structure that stores captured closure values.
    AnonymousState {
        closure_node_id: NodeId,
        captures: Vec<TypeId>,
        params: Vec<TypeId>,
        ret: TypeId,
    },

    /// Named type alias `type A = B`.
    Alias(SymbolId, TypeId),

    /// Generic parameter placeholder such as `T` in `impl[T]`.
    Param(SymbolId),

    /// Associated type placeholder or instantiation such as `Out` or `Out[T]`.
    Associated(DefId, Vec<GenericArg>),

    Function {
        params: Vec<TypeId>,
        ret: TypeId,
        is_variadic: bool,
    },

    /// Function item paired with its bound generic arguments.
    FnDef(DefId, Vec<GenericArg>),

    /// Unknown or invalid type.
    Error,

    /// Namespace marker used when a path resolves to a module.
    Module(DefId),

    // Local inference variable used by the unification engine.
    TypeVar(u32),

    /// Anonymous struct type compared by structural equivalence.
    /// Fields must be sorted by name before construction to keep hashing stable.
    AnonymousStruct(bool, Vec<AnonymousField>),

    /// Anonymous union type.
    AnonymousUnion(bool, Vec<AnonymousField>),

    /// Anonymous enum or algebraic data type.
    AnonymousEnum(AnonymousEnum),

    /// Payload union used by an anonymous enum layout.
    AnonymousEnumPayload(TypeId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrimitiveType {
    Void,
    Bool,
    I8,
    I16,
    I32,
    I64,
    I128,
    ISize,
    U8,
    U16,
    U32,
    U64,
    U128,
    USize,
    F32,
    F64,
    Never,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AnonymousField {
    pub name: SymbolId,
    pub ty: TypeId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AnonymousEnum {
    pub backing_ty: Option<TypeId>,
    pub builtin: Option<BuiltinAnonymousEnumKind>,
    pub variants: Vec<AnonymousVariant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinAnonymousEnumKind {
    Optional,
    Result,
}

impl AnonymousEnum {
    pub fn builtin_optional_payload(&self) -> Option<TypeId> {
        if self.builtin != Some(BuiltinAnonymousEnumKind::Optional) {
            return None;
        }

        self.variants
            .iter()
            .find(|variant| variant.payload_ty.is_some())
            .and_then(|variant| variant.payload_ty)
    }

    pub fn builtin_result_types(&self) -> Option<(TypeId, TypeId)> {
        if self.builtin != Some(BuiltinAnonymousEnumKind::Result) {
            return None;
        }

        let ok = self
            .variants
            .iter()
            .find(|variant| variant.payload_ty.is_some())
            .and_then(|variant| variant.payload_ty)?;
        let err = self
            .variants
            .iter()
            .rev()
            .find(|variant| variant.payload_ty.is_some())
            .and_then(|variant| variant.payload_ty)?;
        Some((ok, err))
    }
}

#[derive(Debug, Clone)]
pub struct AnonymousVariant {
    pub name: SymbolId,
    pub name_span: Span,
    pub payload_ty: Option<TypeId>,
    pub explicit_value: Option<i128>,
}

impl PartialEq for AnonymousVariant {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.payload_ty == other.payload_ty
            && self.explicit_value == other.explicit_value
    }
}

impl Eq for AnonymousVariant {}

impl Hash for AnonymousVariant {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.payload_ty.hash(state);
        self.explicit_value.hash(state);
    }
}

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

#[cfg(test)]
mod tests {
    use super::{AnonymousEnum, AnonymousVariant, TypeId, TypeKind, TypeRegistry};
    use kernc_utils::{FileId, Span, SymbolId};

    #[test]
    fn anonymous_enum_identity_ignores_variant_spans() {
        let mut registry = TypeRegistry::new();
        let variant_name = SymbolId(7);

        let first = registry.intern(TypeKind::AnonymousEnum(AnonymousEnum {
            backing_ty: Some(TypeId::U32),
            builtin: None,
            variants: vec![AnonymousVariant {
                name: variant_name,
                name_span: Span {
                    file: FileId(0),
                    start: 1,
                    end: 3,
                },
                payload_ty: Some(TypeId::I32),
                explicit_value: Some(9),
            }],
        }));

        let second = registry.intern(TypeKind::AnonymousEnum(AnonymousEnum {
            backing_ty: Some(TypeId::U32),
            builtin: None,
            variants: vec![AnonymousVariant {
                name: variant_name,
                name_span: Span {
                    file: FileId(1),
                    start: 40,
                    end: 44,
                },
                payload_ty: Some(TypeId::I32),
                explicit_value: Some(9),
            }],
        }));

        assert_eq!(first, second);
    }
}
