use super::*;
use crate::ty::{ConstGeneric, GenericArg, LayoutEngine};

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    pub(super) fn instantiate_trait_assoc_contract_ty(
        &mut self,
        ty: TypeId,
        generic_args: &HashMap<SymbolId, GenericArg>,
        assoc_targets: &HashMap<DefId, TypeId>,
        trait_def_id: DefId,
        trait_args: &[GenericArg],
        self_ty: TypeId,
    ) -> TypeId {
        let ty = {
            let mut subst = Substituter::new(&mut self.ctx.type_registry, generic_args);
            subst.substitute(ty)
        };
        let ty = crate::checker::substitute_associated_types(
            &mut self.ctx.type_registry,
            ty,
            assoc_targets,
        );
        self.substitute_trait_assoc_contract_self(ty, trait_def_id, trait_args, self_ty)
    }

    fn substitute_trait_assoc_contract_self_arg(
        &mut self,
        arg: GenericArg,
        trait_def_id: DefId,
        trait_args: &[GenericArg],
        self_ty: TypeId,
    ) -> GenericArg {
        match arg {
            GenericArg::Type(ty) => GenericArg::Type(self.substitute_trait_assoc_contract_self(
                ty,
                trait_def_id,
                trait_args,
                self_ty,
            )),
            GenericArg::Const(value) => GenericArg::Const(value),
        }
    }

    fn substitute_trait_assoc_contract_self(
        &mut self,
        ty: TypeId,
        trait_def_id: DefId,
        trait_args: &[GenericArg],
        self_ty: TypeId,
    ) -> TypeId {
        let kind = self.ctx.type_registry.get(ty).clone();
        match kind {
            TypeKind::Primitive(_)
            | TypeKind::Simd { .. }
            | TypeKind::Error
            | TypeKind::Module(_)
            | TypeKind::TypeVar(_)
            | TypeKind::Param(_) => ty,
            TypeKind::Associated(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.substitute_trait_assoc_contract_self_arg(
                            arg,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::Associated(def_id, new_args))
            }
            TypeKind::Pointer { is_mut, elem } => {
                let new_elem = self.substitute_trait_assoc_contract_self(
                    elem,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut,
                    elem: new_elem,
                })
            }
            TypeKind::VolatilePtr { is_mut, elem } => {
                let new_elem = self.substitute_trait_assoc_contract_self(
                    elem,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                    is_mut,
                    elem: new_elem,
                })
            }
            TypeKind::Slice { is_mut, elem } => {
                let new_elem = self.substitute_trait_assoc_contract_self(
                    elem,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx.type_registry.intern(TypeKind::Slice {
                    is_mut,
                    elem: new_elem,
                })
            }
            TypeKind::Array { is_mut, elem, len } => {
                let new_elem = self.substitute_trait_assoc_contract_self(
                    elem,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx.type_registry.intern(TypeKind::Array {
                    is_mut,
                    elem: new_elem,
                    len,
                })
            }
            TypeKind::ArrayInfer { is_mut, elem } => {
                let new_elem = self.substitute_trait_assoc_contract_self(
                    elem,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx.type_registry.intern(TypeKind::ArrayInfer {
                    is_mut,
                    elem: new_elem,
                })
            }
            TypeKind::Function {
                params,
                ret,
                is_variadic,
            } => {
                let new_params = params
                    .into_iter()
                    .map(|param| {
                        self.substitute_trait_assoc_contract_self(
                            param,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                let new_ret = self.substitute_trait_assoc_contract_self(
                    ret,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx.type_registry.intern(TypeKind::Function {
                    params: new_params,
                    ret: new_ret,
                    is_variadic,
                })
            }
            TypeKind::Def(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.substitute_trait_assoc_contract_self_arg(
                            arg,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::Def(def_id, new_args))
            }
            TypeKind::Enum(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.substitute_trait_assoc_contract_self_arg(
                            arg,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::Enum(def_id, new_args))
            }
            TypeKind::EnumPayload(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.substitute_trait_assoc_contract_self_arg(
                            arg,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::EnumPayload(def_id, new_args))
            }
            TypeKind::FnDef(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.substitute_trait_assoc_contract_self_arg(
                            arg,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::FnDef(def_id, new_args))
            }
            TypeKind::Alias(name, target) => {
                let new_target = self.substitute_trait_assoc_contract_self(
                    target,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx
                    .type_registry
                    .intern(TypeKind::Alias(name, new_target))
            }
            TypeKind::TraitObject(def_id, args, assoc_bindings) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.substitute_trait_assoc_contract_self_arg(
                            arg,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect::<Vec<_>>();
                let new_assoc_bindings = assoc_bindings
                    .into_iter()
                    .map(|(assoc_def_id, assoc_ty)| {
                        (
                            assoc_def_id,
                            self.substitute_trait_assoc_contract_self(
                                assoc_ty,
                                trait_def_id,
                                trait_args,
                                self_ty,
                            ),
                        )
                    })
                    .collect::<Vec<_>>();
                if def_id == trait_def_id
                    && new_assoc_bindings.is_empty()
                    && new_args.as_slice() == trait_args
                {
                    return self_ty;
                }
                self.ctx.type_registry.intern(TypeKind::TraitObject(
                    def_id,
                    new_args,
                    new_assoc_bindings,
                ))
            }
            TypeKind::Projection {
                target,
                trait_def_id: projection_trait_def_id,
                trait_args: projection_trait_args,
                assoc_def_id,
                assoc_args,
            } => {
                let new_target = self.substitute_trait_assoc_contract_self(
                    target,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                let new_trait_args = projection_trait_args
                    .into_iter()
                    .map(|arg| {
                        self.substitute_trait_assoc_contract_self_arg(
                            arg,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                let new_assoc_args = assoc_args
                    .into_iter()
                    .map(|arg| {
                        self.substitute_trait_assoc_contract_self_arg(
                            arg,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                self.ctx.type_registry.intern(TypeKind::Projection {
                    target: new_target,
                    trait_def_id: projection_trait_def_id,
                    trait_args: new_trait_args,
                    assoc_def_id,
                    assoc_args: new_assoc_args,
                })
            }
            TypeKind::ClosureInterface { params, ret } => {
                let new_params = params
                    .into_iter()
                    .map(|param| {
                        self.substitute_trait_assoc_contract_self(
                            param,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                let new_ret = self.substitute_trait_assoc_contract_self(
                    ret,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx.type_registry.intern(TypeKind::ClosureInterface {
                    params: new_params,
                    ret: new_ret,
                })
            }
            TypeKind::AnonymousState {
                closure_node_id,
                captures,
                params,
                ret,
            } => {
                let new_captures = captures
                    .into_iter()
                    .map(|capture| {
                        self.substitute_trait_assoc_contract_self(
                            capture,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                let new_params = params
                    .into_iter()
                    .map(|param| {
                        self.substitute_trait_assoc_contract_self(
                            param,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                let new_ret = self.substitute_trait_assoc_contract_self(
                    ret,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx.type_registry.intern(TypeKind::AnonymousState {
                    closure_node_id,
                    captures: new_captures,
                    params: new_params,
                    ret: new_ret,
                })
            }
            TypeKind::AnonymousStruct(is_extern, fields) => {
                let new_fields = fields
                    .into_iter()
                    .map(|field| crate::ty::AnonymousField {
                        name: field.name,
                        ty: self.substitute_trait_assoc_contract_self(
                            field.ty,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        ),
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousStruct(is_extern, new_fields))
            }
            TypeKind::AnonymousUnion(is_extern, fields) => {
                let new_fields = fields
                    .into_iter()
                    .map(|field| crate::ty::AnonymousField {
                        name: field.name,
                        ty: self.substitute_trait_assoc_contract_self(
                            field.ty,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        ),
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousUnion(is_extern, new_fields))
            }
            TypeKind::AnonymousEnum(enum_def) => {
                let new_backing_ty = enum_def.backing_ty.map(|backing_ty| {
                    self.substitute_trait_assoc_contract_self(
                        backing_ty,
                        trait_def_id,
                        trait_args,
                        self_ty,
                    )
                });
                let new_variants = enum_def
                    .variants
                    .into_iter()
                    .map(|variant| crate::ty::AnonymousVariant {
                        name: variant.name,
                        name_span: variant.name_span,
                        payload_ty: variant.payload_ty.map(|payload_ty| {
                            self.substitute_trait_assoc_contract_self(
                                payload_ty,
                                trait_def_id,
                                trait_args,
                                self_ty,
                            )
                        }),
                        explicit_value: variant.explicit_value,
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousEnum(crate::ty::AnonymousEnum {
                        backing_ty: new_backing_ty,
                        builtin: enum_def.builtin,
                        variants: new_variants,
                    }))
            }
            TypeKind::AnonymousEnumPayload(enum_ty) => {
                let substituted = self.substitute_trait_assoc_contract_self(
                    enum_ty,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousEnumPayload(substituted))
            }
        }
    }

    pub(super) fn push_instantiated_where_bounds(
        &mut self,
        where_clauses: &[ast::WhereClause],
        generic_args: &HashMap<SymbolId, GenericArg>,
        assoc_targets: &HashMap<DefId, TypeId>,
        trait_def_id: DefId,
        trait_args: &[GenericArg],
        self_ty: TypeId,
    ) -> usize {
        let prev_bounds_len = self.ctx.active_bounds.len();
        for clause in where_clauses {
            let target_ty = self
                .ctx
                .node_types
                .get(&clause.target_ty.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let instantiated_target = self.instantiate_trait_assoc_contract_ty(
                target_ty,
                generic_args,
                assoc_targets,
                trait_def_id,
                trait_args,
                self_ty,
            );
            let mut bounds = Vec::new();
            for bound in &clause.bounds {
                let bound_ty = self
                    .ctx
                    .node_types
                    .get(&bound.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let instantiated_bound = self.instantiate_trait_assoc_contract_ty(
                    bound_ty,
                    generic_args,
                    assoc_targets,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                bounds.push(self.ctx.type_registry.normalize(instantiated_bound));
            }
            self.ctx.active_bounds.push((
                self.ctx.type_registry.normalize(instantiated_target),
                bounds,
            ));
        }
        if self.ctx.active_bounds.len() != prev_bounds_len {
            self.ctx.clear_active_bound_caches();
        }
        prev_bounds_len
    }

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

    fn type_contains_params(&mut self, ty: TypeId) -> bool {
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
            let _ = self.ctx.scopes.define(param.name, info);
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
                    self.ctx.node_types.remove(&ty_node.id);
                    self.resolve_type(ty_node, scope)
                })
            }
            _ => {
                self.ctx.node_types.remove(&ty_node.id);
                self.resolve_type(ty_node, scope)
            }
        };
        self.ctx.node_types.insert(ty_node.id, ty);
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
                    self.ctx.node_types.insert(bound.id, TypeId::ERROR);
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
                .with_hint("in Kern, you must explicitly use a pointer for dynamic dispatch, e.g., `*Trait` or `*mut Trait`")
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
