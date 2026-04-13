use crate::SemaContext;
use crate::checker::{ConstEvaluator, ExprChecker, Substituter};
use crate::def::*;
use crate::scope::{ScopeId, SymbolInfo, SymbolKind};
use crate::ty::{
    AnonymousEnum, AnonymousField, AnonymousVariant, BuiltinAnonymousEnumKind, TypeId, TypeKind,
};
use kernc_ast as ast;
use kernc_utils::{Span, SymbolId};
use std::collections::HashMap;

pub struct TypeResolver<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
}

struct FunctionResolveSpec {
    name: SymbolId,
    generics: Vec<ast::GenericParam>,
    where_clauses: Vec<ast::WhereClause>,
    params: Vec<ast::FuncParam>,
    ret_type: ast::TypeNode,
    parent: Option<DefId>,
    is_variadic: bool,
    span: Span,
}

struct AggregateResolveSpec {
    name: SymbolId,
    generics: Vec<ast::GenericParam>,
    where_clauses: Vec<ast::WhereClause>,
    fields: Vec<ast::StructFieldDef>,
}

struct TraitResolveSpec {
    generics: Vec<ast::GenericParam>,
    where_clauses: Vec<ast::WhereClause>,
    supertraits: Vec<ast::TypeNode>,
    assoc_types: Vec<DefId>,
    methods: Vec<ast::StructFieldDef>,
    span: Span,
}

struct TypeAliasResolveSpec {
    name: SymbolId,
    generics: Vec<ast::GenericParam>,
    where_clauses: Vec<ast::WhereClause>,
    target: ast::TypeNode,
}

struct ImplResolveSpec {
    generics: Vec<ast::GenericParam>,
    where_clauses: Vec<ast::WhereClause>,
    target_type: ast::TypeNode,
    trait_type: Option<ast::TypeNode>,
    assoc_types: Vec<DefId>,
    methods: Vec<DefId>,
    span: Span,
}

struct EnumResolveSpec {
    name: SymbolId,
    generics: Vec<ast::GenericParam>,
    where_clauses: Vec<ast::WhereClause>,
    backing_type: Option<Box<ast::TypeNode>>,
    variants: Vec<ast::EnumVariant>,
}

enum ResolveItemSpec {
    Function(FunctionResolveSpec),
    Struct(AggregateResolveSpec),
    Union(AggregateResolveSpec),
    Trait(TraitResolveSpec),
    TypeAlias(TypeAliasResolveSpec),
    Impl(ImplResolveSpec),
    Enum(EnumResolveSpec),
}

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self { ctx }
    }

    pub fn context(&mut self) -> &mut SemaContext<'ctx> {
        self.ctx
    }

    pub fn into_context(self) -> &'a mut SemaContext<'ctx> {
        self.ctx
    }

    pub fn current_scope_id(&self) -> Option<ScopeId> {
        self.ctx.scopes.current_scope_id()
    }

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
        let spec = match &self.ctx.defs[item_id.0 as usize] {
            Def::Function(f) => ResolveItemSpec::Function(FunctionResolveSpec {
                name: f.name,
                generics: f.generics.clone(),
                where_clauses: f.where_clauses.clone(),
                params: f.params.clone(),
                ret_type: f.ret_type.clone(),
                parent: f.parent,
                is_variadic: f.is_variadic,
                span: f.span,
            }),
            Def::Struct(s) => ResolveItemSpec::Struct(AggregateResolveSpec {
                name: s.name,
                generics: s.generics.clone(),
                where_clauses: s.where_clauses.clone(),
                fields: s.fields.clone(),
            }),
            Def::Union(u) => ResolveItemSpec::Union(AggregateResolveSpec {
                name: u.name,
                generics: u.generics.clone(),
                where_clauses: u.where_clauses.clone(),
                fields: u.fields.clone(),
            }),
            Def::Trait(t) => ResolveItemSpec::Trait(TraitResolveSpec {
                generics: t.generics.clone(),
                where_clauses: t.where_clauses.clone(),
                supertraits: t.supertraits.clone(),
                assoc_types: t.assoc_types.clone(),
                methods: t.methods.clone(),
                span: t.span,
            }),
            Def::TypeAlias(t) => ResolveItemSpec::TypeAlias(TypeAliasResolveSpec {
                name: t.name,
                generics: t.generics.clone(),
                where_clauses: t.where_clauses.clone(),
                target: t.target.clone(),
            }),
            Def::Impl(i) => ResolveItemSpec::Impl(ImplResolveSpec {
                generics: i.generics.clone(),
                where_clauses: i.where_clauses.clone(),
                target_type: i.target_type.clone(),
                trait_type: i.trait_type.clone(),
                assoc_types: i.assoc_types.clone(),
                methods: i.methods.clone(),
                span: i.span,
            }),
            Def::Enum(a) => ResolveItemSpec::Enum(EnumResolveSpec {
                name: a.name,
                generics: a.generics.clone(),
                where_clauses: a.where_clauses.clone(),
                backing_type: a.backing_type.clone(),
                variants: a.variants.clone(),
            }),
            Def::AssociatedType(_) | Def::Global(_) | Def::Module(_) => return,
        };

        match spec {
            ResolveItemSpec::Function(f) => self.resolve_function_item(item_id, &f, parent_scope),
            ResolveItemSpec::Struct(s) => self.resolve_struct_item(item_id, &s, parent_scope),
            ResolveItemSpec::Union(u) => self.resolve_union_item(item_id, &u, parent_scope),
            ResolveItemSpec::Trait(t) => self.resolve_trait_item(item_id, &t, parent_scope),
            ResolveItemSpec::TypeAlias(t) => self.resolve_type_alias_item(&t, parent_scope),
            ResolveItemSpec::Impl(i) => self.resolve_impl_item(&i, parent_scope),
            ResolveItemSpec::Enum(a) => self.resolve_enum_item(item_id, &a, parent_scope),
        }
    }

    fn resolve_function_item(
        &mut self,
        item_id: DefId,
        f: &FunctionResolveSpec,
        parent_scope: ScopeId,
    ) {
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
            .map(|param| self.ctx.type_registry.intern(TypeKind::Param(param.name)))
            .collect();
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

    fn resolve_struct_item(
        &mut self,
        item_id: DefId,
        s: &AggregateResolveSpec,
        parent_scope: ScopeId,
    ) {
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

    fn resolve_union_item(
        &mut self,
        item_id: DefId,
        u: &AggregateResolveSpec,
        parent_scope: ScopeId,
    ) {
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

    fn resolve_trait_item(&mut self, item_id: DefId, t: &TraitResolveSpec, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let trait_scope = self.ctx.scopes.enter_scope();

        let self_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::TraitObject(item_id, vec![]));
        self.bind_self_type(self_ty, trait_scope, t.span);

        self.bind_generics(&t.generics, trait_scope);
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

    fn resolve_type_alias_item(&mut self, t: &TypeAliasResolveSpec, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let alias_scope = self.ctx.scopes.enter_scope();

        self.bind_generics(&t.generics, alias_scope);
        self.resolve_where_clauses(&t.where_clauses, alias_scope);
        let target_ty = self.resolve_type(&t.target, alias_scope);

        self.ctx.scopes.exit_scope();
        self.ctx.scopes.set_current_scope(parent_scope);
        self.ctx.scopes.update_type(t.name, target_ty);
    }

    fn resolve_impl_item(&mut self, i: &ImplResolveSpec, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let impl_scope = self.ctx.scopes.enter_scope();

        self.bind_generics(&i.generics, impl_scope);
        self.resolve_where_clauses(&i.where_clauses, impl_scope);

        let target_ty_id = self.resolve_type(&i.target_type, impl_scope);
        self.bind_self_type(target_ty_id, impl_scope, i.span);

        let mut resolved_trait_ty = None;
        if let Some(trait_ty) = &i.trait_type {
            resolved_trait_ty = Some(self.resolve_type(trait_ty, impl_scope));
        }

        self.bind_impl_assoc_types(&i.assoc_types, resolved_trait_ty, impl_scope, i.span);

        for &method_id in &i.methods {
            self.resolve_item(method_id, impl_scope);
        }

        self.ctx.scopes.exit_scope();
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
                is_pub: false,
                is_mut: false,
            };
            self.ctx.scopes.set_current_scope(scope);
            let _ = self.ctx.scopes.define(assoc_def.name, info);
        }
    }

    fn resolve_assoc_type_bounds(&mut self, assoc_type_ids: &[DefId], parent_scope: ScopeId) {
        for &assoc_id in assoc_type_ids {
            let Some(Def::AssociatedType(assoc_def)) = self.ctx.defs.get(assoc_id.0 as usize).cloned() else {
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
    ) {
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
                is_pub: false,
                is_mut: false,
            };
            self.ctx.scopes.set_current_scope(scope);
            let _ = self.ctx.scopes.define(assoc_def.name, info);
        }

        let Some(trait_ty) = resolved_trait_ty else {
            for &assoc_id in assoc_type_ids {
                if let Some(Def::AssociatedType(assoc_def)) =
                    self.ctx.defs.get(assoc_id.0 as usize)
                {
                    self.ctx
                        .struct_error(assoc_def.span, "associated type definitions require a trait impl")
                        .with_hint("write `impl Type: Trait { ... }` when defining associated types")
                        .emit();
                }
            }
            return;
        };

        let trait_norm = self.ctx.type_registry.normalize(trait_ty);
        let TypeKind::TraitObject(trait_def_id, _) = self.ctx.type_registry.get(trait_norm).clone() else {
            self.ctx.emit_error(span, "impl trait target is not a trait");
            return;
        };

        let trait_assoc_ids = match self.ctx.defs.get(trait_def_id.0 as usize) {
            Some(Def::Trait(trait_def)) => trait_def.assoc_types.clone(),
            _ => Vec::new(),
        };

        let mut trait_assoc_names = HashMap::new();
        for trait_assoc_id in trait_assoc_ids {
            let Some(Def::AssociatedType(trait_assoc)) =
                self.ctx.defs.get(trait_assoc_id.0 as usize).cloned()
            else {
                continue;
            };
            trait_assoc_names.insert(trait_assoc.name, trait_assoc_id);

            let Some(&impl_assoc_id) = impl_assoc_by_name.get(&trait_assoc.name) else {
                self.ctx
                    .struct_error(span, format!("missing associated type definition `{}` in impl", self.ctx.resolve(trait_assoc.name)))
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

        for &assoc_id in assoc_type_ids {
            let Some(Def::AssociatedType(assoc_def)) = self.ctx.defs.get(assoc_id.0 as usize).cloned() else {
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
            }
            if let Def::AssociatedType(updated) = &mut self.ctx.defs[assoc_id.0 as usize] {
                updated.resolved_bounds = resolved_bounds;
            }
        }
    }

    fn resolve_enum_item(&mut self, item_id: DefId, a: &EnumResolveSpec, parent_scope: ScopeId) {
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
                self.resolve_type(payload_ty, adt_scope);
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

    // ==========================================
    //          Core type conversion logic
    // ==========================================

    /// Convert an AST `TypeNode` into a semantic `TypeId`.
    pub fn resolve_type(&mut self, ty_node: &ast::TypeNode, env_scope: ScopeId) -> TypeId {
        // Prefer types already inferred by the expression checker, especially for `@typeOf`.
        if let Some(&cached_ty) = self.ctx.node_types.get(&ty_node.id)
            && cached_ty != TypeId::ERROR
        {
            return cached_ty;
        }

        let ty_id = match &ty_node.kind {
            ast::TypeKind::Path {
                segments, generics, ..
            } => self.resolve_path_type(segments, generics, env_scope, ty_node.span),
            ast::TypeKind::Void => TypeId::VOID,
            ast::TypeKind::Optional { inner } => {
                let inner_ty = self.resolve_type(inner, env_scope);
                self.make_builtin_optional_type(inner_ty, ty_node.span)
            }
            ast::TypeKind::Result { ok, err } => {
                let ok_ty = self.resolve_type(ok, env_scope);
                let err_ty = self.resolve_type(err, env_scope);
                self.make_builtin_result_type(ok_ty, err_ty)
            }

            // Inline anonymous struct.
            ast::TypeKind::Struct { is_extern, fields } => {
                let mut anon_fields =
                    self.resolve_anonymous_fields(fields, env_scope, ty_node.span, "struct", true);

                if !*is_extern {
                    anon_fields.sort_by_key(|f| f.name);
                }

                self.check_duplicate_anon_fields(&anon_fields, ty_node.span, "anonymous struct");
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousStruct(*is_extern, anon_fields))
            }

            ast::TypeKind::Union { is_extern, fields } => {
                let mut anon_fields =
                    self.resolve_anonymous_fields(fields, env_scope, ty_node.span, "union", false);
                anon_fields.sort_by_key(|f| f.name);
                self.check_duplicate_anon_fields(&anon_fields, ty_node.span, "anonymous union");
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousUnion(*is_extern, anon_fields))
            }

            ast::TypeKind::Enum {
                backing_type,
                variants,
            } => {
                let backing_ty = backing_type.as_ref().map(|bt| {
                    let resolved_ty = self.resolve_type(bt, env_scope);
                    if !self.ctx.type_registry.is_integer(resolved_ty)
                        && resolved_ty != TypeId::ERROR
                    {
                        self.ctx
                            .emit_error(bt.span, "anonymous enum backing type must be an integer");
                    }
                    resolved_ty
                });

                let mut anon_variants = Vec::new();
                for variant in variants {
                    let payload_ty = variant.payload_type.as_ref().map(|payload_ty| {
                        let resolved_ty = self.resolve_type(payload_ty, env_scope);
                        self.ensure_sized(resolved_ty, payload_ty.span);
                        resolved_ty
                    });

                    let explicit_value = variant.value.as_ref().map(|value_expr| {
                        self.resolve_expr(value_expr, env_scope);
                        self.ctx.scopes.set_current_scope(env_scope);
                        let mut evaluator = ConstEvaluator::new(self.ctx);
                        evaluator.eval_math(value_expr).unwrap_or(0)
                    });

                    anon_variants.push(AnonymousVariant {
                        name: variant.name,
                        name_span: variant.name_span,
                        payload_ty,
                        explicit_value,
                    });
                }

                self.check_duplicate_anon_variants(&anon_variants, ty_node.span);

                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousEnum(AnonymousEnum {
                        backing_ty,
                        builtin: None,
                        variants: anon_variants,
                    }))
            }

            ast::TypeKind::Pointer { is_mut, elem } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut: *is_mut,
                    elem: base,
                })
            }
            ast::TypeKind::VolatilePtr { is_mut, elem } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                    is_mut: *is_mut,
                    elem: base,
                })
            }
            ast::TypeKind::Slice { is_mut, elem } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.type_registry.intern(TypeKind::Slice {
                    is_mut: *is_mut,
                    elem: base,
                })
            }
            ast::TypeKind::Array { is_mut, elem, len } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.scopes.set_current_scope(env_scope);
                let mut evaluator = ConstEvaluator::new(self.ctx);
                let Ok(length) = evaluator.eval_usize(len) else {
                    return TypeId::ERROR;
                };
                if length > u32::MAX as u64 {
                    self.ctx
                        .struct_error(
                            len.span,
                            format!(
                                "array length {} exceeds the current compiler limit of {} elements",
                                length,
                                u32::MAX
                            ),
                        )
                        .with_hint(
                            "LLVM array types are emitted with a 32-bit element count; split the object or allocate dynamically instead",
                        )
                        .emit();
                    return TypeId::ERROR;
                }
                self.ctx.type_registry.intern(TypeKind::Array {
                    is_mut: *is_mut,
                    elem: base,
                    len: length,
                })
            }
            ast::TypeKind::ArrayInfer { is_mut, elem } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.type_registry.intern(TypeKind::ArrayInfer {
                    is_mut: *is_mut,
                    elem: base,
                })
            }
            ast::TypeKind::Function {
                params,
                ret,
                is_variadic,
            } => {
                let mut param_tys = Vec::with_capacity(params.len());
                for p in params {
                    param_tys.push(self.resolve_type(p, env_scope));
                }
                let ret_ty = match ret {
                    Some(r) => self.resolve_type(r, env_scope),
                    None => TypeId::VOID,
                };
                self.ctx.type_registry.intern(TypeKind::Function {
                    params: param_tys,
                    ret: ret_ty,
                    is_variadic: *is_variadic,
                })
            }
            ast::TypeKind::SelfType => {
                self.ctx.scopes.set_current_scope(env_scope);
                let self_sym = self.ctx.intern("Self");
                if let Some(info) = self.ctx.scopes.resolve(self_sym) {
                    info.type_id
                } else {
                    self.ctx.struct_error(ty_node.span, "the `Self` type is only valid inside `impl` blocks or `trait` definitions")
                        .with_hint("you are using it in a global or standard function context")
                        .emit();
                    TypeId::ERROR
                }
            }
            ast::TypeKind::Never => TypeId::NEVER,
            ast::TypeKind::Infer => {
                self.ctx.struct_error(ty_node.span, "type inference `_` is not allowed as a standalone type")
                    .with_hint("in Kern, the `_` placeholder is exclusively used for array length inference, e.g., `[_]u8.{ 1, 2, 3 }`")
                    .emit();
                TypeId::ERROR
            }
            ast::TypeKind::ClosureInterface { params, ret } => {
                let mut param_tys = Vec::with_capacity(params.len());
                for p in params {
                    param_tys.push(self.resolve_type(p, env_scope));
                }
                let ret_ty = match ret {
                    Some(r) => self.resolve_type(r, env_scope),
                    None => TypeId::VOID,
                };
                self.ctx.type_registry.intern(TypeKind::ClosureInterface {
                    params: param_tys,
                    ret: ret_ty,
                })
            }

            ast::TypeKind::TypeOf(expr) => {
                // Placeholder until anonymous unions are fully modeled here.
                self.resolve_expr(expr, env_scope);
                TypeId::ERROR
            }
            // Named nominal types are collected earlier and should not appear as anonymous shapes here.
            _ => {
                self.ctx
                    .emit_error(ty_node.span, "Invalid or unsupported type construction");
                TypeId::ERROR
            }
        };

        self.ctx.node_types.insert(ty_node.id, ty_id);
        ty_id
    }

    fn resolve_anonymous_fields(
        &mut self,
        fields: &[ast::StructFieldDef],
        env_scope: ScopeId,
        _span: Span,
        kind_name: &str,
        _allow_default_values: bool,
    ) -> Vec<AnonymousField> {
        let mut anon_fields = Vec::with_capacity(fields.len());

        for f in fields {
            let f_ty = self.resolve_type(&f.type_node, env_scope);
            self.ensure_sized(f_ty, f.type_node.span);

            if f.is_pub {
                let msg = format!("anonymous {} fields cannot be declared pub", kind_name);
                self.ctx
                    .struct_error(f.span, msg)
                    .with_hint(
                        "field-level `pub` is only supported on named declarations like `type Name = struct { ... }`",
                    )
                    .emit();
            }

            if f.default_value.is_some() {
                let msg = format!("anonymous {}s cannot have default field values", kind_name);
                self.ctx
                    .struct_error(f.span, msg)
                    .with_hint("default values are only allowed in named struct declarations (`type Name = struct { ... }`)")
                    .emit();
            }

            anon_fields.push(AnonymousField {
                name: f.name,
                ty: f_ty,
            });
        }

        anon_fields
    }

    fn make_builtin_optional_type(&mut self, inner_ty: TypeId, span: Span) -> TypeId {
        let inner_norm = self.ctx.type_registry.normalize(inner_ty);
        if matches!(
            self.ctx.type_registry.get(inner_norm),
            TypeKind::VolatilePtr { .. }
        ) {
            self.ctx
                .struct_error(
                    span,
                    "`?^T` is not a valid type; `^T` already covers raw address `0`",
                )
                .with_hint("use `^T` for raw addresses or `?*T` for nullable object pointers")
                .emit();
            return TypeId::ERROR;
        }

        let some = self.ctx.intern("Some");
        let none = self.ctx.intern("None");
        self.ctx
            .type_registry
            .intern(TypeKind::AnonymousEnum(AnonymousEnum {
                backing_ty: None,
                builtin: Some(BuiltinAnonymousEnumKind::Optional),
                variants: vec![
                    AnonymousVariant {
                        name: some,
                        name_span: Span::default(),
                        payload_ty: Some(inner_ty),
                        explicit_value: None,
                    },
                    AnonymousVariant {
                        name: none,
                        name_span: Span::default(),
                        payload_ty: None,
                        explicit_value: None,
                    },
                ],
            }))
    }

    fn make_builtin_result_type(&mut self, ok_ty: TypeId, err_ty: TypeId) -> TypeId {
        let ok = self.ctx.intern("Ok");
        let err = self.ctx.intern("Err");
        self.ctx
            .type_registry
            .intern(TypeKind::AnonymousEnum(AnonymousEnum {
                backing_ty: None,
                builtin: Some(BuiltinAnonymousEnumKind::Result),
                variants: vec![
                    AnonymousVariant {
                        name: ok,
                        name_span: Span::default(),
                        payload_ty: Some(ok_ty),
                        explicit_value: None,
                    },
                    AnonymousVariant {
                        name: err,
                        name_span: Span::default(),
                        payload_ty: Some(err_ty),
                        explicit_value: None,
                    },
                ],
            }))
    }

    fn check_duplicate_anon_fields(
        &mut self,
        fields: &[AnonymousField],
        span: Span,
        kind_name: &str,
    ) {
        for i in 1..fields.len() {
            if fields[i - 1].name == fields[i].name {
                let name_str = self.ctx.resolve(fields[i].name).to_string();
                self.ctx
                    .struct_error(
                        span,
                        format!("duplicate field `{}` in {}", name_str, kind_name),
                    )
                    .emit();
            }
        }
    }

    fn check_duplicate_anon_variants(&mut self, variants: &[AnonymousVariant], span: Span) {
        let mut sorted = variants.to_vec();
        sorted.sort_by_key(|variant| variant.name);
        for i in 1..sorted.len() {
            if sorted[i - 1].name == sorted[i].name {
                let name_str = self.ctx.resolve(sorted[i].name).to_string();
                self.ctx
                    .struct_error(
                        span,
                        format!("duplicate variant `{}` in anonymous enum", name_str),
                    )
                    .emit();
            }
        }
    }

    // Recursively resolve every nested `TypeNode` inside an expression tree.
    fn resolve_pattern(&mut self, pattern: &ast::Pattern, scope: ScopeId) {
        match &pattern.kind {
            ast::PatternKind::Binding(_)
            | ast::PatternKind::Ignore
            | ast::PatternKind::Variant(_) => {
                if let ast::PatternKind::Variant(variant) = &pattern.kind
                    && let Some(ty) = &variant.target_type
                {
                    self.resolve_type(ty, scope);
                }
            }
            ast::PatternKind::Destructure(destructure) => {
                if let Some(ty) = &destructure.target_type {
                    self.resolve_type(ty, scope);
                }
                for field in &destructure.fields {
                    self.resolve_pattern(&field.pattern, scope);
                }
            }
        }
    }

    fn resolve_expr(&mut self, expr: &ast::Expr, scope: ScopeId) {
        match &expr.kind {
            ast::ExprKind::Let {
                pattern,
                init,
                else_pattern,
                else_branch,
            } => {
                self.resolve_pattern(&pattern.pattern, scope);
                if let Some(else_pattern) = else_pattern {
                    self.resolve_pattern(else_pattern, scope);
                }
                self.resolve_expr(init, scope);
                if let Some(else_branch) = else_branch {
                    self.resolve_expr(else_branch, scope);
                }
            }
            ast::ExprKind::Static { init, .. } => {
                self.resolve_expr(init, scope);
            }
            ast::ExprKind::As { lhs, target } => {
                self.resolve_expr(lhs, scope);
                self.resolve_type(target, scope); // Resolve captured type nodes.
            }
            ast::ExprKind::TypeNode(type_node) => {
                self.resolve_type(type_node, scope);
            }
            ast::ExprKind::Block { stmts, result } => {
                for stmt in stmts {
                    match &stmt.kind {
                        ast::StmtKind::ExprStmt(e) | ast::StmtKind::ExprValue(e) => {
                            self.resolve_expr(e, scope);
                        }
                    }
                }
                if let Some(r) = result {
                    self.resolve_expr(r, scope);
                }
            }
            ast::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.resolve_expr(cond, scope);
                self.resolve_expr(then_branch, scope);
                if let Some(e) = else_branch {
                    self.resolve_expr(e, scope);
                }
            }
            ast::ExprKind::Match { target, arms } => {
                self.resolve_expr(target, scope);
                for arm in arms {
                    for pat in &arm.patterns {
                        match &pat.kind {
                            ast::MatchPatternKind::Value(e) => self.resolve_expr(e, scope),
                            ast::MatchPatternKind::Range { start, end, .. } => {
                                self.resolve_expr(start, scope);
                                self.resolve_expr(end, scope);
                            }
                            ast::MatchPatternKind::Pattern(pattern) => {
                                self.resolve_pattern(pattern, scope);
                            }
                        }
                    }
                    self.resolve_expr(&arm.body, scope);
                }
            }
            ast::ExprKind::For {
                init,
                cond,
                post,
                body,
            } => {
                if let Some(e) = init {
                    self.resolve_expr(e, scope);
                }
                if let Some(e) = cond {
                    self.resolve_expr(e, scope);
                }
                if let Some(e) = post {
                    self.resolve_expr(e, scope);
                }
                self.resolve_expr(body, scope);
            }
            ast::ExprKind::Closure {
                captures,
                params,
                ret_type,
                body,
            } => {
                for cap in captures {
                    self.resolve_expr(&cap.value, scope);
                }
                for param in params {
                    self.resolve_type(&param.type_node, scope);
                }
                self.resolve_type(ret_type, scope);
                self.resolve_expr(body, scope);
            }
            ast::ExprKind::Binary { lhs, rhs, .. } | ast::ExprKind::Assign { lhs, rhs, .. } => {
                self.resolve_expr(lhs, scope);
                self.resolve_expr(rhs, scope);
            }
            ast::ExprKind::Unary { operand, .. } => {
                self.resolve_expr(operand, scope);
            }
            ast::ExprKind::FieldAccess { lhs, .. } => {
                self.resolve_expr(lhs, scope);
            }
            ast::ExprKind::Propagate { operand, .. } => {
                self.resolve_expr(operand, scope);
            }
            ast::ExprKind::IndexAccess { lhs, index, .. } => {
                self.resolve_expr(lhs, scope);
                self.resolve_expr(index, scope);
            }
            ast::ExprKind::Call { callee, args } => {
                self.resolve_expr(callee, scope);
                for arg in args {
                    self.resolve_expr(arg, scope);
                }
            }
            ast::ExprKind::GenericInstantiation { target, types } => {
                self.resolve_expr(target, scope);
                // Resolve generic arguments.
                for ty in types {
                    self.resolve_type(ty, scope);
                }
            }
            ast::ExprKind::DataInit { type_node, literal } => {
                // Resolve the elided-initialization prefix type.
                if let Some(ty) = type_node {
                    self.resolve_type(ty, scope);
                }
                match literal {
                    ast::DataLiteralKind::Struct(fields) => {
                        for f in fields {
                            self.resolve_expr(&f.value, scope);
                        }
                    }
                    ast::DataLiteralKind::Array(elems) => {
                        for e in elems {
                            self.resolve_expr(e, scope);
                        }
                    }
                    ast::DataLiteralKind::Repeat { value, count } => {
                        self.resolve_expr(value, scope);
                        self.resolve_expr(count, scope);
                    }
                    ast::DataLiteralKind::Scalar(inner) => {
                        self.resolve_expr(inner, scope);
                    }
                }
            }
            ast::ExprKind::SliceOp {
                lhs, start, end, ..
            } => {
                self.resolve_expr(lhs, scope);
                if let Some(s) = start {
                    self.resolve_expr(s, scope);
                }
                if let Some(e) = end {
                    self.resolve_expr(e, scope);
                }
            }
            ast::ExprKind::Defer { expr: e } => self.resolve_expr(e, scope),
            ast::ExprKind::Return(Some(e)) => self.resolve_expr(e, scope),

            // Leaf nodes such as identifiers and literals contain no nested type nodes.
            _ => {}
        }
    }

    /// Strict path-based type resolution, including `module.submodule.Type[Generic]`.
    fn resolve_path_type(
        &mut self,
        segments: &[SymbolId],
        generics: &[ast::TypeNode],
        env_scope: ScopeId,
        span: Span,
    ) -> TypeId {
        if segments.is_empty() {
            return TypeId::ERROR;
        }

        let mut curr_scope = env_scope;
        let mut target_symbol = None;

        // Resolve the path segment by segment.
        for (i, &segment) in segments.iter().enumerate() {
            if i == 0 {
                // First segment: a single identifier may refer to a builtin primitive.
                if segments.len() == 1 {
                    let name_str = self.ctx.resolve(segment).to_string();
                    if let Some(prim_id) = self.resolve_builtin_primitive(&name_str) {
                        if !generics.is_empty() {
                            self.ctx
                                .emit_error(span, "Primitive types do not take generic arguments");
                        }
                        return prim_id;
                    }
                }

                // Walk outward through lexical scopes.
                self.ctx.scopes.set_current_scope(curr_scope);
                target_symbol = self.ctx.scopes.resolve(segment).cloned();
            } else {
                // Later segments must resolve inside the previous module scope only.
                target_symbol = self.ctx.scopes.resolve_in(curr_scope, segment).cloned();
            }

            let sym = match target_symbol.as_ref() {
                Some(s) => s,
                None => {
                    let name = self.ctx.resolve(segment).to_string();
                    if i == 0 {
                        self.ctx
                            .emit_error(span, format!("Cannot find type `{}` in this scope", name));
                    } else {
                        self.ctx.emit_error(
                            span,
                            format!("Cannot find `{}` in the target module", name),
                        );
                    }
                    return TypeId::ERROR;
                }
            };

            // Intermediate segments must resolve to modules.
            if i < segments.len() - 1 {
                if sym.kind == SymbolKind::Module {
                    let Some(mod_def_id) =
                        self.required_def_id(sym, span, "module path segment", segment)
                    else {
                        return TypeId::ERROR;
                    };
                    let Some(module_scope) = self.module_scope_from_def(mod_def_id, span, segment)
                    else {
                        return TypeId::ERROR;
                    };
                    curr_scope = module_scope;
                } else {
                    let name = self.ctx.resolve(segment).to_string();
                    self.ctx
                        .emit_error(span, format!("`{}` is not a module", name));
                    return TypeId::ERROR;
                }
            }
        }

        let Some(final_sym) = target_symbol else {
            self.ctx.emit_ice(
                span,
                "Type path resolution reached the end of a non-empty path without a final symbol",
            );
            return TypeId::ERROR;
        };

        // Resolve attached generic arguments in the original lookup scope.
        let mut resolved_generics = Vec::with_capacity(generics.len());
        for gen_ast in generics {
            resolved_generics.push(self.resolve_type(gen_ast, env_scope));
        }

        // Validate the kind of the final resolved symbol.
        match final_sym.kind {
            SymbolKind::Struct | SymbolKind::Union => {
                let Some(def_id) = self.required_def_id(&final_sym, span, "type", segments[0])
                else {
                    return TypeId::ERROR;
                };
                if !self.check_type_generic_bounds(span, def_id, &resolved_generics) {
                    return TypeId::ERROR;
                }
                self.ctx
                    .type_registry
                    .intern(TypeKind::Def(def_id, resolved_generics))
            }
            SymbolKind::Enum => {
                let Some(def_id) = self.required_def_id(&final_sym, span, "enum type", segments[0])
                else {
                    return TypeId::ERROR;
                };
                if !self.check_type_generic_bounds(span, def_id, &resolved_generics) {
                    return TypeId::ERROR;
                }
                self.ctx
                    .type_registry
                    .intern(TypeKind::Enum(def_id, resolved_generics))
            }
            SymbolKind::Trait => {
                let Some(def_id) =
                    self.required_def_id(&final_sym, span, "trait object type", segments[0])
                else {
                    return TypeId::ERROR;
                };
                if !self.check_type_generic_bounds(span, def_id, &resolved_generics) {
                    return TypeId::ERROR;
                }
                self.ctx
                    .type_registry
                    .intern(TypeKind::TraitObject(def_id, resolved_generics))
            }
            SymbolKind::TypeParam => {
                if !resolved_generics.is_empty() {
                    self.ctx
                        .emit_error(span, "Type parameters cannot take generic arguments");
                }
                final_sym.type_id // Type parameters already carry their final `TypeId`.
            }
            SymbolKind::AssociatedType => {
                let Some(def_id) =
                    self.required_def_id(&final_sym, span, "associated type", segments[0])
                else {
                    return TypeId::ERROR;
                };

                let Some(assoc_def) = self.ctx.defs.get(def_id.0 as usize).and_then(|def| match def {
                    Def::AssociatedType(assoc) => Some(assoc.clone()),
                    _ => None,
                }) else {
                    self.ctx.emit_ice(span, "associated type symbol does not point to an associated type def");
                    return TypeId::ERROR;
                };

                if assoc_def.generics.len() != resolved_generics.len() {
                    self.ctx.emit_error(
                        span,
                        format!(
                            "associated type `{}` expects {} generic arguments, but {} were provided",
                            self.last_segment_name(segments),
                            assoc_def.generics.len(),
                            resolved_generics.len()
                        ),
                    );
                    return TypeId::ERROR;
                }

                if let Some(target) = assoc_def.target.as_ref() {
                    let target_ty = self
                        .ctx
                        .node_types
                        .get(&target.id)
                        .copied()
                        .unwrap_or(final_sym.type_id);
                    if resolved_generics.is_empty() {
                        return target_ty;
                    }
                    let mut map = std::collections::HashMap::new();
                    for (param, arg) in assoc_def.generics.iter().zip(resolved_generics.iter()) {
                        map.insert(param.name, *arg);
                    }
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                    return subst.substitute(target_ty);
                }

                self.ctx
                    .type_registry
                    .intern(TypeKind::Associated(def_id, resolved_generics))
            }
            SymbolKind::TypeAlias => {
                // Compiler-injected `T` or `Self` entries have no physical def and already store the right type.
                if final_sym.def_id.is_none() {
                    return final_sym.type_id;
                }
                let Some(def_id) =
                    self.required_def_id(&final_sym, span, "type alias", segments[0])
                else {
                    return TypeId::ERROR;
                };
                if !self.check_type_generic_bounds(span, def_id, &resolved_generics) {
                    return TypeId::ERROR;
                }

                // Re-read the latest target type instead of trusting a stale imported clone.
                let target_ty = if let Def::TypeAlias(t_def) = &self.ctx.defs[def_id.0 as usize] {
                    self.ctx
                        .node_types
                        .get(&t_def.target.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR)
                } else {
                    TypeId::ERROR
                };

                // Avoid silently propagating stale `ERROR` types back into the AST.
                if target_ty == TypeId::ERROR {
                    let name = self.last_segment_name(segments);
                    self.ctx.struct_error(span, format!("type alias `{}` could not be resolved", name))
                        .with_hint("this might be caused by an invalid circular alias dependency or use before resolution")
                        .emit();
                    return TypeId::ERROR;
                }

                if resolved_generics.is_empty() {
                    // No generic arguments were supplied, so forward the alias target directly.
                    target_ty
                } else {
                    // Load the alias definition to recover its generic parameter names.
                    if let Def::TypeAlias(t_def) = &self.ctx.defs[def_id.0 as usize] {
                        if t_def.generics.len() != resolved_generics.len() {
                            self.ctx.emit_error(
                                span,
                                format!(
                                    "Type alias `{}` expects {} generic arguments, but {} were provided",
                                    self.last_segment_name(segments),
                                    t_def.generics.len(),
                                    resolved_generics.len()
                                ),
                            );
                            return TypeId::ERROR;
                        }

                        // Build the substitution map and apply it.
                        let mut map = std::collections::HashMap::new();
                        for (i, param) in t_def.generics.iter().enumerate() {
                            map.insert(param.name, resolved_generics[i]);
                        }
                        let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                        subst.substitute(target_ty)
                    } else {
                        self.ctx.emit_ice(
                            span,
                            format!(
                                "Type alias symbol `{}` resolved to non-alias def {:?}",
                                self.last_segment_name(segments),
                                def_id
                            ),
                        );
                        TypeId::ERROR
                    }
                }
            }
            _ => {
                let name = self.last_segment_name(segments);
                self.ctx.emit_error(
                    span,
                    format!(
                        "`{}` is a {}, not a type",
                        name,
                        self.kind_to_string(final_sym.kind)
                    ),
                );
                TypeId::ERROR
            }
        }
    }

    // ==========================================
    //               Helpers
    // ==========================================

    fn resolve_builtin_primitive(&mut self, name: &str) -> Option<TypeId> {
        let scalar = match name {
            "void" => Some(TypeId::VOID),
            "bool" => Some(TypeId::BOOL),
            "i8" => Some(TypeId::I8),
            "i16" => Some(TypeId::I16),
            "i32" => Some(TypeId::I32),
            "i64" => Some(TypeId::I64),
            "i128" => Some(TypeId::I128),
            "isize" => Some(TypeId::ISIZE),
            "u8" => Some(TypeId::U8),
            "u16" => Some(TypeId::U16),
            "u32" => Some(TypeId::U32),
            "u64" => Some(TypeId::U64),
            "u128" => Some(TypeId::U128),
            "usize" => Some(TypeId::USIZE),
            "f32" => Some(TypeId::F32),
            "f64" => Some(TypeId::F64),
            "str" => Some(TypeId::STR),
            "never" => Some(TypeId::NEVER),
            _ => None,
        };

        if scalar.is_some() {
            return scalar;
        }

        self.parse_builtin_simd(name)
    }

    fn parse_builtin_simd(&mut self, name: &str) -> Option<TypeId> {
        let (base, lanes) = name.rsplit_once('x')?;
        let lanes: u16 = lanes.parse().ok()?;
        if lanes == 0 {
            return None;
        }

        let elem = match base {
            "bool" => Some(TypeId::BOOL),
            "i8" => Some(TypeId::I8),
            "i16" => Some(TypeId::I16),
            "i32" => Some(TypeId::I32),
            "i64" => Some(TypeId::I64),
            "i128" => Some(TypeId::I128),
            "isize" => Some(TypeId::ISIZE),
            "u8" => Some(TypeId::U8),
            "u16" => Some(TypeId::U16),
            "u32" => Some(TypeId::U32),
            "u64" => Some(TypeId::U64),
            "u128" => Some(TypeId::U128),
            "usize" => Some(TypeId::USIZE),
            "f32" => Some(TypeId::F32),
            "f64" => Some(TypeId::F64),
            _ => None,
        }?;

        Some(
            self.ctx
                .type_registry
                .intern(TypeKind::Simd { elem, lanes }),
        )
    }

    fn check_type_generic_bounds(&mut self, span: Span, def_id: DefId, arg_tys: &[TypeId]) -> bool {
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
        let Def::Impl(impl_def) = self.ctx.defs[impl_id.0 as usize].clone() else {
            return;
        };
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
            let self_ty = self
                .ctx
                .type_registry
                .intern(TypeKind::TraitObject(def_id, self_args));
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
            | TypeKind::TraitObject(_, args)
            | TypeKind::Associated(_, args)
            | TypeKind::FnDef(_, args) => {
                args.into_iter().any(|arg| self.type_contains_params(arg))
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

    fn required_def_id(
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

    fn module_scope_from_def(
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

    fn last_segment_name(&self, segments: &[SymbolId]) -> String {
        segments
            .last()
            .map(|sym| self.ctx.resolve(*sym).to_string())
            .unwrap_or_else(|| "<empty-path>".to_string())
    }

    fn bind_generics(&mut self, generics: &[ast::GenericParam], scope: ScopeId) {
        self.ctx.scopes.set_current_scope(scope);

        // Inject every generic parameter name into the current scope.
        for param in generics {
            let param_ty = self.ctx.type_registry.intern(TypeKind::Param(param.name));
            let info = SymbolInfo {
                kind: SymbolKind::TypeParam,
                node_id: self.ctx.next_node_id(),
                type_id: param_ty,
                def_id: None,
                span: param.span,
                is_pub: false,
                is_mut: false,
            };
            let _ = self.ctx.scopes.define(param.name, info);
        }
    }

    /// Resolve every type node in where-clauses so they are cached in `ctx.node_types`.
    fn resolve_where_clauses(&mut self, clauses: &[ast::WhereClause], scope: ScopeId) {
        for clause in clauses {
            // Resolve the constrained target type on the left-hand side.
            self.resolve_type(&clause.target_ty, scope);
            // Resolve every trait bound on the right-hand side.
            for bound in &clause.bounds {
                self.resolve_type(bound, scope);
            }
        }
    }

    fn bind_self_type(&mut self, target_ty: TypeId, scope: ScopeId, span: Span) {
        self.ctx.scopes.set_current_scope(scope);
        let self_sym = self.ctx.intern("Self");
        let info = SymbolInfo {
            kind: SymbolKind::TypeAlias,
            node_id: self.ctx.next_node_id(),
            type_id: target_ty,
            def_id: None,
            span,
            is_pub: false,
            is_mut: false,
        };
        // Allow shadowing here so generic bindings can override outer names.
        let _ = self.ctx.scopes.define(self_sym, info);
    }

    fn kind_to_string(&self, kind: SymbolKind) -> &'static str {
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

    fn ensure_sized(&mut self, ty: TypeId, span: Span) {
        let norm = self.ctx.type_registry.normalize(ty);
        if matches!(self.ctx.type_registry.get(norm), TypeKind::TraitObject(..)) {
            self.ctx.struct_error(span, "trait objects have dynamic size and cannot be used as naked types")
                .with_hint("in Kern, you must explicitly use a pointer for dynamic dispatch, e.g., `*Trait` or `*mut Trait`")
                .emit();
        }
    }
}
