use crate::SemaContext;
use crate::def::*;
use crate::scope::{SymbolInfo, SymbolKind};
use crate::ty::TypeId;
use kernc_ast::{self as ast, Decl, DeclKind, TypeKind};
use kernc_utils::{NodeId, Span, SymbolId};

struct FunctionCollectSpec<'a> {
    vis: Visibility,
    parent_impl: Option<DefId>,
    is_const: bool,
    is_extern: bool,
    generics: &'a [ast::GenericParam],
    where_clauses: &'a [ast::WhereClause],
    params: &'a [ast::FuncParam],
    ret_type: &'a ast::TypeNode,
    body: &'a Option<Box<ast::Expr>>,
    is_variadic: bool,
}

struct AliasCollectSpec<'a> {
    vis: Visibility,
    where_clauses: &'a [ast::WhereClause],
    bounds: &'a [ast::TypeNode],
    is_extern: bool,
    generics: &'a [ast::GenericParam],
    target: &'a ast::TypeNode,
}

struct OwnedDeclHeader {
    node_id: NodeId,
    span: Span,
    name_span: Span,
    name: SymbolId,
    docs: Option<ast::DocBlock>,
    attributes: Vec<ast::Attribute>,
    vis: Visibility,
}

struct FunctionCollectOwnedSpec {
    header: OwnedDeclHeader,
    parent_impl: Option<DefId>,
    is_const: bool,
    is_extern: bool,
    generics: Vec<ast::GenericParam>,
    where_clauses: Vec<ast::WhereClause>,
    params: Vec<ast::FuncParam>,
    ret_type: ast::TypeNode,
    body: Option<Box<ast::Expr>>,
    is_variadic: bool,
}

struct GlobalCollectOwnedSpec {
    header: OwnedDeclHeader,
    is_extern: bool,
    value: ast::Expr,
    is_static: bool,
    is_mut: bool,
}

struct AliasCollectOwnedSpec {
    header: OwnedDeclHeader,
    is_extern: bool,
    generics: Vec<ast::GenericParam>,
    where_clauses: Vec<ast::WhereClause>,
    bounds: Vec<ast::TypeNode>,
    target: ast::TypeNode,
}

struct SymbolDefSpec {
    name: SymbolId,
    kind: SymbolKind,
    node_id: NodeId,
    def_id: Option<DefId>,
    span: Span,
    vis: Visibility,
    is_mut: bool,
}

pub struct Collector<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
    current_module: Option<DefId>,
    current_module_imported: bool,
}

impl<'a, 'ctx> Collector<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self {
            ctx,
            current_module: None,
            current_module_imported: false,
        }
    }

    pub fn context(&mut self) -> &mut SemaContext<'ctx> {
        self.ctx
    }

    pub fn into_context(self) -> &'a mut SemaContext<'ctx> {
        self.ctx
    }

    fn emit_impl_assoc_type_bounds_error(
        &mut self,
        assoc_name: SymbolId,
        assoc_span: Span,
        first_bound_span: Span,
    ) {
        let assoc_name = self.ctx.resolve(assoc_name).to_string();
        self.ctx
            .struct_error(
                first_bound_span,
                format!(
                    "associated type `{}` in an impl cannot declare trait bounds",
                    assoc_name
                ),
            )
            .with_span_label(
                assoc_span,
                "this impl-associated type must choose a concrete target only",
            )
            .with_hint(format!(
                "write `type {} = ConcreteType;` in the impl",
                assoc_name
            ))
            .with_hint(format!(
                "declare the contract on the trait instead, for example `type {}: Bound;`",
                assoc_name
            ))
            .emit();
    }

    /// Collect all top-level members from a module AST into semantic definitions.
    pub fn collect_ast(&mut self, mod_id: DefId, module: &ast::Module) {
        let (scope_id, submodules) =
            if let Some(Def::Module(m)) = self.ctx.defs.get(mod_id.0 as usize) {
                (m.scope_id, m.submodules.clone())
            } else {
                self.ctx.emit_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Collect): DefId {} is not a module during AST collection.",
                        mod_id.0
                    ),
                );
                return;
            };

        let parent_module = self.current_module;
        let parent_module_imported = self.current_module_imported;
        self.current_module = Some(mod_id);
        self.current_module_imported = matches!(
            self.ctx.defs.get(mod_id.0 as usize),
            Some(Def::Module(ModuleDef {
                is_imported: true,
                ..
            }))
        );

        let prev_scope = self.ctx.scopes.current_scope_id();
        self.ctx.scopes.set_current_scope(scope_id);

        let mut item_ids = Vec::new();
        let mut imports = Vec::new();

        // Collect imports, submodule declarations, and regular items in one pass.
        for decl in &module.decls {
            match &decl.kind {
                DeclKind::Use { kind, path, target } => {
                    imports.push(ImportDef {
                        path_kind: *kind,
                        path: path.clone(),
                        target: target.clone(),
                        vis: decl.vis,
                        span: decl.span,
                        binding_span: decl.name_span,
                    });
                }
                DeclKind::ModDecl => {
                    if let Some(&sub_id) = submodules.get(&decl.name) {
                        self.define_symbol(SymbolDefSpec {
                            name: decl.name,
                            kind: SymbolKind::Module,
                            node_id: decl.id,
                            def_id: Some(sub_id),
                            span: decl.name_span,
                            vis: decl.vis,
                            is_mut: false,
                        });
                    }
                }
                DeclKind::ExternBlock { decls, .. } => {
                    for ext_decl in decls {
                        if let Some(def_id) = self.collect_decl(ext_decl, None, true, &[]) {
                            item_ids.push(def_id);
                        }
                    }
                }
                _ => {
                    if let Some(def_id) = self.collect_decl(decl, None, false, &[]) {
                        item_ids.push(def_id);
                    }
                }
            }
        }

        if let Def::Module(m) = &mut self.ctx.defs[mod_id.0 as usize] {
            m.items = item_ids;
            m.imports = imports;
            m.docs = Self::clone_docs_if_present(&module.docs);
        }

        if let Some(prev) = prev_scope {
            self.ctx.scopes.set_current_scope(prev);
        }
        self.current_module = parent_module;
        self.current_module_imported = parent_module_imported;
    }

    pub fn collect_ast_owned(&mut self, mod_id: DefId, module: ast::Module) {
        let (scope_id, submodules) =
            if let Some(Def::Module(m)) = self.ctx.defs.get(mod_id.0 as usize) {
                (m.scope_id, m.submodules.clone())
            } else {
                self.ctx.emit_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Collect): DefId {} is not a module during AST collection.",
                        mod_id.0
                    ),
                );
                return;
            };

        let parent_module = self.current_module;
        let parent_module_imported = self.current_module_imported;
        self.current_module = Some(mod_id);
        self.current_module_imported = matches!(
            self.ctx.defs.get(mod_id.0 as usize),
            Some(Def::Module(ModuleDef {
                is_imported: true,
                ..
            }))
        );

        let prev_scope = self.ctx.scopes.current_scope_id();
        self.ctx.scopes.set_current_scope(scope_id);

        let ast::Module { docs, decls, .. } = module;
        let mut item_ids = Vec::new();
        let mut imports = Vec::new();

        for decl in decls {
            match decl {
                Decl {
                    kind: DeclKind::Use { kind, path, target },
                    span,
                    name_span,
                    ..
                } => {
                    imports.push(ImportDef {
                        path_kind: kind,
                        path,
                        target,
                        vis: decl.vis,
                        span,
                        binding_span: name_span,
                    });
                }
                Decl {
                    kind: DeclKind::ModDecl,
                    id,
                    name,
                    name_span,
                    vis,
                    ..
                } => {
                    if let Some(&sub_id) = submodules.get(&name) {
                        self.define_symbol(SymbolDefSpec {
                            name,
                            kind: SymbolKind::Module,
                            node_id: id,
                            def_id: Some(sub_id),
                            span: name_span,
                            vis,
                            is_mut: false,
                        });
                    }
                }
                Decl {
                    kind: DeclKind::ExternBlock { decls, .. },
                    ..
                } => {
                    for ext_decl in decls {
                        if let Some(def_id) = self.collect_decl_owned(ext_decl, None, true, &[]) {
                            item_ids.push(def_id);
                        }
                    }
                }
                other_decl => {
                    if let Some(def_id) = self.collect_decl_owned(other_decl, None, false, &[]) {
                        item_ids.push(def_id);
                    }
                }
            }
        }

        if let Def::Module(m) = &mut self.ctx.defs[mod_id.0 as usize] {
            m.items = item_ids;
            m.imports = imports;
            m.docs = Self::take_docs_if_present(docs);
        }

        if let Some(prev) = prev_scope {
            self.ctx.scopes.set_current_scope(prev);
        }
        self.current_module = parent_module;
        self.current_module_imported = parent_module_imported;
    }

    /// Collect a single declaration.
    /// `parent_impl` identifies the enclosing impl block, if any.
    /// `force_extern` marks declarations originating from an `extern` block.
    fn collect_decl(
        &mut self,
        decl: &Decl,
        parent_impl: Option<DefId>,
        force_extern: bool,
        impl_generics: &[ast::GenericParam],
    ) -> Option<DefId> {
        let vis = decl.vis;

        match &decl.kind {
            DeclKind::Function {
                generics,
                where_clauses,
                params,
                ret_type,
                body,
                is_const,
                is_extern,
                is_variadic,
            } => {
                // Impl methods see both impl-level and method-level generic parameters.
                let mut combined_generics = impl_generics.to_vec();
                combined_generics.extend_from_slice(generics);

                self.collect_function(
                    decl,
                    FunctionCollectSpec {
                        vis,
                        parent_impl,
                        is_const: *is_const,
                        is_extern: force_extern || *is_extern,
                        generics: &combined_generics,
                        where_clauses,
                        params,
                        ret_type,
                        body,
                        is_variadic: *is_variadic,
                    },
                )
            }
            DeclKind::Var {
                value,
                is_static,
                is_extern,
                is_mut,
            } => self.collect_global(
                decl,
                vis,
                force_extern || *is_extern,
                value,
                *is_static,
                *is_mut,
            ),
            DeclKind::TypeAlias {
                generics,
                target,
                is_extern,
                where_clauses,
                bounds,
            } => self.collect_type_alias_or_struct(
                decl,
                AliasCollectSpec {
                    vis,
                    where_clauses,
                    bounds,
                    is_extern: force_extern || *is_extern,
                    generics,
                    target,
                },
            ),
            DeclKind::Impl {
                generics,
                where_clauses,
                target_type,
                trait_type,
                decls,
            } => self.collect_impl(
                decl,
                generics,
                where_clauses,
                target_type,
                trait_type,
                decls,
            ),
            DeclKind::ExternBlock { .. } => {
                // Extern blocks must be flattened by `collect_ast` before reaching here.
                // Arriving here means the AST contains an invalid nesting.
                self.ctx.emit_error(
                    decl.span,
                    "`extern` blocks are only allowed at the module top-level",
                );
                None
            }
            // Already handled by `collect_ast`.
            DeclKind::Use { .. } => None,
            DeclKind::ModDecl => None,
        }
    }

    fn collect_decl_owned(
        &mut self,
        decl: Decl,
        parent_impl: Option<DefId>,
        force_extern: bool,
        impl_generics: &[ast::GenericParam],
    ) -> Option<DefId> {
        let vis = decl.vis;
        let Decl {
            id,
            span,
            name_span,
            name,
            docs,
            attributes,
            kind,
            ..
        } = decl;
        let header = OwnedDeclHeader {
            node_id: id,
            span,
            name_span,
            name,
            docs,
            attributes,
            vis,
        };

        match kind {
            DeclKind::Function {
                generics,
                where_clauses,
                params,
                ret_type,
                body,
                is_const,
                is_extern,
                is_variadic,
            } => {
                let mut combined_generics = impl_generics.to_vec();
                combined_generics.extend(generics);

                self.collect_function_owned(FunctionCollectOwnedSpec {
                    header,
                    parent_impl,
                    is_const,
                    is_extern: force_extern || is_extern,
                    generics: combined_generics,
                    where_clauses,
                    params,
                    ret_type,
                    body,
                    is_variadic,
                })
            }
            DeclKind::Var {
                value,
                is_static,
                is_extern,
                is_mut,
            } => self.collect_global_owned(GlobalCollectOwnedSpec {
                header,
                is_extern: force_extern || is_extern,
                value,
                is_static,
                is_mut,
            }),
            DeclKind::TypeAlias {
                generics,
                target,
                is_extern,
                where_clauses,
                bounds,
            } => self.collect_type_alias_or_struct_owned(AliasCollectOwnedSpec {
                header,
                is_extern: force_extern || is_extern,
                generics,
                where_clauses,
                bounds,
                target,
            }),
            DeclKind::Impl {
                generics,
                where_clauses,
                target_type,
                trait_type,
                decls,
            } => self.collect_impl_owned(
                span,
                generics,
                where_clauses,
                target_type,
                trait_type,
                decls,
            ),
            DeclKind::ExternBlock { .. } => {
                self.ctx.emit_error(
                    span,
                    "`extern` blocks are only allowed at the module top-level",
                );
                None
            }
            DeclKind::Use { .. } => None,
            DeclKind::ModDecl => None,
        }
    }

    fn collect_function(&mut self, decl: &Decl, spec: FunctionCollectSpec<'_>) -> Option<DefId> {
        let def_id = DefId(self.ctx.defs.len() as u32);

        let mut actual_params = spec.params.to_vec();

        if spec.parent_impl.is_some() {
            let self_sym = self.ctx.intern("self");
            let node_id = self.ctx.next_node_id();

            actual_params.insert(
                0,
                ast::FuncParam {
                    pattern: ast::BindingPattern {
                        name: self_sym,
                        name_span: decl.span,
                        is_mut: false,
                        span: decl.span,
                    },
                    type_node: ast::TypeNode {
                        id: node_id,
                        span: decl.span,
                        kind: ast::TypeKind::SelfType,
                    },
                    span: decl.span,
                },
            );
        }

        let func_def = FunctionDef {
            id: def_id,
            name: decl.name,
            name_span: decl.name_span,
            vis: spec.vis,
            parent: spec.parent_impl.or(self.current_module),
            is_imported: self.current_module_imported,
            generics: spec.generics.to_vec(),
            where_clauses: spec.where_clauses.to_vec(),
            params: actual_params,
            ret_type: spec.ret_type.clone(),
            body: spec.body.clone(),
            is_const: spec.is_const,
            is_extern: spec.is_extern,
            is_variadic: spec.is_variadic,
            is_intrinsic: false,
            span: decl.span,
            resolved_sig: None,
            docs: Self::clone_docs_if_present(&decl.docs),
            attributes: decl.attributes.clone(),
        };

        self.ctx.add_def(Def::Function(func_def));
        self.ctx
            .register_def_owner(def_id, self.current_module, self.current_owner_scope());

        // Only free functions are inserted into the surrounding lexical scope.
        if spec.parent_impl.is_none() {
            self.define_symbol(SymbolDefSpec {
                name: decl.name,
                kind: SymbolKind::Function,
                node_id: decl.id,
                def_id: Some(def_id),
                span: decl.name_span,
                vis: spec.vis,
                is_mut: false,
            });
        }

        Some(def_id)
    }

    fn collect_function_owned(&mut self, spec: FunctionCollectOwnedSpec) -> Option<DefId> {
        let FunctionCollectOwnedSpec {
            header:
                OwnedDeclHeader {
                    node_id,
                    span,
                    name_span,
                    name,
                    docs,
                    attributes,
                    vis,
                },
            parent_impl,
            is_const,
            is_extern,
            generics,
            where_clauses,
            params,
            ret_type,
            body,
            is_variadic,
        } = spec;
        let def_id = DefId(self.ctx.defs.len() as u32);
        let mut actual_params = params;

        if parent_impl.is_some() {
            let self_sym = self.ctx.intern("self");
            let self_node_id = self.ctx.next_node_id();

            actual_params.insert(
                0,
                ast::FuncParam {
                    pattern: ast::BindingPattern {
                        name: self_sym,
                        name_span: span,
                        is_mut: false,
                        span,
                    },
                    type_node: ast::TypeNode {
                        id: self_node_id,
                        span,
                        kind: ast::TypeKind::SelfType,
                    },
                    span,
                },
            );
        }

        let func_def = FunctionDef {
            id: def_id,
            name,
            name_span,
            vis,
            parent: parent_impl.or(self.current_module),
            is_imported: self.current_module_imported,
            generics,
            where_clauses,
            params: actual_params,
            ret_type,
            body,
            is_const,
            is_extern,
            is_variadic,
            is_intrinsic: false,
            span,
            resolved_sig: None,
            docs: Self::take_docs_if_present(docs),
            attributes,
        };

        self.ctx.add_def(Def::Function(func_def));
        self.ctx
            .register_def_owner(def_id, self.current_module, self.current_owner_scope());

        if parent_impl.is_none() {
            self.define_symbol(SymbolDefSpec {
                name,
                kind: SymbolKind::Function,
                node_id,
                def_id: Some(def_id),
                span: name_span,
                vis,
                is_mut: false,
            });
        }

        Some(def_id)
    }

    fn collect_global(
        &mut self,
        decl: &Decl,
        vis: Visibility,
        is_extern: bool,
        value: &ast::Expr,
        is_static: bool,
        is_mut: bool,
    ) -> Option<DefId> {
        let def_id = DefId(self.ctx.defs.len() as u32);

        let global_def = GlobalDef {
            id: def_id,
            name: decl.name,
            vis,
            parent: self.current_module,
            is_imported: self.current_module_imported,
            value: value.clone(),
            is_static,
            is_extern,
            is_mut,
            span: decl.span,
            docs: Self::clone_docs_if_present(&decl.docs),
            attributes: decl.attributes.clone(),
        };

        self.ctx.add_def(Def::Global(global_def));
        self.ctx
            .register_def_owner(def_id, self.current_module, self.current_owner_scope());

        let sym_kind = if is_static {
            SymbolKind::Static
        } else {
            SymbolKind::Const
        };
        self.define_symbol(SymbolDefSpec {
            name: decl.name,
            kind: sym_kind,
            node_id: decl.id,
            def_id: Some(def_id),
            span: decl.name_span,
            vis,
            is_mut,
        });

        Some(def_id)
    }

    fn collect_global_owned(&mut self, spec: GlobalCollectOwnedSpec) -> Option<DefId> {
        let GlobalCollectOwnedSpec {
            header:
                OwnedDeclHeader {
                    node_id,
                    span,
                    name_span,
                    name,
                    docs,
                    attributes,
                    vis,
                },
            is_extern,
            value,
            is_static,
            is_mut,
        } = spec;
        let def_id = DefId(self.ctx.defs.len() as u32);

        let global_def = GlobalDef {
            id: def_id,
            name,
            vis,
            parent: self.current_module,
            is_imported: self.current_module_imported,
            value,
            is_static,
            is_extern,
            is_mut,
            span,
            docs: Self::take_docs_if_present(docs),
            attributes,
        };

        self.ctx.add_def(Def::Global(global_def));
        self.ctx
            .register_def_owner(def_id, self.current_module, self.current_owner_scope());

        let sym_kind = if is_static {
            SymbolKind::Static
        } else {
            SymbolKind::Const
        };

        self.define_symbol(SymbolDefSpec {
            name,
            kind: sym_kind,
            node_id,
            def_id: Some(def_id),
            span: name_span,
            vis,
            is_mut,
        });

        Some(def_id)
    }

    /// Lower `type Name = Target` into the corresponding semantic definition kind.
    fn collect_trait_assoc_types(
        &mut self,
        trait_id: DefId,
        assoc_types: &[ast::AssociatedTypeDecl],
    ) -> Vec<DefId> {
        let mut ids = Vec::with_capacity(assoc_types.len());
        for assoc in assoc_types {
            let def_id = DefId(self.ctx.defs.len() as u32);
            self.ctx.add_def(Def::AssociatedType(AssociatedTypeDef {
                id: def_id,
                name: assoc.name,
                parent_trait: Some(trait_id),
                parent_impl: None,
                implemented_trait_assoc: None,
                is_imported: self.current_module_imported,
                generics: assoc.generics.clone(),
                bounds: assoc.bounds.clone(),
                where_clauses: assoc.where_clauses.clone(),
                target: None,
                resolved_bounds: Vec::new(),
                span: assoc.span,
                docs: Self::clone_docs_if_present(&assoc.docs),
            }));
            self.ctx
                .register_def_owner(def_id, self.current_module, self.current_owner_scope());
            ids.push(def_id);
        }
        ids
    }

    fn collect_trait_assoc_types_owned(
        &mut self,
        trait_id: DefId,
        assoc_types: Vec<ast::AssociatedTypeDecl>,
    ) -> Vec<DefId> {
        let mut ids = Vec::with_capacity(assoc_types.len());
        for assoc in assoc_types {
            let def_id = DefId(self.ctx.defs.len() as u32);
            self.ctx.add_def(Def::AssociatedType(AssociatedTypeDef {
                id: def_id,
                name: assoc.name,
                parent_trait: Some(trait_id),
                parent_impl: None,
                implemented_trait_assoc: None,
                is_imported: self.current_module_imported,
                generics: assoc.generics,
                bounds: assoc.bounds,
                where_clauses: assoc.where_clauses,
                target: None,
                resolved_bounds: Vec::new(),
                span: assoc.span,
                docs: Self::take_docs_if_present(assoc.docs),
            }));
            self.ctx
                .register_def_owner(def_id, self.current_module, self.current_owner_scope());
            ids.push(def_id);
        }
        ids
    }

    fn collect_type_alias_or_struct(
        &mut self,
        decl: &Decl,
        spec: AliasCollectSpec<'_>,
    ) -> Option<DefId> {
        let def_id = DefId(self.ctx.defs.len() as u32);
        let mut sym_kind = SymbolKind::TypeAlias;
        let mut pending_trait_assoc_types = None;

        let def = match &spec.target.kind {
            // TODO:
            TypeKind::Struct {
                is_extern: target_extern,
                fields,
            } => {
                sym_kind = SymbolKind::Struct;
                Def::Struct(StructDef {
                    id: def_id,
                    name: decl.name,
                    vis: spec.vis,
                    parent_module: self.current_module,
                    is_imported: self.current_module_imported,
                    generics: spec.generics.to_vec(),
                    where_clauses: spec.where_clauses.to_vec(),
                    fields: fields.clone(),
                    is_extern: spec.is_extern || *target_extern,
                    span: decl.span,
                    docs: Self::clone_docs_if_present(&decl.docs),
                    attributes: decl.attributes.clone(),
                })
            }
            TypeKind::Union {
                is_extern: target_extern,
                fields,
            } => {
                sym_kind = SymbolKind::Union;
                Def::Union(UnionDef {
                    id: def_id,
                    name: decl.name,
                    vis: spec.vis,
                    parent_module: self.current_module,
                    is_imported: self.current_module_imported,
                    generics: spec.generics.to_vec(),
                    where_clauses: spec.where_clauses.to_vec(),
                    fields: fields.clone(),
                    is_extern: spec.is_extern || *target_extern,
                    span: decl.span,
                    docs: Self::clone_docs_if_present(&decl.docs),
                })
            }
            TypeKind::Enum {
                backing_type,
                variants,
            } => {
                if spec.is_extern {
                    self.ctx
                        .struct_error(decl.span, "enum types do not support `extern`")
                        .with_hint("use `extern` on structs or unions for C-ABI layout control")
                        .emit();
                }
                sym_kind = SymbolKind::Enum;
                Def::Enum(EnumDef {
                    id: def_id,
                    name: decl.name,
                    vis: spec.vis,
                    is_imported: self.current_module_imported,
                    generics: spec.generics.to_vec(),
                    where_clauses: spec.where_clauses.to_vec(),
                    backing_type: backing_type.clone(),
                    variants: variants.clone(),
                    span: decl.span,
                    docs: Self::clone_docs_if_present(&decl.docs),
                })
            }
            TypeKind::Trait {
                assoc_types,
                methods,
            } => {
                sym_kind = SymbolKind::Trait;
                pending_trait_assoc_types = Some(assoc_types.clone());
                Def::Trait(TraitDef {
                    id: def_id,
                    name: decl.name,
                    vis: spec.vis,
                    is_imported: self.current_module_imported,
                    generics: spec.generics.to_vec(),
                    where_clauses: spec.where_clauses.to_vec(),
                    supertraits: spec.bounds.to_vec(),
                    assoc_types: Vec::new(),
                    methods: methods.clone(),
                    resolved_methods: Vec::new(),
                    resolved_supertraits: Vec::new(),
                    is_builtin: false,
                    span: decl.span,
                    docs: Self::clone_docs_if_present(&decl.docs),
                })
            }
            _ => {
                // True type aliases preserve the aliased target rather than becoming a new nominal type.
                Def::TypeAlias(TypeAliasDef {
                    id: def_id,
                    name: decl.name,
                    vis: spec.vis,
                    is_imported: self.current_module_imported,
                    generics: spec.generics.to_vec(),
                    where_clauses: spec.where_clauses.to_vec(),
                    target: spec.target.clone(),
                    span: decl.span,
                    docs: Self::clone_docs_if_present(&decl.docs),
                })
            }
        };

        self.ctx.add_def(def);
        self.ctx
            .register_def_owner(def_id, self.current_module, self.current_owner_scope());
        if let Some(assoc_types) = pending_trait_assoc_types {
            let assoc_type_ids = self.collect_trait_assoc_types(def_id, &assoc_types);
            if let Def::Trait(trait_def) = &mut self.ctx.defs[def_id.0 as usize] {
                trait_def.assoc_types = assoc_type_ids;
            }
        }
        self.define_symbol(SymbolDefSpec {
            name: decl.name,
            kind: sym_kind,
            node_id: decl.id,
            def_id: Some(def_id),
            span: decl.name_span,
            vis: spec.vis,
            is_mut: false,
        });

        Some(def_id)
    }

    fn collect_type_alias_or_struct_owned(&mut self, spec: AliasCollectOwnedSpec) -> Option<DefId> {
        let AliasCollectOwnedSpec {
            header:
                OwnedDeclHeader {
                    node_id,
                    span,
                    name_span,
                    name,
                    docs,
                    attributes,
                    vis,
                },
            is_extern,
            generics,
            where_clauses,
            bounds,
            target,
        } = spec;
        let def_id = DefId(self.ctx.defs.len() as u32);
        let mut sym_kind = SymbolKind::TypeAlias;
        let mut pending_trait_assoc_types = None;

        let ast::TypeNode {
            id: target_id,
            span: target_span,
            kind: target_kind,
        } = target;

        let def = match target_kind {
            TypeKind::Struct {
                is_extern: target_extern,
                fields,
            } => {
                sym_kind = SymbolKind::Struct;
                Def::Struct(StructDef {
                    id: def_id,
                    name,
                    vis,
                    parent_module: self.current_module,
                    is_imported: self.current_module_imported,
                    generics,
                    where_clauses,
                    fields,
                    is_extern: is_extern || target_extern,
                    span,
                    docs: Self::take_docs_if_present(docs),
                    attributes,
                })
            }
            TypeKind::Union {
                is_extern: target_extern,
                fields,
            } => {
                sym_kind = SymbolKind::Union;
                Def::Union(UnionDef {
                    id: def_id,
                    name,
                    vis,
                    parent_module: self.current_module,
                    is_imported: self.current_module_imported,
                    generics,
                    where_clauses,
                    fields,
                    is_extern: is_extern || target_extern,
                    span,
                    docs: Self::take_docs_if_present(docs),
                })
            }
            TypeKind::Enum {
                backing_type,
                variants,
            } => {
                if is_extern {
                    self.ctx
                        .struct_error(span, "enum types do not support `extern`")
                        .with_hint("use `extern` on structs or unions for C-ABI layout control")
                        .emit();
                }
                sym_kind = SymbolKind::Enum;
                Def::Enum(EnumDef {
                    id: def_id,
                    name,
                    vis,
                    is_imported: self.current_module_imported,
                    generics,
                    where_clauses,
                    backing_type,
                    variants,
                    span,
                    docs: Self::take_docs_if_present(docs),
                })
            }
            TypeKind::Trait {
                assoc_types,
                methods,
            } => {
                sym_kind = SymbolKind::Trait;
                pending_trait_assoc_types = Some(assoc_types);
                Def::Trait(TraitDef {
                    id: def_id,
                    name,
                    vis,
                    is_imported: self.current_module_imported,
                    generics,
                    where_clauses,
                    supertraits: bounds,
                    assoc_types: Vec::new(),
                    methods,
                    resolved_methods: Vec::new(),
                    resolved_supertraits: Vec::new(),
                    is_builtin: false,
                    span,
                    docs: Self::take_docs_if_present(docs),
                })
            }
            kind => Def::TypeAlias(TypeAliasDef {
                id: def_id,
                name,
                vis,
                is_imported: self.current_module_imported,
                generics,
                where_clauses,
                target: ast::TypeNode {
                    id: target_id,
                    span: target_span,
                    kind,
                },
                span,
                docs: Self::take_docs_if_present(docs),
            }),
        };

        self.ctx.add_def(def);
        self.ctx
            .register_def_owner(def_id, self.current_module, self.current_owner_scope());
        if let Some(assoc_types) = pending_trait_assoc_types {
            let assoc_type_ids = self.collect_trait_assoc_types_owned(def_id, assoc_types);
            if let Def::Trait(trait_def) = &mut self.ctx.defs[def_id.0 as usize] {
                trait_def.assoc_types = assoc_type_ids;
            }
        }
        self.define_symbol(SymbolDefSpec {
            name,
            kind: sym_kind,
            node_id,
            def_id: Some(def_id),
            span: name_span,
            vis,
            is_mut: false,
        });

        Some(def_id)
    }

    fn collect_impl(
        &mut self,
        decl: &Decl,
        generics: &[ast::GenericParam],
        where_clauses: &[ast::WhereClause],
        target_type: &ast::TypeNode,
        trait_type: &Option<ast::TypeNode>,
        decls: &[Decl],
    ) -> Option<DefId> {
        let impl_id = DefId(self.ctx.defs.len() as u32);
        self.ctx.impl_index.global_impls.push(impl_id);
        if trait_type.is_some() {
            self.ctx.impl_index.trait_impls.push(impl_id);
        }
        let mut assoc_type_ids = Vec::new();
        let mut method_ids = Vec::new();
        self.ctx.add_def(Def::Impl(ImplDef {
            id: impl_id,
            parent_module: self.current_module,
            is_imported: self.current_module_imported,
            generics: generics.to_vec(),
            where_clauses: where_clauses.to_vec(),
            target_type: target_type.clone(),
            trait_type: trait_type.clone(),
            assoc_types: Vec::new(),
            methods: Vec::new(),
            span: decl.span,
        }));
        self.ctx
            .register_def_owner(impl_id, self.current_module, self.current_owner_scope());

        self.ctx.scopes.enter_scope();
        self.inject_generic_params(generics);

        for method_decl in decls {
            if matches!(method_decl.kind, DeclKind::Function { .. }) {
                if let Some(m_id) = self.collect_decl(method_decl, Some(impl_id), false, generics) {
                    method_ids.push(m_id);
                }
            } else if let DeclKind::TypeAlias {
                generics,
                bounds,
                where_clauses,
                target,
                is_extern: false,
            } = &method_decl.kind
            {
                if trait_type.is_none() {
                    self.ctx.emit_error(
                        method_decl.span,
                        "associated type definitions are only allowed in trait impls",
                    );
                    continue;
                }
                if let Some(first_bound) = bounds.first() {
                    self.emit_impl_assoc_type_bounds_error(
                        method_decl.name,
                        method_decl.span,
                        first_bound.span,
                    );
                }
                let def_id = DefId(self.ctx.defs.len() as u32);
                self.ctx.add_def(Def::AssociatedType(AssociatedTypeDef {
                    id: def_id,
                    name: method_decl.name,
                    parent_trait: None,
                    parent_impl: Some(impl_id),
                    implemented_trait_assoc: None,
                    is_imported: self.current_module_imported,
                    generics: generics.clone(),
                    bounds: Vec::new(),
                    where_clauses: where_clauses.clone(),
                    target: Some(target.clone()),
                    resolved_bounds: Vec::new(),
                    span: method_decl.span,
                    docs: Self::clone_docs_if_present(&method_decl.docs),
                }));
                self.ctx.register_def_owner(
                    def_id,
                    self.current_module,
                    self.current_owner_scope(),
                );
                assoc_type_ids.push(def_id);
            } else {
                self.ctx.emit_error(
                    method_decl.span,
                    "Only functions and associated type definitions are allowed inside `impl` blocks",
                );
            }
        }

        self.ctx.scopes.exit_scope();

        if let Def::Impl(i) = &mut self.ctx.defs[impl_id.0 as usize] {
            i.assoc_types = assoc_type_ids;
            i.methods = method_ids.clone();
        }

        for method_id in method_ids {
            let Some(Def::Function(function)) = self.ctx.defs.get(method_id.0 as usize) else {
                continue;
            };
            self.ctx
                .impl_index
                .impl_methods_by_name
                .entry(function.name)
                .or_default()
                .push(method_id);
        }

        Some(impl_id)
    }

    fn collect_impl_owned(
        &mut self,
        span: Span,
        generics: Vec<ast::GenericParam>,
        where_clauses: Vec<ast::WhereClause>,
        target_type: ast::TypeNode,
        trait_type: Option<ast::TypeNode>,
        decls: Vec<Decl>,
    ) -> Option<DefId> {
        let impl_id = DefId(self.ctx.defs.len() as u32);
        self.ctx.impl_index.global_impls.push(impl_id);
        if trait_type.is_some() {
            self.ctx.impl_index.trait_impls.push(impl_id);
        }
        let mut assoc_type_ids = Vec::new();
        let mut method_ids = Vec::new();
        self.ctx.add_def(Def::Impl(ImplDef {
            id: impl_id,
            parent_module: self.current_module,
            is_imported: self.current_module_imported,
            generics: generics.clone(),
            where_clauses,
            target_type,
            trait_type: trait_type.clone(),
            assoc_types: Vec::new(),
            methods: Vec::new(),
            span,
        }));
        self.ctx
            .register_def_owner(impl_id, self.current_module, self.current_owner_scope());

        self.ctx.scopes.enter_scope();
        self.inject_generic_params(&generics);

        for method_decl in decls {
            match method_decl {
                decl @ Decl {
                    kind: DeclKind::Function { .. },
                    ..
                } => {
                    if let Some(m_id) =
                        self.collect_decl_owned(decl, Some(impl_id), false, &generics)
                    {
                        method_ids.push(m_id);
                    }
                }
                Decl {
                    name,
                    span,
                    docs,
                    kind:
                        DeclKind::TypeAlias {
                            generics,
                            bounds,
                            where_clauses,
                            target,
                            is_extern: false,
                        },
                    ..
                } => {
                    if trait_type.is_none() {
                        self.ctx.emit_error(
                            span,
                            "associated type definitions are only allowed in trait impls",
                        );
                        continue;
                    }
                    if let Some(first_bound) = bounds.first() {
                        self.emit_impl_assoc_type_bounds_error(name, span, first_bound.span);
                    }
                    let def_id = DefId(self.ctx.defs.len() as u32);
                    self.ctx.add_def(Def::AssociatedType(AssociatedTypeDef {
                        id: def_id,
                        name,
                        parent_trait: None,
                        parent_impl: Some(impl_id),
                        implemented_trait_assoc: None,
                        is_imported: self.current_module_imported,
                        generics,
                        bounds: Vec::new(),
                        where_clauses,
                        target: Some(target),
                        resolved_bounds: Vec::new(),
                        span,
                        docs: Self::take_docs_if_present(docs),
                    }));
                    self.ctx.register_def_owner(
                        def_id,
                        self.current_module,
                        self.current_owner_scope(),
                    );
                    assoc_type_ids.push(def_id);
                }
                Decl { span, .. } => {
                    self.ctx
                        .emit_error(span, "Only functions and associated type definitions are allowed inside `impl` blocks");
                }
            }
        }

        self.ctx.scopes.exit_scope();

        if let Def::Impl(i) = &mut self.ctx.defs[impl_id.0 as usize] {
            i.assoc_types = assoc_type_ids;
            i.methods = method_ids.clone();
        }

        for method_id in method_ids {
            let Some(Def::Function(function)) = self.ctx.defs.get(method_id.0 as usize) else {
                continue;
            };
            self.ctx
                .impl_index
                .impl_methods_by_name
                .entry(function.name)
                .or_default()
                .push(method_id);
        }

        Some(impl_id)
    }

    // ==========================================
    //               Helpers
    // ==========================================

    /// Register a symbol in the current scope and surface duplicate-definition diagnostics.
    fn define_symbol(&mut self, spec: SymbolDefSpec) {
        // `_` is intentionally not entered into the symbol table.
        if self.ctx.resolve(spec.name) == "_" {
            return;
        }
        let info = SymbolInfo {
            kind: spec.kind,
            node_id: spec.node_id,
            type_id: TypeId::ERROR, // Types are resolved later.
            def_id: spec.def_id,
            span: spec.span, // Preserve the definition site for diagnostics.
            vis: spec.vis,
            is_mut: spec.is_mut,
        };

        // Emit a multi-span diagnostic that points at both definitions.
        if let Err(old_info) = self.ctx.scopes.define(spec.name, info) {
            let name_str = self.ctx.resolve(spec.name).to_string();

            self.ctx
                .struct_error(
                    spec.span,
                    format!("the name `{}` is defined multiple times", name_str),
                )
                .with_hint(format!(
                    "`{}` must be defined only once in the same scope",
                    name_str
                ))
                .with_span_label(
                    old_info.span,
                    format!("previous definition of `{}` was here", name_str),
                )
                .emit();
        }
    }

    fn inject_generic_params(&mut self, generics: &[ast::GenericParam]) {
        for param in generics {
            let generic_node_id = self.ctx.next_node_id();
            let kind = match param.kind {
                ast::GenericParamKind::Type => SymbolKind::TypeParam,
                ast::GenericParamKind::Const { .. } => SymbolKind::ConstParam,
            };

            self.define_symbol(SymbolDefSpec {
                name: param.name,
                kind,
                node_id: generic_node_id,
                def_id: None,
                span: param.span,
                vis: Visibility::Private,
                is_mut: false,
            });
        }
    }

    fn clone_docs_if_present(docs: &Option<ast::DocBlock>) -> Option<ast::DocBlock> {
        match docs {
            Some(block) if !block.lines.is_empty() => Some(block.clone()),
            _ => None,
        }
    }

    fn take_docs_if_present(docs: Option<ast::DocBlock>) -> Option<ast::DocBlock> {
        match docs {
            Some(block) if !block.lines.is_empty() => Some(block),
            _ => None,
        }
    }

    fn current_owner_scope(&self) -> Option<crate::scope::ScopeId> {
        let module_id = self.current_module?;
        match self.ctx.defs.get(module_id.0 as usize) {
            Some(Def::Module(module)) => Some(module.scope_id),
            _ => None,
        }
    }
}
