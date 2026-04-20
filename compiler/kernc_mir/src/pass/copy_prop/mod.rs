mod analysis;
mod rewrite;
mod uses;

use super::MirPassReport;
use crate::{MirInstruction, MirModule, MirPlace};
use analysis::{collect_copy_candidates, resolve_replacements};
use rewrite::rewrite_block;
use uses::count_local_uses;

pub(super) fn run_local_copy_propagation(module: &mut MirModule) -> MirPassReport {
    let mut report = MirPassReport {
        name: "local_copy_propagation",
        ..MirPassReport::default()
    };

    for function in &mut module.functions {
        let Some(body) = &mut function.body else {
            continue;
        };
        let body_report = rewrite_body(body);
        if body_report.changed() {
            report.changed_bodies += 1;
            report.operand_rewrites += body_report.operand_rewrites;
            report.removed_let_instructions += body_report.removed_let_instructions;
        }
    }

    report
}

fn rewrite_body(body: &mut crate::MirBody) -> MirPassReport {
    let candidates = collect_copy_candidates(body);
    let replacements = resolve_replacements(&candidates);
    if replacements.is_empty() {
        return MirPassReport {
            name: "local_copy_propagation",
            ..MirPassReport::default()
        };
    }

    let mut operand_rewrites = 0;
    for block in &mut body.blocks {
        operand_rewrites += rewrite_block(block, &replacements);
    }

    let remaining_uses = count_local_uses(body);
    let mut removed_let_instructions = 0;
    for block in &mut body.blocks {
        let before = block.instructions.len();
        block
            .instructions
            .retain(|instruction| match &instruction.kind {
                MirInstruction::Let {
                    place: MirPlace::Local(local),
                    ..
                } => !replacements.contains_key(local) || remaining_uses.contains_key(local),
                _ => true,
            });
        removed_let_instructions += before - block.instructions.len();
    }

    let mut report = MirPassReport {
        name: "local_copy_propagation",
        operand_rewrites,
        removed_let_instructions,
        ..MirPassReport::default()
    };
    if report.operand_rewrites > 0 || report.removed_let_instructions > 0 {
        report.changed_bodies = 1;
    }
    report
}
