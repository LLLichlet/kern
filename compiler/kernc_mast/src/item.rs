use crate::{MastBlock, MastExpr, MonoId};
use kernc_ast::MetaItem;
use kernc_sema::def::DefId;
use kernc_sema::ty::TypeId;
use kernc_utils::SymbolId;
use std::collections::HashMap;

/// MAST 模块 (编译单元的最终扁平化表示)
/// 一切都被平铺，没有嵌套模块，没有 Impl 块，没有泛型。
#[derive(Debug, Clone)]
pub struct MastModule {
    pub name: String,
    pub structs: Vec<MastStruct>,
    pub globals: Vec<MastGlobal>, // 所有 static (含全局和局部) 都被提升到这里
    pub functions: Vec<MastFunction>,
    // 记录前端抽象实体到后端物理实体的映射
    pub def_mono_map: HashMap<(DefId, Vec<TypeId>), MonoId>,
    pub pure_enum_tag_map: HashMap<(DefId, Vec<TypeId>), TypeId>,
    pub adt_union_map: HashMap<MonoId, MonoId>,
    pub anon_struct_map: HashMap<TypeId, MonoId>,
    pub anon_union_map: HashMap<TypeId, MonoId>,
    pub anon_enum_map: HashMap<TypeId, MonoId>,
}

#[derive(Debug, Clone)]
pub struct MastStruct {
    pub id: MonoId,
    pub name: String, // 扁平化后的全限定名，例如 "std_collections_ArrayList_i32"
    pub fields: Vec<MastField>,
    pub is_extern: bool, // 用于对接 C 的 struct
    pub is_union: bool,
    pub largest_field_idx: usize,
    pub attributes: Vec<MetaItem>,
}

#[derive(Debug, Clone)]
pub struct MastField {
    pub name: SymbolId,
    pub ty: TypeId, // 保证是绝对具体的类型，绝不含 Param
}

#[derive(Debug, Clone)]
pub struct MastGlobal {
    pub id: MonoId,
    pub name: String, // 扁平化的全局符号名
    pub ty: TypeId,
    pub is_mut: bool,           // 对应 static mut
    pub init: Option<MastExpr>, // extern 的时候为 None。初始化必须是常量表达式。
    pub is_extern: bool,
    pub attributes: Vec<MetaItem>,
}

#[derive(Debug, Clone)]
pub struct MastFunction {
    pub id: MonoId,
    pub name: String, // 例如 "Point_i32_move_by" (方法被扁平化为普通函数)
    pub params: Vec<MastParam>,
    pub ret_ty: TypeId,
    pub body: Option<MastBlock>, // extern 时为 None
    pub is_extern: bool,
    pub is_variadic: bool,
    pub attributes: Vec<MetaItem>,
}

#[derive(Debug, Clone)]
pub struct MastParam {
    pub name: SymbolId,
    pub ty: TypeId,
    pub is_mut: bool,
}
