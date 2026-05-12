use super::*;

impl<'a, 'ctx> BuiltinInjector<'a, 'ctx> {
    pub(super) fn inject_size_of(&mut self) {
        let name_id = self.ctx.intern("@sizeOf");
        // Generic parameter `T` with no additional bounds.
        let param_t = self.new_builtin_param("T");

        // Build the semantic signature `fn[T]() -> usize`.
        let ret_type_id = self.ctx.next_node_id();
        let sig_ty = {
            let _ = self.ctx.type_registry.intern(TypeKind::Param(param_t.name));
            self.ctx.set_node_type(ret_type_id, TypeId::USIZE);
            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![],
                ret: TypeId::USIZE,
                is_variadic: false,
            })
        };

        let def_id = self.ctx.add_def_with(|def_id| {
            Def::Function(FunctionDef {
                id: def_id,
                name: name_id,
                name_span: Default::default(),
                vis: Visibility::Public,
                parent: None,
                default_trait_method: None,
                is_imported: false,
                generics: vec![param_t],
                where_clauses: vec![],
                params: vec![],
                ret_type: TypeNode {
                    id: ret_type_id,
                    span: Default::default(),
                    kind: ast::TypeKind::Infer,
                },
                body: None,
                is_const: false,
                is_extern: false,
                is_variadic: false,
                is_intrinsic: true,
                resolved_sig: Some(sig_ty),
                span: Default::default(),
                docs: None,
                attributes: vec![],
            })
        });

        let root_scope = ScopeId(0);
        self.ctx.scopes.set_current_scope(root_scope);
        let info = SymbolInfo {
            kind: SymbolKind::Function,
            node_id: self.ctx.next_node_id(),
            type_id: self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, vec![])),
            def_id: Some(def_id),
            span: Span::default(),
            vis: Visibility::Public, // All builtin intrinsics are globally visible.
            is_mut: false,
        };
        let _ = self.ctx.scopes.define(name_id, info);
    }

    // Inject `@alignOf[T]() -> usize`.
    pub(super) fn inject_align_of(&mut self) {
        let name_id = self.ctx.intern("@alignOf");
        let param_t = self.new_builtin_param("T");

        let ret_type_id = self.ctx.next_node_id();
        let sig_ty = {
            let _ = self.ctx.type_registry.intern(TypeKind::Param(param_t.name));
            self.ctx.set_node_type(ret_type_id, TypeId::USIZE);
            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![],
                ret: TypeId::USIZE,
                is_variadic: false,
            })
        };

        let def_id = self.ctx.add_def_with(|def_id| {
            Def::Function(FunctionDef {
                id: def_id,
                name: name_id,
                name_span: Default::default(),
                vis: Visibility::Public,
                parent: None,
                default_trait_method: None,
                is_imported: false,
                generics: vec![param_t],
                where_clauses: vec![],
                params: vec![],
                ret_type: TypeNode {
                    id: ret_type_id,
                    span: Default::default(),
                    kind: ast::TypeKind::Infer,
                },
                body: None,
                is_const: false,
                is_extern: false,
                is_variadic: false,
                is_intrinsic: true,
                resolved_sig: Some(sig_ty),
                span: Default::default(),
                docs: None,
                attributes: vec![],
            })
        });
        let root_scope = ScopeId(0);
        self.ctx.scopes.set_current_scope(root_scope);
        let info = SymbolInfo {
            kind: SymbolKind::Function,
            node_id: self.ctx.next_node_id(),
            type_id: self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, vec![])),
            def_id: Some(def_id),
            span: Default::default(),
            vis: Visibility::Public,
            is_mut: false,
        };
        let _ = self.ctx.scopes.define(name_id, info);
    }

    // Inject `@unreachable() -> !`.
    pub(super) fn inject_unreachable(&mut self) {
        let name_id = self.ctx.intern("@unreachable");
        let ret_id = self.ctx.next_node_id();
        let sig_ty = {
            self.ctx.set_node_type(ret_id, TypeId::NEVER);
            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![],
                ret: TypeId::NEVER,
                is_variadic: false,
            })
        };

        let def_id = self.ctx.add_def_with(|def_id| {
            Def::Function(FunctionDef {
                id: def_id,
                name: name_id,
                name_span: Default::default(),
                vis: Visibility::Public,
                parent: None,
                default_trait_method: None,
                is_imported: false,
                generics: vec![],
                where_clauses: vec![],
                params: vec![],
                ret_type: TypeNode {
                    id: ret_id,
                    span: Default::default(),
                    kind: ast::TypeKind::Never, // Map directly to the semantic `Never` type.
                },
                body: None,
                is_const: false,
                is_extern: false,
                is_variadic: false,
                is_intrinsic: true,
                resolved_sig: Some(sig_ty),
                span: Default::default(),
                docs: None,
                attributes: vec![],
            })
        });
        let root_scope = ScopeId(0);
        self.ctx.scopes.set_current_scope(root_scope);
        let info = SymbolInfo {
            kind: SymbolKind::Function,
            node_id: self.ctx.next_node_id(),
            type_id: self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, vec![])),
            def_id: Some(def_id),
            span: Default::default(),
            vis: Visibility::Public,
            is_mut: false,
        };
        let _ = self.ctx.scopes.define(name_id, info);
    }

    pub(super) fn inject_bitwise(&mut self, name: &str, int_trait_id: DefId) {
        let name_id = self.ctx.intern(name);
        let trait_node = ast::TypeNode {
            id: self.ctx.next_node_id(),
            span: Default::default(),
            kind: ast::TypeKind::Infer,
        };
        let trait_ty = self.builtin_trait_ty_by_id(int_trait_id, vec![]);
        self.ctx.set_node_type(trait_node.id, trait_ty);

        let param_t = self.new_builtin_param("T");

        let target_node = ast::TypeNode {
            id: self.ctx.next_node_id(),
            span: Default::default(),
            kind: ast::TypeKind::Infer,
        };
        let val_param_id = self.ctx.next_node_id();
        let ret_id = self.ctx.next_node_id();
        let val_name = self.ctx.intern("val");

        let sig_ty = {
            let t_ty = self.ctx.type_registry.intern(TypeKind::Param(param_t.name));
            self.ctx.set_node_type(target_node.id, t_ty);
            self.ctx.set_node_type(val_param_id, t_ty);
            self.ctx.set_node_type(ret_id, t_ty);
            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![t_ty],
                ret: t_ty,
                is_variadic: false,
            })
        };

        let def_id = self.ctx.add_def_with(|def_id| {
            Def::Function(FunctionDef {
                id: def_id,
                name: name_id,
                name_span: Default::default(),
                vis: Visibility::Public,
                parent: None,
                default_trait_method: None,
                is_imported: false,
                generics: vec![param_t],
                where_clauses: vec![ast::WhereClause {
                    span: Default::default(),
                    target_ty: target_node,
                    bounds: vec![trait_node],
                }],
                params: vec![ast::FuncParam {
                    pattern: ast::BindingPattern {
                        name: val_name,
                        name_span: Default::default(),
                        is_mut: false,
                        span: Default::default(),
                    },
                    type_node: ast::TypeNode {
                        id: val_param_id,
                        span: Default::default(),
                        kind: ast::TypeKind::Infer,
                    },
                    span: Default::default(),
                }],
                ret_type: ast::TypeNode {
                    id: ret_id,
                    span: Default::default(),
                    kind: ast::TypeKind::Infer,
                },
                body: None,
                is_const: false,
                is_extern: false,
                is_variadic: false,
                is_intrinsic: true,
                resolved_sig: Some(sig_ty),
                span: Default::default(),
                docs: None,
                attributes: vec![],
            })
        });
        let root_scope = ScopeId(0);
        self.ctx.scopes.set_current_scope(root_scope);
        let info = SymbolInfo {
            kind: SymbolKind::Function,
            node_id: self.ctx.next_node_id(),
            type_id: self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, vec![])),
            def_id: Some(def_id),
            span: Default::default(),
            vis: Visibility::Public,
            is_mut: false,
        };
        let _ = self.ctx.scopes.define(name_id, info);
    }

    // Inject zero-argument hardware-style intrinsics.
    pub(super) fn inject_void_intrinsic(&mut self, name: &str, is_divergent: bool) {
        let name_id = self.ctx.intern(name);
        let ret_id = self.ctx.next_node_id();

        let ret_type = if is_divergent {
            TypeId::NEVER
        } else {
            TypeId::VOID
        };
        let ast_ret_kind = if is_divergent {
            ast::TypeKind::Never
        } else {
            ast::TypeKind::Infer
        }; // `void` has no dedicated AST node, so `Infer` hits the cached semantic type.

        let sig_ty = {
            self.ctx.set_node_type(ret_id, ret_type);
            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![],
                ret: ret_type,
                is_variadic: false,
            })
        };

        let def_id = self.ctx.add_def_with(|def_id| {
            Def::Function(FunctionDef {
                id: def_id,
                name: name_id,
                name_span: Default::default(),
                vis: Visibility::Public,
                parent: None,
                default_trait_method: None,
                is_imported: false,
                generics: vec![],
                where_clauses: vec![],
                params: vec![],
                ret_type: ast::TypeNode {
                    id: ret_id,
                    span: Default::default(),
                    kind: ast_ret_kind,
                },
                body: None,
                is_const: false,
                is_extern: false,
                is_variadic: false,
                is_intrinsic: true,
                resolved_sig: Some(sig_ty),
                span: Default::default(),
                docs: None,
                attributes: vec![],
            })
        });
        let root_scope = ScopeId(0);
        self.ctx.scopes.set_current_scope(root_scope);
        let info = SymbolInfo {
            kind: SymbolKind::Function,
            node_id: self.ctx.next_node_id(),
            type_id: self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, vec![])),
            def_id: Some(def_id),
            span: Default::default(),
            vis: Visibility::Public,
            is_mut: false,
        };
        let _ = self.ctx.scopes.define(name_id, info);
    }

    pub(super) fn inject_memory_intrinsic(&mut self, kind: MemoryIntrinsicKind) {
        let name = kind.name();
        let name_id = self.ctx.intern(name);
        // Shared memory intrinsic parameter types: dest, src/val, and len.
        let ptr_mut_u8 = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: true,
            elem: TypeId::U8,
        });
        let ptr_u8 = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::U8,
        });

        let param_dest_id = self.ctx.next_node_id();
        let param_src_val_id = self.ctx.next_node_id();
        let param_len_id = self.ctx.next_node_id();
        let ret_id = self.ctx.next_node_id();
        let dest_name = self.ctx.intern("dest");
        let src_or_value_name = self.ctx.intern(kind.src_or_value_name());
        let len_name = self.ctx.intern("len");

        let sig_ty = {
            self.ctx.set_node_type(param_dest_id, ptr_mut_u8);
            self.ctx
                .set_node_type(param_src_val_id, kind.src_or_value_type(ptr_u8));
            self.ctx.set_node_type(param_len_id, TypeId::USIZE);
            self.ctx.set_node_type(ret_id, TypeId::VOID);

            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![ptr_mut_u8, kind.src_or_value_type(ptr_u8), TypeId::USIZE],
                ret: TypeId::VOID,
                is_variadic: false,
            })
        };

        let def_id = self.ctx.add_def_with(|def_id| {
            Def::Function(FunctionDef {
                id: def_id,
                name: name_id,
                name_span: Default::default(),
                vis: Visibility::Public,
                parent: None,
                default_trait_method: None,
                is_imported: false,
                generics: vec![], // Memory intrinsics always operate on raw bytes.
                where_clauses: vec![],
                params: vec![
                    ast::FuncParam {
                        pattern: ast::BindingPattern {
                            name: dest_name,
                            name_span: Default::default(),
                            is_mut: false,
                            span: Default::default(),
                        },
                        type_node: ast::TypeNode {
                            id: param_dest_id,
                            span: Default::default(),
                            kind: ast::TypeKind::Infer,
                        },
                        span: Default::default(),
                    },
                    ast::FuncParam {
                        pattern: ast::BindingPattern {
                            name: src_or_value_name,
                            name_span: Default::default(),
                            is_mut: false,
                            span: Default::default(),
                        },
                        type_node: ast::TypeNode {
                            id: param_src_val_id,
                            span: Default::default(),
                            kind: ast::TypeKind::Infer,
                        },
                        span: Default::default(),
                    },
                    ast::FuncParam {
                        pattern: ast::BindingPattern {
                            name: len_name,
                            name_span: Default::default(),
                            is_mut: false,
                            span: Default::default(),
                        },
                        type_node: ast::TypeNode {
                            id: param_len_id,
                            span: Default::default(),
                            kind: ast::TypeKind::Infer,
                        },
                        span: Default::default(),
                    },
                ],
                ret_type: ast::TypeNode {
                    id: ret_id,
                    span: Default::default(),
                    kind: ast::TypeKind::Infer,
                },
                body: None,
                is_const: false,
                is_extern: false,
                is_variadic: false,
                is_intrinsic: true,
                resolved_sig: Some(sig_ty),
                span: Default::default(),
                docs: None,
                attributes: vec![],
            })
        });
        let node_id = self.ctx.next_node_id();
        let _ = self.ctx.scopes.define(
            name_id,
            SymbolInfo {
                kind: SymbolKind::Function,
                node_id,
                type_id: self
                    .ctx
                    .type_registry
                    .intern(TypeKind::FnDef(def_id, vec![])),
                def_id: Some(def_id),
                span: Default::default(),
                vis: Visibility::Public,
                is_mut: false,
            },
        );
    }

    pub(super) fn inject_atomic_load(&mut self) {
        let param_t = self.new_builtin_param("T");
        let t_ty = self.ctx.type_registry.intern(TypeKind::Param(param_t.name));
        let ptr_t = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: t_ty,
        });
        self.inject_builtin_function(
            "@atomicLoad",
            vec![param_t],
            vec![("ptr", ptr_t), ("order", TypeId::U8)],
            t_ty,
        );
    }

    pub(super) fn inject_atomic_store(&mut self) {
        let param_t = self.new_builtin_param("T");
        let t_ty = self.ctx.type_registry.intern(TypeKind::Param(param_t.name));
        let ptr_t = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: true,
            elem: t_ty,
        });
        self.inject_builtin_function(
            "@atomicStore",
            vec![param_t],
            vec![("ptr", ptr_t), ("val", t_ty), ("order", TypeId::U8)],
            TypeId::VOID,
        );
    }

    pub(super) fn inject_atomic_cas(&mut self, name: &str) {
        let param_t = self.new_builtin_param("T");
        let t_ty = self.ctx.type_registry.intern(TypeKind::Param(param_t.name));
        let ptr_t = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: true,
            elem: t_ty,
        });
        let success_name = self.ctx.intern("success");
        let value_name = self.ctx.intern("value");
        let ret_ty = self.ctx.type_registry.intern(TypeKind::AnonymousStruct(
            false,
            vec![
                crate::ty::AnonymousField {
                    name: success_name,
                    ty: TypeId::BOOL,
                },
                crate::ty::AnonymousField {
                    name: value_name,
                    ty: t_ty,
                },
            ],
        ));

        self.inject_builtin_function(
            name,
            vec![param_t],
            vec![
                ("ptr", ptr_t),
                ("expected", t_ty),
                ("desired", t_ty),
                ("succ", TypeId::U8),
                ("fail", TypeId::U8),
            ],
            ret_ty,
        );
    }

    pub(super) fn inject_atomic_xchg(&mut self) {
        let param_t = self.new_builtin_param("T");
        let t_ty = self.ctx.type_registry.intern(TypeKind::Param(param_t.name));
        let ptr_t = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: true,
            elem: t_ty,
        });
        self.inject_builtin_function(
            "@atomicXchg",
            vec![param_t],
            vec![("ptr", ptr_t), ("val", t_ty), ("order", TypeId::U8)],
            t_ty,
        );
    }

    pub(super) fn inject_loc(&mut self) {
        // `@loc` has a call-site result type because `file` is `[N]u8`.
        self.inject_builtin_function("@loc", vec![], vec![], TypeId::VOID);
    }

    pub(super) fn inject_check(&mut self) {
        let param_t = self.new_builtin_param("T");
        let t_ty = self.ctx.type_registry.intern(TypeKind::Param(param_t.name));
        // `@check` has a call-site result type because `source` is `[N]u8`.
        self.inject_builtin_function("@check", vec![param_t], vec![("value", t_ty)], TypeId::VOID);
    }

    pub(super) fn inject_atomic_rmw(&mut self, name: &str) {
        let param_t = self.new_builtin_param("T");
        let t_ty = self.ctx.type_registry.intern(TypeKind::Param(param_t.name));
        let ptr_t = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: true,
            elem: t_ty,
        });
        self.inject_builtin_function(
            name,
            vec![param_t],
            vec![("ptr", ptr_t), ("val", t_ty), ("order", TypeId::U8)],
            t_ty,
        );
    }

    pub(super) fn inject_atomic_fence(&mut self) {
        self.inject_builtin_function("@fence", vec![], vec![("order", TypeId::U8)], TypeId::VOID);
    }

    pub(super) fn inject_simd_any(&mut self) {
        let param_mask = self.new_builtin_param("Mask");
        let mask_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_mask.name));
        self.inject_builtin_function(
            "@simdAny",
            vec![param_mask],
            vec![("mask", mask_ty)],
            TypeId::BOOL,
        );
    }

    pub(super) fn inject_simd_all(&mut self) {
        let param_mask = self.new_builtin_param("Mask");
        let mask_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_mask.name));
        self.inject_builtin_function(
            "@simdAll",
            vec![param_mask],
            vec![("mask", mask_ty)],
            TypeId::BOOL,
        );
    }

    pub(super) fn inject_simd_bitmask(&mut self) {
        let param_mask = self.new_builtin_param("Mask");
        let mask_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_mask.name));
        self.inject_builtin_function(
            "@simdBitmask",
            vec![param_mask],
            vec![("mask", mask_ty)],
            TypeId::USIZE,
        );
    }

    pub(super) fn inject_simd_select(&mut self) {
        let param_mask = self.new_builtin_param("Mask");
        let param_value = self.new_builtin_param("Value");
        let mask_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_mask.name));
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        self.inject_builtin_function(
            "@simdSelect",
            vec![param_mask, param_value],
            vec![
                ("mask", mask_ty),
                ("on_true", value_ty),
                ("on_false", value_ty),
            ],
            value_ty,
        );
    }

    pub(super) fn inject_simd_shuffle(&mut self) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        let index_slice = self.ctx.type_registry.intern(TypeKind::Slice {
            is_mut: false,
            elem: TypeId::U32,
        });
        self.inject_builtin_function(
            "@simdShuffle",
            vec![param_value],
            vec![
                ("lhs", value_ty),
                ("rhs", value_ty),
                ("indices", index_slice),
            ],
            value_ty,
        );
    }

    pub(super) fn inject_simd_swizzle(&mut self) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        let index_slice = self.ctx.type_registry.intern(TypeKind::Slice {
            is_mut: false,
            elem: TypeId::U32,
        });
        self.inject_builtin_function(
            "@simdSwizzle",
            vec![param_value],
            vec![("value", value_ty), ("indices", index_slice)],
            value_ty,
        );
    }

    pub(super) fn inject_simd_reduce(&mut self, name: &str) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        self.inject_builtin_function(name, vec![param_value], vec![("value", value_ty)], value_ty);
    }

    pub(super) fn inject_simd_extract_half(&mut self, name: &str) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        self.inject_builtin_function(
            name,
            vec![param_value],
            vec![("value", TypeId::BOOL)],
            value_ty,
        );
    }

    pub(super) fn inject_simd_insert_half(&mut self, name: &str) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        self.inject_builtin_function(
            name,
            vec![param_value],
            vec![("base", value_ty), ("half", TypeId::BOOL)],
            value_ty,
        );
    }

    pub(super) fn inject_simd_permute_unary(&mut self, name: &str) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        self.inject_builtin_function(name, vec![param_value], vec![("value", value_ty)], value_ty);
    }

    pub(super) fn inject_simd_rotate(&mut self, name: &str) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        self.inject_builtin_function(
            name,
            vec![param_value],
            vec![("value", value_ty), ("amount", TypeId::USIZE)],
            value_ty,
        );
    }

    pub(super) fn inject_simd_abs(&mut self) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        self.inject_builtin_function(
            "@simdAbs",
            vec![param_value],
            vec![("value", value_ty)],
            value_ty,
        );
    }

    pub(super) fn inject_simd_float_unary(&mut self, name: &str) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        self.inject_builtin_function(name, vec![param_value], vec![("value", value_ty)], value_ty);
    }

    pub(super) fn inject_simd_pairwise(&mut self, name: &str) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        self.inject_builtin_function(
            name,
            vec![param_value],
            vec![("lhs", value_ty), ("rhs", value_ty)],
            value_ty,
        );
    }

    pub(super) fn inject_simd_clamp(&mut self) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        self.inject_builtin_function(
            "@simdClamp",
            vec![param_value],
            vec![("value", value_ty), ("lo", value_ty), ("hi", value_ty)],
            value_ty,
        );
    }

    pub(super) fn inject_simd_splat(&mut self) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        self.inject_builtin_function(
            "@simdSplat",
            vec![param_value],
            vec![("value", TypeId::BOOL)],
            value_ty,
        );
    }

    pub(super) fn inject_simd_cast(&mut self) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        self.inject_builtin_function(
            "@simdCast",
            vec![param_value],
            vec![("value", TypeId::BOOL)],
            value_ty,
        );
    }

    pub(super) fn inject_simd_bitcast(&mut self) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        self.inject_builtin_function(
            "@simdBitcast",
            vec![param_value],
            vec![("value", TypeId::BOOL)],
            value_ty,
        );
    }

    pub(super) fn inject_simd_load(&mut self) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        let raw_ptr = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::U8,
        });
        self.inject_builtin_function(
            "@simdLoad",
            vec![param_value],
            vec![("ptr", raw_ptr), ("align", TypeId::USIZE)],
            value_ty,
        );
    }

    pub(super) fn inject_simd_store(&mut self) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        let raw_ptr = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: true,
            elem: TypeId::U8,
        });
        self.inject_builtin_function(
            "@simdStore",
            vec![param_value],
            vec![
                ("ptr", raw_ptr),
                ("value", value_ty),
                ("align", TypeId::USIZE),
            ],
            TypeId::VOID,
        );
    }

    pub(super) fn inject_simd_masked_load(&mut self) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        let raw_ptr = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::U8,
        });
        self.inject_builtin_function(
            "@simdMaskedLoad",
            vec![param_value],
            vec![
                ("ptr", raw_ptr),
                ("mask", TypeId::BOOL),
                ("or_else", value_ty),
                ("align", TypeId::USIZE),
            ],
            value_ty,
        );
    }

    pub(super) fn inject_simd_masked_store(&mut self) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        let raw_ptr = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: true,
            elem: TypeId::U8,
        });
        self.inject_builtin_function(
            "@simdMaskedStore",
            vec![param_value],
            vec![
                ("ptr", raw_ptr),
                ("mask", TypeId::BOOL),
                ("value", value_ty),
                ("align", TypeId::USIZE),
            ],
            TypeId::VOID,
        );
    }

    pub(super) fn inject_simd_gather(&mut self) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        let raw_ptr = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::U8,
        });
        let index_ptr = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::USIZE,
        });
        self.inject_builtin_function(
            "@simdGather",
            vec![param_value],
            vec![("ptr", raw_ptr), ("indices", index_ptr)],
            value_ty,
        );
    }

    pub(super) fn inject_simd_scatter(&mut self) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        let raw_ptr = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: true,
            elem: TypeId::U8,
        });
        let index_ptr = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::USIZE,
        });
        self.inject_builtin_function(
            "@simdScatter",
            vec![param_value],
            vec![
                ("ptr", raw_ptr),
                ("indices", index_ptr),
                ("value", value_ty),
            ],
            TypeId::VOID,
        );
    }

    pub(super) fn inject_simd_masked_gather(&mut self) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        let raw_ptr = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::U8,
        });
        let index_ptr = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::USIZE,
        });
        self.inject_builtin_function(
            "@simdMaskedGather",
            vec![param_value],
            vec![
                ("ptr", raw_ptr),
                ("indices", index_ptr),
                ("mask", TypeId::BOOL),
                ("or_else", value_ty),
            ],
            value_ty,
        );
    }

    pub(super) fn inject_simd_masked_scatter(&mut self) {
        let param_value = self.new_builtin_param("Value");
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        let raw_ptr = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: true,
            elem: TypeId::U8,
        });
        let index_ptr = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::USIZE,
        });
        self.inject_builtin_function(
            "@simdMaskedScatter",
            vec![param_value],
            vec![
                ("ptr", raw_ptr),
                ("indices", index_ptr),
                ("mask", TypeId::BOOL),
                ("value", value_ty),
            ],
            TypeId::VOID,
        );
    }

    pub(super) fn inject_builtin_function(
        &mut self,
        name: &str,
        generics: Vec<GenericParam>,
        params: Vec<(&str, TypeId)>,
        ret_ty: TypeId,
    ) {
        let name_id = self.ctx.intern(name);
        let mut param_defs = Vec::with_capacity(params.len());
        let mut param_tys = Vec::with_capacity(params.len());

        for (param_name, ty) in params {
            let type_node_id = self.ctx.next_node_id();
            self.ctx.set_node_type(type_node_id, ty);
            param_tys.push(ty);
            param_defs.push(ast::FuncParam {
                pattern: ast::BindingPattern {
                    name: self.ctx.intern(param_name),
                    name_span: Default::default(),
                    is_mut: false,
                    span: Default::default(),
                },
                type_node: ast::TypeNode {
                    id: type_node_id,
                    span: Default::default(),
                    kind: ast::TypeKind::Infer,
                },
                span: Default::default(),
            });
        }

        let ret_id = self.ctx.next_node_id();
        self.ctx.set_node_type(ret_id, ret_ty);
        let sig_ty = self.ctx.type_registry.intern(TypeKind::Function {
            params: param_tys,
            ret: ret_ty,
            is_variadic: false,
        });

        let ret_kind = if ret_ty == TypeId::NEVER {
            ast::TypeKind::Never
        } else {
            ast::TypeKind::Infer
        };

        let def_id = self.ctx.add_def_with(|def_id| {
            Def::Function(FunctionDef {
                id: def_id,
                name: name_id,
                name_span: Default::default(),
                vis: Visibility::Public,
                parent: None,
                default_trait_method: None,
                is_imported: false,
                generics,
                where_clauses: vec![],
                params: param_defs,
                ret_type: ast::TypeNode {
                    id: ret_id,
                    span: Default::default(),
                    kind: ret_kind,
                },
                body: None,
                is_const: false,
                is_extern: false,
                is_variadic: false,
                is_intrinsic: true,
                resolved_sig: Some(sig_ty),
                span: Default::default(),
                docs: None,
                attributes: vec![],
            })
        });
        self.ctx.scopes.set_current_scope(ScopeId(0));
        let node_id = self.ctx.next_node_id();
        let _ = self.ctx.scopes.define(
            name_id,
            SymbolInfo {
                kind: SymbolKind::Function,
                node_id,
                type_id: self
                    .ctx
                    .type_registry
                    .intern(TypeKind::FnDef(def_id, vec![])),
                def_id: Some(def_id),
                span: Default::default(),
                vis: Visibility::Public,
                is_mut: false,
            },
        );
    }
}
