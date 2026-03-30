pub use crate::llvm_api::{
    AddressSpace, AtomicOrdering, AtomicRMWBinOp, FloatPredicate, IntPredicate, OptimizationLevel,
};

pub mod llvm_sys {
    pub use llvm_sys::*;
}

pub mod builder {
    pub use crate::llvm_api::Builder;
}

pub mod context {
    pub use crate::llvm_api::Context;
}

pub mod module {
    pub use crate::llvm_api::{Linkage, Module};
}

pub mod types {
    pub use crate::llvm_api::{
        ArrayType, AsTypeRef, BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FloatType,
        FunctionType, IntType, PointerType, ScalableVectorType, StructType, VectorType, VoidType,
    };
}

pub mod values {
    pub use crate::llvm_api::{
        ArrayValue, AsValueRef, BasicMetadataValueEnum, BasicValue, BasicValueEnum, CallSiteValue,
        FloatValue, FunctionValue, GlobalValue, InstructionValue, IntValue, PhiValue, PointerValue,
        StructValue,
    };
}

pub mod basic_block {
    pub use crate::llvm_api::BasicBlock;
}

pub mod attributes {
    pub use crate::llvm_api::{Attribute, AttributeLoc};
}

pub mod intrinsics {
    use crate::llvm_api::{BasicTypeEnum, FunctionValue, Module};

    #[derive(Clone, Copy)]
    pub struct Intrinsic {
        name: &'static str,
    }

    impl Intrinsic {
        pub fn find(name: &str) -> Option<Self> {
            match name {
                "llvm.trap" => Some(Self { name: "llvm.trap" }),
                "llvm.debugtrap" => Some(Self { name: "llvm.debugtrap" }),
                "llvm.ctpop" => Some(Self { name: "llvm.ctpop" }),
                "llvm.ctlz" => Some(Self { name: "llvm.ctlz" }),
                "llvm.cttz" => Some(Self { name: "llvm.cttz" }),
                "llvm.bswap" => Some(Self { name: "llvm.bswap" }),
                _ => None,
            }
        }

        pub fn get_declaration<'ctx>(
            self,
            module: &Module<'ctx>,
            types: &[BasicTypeEnum<'ctx>],
        ) -> Option<FunctionValue<'ctx>> {
            module.get_intrinsic_declaration(self.name, types)
        }
    }
}
