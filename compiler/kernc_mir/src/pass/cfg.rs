//! Control-flow graph cleanup passes.
//!
//! CFG cleanup threads empty goto chains, folds constant branches/switches, and
//! removes unreachable blocks. Block ids are remapped after pruning so the MIR
//! verifier's dense-slot invariant remains true.

use super::MirPassReport;
use crate::{MirBlockId, MirBody, MirConst, MirModule, MirOperand, MirRvalue, MirTerminator};
use std::collections::{HashMap, HashSet};

pub(super) fn run_branch_folding(module: &mut MirModule) -> MirPassReport {
    let mut report = MirPassReport {
        name: "branch_folding",
        ..MirPassReport::default()
    };

    for function in &mut module.functions {
        let Some(body) = &mut function.body else {
            continue;
        };
        let mut body_changed = false;
        for block in &mut body.blocks {
            let Some(new_terminator) = folded_terminator(&block.terminator.kind) else {
                continue;
            };
            block.terminator.kind = new_terminator;
            report.terminator_rewrites += 1;
            body_changed = true;
        }
        if body_changed {
            report.changed_bodies += 1;
        }
    }

    report
}

pub(super) fn run_cfg_thread_jumps(module: &mut MirModule) -> MirPassReport {
    let mut report = MirPassReport {
        name: "cfg_thread_jumps",
        ..MirPassReport::default()
    };

    for function in &mut module.functions {
        let Some(body) = &mut function.body else {
            continue;
        };
        let mut body_changed = false;
        for index in 0..body.blocks.len() {
            let Some(new_terminator) = threaded_terminator(body, index) else {
                continue;
            };
            body.blocks[index].terminator.kind = new_terminator;
            report.terminator_rewrites += 1;
            body_changed = true;
        }
        if body_changed {
            report.changed_bodies += 1;
        }
    }

    report
}

pub(super) fn run_cfg_prune_unreachable_blocks(module: &mut MirModule) -> MirPassReport {
    let mut report = MirPassReport {
        name: "cfg_prune_unreachable_blocks",
        ..MirPassReport::default()
    };

    for function in &mut module.functions {
        let Some(body) = &mut function.body else {
            continue;
        };
        let removed_blocks = prune_unreachable_blocks(body);
        if removed_blocks == 0 {
            continue;
        }
        report.changed_bodies += 1;
        report.removed_blocks += removed_blocks;
    }

    report
}

fn folded_terminator(terminator: &MirTerminator) -> Option<MirTerminator> {
    match terminator {
        MirTerminator::Branch {
            cond,
            then_block,
            else_block,
        } => {
            if then_block == else_block {
                return Some(MirTerminator::Goto(*then_block));
            }
            const_bool_from_rvalue(cond)
                .map(|value| MirTerminator::Goto(if value { *then_block } else { *else_block }))
        }
        MirTerminator::Switch {
            target,
            cases,
            default_block,
        } => {
            let value = const_u128_from_rvalue(target)?;
            if let Some(case) = cases.iter().find(|case| case.values.contains(&value)) {
                return Some(MirTerminator::Goto(case.block));
            }
            default_block.map(MirTerminator::Goto)
        }
        _ => None,
    }
}

fn const_bool_from_rvalue(rvalue: &MirRvalue) -> Option<bool> {
    let MirRvalue::Use(MirOperand::Const(value)) = rvalue else {
        return None;
    };
    match value {
        MirConst::Bool { value } => Some(*value),
        _ => None,
    }
}

fn const_u128_from_rvalue(rvalue: &MirRvalue) -> Option<u128> {
    let MirRvalue::Use(MirOperand::Const(value)) = rvalue else {
        return None;
    };
    match value {
        MirConst::Integer { value, .. } => Some(*value),
        _ => None,
    }
}

fn threaded_terminator(body: &MirBody, block_index: usize) -> Option<MirTerminator> {
    let terminator = &body.blocks[block_index].terminator.kind;
    match terminator {
        MirTerminator::Goto(target) => {
            let threaded = thread_target(body, *target)?;
            Some(MirTerminator::Goto(threaded))
        }
        MirTerminator::Branch {
            cond,
            then_block,
            else_block,
        } => {
            let threaded_then = thread_target(body, *then_block);
            let threaded_else = thread_target(body, *else_block);
            if threaded_then.is_none() && threaded_else.is_none() {
                return None;
            }
            Some(MirTerminator::Branch {
                cond: cond.clone(),
                then_block: threaded_then.unwrap_or(*then_block),
                else_block: threaded_else.unwrap_or(*else_block),
            })
        }
        MirTerminator::Switch {
            target,
            cases,
            default_block,
        } => {
            let mut changed = false;
            let mut new_cases = Vec::with_capacity(cases.len());
            for case in cases {
                let threaded = thread_target(body, case.block);
                changed |= threaded.is_some();
                let mut case = case.clone();
                case.block = threaded.unwrap_or(case.block);
                new_cases.push(case);
            }
            let new_default = default_block.and_then(|block| {
                let threaded = thread_target(body, block);
                changed |= threaded.is_some();
                Some(threaded.unwrap_or(block))
            });
            if !changed {
                return None;
            }
            Some(MirTerminator::Switch {
                target: target.clone(),
                cases: new_cases,
                default_block: new_default,
            })
        }
        MirTerminator::Return(_) | MirTerminator::Unreachable => None,
    }
}

fn thread_target(body: &MirBody, start: MirBlockId) -> Option<MirBlockId> {
    let mut current = start;
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(current) {
            return None;
        }
        let block = &body.blocks[current.0 as usize];
        if !block.instructions.is_empty() {
            break;
        }
        let MirTerminator::Goto(next) = block.terminator.kind else {
            break;
        };
        if next == current {
            break;
        }
        current = next;
    }
    (current != start).then_some(current)
}

fn prune_unreachable_blocks(body: &mut MirBody) -> usize {
    let reachable = collect_reachable_blocks(body);
    if reachable.len() == body.blocks.len() {
        return 0;
    }

    let old_block_count = body.blocks.len();
    let mut remap = HashMap::new();
    let mut new_blocks = Vec::with_capacity(reachable.len());
    for block in &body.blocks {
        if !reachable.contains(&block.id) {
            continue;
        }
        let new_id = MirBlockId(new_blocks.len() as u32);
        remap.insert(block.id, new_id);

        let mut block = block.clone();
        block.id = new_id;
        new_blocks.push(block);
    }

    for block in &mut new_blocks {
        remap_terminator_targets(&mut block.terminator.kind, &remap);
    }
    // The entry block seeds reachability, so it must always be retained when
    // pruning succeeds.
    body.entry = *remap
        .get(&body.entry)
        .expect("entry block must remain reachable");
    body.blocks = new_blocks;

    old_block_count - body.blocks.len()
}

fn collect_reachable_blocks(body: &MirBody) -> HashSet<MirBlockId> {
    let mut reachable = HashSet::new();
    let mut worklist = vec![body.entry];
    while let Some(block) = worklist.pop() {
        if !reachable.insert(block) {
            continue;
        }
        let terminator = &body.blocks[block.0 as usize].terminator.kind;
        push_successors(terminator, &mut worklist);
    }
    reachable
}

fn push_successors(terminator: &MirTerminator, worklist: &mut Vec<MirBlockId>) {
    match terminator {
        MirTerminator::Goto(target) => worklist.push(*target),
        MirTerminator::Branch {
            then_block,
            else_block,
            ..
        } => {
            worklist.push(*then_block);
            worklist.push(*else_block);
        }
        MirTerminator::Switch {
            cases,
            default_block,
            ..
        } => {
            for case in cases {
                worklist.push(case.block);
            }
            if let Some(default_block) = default_block {
                worklist.push(*default_block);
            }
        }
        MirTerminator::Return(_) | MirTerminator::Unreachable => {}
    }
}

fn remap_terminator_targets(
    terminator: &mut MirTerminator,
    remap: &HashMap<MirBlockId, MirBlockId>,
) {
    match terminator {
        MirTerminator::Goto(target) => *target = remap[target],
        MirTerminator::Branch {
            then_block,
            else_block,
            ..
        } => {
            *then_block = remap[then_block];
            *else_block = remap[else_block];
        }
        MirTerminator::Switch {
            cases,
            default_block,
            ..
        } => {
            for case in cases {
                case.block = remap[&case.block];
            }
            if let Some(default_block) = default_block {
                *default_block = remap[default_block];
            }
        }
        MirTerminator::Return(_) | MirTerminator::Unreachable => {}
    }
}
