use super::CodeGenerator;
use super::accumulate_alloca_site;
use crate::basic_block::BasicBlock;
use crate::types::BasicTypeEnum;
use crate::values::{BasicValueEnum, FunctionValue, PointerValue};
use kernc_mast::{MastBlock, MastFunction, MastStmt};

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

    fn function_param_value(
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

    pub(crate) fn compile_function(&mut self, func: &MastFunction) {
        let Some(llvm_func) = self.functions.get(&func.id).copied() else {
            self.sess.emit_ice(
                kernc_utils::Span::default(),
                format!(
                    "Function MonoId {:?} was not declared before compilation",
                    func.id
                ),
            );
            return;
        };

        // ==========================================
        // 1. Save the caller's codegen context.
        // ==========================================
        let saved_locals = std::mem::take(&mut self.locals);
        let saved_loop_targets = std::mem::take(&mut self.loop_targets);
        let saved_insert_block = self.builder.get_insert_block();

        // 2. Set up the new function environment.
        let entry_block = self.context.append_basic_block(llvm_func, "entry");
        self.builder.position_at_end(entry_block);

        // Materialize parameters into allocas.
        for (i, param) in func.params.iter().enumerate() {
            let Some(param_val) = self.function_param_value(llvm_func, i, &func.name) else {
                self.restore_codegen_context(saved_locals, saved_loop_targets, saved_insert_block);
                return;
            };
            let param_ty = self.get_llvm_type(param.ty);

            let alloca = self.create_entry_block_alloca(param_ty, &format!("arg_{}", param.name.0));
            self.builder.build_store(alloca, param_val).unwrap();
            self.locals.insert(param.name, alloca);
        }

        // 3. Compile the function body block.
        if let Some(body) = &func.body {
            let block_res = self.compile_block(body);

            let Some(current_block) = self.builder.get_insert_block() else {
                self.sess.emit_ice(
                    kernc_utils::Span::default(),
                    format!(
                        "Builder lost its insertion point while compiling function `{}`",
                        func.name
                    ),
                );
                self.restore_codegen_context(saved_locals, saved_loop_targets, saved_insert_block);
                return;
            };
            if current_block.get_terminator().is_none() {
                // Emit an implicit `ret` when the body did not terminate explicitly.
                if self.is_void_type(func.ret_ty) {
                    self.builder.build_return(None).unwrap();
                } else if let Some(val) = block_res {
                    self.builder.build_return(Some(&val)).unwrap();
                } else {
                    self.builder.build_unreachable().unwrap();
                }
            }
        }

        // ==========================================
        // 4. Restore the caller's codegen context.
        // ==========================================
        self.restore_codegen_context(saved_locals, saved_loop_targets, saved_insert_block);
    }

    pub(super) fn compile_block(&mut self, block: &MastBlock) -> Option<BasicValueEnum<'ctx>> {
        // Snapshot the visible locals before entering the block.
        // Inner bindings, especially enum payload names, must not leak after block exit.
        let saved_locals = self.locals.clone();

        // 1. Emit ordinary statements.
        for stmt in &block.stmts {
            let Some(current_block) = self.current_insert_block("block statement") else {
                self.locals = saved_locals;
                return None;
            };
            if current_block.get_terminator().is_some() {
                self.locals = saved_locals;
                return None;
            }

            match stmt {
                MastStmt::Let {
                    name,
                    ty,
                    is_mut: _,
                    init,
                } => {
                    let init_val = self.compile_expr(init);
                    if self.current_block_is_terminated() {
                        self.locals = saved_locals;
                        return None;
                    }
                    let llvm_ty = self.get_llvm_type(*ty);
                    // Always alloca first and let LLVM's mem2reg pass promote when possible.
                    let alloca =
                        self.create_entry_block_alloca(llvm_ty, &format!("let_{}", name.0));
                    self.builder.build_store(alloca, init_val).unwrap();
                    self.locals.insert(*name, alloca);
                }
                MastStmt::Expr(expr) => {
                    self.compile_expr(expr);
                }
            }
        }

        let Some(current_block) = self.current_insert_block("block result") else {
            self.locals = saved_locals;
            return None;
        };
        if current_block.get_terminator().is_some() {
            self.locals = saved_locals;
            return None;
        }

        // 2. Compute the block result before running defers.
        let mut result_val = None;
        if let Some(result_expr) = &block.result {
            result_val = Some(self.compile_expr(result_expr));
            let Some(result_block) = self.current_insert_block("block result expression") else {
                self.locals = saved_locals;
                return None;
            };
            if result_block.get_terminator().is_some() {
                self.locals = saved_locals;
                return None;
            }
        }

        // 3. Run defers after computing the result but before leaving the block.
        for defer_expr in &block.defers {
            let Some(defer_block) = self.current_insert_block("block defer") else {
                self.locals = saved_locals;
                return None;
            };
            if defer_block.get_terminator().is_some() {
                self.locals = saved_locals;
                return None;
            }
            self.compile_expr(defer_expr);
        }

        // 4. Yield the value captured before defer execution.
        // Restore the outer local map so later lookups cannot resolve to stale shadowing slots.
        self.locals = saved_locals;
        result_val
    }

    /// Safely allocate local storage at the beginning of the current function's entry block.
    /// This avoids repeated loop-local `alloca` growth.
    pub(crate) fn create_entry_block_alloca(
        &mut self,
        llvm_ty: BasicTypeEnum<'ctx>,
        name: &str,
    ) -> PointerValue<'ctx> {
        self.record_alloca_site(name);
        let builder = self.context.create_builder();

        // Recover the function currently being built.
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

        // Find the function's entry block.
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

        // Insert before the first entry instruction when possible.
        match entry_block.get_first_instruction() {
            Some(first_instr) => builder.position_before(&first_instr),
            None => builder.position_at_end(entry_block),
        }

        builder.build_alloca(llvm_ty, name).unwrap()
    }

    fn restore_codegen_context(
        &mut self,
        saved_locals: std::collections::HashMap<kernc_utils::SymbolId, PointerValue<'ctx>>,
        saved_loop_targets: Vec<(BasicBlock<'ctx>, BasicBlock<'ctx>)>,
        saved_insert_block: Option<BasicBlock<'ctx>>,
    ) {
        self.locals = saved_locals;
        self.loop_targets = saved_loop_targets;
        if let Some(block) = saved_insert_block {
            self.builder.position_at_end(block);
        } else {
            self.builder.clear_insertion_position();
        }
    }
}
