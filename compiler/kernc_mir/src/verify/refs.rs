//! MIR reference verifier helpers.
//!
//! Places can contain nested projections through locals, fields, indices, and
//! dereferences. These helpers validate that every local reference is in range
//! before rvalue and terminator verification proceeds.

use super::MirVerifyError;
use crate::{MirBlockId, MirFunction, MirLocalId, MirOperand, MirPlace, MirSliceBase};

pub(super) fn verify_block_ref(
    function: &MirFunction,
    block: MirBlockId,
    block_count: u32,
) -> Result<(), MirVerifyError> {
    if block.0 < block_count {
        return Ok(());
    }
    Err(MirVerifyError {
        function: function.name.clone(),
        message: format!("block ref {:?} is out of range", block),
    })
}

fn verify_local_ref(
    function: &MirFunction,
    local: MirLocalId,
    local_count: u32,
) -> Result<(), MirVerifyError> {
    if local.0 < local_count {
        return Ok(());
    }
    Err(MirVerifyError {
        function: function.name.clone(),
        message: format!("local ref {:?} is out of range", local),
    })
}

pub(super) fn verify_operand(
    function: &MirFunction,
    operand: &MirOperand,
    local_count: u32,
) -> Result<(), MirVerifyError> {
    match operand {
        MirOperand::Local(local) => verify_local_ref(function, *local, local_count),
        MirOperand::Const(_) => Ok(()),
    }
}

pub(super) fn verify_place(
    function: &MirFunction,
    place: &MirPlace,
    local_count: u32,
) -> Result<(), MirVerifyError> {
    match place {
        MirPlace::Local(local) => verify_local_ref(function, *local, local_count),
        MirPlace::Global(_) => Ok(()),
        MirPlace::Deref(operand) => verify_operand(function, operand, local_count),
        MirPlace::Field { base, .. } => verify_place(function, base, local_count),
        MirPlace::Index { base, index } => {
            verify_place(function, base, local_count)?;
            verify_operand(function, index, local_count)
        }
    }
}

pub(super) fn verify_slice_base(
    function: &MirFunction,
    base: &MirSliceBase,
    local_count: u32,
) -> Result<(), MirVerifyError> {
    match base {
        MirSliceBase::Operand(operand) => verify_operand(function, operand, local_count),
        MirSliceBase::Place(place) => verify_place(function, place, local_count),
    }
}
