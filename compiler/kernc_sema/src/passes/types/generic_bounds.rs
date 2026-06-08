//! Generic argument bound validation.
//!
//! After generic arguments are resolved, this module substitutes them into
//! where-clauses and asks the trait solver whether each concrete obligation is
//! satisfied.  Parametric arguments are deferred until instantiation.

use super::*;

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    pub(super) fn check_type_generic_bounds(
        &mut self,
        span: Span,
        def_id: DefId,
        arg_values: &[GenericArg],
    ) -> bool {
        let Some((item_name, generics, where_clauses, kind_name)) =
            self.generic_def_bounds_info(def_id)
        else {
            return true;
        };

        if generics.len() != arg_values.len() {
            self.ctx.emit_error(
                span,
                format!(
                    "{} `{}` expects {} generic arguments, but {} were provided",
                    kind_name,
                    item_name,
                    generics.len(),
                    arg_values.len()
                ),
            );
            return false;
        }

        if arg_values
            .iter()
            .copied()
            .any(|arg| self.generic_arg_contains_params(arg))
        {
            return true;
        }

        if where_clauses.is_empty() {
            return true;
        }

        self.ensure_where_clause_types_resolved(def_id, &generics, &where_clauses);

        let mut map = HashMap::new();
        for (param, arg_value) in generics.iter().zip(arg_values.iter()) {
            map.insert(param.name, *arg_value);
        }

        let mut pairs_to_check = Vec::new();
        {
            for clause in where_clauses {
                let original_target = self.ctx.node_type_or_error(clause.target_ty.id);
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                let sub_target = subst.substitute(original_target);

                for bound_ast in clause.bounds {
                    let original_bound = self.ctx.node_type_or_error(bound_ast.id);
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
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
        // SAFETY: resolving the impl signature may mutably borrow `ctx`, but it does not reorder
        // or remove definitions. The pointer targets the same `ImplDef` entry after that work.
        let impl_def = unsafe { &*impl_ptr };
        let Some(parent_module) = impl_def.parent_module else {
            return;
        };
        let Def::Module(module_def) = &self.ctx.defs[parent_module.0 as usize] else {
            return;
        };

        let have_target = self.ctx.has_node_type(impl_def.target_type.id);
        let have_trait = impl_def
            .trait_type
            .as_ref()
            .is_none_or(|trait_ty| self.ctx.has_node_type(trait_ty.id));
        let have_bounds = impl_def.where_clauses.iter().all(|clause| {
            self.ctx.has_node_type(clause.target_ty.id)
                && clause
                    .bounds
                    .iter()
                    .all(|bound| self.ctx.has_node_type(bound.id))
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
            !self.ctx.has_node_type(clause.target_ty.id)
                || clause
                    .bounds
                    .iter()
                    .any(|bound| !self.ctx.has_node_type(bound.id))
        });
        if !needs_resolution {
            return;
        }

        let Some(owner_scope) = self.def_owner_module_scope(def_id) else {
            return;
        };

        self.ctx.scopes.set_current_scope(owner_scope);
        let item_scope = self.ctx.scopes.enter_scope();

        let trait_span = match &self.ctx.defs[def_id.0 as usize] {
            Def::Trait(trait_def) => Some(trait_def.span),
            _ => None,
        };
        if let Some(trait_span) = trait_span {
            let self_args = generics
                .iter()
                .map(|param| self.generic_param_placeholder_arg(param, item_scope))
                .collect::<Vec<_>>();
            let self_ty =
                self.ctx
                    .type_registry
                    .intern(TypeKind::TraitObject(def_id, self_args, Vec::new()));
            self.bind_self_type(self_ty, item_scope, trait_span);
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

    pub(super) fn type_contains_params(&mut self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm).clone() {
            TypeKind::Param(_) | TypeKind::TypeVar(_) => true,
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::Alias(_, elem)
            | TypeKind::AnonymousEnumPayload(elem) => self.type_contains_params(elem),
            TypeKind::Array { elem, len, .. } => {
                self.type_contains_params(elem) || self.const_generic_contains_params(len)
            }
            TypeKind::ArrayInfer { elem, .. } => self.type_contains_params(elem),
            TypeKind::Range { start, end, .. } => {
                start.is_some_and(|ty| self.type_contains_params(ty))
                    || end.is_some_and(|ty| self.type_contains_params(ty))
            }
            TypeKind::Def(_, args)
            | TypeKind::Enum(_, args)
            | TypeKind::Associated(_, args)
            | TypeKind::FnDef(_, args) => args
                .into_iter()
                .any(|arg| self.generic_arg_contains_params(arg)),
            TypeKind::TraitObject(_, args, assoc_bindings) => {
                args.into_iter()
                    .any(|arg| self.generic_arg_contains_params(arg))
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
                        .any(|arg| self.generic_arg_contains_params(arg))
                    || assoc_args
                        .into_iter()
                        .any(|arg| self.generic_arg_contains_params(arg))
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

    fn const_generic_contains_params(&mut self, value: ConstGeneric) -> bool {
        self.ctx.type_registry.const_generic_contains_params(value)
    }

    fn generic_arg_contains_params(&mut self, arg: GenericArg) -> bool {
        match arg {
            GenericArg::Type(ty) => ty == TypeId::ERROR || self.type_contains_params(ty),
            GenericArg::Const(value) => self.const_generic_contains_params(value),
        }
    }
}
