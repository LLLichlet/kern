mod format;
mod layout;
mod registry;

pub use format::TypeFormatter;
pub use layout::LayoutEngine;
pub use registry::TypeRegistry;

use crate::def::DefId;
use kernc_utils::SymbolId;

/// 类型的唯一 ID (轻量级 Handle)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub u32);

impl TypeId {
    // 预留前 20 个 ID 给基础类型，方便快速访问
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
    // 字符串字面量类型 (只读切片)
    pub const STR: Self = Self(16);
    pub const NEVER: Self = Self(17);
    // 错误占位符 (防止级联报错)
    pub const ERROR: Self = Self(18);
}

/// 类型的具体结构
/// 注意：这里不包含 field/variant 的具体信息，只包含“形状”。
/// 具体定义存储在 Context 的 Decl 表中。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeKind {
    /// 基础类型 (i32, bool, void...)
    Primitive(PrimitiveType),

    /// 普通指针: *T 或 *mut T
    Pointer {
        is_mut: bool,
        elem: TypeId,
    },

    /// 易失指针: ^T 或 ^mut T
    VolatilePtr {
        is_mut: bool,
        elem: TypeId,
    },

    /// 数组: [N]T 或 [N]mut T
    Array {
        is_mut: bool,
        elem: TypeId,
        len: u64,
    },

    /// 长度待推导的数组 `[_]T` 或 `[_]mut T`
    ArrayInfer {
        is_mut: bool,
        elem: TypeId,
    },

    /// 切片: []T 或 []mut T
    Slice {
        is_mut: bool,
        elem: TypeId,
    },

    /// 引用具体的定义 (Struct/Union)
    /// 这里只存 ID。具体字段信息去 Context 里查。
    /// 这样设计是为了处理递归类型 (e.g., struct Node { next: *Node })
    Def(DefId, Vec<TypeId>),

    /// 代数数据类型 (Enum，融合 Enum 和 ADT)
    Enum(DefId, Vec<TypeId>),

    /// 专门用于表示 Enum 在底层的物理 Union 负载 (Tag 之后的部分)
    EnumPayload(DefId, Vec<TypeId>),

    /// 特征对象 (Trait Object)
    /// 内存布局：胖指针 { data_ptr: *mut void, vtable: *mut VTable }
    TraitObject(DefId, Vec<TypeId>),

    /// 类型别名: type A = B;
    /// 记录了 "A" 这个名字，以及它指向的 "B"
    Alias(SymbolId, TypeId),

    /// 泛型参数占位符 (impl[T] 中的 T)
    /// 在单态化之前，它只是一个名字。
    Param(SymbolId),

    Function {
        params: Vec<TypeId>,
        ret: TypeId,
        is_variadic: bool,
    },

    /// 具体的函数定义项 (Function Item)
    /// 带有其 DefId 和已绑定的泛型实参。例如 `ArrayList.new[i32]`
    FnDef(DefId, Vec<TypeId>),

    /// 未知/错误类型
    Error,

    /// 专门用于表示这是一个模块（Namespace），防止与普通值混淆
    Module(DefId),

    // 类型变量，用于 let a = 10; 的局部推导 (Hindley-Milner 合一引擎使用)
    TypeVar(u32),
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
    Str, // 内部使用的字符串字面量类型
    Never,
}
