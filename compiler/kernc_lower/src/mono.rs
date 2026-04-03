use super::Lowerer;
use kernc_ast as ast;
use kernc_mast::*;
use kernc_sema::LayoutEngine;
use kernc_sema::checker::{ConstEvaluator, ConstValue, Substituter};
use kernc_sema::def::{Def, DefId, GlobalDef};
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{Span, SymbolId};
use std::collections::HashMap;

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    fn placeholder_function(&mut self, id: MonoId, name: String) {
        if self.module.functions.iter().any(|func| func.id == id) {
            return;
        }

        self.module.functions.push(MastFunction {
            id,
            name,
            linkage: MastLinkage::Internal,
            params: vec![],
            ret_ty: TypeId::VOID,
            body: Some(MastBlock {
                stmts: vec![MastStmt::Expr(MastExpr::new(
                    TypeId::VOID,
                    MastExprKind::Trap,
                    Span::default(),
                ))],
                result: None,
                defers: vec![],
            }),
            is_extern: false,
            is_variadic: false,
            attributes: vec![],
        });
    }

    fn placeholder_struct(&mut self, id: MonoId, name: String, is_union: bool) {
        if self.module.structs.iter().any(|strukt| strukt.id == id) {
            return;
        }

        self.module.structs.push(MastStruct {
            id,
            name,
            fields: vec![],
            is_extern: false,
            is_union,
            largest_field_idx: 0,
            union_size: if is_union { 1 } else { 0 },
            union_align: 1,
            attributes: vec![],
        });
    }

    fn placeholder_data_structs(
        &mut self,
        wrapper_id: MonoId,
        payload_union_id: MonoId,
        name: &str,
    ) {
        self.placeholder_struct(payload_union_id, format!("{}_payload", name), true);
        self.placeholder_struct(wrapper_id, name.to_string(), false);
    }

    fn build_generic_subst_map(
        &mut self,
        owner_kind: &str,
        owner_name: &str,
        params: &[ast::GenericParam],
        args: &[TypeId],
    ) -> Option<HashMap<SymbolId, TypeId>> {
        if params.len() != args.len() {
            self.ctx.emit_ice(
                Span::default(),
                format!(
                    "Kern ICE (Lowering): Generics mismatch for {} `{}`. Expected {}, got {}.",
                    owner_kind,
                    owner_name,
                    params.len(),
                    args.len()
                ),
            );
            return None;
        }

        let mut subst_map = HashMap::new();
        for (param, arg) in params.iter().zip(args.iter().copied()) {
            subst_map.insert(param.name, arg);
        }
        Some(subst_map)
    }

    pub(crate) fn instantiate_function(&mut self, def_id: DefId, args: &[TypeId]) -> MonoId {
        let key = (def_id, args.to_vec());
        if let Some(&id) = self.mono_cache.get(&key) {
            return id;
        }

        let id = self.new_mono_id();
        self.mono_cache.insert(key, id);

        let def = if let Def::Function(f) = &self.ctx.defs[def_id.0 as usize] {
            f.clone()
        } else {
            self.ctx.emit_ice(
                Span::default(),
                format!("Kern ICE (Lowering): DefId {} is not a Function!", def_id.0),
            );
            self.placeholder_function(id, format!("__ice_fn_{}", id.0));
            return id;
        };

        let fn_name = self.ctx.resolve(def.name).to_string();
        let Some(subst_map) =
            self.build_generic_subst_map("function", &fn_name, &def.generics, args)
        else {
            self.placeholder_function(id, format!("__ice_fn_{}", id.0));
            return id;
        };

        let mangled_name = self.ctx.get_export_name(def_id, args);

        let raw_ret = def.resolved_sig.map_or(TypeId::VOID, |sig| {
            if let TypeKind::Function { ret, .. } = self.ctx.type_registry.get(sig) {
                *ret
            } else {
                TypeId::VOID
            }
        });

        let mut mast_params = Vec::new();
        for p in &def.params {
            let raw_ty = self
                .ctx
                .node_types
                .get(&p.type_node.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let conc_ty = {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
                subst.substitute(raw_ty)
            };
            self.track_pure_enum_repr_in_type(conc_ty);
            mast_params.push(MastParam {
                name: p.pattern.name,
                ty: conc_ty,
                is_mut: p.pattern.is_mut,
            });
        }

        let conc_ret = {
            let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
            subst.substitute(raw_ret)
        };
        self.track_pure_enum_repr_in_type(conc_ret);

        let saved_local_types = std::mem::take(&mut self.local_types);
        let saved_defer_stack = std::mem::take(&mut self.defer_stack);
        let saved_loop_frames = std::mem::take(&mut self.loop_frames);
        let saved_local_statics = std::mem::take(&mut self.local_statics);

        self.local_types.push(std::collections::HashMap::new());
        for p in &mast_params {
            if let Some(scope) = self.local_types.last_mut() {
                scope.insert(p.name, (p.ty, p.is_mut));
            } else {
                self.ctx.emit_ice(
                    Span::default(),
                    "Kern ICE (Lowering): Missing local type scope while instantiating a function.",
                );
                break;
            }
        }

        let body = if self.function_requires_runtime_body(&def) {
            let prev_scope = self.ctx.scopes.current_scope_id();
            if let Some(owner_scope) = self.function_owner_scope(&def) {
                self.ctx.scopes.set_current_scope(owner_scope);
            }

            let body = def
                .body
                .as_ref()
                .map(|body_expr| self.lower_block_as_body(body_expr, &subst_map, conc_ret));

            if let Some(prev_scope) = prev_scope {
                self.ctx.scopes.set_current_scope(prev_scope);
            }

            body
        } else {
            None
        };

        self.local_types.pop();

        self.local_types = saved_local_types;
        self.defer_stack = saved_defer_stack;
        self.loop_frames = saved_loop_frames;
        self.local_statics = saved_local_statics;

        let mast_fn = MastFunction {
            id,
            name: mangled_name,
            linkage: MastLinkage::External,
            params: mast_params,
            ret_ty: conc_ret,
            body,
            is_extern: def.is_extern,
            is_variadic: def.is_variadic,
            attributes: self.extract_meta_items(&def.attributes),
        };

        self.module.functions.push(mast_fn);
        id
    }

    pub(crate) fn instantiate_struct(&mut self, def_id: DefId, args: &[TypeId]) -> MonoId {
        let key = (def_id, args.to_vec());
        if let Some(&id) = self.mono_cache.get(&key) {
            return id;
        }

        let id = self.new_mono_id();
        self.mono_cache.insert(key, id);

        // Delegate union-like lowering to the dedicated path.
        if let Def::Union(_) = &self.ctx.defs[def_id.0 as usize] {
            return self.instantiate_union(def_id, args, id);
        }

        let def = if let Def::Struct(s) = &self.ctx.defs[def_id.0 as usize] {
            s.clone()
        } else {
            self.ctx.emit_ice(
                Span::default(),
                format!("Kern ICE (Lowering): DefId {} is not a Struct!", def_id.0),
            );
            self.placeholder_struct(id, format!("__ice_struct_{}", id.0), false);
            return id;
        };

        let mangled_name = self.ctx.get_export_name(def_id, args);
        let Some(subst_map) =
            self.build_generic_subst_map("struct", &mangled_name, &def.generics, args)
        else {
            self.placeholder_struct(id, format!("__ice_struct_{}", id.0), false);
            return id;
        };

        let physical_to_ast = {
            let mut layout = LayoutEngine::new(self.ctx);
            let (_, p2a) = layout.get_struct_mapping(def_id, args, 0);
            p2a
        };

        let mut mast_fields = Vec::with_capacity(def.fields.len());

        for &ast_idx in &physical_to_ast {
            let f = &def.fields[ast_idx];
            let raw_ty = self
                .ctx
                .node_types
                .get(&f.type_node.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let conc_ty = {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
                subst.substitute(raw_ty)
            };
            self.track_pure_enum_repr_in_type(conc_ty);
            mast_fields.push(MastField {
                name: f.name,
                ty: conc_ty,
            });
        }

        self.module.structs.push(MastStruct {
            id,
            name: mangled_name,
            fields: mast_fields,
            is_extern: def.is_extern,
            is_union: false,
            largest_field_idx: 0,
            union_size: 0,
            union_align: 1,
            attributes: self.extract_meta_items(&def.attributes),
        });

        id
    }

    pub(crate) fn instantiate_anon_struct(&mut self, norm_ty: TypeId) -> MonoId {
        if let Some(&id) = self.anon_struct_cache.get(&norm_ty) {
            return id;
        }

        let id = self.new_mono_id();
        self.anon_struct_cache.insert(norm_ty, id);

        let (is_extern, fields) = if let TypeKind::AnonymousStruct(ext, f) =
            self.ctx.type_registry.get(norm_ty).clone()
        {
            (ext, f)
        } else {
            self.ctx.emit_ice(
                Span::default(),
                format!(
                    "Kern ICE (Lowering): Expected AnonymousStruct, found {:?}",
                    self.ctx.type_registry.get(norm_ty)
                ),
            );
            self.placeholder_struct(id, format!("__ice_anon_struct_{}", id.0), false);
            return id;
        };

        let mut layout = LayoutEngine::new(self.ctx);
        let (_, physical_to_ast) = layout.get_anon_struct_mapping(is_extern, &fields, 0);

        let mut mast_fields = Vec::with_capacity(fields.len());

        for &ast_idx in &physical_to_ast {
            let f = &fields[ast_idx];
            self.track_pure_enum_repr_in_type(f.ty);
            mast_fields.push(MastField {
                name: f.name,
                ty: f.ty,
            });
        }

        let mangled_name = self.ctx.mangle_type(norm_ty);

        self.module.structs.push(MastStruct {
            id,
            name: mangled_name,
            fields: mast_fields,
            is_extern,
            is_union: false,
            largest_field_idx: 0,
            union_size: 0,
            union_align: 1,
            attributes: vec![],
        });

        id
    }

    pub(crate) fn instantiate_anon_union(&mut self, norm_ty: TypeId) -> MonoId {
        if let Some(&id) = self.anon_union_cache.get(&norm_ty) {
            return id;
        }

        let id = self.new_mono_id();
        self.anon_union_cache.insert(norm_ty, id);

        let (is_extern, fields) =
            if let TypeKind::AnonymousUnion(ext, f) = self.ctx.type_registry.get(norm_ty).clone() {
                (ext, f)
            } else {
                self.ctx.emit_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Lowering): Expected AnonymousUnion, found {:?}",
                        self.ctx.type_registry.get(norm_ty)
                    ),
                );
                self.placeholder_struct(id, format!("__ice_anon_union_{}", id.0), true);
                return id;
            };

        let mut mast_fields = Vec::new();
        let mut max_size = 0;
        let mut max_align = 1;
        let mut largest_field_idx = 0;

        for (idx, field) in fields.iter().enumerate() {
            self.track_pure_enum_repr_in_type(field.ty);
            mast_fields.push(MastField {
                name: field.name,
                ty: field.ty,
            });

            let mut layout = LayoutEngine::new(self.ctx);
            let size = layout.compute_type_size(field.ty);
            let align = layout.compute_type_align(field.ty);
            if size > max_size {
                max_size = size;
                largest_field_idx = idx;
            }
            max_align = max_align.max(align);
        }

        self.module.structs.push(MastStruct {
            id,
            name: self.ctx.mangle_type(norm_ty),
            fields: mast_fields,
            is_extern,
            is_union: true,
            largest_field_idx,
            union_size: max_size.max(1) as usize,
            union_align: max_align.max(1) as usize,
            attributes: vec![],
        });

        id
    }

    pub(crate) fn instantiate_anon_enum(&mut self, norm_ty: TypeId) -> MonoId {
        if let Some(&id) = self.anon_enum_cache.get(&norm_ty) {
            return id;
        }

        let wrapper_id = self.new_mono_id();
        let payload_union_id = self.new_mono_id();
        self.anon_enum_cache.insert(norm_ty, wrapper_id);
        self.adt_union_map.insert(wrapper_id, payload_union_id);

        let enum_def = if let TypeKind::AnonymousEnum(enum_def) =
            self.ctx.type_registry.get(norm_ty).clone()
        {
            enum_def
        } else {
            self.ctx.emit_ice(
                Span::default(),
                format!(
                    "Kern ICE (Lowering): Expected AnonymousEnum, found {:?}",
                    self.ctx.type_registry.get(norm_ty)
                ),
            );
            self.placeholder_data_structs(
                wrapper_id,
                payload_union_id,
                &format!("__ice_anon_enum_{}", wrapper_id.0),
            );
            return wrapper_id;
        };

        let mut union_fields = Vec::new();
        let mut largest_idx = 0;
        let mut max_size = 0;
        let mut max_align = 1;
        for (idx, variant) in enum_def.variants.iter().enumerate() {
            let field_ty = variant.payload_ty.unwrap_or(TypeId::VOID);
            self.track_pure_enum_repr_in_type(field_ty);

            union_fields.push(MastField {
                name: variant.name,
                ty: field_ty,
            });

            if field_ty != TypeId::VOID && field_ty != TypeId::ERROR {
                let mut layout = LayoutEngine::new(self.ctx);
                let size = layout.compute_type_size(field_ty);
                let align = layout.compute_type_align(field_ty);
                if size > max_size {
                    max_size = size;
                    largest_idx = idx;
                }
                max_align = max_align.max(align);
            }
        }

        let mangled_name = self.ctx.mangle_type(norm_ty);

        self.module.structs.push(MastStruct {
            id: payload_union_id,
            name: format!("{}_payload", mangled_name),
            fields: union_fields,
            is_extern: false,
            is_union: true,
            largest_field_idx: largest_idx,
            union_size: max_size.max(1) as usize,
            union_align: max_align.max(1) as usize,
            attributes: vec![],
        });

        let payload_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::AnonymousEnumPayload(norm_ty));
        let tag_ty = enum_def.backing_ty.unwrap_or(TypeId::U32);

        self.module.structs.push(MastStruct {
            id: wrapper_id,
            name: mangled_name,
            fields: vec![
                MastField {
                    name: self.ctx.intern("__tag"),
                    ty: tag_ty,
                },
                MastField {
                    name: self.ctx.intern("__payload"),
                    ty: payload_ty,
                },
            ],
            is_extern: false,
            is_union: false,
            largest_field_idx: 0,
            union_size: 0,
            union_align: 1,
            attributes: vec![],
        });

        wrapper_id
    }

    pub(crate) fn instantiate_union(
        &mut self,
        def_id: DefId,
        args: &[TypeId],
        id: MonoId,
    ) -> MonoId {
        let def = if let Def::Union(u) = &self.ctx.defs[def_id.0 as usize] {
            u.clone()
        } else {
            self.ctx.emit_ice(
                Span::default(),
                format!("Kern ICE (Lowering): DefId {} is not a Union!", def_id.0),
            );
            self.placeholder_struct(id, format!("__ice_union_{}", id.0), true);
            return id;
        };

        let mangled_name = self.ctx.get_export_name(def_id, args);
        let Some(subst_map) =
            self.build_generic_subst_map("union", &mangled_name, &def.generics, args)
        else {
            self.placeholder_struct(id, format!("__ice_union_{}", id.0), true);
            return id;
        };

        let mut mast_fields = Vec::new();
        let mut max_size = 0;
        let mut max_align = 1;
        let mut largest_field_idx = 0;

        for (idx, f) in def.fields.iter().enumerate() {
            let raw_ty = self
                .ctx
                .node_types
                .get(&f.type_node.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let conc_ty = {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
                subst.substitute(raw_ty)
            };
            self.track_pure_enum_repr_in_type(conc_ty);
            mast_fields.push(MastField {
                name: f.name,
                ty: conc_ty,
            });
            let mut le = LayoutEngine::new(self.ctx);
            let size = le.compute_type_size(conc_ty);
            let align = le.compute_type_align(conc_ty);

            if size > max_size {
                max_size = size;
                largest_field_idx = idx;
            }
            max_align = max_align.max(align);
        }

        self.module.structs.push(MastStruct {
            id,
            name: mangled_name,
            fields: mast_fields,
            is_extern: def.is_extern,
            is_union: true,
            largest_field_idx,
            union_size: max_size.max(1) as usize,
            union_align: max_align.max(1) as usize,
            attributes: vec![],
        });
        id
    }

    pub(crate) fn instantiate_data(&mut self, def_id: DefId, args: &[TypeId]) -> MonoId {
        let key = (def_id, args.to_vec());
        if let Some(&id) = self.mono_cache.get(&key) {
            return id;
        }

        let wrapper_id = self.new_mono_id();
        let payload_union_id = self.new_mono_id();
        self.mono_cache.insert(key, wrapper_id);
        self.adt_union_map.insert(wrapper_id, payload_union_id);

        let def = if let Def::Enum(a) = &self.ctx.defs[def_id.0 as usize] {
            a.clone()
        } else {
            self.ctx.emit_ice(
                Span::default(),
                format!(
                    "Kern ICE (Lowering): DefId {} is not an Enum (Data)! ",
                    def_id.0
                ),
            );
            self.placeholder_data_structs(
                wrapper_id,
                payload_union_id,
                &format!("__ice_enum_{}", wrapper_id.0),
            );
            return wrapper_id;
        };

        let mangled_name = self.ctx.get_export_name(def_id, args);
        let Some(subst_map) =
            self.build_generic_subst_map("enum", &mangled_name, &def.generics, args)
        else {
            self.placeholder_data_structs(
                wrapper_id,
                payload_union_id,
                &format!("__ice_enum_{}", wrapper_id.0),
            );
            return wrapper_id;
        };

        // 1. Build the inner payload union.
        let mut union_fields = Vec::new();
        let mut largest_idx = 0;
        let mut max_size = 0;
        let mut max_align = 1;

        for (idx, variant) in def.variants.iter().enumerate() {
            let field_ty = if let Some(payload_ast) = &variant.payload_type {
                let raw_ty = self
                    .ctx
                    .node_types
                    .get(&payload_ast.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                {
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
                    subst.substitute(raw_ty)
                }
            } else {
                TypeId::VOID // Empty unions can be modeled as `void` here.
            };

            union_fields.push(MastField {
                name: variant.name,
                ty: field_ty,
            });

            if field_ty != TypeId::VOID && field_ty != TypeId::ERROR {
                let size = {
                    let mut le = LayoutEngine::new(self.ctx);
                    le.compute_type_size(field_ty)
                };
                let align = {
                    let mut le = LayoutEngine::new(self.ctx);
                    le.compute_type_align(field_ty)
                };

                if size > max_size {
                    max_size = size;
                    largest_idx = idx;
                }
                max_align = max_align.max(align);
            }
        }

        self.module.structs.push(MastStruct {
            id: payload_union_id,
            name: format!("{}_payload", mangled_name),
            fields: union_fields,
            is_extern: false,
            is_union: true,
            largest_field_idx: largest_idx,
            union_size: max_size.max(1) as usize,
            union_align: max_align.max(1) as usize,
            attributes: vec![],
        });

        // 2. Build the outer wrapper struct `(tag + union)` and substitute the tag type.
        let tag_ty = if let Some(bt) = &def.backing_type {
            let raw_tag_ty = self
                .ctx
                .node_types
                .get(&bt.id)
                .copied()
                .unwrap_or(TypeId::U32);
            let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
            subst.substitute(raw_tag_ty)
        } else {
            TypeId::U32 // Fall back to `u32` when no explicit backing type is present.
        };

        let union_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::EnumPayload(def_id, args.to_vec()));

        self.module.structs.push(MastStruct {
            id: wrapper_id,
            name: mangled_name,
            fields: vec![
                MastField {
                    name: self.ctx.intern("__tag"),
                    ty: tag_ty,
                },
                MastField {
                    name: self.ctx.intern("__payload"),
                    ty: union_ty,
                },
            ],
            is_extern: false,
            is_union: false,
            largest_field_idx: 0,
            union_size: 0,
            union_align: 1,
            attributes: vec![],
        });

        wrapper_id
    }

    pub(crate) fn lower_global(&mut self, g: &GlobalDef) {
        let id = match self.global_map.get(&g.id) {
            Some(&id) => id,
            None => {
                let name = self.ctx.resolve(g.name);
                self.ctx.emit_ice(
                    kernc_utils::Span::default(),
                    format!("Kern ICE (Lowering): Global MonoId for `{}` missing.", name),
                );
                let placeholder = self.new_mono_id();
                self.global_map.insert(g.id, placeholder);
                placeholder
            }
        };

        let ty = self
            .ctx
            .node_types
            .get(&g.value.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        self.track_pure_enum_repr_in_type(ty);
        let is_mut = g.is_mut;

        // Perform constant folding.
        let init = if !g.is_extern {
            let prev_scope = self.ctx.scopes.current_scope_id();
            if let Some(owner_scope) = self.global_owner_scope(g.id) {
                self.ctx.scopes.set_current_scope(owner_scope);
            }

            let folded = {
                let mut ce = ConstEvaluator::new(self.ctx);
                if let Ok(val) = ce.eval_inner(&g.value, 0) {
                    match val {
                        ConstValue::Int(v) => {
                            Some(MastExpr::new(ty, MastExprKind::Integer(v as u128), g.span))
                        }
                        ConstValue::Float(f) => {
                            Some(MastExpr::new(ty, MastExprKind::Float(f), g.span))
                        }
                        ConstValue::Bool(b) => {
                            Some(MastExpr::new(ty, MastExprKind::Bool(b), g.span))
                        }
                        _ => Some(self.lower_expr(&g.value, &HashMap::new(), Some(ty))),
                    }
                } else {
                    Some(self.lower_expr(&g.value, &HashMap::new(), Some(ty)))
                }
            };

            if let Some(prev_scope) = prev_scope {
                self.ctx.scopes.set_current_scope(prev_scope);
            }

            folded
        } else {
            None
        };

        self.module.globals.push(MastGlobal {
            id,
            name: self.ctx.get_export_name(g.id, &[]),
            linkage: MastLinkage::External,
            ty,
            is_mut,
            init,
            is_extern: g.is_extern,
            attributes: self.extract_meta_items(&g.attributes),
        });
    }

    pub(crate) fn ensure_global_lowered(&mut self, def_id: DefId) {
        if self.module.globals.iter().any(|global| {
            self.global_map
                .get(&def_id)
                .is_some_and(|mono_id| *mono_id == global.id)
        }) {
            return;
        }

        let Some(Def::Global(global)) = self.ctx.defs.get(def_id.0 as usize).cloned() else {
            return;
        };
        self.lower_global(&global);
    }
}
