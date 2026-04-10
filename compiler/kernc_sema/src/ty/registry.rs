use super::{PrimitiveType, TypeId, TypeKind};
use kernc_utils::FastHashMap;

/// Interning table for semantic types.
#[derive(Clone)]
pub struct TypeRegistry {
    /// Dense storage for type structures.
    types: Vec<TypeKind>,

    /// Deduplication map that guarantees identical types share one `TypeId`.
    interner: FastHashMap<TypeKind, TypeId>,
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
        self.add_primitive(PrimitiveType::Str); // 16
        self.add_primitive(PrimitiveType::Never); // 17

        // 18: Error
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
            | TypeKind::Slice { is_mut, .. }
            | TypeKind::Array { is_mut, .. }
            | TypeKind::ArrayInfer { is_mut, .. } => *is_mut,
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

    /// Return whether the normalized type is `*void` or `*mut void`.
    pub fn is_pointer_to_void(&self, id: TypeId) -> bool {
        match self.get(self.normalize(id)) {
            TypeKind::Pointer { elem, .. } => self.is_void(*elem),
            _ => false,
        }
    }

    /// Return whether the normalized type is specifically `*mut void`.
    pub fn is_mut_pointer_to_void(&self, id: TypeId) -> bool {
        match self.get(self.normalize(id)) {
            TypeKind::Pointer { is_mut: true, elem } => self.is_void(*elem),
            _ => false,
        }
    }
}
