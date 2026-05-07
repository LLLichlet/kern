use super::*;
use crate::ty::{ConstGeneric, GenericArg, LayoutEngine};

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    fn emit_duplicate_generic_param(&mut self, name: SymbolId, span: Span, previous_span: Span) {
        let name_str = self.ctx.resolve(name).to_string();
        self.ctx
            .struct_error(
                span,
                format!(
                    "the generic parameter `{}` is defined multiple times",
                    name_str
                ),
            )
            .with_hint(format!(
                "`{}` must be defined only once in the same generic parameter list",
                name_str
            ))
            .with_span_label(
                previous_span,
                format!(
                    "previous definition of generic parameter `{}` was here",
                    name_str
                ),
            )
            .emit();
    }

    pub(super) fn required_def_id(
        &mut self,
        symbol: &SymbolInfo,
        span: Span,
        context: &str,
        segment: SymbolId,
    ) -> Option<DefId> {
        if let Some(def_id) = symbol.def_id {
            Some(def_id)
        } else {
            self.ctx.emit_ice(
                span,
                format!(
                    "Resolved {} `{}` is missing a DefId",
                    context,
                    self.ctx.resolve(segment)
                ),
            );
            None
        }
    }

    pub(super) fn module_scope_from_def(
        &mut self,
        def_id: DefId,
        span: Span,
        segment: SymbolId,
    ) -> Option<ScopeId> {
        if let Def::Module(m) = &self.ctx.defs[def_id.0 as usize] {
            Some(m.scope_id)
        } else {
            self.ctx.emit_ice(
                span,
                format!(
                    "Resolved module path segment `{}` points to non-module def {:?}",
                    self.ctx.resolve(segment),
                    def_id
                ),
            );
            None
        }
    }

    pub(super) fn last_segment_name(&self, segments: &[ast::TypePathSegment]) -> String {
        segments
            .last()
            .map(|segment| self.ctx.resolve(segment.name).to_string())
            .unwrap_or_else(|| "<empty-path>".to_string())
    }

    pub(super) fn bind_generics(&mut self, generics: &[ast::GenericParam], scope: ScopeId) {
        self.ctx.scopes.set_current_scope(scope);

        for param in generics {
            let (kind, param_ty) = match &param.kind {
                ast::GenericParamKind::Type => (
                    SymbolKind::TypeParam,
                    self.ctx.type_registry.intern(TypeKind::Param(param.name)),
                ),
                ast::GenericParamKind::Const { ty } => (
                    SymbolKind::ConstParam,
                    self.resolve_const_generic_param_type(ty, scope, param.span),
                ),
            };
            let info = SymbolInfo {
                kind,
                node_id: self.ctx.next_node_id(),
                type_id: param_ty,
                def_id: None,
                span: param.span,
                vis: Visibility::Private,
                is_mut: false,
            };
            if let Err(old_info) = self.ctx.scopes.define(param.name, info) {
                self.emit_duplicate_generic_param(param.name, param.span, old_info.span);
            }
        }
    }

    pub(crate) fn resolve_const_generic_param_type(
        &mut self,
        ty_node: &ast::TypeNode,
        scope: ScopeId,
        span: Span,
    ) -> TypeId {
        let ty = match &ty_node.kind {
            ast::TypeKind::Path {
                anchor: None,
                segments,
            } if segments.len() == 1 && segments[0].args.is_empty() => {
                let name = self
                    .ctx
                    .sess
                    .source_manager
                    .slice_source(segments[0].name_span)
                    .trim()
                    .to_string();
                self.resolve_builtin_primitive(&name).unwrap_or_else(|| {
                    self.ctx.facts.node_types.remove(&ty_node.id);
                    self.resolve_type(ty_node, scope)
                })
            }
            _ => {
                self.ctx.facts.node_types.remove(&ty_node.id);
                self.resolve_type(ty_node, scope)
            }
        };
        self.ctx.facts.node_types.insert(ty_node.id, ty);
        if ty != TypeId::ERROR && !self.supports_const_generic_param_type(ty) {
            let found_ty = self.ctx.ty_to_string(ty);
            self.ctx
                .struct_error(
                    span,
                    "const generic parameters must currently use an integer, `bool`, or a payload-less enum type",
                )
                .with_hint(format!("found `{}`", found_ty))
                .with_hint("for example: `N: usize`, `Bits: u32`, `Enabled: bool`, or `Mode: BuildMode`")
                .emit();
            return TypeId::ERROR;
        }
        ty
    }

    fn supports_const_generic_param_type(&mut self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        if self.ctx.type_registry.is_integer(norm) || norm == TypeId::BOOL {
            return true;
        }

        match self.ctx.type_registry.get(norm) {
            TypeKind::Enum(def_id, _) => match &self.ctx.defs[def_id.0 as usize] {
                crate::def::Def::Enum(def) => def
                    .variants
                    .iter()
                    .all(|variant| variant.payload_type.is_none()),
                _ => false,
            },
            TypeKind::AnonymousEnum(enum_def) => enum_def
                .variants
                .iter()
                .all(|variant| variant.payload_ty.is_none()),
            _ => false,
        }
    }

    pub(super) fn generic_param_placeholder_arg(
        &mut self,
        param: &ast::GenericParam,
        scope: ScopeId,
    ) -> GenericArg {
        match &param.kind {
            ast::GenericParamKind::Type => {
                GenericArg::Type(self.ctx.type_registry.intern(TypeKind::Param(param.name)))
            }
            ast::GenericParamKind::Const { ty } => GenericArg::Const(ConstGeneric::Param(
                param.name,
                self.resolve_const_generic_param_type(ty, scope, param.span),
            )),
        }
    }

    pub(super) fn resolve_where_clauses(&mut self, clauses: &[ast::WhereClause], scope: ScopeId) {
        for clause in clauses {
            self.resolve_type(&clause.target_ty, scope);
            for bound in &clause.bounds {
                let bound_ty = self.resolve_type(bound, scope);
                let bound_norm = self.ctx.type_registry.normalize(bound_ty);
                if bound_norm != TypeId::ERROR
                    && !matches!(
                        self.ctx.type_registry.get(bound_norm),
                        TypeKind::TraitObject(..)
                    )
                {
                    let found = self.ctx.ty_to_string(bound_norm);
                    self.ctx
                        .struct_error(bound.span, "where-clause bounds must name a trait")
                        .with_hint(format!("found `{}`", found))
                        .with_hint(
                            "write the right-hand side as a trait, for example `where T: Printable`",
                        )
                        .emit();
                    self.ctx.facts.node_types.insert(bound.id, TypeId::ERROR);
                }
            }
        }
    }

    pub(super) fn bind_self_type(&mut self, target_ty: TypeId, scope: ScopeId, span: Span) {
        self.ctx.scopes.set_current_scope(scope);
        let self_sym = self.ctx.intern("Self");
        let info = SymbolInfo {
            kind: SymbolKind::TypeAlias,
            node_id: self.ctx.next_node_id(),
            type_id: target_ty,
            def_id: None,
            span,
            vis: Visibility::Private,
            is_mut: false,
        };
        let _ = self.ctx.scopes.define(self_sym, info);
    }

    pub(super) fn kind_to_string(&self, kind: SymbolKind) -> &'static str {
        match kind {
            SymbolKind::Var => "variable",
            SymbolKind::Const => "constant",
            SymbolKind::ConstParam => "const parameter",
            SymbolKind::Static => "static variable",
            SymbolKind::Function => "function",
            SymbolKind::Module => "module",
            SymbolKind::Struct => "struct",
            SymbolKind::Union => "union",
            SymbolKind::Enum => "algebraic data type",
            SymbolKind::Trait => "trait",
            SymbolKind::TypeAlias => "type alias",
            SymbolKind::AssociatedType => "associated type",
            SymbolKind::TypeParam => "type parameter",
        }
    }

    pub(super) fn ensure_sized(&mut self, ty: TypeId, span: Span) {
        let norm = self.ctx.type_registry.normalize(ty);
        if matches!(self.ctx.type_registry.get(norm), TypeKind::TraitObject(..)) {
            self.ctx.struct_error(span, "trait objects have dynamic size and cannot be used as naked types")
                .with_hint("in Kern, you must explicitly use a pointer for dynamic dispatch, e.g., `&Trait` or `&mut Trait`")
                .emit();
            return;
        }

        if norm == TypeId::ERROR || self.type_contains_params(norm) {
            return;
        }

        let mut layout = LayoutEngine::new(self.ctx);
        let _ = layout.compute_type_size_at(norm, span);
    }
}
