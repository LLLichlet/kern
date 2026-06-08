//! Workload statistics for MIR modules.
//!
//! These counters summarize how much codegen work a module contains. The driver
//! uses them for reporting and partitioning heuristics, so the visitor here
//! deliberately counts both high-level items and detailed instruction/rvalue
//! shapes.

use crate::{MirCallTarget, MirInstruction, MirLocalKind, MirModule, MirRvalue, MirTerminator};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MirWorkloadStats {
    pub structs: usize,
    pub globals: usize,
    pub globals_with_init: usize,
    pub functions: usize,
    pub function_bodies: usize,
    pub extern_functions: usize,
    pub locals: usize,
    pub param_locals: usize,
    pub let_locals: usize,
    pub blocks: usize,
    pub instructions: usize,
    pub let_instructions: usize,
    pub assign_instructions: usize,
    pub memory_instructions: usize,
    pub inline_asm_instructions: usize,
    pub simd_store_instructions: usize,
    pub simd_masked_store_instructions: usize,
    pub simd_scatter_instructions: usize,
    pub simd_masked_scatter_instructions: usize,
    pub atomic_store_instructions: usize,
    pub fence_instructions: usize,
    pub trap_instructions: usize,
    pub breakpoint_instructions: usize,
    pub eval_instructions: usize,
    pub defer_instructions: usize,
    pub use_rvalues: usize,
    pub call_rvalues: usize,
    pub aggregate_rvalues: usize,
    pub projection_rvalues: usize,
    pub unary_rvalues: usize,
    pub binary_rvalues: usize,
    pub cast_rvalues: usize,
    pub bit_intrinsic_rvalues: usize,
    pub atomic_load_rvalues: usize,
    pub atomic_cas_rvalues: usize,
    pub atomic_rmw_rvalues: usize,
    pub simd_load_rvalues: usize,
    pub simd_masked_load_rvalues: usize,
    pub simd_gather_rvalues: usize,
    pub simd_masked_gather_rvalues: usize,
    pub slice_op_rvalues: usize,
    pub address_of_rvalues: usize,
    pub load_rvalues: usize,
    pub direct_calls: usize,
    pub indirect_calls: usize,
    pub gotos: usize,
    pub branches: usize,
    pub switches: usize,
    pub returns: usize,
    pub unreachable_terminators: usize,
}

impl MirWorkloadStats {
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }
}

impl MirModule {
    pub fn workload_stats(&self) -> MirWorkloadStats {
        let mut stats = MirWorkloadStats {
            structs: self.structs.len(),
            globals: self.globals.len(),
            globals_with_init: self
                .globals
                .iter()
                .filter(|global| global.init.is_some())
                .count(),
            functions: self.functions.len(),
            function_bodies: self
                .functions
                .iter()
                .filter(|function| function.body.is_some())
                .count(),
            extern_functions: self
                .functions
                .iter()
                .filter(|function| function.is_extern)
                .count(),
            ..MirWorkloadStats::default()
        };

        for function in &self.functions {
            let Some(body) = &function.body else {
                continue;
            };
            stats.locals += body.locals.len();
            for local in &body.locals {
                match local.kind {
                    MirLocalKind::Param => stats.param_locals += 1,
                    MirLocalKind::Let => stats.let_locals += 1,
                }
            }
            stats.blocks += body.blocks.len();
            for block in &body.blocks {
                stats.instructions += block.instructions.len();
                for instruction in &block.instructions {
                    match &instruction.kind {
                        MirInstruction::Let { init, .. } => {
                            stats.let_instructions += 1;
                            visit_rvalue(init, &mut stats);
                        }
                        MirInstruction::Assign { value, .. } => {
                            stats.assign_instructions += 1;
                            visit_rvalue(value, &mut stats);
                        }
                        MirInstruction::Memory(_) => {
                            stats.memory_instructions += 1;
                        }
                        MirInstruction::InlineAsm(_) => {
                            stats.inline_asm_instructions += 1;
                        }
                        MirInstruction::SimdStore { .. } => {
                            stats.simd_store_instructions += 1;
                        }
                        MirInstruction::SimdMaskedStore { .. } => {
                            stats.simd_masked_store_instructions += 1;
                        }
                        MirInstruction::SimdScatter { .. } => {
                            stats.simd_scatter_instructions += 1;
                        }
                        MirInstruction::SimdMaskedScatter { .. } => {
                            stats.simd_masked_scatter_instructions += 1;
                        }
                        MirInstruction::AtomicStore { .. } => {
                            stats.atomic_store_instructions += 1;
                        }
                        MirInstruction::Fence { .. } => {
                            stats.fence_instructions += 1;
                        }
                        MirInstruction::Trap => {
                            stats.trap_instructions += 1;
                        }
                        MirInstruction::Breakpoint => {
                            stats.breakpoint_instructions += 1;
                        }
                        MirInstruction::Eval(rvalue) => {
                            stats.eval_instructions += 1;
                            visit_rvalue(rvalue, &mut stats);
                        }
                        MirInstruction::Defer(rvalue) => {
                            stats.defer_instructions += 1;
                            visit_rvalue(rvalue, &mut stats);
                        }
                    }
                }
                match &block.terminator.kind {
                    MirTerminator::Goto(_) => stats.gotos += 1,
                    MirTerminator::Branch { cond, .. } => {
                        stats.branches += 1;
                        visit_rvalue(cond, &mut stats);
                    }
                    MirTerminator::Switch { target, .. } => {
                        stats.switches += 1;
                        visit_rvalue(target, &mut stats);
                    }
                    MirTerminator::Return(value) => {
                        stats.returns += 1;
                        if let Some(value) = value {
                            visit_rvalue(value, &mut stats);
                        }
                    }
                    MirTerminator::Unreachable => stats.unreachable_terminators += 1,
                }
            }
        }

        stats
    }
}

fn visit_rvalue(rvalue: &MirRvalue, stats: &mut MirWorkloadStats) {
    match rvalue {
        MirRvalue::Use(_) => stats.use_rvalues += 1,
        MirRvalue::Call { callee, .. } => {
            stats.call_rvalues += 1;
            match callee {
                MirCallTarget::Direct(_) => stats.direct_calls += 1,
                MirCallTarget::Operand(_) => stats.indirect_calls += 1,
            }
        }
        MirRvalue::Aggregate { .. } => stats.aggregate_rvalues += 1,
        MirRvalue::Projection { .. } => stats.projection_rvalues += 1,
        MirRvalue::Unary { .. } => stats.unary_rvalues += 1,
        MirRvalue::Binary { .. } => stats.binary_rvalues += 1,
        MirRvalue::Cast { .. } => stats.cast_rvalues += 1,
        MirRvalue::BitIntrinsic { .. } => stats.bit_intrinsic_rvalues += 1,
        MirRvalue::AtomicLoad { .. } => stats.atomic_load_rvalues += 1,
        MirRvalue::AtomicCas { .. } => stats.atomic_cas_rvalues += 1,
        MirRvalue::AtomicRmw { .. } => stats.atomic_rmw_rvalues += 1,
        MirRvalue::SimdLoad { .. } => stats.simd_load_rvalues += 1,
        MirRvalue::SimdMaskedLoad { .. } => stats.simd_masked_load_rvalues += 1,
        MirRvalue::SimdGather { .. } => stats.simd_gather_rvalues += 1,
        MirRvalue::SimdMaskedGather { .. } => stats.simd_masked_gather_rvalues += 1,
        MirRvalue::SliceOp { .. } => stats.slice_op_rvalues += 1,
        MirRvalue::SimdUnaryIntrinsic { .. }
        | MirRvalue::SimdBinaryIntrinsic { .. }
        | MirRvalue::SimdReduce { .. }
        | MirRvalue::SimdAny { .. }
        | MirRvalue::SimdAll { .. }
        | MirRvalue::SimdBitmask { .. }
        | MirRvalue::SimdSplat { .. }
        | MirRvalue::SimdCast { .. }
        | MirRvalue::SimdBitcast { .. }
        | MirRvalue::SimdSelect { .. }
        | MirRvalue::SimdShuffle { .. }
        | MirRvalue::SimdInsertHalf { .. } => {}
        MirRvalue::AddressOf(_) => stats.address_of_rvalues += 1,
        MirRvalue::Load(_) => stats.load_rvalues += 1,
    }
}
