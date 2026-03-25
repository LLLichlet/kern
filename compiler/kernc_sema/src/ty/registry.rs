use super::{PrimitiveType, TypeId, TypeKind};
use std::collections::HashMap;

/// 类型仓库
pub struct TypeRegistry {
    /// 存储具体的类型结构
    types: Vec<TypeKind>,

    /// 去重表：保证相同的类型 (如 *i32) 永远拥有相同的 TypeId
    interner: HashMap<TypeKind, TypeId>,
}

impl TypeRegistry {
    pub fn new() -> Self {
        let mut reg = Self {
            types: Vec::new(),
            interner: HashMap::new(),
        };
        reg.init_primitives();
        reg
    }

    fn init_primitives(&mut self) {
        // 顺序必须与 TypeId 中的常量对应
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

    /// 获取或创建类型的 ID
    /// 如果请求 *i32 且之前创建过，直接返回旧 ID
    pub fn intern(&mut self, kind: TypeKind) -> TypeId {
        if let Some(&id) = self.interner.get(&kind) {
            return id;
        }

        let id = TypeId(self.types.len() as u32);
        self.types.push(kind.clone());
        self.interner.insert(kind, id);
        id
    }

    /// 通过 ID 获取类型结构
    pub fn get(&self, id: TypeId) -> &TypeKind {
        &self.types[id.0 as usize]
    }

    /// 规范化类型 (穿透 Alias)
    /// 用于类型检查： check(A) == check(B) 应使用 normalize(A) == normalize(B)
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

    /// 判断是否是整数 (辅助函数)
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

    /// 检查一个类型是否是可变引用/指针
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
            | TypeKind::ArrayInfer { elem, .. } => Some(*elem),
            _ => None,
        }
    }

    pub fn is_closure_interface(&self, id: TypeId) -> bool {
        matches!(self.get(self.normalize(id)), TypeKind::ClosureInterface { .. })
    }
}
