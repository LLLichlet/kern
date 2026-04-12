use super::CodeGenerator;
use crate::basic_block::BasicBlock;
use crate::intrinsics::Intrinsic;
use crate::llvm_sys::core::LLVMSetWeak;
use crate::types::BasicTypeEnum;
use crate::values::{AsValueRef, BasicValueEnum, FunctionValue, PointerValue};
use kernc_ast::{AssignmentOperator, BinaryOperator, UnaryOperator};
use kernc_mir::{
    MirAggregateKind, MirBitIntrinsicKind, MirBlockId, MirBody, MirCallTarget, MirCastKind,
    MirConst, MirFunction, MirGlobal, MirInlineAsm, MirInstruction, MirLocalId, MirMemoryIntrinsic,
    MirOperand, MirPlace, MirProjectionKind, MirRvalue, MirSimdBinaryIntrinsicKind,
    MirSimdReduceKind, MirSimdUnaryIntrinsicKind, MirSliceBase, MirTerminator,
};
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{AtomicOrdering, AtomicRmwOp, Span};
use std::collections::HashMap;

mod atomic;
mod control;
mod place;
mod rvalue;
mod simd_memory;
mod simd_ops;
mod validate;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub(crate) fn compile_mir_global(&mut self, global: &MirGlobal) {
        if global.is_extern {
            return;
        }

        let Some(global_val) =
            self.lookup_declared_global(global.id, kernc_utils::Span::default(), &global.name)
        else {
            return;
        };

        if let Some(init) = &global.init {
            let const_val = self
                .compile_mir_static_init(init)
                .unwrap_or_else(|| self.get_llvm_type(global.ty).const_zero());
            global_val.set_initializer(&const_val);
        } else {
            let llvm_ty = self.get_llvm_type(global.ty);
            global_val.set_initializer(&llvm_ty.const_zero());
        }
    }

    pub(crate) fn compile_mir_function(&mut self, function: &MirFunction) {
        let Some(body) = &function.body else {
            return;
        };
        if !self.validate_mir_body_codegen_ready(function, body) {
            return;
        }

        let Some(llvm_func) = self.functions.get(&function.id).copied() else {
            self.sess.emit_ice(
                Span::default(),
                format!(
                    "Kern ICE (Codegen): MIR function MonoId {:?} was not declared before compilation.",
                    function.id
                ),
            );
            return;
        };

        let saved_locals = std::mem::take(&mut self.locals);
        let saved_mir_locals = std::mem::take(&mut self.mir_locals);
        let saved_loop_targets = std::mem::take(&mut self.loop_targets);
        let saved_insert_block = self.builder.get_insert_block();

        let entry_block = self.context.append_basic_block(llvm_func, "entry");
        self.builder.position_at_end(entry_block);

        let mut next_param_index = 0usize;
        for local in &body.locals {
            let local_name = self.resolve_symbol(local.name).to_string();
            let prefix = match local.kind {
                kernc_mir::MirLocalKind::Param => "arg",
                kernc_mir::MirLocalKind::Let => "let",
            };
            let local_llvm_ty = self.get_llvm_type(local.ty);
            let alloca =
                self.create_entry_block_alloca(local_llvm_ty, &format!("{prefix}_{local_name}"));
            self.mir_locals.insert(local.id, alloca);

            if matches!(local.kind, kernc_mir::MirLocalKind::Param) {
                let Some(param_val) =
                    self.function_param_value(llvm_func, next_param_index, &function.name)
                else {
                    self.restore_codegen_context(
                        saved_locals,
                        saved_mir_locals,
                        saved_loop_targets,
                        saved_insert_block,
                    );
                    return;
                };
                self.builder.build_store(alloca, param_val).unwrap();
                next_param_index += 1;
            }
        }

        let mut llvm_blocks = HashMap::new();
        for block in &body.blocks {
            llvm_blocks.insert(
                block.id,
                self.context
                    .append_basic_block(llvm_func, &format!("mir_bb{}", block.id.0)),
            );
        }

        let Some(entry_target) = llvm_blocks.get(&body.entry).copied() else {
            self.sess.emit_ice(
                Span::default(),
                format!(
                    "Kern ICE (Codegen): MIR entry block {:?} missing for function `{}`.",
                    body.entry, function.name
                ),
            );
            self.restore_codegen_context(
                saved_locals,
                saved_mir_locals,
                saved_loop_targets,
                saved_insert_block,
            );
            return;
        };
        self.builder
            .build_unconditional_branch(entry_target)
            .unwrap();

        for block in &body.blocks {
            let Some(llvm_block) = llvm_blocks.get(&block.id).copied() else {
                continue;
            };
            self.builder.position_at_end(llvm_block);

            for instruction in &block.instructions {
                self.compile_mir_instruction(body, instruction);
                if self.current_block_is_terminated() {
                    break;
                }
            }

            if !self.current_block_is_terminated() {
                self.compile_mir_terminator(body, function, &llvm_blocks, &block.terminator);
            }
        }

        self.restore_codegen_context(
            saved_locals,
            saved_mir_locals,
            saved_loop_targets,
            saved_insert_block,
        );
    }
}
