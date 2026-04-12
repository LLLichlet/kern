use super::MirVerifyError;
use super::refs::{verify_operand, verify_place, verify_slice_base};
use crate::{MirCallTarget, MirFunction, MirMemoryIntrinsic, MirRvalue};

pub(super) fn verify_rvalue(
    function: &MirFunction,
    rvalue: &MirRvalue,
    local_count: u32,
) -> Result<(), MirVerifyError> {
    match rvalue {
        MirRvalue::Use(operand) => verify_operand(function, operand, local_count),
        MirRvalue::Call { callee, args } => {
            match callee {
                MirCallTarget::Direct(_) => {}
                MirCallTarget::Operand(operand) => verify_operand(function, operand, local_count)?,
            }
            for arg in args {
                verify_operand(function, arg, local_count)?;
            }
            Ok(())
        }
        MirRvalue::Aggregate { fields, .. } => {
            for field in fields {
                verify_operand(function, field, local_count)?;
            }
            Ok(())
        }
        MirRvalue::Projection { operand, .. } => verify_operand(function, operand, local_count),
        MirRvalue::Unary { operand, .. }
        | MirRvalue::Cast { operand, .. }
        | MirRvalue::BitIntrinsic { operand, .. }
        | MirRvalue::SimdUnaryIntrinsic { operand, .. }
        | MirRvalue::SimdReduce { operand, .. }
        | MirRvalue::SimdAny { operand }
        | MirRvalue::SimdAll { operand }
        | MirRvalue::SimdBitmask { operand }
        | MirRvalue::SimdSplat { value: operand }
        | MirRvalue::SimdCast { value: operand }
        | MirRvalue::SimdBitcast { value: operand } => {
            verify_operand(function, operand, local_count)
        }
        MirRvalue::Binary { lhs, rhs, .. } | MirRvalue::SimdBinaryIntrinsic { lhs, rhs, .. } => {
            verify_operand(function, lhs, local_count)?;
            verify_operand(function, rhs, local_count)
        }
        MirRvalue::AtomicLoad { ptr, .. } => verify_operand(function, ptr, local_count),
        MirRvalue::AtomicCas {
            ptr,
            expected,
            desired,
            ..
        } => {
            verify_operand(function, ptr, local_count)?;
            verify_operand(function, expected, local_count)?;
            verify_operand(function, desired, local_count)
        }
        MirRvalue::AtomicRmw { ptr, value, .. } => {
            verify_operand(function, ptr, local_count)?;
            verify_operand(function, value, local_count)
        }
        MirRvalue::SimdLoad { ptr, .. } => verify_operand(function, ptr, local_count),
        MirRvalue::SimdMaskedLoad {
            ptr, mask, or_else, ..
        } => {
            verify_operand(function, ptr, local_count)?;
            verify_operand(function, mask, local_count)?;
            verify_operand(function, or_else, local_count)
        }
        MirRvalue::SimdGather { ptr, indices } => {
            verify_operand(function, ptr, local_count)?;
            verify_operand(function, indices, local_count)
        }
        MirRvalue::SimdMaskedGather {
            ptr,
            indices,
            mask,
            or_else,
        } => {
            verify_operand(function, ptr, local_count)?;
            verify_operand(function, indices, local_count)?;
            verify_operand(function, mask, local_count)?;
            verify_operand(function, or_else, local_count)
        }
        MirRvalue::SliceOp {
            lhs,
            start,
            end,
            is_inclusive: _,
        } => {
            verify_slice_base(function, lhs, local_count)?;
            if let Some(start) = start {
                verify_operand(function, start, local_count)?;
            }
            if let Some(end) = end {
                verify_operand(function, end, local_count)?;
            }
            Ok(())
        }
        MirRvalue::SimdSelect {
            mask,
            on_true,
            on_false,
        } => {
            verify_operand(function, mask, local_count)?;
            verify_operand(function, on_true, local_count)?;
            verify_operand(function, on_false, local_count)
        }
        MirRvalue::SimdShuffle { lhs, rhs, .. } => {
            verify_operand(function, lhs, local_count)?;
            verify_operand(function, rhs, local_count)
        }
        MirRvalue::SimdInsertHalf { base, half, .. } => {
            verify_operand(function, base, local_count)?;
            verify_operand(function, half, local_count)
        }
        MirRvalue::AddressOf(place) | MirRvalue::Load(place) => {
            verify_place(function, place, local_count)
        }
    }
}

pub(super) fn verify_memory_intrinsic(
    function: &MirFunction,
    intrinsic: &MirMemoryIntrinsic,
    local_count: u32,
) -> Result<(), MirVerifyError> {
    match intrinsic {
        MirMemoryIntrinsic::Copy { dest, src, len }
        | MirMemoryIntrinsic::Move { dest, src, len } => {
            verify_operand(function, dest, local_count)?;
            verify_operand(function, src, local_count)?;
            verify_operand(function, len, local_count)
        }
        MirMemoryIntrinsic::Set { dest, val, len } => {
            verify_operand(function, dest, local_count)?;
            verify_operand(function, val, local_count)?;
            verify_operand(function, len, local_count)
        }
    }
}
