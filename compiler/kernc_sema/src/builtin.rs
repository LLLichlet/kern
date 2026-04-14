use crate::SemaContext;
use crate::def::*;
use crate::scope::{ScopeId, SymbolInfo, SymbolKind};
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{self as ast, GenericParam, TypeNode};
use kernc_utils::Span;

struct BuiltinMethodSpec<'a> {
    name: &'a str,
    params: Vec<TypeId>,
    ret: TypeId,
}

struct BuiltinTraitSpec<'a> {
    name: &'a str,
    generics: Vec<GenericParam>,
    supertraits: Vec<TypeId>,
    methods: Vec<BuiltinMethodSpec<'a>>,
}

struct BuiltinOperatorTrait<'a> {
    name: &'a str,
    method_name: &'a str,
}

#[derive(Clone, Copy)]
enum MemoryIntrinsicKind {
    Memcpy,
    Memmove,
    Memset,
}

impl MemoryIntrinsicKind {
    fn name(self) -> &'static str {
        match self {
            Self::Memcpy => "@memcpy",
            Self::Memmove => "@memmove",
            Self::Memset => "@memset",
        }
    }

    fn src_or_value_type(self, ptr_u8: TypeId) -> TypeId {
        match self {
            Self::Memcpy | Self::Memmove => ptr_u8,
            Self::Memset => TypeId::U8,
        }
    }

    fn src_or_value_name(self) -> &'static str {
        match self {
            Self::Memcpy | Self::Memmove => "src",
            Self::Memset => "val",
        }
    }
}

const BINARY_OPERATOR_TRAITS: &[BuiltinOperatorTrait<'_>] = &[
    BuiltinOperatorTrait {
        name: "Eq",
        method_name: "eq",
    },
    BuiltinOperatorTrait {
        name: "Lt",
        method_name: "lt",
    },
    BuiltinOperatorTrait {
        name: "Le",
        method_name: "le",
    },
    BuiltinOperatorTrait {
        name: "Gt",
        method_name: "gt",
    },
    BuiltinOperatorTrait {
        name: "Ge",
        method_name: "ge",
    },
    BuiltinOperatorTrait {
        name: "Add",
        method_name: "add",
    },
    BuiltinOperatorTrait {
        name: "Sub",
        method_name: "sub",
    },
    BuiltinOperatorTrait {
        name: "Mul",
        method_name: "mul",
    },
    BuiltinOperatorTrait {
        name: "Div",
        method_name: "div",
    },
    BuiltinOperatorTrait {
        name: "Rem",
        method_name: "rem",
    },
    BuiltinOperatorTrait {
        name: "BitAnd",
        method_name: "bit_and",
    },
    BuiltinOperatorTrait {
        name: "BitOr",
        method_name: "bit_or",
    },
    BuiltinOperatorTrait {
        name: "BitXor",
        method_name: "bit_xor",
    },
    BuiltinOperatorTrait {
        name: "Shl",
        method_name: "shl",
    },
    BuiltinOperatorTrait {
        name: "Shr",
        method_name: "shr",
    },
];

const UNARY_OPERATOR_TRAITS: &[BuiltinOperatorTrait<'_>] = &[
    BuiltinOperatorTrait {
        name: "Neg",
        method_name: "neg",
    },
    BuiltinOperatorTrait {
        name: "BitNot",
        method_name: "bit_not",
    },
    BuiltinOperatorTrait {
        name: "Not",
        method_name: "not",
    },
];

pub struct BuiltinInjector<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
}

impl<'a, 'ctx> BuiltinInjector<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self { ctx }
    }

    pub fn inject(&mut self) {
        // 1. Register builtin marker traits and operator traits owned by the language.
        let int_trait_id = self.inject_builtin_trait(BuiltinTraitSpec {
            name: "Integer",
            generics: vec![],
            supertraits: vec![],
            methods: vec![],
        });
        let int_trait_ty = self.builtin_trait_ty_by_id(int_trait_id, vec![]);
        let signed_int_trait_id = self.inject_builtin_trait(BuiltinTraitSpec {
            name: "SignedInteger",
            generics: vec![],
            supertraits: vec![int_trait_ty],
            methods: vec![],
        });
        let unsigned_int_trait_id = self.inject_builtin_trait(BuiltinTraitSpec {
            name: "UnsignedInteger",
            generics: vec![],
            supertraits: vec![int_trait_ty],
            methods: vec![],
        });
        let float_trait_id = self.inject_builtin_trait(BuiltinTraitSpec {
            name: "Float",
            generics: vec![],
            supertraits: vec![],
            methods: vec![],
        });
        self.inject_operator_traits();

        // 2. Inject builtin impls for primitive types.
        let signed_int_types = [
            TypeId::I8,
            TypeId::I16,
            TypeId::I32,
            TypeId::I64,
            TypeId::I128,
            TypeId::ISIZE,
        ];
        let unsigned_int_types = [
            TypeId::U8,
            TypeId::U16,
            TypeId::U32,
            TypeId::U64,
            TypeId::U128,
            TypeId::USIZE,
        ];
        for &ty in &signed_int_types {
            self.inject_primitive_impl(ty, int_trait_id);
            self.inject_primitive_impl(ty, signed_int_trait_id);
            self.inject_integer_operator_impls(ty);
        }
        for &ty in &unsigned_int_types {
            self.inject_primitive_impl(ty, int_trait_id);
            self.inject_primitive_impl(ty, unsigned_int_trait_id);
            self.inject_integer_operator_impls(ty);
        }

        let float_types = [TypeId::F32, TypeId::F64];
        for &ty in &float_types {
            self.inject_primitive_impl(ty, float_trait_id);
            self.inject_float_operator_impls(ty);
        }
        self.inject_bool_operator_impls();

        // 3. Register builtin intrinsic functions.
        self.inject_size_of();
        self.inject_align_of();
        self.inject_unreachable();
        self.inject_bitwise("@popCount", int_trait_id);
        self.inject_bitwise("@clz", int_trait_id);
        self.inject_bitwise("@ctz", int_trait_id);
        self.inject_bitwise("@bswap", int_trait_id);
        self.inject_void_intrinsic("@trap", true);
        self.inject_void_intrinsic("@breakpoint", false);
        self.inject_memory_intrinsic(MemoryIntrinsicKind::Memcpy);
        self.inject_memory_intrinsic(MemoryIntrinsicKind::Memmove);
        self.inject_memory_intrinsic(MemoryIntrinsicKind::Memset);
        self.inject_atomic_load();
        self.inject_atomic_store();
        self.inject_atomic_cas("@atomicCas");
        self.inject_atomic_cas("@atomicCasWeak");
        self.inject_atomic_xchg();
        self.inject_atomic_rmw("@atomicRmwAdd");
        self.inject_atomic_rmw("@atomicRmwSub");
        self.inject_atomic_rmw("@atomicRmwAnd");
        self.inject_atomic_rmw("@atomicRmwNand");
        self.inject_atomic_rmw("@atomicRmwOr");
        self.inject_atomic_rmw("@atomicRmwXor");
        self.inject_atomic_rmw("@atomicRmwMax");
        self.inject_atomic_rmw("@atomicRmwMin");
        self.inject_atomic_rmw("@atomicRmwUMax");
        self.inject_atomic_rmw("@atomicRmwUMin");
        self.inject_atomic_fence();
        self.inject_simd_any();
        self.inject_simd_all();
        self.inject_simd_bitmask();
        self.inject_simd_select();
        self.inject_simd_shuffle();
        self.inject_simd_swizzle();
        self.inject_simd_permute_unary("@simdReverse");
        self.inject_simd_rotate("@simdRotateLeft");
        self.inject_simd_rotate("@simdRotateRight");
        self.inject_simd_pairwise("@simdInterleaveLo");
        self.inject_simd_pairwise("@simdInterleaveHi");
        self.inject_simd_pairwise("@simdZipLo");
        self.inject_simd_pairwise("@simdZipHi");
        self.inject_simd_pairwise("@simdConcatLo");
        self.inject_simd_pairwise("@simdConcatHi");
        self.inject_simd_pairwise("@simdDeinterleaveLo");
        self.inject_simd_pairwise("@simdDeinterleaveHi");
        self.inject_simd_pairwise("@simdUnzipLo");
        self.inject_simd_pairwise("@simdUnzipHi");
        self.inject_simd_extract_half("@simdLowHalf");
        self.inject_simd_extract_half("@simdHighHalf");
        self.inject_simd_insert_half("@simdWithLowHalf");
        self.inject_simd_insert_half("@simdWithHighHalf");
        self.inject_simd_reduce("@simdReduceAdd");
        self.inject_simd_reduce("@simdReduceMul");
        self.inject_simd_reduce("@simdReduceAnd");
        self.inject_simd_reduce("@simdReduceOr");
        self.inject_simd_reduce("@simdReduceXor");
        self.inject_simd_reduce("@simdReduceMin");
        self.inject_simd_reduce("@simdReduceMax");
        self.inject_simd_abs();
        self.inject_simd_float_unary("@simdSqrt");
        self.inject_simd_float_unary("@simdFloor");
        self.inject_simd_float_unary("@simdCeil");
        self.inject_simd_float_unary("@simdTrunc");
        self.inject_simd_float_unary("@simdRound");
        self.inject_simd_pairwise("@simdMin");
        self.inject_simd_pairwise("@simdMax");
        self.inject_simd_clamp();
        self.inject_simd_splat();
        self.inject_simd_cast();
        self.inject_simd_bitcast();
        self.inject_simd_load();
        self.inject_simd_store();
        self.inject_simd_masked_load();
        self.inject_simd_masked_store();
        self.inject_simd_gather();
        self.inject_simd_scatter();
        self.inject_simd_masked_gather();
        self.inject_simd_masked_scatter();
        self.inject_custom_define_consts();
    }

    // ==========================================
    // Injection helpers
    // ==========================================

    fn new_builtin_param(&mut self, name: &str) -> GenericParam {
        GenericParam {
            name: self.ctx.intern(name),
            span: Span::default(),
        }
    }

    fn builtin_trait_ty_by_id(&mut self, trait_def_id: DefId, args: Vec<TypeId>) -> TypeId {
        self.ctx
            .type_registry
            .intern(TypeKind::TraitObject(trait_def_id, args, Vec::new()))
    }

    fn inject_builtin_trait(&mut self, spec: BuiltinTraitSpec<'_>) -> DefId {
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
            is_pub: true,
            is_mut: false,
        };
        let root_scope = ScopeId(0);
        self.ctx.scopes.set_current_scope(root_scope);
        let _ = self.ctx.scopes.define(name_id, info);

        def_id
    }

    fn inject_binary_operator_trait_with_assoc_out(&mut self, trait_name: &str, method_name: &str) {
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
            is_pub: true,
            is_mut: false,
        };
        let root_scope = ScopeId(0);
        self.ctx.scopes.set_current_scope(root_scope);
        let _ = self.ctx.scopes.define(name_id, info);
    }

    fn inject_unary_operator_trait_with_assoc_out(&mut self, trait_name: &str, method_name: &str) {
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
            is_pub: true,
            is_mut: false,
        };
        let root_scope = ScopeId(0);
        self.ctx.scopes.set_current_scope(root_scope);
        let _ = self.ctx.scopes.define(name_id, info);
    }

    fn inject_operator_traits(&mut self) {
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

    fn inject_primitive_impl(&mut self, target_ty_id: TypeId, trait_def_id: DefId) {
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

    fn inject_operator_impl(
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
            trait_args,
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

    fn inject_integer_operator_impls(&mut self, ty: TypeId) {
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

    fn inject_float_operator_impls(&mut self, ty: TypeId) {
        self.inject_eq_like_impls(ty);
        self.inject_binary_same_type_impls(ty, &["Add", "Sub", "Mul", "Div", "Rem"]);
        self.inject_unary_same_type_impl(ty, "Neg");
    }

    fn inject_bool_operator_impls(&mut self) {
        self.inject_eq_like_impls(TypeId::BOOL);
        self.inject_unary_same_type_impl(TypeId::BOOL, "Not");
    }

    fn inject_eq_like_impls(&mut self, ty: TypeId) {
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

    fn inject_binary_same_type_impls(&mut self, ty: TypeId, trait_names: &[&str]) {
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

    fn inject_shift_impls(&mut self, ty: TypeId) {
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

    fn inject_unary_same_type_impl(&mut self, ty: TypeId, trait_name: &str) {
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
    fn inject_size_of(&mut self) {
        let name_id = self.ctx.intern("@sizeOf");
        let def_id = DefId(self.ctx.defs.len() as u32);

        // Generic parameter `T` with no additional bounds.
        let param_t = GenericParam {
            name: self.ctx.intern("T"),
            span: Default::default(),
        };

        // Build the semantic signature `fn[T]() -> usize`.
        let ret_type_id = self.ctx.next_node_id();
        let sig_ty = {
            let _ = self.ctx.type_registry.intern(TypeKind::Param(param_t.name));
            self.ctx.node_types.insert(ret_type_id, TypeId::USIZE);
            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![],
                ret: TypeId::USIZE,
                is_variadic: false,
            })
        };

        let func_def = FunctionDef {
            id: def_id,
            name: name_id,
            name_span: Default::default(),
            vis: Visibility::Public,
            parent: None,
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
        };

        self.ctx.add_def(Def::Function(func_def));

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
            is_pub: true, // All builtin intrinsics are globally visible.
            is_mut: false,
        };
        let _ = self.ctx.scopes.define(name_id, info);
    }

    // Inject `@alignOf[T]() -> usize`.
    fn inject_align_of(&mut self) {
        let name_id = self.ctx.intern("@alignOf");
        let def_id = DefId(self.ctx.defs.len() as u32);

        let param_t = GenericParam {
            name: self.ctx.intern("T"),
            span: Default::default(),
        };

        let ret_type_id = self.ctx.next_node_id();
        let sig_ty = {
            let _ = self.ctx.type_registry.intern(TypeKind::Param(param_t.name));
            self.ctx.node_types.insert(ret_type_id, TypeId::USIZE);
            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![],
                ret: TypeId::USIZE,
                is_variadic: false,
            })
        };

        let func_def = FunctionDef {
            id: def_id,
            name: name_id,
            name_span: Default::default(),
            vis: Visibility::Public,
            parent: None,
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
        };

        self.ctx.add_def(Def::Function(func_def));
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
            is_pub: true,
            is_mut: false,
        };
        let _ = self.ctx.scopes.define(name_id, info);
    }

    // Inject `@unreachable() -> !`.
    fn inject_unreachable(&mut self) {
        let name_id = self.ctx.intern("@unreachable");
        let def_id = DefId(self.ctx.defs.len() as u32);

        let ret_id = self.ctx.next_node_id();
        let sig_ty = {
            self.ctx.node_types.insert(ret_id, TypeId::NEVER);
            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![],
                ret: TypeId::NEVER,
                is_variadic: false,
            })
        };

        let func_def = FunctionDef {
            id: def_id,
            name: name_id,
            name_span: Default::default(),
            vis: Visibility::Public,
            parent: None,
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
        };

        self.ctx.add_def(Def::Function(func_def));
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
            is_pub: true,
            is_mut: false,
        };
        let _ = self.ctx.scopes.define(name_id, info);
    }

    fn inject_bitwise(&mut self, name: &str, int_trait_id: DefId) {
        let name_id = self.ctx.intern(name);
        let def_id = DefId(self.ctx.defs.len() as u32);

        let trait_node = ast::TypeNode {
            id: self.ctx.next_node_id(),
            span: Default::default(),
            kind: ast::TypeKind::Infer,
        };
        let trait_ty = self.builtin_trait_ty_by_id(int_trait_id, vec![]);
        self.ctx.node_types.insert(trait_node.id, trait_ty);

        let param_t = ast::GenericParam {
            name: self.ctx.intern("T"),
            span: Default::default(),
        };

        let target_node = ast::TypeNode {
            id: self.ctx.next_node_id(),
            span: Default::default(),
            kind: ast::TypeKind::Infer,
        };
        let val_param_id = self.ctx.next_node_id();
        let ret_id = self.ctx.next_node_id();

        let sig_ty = {
            let t_ty = self.ctx.type_registry.intern(TypeKind::Param(param_t.name));
            self.ctx.node_types.insert(target_node.id, t_ty);
            self.ctx.node_types.insert(val_param_id, t_ty);
            self.ctx.node_types.insert(ret_id, t_ty);
            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![t_ty],
                ret: t_ty,
                is_variadic: false,
            })
        };

        let func_def = FunctionDef {
            id: def_id,
            name: name_id,
            name_span: Default::default(),
            vis: Visibility::Public,
            parent: None,
            is_imported: false,
            generics: vec![param_t],
            where_clauses: vec![ast::WhereClause {
                span: Default::default(),
                target_ty: target_node,
                bounds: vec![trait_node],
            }],
            params: vec![ast::FuncParam {
                pattern: ast::BindingPattern {
                    name: self.ctx.intern("val"),
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
        };

        self.ctx.add_def(Def::Function(func_def));
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
            is_pub: true,
            is_mut: false,
        };
        let _ = self.ctx.scopes.define(name_id, info);
    }

    // Inject zero-argument hardware-style intrinsics.
    fn inject_void_intrinsic(&mut self, name: &str, is_divergent: bool) {
        let name_id = self.ctx.intern(name);
        let def_id = DefId(self.ctx.defs.len() as u32);
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
            self.ctx.node_types.insert(ret_id, ret_type);
            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![],
                ret: ret_type,
                is_variadic: false,
            })
        };

        let func_def = FunctionDef {
            id: def_id,
            name: name_id,
            name_span: Default::default(),
            vis: Visibility::Public,
            parent: None,
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
        };

        self.ctx.add_def(Def::Function(func_def));
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
            is_pub: true,
            is_mut: false,
        };
        let _ = self.ctx.scopes.define(name_id, info);
    }

    fn inject_memory_intrinsic(&mut self, kind: MemoryIntrinsicKind) {
        let name = kind.name();
        let name_id = self.ctx.intern(name);
        let def_id = DefId(self.ctx.defs.len() as u32);

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

        let sig_ty = {
            self.ctx.node_types.insert(param_dest_id, ptr_mut_u8);
            self.ctx
                .node_types
                .insert(param_src_val_id, kind.src_or_value_type(ptr_u8));
            self.ctx.node_types.insert(param_len_id, TypeId::USIZE);
            self.ctx.node_types.insert(ret_id, TypeId::VOID);

            self.ctx.type_registry.intern(TypeKind::Function {
                params: vec![ptr_mut_u8, kind.src_or_value_type(ptr_u8), TypeId::USIZE],
                ret: TypeId::VOID,
                is_variadic: false,
            })
        };

        let func_def = FunctionDef {
            id: def_id,
            name: name_id,
            name_span: Default::default(),
            vis: Visibility::Public,
            parent: None,
            is_imported: false,
            generics: vec![], // Memory intrinsics always operate on raw bytes.
            where_clauses: vec![],
            params: vec![
                ast::FuncParam {
                    pattern: ast::BindingPattern {
                        name: self.ctx.intern("dest"),
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
                        name: self.ctx.intern(kind.src_or_value_name()),
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
                        name: self.ctx.intern("len"),
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
        };

        self.ctx.add_def(Def::Function(func_def));
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
                is_pub: true,
                is_mut: false,
            },
        );
    }

    fn inject_atomic_load(&mut self) {
        let param_t = GenericParam {
            name: self.ctx.intern("T"),
            span: Default::default(),
        };
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

    fn inject_atomic_store(&mut self) {
        let param_t = GenericParam {
            name: self.ctx.intern("T"),
            span: Default::default(),
        };
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

    fn inject_atomic_cas(&mut self, name: &str) {
        let param_t = GenericParam {
            name: self.ctx.intern("T"),
            span: Default::default(),
        };
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

    fn inject_atomic_xchg(&mut self) {
        let param_t = GenericParam {
            name: self.ctx.intern("T"),
            span: Default::default(),
        };
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

    fn inject_atomic_rmw(&mut self, name: &str) {
        let param_t = GenericParam {
            name: self.ctx.intern("T"),
            span: Default::default(),
        };
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

    fn inject_atomic_fence(&mut self) {
        self.inject_builtin_function("@fence", vec![], vec![("order", TypeId::U8)], TypeId::VOID);
    }

    fn inject_simd_any(&mut self) {
        let param_mask = GenericParam {
            name: self.ctx.intern("Mask"),
            span: Default::default(),
        };
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

    fn inject_simd_all(&mut self) {
        let param_mask = GenericParam {
            name: self.ctx.intern("Mask"),
            span: Default::default(),
        };
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

    fn inject_simd_bitmask(&mut self) {
        let param_mask = GenericParam {
            name: self.ctx.intern("Mask"),
            span: Default::default(),
        };
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

    fn inject_simd_select(&mut self) {
        let param_mask = GenericParam {
            name: self.ctx.intern("Mask"),
            span: Default::default(),
        };
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_shuffle(&mut self) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_swizzle(&mut self) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_reduce(&mut self, name: &str) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        self.inject_builtin_function(name, vec![param_value], vec![("value", value_ty)], value_ty);
    }

    fn inject_simd_extract_half(&mut self, name: &str) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_insert_half(&mut self, name: &str) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_permute_unary(&mut self, name: &str) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        self.inject_builtin_function(name, vec![param_value], vec![("value", value_ty)], value_ty);
    }

    fn inject_simd_rotate(&mut self, name: &str) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_abs(&mut self) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_float_unary(&mut self, name: &str) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
        let value_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Param(param_value.name));
        self.inject_builtin_function(name, vec![param_value], vec![("value", value_ty)], value_ty);
    }

    fn inject_simd_pairwise(&mut self, name: &str) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_clamp(&mut self) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_splat(&mut self) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_cast(&mut self) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_bitcast(&mut self) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_load(&mut self) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_store(&mut self) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_masked_load(&mut self) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_masked_store(&mut self) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_gather(&mut self) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_scatter(&mut self) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_masked_gather(&mut self) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_simd_masked_scatter(&mut self) {
        let param_value = GenericParam {
            name: self.ctx.intern("Value"),
            span: Default::default(),
        };
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

    fn inject_builtin_function(
        &mut self,
        name: &str,
        generics: Vec<GenericParam>,
        params: Vec<(&str, TypeId)>,
        ret_ty: TypeId,
    ) {
        let name_id = self.ctx.intern(name);
        let def_id = DefId(self.ctx.defs.len() as u32);

        let mut param_defs = Vec::with_capacity(params.len());
        let mut param_tys = Vec::with_capacity(params.len());

        for (param_name, ty) in params {
            let type_node_id = self.ctx.next_node_id();
            self.ctx.node_types.insert(type_node_id, ty);
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
        self.ctx.node_types.insert(ret_id, ret_ty);
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

        let func_def = FunctionDef {
            id: def_id,
            name: name_id,
            name_span: Default::default(),
            vis: Visibility::Public,
            parent: None,
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
        };

        self.ctx.add_def(Def::Function(func_def));
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
                is_pub: true,
                is_mut: false,
            },
        );
    }

    fn inject_custom_define_consts(&mut self) {
        let prev_scope = self.ctx.scopes.current_scope_id();
        self.ctx.scopes.set_current_scope(ScopeId(0));

        let defines = self
            .ctx
            .sess
            .custom_defines
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<Vec<_>>();

        for (name, value) in defines {
            if !is_valid_define_identifier(&name) {
                continue;
            }

            let name_id = self.ctx.intern(&name);
            let def_id = DefId(self.ctx.defs.len() as u32);
            let expr = self.custom_define_expr(&value);
            self.ctx.add_def(Def::Global(GlobalDef {
                id: def_id,
                name: name_id,
                vis: Visibility::Private,
                parent: None,
                is_imported: true,
                value: expr,
                is_static: false,
                is_extern: false,
                is_mut: false,
                span: Span::default(),
                docs: None,
                attributes: Vec::new(),
            }));

            let node_id = self.ctx.next_node_id();
            let _ = self.ctx.scopes.define(
                name_id,
                SymbolInfo {
                    kind: SymbolKind::Const,
                    node_id,
                    type_id: TypeId::ERROR,
                    def_id: Some(def_id),
                    span: Span::default(),
                    is_pub: false,
                    is_mut: false,
                },
            );
        }

        if let Some(prev_scope) = prev_scope {
            self.ctx.scopes.set_current_scope(prev_scope);
        }
    }

    fn custom_define_expr(&mut self, value: &str) -> ast::Expr {
        let kind = match value {
            "true" => ast::ExprKind::Bool(true),
            "false" => ast::ExprKind::Bool(false),
            _ => ast::ExprKind::String(value.to_string()),
        };

        ast::Expr {
            id: self.ctx.next_node_id(),
            span: Span::default(),
            kind,
        }
    }
}

fn is_valid_define_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }

    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}
