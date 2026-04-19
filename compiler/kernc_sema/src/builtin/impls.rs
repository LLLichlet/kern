use super::*;

impl<'a, 'ctx> BuiltinInjector<'a, 'ctx> {
    pub(super) fn inject_primitive_impl(&mut self, target_ty_id: TypeId, trait_def_id: DefId) {
        let def_id = DefId(self.ctx.defs.len() as u32);

        // Fabricate AST nodes so the existing semantic machinery can reuse them.
        let target_id = self.ctx.next_node_id();
        let trait_id = self.ctx.next_node_id();

        let target_node = TypeNode {
            id: target_id,
            span: Default::default(),
            kind: ast::TypeKind::Infer,
        };
        let trait_node = TypeNode {
            id: trait_id,
            span: Default::default(),
            kind: ast::TypeKind::Infer,
        };

        // Seed the real semantic types directly into the node-type cache.
        self.ctx.node_types.insert(target_node.id, target_ty_id);

        let trait_ty = self.builtin_trait_ty_by_id(trait_def_id, vec![]);
        self.ctx.node_types.insert(trait_node.id, trait_ty);

        let impl_def = ImplDef {
            id: def_id,
            parent_module: None,
            is_imported: false,
            generics: vec![],
            where_clauses: vec![],
            target_type: target_node,
            trait_type: Some(trait_node),
            assoc_types: vec![],
            methods: vec![],
            span: Default::default(),
        };
        self.ctx.add_def(Def::Impl(impl_def));
        self.ctx.global_impls.push(def_id);
        self.ctx.trait_impls.push(def_id);
    }

    pub(super) fn inject_operator_impl(
        &mut self,
        target_ty_id: TypeId,
        trait_name: &str,
        trait_args: Vec<TypeId>,
        method_name: &str,
        explicit_param_tys: Vec<TypeId>,
        ret_ty: TypeId,
    ) {
        let Some(trait_def_id) = self.ctx.builtin_def(trait_name) else {
            return;
        };

        let impl_id = DefId(self.ctx.defs.len() as u32);
        let target_id = self.ctx.next_node_id();
        let trait_id = self.ctx.next_node_id();

        let target_node = TypeNode {
            id: target_id,
            span: Span::default(),
            kind: ast::TypeKind::Infer,
        };
        let trait_node = TypeNode {
            id: trait_id,
            span: Span::default(),
            kind: ast::TypeKind::Infer,
        };

        self.ctx.node_types.insert(target_node.id, target_ty_id);
        let trait_ty = self.builtin_trait_ty_by_id(trait_def_id, trait_args.clone());
        self.ctx.node_types.insert(trait_node.id, trait_ty);

        let self_sym = self.ctx.intern("self");
        let self_param_ty_node_id = self.ctx.next_node_id();
        self.ctx
            .node_types
            .insert(self_param_ty_node_id, target_ty_id);
        let mut params = vec![ast::FuncParam {
            pattern: ast::BindingPattern {
                name: self_sym,
                name_span: Span::default(),
                is_mut: false,
                span: Span::default(),
            },
            type_node: TypeNode {
                id: self_param_ty_node_id,
                span: Span::default(),
                kind: ast::TypeKind::Infer,
            },
            span: Span::default(),
        }];

        let mut sig_params = vec![target_ty_id];
        for (index, param_ty) in explicit_param_tys.iter().copied().enumerate() {
            let name = self.ctx.intern(&format!("arg{}", index));
            let type_node_id = self.ctx.next_node_id();
            self.ctx.node_types.insert(type_node_id, param_ty);
            params.push(ast::FuncParam {
                pattern: ast::BindingPattern {
                    name,
                    name_span: Span::default(),
                    is_mut: false,
                    span: Span::default(),
                },
                type_node: TypeNode {
                    id: type_node_id,
                    span: Span::default(),
                    kind: ast::TypeKind::Infer,
                },
                span: Span::default(),
            });
            sig_params.push(param_ty);
        }

        let ret_type_id = self.ctx.next_node_id();
        self.ctx.node_types.insert(ret_type_id, ret_ty);
        let name_id = self.ctx.intern(method_name);
        let sig_ty = self.ctx.type_registry.intern(TypeKind::Function {
            params: sig_params,
            ret: ret_ty,
            is_variadic: false,
        });
        self.ctx.add_def(Def::Impl(ImplDef {
            id: impl_id,
            parent_module: None,
            is_imported: false,
            generics: vec![],
            where_clauses: vec![],
            target_type: target_node,
            trait_type: Some(trait_node),
            assoc_types: vec![],
            methods: vec![],
            span: Span::default(),
        }));
        self.ctx.global_impls.push(impl_id);
        self.ctx.trait_impls.push(impl_id);

        let assoc_specs = match self.ctx.defs.get(trait_def_id.0 as usize) {
            Some(Def::Trait(trait_def)) => {
                let generic_count = trait_def.generics.len();
                let assoc_args = trait_args
                    .iter()
                    .skip(generic_count)
                    .copied()
                    .collect::<Vec<_>>();
                trait_def
                    .assoc_types
                    .iter()
                    .copied()
                    .enumerate()
                    .filter_map(|(assoc_index, trait_assoc_id)| {
                        match self.ctx.defs.get(trait_assoc_id.0 as usize) {
                            Some(Def::AssociatedType(trait_assoc_def)) => Some((
                                trait_assoc_def.name,
                                assoc_args.get(assoc_index).copied().unwrap_or(ret_ty),
                            )),
                            _ => None,
                        }
                    })
                    .collect::<Vec<_>>()
            }
            _ => vec![],
        };
        let mut assoc_type_ids = Vec::with_capacity(assoc_specs.len());
        for (assoc_name, assoc_target_ty) in assoc_specs {
            let assoc_target_node_id = self.ctx.next_node_id();
            self.ctx
                .node_types
                .insert(assoc_target_node_id, assoc_target_ty);
            let assoc_def_id = DefId(self.ctx.defs.len() as u32);
            self.ctx.add_def(Def::AssociatedType(AssociatedTypeDef {
                id: assoc_def_id,
                name: assoc_name,
                parent_trait: Some(trait_def_id),
                parent_impl: Some(impl_id),
                is_imported: false,
                generics: vec![],
                bounds: vec![],
                where_clauses: vec![],
                target: Some(TypeNode {
                    id: assoc_target_node_id,
                    span: Span::default(),
                    kind: ast::TypeKind::Infer,
                }),
                resolved_bounds: vec![],
                span: Span::default(),
                docs: None,
            }));
            assoc_type_ids.push(assoc_def_id);
        }
        let method_def_id = DefId(self.ctx.defs.len() as u32);

        self.ctx.add_def(Def::Function(FunctionDef {
            id: method_def_id,
            name: name_id,
            name_span: Span::default(),
            vis: Visibility::Public,
            parent: Some(impl_id),
            is_imported: false,
            generics: vec![],
            where_clauses: vec![],
            params,
            ret_type: TypeNode {
                id: ret_type_id,
                span: Span::default(),
                kind: ast::TypeKind::Infer,
            },
            body: None,
            is_const: false,
            is_extern: false,
            is_variadic: false,
            is_intrinsic: true,
            resolved_sig: Some(sig_ty),
            span: Span::default(),
            docs: None,
            attributes: vec![],
        }));
        let trait_assoc_ids = self
            .ctx
            .defs
            .get(trait_def_id.0 as usize)
            .and_then(|def| match def {
                Def::Trait(trait_def) => Some(trait_def.assoc_types.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let canonical_assoc_bindings = trait_assoc_ids
            .into_iter()
            .zip(assoc_type_ids.iter().copied())
            .map(|(trait_assoc_id, impl_assoc_id)| {
                let target_ty = match self.ctx.defs.get(impl_assoc_id.0 as usize) {
                    Some(Def::AssociatedType(assoc_def)) => assoc_def
                        .target
                        .as_ref()
                        .and_then(|target| self.ctx.node_types.get(&target.id).copied())
                        .unwrap_or(TypeId::ERROR),
                    _ => TypeId::ERROR,
                };
                (trait_assoc_id, target_ty)
            })
            .collect();
        let canonical_trait_ty = self.ctx.type_registry.intern(TypeKind::TraitObject(
            trait_def_id,
            crate::ty::wrap_type_args(trait_args),
            canonical_assoc_bindings,
        ));
        self.ctx.node_types.insert(trait_id, canonical_trait_ty);

        if let Some(Def::Impl(impl_def)) = self.ctx.defs.get_mut(impl_id.0 as usize) {
            impl_def.assoc_types = assoc_type_ids;
            impl_def.methods = vec![method_def_id];
        }
        self.ctx
            .impl_methods_by_name
            .entry(name_id)
            .or_default()
            .push(method_def_id);
    }

    pub(super) fn inject_integer_operator_impls(&mut self, ty: TypeId) {
        self.inject_eq_like_impls(ty);
        self.inject_binary_same_type_impls(
            ty,
            &[
                "Add", "Sub", "Mul", "Div", "Rem", "BitAnd", "BitOr", "BitXor",
            ],
        );
        self.inject_shift_impls(ty);
        self.inject_unary_same_type_impl(ty, "Neg");
        self.inject_unary_same_type_impl(ty, "BitNot");
    }

    pub(super) fn inject_float_operator_impls(&mut self, ty: TypeId) {
        self.inject_eq_like_impls(ty);
        self.inject_binary_same_type_impls(ty, &["Add", "Sub", "Mul", "Div", "Rem"]);
        self.inject_unary_same_type_impl(ty, "Neg");
    }

    pub(super) fn inject_bool_operator_impls(&mut self) {
        self.inject_eq_like_impls(TypeId::BOOL);
        self.inject_unary_same_type_impl(TypeId::BOOL, "Not");
    }

    pub(super) fn inject_eq_like_impls(&mut self, ty: TypeId) {
        for spec in ["Eq", "Lt", "Le", "Gt", "Ge"] {
            let descriptor = BINARY_OPERATOR_TRAITS
                .iter()
                .find(|entry| entry.name == spec)
                .unwrap();
            self.inject_operator_impl(
                ty,
                descriptor.name,
                vec![ty],
                descriptor.method_name,
                vec![ty],
                TypeId::BOOL,
            );
        }
    }

    pub(super) fn inject_binary_same_type_impls(&mut self, ty: TypeId, trait_names: &[&str]) {
        for trait_name in trait_names {
            let descriptor = BINARY_OPERATOR_TRAITS
                .iter()
                .find(|entry| entry.name == *trait_name)
                .unwrap();
            self.inject_operator_impl(
                ty,
                descriptor.name,
                vec![ty],
                descriptor.method_name,
                vec![ty],
                ty,
            );
        }
    }

    pub(super) fn inject_shift_impls(&mut self, ty: TypeId) {
        for trait_name in ["Shl", "Shr"] {
            let descriptor = BINARY_OPERATOR_TRAITS
                .iter()
                .find(|entry| entry.name == trait_name)
                .unwrap();
            self.inject_operator_impl(
                ty,
                descriptor.name,
                vec![ty],
                descriptor.method_name,
                vec![ty],
                ty,
            );
        }
    }

    pub(super) fn inject_unary_same_type_impl(&mut self, ty: TypeId, trait_name: &str) {
        let descriptor = UNARY_OPERATOR_TRAITS
            .iter()
            .find(|entry| entry.name == trait_name)
            .unwrap();
        self.inject_operator_impl(
            ty,
            descriptor.name,
            vec![],
            descriptor.method_name,
            vec![],
            ty,
        );
    }

    // Inject `@sizeOf[T]() -> usize`.
}
