use super::CodeGenerator;
use inkwell::types::{BasicType, BasicTypeEnum};
use kernc_ast as ast;
use kernc_mast::{MastExpr, MastExprKind, MastFunction, MastGlobal, MastStruct};
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::Span;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
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

        if size % chunk_size != 0 {
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

    fn compile_const_expr(
        &mut self,
        expr: &MastExpr,
    ) -> Option<inkwell::values::BasicValueEnum<'ctx>> {
        match &expr.kind {
            MastExprKind::Integer(val) => {
                let int_type = self.get_llvm_type(expr.ty).into_int_type();
                Some(int_type.const_int(*val as u64, false).into())
            }
            MastExprKind::Float(val) => {
                let float_type = self.get_llvm_type(expr.ty).into_float_type();
                Some(float_type.const_float(*val).into())
            }
            MastExprKind::Bool(val) => Some(
                self.context
                    .bool_type()
                    .const_int(if *val { 1 } else { 0 }, false)
                    .into(),
            ),
            MastExprKind::StringLiteral(s) => {
                Some(self.context.const_string(s.as_bytes(), false).into())
            }
            MastExprKind::ArrayInit(elems) => {
                let array_ty = self.get_llvm_type(expr.ty).into_array_type();
                let elem_ty = self
                    .type_registry
                    .get_elem_type(expr.ty)
                    .map(|ty| self.get_llvm_type(ty));
                let elem_consts: Vec<_> = elems
                    .iter()
                    .filter_map(|elem| self.compile_const_expr(elem))
                    .collect();
                if elem_consts.len() != elems.len() {
                    return None;
                }

                match elem_ty {
                    Some(BasicTypeEnum::IntType(int_ty)) => Some(
                        int_ty
                            .const_array(
                                &elem_consts
                                    .iter()
                                    .map(|v| v.into_int_value())
                                    .collect::<Vec<_>>(),
                            )
                            .into(),
                    ),
                    Some(BasicTypeEnum::FloatType(float_ty)) => Some(
                        float_ty
                            .const_array(
                                &elem_consts
                                    .iter()
                                    .map(|v| v.into_float_value())
                                    .collect::<Vec<_>>(),
                            )
                            .into(),
                    ),
                    Some(BasicTypeEnum::PointerType(ptr_ty)) => Some(
                        ptr_ty
                            .const_array(
                                &elem_consts
                                    .iter()
                                    .map(|v| v.into_pointer_value())
                                    .collect::<Vec<_>>(),
                            )
                            .into(),
                    ),
                    Some(BasicTypeEnum::StructType(struct_ty)) => Some(
                        struct_ty
                            .const_array(
                                &elem_consts
                                    .iter()
                                    .map(|v| v.into_struct_value())
                                    .collect::<Vec<_>>(),
                            )
                            .into(),
                    ),
                    Some(BasicTypeEnum::ArrayType(nested_array_ty)) => Some(
                        nested_array_ty
                            .const_array(
                                &elem_consts
                                    .iter()
                                    .map(|v| v.into_array_value())
                                    .collect::<Vec<_>>(),
                            )
                            .into(),
                    ),
                    _ => Some(array_ty.const_zero().into()),
                }
            }
            MastExprKind::StructInit { struct_id, fields } => {
                let struct_ty = *self.structs.get(struct_id)?;
                let field_consts: Vec<_> = fields
                    .iter()
                    .filter_map(|field| self.compile_const_expr(field))
                    .collect();
                if field_consts.len() != fields.len() {
                    return None;
                }
                Some(struct_ty.const_named_struct(&field_consts).into())
            }
            MastExprKind::UnionInit {
                union_id, value, ..
            } => {
                let union_ty = *self.structs.get(union_id)?;
                let value_const = self.compile_const_expr(value)?;
                if union_ty.count_fields() == 1
                    && union_ty.get_field_type_at_index(0) == Some(value_const.get_type())
                {
                    Some(union_ty.const_named_struct(&[value_const]).into())
                } else {
                    Some(union_ty.const_zero().into())
                }
            }
            MastExprKind::DataInit {
                data_struct_id,
                tag_value,
                payload,
            } => {
                let struct_ty = *self.structs.get(data_struct_id)?;
                let tag_ty = struct_ty.get_field_type_at_index(0)?.into_int_type();
                let tag_val = tag_ty.const_int(*tag_value as u64, false);

                let union_ty = struct_ty.get_field_type_at_index(1)?.into_struct_type();
                let union_val = if payload.ty == TypeId::VOID || payload.ty == TypeId::ERROR {
                    union_ty.const_zero()
                } else {
                    let payload_const = self.compile_const_expr(payload)?;
                    if union_ty.count_fields() == 1
                        && union_ty.get_field_type_at_index(0) == Some(payload_const.get_type())
                    {
                        union_ty.const_named_struct(&[payload_const])
                    } else {
                        union_ty.const_zero()
                    }
                };

                Some(
                    struct_ty
                        .const_named_struct(&[tag_val.into(), union_val.into()])
                        .into(),
                )
            }
            MastExprKind::FuncRef(mono_id) => self
                .functions
                .get(mono_id)
                .map(|func| func.as_global_value().as_pointer_value().into()),
            MastExprKind::GlobalRef(mono_id) => self
                .globals
                .get(mono_id)
                .map(|global| global.as_pointer_value().into()),
            MastExprKind::Undef => Some(self.get_llvm_type(expr.ty).const_zero()),
            _ => None,
        }
    }

    fn lookup_declared_global(
        &mut self,
        global_id: kernc_mast::MonoId,
        span: Span,
        name: &str,
    ) -> Option<inkwell::values::GlobalValue<'ctx>> {
        match self.globals.get(&global_id).copied() {
            Some(global) => Some(global),
            None => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): global `{}` is in MAST but missing from LLVM globals map.",
                        name
                    ),
                );
                None
            }
        }
    }

    fn lookup_declared_struct(
        &mut self,
        struct_id: kernc_mast::MonoId,
        span: Span,
        name: &str,
    ) -> Option<inkwell::types::StructType<'ctx>> {
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

    pub(crate) fn compile_global(&mut self, global: &MastGlobal) {
        if global.is_extern {
            return;
        }
        let Some(global_val) =
            self.lookup_declared_global(global.id, kernc_utils::Span::default(), &global.name)
        else {
            return;
        };

        if let Some(init) = &global.init {
            let const_val = self
                .compile_const_expr(init)
                .unwrap_or_else(|| self.get_llvm_type(global.ty).const_zero());

            global_val.set_initializer(&const_val);
        } else if !global.is_extern {
            let llvm_ty = self.get_llvm_type(global.ty);
            global_val.set_initializer(&llvm_ty.const_zero());
        }
    }

    pub(crate) fn declare_structs(&mut self, structs: &[MastStruct]) {
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
                let storage_ty = self.union_storage_type(
                    s.union_size,
                    s.union_align,
                    Span::default(),
                    &s.name,
                );
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

    pub(crate) fn declare_globals(&mut self, globals: &[MastGlobal]) {
        for g in globals {
            let mut llvm_symbol_name = g.name.clone();
            let mut link_section = None;
            let mut align_bytes = None;

            for attr in &g.attributes {
                if let ast::MetaItem::Call(id, expr) = attr {
                    let name_str = self.resolve_symbol(*id);
                    if name_str == "export_name" {
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

            // 只有当 "绑定本身不可变" 且 "物理类型也没有要求可变内存" 时，才能作为 LLVM 物理常量
            let is_binding_mut = g.is_mut;
            let is_memory_mut = self.requires_mutable_memory(g.ty);
            global_val.set_constant(!(is_binding_mut || is_memory_mut));

            if g.is_extern {
                global_val.set_linkage(inkwell::module::Linkage::External);
            } else {
                global_val.set_initializer(&llvm_ty.const_zero());
            }

            if let Some(sec) = link_section {
                global_val.set_section(Some(&sec));
            }
            if let Some(align) = align_bytes {
                global_val.set_alignment(align);
            }

            self.globals.insert(g.id, global_val);
        }
    }

    pub(crate) fn declare_functions(&mut self, functions: &[MastFunction]) {
        for f in functions {
            let ret_ty = self.get_llvm_type(f.ret_ty);

            let mut param_types = Vec::new();
            for p in &f.params {
                param_types.push(self.get_llvm_type(p.ty).into());
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
                    _ => {
                        self.sess.emit_ice(
                            Span::default(),
                            format!("Invalid LLVM return type for function {}", f.name),
                        );
                        self.context
                            .void_type()
                            .fn_type(&param_types, f.is_variadic)
                    }
                }
            };

            let mut llvm_symbol_name = f.name.clone();
            let mut is_cold = false;
            let mut is_naked = false;
            let mut link_section = None;

            for attr in &f.attributes {
                match attr {
                    ast::MetaItem::Call(id, expr) => {
                        let name_str = self.resolve_symbol(*id);
                        if name_str == "export_name" {
                            if let ast::ExprKind::String(s) = &expr.kind {
                                llvm_symbol_name = s.clone();
                            }
                        } else if name_str == "link_section"
                            && let ast::ExprKind::String(s) = &expr.kind
                        {
                            link_section = Some(s.clone());
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

            if is_cold {
                let kind_id = inkwell::attributes::Attribute::get_named_enum_kind_id("cold");
                let cold_attr = self.context.create_enum_attribute(kind_id, 0);
                llvm_func.add_attribute(inkwell::attributes::AttributeLoc::Function, cold_attr);
            }
            if is_naked {
                let kind_id = inkwell::attributes::Attribute::get_named_enum_kind_id("naked");
                let naked_attr = self.context.create_enum_attribute(kind_id, 0);
                llvm_func.add_attribute(inkwell::attributes::AttributeLoc::Function, naked_attr);
            }
            if let Some(sec) = link_section {
                llvm_func.as_global_value().set_section(Some(&sec));
            }

            self.functions.insert(f.id, llvm_func);
        }
    }

    /// 探测一个类型是否在物理内存上要求可写 (内部可变性)
    fn requires_mutable_memory(&self, ty: TypeId) -> bool {
        let norm_ty = self.type_registry.normalize(ty);
        match self.type_registry.get(norm_ty).clone() {
            // 如果是数组，且明确标记了 is_mut，物理内存必须可写
            TypeKind::Array { is_mut, .. } => is_mut,
            // 如果是切片或指针本身作为全局变量被直接分配内存，且携带 mut，物理上也放行
            TypeKind::Slice { is_mut, .. } => is_mut,
            TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => is_mut,
            _ => false,
        }
    }
}
