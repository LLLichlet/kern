use crate::codegen::CodeGenerator;
use crate::basic_block::BasicBlock;
use crate::types::BasicTypeEnum;
use crate::values::{BasicValue, BasicValueEnum, FunctionValue};
use kernc_mast::{MastBlock, MastExpr, MastSwitchCase};
use kernc_sema::ty::TypeId;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    fn current_function_for_control(
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

    fn active_loop_target(
        &mut self,
        context: &str,
    ) -> Option<(BasicBlock<'ctx>, BasicBlock<'ctx>)> {
        match self.loop_targets.last().copied() {
            Some(targets) => Some(targets),
            None => {
                self.sess.emit_ice(
                    kernc_utils::Span::default(),
                    format!(
                        "Kern ICE (Codegen): {} encountered outside of any active loop target.",
                        context
                    ),
                );
                None
            }
        }
    }

    pub(crate) fn compile_if(
        &mut self,
        cond: &MastExpr,
        then_branch: &MastBlock,
        else_branch: Option<&MastBlock>,
        expr_ty: TypeId,
        expected_llvm_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let cond_val = self.compile_expr(cond).into_int_value();
        let Some(parent_func) = self.current_function_for_control("if expression") else {
            return self.context.i8_type().const_zero().into();
        };

        let then_bb = self.context.append_basic_block(parent_func, "then");
        let merge_bb = self.context.append_basic_block(parent_func, "ifcont");
        let else_bb = else_branch.map(|_| self.context.append_basic_block(parent_func, "else"));
        let false_bb = else_bb.unwrap_or(merge_bb);

        self.builder
            .build_conditional_branch(cond_val, then_bb, false_bb)
            .unwrap();

        let mut incoming = Vec::new();
        let mut merge_reachable = else_branch.is_none();

        self.builder.position_at_end(then_bb);
        let then_result = self.compile_block(then_branch);
        let then_exit_bb = self.builder.get_insert_block().unwrap();
        if then_exit_bb.get_terminator().is_none() {
            self.builder.build_unconditional_branch(merge_bb).unwrap();
            merge_reachable = true;
            if let Some(val) = then_result {
                incoming.push((val, then_exit_bb));
            }
        }

        if let (Some(else_bb), Some(else_branch)) = (else_bb, else_branch) {
            self.builder.position_at_end(else_bb);
            let else_result = self.compile_block(else_branch);
            let else_exit_bb = self.builder.get_insert_block().unwrap();
            if else_exit_bb.get_terminator().is_none() {
                self.builder.build_unconditional_branch(merge_bb).unwrap();
                merge_reachable = true;
                if let Some(val) = else_result {
                    incoming.push((val, else_exit_bb));
                }
            }
        }

        self.builder.position_at_end(merge_bb);
        if !merge_reachable {
            self.builder.build_unreachable().unwrap();
            self.get_undef_val(expected_llvm_ty)
        } else if expr_ty != TypeId::VOID && incoming.is_empty() {
            self.sess.emit_ice(
                cond.span,
                "Kern ICE (Codegen): reachable `if` expression with non-void type produced no incoming values.",
            );
            self.get_undef_val(expected_llvm_ty)
        } else if expr_ty != TypeId::VOID {
            let phi = self.builder.build_phi(expected_llvm_ty, "iftmp").unwrap();
            let mut incoming_refs = Vec::new();
            for (val, bb) in &incoming {
                incoming_refs.push((val as &dyn BasicValue<'ctx>, *bb));
            }
            phi.add_incoming(&incoming_refs);
            phi.as_basic_value()
        } else {
            self.context.i8_type().const_zero().into()
        }
    }

    pub(crate) fn compile_loop(
        &mut self,
        body: &MastBlock,
        latch: Option<&MastBlock>,
        expr_ty: TypeId,
        expected_llvm_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let Some(parent_func) = self.current_function_for_control("loop expression") else {
            return self.context.i8_type().const_zero().into();
        };

        let loop_bb = self.context.append_basic_block(parent_func, "loop");
        let latch_bb = self.context.append_basic_block(parent_func, "latch");
        let merge_bb = self.context.append_basic_block(parent_func, "loopcont");

        self.builder.build_unconditional_branch(loop_bb).unwrap();
        self.builder.position_at_end(loop_bb);

        self.loop_targets.push((latch_bb, merge_bb));

        self.compile_block(body);

        let loop_exit_bb = self.builder.get_insert_block().unwrap();
        if loop_exit_bb.get_terminator().is_none() {
            self.builder.build_unconditional_branch(latch_bb).unwrap();
        }

        self.loop_targets.pop();

        self.builder.position_at_end(latch_bb);
        if let Some(latch_block) = latch {
            self.compile_block(latch_block);
        }
        let latch_exit_bb = self.builder.get_insert_block().unwrap();
        if latch_exit_bb.get_terminator().is_none() {
            self.builder.build_unconditional_branch(loop_bb).unwrap();
        }

        self.builder.position_at_end(merge_bb);
        if expr_ty != TypeId::VOID {
            self.builder.build_unreachable().unwrap();
            self.get_undef_val(expected_llvm_ty)
        } else {
            self.context.i8_type().const_zero().into()
        }
    }

    pub(crate) fn compile_break(&mut self) -> BasicValueEnum<'ctx> {
        let Some((_, merge_bb)) = self.active_loop_target("`break`") else {
            return self.context.i8_type().const_zero().into();
        };
        self.builder.build_unconditional_branch(merge_bb).unwrap();
        self.context.i8_type().const_zero().into()
    }

    pub(crate) fn compile_continue(&mut self) -> BasicValueEnum<'ctx> {
        let Some((loop_bb, _)) = self.active_loop_target("`continue`") else {
            return self.context.i8_type().const_zero().into();
        };
        self.builder.build_unconditional_branch(loop_bb).unwrap();
        self.context.i8_type().const_zero().into()
    }

    pub(crate) fn compile_switch(
        &mut self,
        target: &MastExpr,
        cases: &[MastSwitchCase],
        default_case: Option<&MastBlock>,
        expr_ty: TypeId,
        expected_llvm_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let target_val = self.compile_expr(target).into_int_value();
        let Some(parent_func) = self.current_function_for_control("switch expression") else {
            return self.context.i8_type().const_zero().into();
        };

        let merge_bb = self.context.append_basic_block(parent_func, "switchcont");
        let default_bb = self.context.append_basic_block(parent_func, "default");
        let mut case_blocks = Vec::new();
        let mut llvm_cases = Vec::new();

        for case in cases {
            let case_bb = self.context.append_basic_block(parent_func, "case");
            case_blocks.push(case_bb);
            for &val in &case.values {
                let int_val = target_val.get_type().const_int(val as u64, false);
                llvm_cases.push((int_val, case_bb));
            }
        }

        self.builder
            .build_switch(target_val, default_bb, &llvm_cases)
            .unwrap();

        let mut incoming = Vec::new();
        let mut merge_reachable = false;

        self.builder.position_at_end(default_bb);
        if let Some(def_block) = default_case {
            let def_val = self.compile_block(def_block);
            let def_exit_bb = self.builder.get_insert_block().unwrap();
            if def_exit_bb.get_terminator().is_none() {
                self.builder.build_unconditional_branch(merge_bb).unwrap();
                merge_reachable = true;
                if let Some(val) = def_val {
                    incoming.push((val, def_exit_bb));
                }
            }
        } else {
            self.builder.build_unreachable().unwrap();
        }

        for (i, case) in cases.iter().enumerate() {
            self.builder.position_at_end(case_blocks[i]);
            let case_val = self.compile_block(&case.body);
            let case_exit_bb = self.builder.get_insert_block().unwrap();

            if case_exit_bb.get_terminator().is_none() {
                self.builder.build_unconditional_branch(merge_bb).unwrap();
                merge_reachable = true;
                if let Some(val) = case_val {
                    incoming.push((val, case_exit_bb));
                }
            }
        }

        self.builder.position_at_end(merge_bb);
        if !merge_reachable {
            self.builder.build_unreachable().unwrap();
            self.get_undef_val(expected_llvm_ty)
        } else if expr_ty != TypeId::VOID && incoming.is_empty() {
            self.sess.emit_ice(
                target.span,
                "Kern ICE (Codegen): reachable `match` expression with non-void type produced no incoming values.",
            );
            self.get_undef_val(expected_llvm_ty)
        } else if expr_ty != TypeId::VOID {
            let phi = self
                .builder
                .build_phi(expected_llvm_ty, "switchtmp")
                .unwrap();
            let mut incoming_refs = Vec::new();
            for (val, bb) in &incoming {
                incoming_refs.push((val as &dyn BasicValue<'ctx>, *bb));
            }
            phi.add_incoming(&incoming_refs);
            phi.as_basic_value()
        } else {
            self.context.i8_type().const_zero().into()
        }
    }

    pub(crate) fn compile_block_expr(&mut self, block: &MastBlock) -> BasicValueEnum<'ctx> {
        if let Some(res) = self.compile_block(block) {
            res
        } else {
            self.context.i8_type().const_zero().into()
        }
    }

    pub(crate) fn compile_return(&mut self, ret_val: Option<&MastExpr>) -> BasicValueEnum<'ctx> {
        if let Some(val) = ret_val {
            let llvm_val = self.compile_expr(val);
            if self.is_void_type(val.ty) {
                self.builder.build_return(None).unwrap();
            } else {
                self.builder.build_return(Some(&llvm_val)).unwrap();
            }
        } else {
            self.builder.build_return(None).unwrap();
        }
        self.context.i8_type().const_zero().into()
    }
}
