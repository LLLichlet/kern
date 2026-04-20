use super::uses::collect_rooted_place_uses_in_rvalue;
use crate::{MirBody, MirInstruction, MirOperand, MirPlace, MirRvalue};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub(super) struct CopyCandidate {
    pub(super) replacement: MirOperand,
}

pub(super) fn collect_copy_candidates(body: &MirBody) -> HashMap<crate::MirLocalId, CopyCandidate> {
    let mut init_uses = HashMap::new();
    for local in &body.locals {
        if local.is_mut {
            continue;
        }
        for block in &body.blocks {
            for instruction in &block.instructions {
                let instruction = &instruction.kind;
                if let MirInstruction::Let {
                    place: MirPlace::Local(id),
                    init: MirRvalue::Use(operand),
                } = instruction
                    && *id == local.id
                {
                    init_uses.insert(local.id, operand.clone());
                }
            }
        }
    }

    let mut assigned = HashSet::new();
    let mut rooted_place_uses = HashSet::new();
    for block in &body.blocks {
        for instruction in &block.instructions {
            let instruction = &instruction.kind;
            match instruction {
                MirInstruction::Let { init, .. } => {
                    collect_rooted_place_uses_in_rvalue(init, &mut rooted_place_uses);
                }
                MirInstruction::Assign { place, .. } => {
                    if let Some(root) = super::uses::root_local(place) {
                        rooted_place_uses.insert(root);
                    }
                    if let MirPlace::Local(local) = place {
                        assigned.insert(*local);
                    }
                }
                MirInstruction::Memory(_) => {}
                MirInstruction::InlineAsm(_) => {}
                MirInstruction::SimdStore { .. } => {}
                MirInstruction::SimdMaskedStore { .. } => {}
                MirInstruction::SimdScatter { .. } => {}
                MirInstruction::SimdMaskedScatter { .. } => {}
                MirInstruction::AtomicStore { .. } => {}
                MirInstruction::Fence { .. } => {}
                MirInstruction::Trap | MirInstruction::Breakpoint => {}
                MirInstruction::Eval(rvalue) | MirInstruction::Defer(rvalue) => {
                    collect_rooted_place_uses_in_rvalue(rvalue, &mut rooted_place_uses);
                }
            }
        }
        match &block.terminator.kind {
            crate::MirTerminator::Goto(_) | crate::MirTerminator::Unreachable => {}
            crate::MirTerminator::Branch { cond, .. } => {
                collect_rooted_place_uses_in_rvalue(cond, &mut rooted_place_uses);
            }
            crate::MirTerminator::Switch { target, .. } => {
                collect_rooted_place_uses_in_rvalue(target, &mut rooted_place_uses);
            }
            crate::MirTerminator::Return(value) => {
                if let Some(value) = value {
                    collect_rooted_place_uses_in_rvalue(value, &mut rooted_place_uses);
                }
            }
        }
    }

    init_uses
        .into_iter()
        .filter(|(local, replacement)| {
            if assigned.contains(local) || rooted_place_uses.contains(local) {
                return false;
            }
            match replacement {
                MirOperand::Local(src) => {
                    !assigned.contains(src) && !rooted_place_uses.contains(src)
                }
                MirOperand::Const(_) => true,
            }
        })
        .map(|(local, replacement)| (local, CopyCandidate { replacement }))
        .collect()
}

pub(super) fn resolve_replacements(
    candidates: &HashMap<crate::MirLocalId, CopyCandidate>,
) -> HashMap<crate::MirLocalId, MirOperand> {
    let mut resolved = HashMap::new();
    for &local in candidates.keys() {
        let mut visiting = HashSet::new();
        if let Some(replacement) =
            resolve_replacement(local, candidates, &mut resolved, &mut visiting)
        {
            resolved.insert(local, replacement);
        }
    }
    resolved
}

fn resolve_replacement(
    local: crate::MirLocalId,
    candidates: &HashMap<crate::MirLocalId, CopyCandidate>,
    resolved: &mut HashMap<crate::MirLocalId, MirOperand>,
    visiting: &mut HashSet<crate::MirLocalId>,
) -> Option<MirOperand> {
    if let Some(replacement) = resolved.get(&local) {
        return Some(replacement.clone());
    }
    if !visiting.insert(local) {
        return None;
    }

    let replacement = match &candidates.get(&local)?.replacement {
        MirOperand::Const(value) => Some(MirOperand::Const(value.clone())),
        MirOperand::Local(src) => {
            if candidates.contains_key(src) {
                resolve_replacement(*src, candidates, resolved, visiting)
            } else {
                Some(MirOperand::Local(*src))
            }
        }
    };

    visiting.remove(&local);
    if let Some(replacement) = &replacement {
        resolved.insert(local, replacement.clone());
    }
    replacement
}
