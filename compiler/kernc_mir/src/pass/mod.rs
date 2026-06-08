//! MIR optimization and cleanup pass pipeline.
//!
//! The default pipeline runs small local rewrites after lowering: copy
//! propagation, jump threading, branch folding, and unreachable-block pruning.
//! Each pass reports what changed so the driver/tests can audit effectiveness.

mod cfg;
mod copy_prop;

use crate::MirModule;
use cfg::{run_branch_folding, run_cfg_prune_unreachable_blocks, run_cfg_thread_jumps};
use copy_prop::run_local_copy_propagation;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MirPassReport {
    pub name: &'static str,
    pub changed_bodies: usize,
    pub operand_rewrites: usize,
    pub removed_let_instructions: usize,
    pub terminator_rewrites: usize,
    pub removed_blocks: usize,
}

impl MirPassReport {
    pub fn changed(&self) -> bool {
        self.changed_bodies > 0
            || self.operand_rewrites > 0
            || self.removed_let_instructions > 0
            || self.terminator_rewrites > 0
            || self.removed_blocks > 0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MirPassPipelineReport {
    pub passes: Vec<MirPassReport>,
}

impl MirPassPipelineReport {
    pub fn is_empty(&self) -> bool {
        self.passes.is_empty()
    }
}

pub fn run_default_pass_pipeline(module: &mut MirModule) -> MirPassPipelineReport {
    let mut pipeline = MirPassPipelineReport::default();
    pipeline.passes.push(run_local_copy_propagation(module));
    pipeline.passes.push(run_cfg_thread_jumps(module));
    pipeline.passes.push(run_branch_folding(module));
    pipeline
        .passes
        .push(run_cfg_prune_unreachable_blocks(module));
    pipeline
}
