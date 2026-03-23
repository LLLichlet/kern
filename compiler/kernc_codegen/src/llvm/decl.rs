use super::CodeGenerator;
use inkwell::AddressSpace;
use inkwell::types::BasicTypeEnum;
use kernc_ast as ast;
use kernc_mast::{MastExprKind, MastFunction, MastGlobal, MastStruct};
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::Span;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub fn compile_global(&mut self, global: &MastGlobal) {
        if global.is_extern {
            return;
        }
        let global_val = match self.globals.get(&global.id) {
            Some(val) => *val,
            None => {
                self.sess.emit_ice(
                    kernc_utils::Span::default(), 
                    format!("Kern ICE (Codegen): Global `{}` is in MAST but missing from LLVM globals map. `declare_globals` pass failed to register it!", global.name)
                );
                unreachable!()
            }
        };

        if let Some(init) = &global.init {
            let const_val: inkwell::values::BasicValueEnum<'ctx> = match &init.kind {
                MastExprKind::Integer(val) => {
                    let int_type = self.get_llvm_type(init.ty).into_int_type();
                    int_type.const_int(*val as u64, false).into()
                }
                MastExprKind::Float(val) => {
                    let float_type = self.get_llvm_type(init.ty).into_float_type();
                    float_type.const_float(*val).into()
                }
                MastExprKind::Bool(val) => self
                    .context
                    .bool_type()
                    .const_int(if *val { 1 } else { 0 }, false)
                    .into(),
                MastExprKind::StringLiteral(s) => {
                    let bytes = self.context.const_string(s.as_bytes(), false);
                    bytes.into()
                }
                MastExprKind::ArrayInit(elems) => {
                    let mut ptr_vals = Vec::new();
                    for e in elems {
                        if let MastExprKind::FuncRef(mono_id) = e.kind {
                            if let Some(func_val) = self.functions.get(&mono_id) {
                                ptr_vals.push(func_val.as_global_value().as_pointer_value());
                            } else {
                                self.sess.emit_ice(e.span, "Function reference in array init not found in LLVM functions map".to_string());
                                unreachable!()
                            }
                        } else {
                            ptr_vals.push(self.context.ptr_type(AddressSpace::default()).const_null());
                        }
                    }
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    ptr_ty.const_array(&ptr_vals).into()
                }
                _ => self.get_llvm_type(global.ty).const_zero(),
            };

            global_val.set_initializer(&const_val);
        } else if !global.is_extern {
            let llvm_ty = self.get_llvm_type(global.ty);
            global_val.set_initializer(&llvm_ty.const_zero());
        }
    }

    pub fn declare_structs(&mut self, structs: &[MastStruct]) {
        for s in structs {
            let llvm_struct = self.context.opaque_struct_type(&s.name);
            self.structs.insert(s.id, llvm_struct);
            if s.is_union {
                self.union_ids.insert(s.id);
            }
        }

        for s in structs {
            let llvm_struct = match self.structs.get(&s.id) {
                Some(st) => *st,  
                None => {
                    self.sess.emit_ice(kernc_utils::Span::default(), format!("Struct {} disappeared during opaque body filling", s.name));
                    unreachable!()
                }
            };

            let is_packed = s.attributes.iter().any(|attr| {
                matches!(attr, ast::MetaItem::Marker(id) if self.resolve_symbol(*id) == "packed")
            });

            if s.is_union {
                let target_ty = self.get_llvm_type(s.fields[s.largest_field_idx].ty);
                llvm_struct.set_body(&[target_ty], is_packed);
            } else {
                let mut field_types = Vec::new();
                for field in &s.fields {
                    field_types.push(self.get_llvm_type(field.ty));
                }
                llvm_struct.set_body(&field_types, is_packed);
            }
        }
    }

    pub fn declare_globals(&mut self, globals: &[MastGlobal]) {
        for g in globals {
            let mut llvm_symbol_name = g.name.clone();
            let mut link_section = None;
            let mut align_bytes = None;

            for attr in &g.attributes {
                match attr {
                    ast::MetaItem::Call(id, expr) => {
                        let name_str = self.resolve_symbol(*id);
                        if name_str == "export_name" {
                            if let ast::ExprKind::String(s) = &expr.kind {
                                llvm_symbol_name = s.clone();
                            }
                        } else if name_str == "link_section" {
                            if let ast::ExprKind::String(s) = &expr.kind {
                                link_section = Some(s.clone());
                            }
                        } else if name_str == "align" {
                            if let ast::ExprKind::Integer(val) = &expr.kind {
                                align_bytes = Some(*val as u32);
                            }
                        }
                    }
                    _ => {}
                }
            }

            if g.is_extern {
                if let Some(existing_global) = self.module.get_global(&llvm_symbol_name) {
                    self.globals.insert(g.id, existing_global);
                    continue;
                }
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

    pub fn declare_functions(&mut self, functions: &[MastFunction]) {
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
                        self.sess.emit_ice(Span::default(), format!("Invalid LLVM return type for function {}", f.name));
                        unreachable!()
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
                        } else if name_str == "link_section" {
                            if let ast::ExprKind::String(s) = &expr.kind {
                                link_section = Some(s.clone());
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

            if f.is_extern {
                if let Some(existing_func) = self.module.get_function(&llvm_symbol_name) {
                    self.functions.insert(f.id, existing_func);
                    continue;
                }
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
