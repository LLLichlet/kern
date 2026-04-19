use crate::SemaContext;
use crate::def::*;
use crate::scope::{ScopeId, SymbolInfo, SymbolKind};
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{self as ast, GenericParam, TypeNode};
use kernc_utils::Span;

mod custom;
mod impls;
mod intrinsics;
mod traits;

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
        self.inject_loc();
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
            kind: ast::GenericParamKind::Type,
        }
    }
}
