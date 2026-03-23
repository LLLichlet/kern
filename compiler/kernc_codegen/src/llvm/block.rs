use super::CodeGenerator;
use inkwell::types::BasicTypeEnum;
use inkwell::values::BasicValueEnum;
use kernc_mast::{MastBlock, MastFunction, MastStmt};

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub fn compile_function(&mut self, func: &MastFunction) {
        let llvm_func = self.functions.get(&func.id).unwrap().clone();

        // ==========================================
        // 1. 现场保护 (Save Caller Context)
        // 完美解决泛型单态化和按需编译导致的重入污染问题
        // ==========================================
        let saved_locals = std::mem::take(&mut self.locals);
        let saved_loop_targets = std::mem::take(&mut self.loop_targets);
        let saved_insert_block = self.builder.get_insert_block();

        // 2. 建立新函数的环境
        let entry_block = self.context.append_basic_block(llvm_func, "entry");
        self.builder.position_at_end(entry_block);

        // 分配参数
        for (i, param) in func.params.iter().enumerate() {
            let param_val = llvm_func.get_nth_param(i as u32).unwrap();
            let param_ty = self.get_llvm_type(param.ty);

            let alloca = self.create_entry_block_alloca(param_ty, &format!("arg_{}", param.name.0));
            self.builder.build_store(alloca, param_val).unwrap();
            self.locals.insert(param.name, alloca);
        }

        // 3. 编译函数体 (Block)
        if let Some(body) = &func.body {
            let block_res = self.compile_block(body);

            let current_block = self.builder.get_insert_block().unwrap();
            if current_block.get_terminator().is_none() {
                // 自动生成 ret 指令 (拦截虚假的 Void 返回值)
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
        // 4. 现场恢复 (Restore Caller Context)
        // 保证宿主函数的 defer、break 和后续 IR 生成完全正常
        // ==========================================
        self.locals = saved_locals;
        self.loop_targets = saved_loop_targets;
        if let Some(block) = saved_insert_block {
            self.builder.position_at_end(block);
        } else {
            self.builder.clear_insertion_position();
        }
    }

    pub(super) fn compile_block(&mut self, block: &MastBlock) -> Option<BasicValueEnum<'ctx>> {
        // 1. 执行普通语句
        for stmt in &block.stmts {
            let current_block = self.builder.get_insert_block().unwrap();
            if current_block.get_terminator().is_some() {
                break;
            }

            match stmt {
                MastStmt::Let {
                    name,
                    ty,
                    is_mut: _,
                    init,
                } => {
                    let init_val = self.compile_expr(init);
                    let llvm_ty = self.get_llvm_type(*ty);
                    // 无论可不可变，统一 alloca，交给 LLVM 的 mem2reg pass 去做寄存器提升优化
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

        let current_block = self.builder.get_insert_block().unwrap();
        if current_block.get_terminator().is_some() {
            return None;
        }

        // 2. 求出返回值，用 SSA 寄存器暂存
        let mut result_val = None;
        if let Some(result_expr) = &block.result {
            result_val = Some(self.compile_expr(result_expr));
        }

        // 3. 在求出返回值之后，块退出之前，执行所有的 defer
        for defer_expr in &block.defers {
            self.compile_expr(defer_expr);
        }

        // 4. Yield 这个在 defer 执行前就已经算好的值
        result_val
    }

    /// 在当前函数的 entry block 首部安全地分配局部变量内存。
    /// 这样可以避免在循环内部调用 alloca 导致的栈溢出。
    pub(crate) fn create_entry_block_alloca(
        &self,
        llvm_ty: BasicTypeEnum<'ctx>,
        name: &str,
    ) -> inkwell::values::PointerValue<'ctx> {
        let builder = self.context.create_builder();

        // 获取当前正在构建的函数
        let current_block = self.builder.get_insert_block().unwrap();
        let current_func = current_block.get_parent().unwrap();

        // 获取该函数的 entry block
        let entry_block = current_func.get_first_basic_block().unwrap();

        // 将插入点设置在 entry block 的第一条指令之前
        match entry_block.get_first_instruction() {
            Some(first_instr) => builder.position_before(&first_instr),
            None => builder.position_at_end(entry_block),
        }

        builder.build_alloca(llvm_ty, name).unwrap()
    }
}
