use super::ExprChecker;
use crate::checker::ConstEvaluator;
use crate::def::{Def, DefId};
use crate::passes::ImportResolver;
use crate::passes::TypeResolver;
use crate::query::{MemberQuery, MemberQueryEnv};
use crate::scope::{SymbolInfo, SymbolKind};
use crate::semantic::SemanticSymbolKind;
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{self as ast, Expr, TypeNode, Visibility};
use kernc_utils::{DiagnosticCode, FastHashSet, NodeId, Span, SymbolId};

#[derive(Clone, Copy)]
struct ResolvedPatternField {
    name: SymbolId,
    ty: TypeId,
    definition_span: Option<Span>,
}

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    fn resolve_namespace_expr(&mut self, expr: &Expr) -> Option<TypeId> {
        let ty = match &expr.kind {
            ast::ExprKind::TypeNode(type_node) => self.evaluate_dynamic_typeof(type_node),
            ast::ExprKind::Identifier(name) => {
                let info = self.ctx.scopes.resolve(*name)?.clone();
                if info.kind != SymbolKind::Module && !Self::symbol_is_type_namespace(info.kind) {
                    return None;
                }
                self.resolved_symbol_type(&info, expr.span)
            }
            ast::ExprKind::AnchoredPath { anchor, name, .. } => {
                let (owner_module, start_scope) = self.anchored_start_scope(*anchor, expr.span)?;
                let info = self.ctx.scopes.resolve_in(start_scope, *name)?.clone();
                if info.kind != SymbolKind::Module && !Self::symbol_is_type_namespace(info.kind) {
                    return None;
                }
                let current_module = self.cached_current_module_id();
                if !self
                    .ctx
                    .visibility_allows_access(info.vis, owner_module, current_module)
                {
                    self.ctx
                        .struct_error(
                            expr.span,
                            format!(
                                "cannot access private item `{}` through anchored path",
                                self.ctx.resolve(*name)
                            ),
                        )
                        .emit();
                    TypeId::ERROR
                } else {
                    self.resolved_symbol_type(&info, expr.span)
                }
            }
            ast::ExprKind::GenericInstantiation { target, args } => {
                let _ = self.resolve_type_namespace_expr(target)?;
                self.check_generic_instantiation(target, args, expr.span)
            }
            ast::ExprKind::FieldAccess {
                lhs,
                field,
                field_span,
            } => {
                let lhs_ty = self.resolve_namespace_expr(lhs)?;
                let lhs_norm = self.resolve_tv(lhs_ty);
                let TypeKind::Module(mod_def_id) = self.ctx.type_registry.get(lhs_norm).clone()
                else {
                    return None;
                };

                let Def::Module(module) = &self.ctx.defs[mod_def_id.0 as usize] else {
                    self.ctx.emit_ice(
                        expr.span,
                        format!(
                            "Kern ICE (Typeck): Expected module definition while resolving namespace access for DefId {}.",
                            mod_def_id.0
                        ),
                    );
                    return Some(TypeId::ERROR);
                };

                let target_info = self.ctx.scopes.resolve_in(module.scope_id, *field)?.clone();
                if target_info.kind != SymbolKind::Module
                    && !Self::symbol_is_type_namespace(target_info.kind)
                {
                    return None;
                }

                let current_module_id = self.cached_current_module_id();
                if !self.ctx.visibility_allows_access(
                    target_info.vis,
                    mod_def_id,
                    current_module_id,
                ) {
                    let field_name = self.ctx.resolve(*field);
                    self.ctx
                        .struct_error(
                            expr.span,
                            format!("module has no visible member `{}`", field_name),
                        )
                        .emit();
                    return Some(TypeId::ERROR);
                }

                let mod_ty = self.ctx.type_registry.intern(TypeKind::Module(mod_def_id));
                self.ctx.facts.node_types.insert(lhs.id, mod_ty);
                self.ctx
                    .record_identifier_reference(*field_span, target_info.span);
                if target_info.kind == SymbolKind::Module {
                    self.ctx
                        .type_registry
                        .intern(TypeKind::Module(target_info.def_id.unwrap()))
                } else {
                    self.resolved_symbol_type(&target_info, *field_span)
                }
            }
            _ => return None,
        };

        self.ctx.facts.node_types.insert(expr.id, ty);
        Some(ty)
    }

    pub(crate) fn resolve_type_namespace_expr(&mut self, expr: &Expr) -> Option<TypeId> {
        let ty = self.resolve_namespace_expr(expr)?;
        let norm = self.resolve_tv(ty);
        if matches!(self.ctx.type_registry.get(norm), TypeKind::Module(_)) {
            return None;
        }
        Some(ty)
    }

    fn define_local_symbol(
        &mut self,
        name: SymbolId,
        info: SymbolInfo,
        semantic_kind: SemanticSymbolKind,
    ) {
        if self.is_discard_name(name) {
            return;
        }

        if let Err(old_info) = self.ctx.scopes.define(name, info.clone()) {
            let name_str = self.ctx.resolve(name).to_string();
            self.ctx
                .struct_error(
                    info.span,
                    format!("the name `{}` is defined multiple times", name_str),
                )
                .with_hint(format!(
                    "`{}` must be defined only once in the same binding scope",
                    name_str
                ))
                .with_span_label(
                    old_info.span,
                    format!("previous definition of `{}` was here", name_str),
                )
                .emit();
            return;
        }

        self.ctx.record_symbol_definition(
            info.span,
            semantic_kind,
            info.is_mut,
            info.vis.is_public(),
        );
        self.record_current_binding(name);
    }

    fn symbol_is_type_namespace(kind: SymbolKind) -> bool {
        matches!(
            kind,
            SymbolKind::Struct
                | SymbolKind::Union
                | SymbolKind::Enum
                | SymbolKind::Trait
                | SymbolKind::TypeAlias
                | SymbolKind::TypeParam
        )
    }

    fn is_discard_name(&self, name: SymbolId) -> bool {
        self.ctx.resolve(name) == "_"
    }

    fn pattern_binds_name(&self, name: SymbolId, entered_scope: bool) -> bool {
        !self.is_discard_name(name) && (!entered_scope || self.ctx.scopes.resolve(name).is_some())
    }

    fn pattern_needs_scope_extension(&self, pattern: &ast::Pattern, entered_scope: bool) -> bool {
        match &pattern.kind {
            ast::PatternKind::Binding(binding) => {
                self.pattern_binds_name(binding.name, entered_scope)
            }
            ast::PatternKind::Destructure(destructure) => destructure
                .fields
                .iter()
                .any(|field| self.pattern_needs_scope_extension(&field.pattern, entered_scope)),
            ast::PatternKind::Ignore | ast::PatternKind::Variant(_) => false,
        }
    }

    pub(crate) fn let_pattern_needs_scope_extension(
        &self,
        pattern: &ast::LetPattern,
        entered_scope: bool,
    ) -> bool {
        self.pattern_needs_scope_extension(&pattern.pattern, entered_scope)
    }

    pub(crate) fn binding_pattern_needs_scope_extension(
        &self,
        pattern: &ast::BindingPattern,
        entered_scope: bool,
    ) -> bool {
        self.pattern_binds_name(pattern.name, entered_scope)
    }

    pub(crate) fn import_needs_scope_extension(
        &self,
        import: &crate::def::ImportDef,
        entered_scope: bool,
    ) -> bool {
        ImportResolver::binding_names(import)
            .into_iter()
            .any(|name| !entered_scope || self.ctx.scopes.resolve(name).is_some())
    }

    fn define_pattern_binding(
        &mut self,
        node_id: NodeId,
        binding: &ast::BindingPattern,
        ty: TypeId,
    ) {
        let info = SymbolInfo {
            kind: SymbolKind::Var,
            node_id,
            type_id: ty,
            def_id: None,
            span: binding.name_span,
            vis: Visibility::Private,
            is_mut: binding.is_mut,
        };
        self.define_local_symbol(binding.name, info, SemanticSymbolKind::Variable);
    }

    fn check_pattern_explicit_type(
        &mut self,
        explicit_ty_ast: Option<&TypeNode>,
        actual_ty: TypeId,
        span: Span,
    ) {
        let Some(explicit_ty_ast) = explicit_ty_ast else {
            return;
        };

        let mut resolver = TypeResolver::new(self.ctx);
        let scope = resolver.current_scope_id().unwrap();
        let explicit_ty = resolver.resolve_type(explicit_ty_ast, scope);

        let actual_ty = self.resolve_tv(actual_ty);
        let mut map = std::collections::HashMap::new();
        if !self.unify(actual_ty, explicit_ty, &mut map) && actual_ty != explicit_ty {
            self.emit_mismatch_error(span, actual_ty, explicit_ty);
        }
    }

    fn is_pattern_field_pun(&self, field: &ast::DestructurePatternField) -> bool {
        matches!(
            &field.pattern.kind,
            ast::PatternKind::Binding(binding)
                if binding.name == field.name
                    && !binding.is_mut
                    && binding.name_span == field.name_span
                    && binding.span == field.name_span
        )
    }

    fn variant_payload_type(
        &mut self,
        norm_target: TypeId,
        variant_name: SymbolId,
        span: Span,
    ) -> Option<Option<TypeId>> {
        match self.ctx.type_registry.get(norm_target).clone() {
            TypeKind::Enum(def_id, generic_args) => {
                let adt_def = self.match_enum_def(def_id, span, "inspect a pattern variant")?;
                // Safety: semantic defs are immutable while type checking expressions.
                let adt_def = unsafe { &*adt_def };
                let generic_map =
                    self.positional_generic_subst_map(&adt_def.generics, &generic_args);
                let variant = adt_def.variants.iter().find(|v| v.name == variant_name)?;
                let definition_span = variant.name_span;
                self.ctx.record_identifier_reference(span, definition_span);

                let payload_ty = variant.payload_type.as_ref().map(|payload_ast| {
                    let payload_ty = self
                        .ctx
                        .facts
                        .node_types
                        .get(&payload_ast.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR);
                    self.substitute_type_with_generic_arg_map(payload_ty, &generic_map)
                });
                Some(payload_ty)
            }
            TypeKind::AnonymousEnum(enum_def) => {
                let variant = enum_def.variants.iter().find(|v| v.name == variant_name)?;
                self.ctx
                    .record_identifier_reference(span, variant.name_span);
                Some(variant.payload_ty)
            }
            _ => None,
        }
    }

    fn resolve_struct_pattern_fields(
        &mut self,
        norm_target: TypeId,
        span: Span,
    ) -> Option<(Vec<ResolvedPatternField>, String)> {
        match self.ctx.type_registry.get(norm_target).clone() {
            TypeKind::Def(def_id, generic_args) => match self.ctx.defs[def_id.0 as usize].clone() {
                Def::Struct(def) => {
                    let generic_map =
                        self.positional_generic_subst_map(&def.generics, &generic_args);
                    let mut fields = Vec::with_capacity(def.fields.len());
                    for field in &def.fields {
                        let field_ty = self
                            .ctx
                            .facts
                            .node_types
                            .get(&field.type_node.id)
                            .copied()
                            .unwrap_or(TypeId::ERROR);
                        let field_ty =
                            self.substitute_type_with_generic_arg_map(field_ty, &generic_map);

                        fields.push(ResolvedPatternField {
                            name: field.name,
                            ty: field_ty,
                            definition_span: Some(field.name_span),
                        });
                    }

                    Some((fields, self.ctx.resolve(def.name).to_string()))
                }
                Def::Union(def) => {
                    self.ctx
                        .struct_error(
                            span,
                            format!(
                                "destructuring patterns are not supported for union `{}`",
                                self.ctx.resolve(def.name)
                            ),
                        )
                        .with_hint("union values do not carry an active-field tag; access them explicitly instead")
                        .emit();
                    None
                }
                _ => None,
            },
            TypeKind::AnonymousStruct(_, fields) => Some((
                fields
                    .iter()
                    .map(|field| ResolvedPatternField {
                        name: field.name,
                        ty: field.ty,
                        definition_span: None,
                    })
                    .collect(),
                self.ctx.ty_to_string(norm_target),
            )),
            TypeKind::AnonymousUnion(_, _) => {
                self.ctx
                    .struct_error(span, "destructuring patterns are not supported for anonymous unions")
                    .with_hint("union values do not carry an active-field tag; access them explicitly instead")
                    .emit();
                None
            }
            _ => None,
        }
    }

    pub(super) fn pattern_is_irrefutable(
        &mut self,
        pattern: &ast::Pattern,
        actual_ty: TypeId,
    ) -> bool {
        match &pattern.kind {
            ast::PatternKind::Binding(_) | ast::PatternKind::Ignore => true,
            ast::PatternKind::Variant(_) => false,
            ast::PatternKind::Destructure(destructure) => {
                let actual_ty = self.resolve_tv(actual_ty);
                if matches!(
                    self.ctx.type_registry.get(actual_ty),
                    TypeKind::Enum(_, _) | TypeKind::AnonymousEnum(_)
                ) {
                    return false;
                }

                let Some((field_defs, _)) =
                    self.resolve_struct_pattern_fields(actual_ty, pattern.span)
                else {
                    return false;
                };

                destructure.fields.iter().all(|field| {
                    field_defs
                        .iter()
                        .find(|candidate| candidate.name == field.name)
                        .map(|resolved| self.pattern_is_irrefutable(&field.pattern, resolved.ty))
                        .unwrap_or(false)
                })
            }
        }
    }

    pub(super) fn check_pattern(
        &mut self,
        node_id: NodeId,
        pattern: &ast::Pattern,
        actual_ty: TypeId,
    ) {
        match &pattern.kind {
            ast::PatternKind::Binding(binding) => {
                self.define_pattern_binding(node_id, binding, actual_ty);
            }
            ast::PatternKind::Ignore => {}
            ast::PatternKind::Variant(variant) => {
                self.check_pattern_explicit_type(
                    variant.target_type.as_deref(),
                    actual_ty,
                    pattern.span,
                );
                let norm_target = self.resolve_tv(actual_ty);

                let Some(payload_ty) = self.variant_payload_type(
                    norm_target,
                    variant.variant_name,
                    variant.variant_span,
                ) else {
                    self.ctx
                        .struct_error(
                            pattern.span,
                            "variant pattern is only allowed on enum values",
                        )
                        .emit();
                    return;
                };

                if payload_ty.is_some() {
                    let variant_name = self.ctx.resolve(variant.variant_name).to_string();
                    self.ctx
                        .struct_error(
                            pattern.span,
                            format!("variant `{}` requires payload destructuring", variant_name),
                        )
                        .with_hint(format!(
                            "write this as `.{{ {}: value }}` or `Type.{{ {}: value }}`",
                            variant_name, variant_name
                        ))
                        .emit();
                }
            }
            ast::PatternKind::Destructure(destructure) => {
                self.check_pattern_explicit_type(
                    destructure.target_type.as_deref(),
                    actual_ty,
                    pattern.span,
                );

                let norm_target = self.resolve_tv(actual_ty);
                match self.ctx.type_registry.get(norm_target).clone() {
                    TypeKind::Enum(_, _) | TypeKind::AnonymousEnum(_) => {
                        if destructure.fields.len() != 1 {
                            self.ctx
                                .struct_error(
                                    pattern.span,
                                    "enum destructuring patterns must specify exactly one variant",
                                )
                                .with_hint("use `.{ Variant: pattern }` for payload variants or `.Variant` for payload-less variants")
                                .emit();
                            return;
                        }

                        let field = &destructure.fields[0];
                        let Some(payload_ty) =
                            self.variant_payload_type(norm_target, field.name, field.name_span)
                        else {
                            self.ctx
                                .struct_error(
                                    field.span,
                                    format!(
                                        "variant `{}` not found in enum pattern",
                                        self.ctx.resolve(field.name)
                                    ),
                                )
                                .emit();
                            return;
                        };

                        if let Some(payload_ty) = payload_ty {
                            if self.is_pattern_field_pun(field) {
                                let field_name = self.ctx.resolve(field.name).to_string();
                                self.ctx
                                    .struct_error(
                                        field.span,
                                        format!(
                                            "variant `{}` requires an explicit payload pattern",
                                            field_name
                                        ),
                                    )
                                    .with_hint(format!(
                                        "write this as `.{{ {}: value }}`",
                                        field_name
                                    ))
                                    .emit();
                                return;
                            }

                            self.check_pattern(node_id, &field.pattern, payload_ty);
                        } else {
                            let field_name = self.ctx.resolve(field.name).to_string();
                            self.ctx
                                .struct_error(
                                    field.span,
                                    format!("variant `{}` does not take a payload", field_name),
                                )
                                .with_hint(format!(
                                    "use `.{}` for the payload-less form",
                                    field_name
                                ))
                                .emit();
                        }
                    }
                    _ => {
                        let Some((field_defs, type_name)) =
                            self.resolve_struct_pattern_fields(norm_target, pattern.span)
                        else {
                            self.ctx
                                .struct_error(
                                    pattern.span,
                                    "destructuring patterns are only supported on structs and enum values",
                                )
                                .emit();
                            return;
                        };

                        let mut seen = FastHashSet::default();
                        for field in &destructure.fields {
                            if !seen.insert(field.name) {
                                self.ctx
                                    .struct_error(
                                        field.span,
                                        format!(
                                            "field `{}` is destructured more than once",
                                            self.ctx.resolve(field.name)
                                        ),
                                    )
                                    .emit();
                                continue;
                            }

                            let Some(resolved_field) = field_defs
                                .iter()
                                .find(|candidate| candidate.name == field.name)
                            else {
                                self.ctx
                                    .struct_error(
                                        field.span,
                                        format!(
                                            "field `{}` does not exist in `{}`",
                                            self.ctx.resolve(field.name),
                                            type_name
                                        ),
                                    )
                                    .emit();
                                continue;
                            };

                            if let Some(definition_span) = resolved_field.definition_span {
                                self.ctx
                                    .record_identifier_reference(field.name_span, definition_span);
                            }

                            self.check_pattern(node_id, &field.pattern, resolved_field.ty);
                        }
                    }
                }
            }
        }
    }

    fn let_else_uncovered_witness(
        &mut self,
        primary: &ast::Pattern,
        arms: &[ast::LetElseArm],
        target_ty: TypeId,
    ) -> Option<String> {
        let mut patterns = Vec::with_capacity(arms.len() + 1);
        patterns.push(primary);
        patterns.extend(arms.iter().map(|arm| &arm.pattern));
        self.uncovered_pattern_witness(target_ty, &patterns)
    }

    fn emit_let_else_unreachable_arm(&mut self, span: Span) {
        self.ctx
            .struct_error(
                span,
                "`let ... else` arm does not match any remaining failure case",
            )
            .with_hint("earlier patterns already cover every value matched by this arm")
            .emit();
    }

    fn cached_current_module_id(&mut self) -> Option<DefId> {
        let current_scope = self.ctx.scopes.current_scope_id()?;
        if let Some((cached_scope, module_id)) = self.current_module_cache
            && cached_scope == current_scope
        {
            return module_id;
        }

        let module_id = self.ctx.module_for_scope(current_scope);
        self.current_module_cache = Some((current_scope, module_id));
        module_id
    }

    fn global_owner_scope(&self, def_id: DefId) -> Option<crate::scope::ScopeId> {
        self.ctx.def_owner_scope(def_id)
    }

    fn resolved_symbol_type(&mut self, info: &SymbolInfo, span: Span) -> TypeId {
        self.ctx.record_identifier_reference(span, info.span);

        if info.kind == SymbolKind::Function {
            return self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(info.def_id.unwrap(), vec![]));
        }
        if info.kind == SymbolKind::Module {
            return self
                .ctx
                .type_registry
                .intern(TypeKind::Module(info.def_id.unwrap()));
        }
        if let Some(def_id) = info.def_id {
            match info.kind {
                SymbolKind::Struct | SymbolKind::Union => {
                    return self.ctx.type_registry.intern(TypeKind::Def(def_id, vec![]));
                }
                SymbolKind::Enum => {
                    return self
                        .ctx
                        .type_registry
                        .intern(TypeKind::Enum(def_id, vec![]));
                }
                SymbolKind::Trait => {
                    return self.ctx.type_registry.intern(TypeKind::TraitObject(
                        def_id,
                        vec![],
                        Vec::new(),
                    ));
                }
                _ => {}
            }
        }
        if info.kind == SymbolKind::TypeAlias {
            if let Some(def_id) = info.def_id
                && let Def::TypeAlias(alias_def) = &self.ctx.defs[def_id.0 as usize]
            {
                let resolved_ty = self
                    .ctx
                    .facts
                    .node_types
                    .get(&alias_def.target.id)
                    .copied()
                    .unwrap_or(info.type_id);
                if resolved_ty != TypeId::ERROR {
                    return resolved_ty;
                }
            }
            return info.type_id;
        }

        if info.type_id == TypeId::ERROR
            && let Some(def_id) = info.def_id
        {
            let global_expr_ptr = if let Def::Global(g) = &self.ctx.defs[def_id.0 as usize] {
                Some(std::ptr::from_ref(&g.value))
            } else {
                None
            };

            if let Some(g_expr_ptr) = global_expr_ptr {
                let g_expr = unsafe { &*g_expr_ptr };
                if let Some(&actual_ty) = self.ctx.facts.node_types.get(&g_expr.id) {
                    return actual_ty;
                }
                let prev_scope = self.ctx.scopes.current_scope_id();
                if let Some(owner_scope) = self.global_owner_scope(def_id) {
                    self.ctx.scopes.set_current_scope(owner_scope);
                }
                let computed_ty = self.check_expr(g_expr, None);
                if let Some(prev_scope) = prev_scope {
                    self.ctx.scopes.set_current_scope(prev_scope);
                }
                return computed_ty;
            }
        }

        info.type_id
    }

    pub(crate) fn anchored_start_scope(
        &mut self,
        anchor: ast::PathAnchor,
        span: Span,
    ) -> Option<(DefId, crate::scope::ScopeId)> {
        let current_scope = self.ctx.scopes.current_scope_id()?;
        let Some(current_module) = self.ctx.module_for_scope(current_scope) else {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Typeck): could not determine current module for anchored path",
            );
            return None;
        };

        let target_module = match anchor {
            ast::PathAnchor::Parent => {
                let Some(parent) = self.ctx.module_parent(current_module) else {
                    self.ctx
                        .struct_error(span, "Cannot use `..` from the root module")
                        .emit();
                    return None;
                };
                parent
            }
            ast::PathAnchor::Package => self.ctx.module_root(current_module),
        };

        let Def::Module(module) = &self.ctx.defs[target_module.0 as usize] else {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Typeck): anchored path target is not a module",
            );
            return None;
        };

        Some((target_module, module.scope_id))
    }

    pub(crate) fn check_identifier(&mut self, name: SymbolId, span: Span) -> TypeId {
        if let Some(info) = self.ctx.scopes.resolve(name).cloned() {
            self.resolved_symbol_type(&info, span)
        } else {
            let name_str = self.ctx.resolve(name).to_string();
            self.ctx
                .struct_error(span, format!("use of undeclared identifier `{}`", name_str))
                .with_hint("make sure the variable or function is defined before using it")
                .emit();
            TypeId::ERROR
        }
    }

    pub(crate) fn check_anchored_identifier(
        &mut self,
        anchor: ast::PathAnchor,
        name: SymbolId,
        span: Span,
    ) -> TypeId {
        let Some((owner_module, start_scope)) = self.anchored_start_scope(anchor, span) else {
            return TypeId::ERROR;
        };
        let Some(info) = self.ctx.scopes.resolve_in(start_scope, name).cloned() else {
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "cannot find `{}` in the anchored module path",
                        self.ctx.resolve(name)
                    ),
                )
                .emit();
            return TypeId::ERROR;
        };

        let current_module = self.cached_current_module_id();
        if !self
            .ctx
            .visibility_allows_access(info.vis, owner_module, current_module)
        {
            self.ctx
                .struct_error(
                    span,
                    format!("module has no visible member `{}`", self.ctx.resolve(name)),
                )
                .emit();
            return TypeId::ERROR;
        }

        self.resolved_symbol_type(&info, span)
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
        else_clause: Option<&ast::LetElseClause>,
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

        let is_irrefutable = self.pattern_is_irrefutable(&pattern.pattern, norm_init);
        match (is_irrefutable, else_clause.as_ref().map(|_| ())) {
            (true, Some(_)) => {
                self.ctx
                    .struct_error(span, "irrefutable `let` patterns cannot use `else`")
                    .with_code(DiagnosticCode::IrrefutableLetElse)
                    .with_hint(
                        "remove the `else` block or use a refutable enum pattern like `.{ Ok: value }`",
                    )
                    .emit();
            }
            (false, None) => {
                self.ctx
                    .struct_error(span, "refutable `let` patterns require an `else` branch")
                    .with_hint(
                        "write this as `let .{ Variant: value } = expr else return ...;` or another diverging expression",
                    )
                    .emit();
            }
            _ => {}
        }

        if let Some(else_clause) = else_clause {
            match else_clause {
                ast::LetElseClause::Expr(else_expr) => {
                    let else_ty = self.check_expr(else_expr, None);
                    let norm_else = self.resolve_tv(else_ty);
                    if norm_else != TypeId::NEVER && norm_else != TypeId::ERROR {
                        self.ctx
                            .struct_error(
                                else_expr.span,
                                "`let ... else` failure branches must diverge",
                            )
                            .with_hint(
                                "end the `else` block with `return`, `break`, `continue`, or another diverging expression",
                            )
                            .emit();
                    }
                }
                ast::LetElseClause::Arms(arms) => {
                    if arms.is_empty() {
                        self.ctx
                            .struct_error(span, "`let ... else` arm blocks cannot be empty")
                            .with_hint(
                                "write at least one failure arm, or use a plain diverging `else` expression",
                            )
                            .emit();
                    }

                    let has_constructor_coverage = self.coverage_constructors(norm_init).is_some();
                    let mut seen_patterns = Vec::new();
                    if let Some(lowered) = self.coverage_lower_pattern(&pattern.pattern, norm_init)
                    {
                        seen_patterns.push(vec![lowered]);
                    }
                    let mut failure_closed = false;

                    for arm in arms {
                        let irrefutable = self.pattern_is_irrefutable(&arm.pattern, norm_init);
                        if failure_closed {
                            self.emit_let_else_unreachable_arm(arm.pattern.span);
                        } else if has_constructor_coverage
                            && let Some(lowered) =
                                self.coverage_lower_pattern(&arm.pattern, norm_init)
                        {
                            if self.coverage_vector_is_useful(
                                &[norm_init],
                                &seen_patterns,
                                std::slice::from_ref(&lowered),
                            ) {
                                seen_patterns.push(vec![lowered]);
                                if irrefutable
                                    || self.coverage_matrix_is_exhaustive(norm_init, &seen_patterns)
                                {
                                    failure_closed = true;
                                }
                            } else {
                                self.emit_let_else_unreachable_arm(arm.pattern.span);
                            }
                        } else if irrefutable {
                            failure_closed = true;
                        }

                        self.ctx.scopes.enter_scope();
                        self.check_pattern(node_id, &arm.pattern, init_ty);

                        let body_ty = self.check_expr(&arm.body, None);
                        let norm_body = self.resolve_tv(body_ty);
                        if norm_body != TypeId::NEVER && norm_body != TypeId::ERROR {
                            self.ctx
                                .struct_error(
                                    arm.body.span,
                                    "`let ... else` failure branches must diverge",
                                )
                                .with_hint(
                                    "end the `else` block with `return`, `break`, `continue`, or another diverging expression",
                                )
                                .emit();
                        }
                        self.ctx.scopes.exit_scope();
                    }

                    if let Some(witness) =
                        self.let_else_uncovered_witness(&pattern.pattern, arms, norm_init)
                    {
                        self.ctx
                            .struct_error(
                                else_clause.span(),
                                "`let ... else` arms do not cover all remaining failure cases",
                            )
                            .with_hint(format!(
                                "for example, this value is still uncovered: `{}`",
                                witness
                            ))
                            .emit();
                    }
                }
            }
        }

        self.check_pattern(node_id, &pattern.pattern, init_ty);
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
            span: pattern.name_span,
            vis: Visibility::Private,
            is_mut: pattern.is_mut,
        };
        self.define_local_symbol(pattern.name, info, SemanticSymbolKind::Static);

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
            TypeKind::Simd { elem, lanes } => {
                let mut evaluator = ConstEvaluator::new(self.ctx);
                let Ok(lane_idx) = evaluator.eval_usize(index) else {
                    self.ctx
                        .struct_error(
                            index.span,
                            "SIMD lane index must be a compile-time constant",
                        )
                        .with_hint("example: `vec.[2]`")
                        .emit();
                    return TypeId::ERROR;
                };

                if lane_idx >= lanes as u64 {
                    self.ctx
                        .struct_error(
                            index.span,
                            format!(
                                "SIMD lane index {} is out of bounds for `{}`",
                                lane_idx,
                                self.ctx.ty_to_string(norm_lhs)
                            ),
                        )
                        .emit();
                    return TypeId::ERROR;
                }

                elem
            }
            TypeKind::Array { elem, .. } | TypeKind::Slice { elem, .. } => elem,
            TypeKind::Error => TypeId::ERROR,
            _ => {
                self.ctx
                    .struct_error(lhs.span, "cannot index into this type")
                    .with_hint("only arrays, slices, and SIMD values support `.[]`")
                    .emit();
                TypeId::ERROR
            }
        }
    }

    pub(crate) fn expr_is_type_namespace(&mut self, expr: &Expr) -> bool {
        self.resolve_type_namespace_expr(expr).is_some()
    }

    fn check_payloadless_enum_variant_access(
        &mut self,
        target_ty: TypeId,
        field: SymbolId,
        field_span: Span,
        span: Span,
    ) -> Option<TypeId> {
        let resolved_target = self.resolve_tv(target_ty);
        let norm_target = self.ctx.normalize_concrete_type(resolved_target);

        match self.ctx.type_registry.get(norm_target).clone() {
            TypeKind::Enum(def_id, _) => {
                let adt_def = self.match_enum_def(def_id, span, "access an enum variant")?;
                // Safety: semantic defs are immutable while type checking expressions.
                let adt_def = unsafe { &*adt_def };
                let Some(variant) = adt_def
                    .variants
                    .iter()
                    .find(|variant| variant.name == field)
                else {
                    let available_variants: Vec<String> = adt_def
                        .variants
                        .iter()
                        .map(|variant| format!(".{}", self.ctx.resolve(variant.name)))
                        .collect();
                    let mut diag = self.ctx.struct_error(
                        span,
                        format!(
                            "variant `{}` not found in enum type `{}`",
                            self.ctx.resolve(field),
                            self.ctx.ty_to_string(target_ty)
                        ),
                    );

                    if !available_variants.is_empty() {
                        diag = diag.with_hint(format!(
                            "available variants: {}",
                            available_variants.join(", ")
                        ));
                    }
                    diag.emit();
                    return Some(TypeId::ERROR);
                };

                self.ctx
                    .record_identifier_reference(field_span, variant.name_span);

                if variant.payload_type.is_some() {
                    let target_str = self.ctx.ty_to_string(target_ty);
                    let field_str = self.ctx.resolve(field).to_string();
                    self.ctx
                        .struct_error(span, format!("variant `{}` requires a payload", field_str))
                        .with_hint(format!(
                            "initialize it as `{}.{{ {}: value }}`",
                            target_str, field_str
                        ))
                        .emit();
                    return Some(TypeId::ERROR);
                }

                Some(target_ty)
            }
            TypeKind::AnonymousEnum(enum_def) => {
                let Some(variant) = enum_def
                    .variants
                    .iter()
                    .find(|variant| variant.name == field)
                else {
                    let available_variants: Vec<String> = enum_def
                        .variants
                        .iter()
                        .map(|variant| format!(".{}", self.ctx.resolve(variant.name)))
                        .collect();
                    let mut diag = self.ctx.struct_error(
                        span,
                        format!(
                            "variant `{}` not found in enum type `{}`",
                            self.ctx.resolve(field),
                            self.ctx.ty_to_string(target_ty)
                        ),
                    );

                    if !available_variants.is_empty() {
                        diag = diag.with_hint(format!(
                            "available variants: {}",
                            available_variants.join(", ")
                        ));
                    }
                    diag.emit();
                    return Some(TypeId::ERROR);
                };

                self.ctx
                    .record_identifier_reference(field_span, variant.name_span);

                if variant.payload_ty.is_some() {
                    let target_str = self.ctx.ty_to_string(target_ty);
                    let field_str = self.ctx.resolve(field).to_string();
                    self.ctx
                        .struct_error(span, format!("variant `{}` requires a payload", field_str))
                        .with_hint(format!(
                            "initialize it as `{}.{{ {}: value }}`",
                            target_str, field_str
                        ))
                        .emit();
                    return Some(TypeId::ERROR);
                }

                Some(target_ty)
            }
            _ => None,
        }
    }

    pub(crate) fn check_field_access(
        &mut self,
        expr_id: NodeId,
        lhs: &Expr,
        field: SymbolId,
        field_span: Span,
        span: Span,
    ) -> TypeId {
        let lhs_ty = self
            .resolve_type_namespace_expr(lhs)
            .unwrap_or_else(|| self.check_expr(lhs, None));
        if lhs_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

        // Peel pointers before checking aggregate or module members.
        let base_norm = self.get_base_type(lhs_ty);

        // Modules are namespaces, so member lookup uses the peeled base type directly.
        if let TypeKind::Module(mod_def_id) = self.ctx.type_registry.get(base_norm).clone() {
            let started = self.timing_start();
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
            if let Some(target_info) = self.ctx.scopes.resolve_in(mod_scope, field).cloned() {
                let current_module_id = self.cached_current_module_id();
                if !self.ctx.visibility_allows_access(
                    target_info.vis,
                    mod_def_id,
                    current_module_id,
                ) {
                    let field_name = self.ctx.resolve(field);
                    self.ctx
                        .struct_error(
                            span,
                            format!("module has no visible member `{}`", field_name),
                        )
                        .emit();
                    self.record_expr_timing(started, |stats, elapsed| {
                        stats.access_field_module += elapsed;
                    });
                    return TypeId::ERROR;
                }
                let definition_span = target_info.span;
                let target_kind = target_info.kind;
                let target_def_id = target_info.def_id;
                let target_type_id = target_info.type_id;
                let real_ty = if target_kind == SymbolKind::Function {
                    self.ctx
                        .type_registry
                        .intern(TypeKind::FnDef(target_def_id.unwrap(), vec![]))
                } else if target_kind == SymbolKind::Module {
                    self.ctx
                        .type_registry
                        .intern(TypeKind::Module(target_def_id.unwrap()))
                } else if target_type_id == TypeId::ERROR {
                    if let Some(def_id) = target_def_id {
                        let global_expr_ptr =
                            if let Def::Global(g) = &self.ctx.defs[def_id.0 as usize] {
                                Some(std::ptr::from_ref(&g.value))
                            } else {
                                None
                            };

                        if let Some(g_expr) = global_expr_ptr {
                            // Safety: expression storage inside semantic defs is immutable during
                            // type checking; borrowing via raw pointer avoids cloning large ASTs.
                            let g_expr = unsafe { &*g_expr };
                            if let Some(&actual_ty) = self.ctx.facts.node_types.get(&g_expr.id) {
                                actual_ty
                            } else {
                                let prev_scope = self.ctx.scopes.current_scope_id();
                                if let Some(owner_scope) = self.global_owner_scope(def_id) {
                                    self.ctx.scopes.set_current_scope(owner_scope);
                                }
                                let computed_ty = self.check_expr(g_expr, None);
                                if let Some(prev_scope) = prev_scope {
                                    self.ctx.scopes.set_current_scope(prev_scope);
                                }
                                computed_ty
                            }
                        } else {
                            target_type_id
                        }
                    } else {
                        target_type_id
                    }
                } else {
                    target_type_id
                };

                let mod_ty = self.ctx.type_registry.intern(TypeKind::Module(mod_def_id));
                self.ctx.facts.node_types.insert(lhs.id, mod_ty);
                self.ctx
                    .record_identifier_reference(field_span, definition_span);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.access_field_module += elapsed;
                });
                return real_ty;
            } else {
                let field_name = self.ctx.resolve(field);
                self.ctx
                    .struct_error(
                        span,
                        format!("module has no visible member `{}`", field_name),
                    )
                    .emit();
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.access_field_module += elapsed;
                });
                return TypeId::ERROR;
            }
        }

        if self.expr_is_type_namespace(lhs) {
            let started = self.timing_start();
            if let Some(enum_ty) =
                self.check_payloadless_enum_variant_access(lhs_ty, field, field_span, span)
            {
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.access_field_enum_variant += elapsed;
                });
                return enum_ty;
            }
            self.record_expr_timing(started, |stats, elapsed| {
                stats.access_field_enum_variant += elapsed;
            });
        }

        let started = self.timing_start();
        if let Some(resolution) = self.try_find_field_or_method_silent(lhs_ty, field, span) {
            self.ctx
                .record_identifier_reference(field_span, resolution.candidate.definition_span);
            if let Some(owner_trait_ty) = resolution.owner_trait_ty {
                self.ctx.set_trait_method_owner(expr_id, owner_trait_ty);
            }
            self.record_expr_timing(started, |stats, elapsed| {
                stats.access_field_member_query += elapsed;
            });
            return resolution.candidate.type_id;
        }
        self.record_expr_timing(started, |stats, elapsed| {
            stats.access_field_member_query += elapsed;
        });

        // No field or method matched. Emit the detailed fallback diagnostic.
        let miss_started = self.timing_start();
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
        self.record_expr_timing(miss_started, |stats, elapsed| {
            stats.access_field_miss += elapsed;
        });

        TypeId::ERROR
    }

    /// Resolve a field or method without emitting diagnostics on failure.
    fn try_find_field_or_method_silent(
        &mut self,
        lhs_ty: TypeId,
        field: SymbolId,
        span: Span,
    ) -> Option<crate::query::MemberResolution> {
        let lhs_ty = self.resolve_tv(lhs_ty);
        let active_bounds_ptr = std::ptr::from_ref(self.ctx.analysis.active_bounds.as_slice());
        let current_module_id = self.cached_current_module_id();
        let mut query = MemberQuery::new(self.ctx);
        // Safety: member queries only read active generic bounds. The query may mutate other
        // semantic state, but it does not resize or replace `ctx.analysis.active_bounds`.
        let env = unsafe { MemberQueryEnv::from_active_bounds(&*active_bounds_ptr) };
        query.resolve_named_member(current_module_id, lhs_ty, field, &env, span)
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
        let lhs_kind = self.ctx.type_registry.get(norm_lhs).clone();
        let base_allows_mut_slice = match lhs_kind {
            TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => is_mut,
            TypeKind::Slice { is_mut, .. } => is_mut,
            _ => self.is_lvalue_mutable(lhs),
        };

        if is_mut && lhs_ty != TypeId::ERROR {
            match lhs_kind {
                TypeKind::Slice { is_mut: false, .. } => {
                    self.ctx
                        .struct_error(
                            span,
                            "cannot create a mutable slice from an immutable slice view",
                        )
                        .with_hint(
                            "`let mut` on the slice binding only makes the slice handle reassignable; it does not change `[]T` into `[]mut T`",
                        )
                        .emit();
                }
                _ if !base_allows_mut_slice => {
                    self.ctx
                        .struct_error(
                            span,
                            "cannot create a mutable slice from an immutable location",
                        )
                        .with_hint(
                            "ensure the source is a mutable binding, a mutable field path, or a mutable pointer (`*mut T` / `^mut T`)",
                        )
                        .emit();
                }
                _ => {}
            }
        }

        match lhs_kind {
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

    /// Auto-deref pointers until the underlying aggregate or module type is reached.
    fn get_base_type(&mut self, mut base_ty: TypeId) -> TypeId {
        loop {
            let norm = self.resolve_tv(base_ty);
            match self.ctx.type_registry.get(norm).clone() {
                // Keep peeling pointer layers.
                TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                    base_ty = elem;
                }
                // Stop at the first non-pointer type.
                _ => return norm,
            }
        }
    }
}
