//! MIR structural verifier.
//!
//! The verifier checks that dense block/local slots match their ids, all
//! references stay in range, and rvalues/terminators are structurally valid.
//! Passes and tests use this as a cheap guard after MIR rewrites.

mod refs;
mod rvalue;

use crate::{MirBlockId, MirBody, MirFunction, MirInstruction, MirModule, MirTerminator};
use refs::{verify_block_ref, verify_place};
use rvalue::{verify_memory_intrinsic, verify_rvalue};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirVerifyError {
    pub function: String,
    pub message: String,
}

pub fn verify_module(module: &MirModule) -> Result<(), MirVerifyError> {
    for function in &module.functions {
        let Some(body) = &function.body else {
            continue;
        };
        verify_body(function, body)?;
    }
    Ok(())
}

fn verify_body(function: &MirFunction, body: &MirBody) -> Result<(), MirVerifyError> {
    let local_count = body.locals.len() as u32;
    let block_count = body.blocks.len() as u32;

    verify_entry_block(function, body.entry, block_count)?;
    verify_local_slots(function, body)?;
    verify_block_slots(function, body)?;

    for block in &body.blocks {
        for instruction in &block.instructions {
            verify_instruction(function, &instruction.kind, local_count)?;
        }
        verify_terminator(function, &block.terminator.kind, local_count, block_count)?;
    }

    Ok(())
}

fn verify_entry_block(
    function: &MirFunction,
    entry: MirBlockId,
    block_count: u32,
) -> Result<(), MirVerifyError> {
    if entry.0 < block_count {
        return Ok(());
    }
    Err(MirVerifyError {
        function: function.name.clone(),
        message: format!("entry block {:?} is out of range", entry),
    })
}

fn verify_local_slots(function: &MirFunction, body: &MirBody) -> Result<(), MirVerifyError> {
    for (index, local) in body.locals.iter().enumerate() {
        let expected = crate::MirLocalId(index as u32);
        if local.id != expected {
            return Err(MirVerifyError {
                function: function.name.clone(),
                message: format!("local id {:?} does not match slot {:?}", local.id, expected),
            });
        }
    }
    Ok(())
}

fn verify_block_slots(function: &MirFunction, body: &MirBody) -> Result<(), MirVerifyError> {
    for (index, block) in body.blocks.iter().enumerate() {
        let expected = MirBlockId(index as u32);
        if block.id != expected {
            return Err(MirVerifyError {
                function: function.name.clone(),
                message: format!("block id {:?} does not match slot {:?}", block.id, expected),
            });
        }
    }
    Ok(())
}

fn verify_instruction(
    function: &MirFunction,
    instruction: &MirInstruction,
    local_count: u32,
) -> Result<(), MirVerifyError> {
    match instruction {
        MirInstruction::Let { place, init } => {
            verify_place(function, place, local_count)?;
            verify_rvalue(function, init, local_count)?;
        }
        MirInstruction::Assign { place, value, .. } => {
            verify_place(function, place, local_count)?;
            verify_rvalue(function, value, local_count)?;
        }
        MirInstruction::Memory(intrinsic) => {
            verify_memory_intrinsic(function, intrinsic, local_count)?;
        }
        MirInstruction::InlineAsm(asm) => {
            for input in &asm.input_args {
                refs::verify_operand(function, input, local_count)?;
            }
            for output in &asm.output_ptrs {
                refs::verify_operand(function, output, local_count)?;
            }
        }
        MirInstruction::SimdStore { ptr, value, .. } => {
            refs::verify_operand(function, ptr, local_count)?;
            refs::verify_operand(function, value, local_count)?;
        }
        MirInstruction::SimdMaskedStore {
            ptr, mask, value, ..
        } => {
            refs::verify_operand(function, ptr, local_count)?;
            refs::verify_operand(function, mask, local_count)?;
            refs::verify_operand(function, value, local_count)?;
        }
        MirInstruction::SimdScatter {
            ptr,
            indices,
            value,
        } => {
            refs::verify_operand(function, ptr, local_count)?;
            refs::verify_operand(function, indices, local_count)?;
            refs::verify_operand(function, value, local_count)?;
        }
        MirInstruction::SimdMaskedScatter {
            ptr,
            indices,
            mask,
            value,
        } => {
            refs::verify_operand(function, ptr, local_count)?;
            refs::verify_operand(function, indices, local_count)?;
            refs::verify_operand(function, mask, local_count)?;
            refs::verify_operand(function, value, local_count)?;
        }
        MirInstruction::AtomicStore { ptr, value, .. } => {
            refs::verify_operand(function, ptr, local_count)?;
            refs::verify_operand(function, value, local_count)?;
        }
        MirInstruction::Fence { .. } => {}
        MirInstruction::Trap | MirInstruction::Breakpoint => {}
        MirInstruction::Eval(rvalue) | MirInstruction::Defer(rvalue) => {
            verify_rvalue(function, rvalue, local_count)?;
        }
    }
    Ok(())
}

fn verify_terminator(
    function: &MirFunction,
    terminator: &MirTerminator,
    local_count: u32,
    block_count: u32,
) -> Result<(), MirVerifyError> {
    match terminator {
        MirTerminator::Goto(target) => verify_block_ref(function, *target, block_count),
        MirTerminator::Branch {
            cond,
            then_block,
            else_block,
        } => {
            verify_rvalue(function, cond, local_count)?;
            verify_block_ref(function, *then_block, block_count)?;
            verify_block_ref(function, *else_block, block_count)?;
            Ok(())
        }
        MirTerminator::Switch {
            target,
            cases,
            default_block,
        } => {
            verify_rvalue(function, target, local_count)?;
            for case in cases {
                verify_block_ref(function, case.block, block_count)?;
            }
            if let Some(default_block) = default_block {
                verify_block_ref(function, *default_block, block_count)?;
            }
            Ok(())
        }
        MirTerminator::Return(value) => {
            if let Some(value) = value {
                verify_rvalue(function, value, local_count)?;
            }
            Ok(())
        }
        MirTerminator::Unreachable => Ok(()),
    }
}
