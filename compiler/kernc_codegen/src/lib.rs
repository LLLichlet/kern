//! LLVM code generation backend.
//!
//! This crate turns MIR modules into LLVM IR, emits objects/bitcode, optionally
//! runs ThinLTO, and exposes codegen diagnostics such as instruction counts,
//! alloca cleanup stats, and hot-function summaries.

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
pub use thinlto::{ThinLtoModule, ThinLtoObject, ThinLtoObjectKind, ThinLtoOptions, run_thin_lto};
