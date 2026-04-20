#![doc = include_str!("../README.md")]

mod ids;
mod ir;
mod pass;
mod stats;
mod summary;
mod verify;

#[cfg(test)]
mod tests;

pub use ids::{MirBlockId, MirLocalId};
pub use ir::{
    MirAggregateKind, MirBitIntrinsicKind, MirBlock, MirBody, MirCallTarget, MirCastKind, MirConst,
    MirField, MirFunction, MirGlobal, MirInlineAsm, MirInlineHint, MirInstruction,
    MirInstructionData, MirLinkage, MirLocal, MirLocalKind, MirMemoryIntrinsic, MirModule,
    MirOperand, MirParam, MirPlace, MirProjectionKind, MirRvalue, MirSimdBinaryIntrinsicKind,
    MirSimdReduceKind, MirSimdUnaryIntrinsicKind, MirSliceBase, MirStaticInit, MirStruct,
    MirSwitchTarget, MirTerminator, MirTerminatorData,
};
pub use pass::{MirPassPipelineReport, MirPassReport, run_default_pass_pipeline};
pub use stats::MirWorkloadStats;
pub use summary::{
    MirDirectCalleeCallsiteCount, MirFunctionSummary, MirGlobalSummary, MirItemBodyRole,
    MirReferenceSummary, MirSummaryIndex,
};
pub use verify::{MirVerifyError, verify_module};
