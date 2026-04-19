use super::Lowerer;
use std::collections::HashMap;

use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_mast::*;
use kernc_mono::MonoId;
use kernc_sema::checker::ConstEvaluator;
use kernc_sema::def::{Def, DefId, StructDef, UnionDef};
use kernc_sema::ty::{GenericArg, TypeId, TypeKind};
use kernc_utils::{Span, SymbolId};

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    fn lower_literal_ice(&mut self, span: Span, message: impl Into<String>) -> MastExprKind {
        self.ctx.emit_ice(span, message);
        MastExprKind::Trap
    }

    fn require_enum_def(
        &mut self,
        def_id: DefId,
        span: Span,
        context: &str,
    ) -> Option<kernc_sema::def::EnumDef> {
        match self.ctx.defs.get(def_id.0 as usize).cloned() {
            Some(Def::Enum(def)) => Some(def),
            Some(other) => {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Lowering): Expected enum definition while trying to {}, found {:?}.",
                        context, other
                    ),
                );
                None
            }
            None => {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Lowering): Missing DefId {} while trying to {}.",
                        def_id.0, context
                    ),
                );
                None
            }
        }
    }

    fn require_anon_enum(
        &mut self,
        concrete_ty: TypeId,
        span: Span,
        context: &str,
    ) -> Option<kernc_sema::ty::AnonymousEnum> {
        let norm_ty = self.ctx.type_registry.normalize(concrete_ty);
        match self.ctx.type_registry.get(norm_ty).clone() {
            TypeKind::AnonymousEnum(enum_def) => Some(enum_def),
            other => {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Lowering): Expected anonymous enum while trying to {}, found {:?}.",
                        context, other
                    ),
                );
                None
            }
        }
    }

    fn require_identifier(&mut self, expr: &Expr, context: &str) -> Option<SymbolId> {
        match expr.kind {
            ExprKind::Identifier(id) => Some(id),
            _ => {
                self.ctx.emit_ice(
                    expr.span,
                    format!(
                        "Kern ICE (Lowering): Expected identifier while trying to {}.",
                        context
                    ),
                );
                None
            }
        }
    }

    pub(crate) fn vtable_global_type(&mut self, vtable_id: MonoId, span: Span) -> Option<TypeId> {
        match self
            .module
            .globals
            .iter()
            .find(|global| global.id == vtable_id)
        {
            Some(global) => Some(global.ty),
            None => {
                self.ctx.emit_ice(
                    span,
                    "Kern ICE (Lowering): VTable global missing when constructing trait object literal.",
                );
                None
            }
        }
    }

    pub(crate) fn vtable_global_addr_expr(
        &mut self,
        vtable_id: MonoId,
        span: Span,
    ) -> Option<MastExpr> {
        let global_array_ty = self.vtable_global_type(vtable_id, span)?;
        let array_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: global_array_ty,
        });

        Some(MastExpr::new(
            array_ptr_ty,
            MastExprKind::AddressOf(Box::new(MastExpr::new(
                global_array_ty,
                MastExprKind::GlobalRef(vtable_id),
                span,
            ))),
            span,
        ))
    }

    pub(crate) fn vtable_global_meta_expr(
        &mut self,
        vtable_id: MonoId,
        span: Span,
    ) -> Option<MastExpr> {
        Some(MastExpr::new(
            TypeId::USIZE,
            MastExprKind::Cast {
                kind: MastCastKind::PtrToInt,
                operand: Box::new(self.vtable_global_addr_expr(vtable_id, span)?),
            },
            span,
        ))
    }

    pub(crate) fn vtable_global_void_ptr_expr(
        &mut self,
        vtable_id: MonoId,
        span: Span,
    ) -> Option<MastExpr> {
        let global_array_ty = self.vtable_global_type(vtable_id, span)?;
        let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::VOID,
        });

        Some(MastExpr::new(
            void_ptr_ty,
            MastExprKind::AddressOf(Box::new(MastExpr::new(
                global_array_ty,
                MastExprKind::GlobalRef(vtable_id),
                span,
            ))),
            span,
        ))
    }

    pub(crate) fn lower_string_literal(&mut self, s: &str, span: Span) -> MastExprKind {
        let global_id = self.new_mono_id();
        let len = s.len() as u64;
        let array_ty = self.ctx.type_registry.intern(TypeKind::Array {
            is_mut: false, // String constants are immutable.
            elem: TypeId::U8,
            len: self.usize_const_generic(len),
        });

        self.module.globals.push(MastGlobal {
            id: global_id,
            name: format!(".str.{}.{}", self.module.name, global_id.0),
            linkage: MastLinkage::Internal,
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
        subst_map: &HashMap<SymbolId, GenericArg>,
        concrete_ty: TypeId,
        is_mut: bool,
    ) -> MastExprKind {
        let global_id = self.new_mono_id();
        let lower_init = self.lower_expr(init, subst_map, Some(concrete_ty));

        self.module.globals.push(MastGlobal {
            id: global_id,
            name: format!("local_static_{}_{}", self.ctx.resolve(name), global_id.0),
            linkage: MastLinkage::Internal,
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
        subst_map: &HashMap<SymbolId, GenericArg>,
        concrete_ty: TypeId,
        span: Span,
    ) -> MastExprKind {
        let norm = self.ctx.type_registry.get(concrete_ty).clone();

        if let TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } = norm {
            let inner_norm = self.ctx.type_registry.normalize(elem);
            if matches!(
                self.ctx.type_registry.get(inner_norm),
                TypeKind::ClosureInterface { .. }
            ) {
                let raw_expr_opt = match literal {
                    ast::DataLiteralKind::Scalar(inner) => Some(inner.as_ref()),
                    ast::DataLiteralKind::Struct(fields) if fields.len() == 1 => {
                        Some(&fields[0].value)
                    }
                    _ => None,
                };

                // Only lower through this path after successful extraction.
                if let Some(raw_expr) = raw_expr_opt {
                    let raw_mast = self.lower_expr(raw_expr, subst_map, None);

                    // Recover the underlying `AnonymousState` and its `NodeId` from the raw MAST type.
                    let raw_norm = self.ctx.type_registry.normalize(raw_mast.ty);
                    if let TypeKind::Pointer { elem: raw_elem, .. }
                    | TypeKind::VolatilePtr { elem: raw_elem, .. } =
                        self.ctx.type_registry.get(raw_norm).clone()
                    {
                        let raw_inner_norm = self.ctx.type_registry.normalize(raw_elem);

                        if let TypeKind::AnonymousState {
                            closure_node_id, ..
                        } = self.ctx.type_registry.get(raw_inner_norm)
                        {
                            // Look up the corresponding function `MonoId`.
                            let func_mono_id = self.get_closure_func_mono_id(*closure_node_id);

                            // Assemble the fat pointer payload.
                            let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
                                is_mut: false,
                                elem: TypeId::VOID,
                            });

                            let data_ptr_cast = MastExpr::new(
                                void_ptr_ty,
                                MastExprKind::Cast {
                                    kind: MastCastKind::Bitcast,
                                    operand: Box::new(raw_mast),
                                },
                                span,
                            );

                            let func_ref = MastExpr::new(
                                TypeId::VOID,
                                MastExprKind::FuncRef(func_mono_id),
                                span,
                            );
                            let code_ptr_cast = MastExpr::new(
                                TypeId::USIZE,
                                MastExprKind::Cast {
                                    kind: MastCastKind::PtrToInt,
                                    operand: Box::new(func_ref),
                                },
                                span,
                            );

                            return MastExprKind::ConstructFatPointer {
                                data_ptr: Box::new(data_ptr_cast),
                                meta: Box::new(code_ptr_cast),
                            };
                        }
                    }
                }

                // If extraction fails, rethrow the original error.
                self.ctx.struct_error(span, "invalid closure fat pointer construction")
                .with_hint("expected syntax: `*mut Fn(...).{ raw_pointer }`")
                .with_hint("the raw pointer must explicitly be a pointer to the closure's anonymous state")
                .emit();
                return MastExprKind::Undef;
            }
        }

        match literal {
            ast::DataLiteralKind::Struct(fields) => {
                self.lower_struct_union_data_init(fields, subst_map, concrete_ty)
            }
            ast::DataLiteralKind::Array(elems) => {
                let is_target_array_like = matches!(
                    norm,
                    TypeKind::Array { .. }
                        | TypeKind::ArrayInfer { .. }
                        | TypeKind::Slice { .. }
                        | TypeKind::Simd { .. }
                );
                if elems.is_empty() && !is_target_array_like {
                    // Treat these as empty aggregates so they are still instantiated correctly.
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

    /// Unified routing entry for aggregate data initialization.
    pub(crate) fn lower_struct_union_data_init(
        &mut self,
        fields: &[ast::StructFieldInit],
        subst_map: &HashMap<SymbolId, GenericArg>,
        concrete_ty: TypeId,
    ) -> MastExprKind {
        let norm = self.ctx.type_registry.get(concrete_ty).clone();

        // Sema accepts `void.{}` and contextual `.{}` as zero-sized initializers.
        // Lower them directly to a zero-sized value instead of routing them
        // through the ordinary aggregate machinery.
        if self.ctx.type_registry.is_void(concrete_ty) {
            debug_assert!(
                fields.is_empty(),
                "void aggregate initialization should not carry fields after sema"
            );
            let _ = subst_map;
            return MastExprKind::Undef;
        }

        match norm {
            TypeKind::Enum(def_id, gen_args) => {
                self.lower_data_payload_init(fields, def_id, &gen_args, subst_map)
            }
            TypeKind::AnonymousEnum(..) => {
                self.lower_anon_enum_payload_init(fields, concrete_ty, subst_map)
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
                        self.lower_literal_ice(
                            Span::default(),
                            "Kern ICE (Lowering): DefId must point to a Struct or Union during structural initialization.",
                        )
                    }
                }
            }
            TypeKind::AnonymousStruct(..) => {
                self.lower_anon_struct_init(fields, concrete_ty, subst_map)
            }
            TypeKind::AnonymousUnion(..) => {
                self.lower_anon_union_init(fields, concrete_ty, subst_map)
            }
            _ => {
                self.ctx.emit_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Lowering): Invalid type for structural initialization: {:?}",
                        norm
                    ),
                );
                MastExprKind::Trap
            }
        }
    }

    /// Helper 1: lower payload-carrying enum variant initialization.
    pub(crate) fn lower_data_payload_init(
        &mut self,
        fields: &[ast::StructFieldInit],
        def_id: DefId,
        gen_args: &[GenericArg],
        subst_map: &HashMap<SymbolId, GenericArg>,
    ) -> MastExprKind {
        let mono_id = self.instantiate_data(def_id, gen_args);
        let Some(def) =
            self.require_enum_def(def_id, Span::default(), "lower an enum payload literal")
        else {
            return MastExprKind::Trap;
        };

        let init_f = &fields[0];
        let Some((variant_idx, tag_val)) =
            self.named_enum_variant_info(&def, init_f.name, init_f.value.span)
        else {
            return MastExprKind::Trap;
        };

        let mut variant_subst_map = HashMap::new();
        for (i, param) in def.generics.iter().enumerate() {
            variant_subst_map.insert(param.name, gen_args[i]);
        }

        let variant_def = &def.variants[variant_idx];
        let payload_id = match &variant_def.payload_type {
            Some(p) => p.id,
            None => {
                return self.lower_literal_ice(
                    init_f.value.span,
                    "Kern ICE (Lowering): Attempted to initialize payload for a variant without payload.",
                );
            }
        };

        let raw_payload_ty = self
            .ctx
            .node_types
            .get(&payload_id)
            .copied()
            .unwrap_or(TypeId::ERROR);

        let conc_payload_ty = self.substitute_type_with_map(raw_payload_ty, &variant_subst_map);

        let payload_expr = self.lower_expr(&init_f.value, subst_map, Some(conc_payload_ty));

        MastExprKind::DataInit {
            data_struct_id: mono_id,
            tag_value: tag_val,
            payload: Box::new(payload_expr),
        }
    }

    /// Helper 2: lower ordinary struct initialization.
    pub(crate) fn lower_struct_init(
        &mut self,
        fields: &[ast::StructFieldInit],
        def_id: DefId,
        s: &StructDef,
        gen_args: &[GenericArg],
        subst_map: &HashMap<SymbolId, GenericArg>,
    ) -> MastExprKind {
        let mono_id = self.instantiate_struct(def_id, gen_args);

        let mut struct_subst_map = HashMap::new();
        for (i, param) in s.generics.iter().enumerate() {
            struct_subst_map.insert(param.name, gen_args[i]);
        }

        let mut ast_ordered_exprs = Vec::new();
        for f_def in &s.fields {
            let raw_f_ty = self
                .ctx
                .node_types
                .get(&f_def.type_node.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let conc_f_ty = self.substitute_type_with_map(raw_f_ty, &struct_subst_map);

            if let Some(init_f) = fields.iter().find(|f| f.name == f_def.name) {
                ast_ordered_exprs.push(self.lower_expr(&init_f.value, subst_map, Some(conc_f_ty)));
            } else {
                // Field defaults are type-checked in the data type's own generic
                // scope, so they must be lowered with that substitution map
                // rather than the caller's surrounding context.
                ast_ordered_exprs.push(self.lower_expr(
                    f_def.default_value.as_ref().unwrap(),
                    &struct_subst_map,
                    Some(conc_f_ty),
                ));
            }
        }

        let (_, physical_to_ast) = self.cached_named_struct_mapping(def_id, gen_args);

        let mut physical_ordered_exprs = Vec::with_capacity(s.fields.len());
        for &ast_idx in &physical_to_ast {
            physical_ordered_exprs.push(ast_ordered_exprs[ast_idx].clone());
        }

        MastExprKind::StructInit {
            struct_id: mono_id,
            fields: physical_ordered_exprs,
        }
    }

    /// Helper 3: lower union initialization.
    pub(crate) fn lower_union_init(
        &mut self,
        fields: &[ast::StructFieldInit],
        def_id: DefId,
        u: &UnionDef,
        gen_args: &[GenericArg],
        subst_map: &HashMap<SymbolId, GenericArg>,
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
                return self.lower_literal_ice(
                    init_f.value.span,
                    format!(
                        "Kern ICE (Lowering): Field `{}` not found in union.",
                        self.ctx.resolve(init_f.name)
                    ),
                );
            }
        };

        let raw_f_ty = self
            .ctx
            .node_types
            .get(&u.fields[field_idx].type_node.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let conc_f_ty = self.substitute_type_with_map(raw_f_ty, &union_subst_map);

        let val_expr = self.lower_expr(&init_f.value, subst_map, Some(conc_f_ty));

        MastExprKind::UnionInit {
            union_id: mono_id,
            field_idx,
            value: Box::new(val_expr),
        }
    }

    pub(crate) fn lower_anon_struct_init(
        &mut self,
        fields: &[ast::StructFieldInit],
        concrete_ty: TypeId,
        subst_map: &HashMap<SymbolId, GenericArg>,
    ) -> MastExprKind {
        let norm_ty = self.ctx.type_registry.normalize(concrete_ty);
        let (is_extern, anon_fields) = if let TypeKind::AnonymousStruct(is_extern, fields) =
            self.ctx.type_registry.get(norm_ty).clone()
        {
            (is_extern, fields)
        } else {
            return self.lower_literal_ice(
                Span::default(),
                "Kern ICE (Lowering): Expected anonymous struct during literal lowering.",
            );
        };

        let struct_id = self.instantiate_anon_struct(norm_ty);
        let mut ast_ordered_exprs = Vec::new();
        for field_def in &anon_fields {
            let Some(init_f) = fields.iter().find(|field| field.name == field_def.name) else {
                return self.lower_literal_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Lowering): Missing field `{}` in anonymous struct literal.",
                        self.ctx.resolve(field_def.name)
                    ),
                );
            };
            ast_ordered_exprs.push(self.lower_expr(&init_f.value, subst_map, Some(field_def.ty)));
        }

        let (_, physical_to_ast) =
            self.cached_anon_struct_mapping(norm_ty, is_extern, &anon_fields);

        let mut physical_ordered_exprs = Vec::with_capacity(anon_fields.len());
        for &ast_idx in &physical_to_ast {
            physical_ordered_exprs.push(ast_ordered_exprs[ast_idx].clone());
        }

        MastExprKind::StructInit {
            struct_id,
            fields: physical_ordered_exprs,
        }
    }

    pub(crate) fn lower_anon_union_init(
        &mut self,
        fields: &[ast::StructFieldInit],
        concrete_ty: TypeId,
        subst_map: &HashMap<SymbolId, GenericArg>,
    ) -> MastExprKind {
        let norm_ty = self.ctx.type_registry.normalize(concrete_ty);
        let anon_fields = if let TypeKind::AnonymousUnion(_, fields) =
            self.ctx.type_registry.get(norm_ty).clone()
        {
            fields
        } else {
            return self.lower_literal_ice(
                Span::default(),
                "Kern ICE (Lowering): Expected anonymous union during literal lowering.",
            );
        };

        let union_id = self.instantiate_anon_union(norm_ty);
        let init_f = &fields[0];
        let field_idx = anon_fields
            .iter()
            .position(|field| field.name == init_f.name)
            .unwrap_or(usize::MAX);
        if field_idx == usize::MAX {
            return self.lower_literal_ice(
                init_f.span,
                format!(
                    "Kern ICE (Lowering): Field `{}` not found in anonymous union.",
                    self.ctx.resolve(init_f.name)
                ),
            );
        }
        let field_ty = anon_fields[field_idx].ty;
        let value = self.lower_expr(&init_f.value, subst_map, Some(field_ty));

        MastExprKind::UnionInit {
            union_id,
            field_idx,
            value: Box::new(value),
        }
    }

    pub(crate) fn lower_anon_enum_payload_init(
        &mut self,
        fields: &[ast::StructFieldInit],
        concrete_ty: TypeId,
        subst_map: &HashMap<SymbolId, GenericArg>,
    ) -> MastExprKind {
        let norm_ty = self.ctx.type_registry.normalize(concrete_ty);
        let Some(enum_def) = self.require_anon_enum(
            concrete_ty,
            Span::default(),
            "lower an anonymous enum payload literal",
        ) else {
            return MastExprKind::Trap;
        };

        let mono_id = self.instantiate_anon_enum(norm_ty);
        let init_f = &fields[0];
        let Some(tag_value) = self.anon_enum_tag_value(&enum_def, init_f.name) else {
            return self.lower_literal_ice(
                init_f.span,
                "Kern ICE (Lowering): Anonymous enum variant not found during payload lowering.",
            );
        };

        let Some(variant) = enum_def
            .variants
            .iter()
            .find(|variant| variant.name == init_f.name)
        else {
            return self.lower_literal_ice(
                init_f.span,
                "Kern ICE (Lowering): Failed to resolve anonymous enum variant during payload lowering.",
            );
        };
        let Some(payload_ty) = variant.payload_ty else {
            return self.lower_literal_ice(
                init_f.span,
                "Kern ICE (Lowering): Attempted to build anonymous enum payload for a payload-less variant.",
            );
        };
        let payload = self.lower_expr(&init_f.value, subst_map, Some(payload_ty));

        MastExprKind::DataInit {
            data_struct_id: mono_id,
            tag_value: tag_value as u128,
            payload: Box::new(payload),
        }
    }

    pub(crate) fn lower_array_init(
        &mut self,
        elems: &[Expr],
        subst_map: &HashMap<SymbolId, GenericArg>,
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
        subst_map: &HashMap<SymbolId, GenericArg>,
        concrete_ty: TypeId,
    ) -> MastExprKind {
        let elem_ty = self.ctx.type_registry.get_elem_type(concrete_ty);
        let elem = self.lower_expr(value, subst_map, elem_ty);
        let array_len = if let TypeKind::Array { len, .. } = self
            .ctx
            .type_registry
            .get(self.ctx.type_registry.normalize(concrete_ty))
        {
            self.const_generic_usize(*len, value.span).unwrap_or(0)
        } else {
            0
        };
        MastExprKind::ArrayInit(vec![elem; array_len as usize])
    }

    pub(crate) fn lower_scalar_init(
        &mut self,
        inner: &Expr,
        subst_map: &HashMap<SymbolId, GenericArg>,
        concrete_ty: TypeId,
        span: Span,
    ) -> MastExprKind {
        let norm = self.ctx.type_registry.get(concrete_ty).clone();

        match norm {
            TypeKind::Enum(def_id, gen_args) => {
                self.lower_data_scalar_init(inner, def_id, &gen_args)
            }
            TypeKind::AnonymousEnum(..) => self.lower_anon_enum_scalar_init(inner, concrete_ty),
            // Intercept fat-pointer decay.
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let elem_norm = self.ctx.type_registry.normalize(elem);
                if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(elem_norm) {
                    return self.lower_trait_object_init(
                        inner,
                        subst_map,
                        concrete_ty,
                        elem_norm,
                        span,
                    );
                }
                // Non-trait targets behave like ordinary scalar values.
                self.lower_expr(inner, subst_map, Some(concrete_ty)).kind
            }
            _ => self.lower_expr(inner, subst_map, Some(concrete_ty)).kind,
        }
    }

    /// Helper: build a payload-free enum variant such as `Option.None`.
    pub(crate) fn lower_data_scalar_init(
        &mut self,
        inner: &Expr,
        def_id: DefId,
        gen_args: &[GenericArg],
    ) -> MastExprKind {
        let Some(def) = self.require_enum_def(def_id, inner.span, "lower a scalar enum literal")
        else {
            return MastExprKind::Trap;
        };

        let Some(variant_name) = self.require_identifier(inner, "lower a scalar enum literal")
        else {
            return MastExprKind::Trap;
        };

        let Some((_, tag_val)) = self.named_enum_variant_info(&def, variant_name, inner.span)
        else {
            return MastExprKind::Trap;
        };

        // Pure-data enums with no payload can lower directly to an integer constant.
        if self.is_pure_enum(&def) {
            self.record_pure_enum_tag_ty(def_id, gen_args);
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

    pub(crate) fn lower_anon_enum_scalar_init(
        &mut self,
        inner: &Expr,
        concrete_ty: TypeId,
    ) -> MastExprKind {
        let norm_ty = self.ctx.type_registry.normalize(concrete_ty);
        let Some(enum_def) = self.require_anon_enum(
            concrete_ty,
            inner.span,
            "lower a scalar anonymous enum literal",
        ) else {
            return MastExprKind::Trap;
        };

        let Some(variant_name) =
            self.require_identifier(inner, "lower a scalar anonymous enum literal")
        else {
            return MastExprKind::Trap;
        };

        let Some(tag_value) = self.anon_enum_tag_value(&enum_def, variant_name) else {
            return self.lower_literal_ice(
                inner.span,
                "Kern ICE (Lowering): Anonymous enum variant not found during scalar lowering.",
            );
        };

        if enum_def
            .variants
            .iter()
            .all(|variant| variant.payload_ty.is_none())
        {
            MastExprKind::Integer(tag_value as u128)
        } else {
            let mono_id = self.instantiate_anon_enum(norm_ty);
            MastExprKind::DataInit {
                data_struct_id: mono_id,
                tag_value: tag_value as u128,
                payload: Box::new(MastExpr::new(TypeId::VOID, MastExprKind::Undef, inner.span)),
            }
        }
    }

    /// Helper: build a trait-object fat pointer.
    pub(crate) fn lower_trait_object_init(
        &mut self,
        inner: &Expr,
        subst_map: &HashMap<SymbolId, GenericArg>,
        target_ptr_ty: TypeId,
        trait_norm: TypeId,
        span: Span,
    ) -> MastExprKind {
        let l = self.lower_expr(inner, subst_map, None);
        let l_norm = self.ctx.type_registry.normalize(l.ty);
        let l_is_fat_pointer_value = match self.ctx.type_registry.get(l_norm).clone() {
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => matches!(
                self.ctx
                    .type_registry
                    .get(self.ctx.type_registry.normalize(elem)),
                TypeKind::TraitObject(..) | TypeKind::ClosureInterface { .. }
            ),
            _ => false,
        };

        let source_trait_norm = match self.ctx.type_registry.get(l_norm) {
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let elem_norm = self.ctx.type_registry.normalize(*elem);
                if matches!(
                    self.ctx.type_registry.get(elem_norm),
                    TypeKind::TraitObject(..)
                ) {
                    Some(elem_norm)
                } else {
                    None
                }
            }
            _ => None,
        };

        if let Some(source_trait_norm) = source_trait_norm
            && self.is_trait_object_upcast(source_trait_norm, trait_norm)
        {
            return self
                .lower_trait_object_upcast(l, target_ptr_ty, source_trait_norm, trait_norm, span)
                .kind;
        }

        let (data_ptr_expr, data_ptr_ty, receiver_ty) = if l_is_fat_pointer_value {
            let boxed_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
                is_mut: false,
                elem: l.ty,
            });
            (
                MastExpr::new(
                    boxed_ptr_ty,
                    MastExprKind::AddressOf(Box::new(l.clone())),
                    span,
                ),
                boxed_ptr_ty,
                l.ty,
            )
        } else {
            (l.clone(), l.ty, l.ty)
        };

        // Look up or synthesize the vtable.
        let vtable_id = self.get_or_create_vtable(data_ptr_ty, receiver_ty, trait_norm);
        let Some(meta_expr) = self.vtable_global_meta_expr(vtable_id, span) else {
            return MastExprKind::Trap;
        };

        // Build the low-level constructor payload.
        MastExprKind::ConstructFatPointer {
            data_ptr: Box::new(data_ptr_expr),
            meta: Box::new(meta_expr),
        }
    }

    pub(crate) fn lower_enum_literal(
        &mut self,
        variant_name: SymbolId,
        concrete_ty: TypeId,
    ) -> MastExprKind {
        let norm_ty = self.ctx.type_registry.normalize(concrete_ty);
        if let TypeKind::AnonymousEnum(enum_def) = self.ctx.type_registry.get(norm_ty).clone() {
            let Some(tag_value) = self.anon_enum_tag_value(&enum_def, variant_name) else {
                return self.lower_literal_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Lowering): Variant `{}` not found in anonymous enum literal resolution.",
                        self.ctx.resolve(variant_name)
                    ),
                );
            };

            if enum_def
                .variants
                .iter()
                .all(|variant| variant.payload_ty.is_none())
            {
                return MastExprKind::Integer(tag_value as u128);
            }

            let mono_id = self.instantiate_anon_enum(norm_ty);
            return MastExprKind::DataInit {
                data_struct_id: mono_id,
                tag_value: tag_value as u128,
                payload: Box::new(MastExpr::new(
                    TypeId::VOID,
                    MastExprKind::Undef,
                    Span::default(),
                )),
            };
        }

        let (def_id, gen_args) =
            if let TypeKind::Enum(id, args) = self.ctx.type_registry.get(norm_ty) {
                (*id, args.clone())
            } else {
                return self.lower_literal_ice(
                    Span::default(),
                    "Kern ICE (Lowering): Expected Enum type for enum literal.",
                );
            };

        let Some(data_def) =
            self.require_enum_def(def_id, Span::default(), "lower an enum literal")
        else {
            return MastExprKind::Trap;
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
                // Return the raw integer when possible; otherwise wrap it in `DataInit`.
                if self.is_pure_enum(&data_def) {
                    self.record_pure_enum_tag_ty(def_id, &gen_args);
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
        self.ctx.emit_ice(
            Span::default(),
            format!(
                "Kern ICE (Lowering): Variant `{}` not found in enum literal resolution.",
                self.ctx.resolve(variant_name)
            ),
        );
        MastExprKind::Trap
    }

    fn anon_enum_tag_value(
        &self,
        enum_def: &kernc_sema::ty::AnonymousEnum,
        variant_name: SymbolId,
    ) -> Option<i128> {
        self.anon_enum_variant_info(enum_def, variant_name)
            .map(|(_, tag_value)| tag_value as i128)
    }

    pub(crate) fn named_enum_variant_info(
        &mut self,
        def: &kernc_sema::def::EnumDef,
        variant_name: SymbolId,
        span: Span,
    ) -> Option<(usize, u128)> {
        let mut current_val: i128 = 0;

        for (idx, variant) in def.variants.iter().enumerate() {
            if let Some(v_expr) = &variant.value {
                let mut ce = ConstEvaluator::new(self.ctx);
                if let Ok(val) = ce.eval_math(v_expr) {
                    current_val = val;
                }
            }

            if variant.name == variant_name {
                return Some((idx, current_val as u128));
            }

            current_val += 1;
        }

        self.ctx.emit_ice(
            span,
            format!(
                "Kern ICE (Lowering): Variant `{}` not found in enum.",
                self.ctx.resolve(variant_name)
            ),
        );
        None
    }

    pub(crate) fn anon_enum_variant_info(
        &self,
        enum_def: &kernc_sema::ty::AnonymousEnum,
        variant_name: SymbolId,
    ) -> Option<(usize, u128)> {
        let mut current_val = 0;
        for (idx, variant) in enum_def.variants.iter().enumerate() {
            if let Some(value) = variant.explicit_value {
                current_val = value;
            }
            if variant.name == variant_name {
                return Some((idx, current_val as u128));
            }
            current_val += 1;
        }
        None
    }
}
