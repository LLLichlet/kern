use crate::{
    MirBlock, MirCallTarget, MirInstruction, MirMemoryIntrinsic, MirOperand, MirPlace, MirRvalue,
    MirSliceBase,
};
use std::collections::HashMap;

pub(super) fn rewrite_block(
    block: &mut MirBlock,
    replacements: &HashMap<crate::MirLocalId, MirOperand>,
) -> usize {
    let mut rewrites = 0;
    for instruction in &mut block.instructions {
        match instruction {
            MirInstruction::Let { init, .. } => rewrites += rewrite_rvalue(init, replacements),
            MirInstruction::Assign { place, value, .. } => {
                rewrites += rewrite_place(place, replacements);
                rewrites += rewrite_rvalue(value, replacements);
            }
            MirInstruction::Memory(intrinsic) => {
                rewrites += rewrite_memory_intrinsic(intrinsic, replacements)
            }
            MirInstruction::InlineAsm(asm) => {
                for input in &mut asm.input_args {
                    rewrites += rewrite_operand(input, replacements);
                }
                for output in &mut asm.output_ptrs {
                    rewrites += rewrite_operand(output, replacements);
                }
            }
            MirInstruction::SimdStore { ptr, value, .. } => {
                rewrites += rewrite_operand(ptr, replacements);
                rewrites += rewrite_operand(value, replacements);
            }
            MirInstruction::SimdMaskedStore {
                ptr, mask, value, ..
            } => {
                rewrites += rewrite_operand(ptr, replacements);
                rewrites += rewrite_operand(mask, replacements);
                rewrites += rewrite_operand(value, replacements);
            }
            MirInstruction::SimdScatter {
                ptr,
                indices,
                value,
            } => {
                rewrites += rewrite_operand(ptr, replacements);
                rewrites += rewrite_operand(indices, replacements);
                rewrites += rewrite_operand(value, replacements);
            }
            MirInstruction::SimdMaskedScatter {
                ptr,
                indices,
                mask,
                value,
            } => {
                rewrites += rewrite_operand(ptr, replacements);
                rewrites += rewrite_operand(indices, replacements);
                rewrites += rewrite_operand(mask, replacements);
                rewrites += rewrite_operand(value, replacements);
            }
            MirInstruction::AtomicStore { ptr, value, .. } => {
                rewrites += rewrite_operand(ptr, replacements);
                rewrites += rewrite_operand(value, replacements);
            }
            MirInstruction::Fence { .. } => {}
            MirInstruction::Trap | MirInstruction::Breakpoint => {}
            MirInstruction::Eval(rvalue) | MirInstruction::Defer(rvalue) => {
                rewrites += rewrite_rvalue(rvalue, replacements);
            }
        }
    }

    match &mut block.terminator {
        crate::MirTerminator::Goto(_) | crate::MirTerminator::Unreachable => {}
        crate::MirTerminator::Branch { cond, .. } => rewrites += rewrite_rvalue(cond, replacements),
        crate::MirTerminator::Switch { target, .. } => {
            rewrites += rewrite_rvalue(target, replacements)
        }
        crate::MirTerminator::Return(value) => {
            if let Some(value) = value {
                rewrites += rewrite_rvalue(value, replacements);
            }
        }
    }

    rewrites
}

fn rewrite_memory_intrinsic(
    intrinsic: &mut MirMemoryIntrinsic,
    replacements: &HashMap<crate::MirLocalId, MirOperand>,
) -> usize {
    match intrinsic {
        MirMemoryIntrinsic::Copy { dest, src, len }
        | MirMemoryIntrinsic::Move { dest, src, len } => {
            rewrite_operand(dest, replacements)
                + rewrite_operand(src, replacements)
                + rewrite_operand(len, replacements)
        }
        MirMemoryIntrinsic::Set { dest, val, len } => {
            rewrite_operand(dest, replacements)
                + rewrite_operand(val, replacements)
                + rewrite_operand(len, replacements)
        }
    }
}

fn rewrite_rvalue(
    rvalue: &mut MirRvalue,
    replacements: &HashMap<crate::MirLocalId, MirOperand>,
) -> usize {
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
        | MirRvalue::SimdBitcast { value: operand } => rewrite_operand(operand, replacements),
        MirRvalue::Call { callee, args } => {
            let mut rewrites = 0;
            if let MirCallTarget::Operand(operand) = callee {
                rewrites += rewrite_operand(operand, replacements);
            }
            for arg in args {
                rewrites += rewrite_operand(arg, replacements);
            }
            rewrites
        }
        MirRvalue::Aggregate { fields, .. } => fields
            .iter_mut()
            .map(|field| rewrite_operand(field, replacements))
            .sum(),
        MirRvalue::Binary { lhs, rhs, .. } => {
            rewrite_operand(lhs, replacements) + rewrite_operand(rhs, replacements)
        }
        MirRvalue::SimdBinaryIntrinsic { lhs, rhs, .. } => {
            rewrite_operand(lhs, replacements) + rewrite_operand(rhs, replacements)
        }
        MirRvalue::AtomicLoad { ptr, .. } => rewrite_operand(ptr, replacements),
        MirRvalue::AtomicCas {
            ptr,
            expected,
            desired,
            ..
        } => {
            rewrite_operand(ptr, replacements)
                + rewrite_operand(expected, replacements)
                + rewrite_operand(desired, replacements)
        }
        MirRvalue::AtomicRmw { ptr, value, .. } => {
            rewrite_operand(ptr, replacements) + rewrite_operand(value, replacements)
        }
        MirRvalue::SimdLoad { ptr, .. } => rewrite_operand(ptr, replacements),
        MirRvalue::SimdMaskedLoad {
            ptr, mask, or_else, ..
        } => {
            rewrite_operand(ptr, replacements)
                + rewrite_operand(mask, replacements)
                + rewrite_operand(or_else, replacements)
        }
        MirRvalue::SimdGather { ptr, indices } => {
            rewrite_operand(ptr, replacements) + rewrite_operand(indices, replacements)
        }
        MirRvalue::SimdMaskedGather {
            ptr,
            indices,
            mask,
            or_else,
        } => {
            rewrite_operand(ptr, replacements)
                + rewrite_operand(indices, replacements)
                + rewrite_operand(mask, replacements)
                + rewrite_operand(or_else, replacements)
        }
        MirRvalue::SliceOp {
            lhs,
            start,
            end,
            is_inclusive: _,
        } => {
            let mut rewrites = rewrite_slice_base(lhs, replacements);
            if let Some(start) = start {
                rewrites += rewrite_operand(start, replacements);
            }
            if let Some(end) = end {
                rewrites += rewrite_operand(end, replacements);
            }
            rewrites
        }
        MirRvalue::SimdSelect {
            mask,
            on_true,
            on_false,
        } => {
            rewrite_operand(mask, replacements)
                + rewrite_operand(on_true, replacements)
                + rewrite_operand(on_false, replacements)
        }
        MirRvalue::SimdShuffle { lhs, rhs, .. } => {
            rewrite_operand(lhs, replacements) + rewrite_operand(rhs, replacements)
        }
        MirRvalue::SimdInsertHalf { base, half, .. } => {
            rewrite_operand(base, replacements) + rewrite_operand(half, replacements)
        }
        MirRvalue::AddressOf(place) | MirRvalue::Load(place) => rewrite_place(place, replacements),
    }
}

fn rewrite_place(
    place: &mut MirPlace,
    replacements: &HashMap<crate::MirLocalId, MirOperand>,
) -> usize {
    match place {
        MirPlace::Local(_) => 0,
        MirPlace::Global(_) => 0,
        MirPlace::Deref(operand) => rewrite_operand(operand, replacements),
        MirPlace::Field { base, .. } => rewrite_place(base, replacements),
        MirPlace::Index { base, index } => {
            rewrite_place(base, replacements) + rewrite_operand(index, replacements)
        }
    }
}

fn rewrite_slice_base(
    base: &mut MirSliceBase,
    replacements: &HashMap<crate::MirLocalId, MirOperand>,
) -> usize {
    match base {
        MirSliceBase::Operand(operand) => rewrite_operand(operand, replacements),
        MirSliceBase::Place(place) => rewrite_place(place, replacements),
    }
}

fn rewrite_operand(
    operand: &mut MirOperand,
    replacements: &HashMap<crate::MirLocalId, MirOperand>,
) -> usize {
    let MirOperand::Local(local) = operand else {
        return 0;
    };
    let Some(replacement) = replacements.get(local) else {
        return 0;
    };
    *operand = replacement.clone();
    1
}
