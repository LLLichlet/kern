use super::*;

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    /// Run the full type-resolution pass in two stages.
    pub fn resolve_all(&mut self) {
        let module_ids = self.collect_module_ids();
        self.resolve_module_pass(&module_ids, true);
        self.resolve_module_pass(&module_ids, false);
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
                .intern(TypeKind::TraitObject(
                    item_id,
                    self_args,
                    Vec::new(),
                ));
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
            self.bind_impl_assoc_types(&i.assoc_types, resolved_trait_ty, impl_scope, i.span);
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

        let (trait_generics_len, trait_assoc_ids) = match self.ctx.defs.get(trait_def_id.0 as usize)
        {
            Some(Def::Trait(trait_def)) => {
                (trait_def.generics.len(), trait_def.assoc_types.clone())
            }
            _ => (0, Vec::new()),
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

        let generic_args = trait_args
            .iter()
            .take(trait_generics_len)
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
}
