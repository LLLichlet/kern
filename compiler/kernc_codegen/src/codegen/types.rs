use super::CodeGenerator;
use crate::AddressSpace;
use crate::types::BasicTypeEnum;
use crate::values::BasicValueEnum;
use kernc_mono::MonoId;
use kernc_sema::ty::{PrimitiveType, TypeId, TypeKind};
use kernc_utils::Span;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub(crate) fn llvm_integer_storage_type(&mut self, ty: TypeId) -> Option<BasicTypeEnum<'ctx>> {
        let norm = self.type_registry.normalize(ty);
        if self.type_registry.is_integer(norm) || norm == TypeId::BOOL {
            return Some(self.get_llvm_type(norm));
        }

        match self.type_registry.get(norm).clone() {
            TypeKind::Enum(def_id, args) => {
                let key = (def_id, args);
                if let Some(&tag_ty) = self.pure_enum_tag_map.get(&key) {
                    return Some(self.get_llvm_type(tag_ty));
                }
            }
            TypeKind::AnonymousEnum(enum_def)
                if enum_def
                    .variants
                    .iter()
                    .all(|variant| variant.payload_ty.is_none()) =>
            {
                return Some(self.get_llvm_type(enum_def.backing_ty.unwrap_or(TypeId::U32)));
            }
            _ => {}
        }

        None
    }

    pub(crate) fn const_generic_usize(
        &mut self,
        value: kernc_sema::ty::ConstGeneric,
        span: Span,
    ) -> Option<u64> {
        match value {
            kernc_sema::ty::ConstGeneric::Value(value) if value.ty == TypeId::USIZE => {
                u64::try_from(value.as_int()?).ok()
            }
            kernc_sema::ty::ConstGeneric::Value(_) | kernc_sema::ty::ConstGeneric::Error => None,
            kernc_sema::ty::ConstGeneric::Param(symbol, _) => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): unresolved const generic `{}` reached code generation.",
                        symbol.0
                    ),
                );
                None
            }
            kernc_sema::ty::ConstGeneric::Expr(expr_id) => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): unresolved const expression `{:?}` reached code generation.",
                        self.type_registry.const_expr(expr_id)
                    ),
                );
                None
            }
        }
    }

    fn invalid_llvm_type(&mut self, span: Span, msg: impl Into<String>) -> BasicTypeEnum<'ctx> {
        self.sess.emit_ice(span, msg);
        self.context.struct_type(&[], false).into()
    }

    fn lookup_instantiated_struct(
        &mut self,
        mono_id: MonoId,
        span: Span,
        context: &str,
    ) -> Option<BasicTypeEnum<'ctx>> {
        match self.structs.get(&mono_id).copied() {
            Some(struct_ty) => Some(struct_ty.as_basic_type_enum()),
            None => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): missing instantiated struct MonoId {:?} for {}.",
                        mono_id, context
                    ),
                );
                None
            }
        }
    }

    pub(crate) fn get_llvm_type(&mut self, ty: TypeId) -> BasicTypeEnum<'ctx> {
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
                PrimitiveType::Void | PrimitiveType::Never => {
                    self.context.struct_type(&[], false).into()
                }
            },
            TypeKind::Simd { elem, lanes } => {
                let lane_ty = self.get_llvm_type(elem);
                lane_ty.vector_type(lanes as u32).into()
            }
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let elem_norm = self.type_registry.normalize(elem);
                // Special-case pointers to trait objects or closure interfaces: they lower as fat-pointer structs.
                if matches!(
                    self.type_registry.get(elem_norm),
                    TypeKind::TraitObject(..) | TypeKind::ClosureInterface { .. }
                ) {
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    let meta_ty = self.context.i64_type(); // Vtable and code pointers are represented as `usize`.
                    return self
                        .context
                        .struct_type(&[ptr_ty.into(), meta_ty.into()], false)
                        .into();
                }

                // Ordinary pointers lower to a single raw pointer.
                self.context.ptr_type(AddressSpace::default()).into()
            }
            TypeKind::Function { .. } | TypeKind::FnDef(..) => {
                self.context.ptr_type(AddressSpace::default()).into()
            }

            TypeKind::Array { elem, len, .. } => {
                let elem_ty = self.get_llvm_type(elem);
                let Some(len) = self.const_generic_usize(len, Span::default()) else {
                    return self.invalid_llvm_type(
                        Span::default(),
                        "Kern ICE (Codegen): array length was not a concrete `usize` during LLVM type construction.",
                    );
                };
                elem_ty.array_type(len as u32).into()
            }

            TypeKind::TraitObject(..) | TypeKind::Slice { .. } => {
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let len_ty = self.context.i64_type();
                self.context
                    .struct_type(&[ptr_ty.into(), len_ty.into()], false)
                    .into()
            }
            TypeKind::Range { .. } => {
                if let Some(&mono_id) = self.range_map.get(&norm)
                    && let Some(struct_ty) =
                        self.lookup_instantiated_struct(mono_id, Span::default(), "range")
                {
                    return struct_ty;
                }

                self.invalid_llvm_type(
                    Span::default(),
                    format!(
                        "Kern ICE (Codegen): Range TypeId({:?}) not instantiated by Lowerer",
                        norm
                    ),
                )
            }
            TypeKind::Def(def_id, args) => {
                let key = (def_id, args.clone());
                if let Some(&mono_id) = self.def_mono_map.get(&key)
                    && let Some(struct_ty) =
                        self.lookup_instantiated_struct(mono_id, Span::default(), "named data type")
                {
                    return struct_ty;
                }

                self.invalid_llvm_type(
                    Span::default(),
                    format!(
                        "Kern ICE (Codegen): DefId {} not instantiated by Lowerer",
                        def_id.0
                    ),
                )
            }
            TypeKind::Enum(def_id, args) => {
                let key = (def_id, args.clone());
                if let Some(&tag_ty) = self.pure_enum_tag_map.get(&key) {
                    return self.get_llvm_type(tag_ty);
                }
                if let Some(&mono_id) = self.def_mono_map.get(&key)
                    && let Some(struct_ty) =
                        self.lookup_instantiated_struct(mono_id, Span::default(), "named enum")
                {
                    return struct_ty;
                }

                self.invalid_llvm_type(
                    Span::default(),
                    format!(
                        "Kern ICE (Codegen): Enum DefId {} was not instantiated or recorded as a pure enum.",
                        def_id.0
                    ),
                )
            }
            TypeKind::EnumPayload(def_id, args) => {
                let key = (def_id, args.clone());
                if let Some(&wrapper_mono_id) = self.def_mono_map.get(&key)
                    && let Some(&payload_mono_id) = self.adt_union_map.get(&wrapper_mono_id)
                    && let Some(struct_ty) = self.lookup_instantiated_struct(
                        payload_mono_id,
                        Span::default(),
                        "enum payload",
                    )
                {
                    return struct_ty;
                }

                self.invalid_llvm_type(
                    Span::default(),
                    format!(
                        "Kern ICE (Codegen): EnumPayload for DefId {} not instantiated",
                        def_id.0
                    ),
                )
            }
            TypeKind::AnonymousStruct(..) => {
                if let Some(&mono_id) = self.anon_struct_map.get(&norm)
                    && let Some(struct_ty) = self.lookup_instantiated_struct(
                        mono_id,
                        Span::default(),
                        "anonymous struct",
                    )
                {
                    return struct_ty;
                }

                self.invalid_llvm_type(
                    Span::default(),
                    format!("Kern ICE (Codegen): AnonymousStruct TypeId({:?}) not instantiated by Lowerer", norm),
                )
            }
            TypeKind::AnonymousUnion(..) => {
                if let Some(&mono_id) = self.anon_union_map.get(&norm)
                    && let Some(struct_ty) = self.lookup_instantiated_struct(
                        mono_id,
                        Span::default(),
                        "anonymous union",
                    )
                {
                    return struct_ty;
                }

                self.invalid_llvm_type(
                    Span::default(),
                    format!("Kern ICE (Codegen): AnonymousUnion TypeId({:?}) not instantiated by Lowerer", norm),
                )
            }
            TypeKind::AnonymousEnum(enum_def)
                if enum_def
                    .variants
                    .iter()
                    .all(|variant| variant.payload_ty.is_none()) =>
            {
                self.get_llvm_type(enum_def.backing_ty.unwrap_or(TypeId::U32))
            }
            TypeKind::AnonymousEnum(..) => {
                if let Some(&mono_id) = self.anon_enum_map.get(&norm)
                    && let Some(struct_ty) = self.lookup_instantiated_struct(
                        mono_id,
                        Span::default(),
                        "anonymous enum",
                    )
                {
                    return struct_ty;
                }

                self.invalid_llvm_type(
                    Span::default(),
                    format!("Kern ICE (Codegen): AnonymousEnum TypeId({:?}) not instantiated by Lowerer", norm),
                )
            }
            TypeKind::AnonymousEnumPayload(enum_ty) => {
                let enum_ty = self.type_registry.normalize(enum_ty);
                if let Some(&wrapper_mono_id) = self.anon_enum_map.get(&enum_ty)
                    && let Some(&payload_mono_id) = self.adt_union_map.get(&wrapper_mono_id)
                    && let Some(struct_ty) = self.lookup_instantiated_struct(
                        payload_mono_id,
                        Span::default(),
                        "anonymous enum payload",
                    )
                {
                    return struct_ty;
                }

                self.invalid_llvm_type(
                    Span::default(),
                    format!(
                        "Kern ICE (Codegen): AnonymousEnumPayload for TypeId({:?}) not instantiated",
                        enum_ty
                    ),
                )
            }
            TypeKind::AnonymousState { captures, .. } => {
                let mut field_tys = Vec::new();
                for cap in captures {
                    field_tys.push(self.get_llvm_type(cap));
                }
                self.context.struct_type(&field_tys, false).into()
            }
            TypeKind::ClosureInterface { .. } => {
                self.invalid_llvm_type(
                    Span::default(),
                    "Kern ICE (Codegen): Naked `ClosureInterface` cannot be materialized. \
                     Sema `ensure_sized` failed to catch this. You must use a fat pointer (e.g., `&Fn`)."
                )
            }

            TypeKind::TypeVar(vid) => {
                self.invalid_llvm_type(
                    Span::default(),
                    format!("Unresolved TypeVar `?T{}` leaked into LLVM Codegen! Semantic Analyzer missed it.", vid)
                )
            }
            _ => {
                self.invalid_llvm_type(
                    Span::default(),
                    format!(
                        "Frontend failed to resolve type! TypeId: {:?}, Kind: {:?}",
                        norm,
                        self.type_registry.get(norm)
                    ),
                )
            }
        }
    }

    /// Return an `undef` value for any LLVM basic type.
    pub(crate) fn get_undef_val(&self, llvm_ty: BasicTypeEnum<'ctx>) -> BasicValueEnum<'ctx> {
        match llvm_ty {
            BasicTypeEnum::ArrayType(t) => t.get_undef().into(),
            BasicTypeEnum::FloatType(t) => t.get_undef().into(),
            BasicTypeEnum::IntType(t) => t.get_undef().into(),
            BasicTypeEnum::PointerType(t) => t.get_undef().into(),
            BasicTypeEnum::StructType(t) => t.get_undef().into(),
            BasicTypeEnum::VectorType(t) => t.get_undef(),
            BasicTypeEnum::ScalableVectorType(t) => t.get_undef(),
        }
    }

    /// Return whether the normalized type is physically `void`.
    pub(crate) fn is_void_type(&self, ty: TypeId) -> bool {
        let norm = self.type_registry.normalize(ty);
        matches!(
            self.type_registry.get(norm),
            TypeKind::Primitive(PrimitiveType::Void)
        )
    }
}
