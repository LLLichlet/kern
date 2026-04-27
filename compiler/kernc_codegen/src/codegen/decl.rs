use super::CodeGenerator;
use crate::attributes::{Attribute, AttributeLoc};
use crate::module::Linkage;
use crate::types::{BasicTypeEnum, StructType};
use crate::values::{BasicValueEnum, GlobalValue};
use kernc_ast as ast;
use kernc_mir::{
    MirConst, MirFunction, MirGlobal, MirInlineHint, MirLinkage, MirStaticInit, MirStruct,
};
use kernc_mono::MonoId;
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::Span;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    fn pack_union_static_chunk(
        &mut self,
        value: BasicValueEnum<'ctx>,
        target_ty: crate::types::IntType<'ctx>,
    ) -> Option<BasicValueEnum<'ctx>> {
        match value {
            BasicValueEnum::IntValue(int_val) => {
                if int_val.get_type().bit_width() != target_ty.bit_width() {
                    return None;
                }
                Some(int_val.const_bitcast(target_ty).into())
            }
            _ => None,
        }
    }

    fn pack_union_static_storage_array(
        &mut self,
        array_ty: crate::types::ArrayType<'ctx>,
        value: BasicValueEnum<'ctx>,
    ) -> Option<BasicValueEnum<'ctx>> {
        let elem_ty = array_ty.get_element_type().into_int_type();
        let mut values = vec![elem_ty.const_zero(); array_ty.len() as usize];
        let first = self
            .pack_union_static_chunk(value, elem_ty)?
            .into_int_value();
        let Some(slot) = values.first_mut() else {
            return None;
        };
        *slot = first;
        Some(elem_ty.const_array(&values).into())
    }

    fn pack_union_static_value(
        &mut self,
        union_ty: StructType<'ctx>,
        value: BasicValueEnum<'ctx>,
    ) -> Option<crate::values::StructValue<'ctx>> {
        if union_ty.count_fields() != 1 {
            return None;
        }
        let field_ty = union_ty.get_field_type_at_index(0)?;
        let storage = if field_ty == value.get_type() {
            value
        } else {
            match field_ty {
                BasicTypeEnum::ArrayType(array_ty) => {
                    self.pack_union_static_storage_array(array_ty, value)?
                }
                _ => return None,
            }
        };
        Some(union_ty.const_named_struct(&[storage]))
    }

    fn has_meta_item_attr(&self, attributes: &[ast::MetaItem], expected: &str) -> bool {
        attributes.iter().any(|attribute| match attribute {
            ast::MetaItem::Call(id, _) | ast::MetaItem::Marker(id) => {
                self.resolve_symbol(*id) == expected
            }
        })
    }

    fn union_storage_type(
        &mut self,
        size: usize,
        align: usize,
        span: Span,
        name: &str,
    ) -> BasicTypeEnum<'ctx> {
        let size = size.max(1);
        let align = align.max(1);

        let (chunk_ty, chunk_size): (BasicTypeEnum<'ctx>, usize) = match align {
            1 => (self.context.i8_type().into(), 1),
            2 => (self.context.i16_type().into(), 2),
            4 => (self.context.i32_type().into(), 4),
            8 => (
                self.context
                    .custom_width_int_type((self.sess.target.pointer_size * 8) as u32)
                    .into(),
                self.sess.target.pointer_size as usize,
            ),
            16 => (self.context.i128_type().into(), 16),
            _ => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): unsupported union alignment {} for `{}`.",
                        align, name
                    ),
                );
                return self.context.i8_type().array_type(size as u32).into();
            }
        };

        if !size.is_multiple_of(chunk_size) {
            self.sess.emit_ice(
                span,
                format!(
                    "Kern ICE (Codegen): union `{}` has size {} not divisible by alignment chunk {}.",
                    name, size, chunk_size
                ),
            );
            return self.context.i8_type().array_type(size as u32).into();
        }

        chunk_ty.array_type((size / chunk_size) as u32).into()
    }

    pub(crate) fn compile_mir_static_init(
        &mut self,
        init: &MirStaticInit,
    ) -> Option<BasicValueEnum<'ctx>> {
        match init {
            MirStaticInit::Const(value) => self.compile_mir_static_const(value),
            MirStaticInit::Array { ty, elems } => {
                let array_ty = self.get_llvm_type(*ty).into_array_type();
                let elem_ty = self
                    .type_registry
                    .get_elem_type(*ty)
                    .map(|elem| self.get_llvm_type(elem));
                let elem_consts = elems
                    .iter()
                    .map(|elem| self.compile_mir_static_init(elem))
                    .collect::<Option<Vec<_>>>()?;

                match elem_ty {
                    Some(BasicTypeEnum::IntType(int_ty)) => Some(
                        int_ty
                            .const_array(
                                &elem_consts
                                    .iter()
                                    .map(|value| value.into_int_value())
                                    .collect::<Vec<_>>(),
                            )
                            .into(),
                    ),
                    Some(BasicTypeEnum::FloatType(float_ty)) => Some(
                        float_ty
                            .const_array(
                                &elem_consts
                                    .iter()
                                    .map(|value| value.into_float_value())
                                    .collect::<Vec<_>>(),
                            )
                            .into(),
                    ),
                    Some(BasicTypeEnum::PointerType(ptr_ty)) => Some(
                        ptr_ty
                            .const_array(
                                &elem_consts
                                    .iter()
                                    .map(|value| value.into_pointer_value())
                                    .collect::<Vec<_>>(),
                            )
                            .into(),
                    ),
                    Some(BasicTypeEnum::StructType(struct_ty)) => Some(
                        struct_ty
                            .const_array(
                                &elem_consts
                                    .iter()
                                    .map(|value| value.into_struct_value())
                                    .collect::<Vec<_>>(),
                            )
                            .into(),
                    ),
                    Some(BasicTypeEnum::ArrayType(nested_array_ty)) => Some(
                        nested_array_ty
                            .const_array(
                                &elem_consts
                                    .iter()
                                    .map(|value| value.into_array_value())
                                    .collect::<Vec<_>>(),
                            )
                            .into(),
                    ),
                    _ => Some(array_ty.const_zero().into()),
                }
            }
            MirStaticInit::FatPointer { ty, data_ptr, meta } => {
                let struct_ty = self.get_llvm_type(*ty).into_struct_type();
                let data_ptr_const = self.compile_mir_static_init(data_ptr)?;
                let meta_const = self.compile_mir_static_init(meta)?;
                Some(
                    struct_ty
                        .const_named_struct(&[data_ptr_const, meta_const])
                        .into(),
                )
            }
            MirStaticInit::Struct {
                struct_id, fields, ..
            } => {
                let struct_ty = *self.structs.get(struct_id)?;
                let field_consts = fields
                    .iter()
                    .map(|field| self.compile_mir_static_init(field))
                    .collect::<Option<Vec<_>>>()?;
                Some(struct_ty.const_named_struct(&field_consts).into())
            }
            MirStaticInit::Union {
                union_id,
                field_idx: _,
                value,
                ..
            } => {
                let union_ty = *self.structs.get(union_id)?;
                let value_const = self.compile_mir_static_init(value)?;
                Some(
                    self.pack_union_static_value(union_ty, value_const)
                        .unwrap_or_else(|| union_ty.const_zero())
                        .into(),
                )
            }
            MirStaticInit::Data {
                data_struct_id,
                tag_value,
                payload,
                ..
            } => {
                let struct_ty = *self.structs.get(data_struct_id)?;
                let tag_ty = struct_ty.get_field_type_at_index(0)?.into_int_type();
                let tag_val = tag_ty.const_u128(*tag_value);

                let union_ty = struct_ty.get_field_type_at_index(1)?.into_struct_type();
                let union_val = if let Some(payload) = payload {
                    let payload_const = self.compile_mir_static_init(payload)?;
                    self.pack_union_static_value(union_ty, payload_const)
                        .unwrap_or_else(|| union_ty.const_zero())
                } else {
                    union_ty.const_zero()
                };

                Some(
                    struct_ty
                        .const_named_struct(&[tag_val.into(), union_val.into()])
                        .into(),
                )
            }
        }
    }

    fn compile_mir_static_const(&mut self, value: &MirConst) -> Option<BasicValueEnum<'ctx>> {
        match value {
            MirConst::Undef { ty } => Some(self.get_llvm_type(*ty).const_zero()),
            MirConst::Integer { ty, value } => {
                let llvm_ty = self.get_llvm_type(*ty);
                if llvm_ty.is_pointer_type() {
                    let pointer_ty = llvm_ty.into_pointer_type();
                    if *value == 0 {
                        Some(pointer_ty.const_null().into())
                    } else {
                        let int_ty = self
                            .context
                            .custom_width_int_type((self.sess.target.pointer_size * 8) as u32);
                        Some(
                            pointer_ty
                                .const_int_to_ptr(int_ty.const_u128(*value))
                                .into(),
                        )
                    }
                } else {
                    Some(llvm_ty.into_int_type().const_u128(*value).into())
                }
            }
            MirConst::Float { ty, value } => Some(
                self.get_llvm_type(*ty)
                    .into_float_type()
                    .const_float(*value)
                    .into(),
            ),
            MirConst::Bool { value } => Some(
                self.context
                    .bool_type()
                    .const_int(u64::from(*value), false)
                    .into(),
            ),
            MirConst::StringLiteral { value, .. } => {
                Some(self.context.const_string(value.as_bytes(), true).into())
            }
            MirConst::GlobalRef { id, .. } => self
                .globals
                .get(id)
                .map(|global| global.as_pointer_value().into()),
            MirConst::FuncRef { id, .. } => self
                .functions
                .get(id)
                .map(|func| func.as_global_value().as_pointer_value().into()),
        }
    }

    pub(crate) fn lookup_declared_global(
        &mut self,
        global_id: MonoId,
        span: Span,
        name: &str,
    ) -> Option<GlobalValue<'ctx>> {
        match self.globals.get(&global_id).copied() {
            Some(global) => Some(global),
            None => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): global `{}` was declared but missing from LLVM globals map.",
                        name
                    ),
                );
                None
            }
        }
    }

    fn lookup_declared_struct(
        &mut self,
        struct_id: MonoId,
        span: Span,
        name: &str,
    ) -> Option<StructType<'ctx>> {
        match self.structs.get(&struct_id).copied() {
            Some(st) => Some(st),
            None => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): struct `{}` disappeared during declaration lowering.",
                        name
                    ),
                );
                None
            }
        }
    }

    pub(crate) fn declare_mir_structs(&mut self, structs: &[MirStruct]) {
        for s in structs {
            let llvm_struct = self.context.opaque_struct_type(&s.name);
            self.structs.insert(s.id, llvm_struct);
            self.struct_fields
                .insert(s.id, s.fields.iter().map(|field| field.name).collect());
            if s.is_union {
                self.union_ids.insert(s.id);
            }
        }

        for s in structs {
            let Some(llvm_struct) =
                self.lookup_declared_struct(s.id, kernc_utils::Span::default(), &s.name)
            else {
                continue;
            };

            let is_packed = s.attributes.iter().any(|attr| {
                matches!(attr, ast::MetaItem::Marker(id) if self.resolve_symbol(*id) == "packed")
            });

            if s.is_union {
                let storage_ty =
                    self.union_storage_type(s.union_size, s.union_align, Span::default(), &s.name);
                llvm_struct.set_body(&[storage_ty], is_packed);
            } else {
                let mut field_types = Vec::new();
                for field in &s.fields {
                    field_types.push(self.get_llvm_type(field.ty));
                }
                llvm_struct.set_body(&field_types, is_packed);
            }
        }
    }

    pub(crate) fn declare_mir_globals(&mut self, globals: &[MirGlobal]) {
        for g in globals {
            let mut llvm_symbol_name = g.name.clone();
            let mut link_section = None;
            let mut align_bytes = None;
            let mut has_export_name = false;

            for attr in &g.attributes {
                if let ast::MetaItem::Call(id, expr) = attr {
                    let name_str = self.resolve_symbol(*id);
                    if name_str == "export_name" {
                        has_export_name = true;
                        if let ast::ExprKind::String(s) = &expr.kind {
                            llvm_symbol_name = s.clone();
                        }
                    } else if name_str == "link_section" {
                        if let ast::ExprKind::String(s) = &expr.kind {
                            link_section = Some(s.clone());
                        }
                    } else if name_str == "align"
                        && let ast::ExprKind::Integer(val) = &expr.kind
                    {
                        align_bytes = Some(*val as u32);
                    }
                }
            }

            if g.is_extern
                && let Some(existing_global) = self.module.get_global(&llvm_symbol_name)
            {
                self.globals.insert(g.id, existing_global);
                continue;
            }

            let llvm_ty = self.get_llvm_type(g.ty);
            let global_val = self.module.add_global(llvm_ty, None, &llvm_symbol_name);

            let is_binding_mut = g.is_mut;
            let is_memory_mut = self.requires_mutable_memory(g.ty);
            global_val.set_constant(!(is_binding_mut || is_memory_mut));

            match g.linkage {
                MirLinkage::External => global_val.set_linkage(Linkage::External),
                MirLinkage::LinkOnceOdr => {
                    if cfg!(windows) {
                        global_val.set_linkage(Linkage::Internal);
                    } else {
                        global_val.set_linkage(Linkage::WeakOdr);
                    }
                }
                MirLinkage::Internal => global_val.set_linkage(Linkage::Internal),
            }

            if !g.is_extern {
                global_val.set_initializer(&llvm_ty.const_zero());
            }

            if let Some(sec) = link_section.or_else(|| {
                (!has_export_name)
                    .then(|| {
                        self.gc_data_section_for_symbol(
                            &llvm_symbol_name,
                            !(is_binding_mut || is_memory_mut),
                        )
                    })
                    .flatten()
            }) {
                global_val.set_section(Some(&sec));
            }
            if let Some(align) = align_bytes {
                global_val.set_alignment(align);
            }

            if !g.is_extern && self.has_meta_item_attr(&g.attributes, "retain") {
                self.retained_globals.push(global_val.as_pointer_value());
            }
            self.globals.insert(g.id, global_val);
        }
    }

    pub(crate) fn declare_mir_functions(&mut self, functions: &[MirFunction]) {
        for f in functions {
            let ret_ty = self.get_llvm_type(f.ret_ty);

            let mut param_types = Vec::new();
            for p in &f.params {
                param_types.push(self.get_llvm_type(p.ty));
            }

            let fn_type = if f.ret_ty == TypeId::VOID {
                self.context
                    .void_type()
                    .fn_type(&param_types, f.is_variadic)
            } else {
                match ret_ty {
                    BasicTypeEnum::IntType(i) => i.fn_type(&param_types, f.is_variadic),
                    BasicTypeEnum::FloatType(fl) => fl.fn_type(&param_types, f.is_variadic),
                    BasicTypeEnum::PointerType(p) => p.fn_type(&param_types, f.is_variadic),
                    BasicTypeEnum::StructType(s) => s.fn_type(&param_types, f.is_variadic),
                    BasicTypeEnum::ArrayType(a) => a.fn_type(&param_types, f.is_variadic),
                    BasicTypeEnum::VectorType(v) => v.fn_type(&param_types, f.is_variadic),
                    BasicTypeEnum::ScalableVectorType(v) => v.fn_type(&param_types, f.is_variadic),
                }
            };

            let mut llvm_symbol_name = f.name.clone();
            let mut is_cold = false;
            let mut is_naked = false;
            let inline_kind = match f.inline_hint {
                MirInlineHint::None => None,
                MirInlineHint::Inline => Some("alwaysinline"),
                MirInlineHint::NoInline => Some("noinline"),
            };
            let mut link_section = None;
            let mut has_export_name = false;
            let mut target_features = Vec::new();

            for attr in &f.attributes {
                match attr {
                    ast::MetaItem::Call(id, expr) => {
                        let name_str = self.resolve_symbol(*id);
                        if name_str == "export_name" {
                            has_export_name = true;
                            if let ast::ExprKind::String(s) = &expr.kind {
                                llvm_symbol_name = s.clone();
                            }
                        } else if name_str == "link_section"
                            && let ast::ExprKind::String(s) = &expr.kind
                        {
                            link_section = Some(s.clone());
                        } else if name_str == "target_feature"
                            && let ast::ExprKind::String(spec) = &expr.kind
                        {
                            for feature in spec.split(',').map(str::trim).filter(|s| !s.is_empty())
                            {
                                if feature.starts_with('+') || feature.starts_with('-') {
                                    target_features.push(feature.to_string());
                                } else {
                                    target_features.push(format!("+{}", feature));
                                }
                            }
                        }
                    }
                    ast::MetaItem::Marker(id) => {
                        let name_str = self.resolve_symbol(*id);
                        if name_str == "cold" {
                            is_cold = true;
                        } else if name_str == "naked" {
                            is_naked = true;
                        }
                    }
                }
            }

            if f.is_extern
                && let Some(existing_func) = self.module.get_function(&llvm_symbol_name)
            {
                self.functions.insert(f.id, existing_func);
                continue;
            }

            let llvm_func = self.module.add_function(&llvm_symbol_name, fn_type, None);
            match f.linkage {
                MirLinkage::External => llvm_func.as_global_value().set_linkage(Linkage::External),
                MirLinkage::LinkOnceOdr => {
                    if cfg!(windows) {
                        llvm_func.as_global_value().set_linkage(Linkage::Internal);
                    } else {
                        llvm_func.as_global_value().set_linkage(Linkage::WeakOdr);
                    }
                }
                MirLinkage::Internal => llvm_func.as_global_value().set_linkage(Linkage::Internal),
            }

            if is_cold {
                let kind_id = Attribute::get_named_enum_kind_id("cold");
                let cold_attr = self.context.create_enum_attribute(kind_id, 0);
                llvm_func.add_attribute(AttributeLoc::Function, cold_attr);
            }
            if is_naked {
                let kind_id = Attribute::get_named_enum_kind_id("naked");
                let naked_attr = self.context.create_enum_attribute(kind_id, 0);
                llvm_func.add_attribute(AttributeLoc::Function, naked_attr);

                let noinline_kind_id = Attribute::get_named_enum_kind_id("noinline");
                let noinline_attr = self.context.create_enum_attribute(noinline_kind_id, 0);
                llvm_func.add_attribute(AttributeLoc::Function, noinline_attr);
            }
            if let Some(attr_name) = inline_kind
                && !(is_naked && attr_name == "alwaysinline")
            {
                let kind_id = Attribute::get_named_enum_kind_id(attr_name);
                let inline_attr = self.context.create_enum_attribute(kind_id, 0);
                llvm_func.add_attribute(AttributeLoc::Function, inline_attr);
            }
            if !target_features.is_empty() {
                let features = target_features.join(",");
                let attr = self
                    .context
                    .create_string_attribute("target-features", &features);
                llvm_func.add_attribute(AttributeLoc::Function, attr);
            }
            if let Some(sec) = link_section.or_else(|| {
                (!has_export_name)
                    .then(|| self.gc_text_section_for_symbol(&llvm_symbol_name))
                    .flatten()
            }) {
                llvm_func.as_global_value().set_section(Some(&sec));
            }

            if !f.is_extern && self.has_meta_item_attr(&f.attributes, "retain") {
                self.retained_globals
                    .push(llvm_func.as_global_value().as_pointer_value());
            }

            if f.body.is_some() {
                self.attach_debug_info_to_function(f, llvm_func);
            }

            self.functions.insert(f.id, llvm_func);
        }
    }

    /// Detect whether a type requires writable physical storage.
    fn requires_mutable_memory(&self, ty: TypeId) -> bool {
        let norm_ty = self.type_registry.normalize(ty);
        match self.type_registry.get(norm_ty).clone() {
            // Arrays are value aggregates. Writable storage depends on the access path, not the
            // array type itself, so global allocation requirements do not come from `[N]T`.
            TypeKind::Array { .. } => false,
            // Mutable slices and pointers also require writable storage when materialized globally.
            TypeKind::Slice { is_mut, .. } => is_mut,
            TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => is_mut,
            _ => false,
        }
    }
}
