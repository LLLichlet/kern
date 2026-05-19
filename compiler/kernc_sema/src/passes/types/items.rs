//! Top-level item type-resolution driver.
//!
//! This file coordinates the module pass over collected definitions, ensuring
//! each item kind resolves its signature, fields, trait data, impl head, and
//! nested expressions in the correct scope.

use super::*;
use crate::scope::SymbolNamespace;

use kernc_utils::FastHashMap;

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    /// Run the full type-resolution pass in two stages.
    pub fn resolve_all(&mut self) {
        self.resolve_all_cancelable(&CancellationToken::new())
            .expect("fresh cancellation token cannot be canceled");
    }

    pub fn resolve_all_cancelable(
        &mut self,
        cancellation: &CancellationToken,
    ) -> Result<(), Canceled> {
        cancellation.check()?;
        let module_ids = self.collect_module_ids();
        self.measure_phase(
            |timings, duration| timings.resolve_alias_items += duration,
            |this| this.resolve_module_pass_cancelable(&module_ids, true, cancellation),
        )?;
        cancellation.check()?;
        // Aliases are resolved first so later item signatures can normalize through
        // them without depending on source-order details inside a module.
        self.measure_phase(
            |timings, duration| timings.resolve_non_alias_items += duration,
            |this| -> Result<(), Canceled> {
                this.resolve_module_pass_cancelable(&module_ids, false, cancellation)?;
                cancellation.check()?;
                this.rebuild_trait_impl_index_by_trait_cancelable(cancellation)
            },
        )?;
        cancellation.check()?;
        self.measure_phase(
            |timings, duration| timings.validate_supertrait_graph += duration,
            |this| {
                this.validate_supertrait_graph();
                Ok(())
            },
        )?;
        cancellation.check()?;
        self.measure_phase(
            |timings, duration| timings.validate_trait_impl_coherence += duration,
            |this| {
                this.validate_trait_impl_coherence();
                Ok(())
            },
        )?;
        cancellation.check()?;
        self.measure_phase(
            |timings, duration| timings.validate_trait_impl_method_contracts += duration,
            |this| {
                this.validate_trait_impl_method_contracts();
                Ok(())
            },
        )?;
        cancellation.check()?;
        self.measure_phase(
            |timings, duration| timings.validate_impl_associated_type_targets += duration,
            |this| {
                this.validate_impl_associated_type_targets();
                Ok(())
            },
        )?;
        Ok(())
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

    fn rebuild_trait_impl_index_by_trait_cancelable(
        &mut self,
        cancellation: &CancellationToken,
    ) -> Result<(), Canceled> {
        let mut grouped = FastHashMap::default();

        for entry in self.ctx.trait_impl_entries() {
            cancellation.check()?;
            let impl_id = entry.id;
            let impl_def = entry.def;
            self.ensure_impl_signature_types_resolved(impl_id);
            let Some(trait_ty) = impl_def
                .trait_type
                .as_ref()
                .and_then(|trait_ty| self.ctx.node_type(trait_ty.id))
            else {
                continue;
            };
            let trait_ty = self.ctx.type_registry.normalize(trait_ty);
            let TypeKind::TraitObject(trait_def_id, _, _) =
                self.ctx.type_registry.get(trait_ty).clone()
            else {
                continue;
            };
            let Some(trait_key) = self.ctx.trait_def_lookup_key(trait_def_id) else {
                continue;
            };
            grouped
                .entry(trait_key)
                .or_insert_with(Vec::new)
                .push(impl_id);
        }

        self.ctx.set_trait_impl_groups(grouped);
        Ok(())
    }

    fn resolve_module_pass_cancelable(
        &mut self,
        module_ids: &[DefId],
        aliases_only: bool,
        cancellation: &CancellationToken,
    ) -> Result<(), Canceled> {
        for &mod_id in module_ids {
            cancellation.check()?;
            let Some((mod_scope, items)) = self.module_scope_and_items(mod_id) else {
                continue;
            };

            for item_id in items {
                cancellation.check()?;
                let is_alias = matches!(self.ctx.defs[item_id.0 as usize], Def::TypeAlias(_));
                if aliases_only == is_alias {
                    self.resolve_item(item_id, mod_scope);
                }
            }
        }
        Ok(())
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

        // SAFETY: type resolution mutates inference state and selected `resolved_*` fields, but
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
        if let Some(info) = &f.default_trait_method {
            let self_ty = self
                .ctx
                .type_registry
                .intern(TypeKind::Param(info.self_param));
            self.bind_self_type(self_ty, func_scope, f.span);
        } else if let Some(parent_id) = f.parent {
            match self.ctx.defs[parent_id.0 as usize].clone() {
                Def::Impl(i) => {
                    let target_ty = self.ctx.node_type_or_error(i.target_type.id);
                    self.bind_self_type(target_ty, func_scope, f.span);
                }
                Def::Trait(t) => {
                    let self_args = t
                        .generics
                        .iter()
                        .map(|param| self.generic_param_placeholder_arg(param, func_scope))
                        .collect::<Vec<_>>();
                    let self_ty = self.ctx.type_registry.intern(TypeKind::TraitObject(
                        parent_id,
                        self_args,
                        Vec::new(),
                    ));
                    self.bind_self_type(self_ty, func_scope, f.span);
                }
                _ => {}
            }
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
            self.ctx
                .scopes
                .update_type_in_namespace(f.name, SymbolNamespace::Value, fn_def_ty);
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
        self.ctx
            .scopes
            .update_type_in_namespace(s.name, SymbolNamespace::Type, struct_ty);
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
        self.ctx
            .scopes
            .update_type_in_namespace(u.name, SymbolNamespace::Type, union_ty);
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
        let mut default_impls = Vec::new();
        for method in &t.methods {
            let sig_ty = self.resolve_type(&method.signature.type_node, trait_scope);
            resolved_methods.push((method.signature.name, sig_ty));
            if let Some(default_impl) = method.default_impl {
                default_impls.push(default_impl);
            }
        }
        for default_impl in default_impls {
            self.resolve_item(default_impl, trait_scope);
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
        self.ctx.set_node_type(t.target.id, target_ty);

        self.ctx.scopes.exit_scope();
        self.ctx.scopes.set_current_scope(parent_scope);
        self.ctx
            .scopes
            .update_type_in_namespace(t.name, SymbolNamespace::Type, target_ty);
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
            self.reject_explicit_compiler_owned_marker_impl(i, resolved, trait_ty.span);
            resolved_trait_ty = Some(resolved);
        }

        let canonical_trait_ty =
            self.bind_impl_assoc_types(i, &i.assoc_types, resolved_trait_ty, impl_scope, i.span);
        if let (Some(trait_ty), Some(canonical_trait_ty)) = (&i.trait_type, canonical_trait_ty) {
            self.ctx.set_node_type(trait_ty.id, canonical_trait_ty);
        }
        if let Some(resolved_trait_ty) = canonical_trait_ty {
            self.validate_trait_impl_orphan(i, target_ty_id, resolved_trait_ty);
            self.validate_trait_impl_supertrait_contracts_for_impl(
                i,
                target_ty_id,
                resolved_trait_ty,
            );
        }

        if let Def::Impl(updated_i) = &mut self.ctx.defs[i.id.0 as usize] {
            updated_i.resolved_trait_ty = canonical_trait_ty;
        }

        for &method_id in &i.methods {
            self.resolve_item(method_id, impl_scope);
        }

        self.ctx.scopes.exit_scope();
    }

    fn reject_explicit_compiler_owned_marker_impl(
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
        let is_numeric_marker = matches!(
            trait_name.as_str(),
            "Integer" | "SignedInteger" | "UnsignedInteger" | "Float",
        );
        let is_slice_bounds_marker = trait_name == "SliceBounds";
        if !is_numeric_marker && !is_slice_bounds_marker {
            return;
        }

        let marker_kind = if is_numeric_marker {
            "numeric marker"
        } else {
            "slice-bounds marker"
        };
        let assignment_hint = if is_numeric_marker {
            format!(
                "`{}` is assigned by the compiler for builtin numeric types only",
                trait_name
            )
        } else {
            "`SliceBounds` is assigned by the compiler for builtin slice-bound range types only"
                .to_string()
        };
        let abstraction_hint = if is_numeric_marker {
            "define a normal trait if you need a user-extensible numeric-like abstraction"
        } else {
            "define a normal trait if you need a user-extensible slicing abstraction"
        };
        self.ctx
            .struct_error(
                span,
                format!(
                    "builtin {} trait `{}` cannot be implemented explicitly",
                    marker_kind, trait_name
                ),
            )
            .with_hint(assignment_hint)
            .with_hint(abstraction_hint)
            .with_span_label(impl_def.span, "while checking this impl")
            .emit();
    }

    fn resolve_enum_item(&mut self, item_id: DefId, a: &EnumDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let adt_scope = self.ctx.scopes.enter_scope();

        self.bind_generics(&a.generics, adt_scope);
        self.resolve_where_clauses(&a.where_clauses, adt_scope);

        if a.is_extern && a.backing_type.is_none() {
            self.ctx
                .struct_error(
                    a.span,
                    "extern enum declarations must specify an integer backing type",
                )
                .with_hint("write `extern enum Name: u32 { ... }`")
                .emit();
        }

        if let Some(backing_ty) = &a.backing_type {
            let resolved_ty = self.resolve_type(backing_ty, adt_scope);
            if !self.ctx.type_registry.is_integer(resolved_ty) && resolved_ty != TypeId::ERROR {
                self.ctx
                    .emit_error(backing_ty.span, "Enum backing type must be an integer");
            }
        }

        for variant in &a.variants {
            if let Some(payload_ty) = &variant.payload_type {
                if a.is_extern {
                    self.ctx
                        .struct_error(
                            payload_ty.span,
                            "extern enum variants cannot carry payloads",
                        )
                        .with_hint(
                            "use a normal `enum` for tagged unions, or an extern struct/union for C ABI payloads",
                        )
                        .emit();
                }
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
        self.ctx
            .scopes
            .update_type_in_namespace(a.name, SymbolNamespace::Type, adt_ty);
    }
}
