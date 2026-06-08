//! Alloca cleanup and diagnostics.
//!
//! This module scans generated LLVM for remaining stack allocations and reports
//! cleanup statistics/names so regressions in promotion or lowering are visible
//! in compile reports.

use super::CodeGenerator;
use super::accumulate_alloca_site;
use crate::basic_block::BasicBlock;
use crate::types::BasicTypeEnum;
use crate::values::{BasicValueEnum, FunctionValue, PointerValue};

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    fn record_alloca_site(&mut self, name: &str) {
        accumulate_alloca_site(&mut self.alloca_stats, name);
    }

    fn current_insert_block(&mut self, context: &str) -> Option<BasicBlock<'ctx>> {
        match self.builder.get_insert_block() {
            Some(block) => Some(block),
            None => {
                self.sess.emit_ice(
                    kernc_utils::Span::default(),
                    format!(
                        "Kern ICE (Codegen): missing insertion block while compiling {}.",
                        context
                    ),
                );
                None
            }
        }
    }

    pub(crate) fn function_param_value(
        &mut self,
        llvm_func: FunctionValue<'ctx>,
        index: usize,
        func_name: &str,
    ) -> Option<BasicValueEnum<'ctx>> {
        match llvm_func.get_nth_param(index as u32) {
            Some(param) => Some(param),
            None => {
                self.sess.emit_ice(
                    kernc_utils::Span::default(),
                    format!(
                        "Kern ICE (Codegen): function `{}` is missing LLVM parameter {}.",
                        func_name, index
                    ),
                );
                None
            }
        }
    }

    pub(crate) fn create_entry_block_alloca(
        &mut self,
        llvm_ty: BasicTypeEnum<'ctx>,
        name: &str,
    ) -> PointerValue<'ctx> {
        self.record_alloca_site(name);

        let Some(current_block) = self.current_insert_block("entry alloca") else {
            return self.context.ptr_type(Default::default()).const_zero();
        };
        let Some(current_func) = current_block.get_parent() else {
            self.sess.emit_ice(
                kernc_utils::Span::default(),
                format!(
                    "Insertion block has no parent function while allocating local `{}`",
                    name
                ),
            );
            return self.context.ptr_type(Default::default()).const_zero();
        };

        let Some(entry_block) = current_func.get_first_basic_block() else {
            self.sess.emit_ice(
                kernc_utils::Span::default(),
                format!(
                    "Function has no entry block while allocating local `{}`",
                    name
                ),
            );
            return self.context.ptr_type(Default::default()).const_zero();
        };

        match entry_block.get_first_instruction() {
            Some(first_instr) => self.alloca_builder.position_before(&first_instr),
            None => self.alloca_builder.position_at_end(entry_block),
        }

        self.alloca_builder
            .build_alloca(llvm_ty, self.llvm_name(name).as_ref())
            .unwrap()
    }

    pub(crate) fn current_function_for_simd_memory(
        &mut self,
        context: &str,
    ) -> Option<FunctionValue<'ctx>> {
        let Some(block) = self.builder.get_insert_block() else {
            self.sess.emit_ice(
                kernc_utils::Span::default(),
                format!(
                    "Kern ICE (Codegen): missing insertion block while compiling {}.",
                    context
                ),
            );
            return None;
        };
        let Some(func) = block.get_parent() else {
            self.sess.emit_ice(
                kernc_utils::Span::default(),
                format!(
                    "Kern ICE (Codegen): insertion block has no parent function while compiling {}.",
                    context
                ),
            );
            return None;
        };
        Some(func)
    }

    pub(crate) fn restore_codegen_context(
        &mut self,
        saved_locals: std::collections::HashMap<kernc_utils::SymbolId, PointerValue<'ctx>>,
        saved_mir_locals: std::collections::HashMap<kernc_mir::MirLocalId, PointerValue<'ctx>>,
        saved_loop_targets: Vec<(BasicBlock<'ctx>, BasicBlock<'ctx>)>,
        saved_insert_block: Option<BasicBlock<'ctx>>,
    ) {
        self.locals = saved_locals;
        self.mir_locals = saved_mir_locals;
        self.loop_targets = saved_loop_targets;
        if let Some(block) = saved_insert_block {
            self.builder.position_at_end(block);
        } else {
            self.builder.clear_insertion_position();
        }
    }
}
