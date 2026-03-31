use super::ExprChecker;
use crate::def::{Def, DefId};
use crate::passes::TypeResolver;
use crate::query::{MemberQuery, MemberQueryEnv};
use crate::scope::{SymbolInfo, SymbolKind};
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{self as ast, Expr};
use kernc_utils::{NodeId, Span, SymbolId};
use crate::checker::Substituter;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    fn current_module_id(&self) -> Option<DefId> {
        let current_scope = self.ctx.scopes.current_scope_id()?;

        self.ctx
            .defs
            .iter()
            .filter_map(|def| {
                let Def::Module(module) = def else {
                    return None;
                };

                self.ctx
                    .scopes
                    .distance_to_ancestor(current_scope, module.scope_id)
                    .map(|distance| (module.id, distance))
            })
            .min_by_key(|(_, distance)| *distance)
            .map(|(module_id, _)| module_id)
    }

    fn global_owner_scope(&self, def_id: DefId) -> Option<crate::scope::ScopeId> {
        self.ctx.defs.iter().find_map(|def| {
            let Def::Module(module) = def else {
                return None;
            };

            if module.items.contains(&def_id) {
                Some(module.scope_id)
            } else {
                None
            }
        })
    }

    pub(crate) fn check_identifier(&mut self, name: SymbolId, span: Span) -> TypeId {
        if let Some(info) = self.ctx.scopes.resolve(name).cloned() {
            self.ctx.record_identifier_reference(span, info.span);

            if info.kind == SymbolKind::Function {
                return self
                    .ctx
                    .type_registry
                    .intern(TypeKind::FnDef(info.def_id.unwrap(), vec![]));
            }
            // 濡傛灉鏄ā鍧楋紝鏄惧紡鍖呰骞惰繑鍥?TypeKind::Module
            if info.kind == SymbolKind::Module {
                return self
                    .ctx
                    .type_registry
                    .intern(TypeKind::Module(info.def_id.unwrap()));
            }

            // 澶勭悊 `use` 瀵煎叆鎴栦贡搴忓０鏄庣殑甯搁噺/闈欐€佸彉閲忕殑鎸夐渶鎺ㄥ
            if info.type_id == TypeId::ERROR
                && let Some(def_id) = info.def_id
            {
                let global_expr_opt = if let Def::Global(g) = &self.ctx.defs[def_id.0 as usize] {
                    Some(g.value.clone())
                } else {
                    None
                };

                if let Some(g_expr) = global_expr_opt {
                    if let Some(&actual_ty) = self.ctx.node_types.get(&g_expr.id) {
                        return actual_ty;
                    }
                    let prev_scope = self.ctx.scopes.current_scope_id();
                    if let Some(owner_scope) = self.global_owner_scope(def_id) {
                        self.ctx.scopes.set_current_scope(owner_scope);
                    }
                    let computed_ty = self.check_expr(&g_expr, None);
                    if let Some(prev_scope) = prev_scope {
                        self.ctx.scopes.set_current_scope(prev_scope);
                    }
                    return computed_ty;
                }
            }

            info.type_id
        } else {
            let name_str = self.ctx.resolve(name).to_string();
            self.ctx
                .struct_error(span, format!("use of undeclared identifier `{}`", name_str))
                .with_hint("make sure the variable or function is defined before using it")
                .emit();
            TypeId::ERROR
        }
    }

    pub(crate) fn check_self_value(&mut self, span: Span) -> TypeId {
        let self_var = self.ctx.intern("self");
        let self_type = self.ctx.intern("Self");

        if let Some(info) = self.ctx.scopes.resolve(self_var) {
            info.type_id
        } else if let Some(info) = self.ctx.scopes.resolve(self_type) {
            info.type_id
        } else {
            self.ctx
                .struct_error(span, "`self` is not available in this context")
                .with_hint("the `self` keyword is only valid inside method implementations")
                .emit();
            TypeId::ERROR
        }
    }

    pub(crate) fn check_let(
        &mut self,
        node_id: NodeId,
        pattern: &ast::LetPattern,
        init: &Expr,
        else_branch: Option<&Expr>,
        expected_ty: Option<TypeId>,
        span: Span,
    ) -> TypeId {
        let init_ty = self.check_expr(init, expected_ty);
        let norm_init = self.resolve_tv(init_ty);
        if matches!(
            self.ctx.type_registry.get(norm_init),
            TypeKind::TraitObject(..)
        ) {
            self.ctx
                .struct_error(span, "cannot store a naked trait object in a variable")
                .with_hint(
                    "trait objects are dynamically sized; store a pointer (`*mut Trait`) instead",
                )
                .emit();
        }

        match &pattern.kind {
            ast::LetPatternKind::Binding(binding) => {
                if else_branch.is_some() {
                    self.ctx
                        .struct_error(span, "irrefutable `let` bindings cannot use `else`")
                        .with_hint("remove the `else` block or use a refutable variant pattern like `.Ok: value`")
                        .emit();
                }

                let info = SymbolInfo {
                    kind: SymbolKind::Var,
                    node_id,
                    type_id: init_ty,
                    def_id: None,
                    span: binding.span,
                    is_pub: false,
                    is_mut: binding.is_mut,
                };
                let _ = self.ctx.scopes.define(binding.name, info);
            }
            ast::LetPatternKind::Variant(variant) => {
                let Some(else_expr) = else_branch else {
                    self.ctx
                        .struct_error(span, "refutable `let` patterns require an `else` branch")
                        .with_hint(
                            "write this as `let .Variant: value = expr else return ...;` or another diverging expression",
                        )
                        .emit();
                    return TypeId::VOID;
                };

                if let Some(explicit_ty_ast) = &variant.target_type {
                    let mut resolver = TypeResolver::new(self.ctx);
                    let scope = resolver.current_scope_id().unwrap();
                    let explicit_ty = resolver.resolve_type(explicit_ty_ast, scope);

                    let mut map = std::collections::HashMap::new();
                    if !self.unify(norm_init, explicit_ty, &mut map) && norm_init != explicit_ty {
                        self.emit_mismatch_error(span, norm_init, explicit_ty);
                    }
                }

                let mut payload_binding_ty = None;
                match self.ctx.type_registry.get(norm_init).clone() {
                    TypeKind::Enum(def_id, generic_args) => {
                        let Some(adt_def) =
                            self.match_enum_def(def_id, span, "check a let-pattern binding")
                        else {
                            return TypeId::VOID;
                        };

                        if let Some(v) = adt_def
                            .variants
                            .iter()
                            .find(|v| v.name == variant.variant_name)
                        {
                            if let Some(bind_pattern) = &variant.binding {
                                if let Some(payload_ast) = &v.payload_type {
                                    let mut payload_ty = self
                                        .ctx
                                        .node_types
                                        .get(&payload_ast.id)
                                        .copied()
                                        .unwrap_or(TypeId::ERROR);

                                    if !adt_def.generics.is_empty() && !generic_args.is_empty() {
                                        let mut map = std::collections::HashMap::new();
                                        for (i, param) in adt_def.generics.iter().enumerate() {
                                            map.insert(param.name, generic_args[i]);
                                        }
                                        let mut subst =
                                            Substituter::new(&mut self.ctx.type_registry, &map);
                                        payload_ty = subst.substitute(payload_ty);
                                    }

                                    payload_binding_ty = Some((bind_pattern, payload_ty));
                                } else {
                                    self.ctx
                                        .struct_error(
                                            span,
                                            format!(
                                                "variant `{}` has no payload",
                                                self.ctx.resolve(variant.variant_name)
                                            ),
                                        )
                                        .emit();
                                }
                            } else if v.payload_type.is_some() {
                                self.ctx
                                    .struct_error(
                                        span,
                                        format!(
                                            "variant `{}` requires a binding for its payload",
                                            self.ctx.resolve(variant.variant_name)
                                        ),
                                    )
                                    .emit();
                            }
                        } else {
                            self.ctx
                                .struct_error(span, "variant not found in ADT")
                                .emit();
                        }
                    }
                    TypeKind::AnonymousEnum(enum_def) => {
                        if let Some(v) = enum_def
                            .variants
                            .iter()
                            .find(|v| v.name == variant.variant_name)
                        {
                            if let Some(bind_pattern) = &variant.binding {
                                if let Some(payload_ty) = v.payload_ty {
                                    payload_binding_ty = Some((bind_pattern, payload_ty));
                                } else {
                                    self.ctx
                                        .struct_error(
                                            span,
                                            format!(
                                                "variant `{}` has no payload",
                                                self.ctx.resolve(variant.variant_name)
                                            ),
                                        )
                                        .emit();
                                }
                            } else if v.payload_ty.is_some() {
                                self.ctx
                                    .struct_error(
                                        span,
                                        format!(
                                            "variant `{}` requires a binding for its payload",
                                            self.ctx.resolve(variant.variant_name)
                                        ),
                                    )
                                    .emit();
                            }
                        } else {
                            self.ctx
                                .struct_error(span, "variant not found in ADT")
                                .emit();
                        }
                    }
                    TypeKind::Error => {}
                    _ => {
                        self.ctx
                            .struct_error(
                                span,
                                "variant `let` patterns are only allowed on ADT values",
                            )
                            .emit();
                    }
                }

                let else_ty = self.check_expr(else_expr, None);
                let norm_else = self.resolve_tv(else_ty);
                if norm_else != TypeId::NEVER && norm_else != TypeId::ERROR {
                    self.ctx
                        .struct_error(
                            else_expr.span,
                            "`let ... else` failure branches must diverge",
                        )
                        .with_hint("end the `else` block with `return`, `break`, `continue`, or another diverging expression")
                        .emit();
                }

                if let Some((bind_pattern, payload_ty)) = payload_binding_ty {
                    let info = SymbolInfo {
                        kind: SymbolKind::Var,
                        node_id,
                        type_id: payload_ty,
                        def_id: None,
                        span: bind_pattern.span,
                        is_pub: false,
                        is_mut: bind_pattern.is_mut,
                    };
                    let _ = self.ctx.scopes.define(bind_pattern.name, info);
                }
            }
        }
        TypeId::VOID
    }

    pub(crate) fn check_static(
        &mut self,
        node_id: NodeId,
        pattern: &ast::BindingPattern,
        init: &Expr,
        expected_ty: Option<TypeId>,
        span: Span,
    ) -> TypeId {
        let init_ty = self.check_expr(init, expected_ty);
        let norm_init = self.resolve_tv(init_ty);
        if matches!(
            self.ctx.type_registry.get(norm_init),
            TypeKind::TraitObject(..)
        ) {
            self.ctx
                .struct_error(span, "cannot store a naked trait object in a variable")
                .with_hint(
                    "trait objects are dynamically sized; store a pointer (`*mut Trait`) instead",
                )
                .emit();
        }

        let info = SymbolInfo {
            kind: SymbolKind::Static,
            node_id,
            type_id: init_ty,
            def_id: None,
            span: pattern.span,
            is_pub: false,
            is_mut: pattern.is_mut,
        };
        let _ = self.ctx.scopes.define(pattern.name, info);

        TypeId::VOID
    }

    pub(crate) fn check_index_access(
        &mut self,
        lhs: &Expr,
        index: &Expr,
        is_mut: bool,
        span: Span,
    ) -> TypeId {
        if is_mut {
            self.ctx
                .struct_error(
                    span,
                    "mutable indexing `..[]` is not supported for single elements",
                )
                .with_hint(
                    "use standard indexing `.[]` instead. Mutability is inherited automatically.",
                )
                .emit();
        }

        let lhs_ty = self.check_expr(lhs, None);
        let idx_ty = self.check_expr(index, Some(TypeId::USIZE));

        let norm_idx = self.resolve_tv(idx_ty);
        if !self.ctx.type_registry.is_integer(norm_idx) && norm_idx != TypeId::ERROR {
            self.ctx
                .struct_error(index.span, "index must be an integer type")
                .emit();
        }

        let norm_lhs = self.resolve_tv(lhs_ty);
        match self.ctx.type_registry.get(norm_lhs).clone() {
            TypeKind::Array { elem, .. } | TypeKind::Slice { elem, .. } => elem,
            TypeKind::Error => TypeId::ERROR,
            _ => {
                self.ctx
                    .struct_error(lhs.span, "cannot index into a non-array/non-slice type")
                    .emit();
                TypeId::ERROR
            }
        }
    }

    pub(crate) fn check_field_access(
        &mut self,
        expr_id: NodeId,
        lhs: &Expr,
        field: SymbolId,
        span: Span,
    ) -> TypeId {
        let lhs_ty = self.check_expr(lhs, None);
        if lhs_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

        // 1. 鑾峰彇瑙ｅ紩鐢ㄥ悗鐨勫熀纭€绫诲瀷锛圫truct/Union/Enum/Module锛夛紝浠呯敤浜庢ā鍧楀垽瀹氬拰鏈€鍚庡厹搴曠殑瀛楁鏌ユ壘
        let base_norm = self.get_base_type(lhs_ty);

        // 2. 鍩轰簬绫诲瀷绯荤粺澶勭悊澶氱骇妯″潡璁块棶
        // 妯″潡涓嶄細鍖呰９鍦ㄦ寚閽堥噷锛屾墍浠ョ敤 base_norm 鏌ユ槸瀹夊叏鐨?
        if let TypeKind::Module(mod_def_id) = self.ctx.type_registry.get(base_norm).clone() {
            let mod_scope = if let Def::Module(m) = &self.ctx.defs[mod_def_id.0 as usize] {
                m.scope_id
            } else {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Typeck): Expected module definition during module field lookup for DefId {}.",
                        mod_def_id.0
                    ),
                );
                return TypeId::ERROR;
            };
            if let Some(target_info) = self.ctx.scopes.resolve_in(mod_scope, field) {
                let real_ty = if target_info.kind == SymbolKind::Function {
                    self.ctx
                        .type_registry
                        .intern(TypeKind::FnDef(target_info.def_id.unwrap(), vec![]))
                } else if target_info.kind == SymbolKind::Module {
                    self.ctx
                        .type_registry
                        .intern(TypeKind::Module(target_info.def_id.unwrap()))
                } else if target_info.type_id == TypeId::ERROR {
                    if let Some(def_id) = target_info.def_id {
                        let global_expr_opt =
                            if let Def::Global(g) = &self.ctx.defs[def_id.0 as usize] {
                                Some(g.value.clone())
                            } else {
                                None
                            };

                        if let Some(g_expr) = global_expr_opt {
                            if let Some(&actual_ty) = self.ctx.node_types.get(&g_expr.id) {
                                actual_ty
                            } else {
                                let prev_scope = self.ctx.scopes.current_scope_id();
                                if let Some(owner_scope) = self.global_owner_scope(def_id) {
                                    self.ctx.scopes.set_current_scope(owner_scope);
                                }
                                let computed_ty = self.check_expr(&g_expr, None);
                                if let Some(prev_scope) = prev_scope {
                                    self.ctx.scopes.set_current_scope(prev_scope);
                                }
                                computed_ty
                            }
                        } else {
                            target_info.type_id
                        }
                    } else {
                        target_info.type_id
                    }
                } else {
                    target_info.type_id
                };

                let mod_ty = self.ctx.type_registry.intern(TypeKind::Module(mod_def_id));
                self.ctx.node_types.insert(lhs.id, mod_ty);
                return real_ty;
            } else {
                let field_name = self.ctx.resolve(field);
                self.ctx
                    .struct_error(
                        span,
                        format!("module has no public member `{}`", field_name),
                    )
                    .emit();
                return TypeId::ERROR;
            }
        }

        if let Some((ty, owner_trait_ty)) = self.try_find_field_or_method_silent(lhs_ty, field, span)
        {
            if let Some(owner_trait_ty) = owner_trait_ty {
                self.ctx.trait_method_owners.insert(expr_id, owner_trait_ty);
            }
            return ty;
        }

        // 鍏ㄩ儴澶辫触锛屾姏鍑鸿缁嗚瘖鏂?
        let field_str = self.ctx.resolve(field);
        let lhs_str = self.ctx.ty_to_string(lhs_ty);

        self.ctx
            .struct_error(
                span,
                format!(
                    "no field or method named `{}` found on type `{}`",
                    field_str, lhs_str
                ),
            )
            .with_hint(
                "if this is a method, ensure the trait defining it is imported and implemented",
            )
            .with_hint("if this is a struct field, check for typos")
            .emit();

        TypeId::ERROR
    }

    /// 鍔╂墜 2.5锛氶潤榛樻煡鎵惧瓧娈垫垨鏂规硶 (涓嶆姤閿欙紝鏌ヤ笉鍒拌繑鍥?None)
    fn try_find_field_or_method_silent(
        &mut self,
        lhs_ty: TypeId,
        field: SymbolId,
        span: Span,
    ) -> Option<(TypeId, Option<TypeId>)> {
        let env = MemberQueryEnv::from_active_bounds(&self.ctx.active_bounds);
        let current_module_id = self.current_module_id();
        let mut query = MemberQuery::new(self.ctx);
        query
            .resolve_named_member(current_module_id, lhs_ty, field, &env, span)
            .map(|resolution| (resolution.candidate.type_id, resolution.owner_trait_ty))
    }

    pub(crate) fn check_slice_op(
        &mut self,
        lhs: &Expr,
        start: Option<&Expr>,
        end: Option<&Expr>,
        _is_inclusive: bool,
        is_mut: bool,
        span: Span,
    ) -> TypeId {
        let lhs_ty = self.check_expr(lhs, None);

        if let Some(s) = start {
            let s_ty = self.check_expr(s, Some(TypeId::USIZE));
            let s_ty_id = self.resolve_tv(s_ty);
            if !self.ctx.type_registry.is_integer(s_ty_id) {
                self.ctx
                    .struct_error(s.span, "slice start index must be an integer")
                    .emit();
            }
        }
        if let Some(e) = end {
            let e_ty = self.check_expr(e, Some(TypeId::USIZE));
            let e_ty_id = self.resolve_tv(e_ty);
            if !self.ctx.type_registry.is_integer(e_ty_id) {
                self.ctx
                    .struct_error(e.span, "slice end index must be an integer")
                    .emit();
            }
        }

        let norm_lhs = self.resolve_tv(lhs_ty);
        let base_allows_mut_slice = matches!(
            self.ctx.type_registry.get(norm_lhs).clone(),
            TypeKind::Pointer { is_mut: true, .. }
                | TypeKind::VolatilePtr { is_mut: true, .. }
                | TypeKind::Slice { is_mut: true, .. }
                | TypeKind::Array { is_mut: true, .. }
                | TypeKind::ArrayInfer { is_mut: true, .. }
        ) || self.is_lvalue_mutable(lhs);

        // 濡傛灉鏄?`..[`锛屽繀椤荤‘淇濈洰鏍囧唴瀛樺叿鏈夊彲鍙樻潈闄?
        if is_mut && !base_allows_mut_slice && lhs_ty != TypeId::ERROR {
            self.ctx
                .struct_error(
                    span,
                    "cannot create a mutable slice from an immutable location",
                )
                .with_hint("ensure the target is bound with `let mut` or is a mutable pointer")
                .emit();
        }

        match self.ctx.type_registry.get(norm_lhs).clone() {
            TypeKind::Array { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. } => self
                .ctx
                .type_registry
                .intern(TypeKind::Slice { is_mut, elem }),
            TypeKind::Error => TypeId::ERROR,
            _ => {
                self.ctx
                    .struct_error(lhs.span, "cannot slice a non-array/non-slice type")
                    .emit();
                TypeId::ERROR
            }
        }
    }

    /// 杈呭姪鏂规硶 1锛氳嚜鍔ㄨВ寮曠敤 Pointer/VolatilePtr锛岃幏鍙栧簳灞傜殑 Struct/Union/Enum 绫诲瀷
    fn get_base_type(&mut self, mut base_ty: TypeId) -> TypeId {
        loop {
            let norm = self.resolve_tv(base_ty);
            match self.ctx.type_registry.get(norm).clone() {
                // 閬囧埌鎸囬拡锛岃嚜鍔ㄦ墥鎺夊琛ｇ户缁線涓嬫壘
                TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                    base_ty = elem;
                }
                // 鎵惧埌搴曚簡锛岃繑鍥?
                _ => return norm,
            }
        }
    }

}


