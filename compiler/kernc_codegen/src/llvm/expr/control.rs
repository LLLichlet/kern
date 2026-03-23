use crate::llvm::CodeGenerator;
use inkwell::types::BasicTypeEnum;
use inkwell::values::BasicValueEnum;
use kernc_mast::{MastBlock, MastExpr, MastSwitchCase};
use kernc_sema::ty::TypeId;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub(crate) fn compile_if(
        &mut self,
        cond: &MastExpr,
        then_branch: &MastBlock,
        else_branch: Option<&MastBlock>,
        expr_ty: TypeId,
        expected_llvm_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let cond_val = self.compile_expr(cond).into_int_value();
        let parent_func = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();

        let then_bb = self.context.append_basic_block(parent_func, "then");
        let else_bb = self.context.append_basic_block(parent_func, "else");
        let merge_bb = self.context.append_basic_block(parent_func, "ifcont");

        if else_branch.is_some() {
            self.builder
                .build_conditional_branch(cond_val, then_bb, else_bb)
                .unwrap();
        } else {
            self.builder
                .build_conditional_branch(cond_val, then_bb, merge_bb)
                .unwrap();
        }

        let mut incoming = Vec::new();

        // 编译 Then 分支
        self.builder.position_at_end(then_bb);
        let then_result = self.compile_block(then_branch);
        let then_exit_bb = self.builder.get_insert_block().unwrap();
        if then_exit_bb.get_terminator().is_none() {
            self.builder.build_unconditional_branch(merge_bb).unwrap();
            // 只有真实发生跳转，才加入 PHI 节点前驱！
            if let Some(val) = then_result {
                incoming.push((val, then_exit_bb));
            }
        }

        // 编译 Else 分支
        self.builder.position_at_end(else_bb);
        let mut else_result = None;
        if let Some(eb) = else_branch {
            else_result = self.compile_block(eb);
        }
        let else_exit_bb = self.builder.get_insert_block().unwrap();
        if else_exit_bb.get_terminator().is_none() {
            self.builder.build_unconditional_branch(merge_bb).unwrap();
            // 只有真实发生跳转，才加入 PHI 节点前驱
            if let Some(val) = else_result {
                incoming.push((val, else_exit_bb));
            }
        }

        // 生成 PHI 节点
        self.builder.position_at_end(merge_bb);
        if expr_ty != TypeId::VOID && !incoming.is_empty() {
            let phi = self.builder.build_phi(expected_llvm_ty, "iftmp").unwrap();
            let mut incoming_refs = Vec::new();
            for (val, bb) in &incoming {
                incoming_refs.push((val as &dyn inkwell::values::BasicValue<'ctx>, *bb));
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
    ) -> BasicValueEnum<'ctx> {
        let parent_func = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();

        let loop_bb = self.context.append_basic_block(parent_func, "loop");
        let latch_bb = self.context.append_basic_block(parent_func, "latch");
        let merge_bb = self.context.append_basic_block(parent_func, "loopcont");

        self.builder.build_unconditional_branch(loop_bb).unwrap();
        self.builder.position_at_end(loop_bb);

        // 把 latch_bb 压入栈,这样遇到 continue 时就会跳转到 latch_bb
        self.loop_targets.push((latch_bb, merge_bb));

        self.compile_block(body);

        let loop_exit_bb = self.builder.get_insert_block().unwrap();
        if loop_exit_bb.get_terminator().is_none() {
            // 循环体自然结束，跳入 latch 块
            self.builder.build_unconditional_branch(latch_bb).unwrap();
        }

        self.loop_targets.pop();

        // --- 编译 Latch Block (步进逻辑) ---
        self.builder.position_at_end(latch_bb);
        if let Some(latch_block) = latch {
            self.compile_block(latch_block);
        }
        let latch_exit_bb = self.builder.get_insert_block().unwrap();
        if latch_exit_bb.get_terminator().is_none() {
            // 步进执行完毕，跳回循环头进行新一轮判断
            self.builder.build_unconditional_branch(loop_bb).unwrap();
        }

        // --- 编译结束后的出口 ---
        self.builder.position_at_end(merge_bb);
        self.context.i8_type().const_zero().into()
    }

    pub(crate) fn compile_break(&mut self) -> BasicValueEnum<'ctx> {
        let (_, merge_bb) = self.loop_targets.last().expect("Break outside of loop");
        self.builder.build_unconditional_branch(*merge_bb).unwrap();
        self.context.i8_type().const_zero().into()
    }

    pub(crate) fn compile_continue(&mut self) -> BasicValueEnum<'ctx> {
        let (loop_bb, _) = self.loop_targets.last().expect("Continue outside of loop");
        self.builder.build_unconditional_branch(*loop_bb).unwrap();
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
        let parent_func = self
            .builder
            .get_insert_block()
            .unwrap()
            .get_parent()
            .unwrap();

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

        // 编译 Default 分支
        self.builder.position_at_end(default_bb);
        if let Some(def_block) = default_case {
            let def_val = self.compile_block(def_block);
            let def_exit_bb = self.builder.get_insert_block().unwrap();
            if def_exit_bb.get_terminator().is_none() {
                self.builder.build_unconditional_branch(merge_bb).unwrap();
                if let Some(val) = def_val {
                    incoming.push((val, def_exit_bb));
                }
            }
        } else {
            self.builder.build_unreachable().unwrap(); // 前端已保证穷尽性
        }

        // 编译所有 Case 分支
        for (i, case) in cases.iter().enumerate() {
            self.builder.position_at_end(case_blocks[i]);
            let case_val = self.compile_block(&case.body);
            let case_exit_bb = self.builder.get_insert_block().unwrap();

            if case_exit_bb.get_terminator().is_none() {
                self.builder.build_unconditional_branch(merge_bb).unwrap();
                if let Some(val) = case_val {
                    incoming.push((val, case_exit_bb));
                }
            }
        }

        // 构建 PHI 返回值
        self.builder.position_at_end(merge_bb);
        if expr_ty != TypeId::VOID && !incoming.is_empty() {
            let phi = self
                .builder
                .build_phi(expected_llvm_ty, "switchtmp")
                .unwrap();
            let mut incoming_refs = Vec::new();
            for (val, bb) in &incoming {
                incoming_refs.push((val as &dyn inkwell::values::BasicValue<'ctx>, *bb));
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
            // 如果是 void 表达式，忽略它产生的值，强行 build_return(None)
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
