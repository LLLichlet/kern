use inkwell::builder::Builder;
use inkwell::context::Context as LlvmContext;
use inkwell::module::Module as LlvmModule;
use inkwell::targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target};
use inkwell::types::StructType;
use inkwell::values::{FunctionValue, GlobalValue, PointerValue};
use std::collections::HashMap;

use kernc_mast::*;
use kernc_sema::def::{Def, DefId};
use kernc_sema::ty::{TypeRegistry, TypeId};
use kernc_utils::Session;
use kernc_utils::config::OptLevel;

mod block;
mod decl;
mod expr;
mod types;

pub struct CodeGenerator<'ctx, 'a> {
    pub context: &'ctx LlvmContext,
    pub builder: Builder<'ctx>,
    pub module: LlvmModule<'ctx>,

    pub sess: &'a mut Session,
    pub type_registry: &'a TypeRegistry,
    pub ctx_defs: &'a Vec<Def>,

    pub structs: HashMap<MonoId, StructType<'ctx>>,
    pub union_ids: std::collections::HashSet<MonoId>,
    pub globals: HashMap<MonoId, GlobalValue<'ctx>>,
    pub functions: HashMap<MonoId, FunctionValue<'ctx>>,

    pub locals: HashMap<kernc_utils::SymbolId, PointerValue<'ctx>>,
    pub loop_targets: Vec<(
        inkwell::basic_block::BasicBlock<'ctx>,
        inkwell::basic_block::BasicBlock<'ctx>,
    )>,
    pub asm_dialect: inkwell::InlineAsmDialect,

    pub def_mono_map: HashMap<(DefId, Vec<TypeId>), MonoId>,
    pub adt_union_map: HashMap<MonoId, MonoId>,
    pub anon_struct_map: HashMap<TypeId, MonoId>,
}

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub fn new(
        context: &'ctx LlvmContext,
        module_name: &str,
        sess: &'a mut Session,
        type_registry: &'a TypeRegistry,
        ctx_defs: &'a Vec<Def>,
    ) -> Self {
        Self {
            context,
            builder: context.create_builder(),
            module: context.create_module(module_name),
            sess,
            type_registry,
            ctx_defs,
            structs: HashMap::new(),
            union_ids: std::collections::HashSet::new(),
            globals: HashMap::new(),
            functions: HashMap::new(),
            locals: HashMap::new(),
            loop_targets: Vec::new(),
            asm_dialect: inkwell::InlineAsmDialect::Intel,
            def_mono_map: HashMap::new(),
            adt_union_map: HashMap::new(),
            anon_struct_map: HashMap::new(),
        }
    }

    pub fn compile(&mut self, module: &MastModule) {
        
        self.def_mono_map = module.def_mono_map.clone();
        self.adt_union_map = module.adt_union_map.clone();
        self.anon_struct_map = module.anon_struct_map.clone();

        self.declare_structs(&module.structs);
        self.declare_globals(&module.globals);
        self.declare_functions(&module.functions);

        for global in &module.globals {
            self.compile_global(global);
        }

        for function in &module.functions {
            if function.body.is_some() {
                self.compile_function(function);
            }
        }
    }

    pub fn print_ir(&self) {
        self.module.print_to_stderr();
    }

    pub fn emit_to_file(
        &self,
        target_triple_str: &str,
        output_path: &str,
        opt_level: OptLevel,
    ) -> Result<(), String> {
        let path = std::path::Path::new(output_path);

        // 策略 A：Windows 环境防崩溃降级方案 (输出 LLVM IR)
        if target_triple_str.contains("windows") {
            self.module.print_to_file(path).map_err(|e| e.to_string())?;
            return Ok(());
        }

        // 策略 B：Linux / macOS 标准原生编译方案 (输出 Object 文件)
        Target::initialize_native(&InitializationConfig::default())
            .map_err(|e| format!("Failed to initialize native target: {}", e))?;
        Target::initialize_all(&InitializationConfig::default());

        // 2. 解析目标架构三元组
        let triple = inkwell::targets::TargetTriple::create(target_triple_str);

        let target = Target::from_triple(&triple).map_err(|e| e.to_string())?;

        // 动态映射 Kern 优化等级到 LLVM 优化等级
        let llvm_opt_level = match opt_level {
            OptLevel::O0 => inkwell::OptimizationLevel::None,
            OptLevel::O1 => inkwell::OptimizationLevel::Less,
            OptLevel::O2 => inkwell::OptimizationLevel::Default,
            OptLevel::O3 => inkwell::OptimizationLevel::Aggressive,
        };

        // 3. 创建目标机器实例 (配置优化级别、重定位模式等)
        let target_machine = target
            .create_target_machine(
                &triple,
                "generic", // CPU 类型
                "",        // 特性
                llvm_opt_level,
                RelocMode::Default,
                CodeModel::Default,
            )
            .ok_or("Failed to create target machine")?;

        // 4. 将目标机器的数据布局 (Enum Layout) 和三元组写入当前 Module
        self.module
            .set_data_layout(&target_machine.get_target_data().get_data_layout());
        self.module.set_triple(&triple);

        if let Err(err) = self.module.verify() {
            // 如果 IR 有问题，它会打印出极其详细的错误信息（比如哪一行的 PHI 节点类型不对）
            eprintln!("LLVM IR Verification Failed:\n{}", err.to_string());
            // 顺便把畸形的 IR 打印出来，方便肉眼对比
            self.print_ir();
            return Err("Invalid LLVM IR generated".to_string());
        }

        // 5. 触发 LLVM 后端，直接将 IR 编译为二进制的 Object (.o) 文件
        let path = std::path::Path::new(output_path);
        target_machine
            .write_to_file(&self.module, FileType::Object, path)
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    pub fn resolve_symbol(&self, sym: kernc_utils::SymbolId) -> &str {
        self.sess.interner.resolve(sym).unwrap_or("<unknown>")
    }
}
