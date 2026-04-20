use super::*;
use kernc_utils::FastHashMap;

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    /// Run the full type-resolution pass in two stages.
    pub fn resolve_all(&mut self) {
        let module_ids = self.collect_module_ids();
        self.resolve_module_pass(&module_ids, true);
        self.resolve_module_pass(&module_ids, false);
        self.validate_supertrait_graph();
        self.validate_trait_impl_orphans();
        self.validate_trait_impl_coherence();
        self.validate_trait_impl_supertrait_contracts();
        self.validate_impl_associated_type_targets();
    }

    fn collect_module_ids(&self) -> Vec<DefId> {
        self.ctx
            .defs
            .iter()
            .filter_map(|def| {
                if let Def::Module(m) = def {
                    Some(m.id)
                } else {
                    None
                }
            })
            .collect()
    }

    fn resolve_module_pass(&mut self, module_ids: &[DefId], aliases_only: bool) {
        for &mod_id in module_ids {
            let Some((mod_scope, items)) = self.module_scope_and_items(mod_id) else {
                continue;
            };

            for item_id in items {
                let is_alias = matches!(self.ctx.defs[item_id.0 as usize], Def::TypeAlias(_));
                if aliases_only == is_alias {
                    self.resolve_item(item_id, mod_scope);
                }
            }
        }
    }

    fn module_scope_and_items(&mut self, mod_id: DefId) -> Option<(ScopeId, Vec<DefId>)> {
        if let Def::Module(m) = &self.ctx.defs[mod_id.0 as usize] {
            Some((m.scope_id, m.items.clone()))
        } else {
            self.ctx.emit_ice(
                Span::default(),
                format!("TypeResolver expected DefId {:?} to be a module", mod_id),
            );
            None
        }
    }

    fn resolve_item(&mut self, item_id: DefId, parent_scope: ScopeId) {
        let Some(def_ptr) = self
            .ctx
            .defs
            .get(item_id.0 as usize)
            .map(std::ptr::from_ref)
        else {
            return;
        };

        // Safety: type resolution mutates inference state and selected `resolved_*` fields, but
        // it never reorders or removes entries from `ctx.defs`. Raw pointers let us inspect the
        // existing definition payloads without cloning the full AST-backed items first.
        unsafe {
            match &*def_ptr {
                Def::Function(f) => self.resolve_function_item(item_id, f, parent_scope),
                Def::Struct(s) => self.resolve_struct_item(item_id, s, parent_scope),
                Def::Union(u) => self.resolve_union_item(item_id, u, parent_scope),
                Def::Trait(t) => self.resolve_trait_item(item_id, t, parent_scope),
                Def::TypeAlias(t) => self.resolve_type_alias_item(t, parent_scope),
                Def::Impl(i) => self.resolve_impl_item(i, parent_scope),
                Def::Enum(a) => self.resolve_enum_item(item_id, a, parent_scope),
                Def::AssociatedType(_) | Def::Global(_) | Def::Module(_) => {}
            }
        }
    }

    fn resolve_function_item(&mut self, item_id: DefId, f: &FunctionDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let func_scope = self.ctx.scopes.enter_scope();

        self.bind_generics(&f.generics, func_scope);
        self.resolve_where_clauses(&f.where_clauses, func_scope);
        if let Some(parent_id) = f.parent
            && let Def::Impl(i) = &self.ctx.defs[parent_id.0 as usize]
        {
            let target_ty = self
                .ctx
                .node_types
                .get(&i.target_type.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            self.bind_self_type(target_ty, func_scope, f.span);
        }

        let mut param_tys = Vec::new();
        for param in &f.params {
            let p_ty = self.resolve_type(&param.type_node, func_scope);
            self.ensure_sized(p_ty, param.type_node.span);
            param_tys.push(p_ty);
        }
        let ret_ty = self.resolve_type(&f.ret_type, func_scope);
        if ret_ty != TypeId::VOID {
            self.ensure_sized(ret_ty, f.ret_type.span);
        }

        let sig_ty = self.ctx.type_registry.intern(TypeKind::Function {
            params: param_tys,
            ret: ret_ty,
            is_variadic: f.is_variadic,
        });

        if let Def::Function(updated_f) = &mut self.ctx.defs[item_id.0 as usize] {
            updated_f.resolved_sig = Some(sig_ty);
        }

        self.ctx.scopes.exit_scope();

        let gen_args = f
            .generics
            .iter()
            .map(|param| self.generic_param_placeholder_arg(param, func_scope))
            .collect::<Vec<_>>();
        let fn_def_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::FnDef(item_id, gen_args));

        self.ctx.scopes.set_current_scope(parent_scope);

        let is_impl_method = f
            .parent
            .is_some_and(|p_id| matches!(self.ctx.defs[p_id.0 as usize], Def::Impl(_)));
        if !is_impl_method {
            self.ctx.scopes.update_type(f.name, fn_def_ty);
        }
    }

    fn resolve_struct_item(&mut self, item_id: DefId, s: &StructDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let struct_scope = self.ctx.scopes.enter_scope();

        self.bind_generics(&s.generics, struct_scope);
        self.resolve_where_clauses(&s.where_clauses, struct_scope);

        for field in &s.fields {
            let f_ty = self.resolve_type(&field.type_node, struct_scope);
            self.ensure_sized(f_ty, field.type_node.span);
            if let Some(def_val) = &field.default_value {
                self.resolve_expr(def_val, struct_scope);
            }
        }
        self.ctx.scopes.exit_scope();

        let struct_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Def(item_id, Vec::new()));
        self.ctx.scopes.set_current_scope(parent_scope);
        self.ctx.scopes.update_type(s.name, struct_ty);
    }

    fn resolve_union_item(&mut self, item_id: DefId, u: &UnionDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let union_scope = self.ctx.scopes.enter_scope();

        self.bind_generics(&u.generics, union_scope);
        self.resolve_where_clauses(&u.where_clauses, union_scope);

        for field in &u.fields {
            let f_ty = self.resolve_type(&field.type_node, union_scope);
            self.ensure_sized(f_ty, field.type_node.span);
            if let Some(def_val) = &field.default_value {
                self.resolve_expr(def_val, union_scope);
            }
        }
        self.ctx.scopes.exit_scope();

        let union_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Def(item_id, Vec::new()));
        self.ctx.scopes.set_current_scope(parent_scope);
        self.ctx.scopes.update_type(u.name, union_ty);
    }

    fn resolve_trait_item(&mut self, item_id: DefId, t: &TraitDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let trait_scope = self.ctx.scopes.enter_scope();

        self.bind_generics(&t.generics, trait_scope);
        let self_args = t
            .generics
            .iter()
            .map(|param| self.generic_param_placeholder_arg(param, trait_scope))
            .collect::<Vec<_>>();
        let self_ty =
            self.ctx
                .type_registry
                .intern(TypeKind::TraitObject(item_id, self_args, Vec::new()));
        self.bind_self_type(self_ty, trait_scope, t.span);
        self.resolve_where_clauses(&t.where_clauses, trait_scope);
        self.bind_trait_assoc_types(&t.assoc_types, trait_scope);
        self.resolve_assoc_type_bounds(&t.assoc_types, trait_scope);

        let mut resolved_supertraits = Vec::new();
        for supertrait in &t.supertraits {
            resolved_supertraits.push(self.resolve_type(supertrait, trait_scope));
        }

        let mut resolved_methods = Vec::new();
        for method in &t.methods {
            let sig_ty = self.resolve_type(&method.type_node, trait_scope);
            resolved_methods.push((method.name, sig_ty));
        }
        self.ctx.scopes.exit_scope();

        if let Def::Trait(updated_t) = &mut self.ctx.defs[item_id.0 as usize] {
            updated_t.resolved_methods = resolved_methods;
            updated_t.resolved_supertraits = resolved_supertraits;
        }
    }

    fn resolve_type_alias_item(&mut self, t: &TypeAliasDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let alias_scope = self.ctx.scopes.enter_scope();

        self.bind_generics(&t.generics, alias_scope);
        self.resolve_where_clauses(&t.where_clauses, alias_scope);
        let target_ty = self.resolve_type(&t.target, alias_scope);

        self.ctx.scopes.exit_scope();
        self.ctx.scopes.set_current_scope(parent_scope);
        self.ctx.scopes.update_type(t.name, target_ty);
    }

    fn resolve_impl_item(&mut self, i: &ImplDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let impl_scope = self.ctx.scopes.enter_scope();

        self.bind_generics(&i.generics, impl_scope);
        self.resolve_where_clauses(&i.where_clauses, impl_scope);

        let target_ty_id = self.resolve_type(&i.target_type, impl_scope);
        self.bind_self_type(target_ty_id, impl_scope, i.span);

        let mut resolved_trait_ty = None;
        if let Some(trait_ty) = &i.trait_type {
            let resolved = self.resolve_type(trait_ty, impl_scope);
            self.reject_explicit_builtin_numeric_marker_impl(i, resolved, trait_ty.span);
            resolved_trait_ty = Some(resolved);
        }

        let canonical_trait_ty =
            self.bind_impl_assoc_types(i, &i.assoc_types, resolved_trait_ty, impl_scope, i.span);
        if let (Some(trait_ty), Some(canonical_trait_ty)) = (&i.trait_type, canonical_trait_ty) {
            self.ctx.node_types.insert(trait_ty.id, canonical_trait_ty);
        }

        for &method_id in &i.methods {
            self.resolve_item(method_id, impl_scope);
        }

        self.ctx.scopes.exit_scope();
    }

    fn reject_explicit_builtin_numeric_marker_impl(
        &mut self,
        impl_def: &ImplDef,
        trait_ty: TypeId,
        span: Span,
    ) {
        let trait_norm = self.ctx.type_registry.normalize(trait_ty);
        let TypeKind::TraitObject(trait_def_id, _, _) =
            self.ctx.type_registry.get(trait_norm).clone()
        else {
            return;
        };

        let Some(Def::Trait(trait_def)) = self.ctx.defs.get(trait_def_id.0 as usize) else {
            return;
        };
        if !trait_def.is_builtin {
            return;
        }

        let trait_name = self.ctx.resolve(trait_def.name).to_string();
        if !matches!(
            trait_name.as_str(),
            "Integer" | "SignedInteger" | "UnsignedInteger" | "Float"
        ) {
            return;
        }

        self.ctx
            .struct_error(
                span,
                format!(
                    "builtin numeric marker trait `{}` cannot be implemented explicitly",
                    trait_name
                ),
            )
            .with_hint(format!(
                "`{}` is assigned by the compiler for builtin numeric types only",
                trait_name
            ))
            .with_hint(
                "define a normal trait if you need a user-extensible numeric-like abstraction",
            )
            .with_span_label(impl_def.span, "while checking this impl")
            .emit();
    }

    fn check_impl_assoc_type_contracts(
        &mut self,
        impl_def: &ImplDef,
        trait_def_id: DefId,
        trait_generics: &[ast::GenericParam],
        trait_args: &[GenericArg],
        trait_assoc: &AssociatedTypeDef,
        impl_assoc: &AssociatedTypeDef,
        resolved_target: TypeId,
        assoc_targets: &HashMap<DefId, TypeId>,
    ) {
        if resolved_target == TypeId::ERROR {
            return;
        }

        let trait_generic_args = trait_generics
            .iter()
            .zip(trait_args.iter())
            .map(|(param, arg)| (param.name, *arg))
            .collect::<HashMap<_, _>>();

        let prev_bounds_len = self.push_impl_context_where_bounds(impl_def);
        for clause in &impl_assoc.where_clauses {
            let target_ty = self.ctx.type_registry.normalize(
                self.ctx
                    .node_types
                    .get(&clause.target_ty.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR),
            );
            let bounds = clause
                .bounds
                .iter()
                .map(|bound| {
                    self.ctx.type_registry.normalize(
                        self.ctx
                            .node_types
                            .get(&bound.id)
                            .copied()
                            .unwrap_or(TypeId::ERROR),
                    )
                })
                .collect::<Vec<_>>();
            self.ctx.active_bounds.push((target_ty, bounds));
        }
        self.push_instantiated_where_bounds(
            &trait_assoc.where_clauses,
            &trait_generic_args,
            assoc_targets,
            trait_def_id,
            trait_args,
            resolved_target,
        );
        if self.ctx.active_bounds.len() != prev_bounds_len {
            self.ctx.clear_active_bound_caches();
        }

        let assoc_name = self.ctx.resolve(impl_assoc.name).to_string();
        for &bound_ty in &trait_assoc.resolved_bounds {
            let instantiated_bound = self.instantiate_trait_assoc_contract_ty(
                bound_ty,
                &trait_generic_args,
                assoc_targets,
                trait_def_id,
                trait_args,
                resolved_target,
            );
            let instantiated_bound = self.ctx.type_registry.normalize(instantiated_bound);
            if instantiated_bound == TypeId::ERROR {
                continue;
            }

            let bound_ok = {
                let mut checker = ExprChecker::new(self.ctx, None);
                checker.check_trait_impl(resolved_target, instantiated_bound)
            };
            if bound_ok {
                continue;
            }

            let target_str = self.ctx.ty_to_string(resolved_target);
            let bound_str = self.ctx.ty_to_string(instantiated_bound);
            self.ctx
                .struct_error(
                    impl_assoc.span,
                    format!(
                        "associated type `{}` does not satisfy the bounds declared by the trait",
                        assoc_name
                    ),
                )
                .with_span_label(
                    impl_assoc.span,
                    "this impl-associated type target does not implement the required bound",
                )
                .with_span_label(
                    trait_assoc.span,
                    "the trait declares the associated-type contract here",
                )
                .with_hint(format!("required bound: `{}: {}`", target_str, bound_str))
                .emit();
        }

        self.ctx.active_bounds.truncate(prev_bounds_len);
        self.ctx.clear_active_bound_caches();
    }

    fn push_impl_context_where_bounds(&mut self, impl_def: &ImplDef) -> usize {
        let prev_bounds_len = self.ctx.active_bounds.len();
        for clause in &impl_def.where_clauses {
            let target_ty = self.ctx.type_registry.normalize(
                self.ctx
                    .node_types
                    .get(&clause.target_ty.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR),
            );
            let bounds = clause
                .bounds
                .iter()
                .map(|bound| {
                    self.ctx.type_registry.normalize(
                        self.ctx
                            .node_types
                            .get(&bound.id)
                            .copied()
                            .unwrap_or(TypeId::ERROR),
                    )
                })
                .collect::<Vec<_>>();
            self.ctx.active_bounds.push((target_ty, bounds));
        }
        if self.ctx.active_bounds.len() != prev_bounds_len {
            self.ctx.clear_active_bound_caches();
        }
        prev_bounds_len
    }

    fn validate_trait_impl_supertrait_contracts(&mut self) {
        let trait_impl_ids = self.ctx.trait_impls.clone();
        for impl_id in trait_impl_ids {
            let Some(impl_def) = self.ctx.defs.get(impl_id.0 as usize).and_then(|def| {
                if let Def::Impl(impl_def) = def {
                    Some(impl_def.clone())
                } else {
                    None
                }
            }) else {
                continue;
            };

            let Some(trait_ty_node) = &impl_def.trait_type else {
                continue;
            };

            let resolved_target = self
                .ctx
                .type_registry
                .normalize(
                    self.ctx
                        .node_types
                        .get(&impl_def.target_type.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR),
                );
            let resolved_trait = self
                .ctx
                .type_registry
                .normalize(
                    self.ctx
                        .node_types
                        .get(&trait_ty_node.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR),
                );
            if resolved_target == TypeId::ERROR || resolved_trait == TypeId::ERROR {
                continue;
            }

            let TypeKind::TraitObject(trait_def_id, trait_args, assoc_bindings) =
                self.ctx.type_registry.get(resolved_trait).clone()
            else {
                continue;
            };
            let Some(Def::Trait(trait_def)) = self.ctx.defs.get(trait_def_id.0 as usize).cloned()
            else {
                continue;
            };
            if trait_def.resolved_supertraits.is_empty() {
                continue;
            }

            let trait_name = self.ctx.resolve(trait_def.name).to_string();
            let trait_arg_map = trait_def
                .generics
                .iter()
                .zip(trait_args.iter())
                .map(|(param, arg)| (param.name, *arg))
                .collect::<FastHashMap<_, _>>();
            let assoc_binding_map = assoc_bindings
                .into_iter()
                .collect::<FastHashMap<_, _>>();

            let prev_bounds_len = self.push_impl_context_where_bounds(&impl_def);
            for super_ty in trait_def.resolved_supertraits {
                let instantiated_super = if trait_arg_map.is_empty() {
                    super_ty
                } else {
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &trait_arg_map);
                    subst.substitute(super_ty)
                };
                let instantiated_super = crate::checker::substitute_associated_types(
                    &mut self.ctx.type_registry,
                    instantiated_super,
                    &assoc_binding_map,
                );
                let instantiated_super = crate::query::augment_trait_object_assoc_bindings_from_map(
                    self.ctx,
                    instantiated_super,
                    &assoc_binding_map,
                );
                let instantiated_super = self.ctx.type_registry.normalize(instantiated_super);
                if instantiated_super == TypeId::ERROR {
                    continue;
                }

                let super_ok = {
                    let mut checker = ExprChecker::new(self.ctx, None);
                    checker.check_trait_impl(resolved_target, instantiated_super)
                };
                if super_ok {
                    continue;
                }

                let target_str = self.ctx.ty_to_string(resolved_target);
                let super_str = self.ctx.ty_to_string(instantiated_super);
                self.ctx
                    .struct_error(
                        impl_def.span,
                        format!(
                            "impl of trait `{}` is missing a required supertrait proof",
                            trait_name
                        ),
                    )
                    .with_span_label(
                        impl_def.span,
                        "this impl does not prove the declared supertrait contract",
                    )
                    .with_hint(format!("required bound: `{}: {}`", target_str, super_str))
                    .with_hint(
                        "every trait impl must also establish each declared supertrait for the same target",
                    )
                    .emit();
            }
            self.ctx.active_bounds.truncate(prev_bounds_len);
            self.ctx.clear_active_bound_caches();
        }
    }

    fn bind_trait_assoc_types(&mut self, assoc_type_ids: &[DefId], scope: ScopeId) {
        for &assoc_id in assoc_type_ids {
            let Some(Def::AssociatedType(assoc_def)) =
                self.ctx.defs.get(assoc_id.0 as usize).cloned()
            else {
                continue;
            };
            let info = SymbolInfo {
                kind: SymbolKind::AssociatedType,
                node_id: self.ctx.next_node_id(),
                type_id: self
                    .ctx
                    .type_registry
                    .intern(TypeKind::Associated(assoc_id, vec![])),
                def_id: Some(assoc_id),
                span: assoc_def.span,
                vis: Visibility::Private,
                is_mut: false,
            };
            self.ctx.scopes.set_current_scope(scope);
            let _ = self.ctx.scopes.define(assoc_def.name, info);
        }
    }

    fn resolve_assoc_type_bounds(&mut self, assoc_type_ids: &[DefId], parent_scope: ScopeId) {
        for &assoc_id in assoc_type_ids {
            let Some(Def::AssociatedType(assoc_def)) =
                self.ctx.defs.get(assoc_id.0 as usize).cloned()
            else {
                continue;
            };
            self.ctx.scopes.set_current_scope(parent_scope);
            let assoc_scope = self.ctx.scopes.enter_scope();
            self.bind_generics(&assoc_def.generics, assoc_scope);
            self.resolve_where_clauses(&assoc_def.where_clauses, assoc_scope);
            let mut resolved_bounds = Vec::with_capacity(assoc_def.bounds.len());
            for bound in &assoc_def.bounds {
                resolved_bounds.push(self.resolve_type(bound, assoc_scope));
            }
            if let Some(target) = &assoc_def.target {
                self.resolve_type(target, assoc_scope);
            }
            self.ctx.scopes.exit_scope();
            if let Def::AssociatedType(updated) = &mut self.ctx.defs[assoc_id.0 as usize] {
                updated.resolved_bounds = resolved_bounds;
            }
        }
    }

    fn bind_impl_assoc_types(
        &mut self,
        impl_def: &ImplDef,
        assoc_type_ids: &[DefId],
        resolved_trait_ty: Option<TypeId>,
        scope: ScopeId,
        span: Span,
    ) -> Option<TypeId> {
        let mut impl_assoc_by_name = HashMap::new();
        for &assoc_id in assoc_type_ids {
            let Some(Def::AssociatedType(assoc_def)) =
                self.ctx.defs.get(assoc_id.0 as usize).cloned()
            else {
                continue;
            };
            impl_assoc_by_name.insert(assoc_def.name, assoc_id);

            let info = SymbolInfo {
                kind: SymbolKind::AssociatedType,
                node_id: self.ctx.next_node_id(),
                type_id: self
                    .ctx
                    .type_registry
                    .intern(TypeKind::Associated(assoc_id, vec![])),
                def_id: Some(assoc_id),
                span: assoc_def.span,
                vis: Visibility::Private,
                is_mut: false,
            };
            self.ctx.scopes.set_current_scope(scope);
            let _ = self.ctx.scopes.define(assoc_def.name, info);
        }

        let Some(trait_ty) = resolved_trait_ty else {
            for &assoc_id in assoc_type_ids {
                if let Some(Def::AssociatedType(assoc_def)) = self.ctx.defs.get(assoc_id.0 as usize)
                {
                    self.ctx
                        .struct_error(
                            assoc_def.span,
                            "associated type definitions require a trait impl",
                        )
                        .with_hint(
                            "write `impl Type: Trait { ... }` when defining associated types",
                        )
                        .emit();
                }
            }
            return None;
        };

        let trait_norm = self.ctx.type_registry.normalize(trait_ty);
        let TypeKind::TraitObject(trait_def_id, trait_args, _) =
            self.ctx.type_registry.get(trait_norm).clone()
        else {
            self.ctx
                .emit_error(span, "impl trait target is not a trait");
            return Some(trait_ty);
        };

        let (trait_generics, trait_assoc_ids) = match self.ctx.defs.get(trait_def_id.0 as usize) {
            Some(Def::Trait(trait_def)) => {
                (trait_def.generics.clone(), trait_def.assoc_types.clone())
            }
            _ => (Vec::new(), Vec::new()),
        };
        let mut ordered_assoc_targets = vec![None; trait_assoc_ids.len()];

        let mut trait_assoc_names = HashMap::new();
        for (assoc_index, trait_assoc_id) in trait_assoc_ids.iter().copied().enumerate() {
            let Some(Def::AssociatedType(trait_assoc)) =
                self.ctx.defs.get(trait_assoc_id.0 as usize).cloned()
            else {
                continue;
            };
            trait_assoc_names.insert(trait_assoc.name, trait_assoc_id);

            let Some(&impl_assoc_id) = impl_assoc_by_name.get(&trait_assoc.name) else {
                let _ = assoc_index;
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "missing associated type definition `{}` in impl",
                            self.ctx.resolve(trait_assoc.name)
                        ),
                    )
                    .emit();
                continue;
            };

            let Some(Def::AssociatedType(impl_assoc)) =
                self.ctx.defs.get(impl_assoc_id.0 as usize).cloned()
            else {
                continue;
            };

            if trait_assoc.generics.len() != impl_assoc.generics.len() {
                self.ctx
                    .struct_error(
                        impl_assoc.span,
                        format!(
                            "associated type `{}` expects {} generic parameters, but impl provides {}",
                            self.ctx.resolve(trait_assoc.name),
                            trait_assoc.generics.len(),
                            impl_assoc.generics.len()
                        ),
                    )
                    .emit();
            }

            if let Def::AssociatedType(updated) = &mut self.ctx.defs[impl_assoc_id.0 as usize] {
                updated.parent_trait = Some(trait_def_id);
            }
        }

        for (&impl_assoc_name, &impl_assoc_id) in &impl_assoc_by_name {
            if !trait_assoc_names.contains_key(&impl_assoc_name)
                && let Some(Def::AssociatedType(impl_assoc)) =
                    self.ctx.defs.get(impl_assoc_id.0 as usize)
            {
                self.ctx
                    .struct_error(
                        impl_assoc.span,
                        format!(
                            "associated type `{}` is not declared by the target trait",
                            self.ctx.resolve(impl_assoc_name)
                        ),
                    )
                    .emit();
            }
        }

        let mut resolved_impl_assoc_targets = HashMap::new();
        for &assoc_id in assoc_type_ids {
            let Some(Def::AssociatedType(assoc_def)) =
                self.ctx.defs.get(assoc_id.0 as usize).cloned()
            else {
                continue;
            };
            self.ctx.scopes.set_current_scope(scope);
            let assoc_scope = self.ctx.scopes.enter_scope();
            let prev_suppress_unqualified_impl_assoc_types =
                self.suppress_unqualified_impl_assoc_types;
            self.suppress_unqualified_impl_assoc_types = true;
            self.bind_generics(&assoc_def.generics, assoc_scope);
            self.resolve_where_clauses(&assoc_def.where_clauses, assoc_scope);
            let mut resolved_bounds = Vec::with_capacity(assoc_def.bounds.len());
            for bound in &assoc_def.bounds {
                resolved_bounds.push(self.resolve_type(bound, assoc_scope));
            }
            let resolved_target = assoc_def
                .target
                .as_ref()
                .map(|target| self.resolve_type(target, assoc_scope));
            self.suppress_unqualified_impl_assoc_types = prev_suppress_unqualified_impl_assoc_types;
            self.ctx.scopes.exit_scope();
            if let Some(resolved_target) = resolved_target {
                self.ctx.scopes.set_current_scope(scope);
                self.ctx.scopes.update_type(assoc_def.name, resolved_target);
                resolved_impl_assoc_targets.insert(assoc_def.name, resolved_target);
            }
            if let Def::AssociatedType(updated) = &mut self.ctx.defs[assoc_id.0 as usize] {
                updated.resolved_bounds = resolved_bounds;
            }
        }

        let resolved_trait_assoc_targets = resolved_impl_assoc_targets
            .iter()
            .filter_map(|(&assoc_name, &resolved_target)| {
                trait_assoc_names
                    .get(&assoc_name)
                    .copied()
                    .map(|assoc_id| (assoc_id, resolved_target))
            })
            .collect::<HashMap<_, _>>();

        let generic_args = trait_args
            .iter()
            .take(trait_generics.len())
            .copied()
            .collect::<Vec<_>>();
        let trait_assoc_ids = match self.ctx.defs.get(trait_def_id.0 as usize) {
            Some(Def::Trait(trait_def)) => trait_def.assoc_types.clone(),
            _ => Vec::new(),
        };
        for (assoc_index, trait_assoc_id) in trait_assoc_ids.iter().copied().enumerate() {
            let Some(Def::AssociatedType(trait_assoc)) =
                self.ctx.defs.get(trait_assoc_id.0 as usize).cloned()
            else {
                continue;
            };
            if let Some(&resolved_target) = resolved_impl_assoc_targets.get(&trait_assoc.name) {
                ordered_assoc_targets[assoc_index] = Some(resolved_target);
                if let Some(&impl_assoc_id) = impl_assoc_by_name.get(&trait_assoc.name)
                    && let Some(Def::AssociatedType(impl_assoc)) =
                        self.ctx.defs.get(impl_assoc_id.0 as usize).cloned()
                {
                    self.check_impl_assoc_type_contracts(
                        impl_def,
                        trait_def_id,
                        &trait_generics,
                        &trait_args,
                        &trait_assoc,
                        &impl_assoc,
                        resolved_target,
                        &resolved_trait_assoc_targets,
                    );
                }
            }
        }

        let assoc_bindings = trait_assoc_ids
            .iter()
            .copied()
            .zip(ordered_assoc_targets)
            .filter_map(|(assoc_id, target)| target.map(|ty| (assoc_id, ty)))
            .collect::<Vec<_>>();
        Some(self.ctx.type_registry.intern(TypeKind::TraitObject(
            trait_def_id,
            generic_args,
            assoc_bindings,
        )))
    }

    fn resolve_enum_item(&mut self, item_id: DefId, a: &EnumDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let adt_scope = self.ctx.scopes.enter_scope();

        self.bind_generics(&a.generics, adt_scope);
        self.resolve_where_clauses(&a.where_clauses, adt_scope);

        if let Some(backing_ty) = &a.backing_type {
            let resolved_ty = self.resolve_type(backing_ty, adt_scope);
            if !self.ctx.type_registry.is_integer(resolved_ty) && resolved_ty != TypeId::ERROR {
                self.ctx
                    .emit_error(backing_ty.span, "Enum backing type must be an integer");
            }
        }

        for variant in &a.variants {
            if let Some(payload_ty) = &variant.payload_type {
                let resolved_payload = self.resolve_type(payload_ty, adt_scope);
                self.ensure_sized(resolved_payload, payload_ty.span);
            }
        }

        self.ctx.scopes.exit_scope();

        let adt_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Enum(item_id, Vec::new()));

        self.ctx.scopes.set_current_scope(parent_scope);
        self.ctx.scopes.update_type(a.name, adt_ty);
    }

    fn validate_trait_impl_coherence(&mut self) {
        let trait_impl_ids = self.ctx.trait_impls.clone();
        for (index, left_impl_id) in trait_impl_ids.iter().copied().enumerate() {
            for right_impl_id in trait_impl_ids.iter().copied().skip(index + 1) {
                let Some(overlap) = self.overlapping_trait_impl_pair(left_impl_id, right_impl_id)
                else {
                    continue;
                };
                // Coherence permits a unique more-specific specialization, but rejects
                // equal-rank or incomparable overlaps that would make proof search ambiguous.
                if matches!(
                    crate::query::compare_impl_specificity(self.ctx, left_impl_id, right_impl_id),
                    crate::query::ImplSpecificity::LeftMoreSpecific
                        | crate::query::ImplSpecificity::RightMoreSpecific
                ) {
                    continue;
                }

                let left_target = self.ctx.ty_to_string(overlap.left_target_ty);
                let left_trait = self.ctx.ty_to_string(overlap.left_trait_ty);
                let right_target = self.ctx.ty_to_string(overlap.right_target_ty);
                let right_trait = self.ctx.ty_to_string(overlap.right_trait_ty);

                self.ctx
                    .struct_error(
                        overlap.right_span,
                        format!(
                            "overlapping trait impls are not allowed for `{}` and `{}`",
                            right_target, right_trait
                        ),
                    )
                    .with_hint(
                        "Kern requires trait impls to be globally coherent; overlapping heads would make proof search and associated type projection ambiguous",
                    )
                    .with_span_label(
                        overlap.left_span,
                        format!("first impl head: `{} : {}`", left_target, left_trait),
                    )
                    .with_span_label(
                        overlap.right_span,
                        format!("second impl head: `{} : {}`", right_target, right_trait),
                    )
                    .emit();
            }
        }
    }

    fn validate_trait_impl_orphans(&mut self) {
        let trait_impl_ids = self.ctx.trait_impls.clone();
        for impl_id in trait_impl_ids {
            let Some(impl_def) = self.ctx.defs.get(impl_id.0 as usize).and_then(|def| {
                if let Def::Impl(impl_def) = def {
                    Some(impl_def.clone())
                } else {
                    None
                }
            }) else {
                continue;
            };

            if impl_def.is_imported || impl_def.parent_module.is_none() {
                continue;
            }

            let Some(trait_ty_node) = &impl_def.trait_type else {
                continue;
            };

            let target_ty = self
                .ctx
                .node_types
                .get(&impl_def.target_type.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let trait_ty = self
                .ctx
                .node_types
                .get(&trait_ty_node.id)
                .copied()
                .unwrap_or(TypeId::ERROR);

            if target_ty == TypeId::ERROR || trait_ty == TypeId::ERROR {
                continue;
            }

            if self.trait_impl_is_orphan_legal(impl_id, target_ty, trait_ty) {
                continue;
            }

            self.ctx
                .struct_error(
                    impl_def.span,
                    format!(
                        "orphan trait impls are not allowed for `{}` and `{}`",
                        self.ctx.ty_to_string(target_ty),
                        self.ctx.ty_to_string(trait_ty)
                    ),
                )
                .with_hint(
                    "when the trait comes from another package or module root, the impl target must be anchored by a local type (directly or through builtin pointer/slice/array wrappers)",
                )
                .with_hint(
                    "this prevents downstream packages from creating competing global proofs for the same foreign trait and foreign type family",
                )
                .emit();
        }
    }

    fn validate_impl_associated_type_targets(&mut self) {
        let trait_impl_ids = self.ctx.trait_impls.clone();
        for impl_id in trait_impl_ids {
            let Some(impl_def) = self.ctx.defs.get(impl_id.0 as usize).and_then(|def| {
                if let Def::Impl(impl_def) = def {
                    Some(impl_def.clone())
                } else {
                    None
                }
            }) else {
                continue;
            };

            for assoc_id in impl_def.assoc_types {
                let Some(assoc_def) = self.ctx.defs.get(assoc_id.0 as usize).and_then(|def| {
                    if let Def::AssociatedType(assoc_def) = def {
                        Some(assoc_def.clone())
                    } else {
                        None
                    }
                }) else {
                    continue;
                };
                let Some(target) = assoc_def.target.as_ref() else {
                    continue;
                };
                let resolved_target = self
                    .ctx
                    .node_types
                    .get(&target.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                if resolved_target == TypeId::ERROR {
                    continue;
                }

                let _ = self.ctx.normalize_concrete_type(resolved_target);
            }
        }
    }

    fn overlapping_trait_impl_pair(
        &mut self,
        left_impl_id: DefId,
        right_impl_id: DefId,
    ) -> Option<OverlappingTraitImplPair> {
        let (left_impl, right_impl) = {
            let Some(left_impl) = self.ctx.defs.get(left_impl_id.0 as usize).and_then(|def| {
                if let Def::Impl(impl_def) = def {
                    Some(impl_def.clone())
                } else {
                    None
                }
            }) else {
                return None;
            };
            let Some(right_impl) = self.ctx.defs.get(right_impl_id.0 as usize).and_then(|def| {
                if let Def::Impl(impl_def) = def {
                    Some(impl_def.clone())
                } else {
                    None
                }
            }) else {
                return None;
            };
            (left_impl, right_impl)
        };

        let Some(_) = left_impl.trait_type else {
            return None;
        };
        let Some(_) = right_impl.trait_type else {
            return None;
        };
        if left_impl.parent_module.is_none() || right_impl.parent_module.is_none() {
            return None;
        }

        let left_target_ty = self
            .ctx
            .node_types
            .get(&left_impl.target_type.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let left_trait_ty = left_impl
            .trait_type
            .as_ref()
            .and_then(|trait_ty| self.ctx.node_types.get(&trait_ty.id).copied())
            .unwrap_or(TypeId::ERROR);
        let right_target_ty = self
            .ctx
            .node_types
            .get(&right_impl.target_type.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let right_trait_ty = right_impl
            .trait_type
            .as_ref()
            .and_then(|trait_ty| self.ctx.node_types.get(&trait_ty.id).copied())
            .unwrap_or(TypeId::ERROR);
        let left_trait_head_ty = crate::query::erase_trait_assoc_bindings(self.ctx, left_trait_ty);
        let right_trait_head_ty =
            crate::query::erase_trait_assoc_bindings(self.ctx, right_trait_ty);

        if matches!(
            (
                left_target_ty,
                left_trait_head_ty,
                right_target_ty,
                right_trait_head_ty
            ),
            (TypeId::ERROR, _, _, _)
                | (_, TypeId::ERROR, _, _)
                | (_, _, TypeId::ERROR, _)
                | (_, _, _, TypeId::ERROR)
        ) {
            return None;
        }

        let overlaps = {
            let mut checker = ExprChecker::new(self.ctx, None);
            let (left_fresh_target, left_fresh_trait) = Self::freshen_impl_head_types_for_overlap(
                &mut checker,
                &left_impl,
                left_target_ty,
                left_trait_head_ty,
            );
            let (right_fresh_target, right_fresh_trait) = Self::freshen_impl_head_types_for_overlap(
                &mut checker,
                &right_impl,
                right_target_ty,
                right_trait_head_ty,
            );
            let mut type_map = FastHashMap::default();
            let mut const_map = FastHashMap::default();
            checker.unify_with_const_map(
                left_fresh_target,
                right_fresh_target,
                &mut type_map,
                &mut const_map,
            ) && checker.unify_with_const_map(
                left_fresh_trait,
                right_fresh_trait,
                &mut type_map,
                &mut const_map,
            )
        };

        if !overlaps {
            return None;
        }

        Some(OverlappingTraitImplPair {
            left_span: left_impl.span,
            right_span: right_impl.span,
            left_target_ty,
            left_trait_ty,
            right_target_ty,
            right_trait_ty,
        })
    }

    fn freshen_impl_head_types_for_overlap(
        checker: &mut ExprChecker<'_, '_>,
        impl_def: &ImplDef,
        target_ty: TypeId,
        trait_ty: TypeId,
    ) -> (TypeId, TypeId) {
        let mut subst_map = FastHashMap::default();

        for (index, param) in impl_def.generics.iter().enumerate() {
            let fresh_name = checker.ctx.intern(&format!(
                "__coherence_impl{}_{}_{}",
                impl_def.id.0,
                index,
                checker.ctx.resolve(param.name)
            ));
            let fresh_arg = match &param.kind {
                ast::GenericParamKind::Type => GenericArg::Type(checker.fresh_type_var()),
                ast::GenericParamKind::Const { ty } => {
                    let const_ty = checker
                        .ctx
                        .node_types
                        .get(&ty.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR);
                    GenericArg::Const(ConstGeneric::Param(fresh_name, const_ty))
                }
            };
            subst_map.insert(param.name, fresh_arg);
        }

        let mut subst = Substituter::new(&mut checker.ctx.type_registry, &subst_map);
        (subst.substitute(target_ty), subst.substitute(trait_ty))
    }

    fn trait_impl_is_orphan_legal(
        &mut self,
        impl_id: DefId,
        target_ty: TypeId,
        trait_ty: TypeId,
    ) -> bool {
        let Some(impl_home) = self.definition_locality(impl_id) else {
            return true;
        };

        let trait_norm = self.ctx.type_registry.normalize(trait_ty);
        let TypeKind::TraitObject(trait_def_id, _, _) =
            self.ctx.type_registry.get(trait_norm).clone()
        else {
            return false;
        };

        // Trait impls are always legal inside the trait's own package/root. Orphan checking only
        // constrains downstream impls of foreign traits, where the target must contribute a local
        // anchor to keep global proof search coherent.
        if self
            .definition_locality(trait_def_id)
            .is_none_or(|trait_home| trait_home == impl_home)
        {
            return true;
        }

        self.type_has_local_impl_anchor(target_ty, impl_home)
    }

    fn definition_locality(&self, def_id: DefId) -> Option<ImplLocality> {
        let owner_module = self.ctx.def_parent_module(def_id)?;
        Some(self.module_locality(owner_module))
    }

    fn module_locality(&self, module_id: DefId) -> ImplLocality {
        self.ctx.root_module_package_name(module_id).map_or_else(
            || ImplLocality::Root(self.ctx.module_root(module_id)),
            ImplLocality::Package,
        )
    }

    fn type_has_local_impl_anchor(&mut self, ty: TypeId, impl_home: ImplLocality) -> bool {
        let ty = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(ty).clone() {
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::Array { elem, .. }
            | TypeKind::ArrayInfer { elem, .. } => self.type_has_local_impl_anchor(elem, impl_home),
            TypeKind::Alias(_, target) => self.type_has_local_impl_anchor(target, impl_home),
            TypeKind::Def(def_id, _)
            | TypeKind::Enum(def_id, _)
            | TypeKind::Associated(def_id, _)
            | TypeKind::FnDef(def_id, _)
            | TypeKind::TraitObject(def_id, _, _) => {
                self.definition_is_local_anchor(def_id, impl_home)
            }
            TypeKind::AnonymousStruct(..)
            | TypeKind::AnonymousUnion(..)
            | TypeKind::AnonymousEnum(..)
            | TypeKind::ClosureInterface { .. }
            | TypeKind::AnonymousState { .. } => true,
            TypeKind::Primitive(_)
            | TypeKind::Simd { .. }
            | TypeKind::Function { .. }
            | TypeKind::Module(_)
            | TypeKind::Error
            | TypeKind::TypeVar(_)
            | TypeKind::Param(_)
            | TypeKind::Projection { .. }
            | TypeKind::EnumPayload(..)
            | TypeKind::AnonymousEnumPayload(..) => false,
        }
    }

    fn definition_is_local_anchor(&self, def_id: DefId, impl_home: ImplLocality) -> bool {
        self.definition_locality(def_id) == Some(impl_home)
    }
}

struct OverlappingTraitImplPair {
    left_span: Span,
    right_span: Span,
    left_target_ty: TypeId,
    left_trait_ty: TypeId,
    right_target_ty: TypeId,
    right_trait_ty: TypeId,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ImplLocality {
    Package(SymbolId),
    Root(DefId),
}
