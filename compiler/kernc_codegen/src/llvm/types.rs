use super::CodeGenerator;
use inkwell::AddressSpace;
use inkwell::types::{BasicType, BasicTypeEnum};
use inkwell::values::BasicValueEnum;
use kernc_sema::ty::{PrimitiveType, TypeId, TypeKind};
use kernc_utils::Span;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub fn get_llvm_type(&mut self, ty: TypeId) -> BasicTypeEnum<'ctx> {
        let norm = self.type_registry.normalize(ty);

        match self.type_registry.get(norm).clone() {
            TypeKind::Primitive(p) => match p {
                PrimitiveType::I8 | PrimitiveType::U8 => self.context.i8_type().into(),
                PrimitiveType::I16 | PrimitiveType::U16 => self.context.i16_type().into(),
                PrimitiveType::I32 | PrimitiveType::U32 => self.context.i32_type().into(),
                PrimitiveType::I64
                | PrimitiveType::U64
                | PrimitiveType::ISize
                | PrimitiveType::USize => {
                    let ptr_bits = self.sess.target.pointer_size as u32 * 8;
                    self.context.custom_width_int_type(ptr_bits).into()
                }
                PrimitiveType::I128 | PrimitiveType::U128 => self.context.i128_type().into(),
                PrimitiveType::F32 => self.context.f32_type().into(),
                PrimitiveType::F64 => self.context.f64_type().into(),
                PrimitiveType::Bool => self.context.bool_type().into(),
                PrimitiveType::Str => self.context.ptr_type(AddressSpace::default()).into(),
                PrimitiveType::Void | PrimitiveType::Never => self.context.i8_type().into(),
            },
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let elem_norm = self.type_registry.normalize(elem);
                // 特判：指向 Trait Object 或 ClosureInterface 的指针，物理布局是一个胖指针结构体
                if matches!(
                    self.type_registry.get(elem_norm), 
                    TypeKind::TraitObject(..) | TypeKind::ClosureInterface { .. }
                ) {
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let meta_ty = self.context.i64_type(); // 虚表指针 / 代码指针 统一用 i64 (usize)
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
                let key = (def_id, args.clone());
                if let Some(&mono_id) = self.def_mono_map.get(&key) {
                    if let Some(struct_ty) = self.structs.get(&mono_id) {
                        return struct_ty.as_basic_type_enum();
                    }
                }
                
                self.sess.emit_ice(
                    Span::default(),
                    format!("Kern ICE (Codegen): DefId {} not instantiated by Lowerer", def_id.0),
                );
                unreachable!()
            }
            TypeKind::EnumPayload(def_id, args) => {
                let key = (def_id, args.clone());
                if let Some(&wrapper_mono_id) = self.def_mono_map.get(&key) {
                    if let Some(&payload_mono_id) = self.adt_union_map.get(&wrapper_mono_id) {
                        if let Some(struct_ty) = self.structs.get(&payload_mono_id) {
                            return struct_ty.as_basic_type_enum();
                        }
                    }
                }
                
                self.sess.emit_ice(
                    Span::default(),
                    format!("Kern ICE (Codegen): EnumPayload for DefId {} not instantiated", def_id.0),
                );
                unreachable!()
            }
            TypeKind::AnonymousState { captures, .. } => {
                let mut field_tys = Vec::new();
                for cap in captures {
                    field_tys.push(self.get_llvm_type(cap));
                }
                self.context.struct_type(&field_tys, false).into()
            }
            TypeKind::ClosureInterface { .. } => {
                self.sess.emit_ice(
                    Span::default(),
                    "Kern ICE (Codegen): Naked `ClosureInterface` cannot be materialized. \
                     Sema `ensure_sized` failed to catch this. You must use a fat pointer (e.g., `*Fn`)."
                );
                unreachable!()
            }

            TypeKind::TypeVar(vid) => {
                self.sess.emit_ice(
                    Span::default(),
                    format!("Unresolved TypeVar `?T{}` leaked into LLVM Codegen! Semantic Analyzer missed it.", vid)
                );
                unreachable!()
            }
            _ => {
                self.sess.emit_ice(
                    Span::default(),
                    format!(
                        "Frontend failed to resolve type! TypeId: {:?}, Kind: {:?}",
                        norm,
                        self.type_registry.get(norm)
                    ),
                );
                unreachable!()
            }
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
