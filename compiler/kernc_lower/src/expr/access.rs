use super::Lowerer;
use std::collections::HashMap;

use kernc_ast::{Expr, PathAnchor};
use kernc_mast::*;
use kernc_sema::def::{Def, DefId};
use kernc_sema::scope::SymbolKind;
use kernc_sema::ty::{GenericArg, TypeId, TypeKind};
use kernc_utils::{Span, SymbolId};

pub(crate) struct LoweredIdentifier {
    pub(crate) kind: MastExprKind,
    pub(crate) is_local_binding: bool,
}

enum LocalResolvedValue {
    Value(MastExpr),
    Binding,
}

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    fn lower_const_generic_identifier(
        &mut self,
        name: SymbolId,
        subst_map: &HashMap<SymbolId, GenericArg>,
        span: Span,
    ) -> Option<MastExprKind> {
        let GenericArg::Const(value) = subst_map.get(&name).copied()? else {
            return None;
        };

        let kind = match value {
            kernc_sema::ty::ConstGeneric::Value(value) => match value.kind {
                kernc_sema::ty::ConstGenericValueKind::Int(value) => {
                    MastExprKind::Integer(value as u128)
                }
                kernc_sema::ty::ConstGenericValueKind::Bool(value) => MastExprKind::Bool(value),
            },
            other => self.lower_error_kind(
                span,
                format!(
                    "cannot lower unresolved const generic `{}` with value {:?}",
                    self.ctx.resolve(name),
                    other
                ),
            ),
        };

        Some(kind)
    }

    fn lower_access_error(&mut self, span: Span, message: impl Into<String>) -> MastExprKind {
        self.lower_error_kind(span, message)
    }

    fn anchored_start_scope(
        &mut self,
        anchor: PathAnchor,
        span: Span,
    ) -> Option<(DefId, kernc_sema::scope::ScopeId)> {
        let current_scope = self.ctx.scopes.current_scope_id()?;
        let Some(current_module) = self.ctx.module_for_scope(current_scope) else {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Lowering): could not determine current module for anchored path",
            );
            return None;
        };

        let target_module = match anchor {
            PathAnchor::Parent => {
                let Some(parent) = self.ctx.module_parent(current_module) else {
                    return None;
                };
                parent
            }
            PathAnchor::Package => self.ctx.module_root(current_module),
        };

        let Def::Module(module) = &self.ctx.defs[target_module.0 as usize] else {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Lowering): anchored path target is not a module",
            );
            return None;
        };

        Some((target_module, module.scope_id))
    }

    fn local_value_or_binding(&self, name: SymbolId) -> Option<LocalResolvedValue> {
        for scope_idx in (0..self.local_types.len()).rev() {
            if let Some(value) = self
                .local_value_forwardings
                .get(scope_idx)
                .and_then(|scope| scope.get(&name))
                .cloned()
            {
                return Some(LocalResolvedValue::Value(value));
            }

            if self.local_types[scope_idx].contains_key(&name) {
                return Some(LocalResolvedValue::Binding);
            }
        }

        None
    }

    pub(crate) fn lower_identifier_with_locality(
        &mut self,
        expr_id: kernc_utils::NodeId,
        name: SymbolId,
        subst_map: &HashMap<SymbolId, GenericArg>,
    ) -> LoweredIdentifier {
        let name = self.measure_phase("          lower_ident_copy_source", |this| {
            this.identifier_copy_source(expr_id).unwrap_or(name)
        });
        let name = self.measure_phase("          lower_ident_forward_local", |this| {
            this.resolve_forwarded_local(name)
        });
        if let Some(local_value) = self
            .measure_phase("          lower_ident_local_lookup", |this| {
                this.local_value_or_binding(name)
            })
        {
            return match local_value {
                LocalResolvedValue::Value(value) => LoweredIdentifier {
                    kind: value.kind,
                    is_local_binding: false,
                },
                LocalResolvedValue::Binding => LoweredIdentifier {
                    kind: MastExprKind::Var(name),
                    is_local_binding: true,
                },
            };
        }

        if let Some(kind) = self.measure_phase("          lower_ident_const_param", |this| {
            this.lower_const_generic_identifier(name, subst_map, Span::default())
        }) {
            return LoweredIdentifier {
                kind,
                is_local_binding: false,
            };
        }

        let resolved_info = self.measure_phase("          lower_ident_scope_resolve", |this| {
            this.ctx.scopes.resolve(name).cloned()
        });

        // Kern constants are compile-time values, not addressable storage.
        if let Some(info) = resolved_info.as_ref()
            && info.kind == SymbolKind::Const
            && let Some(def_id) = info.def_id
        {
            let Some(expr) = self.measure_phase("          lower_ident_inline_const", |this| {
                this.lower_const_global_value_expr(def_id, Span::default())
            }) else {
                let const_name = self.ctx.resolve(name).to_string();
                return LoweredIdentifier {
                    kind: self.lower_error_kind(
                        Span::default(),
                        format!(
                            "constant `{}` could not be evaluated at compile time",
                            const_name
                        ),
                    ),
                    is_local_binding: false,
                };
            };
            return LoweredIdentifier {
                kind: expr.kind,
                is_local_binding: false,
            };
        }

        // First check whether this resolves to a top-level global.
        if let Some(info) = resolved_info
            && matches!(info.kind, SymbolKind::Static)
            && let Some(def_id) = info.def_id
            && let Some(mono_id) = self.measure_phase("          lower_ident_global_ref", |this| {
                this.ensure_global_lowered(def_id);
                this.global_map.get(&def_id).copied()
            })
        {
            return LoweredIdentifier {
                kind: MastExprKind::GlobalRef(mono_id),
                is_local_binding: false,
            };
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
            return LoweredIdentifier {
                kind: MastExprKind::GlobalRef(mono_id),
                is_local_binding: false,
            };
        }

        // Function references were already intercepted in `mod.rs`, so this must be a normal local binding.
        LoweredIdentifier {
            kind: MastExprKind::Var(name),
            is_local_binding: false,
        }
    }

    pub(crate) fn lower_identifier(
        &mut self,
        expr_id: kernc_utils::NodeId,
        name: SymbolId,
        subst_map: &HashMap<SymbolId, GenericArg>,
    ) -> MastExprKind {
        self.lower_identifier_with_locality(expr_id, name, subst_map)
            .kind
    }

    pub(crate) fn lower_anchored_identifier(
        &mut self,
        anchor: PathAnchor,
        name: SymbolId,
        span: Span,
    ) -> MastExprKind {
        let Some((_owner_module, start_scope)) = self.anchored_start_scope(anchor, span) else {
            return MastExprKind::Trap;
        };

        let resolved_info = self
            .ctx
            .scopes
            .resolve_value_in(start_scope, name)
            .or_else(|| self.ctx.scopes.resolve_namespace_in(start_scope, name))
            .cloned();

        let Some(info) = resolved_info else {
            return self.lower_access_error(
                span,
                format!(
                    "cannot find `{}` in the anchored module path",
                    self.ctx.resolve(name)
                ),
            );
        };

        match info.kind {
            SymbolKind::Const => {
                if let Some(def_id) = info.def_id {
                    if let Some(expr) = self.lower_const_global_value_expr(def_id, span) {
                        return expr.kind;
                    }
                    return self.lower_access_error(
                        span,
                        format!(
                            "constant `{}` could not be evaluated at compile time",
                            self.ctx.resolve(name)
                        ),
                    );
                }
            }
            SymbolKind::Static | SymbolKind::Function => {
                if let Some(def_id) = info.def_id {
                    self.ensure_global_lowered(def_id);
                    if let Some(&mono_id) = self.global_map.get(&def_id) {
                        return MastExprKind::GlobalRef(mono_id);
                    }
                    return self.lower_access_error(
                        span,
                        format!(
                            "module member `{}` could not be lowered because no instantiated value was available",
                            self.ctx.resolve(name)
                        ),
                    );
                }
            }
            SymbolKind::Module => return MastExprKind::Var(name),
            _ => {}
        }

        self.lower_access_error(
            span,
            format!(
                "module member `{}` of kind `{:?}` cannot be used as a runtime value",
                self.ctx.resolve(name),
                info.kind
            ),
        )
    }

    pub(crate) fn lower_field_access(
        &mut self,
        lhs: &Expr,
        field: SymbolId,
        subst_map: &HashMap<SymbolId, GenericArg>,
        span: Span,
    ) -> MastExprKind {
        let expr_ty = self.ctx.node_type(lhs.id).unwrap_or(TypeId::ERROR);
        let expr_ty = self.substitute_type_with_map(expr_ty, subst_map);
        let norm_expr = self.normalize_concrete_type(expr_ty);

        if matches!(
            self.ctx.type_registry.get(norm_expr),
            TypeKind::FnDef(..) | TypeKind::Function { .. }
        ) {
            return self.lower_access_error(
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
            let norm = self.normalize_concrete_type(base_ty);
            match self.ctx.type_registry.get(norm) {
                TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                    base_ty = *elem;
                    deref_expr =
                        MastExpr::new(base_ty, MastExprKind::Deref(Box::new(deref_expr)), span);
                }
                _ => break,
            }
        }

        let norm_base = self.normalize_concrete_type(base_ty);

        if let TypeKind::Module(mod_def_id) = self.ctx.type_registry.get(norm_base).clone() {
            let mod_scope = match &self.ctx.defs[mod_def_id.0 as usize] {
                Def::Module(m) => m.scope_id,
                _ => {
                    self.ctx.emit_ice(
                        span,
                        "Kern ICE (Lowering): Expected Module Def, found something else.",
                    );
                    return MastExprKind::Trap;
                }
            };

            let target_info = match self.ctx.scopes.resolve_in(mod_scope, field).cloned() {
                Some(info) => info,
                None => {
                    return self.lower_access_error(
                        span,
                        format!("module member `{}` is undefined", self.ctx.resolve(field)),
                    );
                }
            };

            match target_info.kind {
                SymbolKind::Module => {
                    return MastExprKind::Var(field);
                }
                SymbolKind::Const => {
                    if let Some(def_id) = target_info.def_id {
                        if let Some(expr) = self.lower_const_global_value_expr(def_id, span) {
                            return expr.kind;
                        }
                        let field_name = self.ctx.resolve(field);
                        return self.lower_access_error(
                            span,
                            format!(
                                "constant `{}` could not be evaluated at compile time",
                                field_name
                            ),
                        );
                    } else {
                        let field_name = self.ctx.resolve(field);
                        return self.lower_access_error(
                            span,
                            format!(
                                "cross-module constant `{}` could not be lowered because it has no global definition",
                                field_name
                            ),
                        );
                    }
                }
                SymbolKind::Static | SymbolKind::Function => {
                    if let Some(def_id) = target_info.def_id {
                        self.ensure_global_lowered(def_id);
                        if let Some(&mono_id) = self.global_map.get(&def_id) {
                            return MastExprKind::GlobalRef(mono_id);
                        }
                        return self.lower_access_error(
                            span,
                            format!(
                                "module member `{}` could not be lowered because no instantiated value was available",
                                self.ctx.resolve(field)
                            ),
                        );
                    } else {
                        return self.lower_access_error(
                            span,
                            format!(
                                "module member `{}` cannot be used as a value because it has no definition backing it",
                                self.ctx.resolve(field)
                            ),
                        );
                    }
                }
                _ => {
                    return self.lower_access_error(
                        span,
                        format!(
                            "module member `{}` of kind `{:?}` cannot be used as a runtime value",
                            self.ctx.resolve(field),
                            target_info.kind
                        ),
                    );
                }
            }
        }

        let Some(field_idx) = self.get_physical_field_index(base_ty, field, span) else {
            return MastExprKind::Trap;
        };

        let struct_id = match self.ctx.type_registry.get(norm_base).clone() {
            TypeKind::Def(def_id, gen_args) => self.instantiate_struct(def_id, &gen_args),
            TypeKind::AnonymousStruct(..) => self.instantiate_anon_struct(norm_base),
            TypeKind::AnonymousUnion(..) => self.instantiate_anon_union(norm_base),
            TypeKind::Range { .. } => self.instantiate_range_struct(norm_base),
            _ => {
                return self.lower_access_error(
                    span,
                    format!(
                        "cannot access field `{}` on base type `{:?}`",
                        self.ctx.resolve(field),
                        norm_base
                    ),
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
        subst_map: &HashMap<SymbolId, GenericArg>,
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
    ) -> Option<usize> {
        let norm = self.normalize_concrete_type(struct_ty);
        let cache_key = (norm, field_name);
        if let Some(&field_idx) = self.field_index_cache.get(&cache_key) {
            return Some(field_idx);
        }

        if let TypeKind::Def(def_id, gen_args) = self.ctx.type_registry.get(norm).clone() {
            if let Def::Struct(s) = &self.ctx.defs[def_id.0 as usize] {
                let ast_idx = match s.fields.iter().position(|f| f.name == field_name) {
                    Some(idx) => idx,
                    None => {
                        self.ctx
                            .struct_error(
                                span,
                                format!(
                                    "field `{}` not found in struct",
                                    self.ctx.resolve(field_name)
                                ),
                            )
                            .emit();
                        return None;
                    }
                };
                let (ast_to_physical, _) = self.cached_named_struct_mapping(def_id, &gen_args);
                // A poisoned layout cache must stop lowering here. Reusing field 0 would silently
                // miscompile the access and turn an internal bug into user-visible memory unsoundness.
                let Some(field_idx) = ast_to_physical.get(ast_idx).copied() else {
                    self.ctx.emit_ice(
                        span,
                        format!(
                            "Kern ICE (Lowering): Physical field mapping missing index {} for `{}`.",
                            ast_idx,
                            self.ctx.resolve(field_name)
                        ),
                    );
                    return None;
                };
                self.field_index_cache.insert(cache_key, field_idx);
                return Some(field_idx);
            } else if let Def::Union(u) = &self.ctx.defs[def_id.0 as usize] {
                let field_idx = match u.fields.iter().position(|f| f.name == field_name) {
                    Some(idx) => idx,
                    None => {
                        self.ctx
                            .struct_error(
                                span,
                                format!(
                                    "field `{}` not found in union",
                                    self.ctx.resolve(field_name)
                                ),
                            )
                            .emit();
                        return None;
                    }
                };
                self.field_index_cache.insert(cache_key, field_idx);
                return Some(field_idx);
            }
        }

        if let TypeKind::AnonymousStruct(is_extern, ref fields) =
            self.ctx.type_registry.get(norm).clone()
        {
            let Some(ast_idx) = fields.iter().position(|f| f.name == field_name) else {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "field `{}` not found in anonymous struct",
                            self.ctx.resolve(field_name)
                        ),
                    )
                    .emit();
                return None;
            };
            let (ast_to_physical, _) = self.cached_anon_struct_mapping(norm, is_extern, fields);
            // Same rule as named structs: never invent a fallback physical slot after cache
            // corruption, because that would target the wrong field and hide the real compiler bug.
            let Some(field_idx) = ast_to_physical.get(ast_idx).copied() else {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Lowering): Physical field mapping missing index {} for anonymous field `{}`.",
                        ast_idx,
                        self.ctx.resolve(field_name)
                    ),
                );
                return None;
            };
            self.field_index_cache.insert(cache_key, field_idx);
            return Some(field_idx);
        }

        if let TypeKind::AnonymousUnion(_, ref fields) = self.ctx.type_registry.get(norm).clone() {
            let Some(field_idx) = fields.iter().position(|f| f.name == field_name) else {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "field `{}` not found in anonymous union",
                            self.ctx.resolve(field_name)
                        ),
                    )
                    .emit();
                return None;
            };
            self.field_index_cache.insert(cache_key, field_idx);
            return Some(field_idx);
        }

        if let TypeKind::Range { start, end, .. } = self.ctx.type_registry.get(norm).clone() {
            let start_name = self.ctx.intern("start");
            let end_name = self.ctx.intern("end");
            let field_idx = if field_name == start_name {
                start.map(|_| 0)
            } else if field_name == end_name {
                end.map(|_| if start.is_some() { 1 } else { 0 })
            } else {
                None
            };
            if let Some(field_idx) = field_idx {
                self.field_index_cache.insert(cache_key, field_idx);
                return Some(field_idx);
            }
        }

        self.ctx
            .struct_error(
                span,
                format!(
                    "cannot compute field index for `{}` on type `{:?}`",
                    self.ctx.resolve(field_name),
                    self.ctx.type_registry.get(norm)
                ),
            )
            .emit();
        None
    }
}
