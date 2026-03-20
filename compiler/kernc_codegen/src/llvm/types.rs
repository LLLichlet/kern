// compiler/kernc_codegen/src/types.rs

use super::CodeGenerator;
use inkwell::AddressSpace;
use inkwell::types::{BasicType, BasicTypeEnum};
use inkwell::values::BasicValueEnum;
use kernc_mast::MonoId;
use kernc_sema::ty::{PrimitiveType, TypeId, TypeKind};

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub fn get_llvm_type(&self, ty: TypeId) -> BasicTypeEnum<'ctx> {
        let norm = self.type_registry.normalize(ty);

        match self.type_registry.get(norm).clone() {
            TypeKind::Primitive(p) => match p {
                PrimitiveType::I8 | PrimitiveType::U8 => self.context.i8_type().into(),
                PrimitiveType::I16 | PrimitiveType::U16 => self.context.i16_type().into(),
                PrimitiveType::I32 | PrimitiveType::U32 => self.context.i32_type().into(),
                PrimitiveType::I64
                | PrimitiveType::U64
                | PrimitiveType::ISize
                | PrimitiveType::USize => self.context.i64_type().into(),
                PrimitiveType::I128 | PrimitiveType::U128 => self.context.i128_type().into(),
                PrimitiveType::F32 => self.context.f32_type().into(),
                PrimitiveType::F64 => self.context.f64_type().into(),
                PrimitiveType::Bool => self.context.bool_type().into(),
                PrimitiveType::Str => self.context.ptr_type(AddressSpace::default()).into(),
                PrimitiveType::Void | PrimitiveType::Never => self.context.i8_type().into(),
            },
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let elem_norm = self.type_registry.normalize(elem);
                // 特判：指向 Trait Object 的指针，其物理布局是一个包含两个字段的结构体
                if matches!(self.type_registry.get(elem_norm), TypeKind::TraitObject(..)) {
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let meta_ty = self.context.i64_type(); // 虚表指针/元数据 统一用 i64 (usize) 存储
                    return self
                        .context
                        .struct_type(&[ptr_ty.into(), meta_ty.into()], false)
                        .into();
                }

                // 普通指针，正常降级为单指针
                self.context.ptr_type(AddressSpace::default()).into()
            }
            TypeKind::Function { .. } | TypeKind::FnDef(..) => {
                self.context.ptr_type(AddressSpace::default()).into()
            }

            TypeKind::Array { elem, len, .. } => {
                let elem_ty = self.get_llvm_type(elem);
                elem_ty.array_type(len as u32).into()
            }

            TypeKind::TraitObject(_, _) | TypeKind::Slice { .. } => {
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let len_ty = self.context.i64_type();
                self.context
                    .struct_type(&[ptr_ty.into(), len_ty.into()], false)
                    .into()
            }
            TypeKind::Def(def_id, args) | TypeKind::Enum(def_id, args) => {
                if def_id.0 as usize >= self.ctx_defs.len() {
                    return self
                        .structs
                        .get(&MonoId(def_id.0))
                        .unwrap()
                        .as_basic_type_enum();
                }

                let def = &self.ctx_defs[def_id.0 as usize];
                let mut mangled_name = (self.ctx_resolve)(def.name().unwrap()).to_string();
                for arg in args {
                    mangled_name.push_str(&format!("_{}", arg.0));
                }

                if let Some(struct_ty) = self.module.get_struct_type(&mangled_name) {
                    struct_ty.into()
                } else {
                    self.context.i8_type().into()
                }
            }
            TypeKind::EnumPayload(def_id, args) => {
                let def = &self.ctx_defs[def_id.0 as usize];
                let mut mangled_name = (self.ctx_resolve)(def.name().unwrap()).to_string();
                for arg in args {
                    mangled_name.push_str(&format!("_{}", arg.0));
                }
                mangled_name.push_str("_payload");

                if let Some(struct_ty) = self.module.get_struct_type(&mangled_name) {
                    struct_ty.into()
                } else {
                    self.context.i8_type().into()
                }
            }
            TypeKind::TypeVar(vid) => {
                panic!(
                    "Kern ICE: Unresolved TypeVar `?T{}` leaked into LLVM Codegen! Sema missed it.",
                    vid
                );
            }
            _ => unreachable!(
                "Frontend failed to resolve type! TypeId: {:?}, Kind: {:?}",
                norm,
                self.type_registry.get(norm)
            ),
        }
    }

    /// 辅助函数：绕过 Inkwell BasicTypeEnum 没有统一 get_undef() 的限制
    pub(crate) fn get_undef_val(&self, llvm_ty: BasicTypeEnum<'ctx>) -> BasicValueEnum<'ctx> {
        match llvm_ty {
            BasicTypeEnum::ArrayType(t) => t.get_undef().into(),
            BasicTypeEnum::FloatType(t) => t.get_undef().into(),
            BasicTypeEnum::IntType(t) => t.get_undef().into(),
            BasicTypeEnum::PointerType(t) => t.get_undef().into(),
            BasicTypeEnum::StructType(t) => t.get_undef().into(),
            BasicTypeEnum::VectorType(t) => t.get_undef().into(),
            BasicTypeEnum::ScalableVectorType(t) => t.get_undef().into(),
        }
    }

    /// 判断当前类型是否在物理上是 Void
    pub(crate) fn is_void_type(&self, ty: TypeId) -> bool {
        let norm = self.type_registry.normalize(ty);
        matches!(
            self.type_registry.get(norm),
            TypeKind::Primitive(PrimitiveType::Void)
        )
    }
}
