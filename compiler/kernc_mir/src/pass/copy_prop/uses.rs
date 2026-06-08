//! Local-use counting and rooted-place detection.
//!
//! Copy propagation needs two related views: ordinary operand use counts for
//! dead-let removal, and rooted place uses where a local's address/projection
//! identity must be preserved.

use crate::{
    MirBody, MirCallTarget, MirInstruction, MirMemoryIntrinsic, MirOperand, MirPlace, MirRvalue,
    MirSliceBase,
};
use std::collections::{HashMap, HashSet};

pub(super) fn count_local_uses(body: &MirBody) -> HashMap<crate::MirLocalId, usize> {
    let mut counts = HashMap::new();
    for block in &body.blocks {
        for instruction in &block.instructions {
            match &instruction.kind {
                MirInstruction::Let { init, .. } => count_rvalue_uses(init, &mut counts),
                MirInstruction::Assign { place, value, .. } => {
                    count_place_uses(place, &mut counts);
                    count_rvalue_uses(value, &mut counts);
                }
                MirInstruction::Memory(intrinsic) => count_memory_uses(intrinsic, &mut counts),
                MirInstruction::InlineAsm(asm) => {
                    for input in &asm.input_args {
                        count_operand_uses(input, &mut counts);
                    }
                    for output in &asm.output_ptrs {
                        count_operand_uses(output, &mut counts);
                    }
                }
                MirInstruction::SimdStore { ptr, value, .. } => {
                    count_operand_uses(ptr, &mut counts);
                    count_operand_uses(value, &mut counts);
                }
                MirInstruction::SimdMaskedStore {
                    ptr, mask, value, ..
                } => {
                    count_operand_uses(ptr, &mut counts);
                    count_operand_uses(mask, &mut counts);
                    count_operand_uses(value, &mut counts);
                }
                MirInstruction::SimdScatter {
                    ptr,
                    indices,
                    value,
                } => {
                    count_operand_uses(ptr, &mut counts);
                    count_operand_uses(indices, &mut counts);
                    count_operand_uses(value, &mut counts);
                }
                MirInstruction::SimdMaskedScatter {
                    ptr,
                    indices,
                    mask,
                    value,
                } => {
                    count_operand_uses(ptr, &mut counts);
                    count_operand_uses(indices, &mut counts);
                    count_operand_uses(mask, &mut counts);
                    count_operand_uses(value, &mut counts);
                }
                MirInstruction::AtomicStore { ptr, value, .. } => {
                    count_operand_uses(ptr, &mut counts);
                    count_operand_uses(value, &mut counts);
                }
                MirInstruction::Fence { .. } => {}
                MirInstruction::Trap | MirInstruction::Breakpoint => {}
                MirInstruction::Eval(rvalue) | MirInstruction::Defer(rvalue) => {
                    count_rvalue_uses(rvalue, &mut counts)
                }
            }
        }
        match &block.terminator.kind {
            crate::MirTerminator::Goto(_) | crate::MirTerminator::Unreachable => {}
            crate::MirTerminator::Branch { cond, .. } => count_rvalue_uses(cond, &mut counts),
            crate::MirTerminator::Switch { target, .. } => count_rvalue_uses(target, &mut counts),
            crate::MirTerminator::Return(value) => {
                if let Some(value) = value {
                    count_rvalue_uses(value, &mut counts);
                }
            }
        }
    }
    counts
}

fn count_memory_uses(
    intrinsic: &MirMemoryIntrinsic,
    counts: &mut HashMap<crate::MirLocalId, usize>,
) {
    match intrinsic {
        MirMemoryIntrinsic::Copy { dest, src, len }
        | MirMemoryIntrinsic::Move { dest, src, len } => {
            count_operand_uses(dest, counts);
            count_operand_uses(src, counts);
            count_operand_uses(len, counts);
        }
        MirMemoryIntrinsic::Set { dest, val, len } => {
            count_operand_uses(dest, counts);
            count_operand_uses(val, counts);
            count_operand_uses(len, counts);
        }
    }
}

fn count_rvalue_uses(rvalue: &MirRvalue, counts: &mut HashMap<crate::MirLocalId, usize>) {
    match rvalue {
        MirRvalue::Use(operand)
        | MirRvalue::Projection { operand, .. }
        | MirRvalue::Unary { operand, .. }
        | MirRvalue::Cast { operand, .. }
        | MirRvalue::BitIntrinsic { operand, .. }
        | MirRvalue::SimdUnaryIntrinsic { operand, .. }
        | MirRvalue::SimdReduce { operand, .. }
        | MirRvalue::SimdAny { operand }
        | MirRvalue::SimdAll { operand }
        | MirRvalue::SimdBitmask { operand }
        | MirRvalue::SimdSplat { value: operand }
        | MirRvalue::SimdCast { value: operand }
        | MirRvalue::SimdBitcast { value: operand } => count_operand_uses(operand, counts),
        MirRvalue::Call { callee, args } => {
            if let MirCallTarget::Operand(operand) = callee {
                count_operand_uses(operand, counts);
            }
            for arg in args {
                count_operand_uses(arg, counts);
            }
        }
        MirRvalue::Aggregate { fields, .. } => {
            for field in fields {
                count_operand_uses(field, counts);
            }
        }
        MirRvalue::Binary { lhs, rhs, .. } => {
            count_operand_uses(lhs, counts);
            count_operand_uses(rhs, counts);
        }
        MirRvalue::SimdBinaryIntrinsic { lhs, rhs, .. } => {
            count_operand_uses(lhs, counts);
            count_operand_uses(rhs, counts);
        }
        MirRvalue::AtomicLoad { ptr, .. } => count_operand_uses(ptr, counts),
        MirRvalue::AtomicCas {
            ptr,
            expected,
            desired,
            ..
        } => {
            count_operand_uses(ptr, counts);
            count_operand_uses(expected, counts);
            count_operand_uses(desired, counts);
        }
        MirRvalue::AtomicRmw { ptr, value, .. } => {
            count_operand_uses(ptr, counts);
            count_operand_uses(value, counts);
        }
        MirRvalue::SimdLoad { ptr, .. } => count_operand_uses(ptr, counts),
        MirRvalue::SimdMaskedLoad {
            ptr, mask, or_else, ..
        } => {
            count_operand_uses(ptr, counts);
            count_operand_uses(mask, counts);
            count_operand_uses(or_else, counts);
        }
        MirRvalue::SimdGather { ptr, indices } => {
            count_operand_uses(ptr, counts);
            count_operand_uses(indices, counts);
        }
        MirRvalue::SimdMaskedGather {
            ptr,
            indices,
            mask,
            or_else,
        } => {
            count_operand_uses(ptr, counts);
            count_operand_uses(indices, counts);
            count_operand_uses(mask, counts);
            count_operand_uses(or_else, counts);
        }
        MirRvalue::SliceOp {
            lhs,
            start,
            end,
            is_inclusive: _,
        } => {
            count_slice_base_uses(lhs, counts);
            if let Some(start) = start {
                count_operand_uses(start, counts);
            }
            if let Some(end) = end {
                count_operand_uses(end, counts);
            }
        }
        MirRvalue::SimdSelect {
            mask,
            on_true,
            on_false,
        } => {
            count_operand_uses(mask, counts);
            count_operand_uses(on_true, counts);
            count_operand_uses(on_false, counts);
        }
        MirRvalue::SimdShuffle { lhs, rhs, .. } => {
            count_operand_uses(lhs, counts);
            count_operand_uses(rhs, counts);
        }
        MirRvalue::SimdInsertHalf { base, half, .. } => {
            count_operand_uses(base, counts);
            count_operand_uses(half, counts);
        }
        MirRvalue::AddressOf(place) | MirRvalue::Load(place) => count_place_uses(place, counts),
    }
}

fn count_place_uses(place: &MirPlace, counts: &mut HashMap<crate::MirLocalId, usize>) {
    match place {
        MirPlace::Local(local) => {
            *counts.entry(*local).or_insert(0) += 1;
        }
        MirPlace::Global(_) => {}
        MirPlace::Deref(operand) => count_operand_uses(operand, counts),
        MirPlace::Field { base, .. } => count_place_uses(base, counts),
        MirPlace::Index { base, index } => {
            count_place_uses(base, counts);
            count_operand_uses(index, counts);
        }
    }
}

fn count_slice_base_uses(base: &MirSliceBase, counts: &mut HashMap<crate::MirLocalId, usize>) {
    match base {
        MirSliceBase::Operand(operand) => count_operand_uses(operand, counts),
        MirSliceBase::Place(place) => count_place_uses(place, counts),
    }
}

fn count_operand_uses(operand: &MirOperand, counts: &mut HashMap<crate::MirLocalId, usize>) {
    if let MirOperand::Local(local) = operand {
        *counts.entry(*local).or_insert(0) += 1;
    }
}

pub(super) fn collect_rooted_place_uses_in_rvalue(
    rvalue: &MirRvalue,
    rooted_place_uses: &mut HashSet<crate::MirLocalId>,
) {
    match rvalue {
        MirRvalue::AddressOf(place) | MirRvalue::Load(place) => {
            if let Some(root) = root_local(place) {
                rooted_place_uses.insert(root);
            }
        }
        MirRvalue::Use(_)
        | MirRvalue::Call { .. }
        | MirRvalue::Aggregate { .. }
        | MirRvalue::Projection { .. }
        | MirRvalue::Unary { .. }
        | MirRvalue::Binary { .. }
        | MirRvalue::Cast { .. }
        | MirRvalue::BitIntrinsic { .. }
        | MirRvalue::AtomicLoad { .. }
        | MirRvalue::AtomicCas { .. }
        | MirRvalue::AtomicRmw { .. }
        | MirRvalue::SimdLoad { .. }
        | MirRvalue::SimdMaskedLoad { .. }
        | MirRvalue::SimdGather { .. }
        | MirRvalue::SimdMaskedGather { .. }
        | MirRvalue::SimdUnaryIntrinsic { .. }
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
        MirRvalue::SliceOp { lhs, .. } => {
            if let MirSliceBase::Place(place) = lhs
                && let Some(root) = root_local(place)
            {
                rooted_place_uses.insert(root);
            }
        }
    }
}

pub(super) fn root_local(place: &MirPlace) -> Option<crate::MirLocalId> {
    match place {
        MirPlace::Local(local) => Some(*local),
        MirPlace::Global(_) => None,
        MirPlace::Deref(_) => None,
        MirPlace::Field { base, .. } | MirPlace::Index { base, .. } => root_local(base),
    }
}
