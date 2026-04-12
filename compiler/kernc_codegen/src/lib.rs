mod codegen;
mod llvm_api;
mod llvm_facade;
mod thinlto;

pub use codegen::{
    AllocaNameStat, CodeGenerator, CodegenAllocaStats, CodegenReport, CodegenTiming,
    EmitObjectReport, EmitObjectTiming, IrCleanupStats, IrFunctionStats, IrInstructionStats,
};
pub use llvm_api::{Context, InlineAsmDialect};
pub use llvm_facade::{
    AddressSpace, AtomicOrdering, AtomicRMWBinOp, FloatPredicate, IntPredicate, OptimizationLevel,
    attributes, basic_block, builder, context, intrinsics, llvm_sys, module, types, values,
};
pub use thinlto::{ThinLtoModule, ThinLtoObject, ThinLtoOptions, run_thin_lto};
