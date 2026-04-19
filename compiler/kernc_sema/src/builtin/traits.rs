use super::*;

impl<'a, 'ctx> BuiltinInjector<'a, 'ctx> {
    pub(super) fn builtin_trait_ty_by_id(
        &mut self,
        trait_def_id: DefId,
        args: Vec<TypeId>,
    ) -> TypeId {
        self.ctx.type_registry.intern(TypeKind::TraitObject(
            trait_def_id,
            crate::ty::wrap_type_args(args),
            Vec::new(),
        ))
    }

    pub(super) fn inject_builtin_trait(&mut self, spec: BuiltinTraitSpec<'_>) -> DefId {
        let name_id = self.ctx.intern(spec.name);
        let def_id = DefId(self.ctx.defs.len() as u32);

        let self_ty = self.builtin_trait_ty_by_id(def_id, vec![]);
        let resolved_methods = spec
            .methods
            .iter()
            .map(|method| {
                let params = std::iter::once(self_ty)
                    .chain(method.params.iter().copied())
                    .collect::<Vec<_>>();
                let sig = self.ctx.type_registry.intern(TypeKind::Function {
                    params,
                    ret: method.ret,
                    is_variadic: false,
                });
                (self.ctx.intern(method.name), sig)
            })
            .collect();

        let trait_def = TraitDef {
            id: def_id,
            name: name_id,
            vis: Visibility::Public,
            is_imported: false,
            generics: spec.generics,
            where_clauses: vec![],
            supertraits: vec![],
            resolved_supertraits: spec.supertraits,
            assoc_types: vec![],
            methods: vec![],
            resolved_methods,
            is_builtin: true,
            span: Span::default(),
            docs: None,
        };

        self.ctx.add_def(Def::Trait(trait_def));
        self.ctx.register_builtin_def(name_id, def_id);

        let info = SymbolInfo {
            kind: SymbolKind::Trait,
            node_id: self.ctx.next_node_id(),
            type_id: self.builtin_trait_ty_by_id(def_id, vec![]),
            def_id: Some(def_id),
            span: Default::default(),
            vis: Visibility::Public,
            is_mut: false,
        };
        let root_scope = ScopeId(0);
        self.ctx.scopes.set_current_scope(root_scope);
        let _ = self.ctx.scopes.define(name_id, info);

        def_id
    }

    pub(super) fn inject_binary_operator_trait_with_assoc_out(
        &mut self,
        trait_name: &str,
        method_name: &str,
    ) {
        let name_id = self.ctx.intern(trait_name);
        let method_name_id = self.ctx.intern(method_name);
        let out_name_id = self.ctx.intern("Out");
        let def_id = DefId(self.ctx.defs.len() as u32);
        let rhs = self.new_builtin_param("Rhs");
        let rhs_ty = self.ctx.type_registry.intern(TypeKind::Param(rhs.name));
        let out_assoc_id = DefId(def_id.0 + 1);
        let out_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Associated(out_assoc_id, vec![]));
        let self_ty = self.builtin_trait_ty_by_id(def_id, vec![]);
        let sig = self.ctx.type_registry.intern(TypeKind::Function {
            params: vec![self_ty, rhs_ty],
            ret: out_ty,
            is_variadic: false,
        });

        self.ctx.add_def(Def::Trait(TraitDef {
            id: def_id,
            name: name_id,
            vis: Visibility::Public,
            is_imported: false,
            generics: vec![rhs],
            where_clauses: vec![],
            supertraits: vec![],
            resolved_supertraits: vec![],
            assoc_types: vec![out_assoc_id],
            methods: vec![],
            resolved_methods: vec![(method_name_id, sig)],
            span: Span::default(),
            is_builtin: true,
            docs: None,
        }));
        self.ctx.add_def(Def::AssociatedType(AssociatedTypeDef {
            id: out_assoc_id,
            name: out_name_id,
            parent_trait: Some(def_id),
            parent_impl: None,
            is_imported: false,
            generics: vec![],
            bounds: vec![],
            where_clauses: vec![],
            target: None,
            resolved_bounds: vec![],
            span: Span::default(),
            docs: None,
        }));
        self.ctx.register_builtin_def(name_id, def_id);

        let info = SymbolInfo {
            kind: SymbolKind::Trait,
            node_id: self.ctx.next_node_id(),
            type_id: self.builtin_trait_ty_by_id(def_id, vec![]),
            def_id: Some(def_id),
            span: Default::default(),
            vis: Visibility::Public,
            is_mut: false,
        };
        let root_scope = ScopeId(0);
        self.ctx.scopes.set_current_scope(root_scope);
        let _ = self.ctx.scopes.define(name_id, info);
    }

    pub(super) fn inject_unary_operator_trait_with_assoc_out(
        &mut self,
        trait_name: &str,
        method_name: &str,
    ) {
        let name_id = self.ctx.intern(trait_name);
        let method_name_id = self.ctx.intern(method_name);
        let out_name_id = self.ctx.intern("Out");
        let def_id = DefId(self.ctx.defs.len() as u32);
        let out_assoc_id = DefId(def_id.0 + 1);
        let out_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Associated(out_assoc_id, vec![]));
        let self_ty = self.builtin_trait_ty_by_id(def_id, vec![]);
        let sig = self.ctx.type_registry.intern(TypeKind::Function {
            params: vec![self_ty],
            ret: out_ty,
            is_variadic: false,
        });

        self.ctx.add_def(Def::Trait(TraitDef {
            id: def_id,
            name: name_id,
            vis: Visibility::Public,
            is_imported: false,
            generics: vec![],
            where_clauses: vec![],
            supertraits: vec![],
            resolved_supertraits: vec![],
            assoc_types: vec![out_assoc_id],
            methods: vec![],
            resolved_methods: vec![(method_name_id, sig)],
            span: Span::default(),
            is_builtin: true,
            docs: None,
        }));
        self.ctx.add_def(Def::AssociatedType(AssociatedTypeDef {
            id: out_assoc_id,
            name: out_name_id,
            parent_trait: Some(def_id),
            parent_impl: None,
            is_imported: false,
            generics: vec![],
            bounds: vec![],
            where_clauses: vec![],
            target: None,
            resolved_bounds: vec![],
            span: Span::default(),
            docs: None,
        }));
        self.ctx.register_builtin_def(name_id, def_id);

        let info = SymbolInfo {
            kind: SymbolKind::Trait,
            node_id: self.ctx.next_node_id(),
            type_id: self.builtin_trait_ty_by_id(def_id, vec![]),
            def_id: Some(def_id),
            span: Default::default(),
            vis: Visibility::Public,
            is_mut: false,
        };
        let root_scope = ScopeId(0);
        self.ctx.scopes.set_current_scope(root_scope);
        let _ = self.ctx.scopes.define(name_id, info);
    }

    pub(super) fn inject_operator_traits(&mut self) {
        let rhs = self.new_builtin_param("Rhs");
        let rhs_ty = self.ctx.type_registry.intern(TypeKind::Param(rhs.name));

        self.inject_builtin_trait(BuiltinTraitSpec {
            name: "Eq",
            generics: vec![rhs.clone()],
            supertraits: vec![],
            methods: vec![BuiltinMethodSpec {
                name: "eq",
                params: vec![rhs_ty],
                ret: TypeId::BOOL,
            }],
        });
        self.inject_builtin_trait(BuiltinTraitSpec {
            name: "Lt",
            generics: vec![rhs.clone()],
            supertraits: vec![],
            methods: vec![BuiltinMethodSpec {
                name: "lt",
                params: vec![rhs_ty],
                ret: TypeId::BOOL,
            }],
        });
        self.inject_builtin_trait(BuiltinTraitSpec {
            name: "Le",
            generics: vec![rhs.clone()],
            supertraits: vec![],
            methods: vec![BuiltinMethodSpec {
                name: "le",
                params: vec![rhs_ty],
                ret: TypeId::BOOL,
            }],
        });
        self.inject_builtin_trait(BuiltinTraitSpec {
            name: "Gt",
            generics: vec![rhs.clone()],
            supertraits: vec![],
            methods: vec![BuiltinMethodSpec {
                name: "gt",
                params: vec![rhs_ty],
                ret: TypeId::BOOL,
            }],
        });
        self.inject_builtin_trait(BuiltinTraitSpec {
            name: "Ge",
            generics: vec![rhs.clone()],
            supertraits: vec![],
            methods: vec![BuiltinMethodSpec {
                name: "ge",
                params: vec![rhs_ty],
                ret: TypeId::BOOL,
            }],
        });

        for spec in [
            ("Add", "add", "@add"),
            ("Sub", "sub", "@sub"),
            ("Mul", "mul", "@mul"),
            ("Div", "div", "@div"),
            ("Rem", "rem", "@rem"),
            ("BitAnd", "bit_and", "@bitAnd"),
            ("BitOr", "bit_or", "@bitOr"),
            ("BitXor", "bit_xor", "@bitXor"),
            ("Shl", "shl", "@shl"),
            ("Shr", "shr", "@shr"),
        ] {
            self.inject_binary_operator_trait_with_assoc_out(spec.0, spec.1);
        }

        for spec in [
            ("Neg", "neg", "@neg"),
            ("BitNot", "bit_not", "@bitNot"),
            ("Not", "not", "@not"),
        ] {
            self.inject_unary_operator_trait_with_assoc_out(spec.0, spec.1);
        }
    }
}
