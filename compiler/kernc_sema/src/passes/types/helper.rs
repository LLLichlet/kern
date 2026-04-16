use super::*;

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    pub(super) fn check_type_generic_bounds(
        &mut self,
        span: Span,
        def_id: DefId,
        arg_tys: &[TypeId],
    ) -> bool {
        let Some((item_name, generics, where_clauses, kind_name)) =
            self.generic_def_bounds_info(def_id)
        else {
            return true;
        };

        if generics.len() != arg_tys.len() {
            self.ctx.emit_error(
                span,
                format!(
                    "{} `{}` expects {} generic arguments, but {} were provided",
                    kind_name,
                    item_name,
                    generics.len(),
                    arg_tys.len()
                ),
            );
            return false;
        }

        if arg_tys
            .iter()
            .any(|&ty| ty == TypeId::ERROR || self.type_contains_params(ty))
        {
            return true;
        }

        if where_clauses.is_empty() {
            return true;
        }

        self.ensure_where_clause_types_resolved(def_id, &generics, &where_clauses);

        let mut map = HashMap::new();
        for (param, arg_ty) in generics.iter().zip(arg_tys.iter()) {
            map.insert(param.name, *arg_ty);
        }

        let mut pairs_to_check = Vec::new();
        {
            let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
            for clause in where_clauses {
                let original_target = self
                    .ctx
                    .node_types
                    .get(&clause.target_ty.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let sub_target = subst.substitute(original_target);

                for bound_ast in clause.bounds {
                    let original_bound = self
                        .ctx
                        .node_types
                        .get(&bound_ast.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR);
                    let sub_bound = subst.substitute(original_bound);
                    pairs_to_check.push((sub_target, sub_bound));
                }
            }
        }

        let mut ok = true;
        for (sub_target, sub_bound) in pairs_to_check {
            if sub_target == TypeId::ERROR || sub_bound == TypeId::ERROR {
                ok = false;
                continue;
            }

            let bound_ok = {
                let mut checker = ExprChecker::new(self.ctx, None);
                checker.check_trait_impl(sub_target, sub_bound)
            };

            if !bound_ok {
                ok = false;
                let target_str = self.ctx.ty_to_string(sub_target);
                let bound_str = self.ctx.ty_to_string(sub_bound);
                self.ctx
                    .struct_error(span, "type does not satisfy trait bounds")
                    .with_hint(format!("required bound: `{}: {}`", target_str, bound_str))
                    .emit();
            }
        }

        ok
    }

    pub(crate) fn ensure_impl_signature_types_resolved(&mut self, impl_id: DefId) {
        let Some(impl_ptr) = self
            .ctx
            .defs
            .get(impl_id.0 as usize)
            .and_then(|def| match def {
                Def::Impl(impl_def) => Some(std::ptr::from_ref(impl_def)),
                _ => None,
            })
        else {
            return;
        };
        // Safety: this helper only reads the impl definition while mutating type state. It never
        // inserts or removes entries from `ctx.defs`, so the pointed-to impl stays valid.
        let impl_def = unsafe { &*impl_ptr };
        let Some(parent_module) = impl_def.parent_module else {
            return;
        };
        let Def::Module(module_def) = &self.ctx.defs[parent_module.0 as usize] else {
            return;
        };

        let have_target = self.ctx.node_types.contains_key(&impl_def.target_type.id);
        let have_trait = impl_def
            .trait_type
            .as_ref()
            .is_none_or(|trait_ty| self.ctx.node_types.contains_key(&trait_ty.id));
        let have_bounds = impl_def.where_clauses.iter().all(|clause| {
            self.ctx.node_types.contains_key(&clause.target_ty.id)
                && clause
                    .bounds
                    .iter()
                    .all(|bound| self.ctx.node_types.contains_key(&bound.id))
        });
        if have_target && have_trait && have_bounds {
            return;
        }

        let parent_scope = module_def.scope_id;
        self.ctx.scopes.set_current_scope(parent_scope);
        let impl_scope = self.ctx.scopes.enter_scope();
        self.bind_generics(&impl_def.generics, impl_scope);
        self.resolve_where_clauses(&impl_def.where_clauses, impl_scope);

        let target_ty = self.resolve_type(&impl_def.target_type, impl_scope);
        self.bind_self_type(target_ty, impl_scope, impl_def.span);
        if let Some(trait_ty) = &impl_def.trait_type {
            self.resolve_type(trait_ty, impl_scope);
        }

        self.ctx.scopes.exit_scope();
        self.ctx.scopes.set_current_scope(parent_scope);
    }

    fn ensure_where_clause_types_resolved(
        &mut self,
        def_id: DefId,
        generics: &[ast::GenericParam],
        where_clauses: &[ast::WhereClause],
    ) {
        let needs_resolution = where_clauses.iter().any(|clause| {
            !self.ctx.node_types.contains_key(&clause.target_ty.id)
                || clause
                    .bounds
                    .iter()
                    .any(|bound| !self.ctx.node_types.contains_key(&bound.id))
        });
        if !needs_resolution {
            return;
        }

        let Some(owner_scope) = self.def_owner_module_scope(def_id) else {
            return;
        };

        self.ctx.scopes.set_current_scope(owner_scope);
        let item_scope = self.ctx.scopes.enter_scope();

        if let Def::Trait(trait_def) = &self.ctx.defs[def_id.0 as usize] {
            let self_args = generics
                .iter()
                .map(|param| self.ctx.type_registry.intern(TypeKind::Param(param.name)))
                .collect();
            let self_ty =
                self.ctx
                    .type_registry
                    .intern(TypeKind::TraitObject(def_id, self_args, Vec::new()));
            self.bind_self_type(self_ty, item_scope, trait_def.span);
        }

        self.bind_generics(generics, item_scope);
        self.resolve_where_clauses(where_clauses, item_scope);
        self.ctx.scopes.exit_scope();
        self.ctx.scopes.set_current_scope(owner_scope);
    }

    fn generic_def_bounds_info(
        &self,
        def_id: DefId,
    ) -> Option<(
        String,
        Vec<ast::GenericParam>,
        Vec<ast::WhereClause>,
        &'static str,
    )> {
        match &self.ctx.defs[def_id.0 as usize] {
            Def::Struct(s) => Some((
                self.ctx.resolve(s.name).to_string(),
                s.generics.clone(),
                s.where_clauses.clone(),
                "struct",
            )),
            Def::Union(u) => Some((
                self.ctx.resolve(u.name).to_string(),
                u.generics.clone(),
                u.where_clauses.clone(),
                "union",
            )),
            Def::Enum(e) => Some((
                self.ctx.resolve(e.name).to_string(),
                e.generics.clone(),
                e.where_clauses.clone(),
                "enum",
            )),
            Def::Trait(t) => Some((
                self.ctx.resolve(t.name).to_string(),
                t.generics.clone(),
                t.where_clauses.clone(),
                "trait",
            )),
            Def::TypeAlias(t) => Some((
                self.ctx.resolve(t.name).to_string(),
                t.generics.clone(),
                t.where_clauses.clone(),
                "type alias",
            )),
            _ => None,
        }
    }

    fn def_owner_module_scope(&self, def_id: DefId) -> Option<ScopeId> {
        self.ctx.def_owner_scope(def_id)
    }

    fn type_contains_params(&mut self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm).clone() {
            TypeKind::Param(_) | TypeKind::TypeVar(_) => true,
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::Alias(_, elem)
            | TypeKind::AnonymousEnumPayload(elem) => self.type_contains_params(elem),
            TypeKind::Array { elem, .. } | TypeKind::ArrayInfer { elem, .. } => {
                self.type_contains_params(elem)
            }
            TypeKind::Def(_, args)
            | TypeKind::Enum(_, args)
            | TypeKind::Associated(_, args)
            | TypeKind::FnDef(_, args) => {
                args.into_iter().any(|arg| self.type_contains_params(arg))
            }
            TypeKind::TraitObject(_, args, assoc_bindings) => {
                args.into_iter().any(|arg| self.type_contains_params(arg))
                    || assoc_bindings
                        .into_iter()
                        .any(|(_, ty)| self.type_contains_params(ty))
            }
            TypeKind::Projection {
                target,
                trait_args,
                assoc_args,
                ..
            } => {
                self.type_contains_params(target)
                    || trait_args
                        .into_iter()
                        .any(|arg| self.type_contains_params(arg))
                    || assoc_args
                        .into_iter()
                        .any(|arg| self.type_contains_params(arg))
            }
            TypeKind::Function { params, ret, .. } | TypeKind::ClosureInterface { params, ret } => {
                params
                    .into_iter()
                    .any(|param| self.type_contains_params(param))
                    || self.type_contains_params(ret)
            }
            TypeKind::AnonymousState {
                captures,
                params,
                ret,
                ..
            } => {
                captures
                    .into_iter()
                    .any(|capture| self.type_contains_params(capture))
                    || params
                        .into_iter()
                        .any(|param| self.type_contains_params(param))
                    || self.type_contains_params(ret)
            }
            TypeKind::AnonymousStruct(_, fields) | TypeKind::AnonymousUnion(_, fields) => fields
                .into_iter()
                .any(|field| self.type_contains_params(field.ty)),
            TypeKind::AnonymousEnum(enum_def) => enum_def.variants.into_iter().any(|variant| {
                variant
                    .payload_ty
                    .is_some_and(|payload_ty| self.type_contains_params(payload_ty))
            }),
            _ => false,
        }
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
            let param_ty = self.ctx.type_registry.intern(TypeKind::Param(param.name));
            let info = SymbolInfo {
                kind: SymbolKind::TypeParam,
                node_id: self.ctx.next_node_id(),
                type_id: param_ty,
                def_id: None,
                span: param.span,
                vis: Visibility::Private,
                is_mut: false,
            };
            let _ = self.ctx.scopes.define(param.name, info);
        }
    }

    pub(super) fn resolve_where_clauses(&mut self, clauses: &[ast::WhereClause], scope: ScopeId) {
        for clause in clauses {
            self.resolve_type(&clause.target_ty, scope);
            for bound in &clause.bounds {
                self.resolve_type(bound, scope);
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
                .with_hint("in Kern, you must explicitly use a pointer for dynamic dispatch, e.g., `*Trait` or `*mut Trait`")
                .emit();
        }
    }
}
