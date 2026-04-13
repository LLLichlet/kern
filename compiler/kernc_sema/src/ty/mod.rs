mod format;
mod layout;
mod registry;

pub(crate) use format::TypeFormatter;
pub use layout::LayoutEngine;
pub use registry::TypeRegistry;

use crate::def::DefId;
use kernc_utils::{NodeId, Span, SymbolId};
use std::hash::{Hash, Hasher};

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
    // String literal type, represented as an immutable slice.
    pub const STR: Self = Self(16);
    pub const NEVER: Self = Self(17);
    // Error placeholder used to suppress cascaded diagnostics.
    pub const ERROR: Self = Self(18);
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

    /// Raw pointer, `*T` or `*mut T`.
    Pointer {
        is_mut: bool,
        elem: TypeId,
    },

    /// Volatile pointer, `^T` or `^mut T`.
    VolatilePtr {
        is_mut: bool,
        elem: TypeId,
    },

    /// Fixed-size array, `[N]T` or `[N]mut T`.
    Array {
        is_mut: bool,
        elem: TypeId,
        len: u64,
    },

    /// Array whose length is inferred later, `[_]T`.
    ArrayInfer {
        is_mut: bool,
        elem: TypeId,
    },

    /// Slice type, `[]T` or `[]mut T`.
    Slice {
        is_mut: bool,
        elem: TypeId,
    },

    /// Reference to a named struct or union definition.
    /// Only the `DefId` is stored here so recursive types remain representable.
    Def(DefId, Vec<TypeId>),

    /// Algebraic data type backed by an enum definition.
    Enum(DefId, Vec<TypeId>),

    /// Physical payload union used by a lowered enum representation.
    EnumPayload(DefId, Vec<TypeId>),

    /// Trait object fat pointer `{ data_ptr, vtable }`.
    TraitObject(DefId, Vec<TypeId>),

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

    Function {
        params: Vec<TypeId>,
        ret: TypeId,
        is_variadic: bool,
    },

    /// Function item paired with its bound generic arguments.
    FnDef(DefId, Vec<TypeId>),

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
    Str, // Internal string-literal primitive.
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
