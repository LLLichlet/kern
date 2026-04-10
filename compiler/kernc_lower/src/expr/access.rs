use super::Lowerer;
use std::collections::HashMap;

use kernc_ast::Expr;
use kernc_mast::*;
use kernc_sema::checker::{ConstEvaluator, ConstValue};
use kernc_sema::def::Def;
use kernc_sema::scope::SymbolKind;
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{Span, SymbolId};

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    fn lower_access_ice(&mut self, span: Span, message: impl Into<String>) -> MastExprKind {
        self.ctx.emit_ice(span, message);
        MastExprKind::Trap
    }

    pub(crate) fn lower_identifier(
        &mut self,
        expr_id: kernc_utils::NodeId,
        name: SymbolId,
    ) -> MastExprKind {
        let name = self.measure_phase("          lower_ident_copy_source", |this| {
            this.identifier_copy_source(expr_id).unwrap_or(name)
        });
        let name = self.measure_phase("          lower_ident_forward_local", |this| {
            this.resolve_forwarded_local(name)
        });
        if let Some(value) = self.measure_phase("          lower_ident_forward_value", |this| {
            this.forwarded_local_value(name)
        }) {
            return value.kind;
        }

        if self.measure_phase("          lower_ident_has_local", |this| {
            this.has_local_binding(name)
        }) {
            return MastExprKind::Var(name);
        }

        let resolved_info = self.measure_phase("          lower_ident_scope_resolve", |this| {
            this.ctx.scopes.resolve(name).cloned()
        });

        // Inline constant values when possible.
        if let Some(info) = resolved_info.as_ref()
            && info.kind == SymbolKind::Const
            && let Some(def_id) = info.def_id
        {
            if let Some(kind) = self.measure_phase("          lower_ident_inline_const", |this| {
                let const_expr_opt = if let Def::Global(g) = &this.ctx.defs[def_id.0 as usize] {
                    Some(g.value.clone())
                } else {
                    None
                };

                let Some(const_expr) = const_expr_opt else {
                    return None;
                };

                let prev_scope = this.ctx.scopes.current_scope_id();
                if let Some(owner_scope) = this.global_owner_scope(def_id) {
                    this.ctx.scopes.set_current_scope(owner_scope);
                }

                let lowered_kind = {
                    let mut ce = ConstEvaluator::new(this.ctx);
                    if let Ok(val) = ce.eval_inner(&const_expr, 0) {
                        match val {
                            ConstValue::Int(v) => Some(MastExprKind::Integer(v as u128)),
                            ConstValue::Float(f) => Some(MastExprKind::Float(f)),
                            ConstValue::Bool(b) => Some(MastExprKind::Bool(b)),
                            ConstValue::String(s) => {
                                Some(this.lower_string_literal(&s, const_expr.span))
                            }
                            _ => None,
                        }
                    } else {
                        None
                    }
                };

                if let Some(prev_scope) = prev_scope {
                    this.ctx.scopes.set_current_scope(prev_scope);
                }

                lowered_kind
            }) {
                return kind;
            }
        }

        // First check whether this resolves to a top-level global.
        if let Some(info) = resolved_info
            && matches!(info.kind, SymbolKind::Const | SymbolKind::Static)
            && let Some(def_id) = info.def_id
        {
            if let Some(mono_id) = self.measure_phase("          lower_ident_global_ref", |this| {
                this.ensure_global_lowered(def_id);
                this.global_map.get(&def_id).copied()
            }) {
                return MastExprKind::GlobalRef(mono_id);
            }
        }

        // Then check for a local-scope static.
        if let Some(mono_id) = self.measure_phase("          lower_ident_local_static", |this| {
            for scope in this.local_statics.iter().rev() {
                if let Some(&mono_id) = scope.get(&name) {
                    return Some(mono_id);
                }
            }
            None
        }) {
            return MastExprKind::GlobalRef(mono_id);
        }

        // Function references were already intercepted in `mod.rs`, so this must be a normal local binding.
        MastExprKind::Var(name)
    }

    pub(crate) fn lower_field_access(
        &mut self,
        lhs: &Expr,
        field: SymbolId,
        subst_map: &HashMap<SymbolId, TypeId>,
        span: Span,
    ) -> MastExprKind {
        let expr_ty = self
            .ctx
            .node_types
            .get(&lhs.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let norm_expr = self.ctx.type_registry.normalize(expr_ty);

        if matches!(
            self.ctx.type_registry.get(norm_expr),
            TypeKind::FnDef(..) | TypeKind::Function { .. }
        ) {
            return self.lower_access_ice(
                span,
                format!(
                    "Attempted to access method `{}` without calling it. Bound Methods are not supported in Kern.",
                    self.ctx.resolve(field)
                ),
            );
        }

        if matches!(
            self.ctx.type_registry.get(norm_expr),
            TypeKind::Enum(..) | TypeKind::AnonymousEnum(_)
        ) {
            return self.lower_enum_literal(field, expr_ty);
        }

        let l = self.lower_expr(lhs, subst_map, None);
        let mut base_ty = l.ty;
        let mut deref_expr = l.clone();

        loop {
            let norm = self.ctx.type_registry.normalize(base_ty);
            match self.ctx.type_registry.get(norm) {
                TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                    base_ty = *elem;
                    deref_expr =
                        MastExpr::new(base_ty, MastExprKind::Deref(Box::new(deref_expr)), span);
                }
                _ => break,
            }
        }

        let norm_base = self.ctx.type_registry.normalize(base_ty);

        if let TypeKind::Module(mod_def_id) = self.ctx.type_registry.get(norm_base).clone() {
            let mod_scope = match &self.ctx.defs[mod_def_id.0 as usize] {
                Def::Module(m) => m.scope_id,
                _ => {
                    return self.lower_access_ice(
                        span,
                        "Kern ICE (Lowering): Expected Module Def, found something else.",
                    );
                }
            };

            let target_info = match self.ctx.scopes.resolve_in(mod_scope, field).cloned() {
                Some(info) => info,
                None => {
                    return self.lower_access_ice(
                        span,
                        format!(
                            "Kern ICE (Lowering): Module field `{}` is undefined. Sema should have caught this.",
                            self.ctx.resolve(field)
                        ),
                    );
                }
            };

            match target_info.kind {
                SymbolKind::Module => {
                    return MastExprKind::Var(field);
                }
                SymbolKind::Const => {
                    if let Some(def_id) = target_info.def_id {
                        let const_expr_opt =
                            if let Def::Global(g) = &self.ctx.defs[def_id.0 as usize] {
                                Some(g.value.clone())
                            } else {
                                None
                            };

                        if let Some(const_expr) = const_expr_opt {
                            let prev_scope = self.ctx.scopes.current_scope_id();
                            if let Some(owner_scope) = self.global_owner_scope(def_id) {
                                self.ctx.scopes.set_current_scope(owner_scope);
                            }

                            let lowered_kind = {
                                let mut ce = ConstEvaluator::new(self.ctx);
                                if let Ok(val) = ce.eval_inner(&const_expr, 0) {
                                    match val {
                                        ConstValue::Int(v) => {
                                            Some(MastExprKind::Integer(v as u128))
                                        }
                                        ConstValue::Float(f) => Some(MastExprKind::Float(f)),
                                        ConstValue::Bool(b) => Some(MastExprKind::Bool(b)),
                                        ConstValue::String(s) => {
                                            Some(self.lower_string_literal(&s, const_expr.span))
                                        }
                                        _ => None,
                                    }
                                } else {
                                    None
                                }
                            };

                            if let Some(prev_scope) = prev_scope {
                                self.ctx.scopes.set_current_scope(prev_scope);
                            }

                            if let Some(kind) = lowered_kind {
                                return kind;
                            }
                        }
                    }

                    // Fall back to the global map when the value cannot be inlined.
                    if let Some(def_id) = target_info.def_id {
                        self.ensure_global_lowered(def_id);
                        if let Some(&mono_id) = self.global_map.get(&def_id) {
                            return MastExprKind::GlobalRef(mono_id);
                        }
                    } else {
                        let field_name = self.ctx.resolve(field);
                        return self.lower_access_ice(
                            span,
                            format!("Kern ICE (Lowering): Cross-module constant `{}` could not be inlined, and its global definition was not found. Phase 1 global collection failed.", field_name)
                        );
                    }
                }
                SymbolKind::Static | SymbolKind::Function => {
                    if let Some(def_id) = target_info.def_id {
                        self.ensure_global_lowered(def_id);
                        if let Some(&mono_id) = self.global_map.get(&def_id) {
                            return MastExprKind::GlobalRef(mono_id);
                        }
                        return self.lower_access_ice(
                            span,
                            format!(
                                "Kern ICE (Lowering): Symbol `{}` found but not instantiated.",
                                self.ctx.resolve(field)
                            ),
                        );
                    } else {
                        return self.lower_access_ice(
                            span,
                            format!(
                                "Kern ICE (Lowering): Symbol `{}` lacks a def_id.",
                                self.ctx.resolve(field)
                            ),
                        );
                    }
                }
                _ => {
                    return self.lower_access_ice(
                        span,
                        format!(
                            "Kern ICE (Lowering): Unsupported symbol kind in module: {:?}",
                            target_info.kind
                        ),
                    );
                }
            }
        }

        let field_idx = self.get_physical_field_index(base_ty, field, span);

        let struct_id = match self.ctx.type_registry.get(norm_base).clone() {
            TypeKind::Def(def_id, gen_args) => self.instantiate_struct(def_id, &gen_args),
            TypeKind::AnonymousStruct(..) => self.instantiate_anon_struct(norm_base),
            TypeKind::AnonymousUnion(..) => self.instantiate_anon_union(norm_base),
            _ => {
                return self.lower_access_ice(
                    span,
                    format!("Kern ICE (Lowering): Attempted to access field `{}` on an invalid base type: {:?}", self.ctx.resolve(field), norm_base)
                );
            }
        };

        MastExprKind::FieldAccess {
            lhs: Box::new(deref_expr),
            struct_id,
            field_idx,
        }
    }

    pub(crate) fn lower_index_access(
        &mut self,
        lhs: &Expr,
        index: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> MastExprKind {
        let l = self.lower_expr(lhs, subst_map, None);
        let idx = self.lower_expr(index, subst_map, Some(TypeId::USIZE));
        MastExprKind::IndexAccess {
            lhs: Box::new(l),
            index: Box::new(idx),
        }
    }

    pub(crate) fn get_physical_field_index(
        &mut self,
        struct_ty: TypeId,
        field_name: SymbolId,
        span: Span,
    ) -> usize {
        let norm = self.ctx.type_registry.normalize(struct_ty);
        let cache_key = (norm, field_name);
        if let Some(&field_idx) = self.field_index_cache.get(&cache_key) {
            return field_idx;
        }

        if let TypeKind::Def(def_id, gen_args) = self.ctx.type_registry.get(norm).clone() {
            if let Def::Struct(s) = &self.ctx.defs[def_id.0 as usize] {
                let ast_idx = match s.fields.iter().position(|f| f.name == field_name) {
                    Some(idx) => idx,
                    None => {
                        self.ctx.emit_ice(
                            span,
                            format!(
                                "Kern ICE (Lowering): Field `{}` not found in struct",
                                self.ctx.resolve(field_name)
                            ),
                        );
                        return 0;
                    }
                };
                let (ast_to_physical, _) = self.cached_named_struct_mapping(def_id, &gen_args);
                let field_idx = ast_to_physical.get(ast_idx).copied().unwrap_or_else(|| {
                    self.ctx.emit_ice(
                        span,
                        format!(
                            "Kern ICE (Lowering): Physical field mapping missing index {} for `{}`.",
                            ast_idx,
                            self.ctx.resolve(field_name)
                        ),
                    );
                    0
                });
                self.field_index_cache.insert(cache_key, field_idx);
                return field_idx;
            } else if let Def::Union(u) = &self.ctx.defs[def_id.0 as usize] {
                let field_idx = match u.fields.iter().position(|f| f.name == field_name) {
                    Some(idx) => idx,
                    None => {
                        self.ctx
                            .emit_ice(span, "Kern ICE: Field not found in union".to_string());
                        0
                    }
                };
                self.field_index_cache.insert(cache_key, field_idx);
                return field_idx;
            }
        }

        if let TypeKind::AnonymousStruct(is_extern, ref fields) =
            self.ctx.type_registry.get(norm).clone()
        {
            let Some(ast_idx) = fields.iter().position(|f| f.name == field_name) else {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Lowering): Field `{}` not found in anonymous struct.",
                        self.ctx.resolve(field_name)
                    ),
                );
                return 0;
            };
            let (ast_to_physical, _) =
                self.cached_anon_struct_mapping(norm, is_extern, fields);
            let field_idx = ast_to_physical.get(ast_idx).copied().unwrap_or_else(|| {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Lowering): Physical field mapping missing index {} for anonymous field `{}`.",
                        ast_idx,
                        self.ctx.resolve(field_name)
                    ),
                );
                0
            });
            self.field_index_cache.insert(cache_key, field_idx);
            return field_idx;
        }

        if let TypeKind::AnonymousUnion(_, ref fields) = self.ctx.type_registry.get(norm).clone() {
            let field_idx = fields
                .iter()
                .position(|f| f.name == field_name)
                .unwrap_or(0);
            self.field_index_cache.insert(cache_key, field_idx);
            return field_idx;
        }

        self.ctx.emit_ice(
            span,
            format!(
                "Kern ICE (Lowering): Failed to compute physical field index for `{}` on type {:?}.",
                self.ctx.resolve(field_name),
                self.ctx.type_registry.get(norm)
            ),
        );
        0
    }
}
