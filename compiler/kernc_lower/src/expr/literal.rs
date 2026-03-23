use super::Lowerer;
use std::collections::HashMap;

use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_mast::*;
use kernc_sema::checker::{ConstEvaluator, Substituter};
use kernc_sema::def::{Def, DefId, StructDef, UnionDef};
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{Span, SymbolId};

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(crate) fn lower_string_literal(&mut self, s: &str, span: Span) -> MastExprKind {
        let global_id = self.new_mono_id();
        let len = s.len() as u64;
        let array_ty = self.ctx.type_registry.intern(TypeKind::Array {
            is_mut: false, // 字符串常量不可变
            elem: TypeId::U8,
            len,
        });

        self.module.globals.push(MastGlobal {
            id: global_id,
            name: format!(".str.{}", global_id.0),
            ty: array_ty,
            is_mut: false,
            init: Some(MastExpr::new(
                array_ty,
                MastExprKind::StringLiteral(s.to_string()),
                span,
            )),
            is_extern: false,
            attributes: vec![],
        });

        let data_ptr = MastExpr::new(
            self.ctx.type_registry.intern(TypeKind::Pointer {
                is_mut: false,
                elem: array_ty,
            }),
            MastExprKind::AddressOf(Box::new(MastExpr::new(
                array_ty,
                MastExprKind::GlobalRef(global_id),
                span,
            ))),
            span,
        );
        let meta = MastExpr::new(TypeId::USIZE, MastExprKind::Integer(len as u128), span);

        MastExprKind::ConstructFatPointer {
            data_ptr: Box::new(data_ptr),
            meta: Box::new(meta),
        }
    }

    pub(crate) fn lower_static_decl(
        &mut self,
        name: SymbolId,
        init: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
        concrete_ty: TypeId,
        is_mut: bool,
    ) -> MastExprKind {
        let global_id = self.new_mono_id();
        let lower_init = self.lower_expr(init, subst_map, Some(concrete_ty));

        self.module.globals.push(MastGlobal {
            id: global_id,
            name: format!("local_static_{}_{}", self.ctx.resolve(name), global_id.0),
            ty: concrete_ty,
            is_mut,
            init: Some(lower_init),
            is_extern: false,
            attributes: vec![],
        });

        if let Some(scope) = self.local_statics.last_mut() {
            scope.insert(name, global_id);
        }

        MastExprKind::GlobalRef(global_id)
    }

    pub(crate) fn lower_data_init(
        &mut self,
        literal: &ast::DataLiteralKind,
        subst_map: &HashMap<SymbolId, TypeId>,
        concrete_ty: TypeId,
        span: Span,
    ) -> MastExprKind {
        let norm = self.ctx.type_registry.get(concrete_ty).clone();

        match literal {
            ast::DataLiteralKind::Struct(fields) => {
                self.lower_struct_union_data_init(fields, subst_map, concrete_ty)
            }
            ast::DataLiteralKind::Array(elems) => {
                let is_target_array_like = matches!(
                    norm,
                    TypeKind::Array { .. } | TypeKind::ArrayInfer { .. } | TypeKind::Slice { .. }
                );
                if elems.is_empty() && !is_target_array_like {
                    // 当作空结构体/联合体/ADT处理，确保它们被正确 Instantiate
                    self.lower_struct_union_data_init(&[], subst_map, concrete_ty)
                } else {
                    self.lower_array_init(elems, subst_map, concrete_ty)
                }
            }
            ast::DataLiteralKind::Repeat { value, .. } => {
                self.lower_repeat_init(value, subst_map, concrete_ty)
            }
            ast::DataLiteralKind::Scalar(inner) => {
                self.lower_scalar_init(inner, subst_map, concrete_ty, span)
            }
        }
    }

    /// 统一聚合数据初始化路由
    pub(crate) fn lower_struct_union_data_init(
        &mut self,
        fields: &[ast::StructFieldInit],
        subst_map: &HashMap<SymbolId, TypeId>,
        concrete_ty: TypeId,
    ) -> MastExprKind {
        let norm = self.ctx.type_registry.get(concrete_ty).clone();

        match norm {
            TypeKind::Enum(def_id, gen_args) => {
                self.lower_data_payload_init(fields, def_id, &gen_args, subst_map)
            }
            TypeKind::Def(def_id, gen_args) => {
                let def = self.ctx.defs[def_id.0 as usize].clone();
                match def {
                    Def::Struct(s) => {
                        self.lower_struct_init(fields, def_id, &s, &gen_args, subst_map)
                    }
                    Def::Union(u) => {
                        self.lower_union_init(fields, def_id, &u, &gen_args, subst_map)
                    }
                    _ => {
                        self.ctx.emit_ice(Span::default(), "Kern ICE (Lowering): DefId must point to a Struct or Union during structural initialization.");
                        unreachable!()
                    }
                }
            }
            _ => {
                self.ctx.emit_ice(Span::default(), format!("Kern ICE (Lowering): Invalid type for structural initialization: {:?}", norm));
                unreachable!()
            }
        }
    }

    /// 辅助 1：处理带有负载的 Enum 变体初始化
    pub(crate) fn lower_data_payload_init(
        &mut self,
        fields: &[ast::StructFieldInit],
        def_id: DefId,
        gen_args: &[TypeId],
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> MastExprKind {
        let mono_id = self.instantiate_data(def_id, gen_args);
        let def = if let Def::Enum(d) = &self.ctx.defs[def_id.0 as usize] {
            d.clone()
        } else {
            self.ctx.emit_ice(Span::default(), "Kern ICE (Lowering): Expected Enum definition.");
            unreachable!()
        };

        let init_f = &fields[0];
        let tag_val = match def.variants.iter().position(|v| v.name == init_f.name) {
            Some(idx) => idx as u128,
            None => {
                self.ctx.emit_ice(init_f.value.span, format!("Kern ICE (Lowering): Variant `{}` not found in enum.", self.ctx.resolve(init_f.name)));
                unreachable!()
            }
        };

        let mut variant_subst_map = HashMap::new();
        for (i, param) in def.generics.iter().enumerate() {
            variant_subst_map.insert(param.name, gen_args[i]);
        }

        let variant_def = &def.variants[tag_val as usize];
        let payload_id = match &variant_def.payload_type {
            Some(p) => p.id,
            None => {
                self.ctx.emit_ice(init_f.value.span, "Kern ICE (Lowering): Attempted to initialize payload for a variant without payload.");
                unreachable!()
            }
        };

        let raw_payload_ty = self.ctx.node_types.get(&payload_id).copied().unwrap_or(TypeId::ERROR);

        let conc_payload_ty = Substituter::new(&mut self.ctx.type_registry, &variant_subst_map)
            .substitute(raw_payload_ty);

        let payload_expr = self.lower_expr(&init_f.value, subst_map, Some(conc_payload_ty));

        MastExprKind::DataInit {
            data_struct_id: mono_id,
            tag_value: tag_val,
            payload: Box::new(payload_expr),
        }
    }

    /// 辅助 2：处理普通 Struct 初始化
    pub(crate) fn lower_struct_init(
        &mut self,
        fields: &[ast::StructFieldInit],
        def_id: DefId,
        s: &StructDef,
        gen_args: &[TypeId],
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> MastExprKind {
        let mono_id = self.instantiate_struct(def_id, gen_args);

        let mut struct_subst_map = HashMap::new();
        for (i, param) in s.generics.iter().enumerate() {
            struct_subst_map.insert(param.name, gen_args[i]);
        }

        let mut ordered_fields = Vec::new();
        for f_def in &s.fields {
            let raw_f_ty = self
                .ctx
                .node_types
                .get(&f_def.type_node.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let conc_f_ty = Substituter::new(&mut self.ctx.type_registry, &struct_subst_map)
                .substitute(raw_f_ty);

            if let Some(init_f) = fields.iter().find(|f| f.name == f_def.name) {
                ordered_fields.push(self.lower_expr(&init_f.value, subst_map, Some(conc_f_ty)));
            } else {
                ordered_fields.push(self.lower_expr(
                    f_def.default_value.as_ref().unwrap(),
                    subst_map,
                    Some(conc_f_ty),
                ));
            }
        }

        MastExprKind::StructInit {
            struct_id: mono_id,
            fields: ordered_fields,
        }
    }

    /// 辅助 3：处理 Union 初始化
    pub(crate) fn lower_union_init(
        &mut self,
        fields: &[ast::StructFieldInit],
        def_id: DefId,
        u: &UnionDef,
        gen_args: &[TypeId],
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> MastExprKind {
        let mono_id = self.instantiate_struct(def_id, gen_args);

        let mut union_subst_map = HashMap::new();
        for (i, param) in u.generics.iter().enumerate() {
            union_subst_map.insert(param.name, gen_args[i]);
        }

        let init_f = &fields[0];
        let field_idx = match u.fields.iter().position(|f| f.name == init_f.name) {
            Some(idx) => idx,
            None => {
                self.ctx.emit_ice(init_f.value.span, format!("Kern ICE (Lowering): Field `{}` not found in union.", self.ctx.resolve(init_f.name)));
                unreachable!()
            }
        };

        let raw_f_ty = self
            .ctx
            .node_types
            .get(&u.fields[field_idx].type_node.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let conc_f_ty =
            Substituter::new(&mut self.ctx.type_registry, &union_subst_map).substitute(raw_f_ty);

        let val_expr = self.lower_expr(&init_f.value, subst_map, Some(conc_f_ty));

        MastExprKind::UnionInit {
            union_id: mono_id,
            field_idx,
            value: Box::new(val_expr),
        }
    }

    pub(crate) fn lower_array_init(
        &mut self,
        elems: &[Expr],
        subst_map: &HashMap<SymbolId, TypeId>,
        concrete_ty: TypeId,
    ) -> MastExprKind {
        let elem_ty = self.ctx.type_registry.get_elem_type(concrete_ty);
        let lowered_elems = elems
            .iter()
            .map(|e| self.lower_expr(e, subst_map, elem_ty))
            .collect();
        MastExprKind::ArrayInit(lowered_elems)
    }

    pub(crate) fn lower_repeat_init(
        &mut self,
        value: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
        concrete_ty: TypeId,
    ) -> MastExprKind {
        let elem_ty = self.ctx.type_registry.get_elem_type(concrete_ty);
        let elem = self.lower_expr(value, subst_map, elem_ty);
        let array_len = if let TypeKind::Array { len, .. } = self
            .ctx
            .type_registry
            .get(self.ctx.type_registry.normalize(concrete_ty))
        {
            *len
        } else {
            0
        };
        MastExprKind::ArrayInit(vec![elem; array_len as usize])
    }

    pub(crate) fn lower_scalar_init(
        &mut self,
        inner: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
        concrete_ty: TypeId,
        span: Span,
    ) -> MastExprKind {
        let norm = self.ctx.type_registry.get(concrete_ty).clone();

        match norm {
            TypeKind::Enum(def_id, gen_args) => {
                self.lower_data_scalar_init(inner, def_id, &gen_args)
            }
            // 拦截胖指针降级
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let elem_norm = self.ctx.type_registry.normalize(elem);
                if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(elem_norm) {
                    return self.lower_trait_object_init(inner, subst_map, elem_norm, span);
                }
                // 如果不是 Trait，当做普通单值
                self.lower_expr(inner, subst_map, Some(concrete_ty)).kind
            }
            _ => self.lower_expr(inner, subst_map, Some(concrete_ty)).kind,
        }
    }

    /// 辅助：构建没有负载的 Enum (例如 Option.None)
    pub(crate) fn lower_data_scalar_init(
        &mut self,
        inner: &Expr,
        def_id: DefId,
        gen_args: &[TypeId],
    ) -> MastExprKind {
        let def = if let Def::Enum(d) = &self.ctx.defs[def_id.0 as usize] {
            d.clone()
        } else {
            unreachable!()
        };

        let variant_name = if let ExprKind::Identifier(id) = &inner.kind {
            *id
        } else {
            unreachable!()
        };

        let tag_val = def
            .variants
            .iter()
            .position(|v| v.name == variant_name)
            .unwrap() as u128;

        // 纯数据优化：如果没有任何负载，直接降级为硬编码整数
        if self.is_pure_enum(&def) {
            MastExprKind::Integer(tag_val)
        } else {
            let mono_id = self.instantiate_data(def_id, gen_args);
            MastExprKind::DataInit {
                data_struct_id: mono_id,
                tag_value: tag_val,
                payload: Box::new(MastExpr::new(TypeId::VOID, MastExprKind::Undef, inner.span)),
            }
        }
    }

    /// 辅助：构建 Trait Object 胖指针
    pub(crate) fn lower_trait_object_init(
        &mut self,
        inner: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
        trait_norm: TypeId,
        span: Span,
    ) -> MastExprKind {
        let l = self.lower_expr(inner, subst_map, None);

        // 查找或生成 VTable
        let vtable_id = self.get_or_create_vtable(l.ty, trait_norm);

        let global_array_ty = match self.module.globals.iter().find(|g| g.id == vtable_id) {
            Some(g) => g.ty,
            None => {
                self.ctx.emit_ice(span, "Kern ICE (Lowering): VTable global missing when constructing trait object literal.");
                unreachable!()
            }
        };
        let array_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: global_array_ty,
        });

        // 生成底层构造器
        MastExprKind::ConstructFatPointer {
            data_ptr: Box::new(l),
            meta: Box::new(MastExpr::new(
                TypeId::USIZE,
                MastExprKind::Cast {
                    kind: MastCastKind::PtrToInt,
                    operand: Box::new(MastExpr::new(
                        array_ptr_ty,
                        MastExprKind::AddressOf(Box::new(MastExpr::new(
                            global_array_ty,
                            MastExprKind::GlobalRef(vtable_id),
                            span,
                        ))),
                        span,
                    )),
                },
                span,
            )),
        }
    }

    pub(crate) fn lower_enum_literal(
        &mut self,
        variant_name: SymbolId,
        concrete_ty: TypeId,
    ) -> MastExprKind {
        let norm_ty = self.ctx.type_registry.normalize(concrete_ty);
        let (def_id, gen_args) =
            if let TypeKind::Enum(id, args) = self.ctx.type_registry.get(norm_ty) {
                (*id, args.clone())
            } else {
                self.ctx.emit_ice(Span::default(), "Kern ICE (Lowering): Expected Enum type for enum literal.");
                unreachable!()
            };

        let data_def = if let Def::Enum(d) = &self.ctx.defs[def_id.0 as usize] {
            d.clone()
        } else {
            self.ctx.emit_ice(Span::default(), "Kern ICE (Lowering): Expected Enum Definition.");
            unreachable!()
        };

        let mut current_val: i128 = 0;
        for v in &data_def.variants {
            if let Some(v_expr) = &v.value {
                let mut ce = ConstEvaluator::new(self.ctx);
                if let Ok(val) = ce.eval_math(v_expr) {
                    current_val = val;
                }
            }
            if v.name == variant_name {
                // 直接返回整数。如果不是，包进 DataInit
                if self.is_pure_enum(&data_def) {
                    return MastExprKind::Integer(current_val as u128);
                } else {
                    let mono_id = self.instantiate_data(def_id, &gen_args);
                    return MastExprKind::DataInit {
                        data_struct_id: mono_id,
                        tag_value: current_val as u128,
                        payload: Box::new(MastExpr::new(
                            TypeId::VOID,
                            MastExprKind::Undef,
                            Span::default(),
                        )),
                    };
                }
            }
            current_val += 1;
        }
        self.ctx.emit_ice(Span::default(), format!("Kern ICE (Lowering): Variant `{}` not found in enum literal resolution.", self.ctx.resolve(variant_name)));
        unreachable!()
    }
}
