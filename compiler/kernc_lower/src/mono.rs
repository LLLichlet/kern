use super::Lowerer;
use kernc_mast::*;
use kernc_sema::LayoutEngine;
use kernc_sema::checker::Substituter;
use kernc_sema::def::{Def, DefId, GlobalDef};
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_sema::checker::ConstValue;
use kernc_utils::Span;
use std::collections::HashMap;

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
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
            unreachable!()
        };

        let all_generic_params = def.generics.clone();

        if all_generic_params.len() != args.len() {
            let fn_name = self.ctx.resolve(def.name);
            self.ctx.emit_ice(
                Span::default(),
                format!("Kern ICE (Lowering): Generics mismatch for function `{}`. Expected {}, got {}. Sema missed this.", fn_name, all_generic_params.len(), args.len())
            );
            unreachable!()
        }

        let mut subst_map = HashMap::new();
        for (i, param) in all_generic_params.iter().enumerate() {
            subst_map.insert(param.name, args[i]);
        }

        let mut mangled_name = self.ctx.resolve(def.name).to_string();
        for arg in args {
            mangled_name.push_str(&format!("_{}", arg.0));
        }

        let raw_ret = def.resolved_sig.map_or(TypeId::VOID, |sig| {
            if let TypeKind::Function { ret, .. } = self.ctx.type_registry.get(sig) {
                *ret
            } else {
                TypeId::VOID
            }
        });

        let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);

        let mut mast_params = Vec::new();
        for p in &def.params {
            let raw_ty = self
                .ctx
                .node_types
                .get(&p.type_node.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let conc_ty = subst.substitute(raw_ty);
            mast_params.push(MastParam {
                name: p.pattern.name,
                ty: conc_ty,
                is_mut: p.pattern.is_mut,
            });
        }

        let conc_ret = subst.substitute(raw_ret);

        let saved_local_types = std::mem::take(&mut self.local_types);
        let saved_defer_stack = std::mem::take(&mut self.defer_stack);
        let saved_loop_frames = std::mem::take(&mut self.loop_frames);
        let saved_local_statics = std::mem::take(&mut self.local_statics);

        self.local_types.push(std::collections::HashMap::new());
        for p in &mast_params {
            self.local_types
                .last_mut()
                .unwrap()
                .insert(p.name, (p.ty, p.is_mut));
        }

        let body = if let Some(body_expr) = &def.body {
            Some(self.lower_block_as_body(body_expr, &subst_map, conc_ret))
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

        let def = if let Def::Struct(s) = &self.ctx.defs[def_id.0 as usize] {
            s.clone()
        } else if let Def::Union(_) = &self.ctx.defs[def_id.0 as usize] {
            return self.instantiate_union(def_id, args, id);
        } else {
            self.ctx.emit_ice(
                Span::default(),
                format!(
                    "Kern ICE (Lowering): DefId {} is not a Struct or Union!",
                    def_id.0
                ),
            );
            unreachable!()
        };

        let mut subst_map = HashMap::new();
        for (i, param) in def.generics.iter().enumerate() {
            subst_map.insert(param.name, args[i]);
        }

        let mut mangled_name = self.ctx.resolve(def.name).to_string();
        for arg in args {
            mangled_name.push_str(&format!("_{}", arg.0));
        }

        let mut mast_fields = Vec::new();
        let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);

        for f in &def.fields {
            let raw_ty = self
                .ctx
                .node_types
                .get(&f.type_node.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let conc_ty = subst.substitute(raw_ty);
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
            attributes: self.extract_meta_items(&def.attributes),
        });
        id
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
            unreachable!()
        };

        let mut subst_map = HashMap::new();
        for (i, param) in def.generics.iter().enumerate() {
            subst_map.insert(param.name, args[i]);
        }

        let mut mangled_name = self.ctx.resolve(def.name).to_string();
        for arg in args {
            mangled_name.push_str(&format!("_{}", arg.0));
        }

        let mut mast_fields = Vec::new();
        let mut max_size = 0;
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
            mast_fields.push(MastField {
                name: f.name,
                ty: conc_ty,
            });
            let mut le = LayoutEngine::new(self.ctx);
            let size = le.compute_type_size(conc_ty);

            if size > max_size {
                max_size = size;
                largest_field_idx = idx;
            }
        }

        self.module.structs.push(MastStruct {
            id,
            name: mangled_name,
            fields: mast_fields,
            is_extern: false,
            is_union: true,
            largest_field_idx,
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
            unreachable!()
        };

        let mut mangled_name = self.ctx.resolve(def.name).to_string();
        for arg in args {
            mangled_name.push_str(&format!("_{}", arg.0));
        }

        let mut subst_map = HashMap::new();
        for (i, param) in def.generics.iter().enumerate() {
            subst_map.insert(param.name, args[i]);
        }

        // 1. 构建内部的 Payload Union
        let mut union_fields = Vec::new();
        let mut largest_idx = 0;
        let mut max_size = 0;

        for (idx, variant) in def.variants.iter().enumerate() {
            let field_ty = if let Some(payload_ast) = &variant.payload_type {
                let raw_ty = self
                    .ctx
                    .node_types
                    .get(&payload_ast.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let conc_ty = {
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
                    subst.substitute(raw_ty)
                };
                conc_ty
            } else {
                TypeId::VOID // LLVM 中对于空 Union 的处理可以是 i8 或者忽略
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

                if size > max_size {
                    max_size = size;
                    largest_idx = idx;
                }
            }
        }

        self.module.structs.push(MastStruct {
            id: payload_union_id,
            name: format!("{}_payload", mangled_name),
            fields: union_fields,
            is_extern: false,
            is_union: true,
            largest_field_idx: largest_idx,
            attributes: vec![],
        });

        // 2. 构建外部的 Wrapper Struct (Tag + Union)
        // 动态获取并泛型替换 ADT 的 Tag 类型
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
            TypeId::U32 // 如果没有指定，默认退化为 u32
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
            attributes: vec![],
        });

        wrapper_id
    }

    pub(crate) fn lower_global(&mut self, g: &GlobalDef) {
        let id = match self.global_map.get(&g.id) {
            Some(&id) => id,
            None => {
                let name = self.ctx.resolve(g.name);
                self.ctx.emit_ice(kernc_utils::Span::default(), format!("Kern ICE (Lowering): Global MonoId for `{}` missing.", name));
                unreachable!()
            }
        };

        let ty = self.ctx.node_types.get(&g.value.id).copied().unwrap_or(TypeId::ERROR);
        let is_mut = g.is_mut;

        // 常量折叠
        let init = if !g.is_extern {
            let mut ce = kernc_sema::checker::ConstEvaluator::new(self.ctx);
            if let Ok(val) = ce.eval_inner(&g.value, 0) {
                match val {
                    ConstValue::Int(v) => Some(MastExpr::new(ty, MastExprKind::Integer(v as u128), g.span)),
                    ConstValue::Float(f) => Some(MastExpr::new(ty, MastExprKind::Float(f), g.span)),
                    ConstValue::Bool(b) => Some(MastExpr::new(ty, MastExprKind::Bool(b), g.span)),
                    _ => Some(self.lower_expr(&g.value, &HashMap::new(), Some(ty))),
                }
            } else {
                Some(self.lower_expr(&g.value, &HashMap::new(), Some(ty)))
            }
        } else {
            None
        };

        self.module.globals.push(MastGlobal {
            id,
            name: self.ctx.resolve(g.name).to_string(),
            ty,
            is_mut,
            init,
            is_extern: g.is_extern,
            attributes: self.extract_meta_items(&g.attributes),
        });
    }
}
