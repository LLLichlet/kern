use crate::llvm_api::{
    AsValueRef, Builder, Context as LlvmContext, FunctionValue, GlobalValue, InlineAsmDialect,
    Module as LlvmModule, PointerValue, StructType,
};
use llvm_sys::core::{
    LLVMDisposeMemoryBuffer, LLVMDisposeMessage, LLVMGetBufferSize, LLVMGetBufferStart,
    LLVMSetTarget,
};
use llvm_sys::error::{LLVMDisposeErrorMessage, LLVMErrorRef, LLVMGetErrorMessage};
use llvm_sys::target::{
    LLVM_InitializeAllAsmParsers, LLVM_InitializeAllAsmPrinters, LLVM_InitializeAllTargetInfos,
    LLVM_InitializeAllTargetMCs, LLVM_InitializeAllTargets, LLVM_InitializeNativeAsmParser,
    LLVM_InitializeNativeAsmPrinter, LLVM_InitializeNativeTarget, LLVMDisposeTargetData,
    LLVMSetModuleDataLayout,
};
use llvm_sys::target_machine::{
    LLVMCodeGenFileType, LLVMCodeGenOptLevel, LLVMCodeModel, LLVMCreateTargetDataLayout,
    LLVMCreateTargetMachine, LLVMDisposeTargetMachine, LLVMGetTargetFromTriple, LLVMRelocMode,
    LLVMTargetMachineEmitToFile, LLVMTargetMachineEmitToMemoryBuffer, LLVMTargetMachineRef,
    LLVMTargetRef,
};
use llvm_sys::transforms::pass_builder::{
    LLVMCreatePassBuilderOptions, LLVMDisposePassBuilderOptions, LLVMRunPassesOnFunction,
};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::ptr;
use std::time::{Duration, Instant};

use kernc_mast::*;
use kernc_sema::def::DefId;
use kernc_sema::ty::{TypeId, TypeRegistry};
use kernc_utils::config::OptLevel;
use kernc_utils::{Session, SymbolId};
use llvm_sys::LLVMOpcode;

mod block;
mod decl;
mod expr;
mod types;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodegenTiming {
    pub name: &'static str,
    pub duration: Duration,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodegenReport {
    pub timings: Vec<CodegenTiming>,
    pub ir_stats: IrInstructionStats,
    pub ir_hot_functions: Vec<IrFunctionStats>,
    pub alloca_stats: CodegenAllocaStats,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IrInstructionStats {
    pub functions: usize,
    pub basic_blocks: usize,
    pub instructions: usize,
    pub allocas: usize,
    pub loads: usize,
    pub stores: usize,
    pub geps: usize,
    pub calls: usize,
    pub phis: usize,
    pub branches: usize,
    pub switches: usize,
    pub returns: usize,
    pub compares: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IrCleanupStats {
    pub before: IrInstructionStats,
    pub after: IrInstructionStats,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IrFunctionStats {
    pub name: String,
    pub basic_blocks: usize,
    pub instructions: usize,
    pub allocas: usize,
    pub loads: usize,
    pub stores: usize,
    pub geps: usize,
    pub calls: usize,
    pub phis: usize,
    pub branches: usize,
    pub returns: usize,
    pub compares: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AllocaNameStat {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodegenAllocaStats {
    pub params: usize,
    pub lets: usize,
    pub addr_of_temps: usize,
    pub materialized_lvalues: usize,
    pub array_to_slice_temps: usize,
    pub union_inits: usize,
    pub data_union_inits: usize,
    pub unnamed: usize,
    pub other: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmitObjectTiming {
    pub name: &'static str,
    pub duration: Duration,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmitObjectReport {
    pub timings: Vec<EmitObjectTiming>,
    pub ir_cleanup_stats: Option<IrCleanupStats>,
    pub remaining_alloca_stats: Option<CodegenAllocaStats>,
    pub remaining_alloca_names: Vec<AllocaNameStat>,
}

pub struct CodeGenerator<'ctx, 'a> {
    context: &'ctx LlvmContext,
    builder: Builder<'ctx>,
    module: LlvmModule<'ctx>,

    sess: &'a mut Session,
    type_registry: &'a TypeRegistry,

    structs: HashMap<MonoId, StructType<'ctx>>,
    struct_fields: HashMap<MonoId, Vec<SymbolId>>,
    union_ids: std::collections::HashSet<MonoId>,
    globals: HashMap<MonoId, GlobalValue<'ctx>>,
    functions: HashMap<MonoId, FunctionValue<'ctx>>,
    function_ret_tys: HashMap<MonoId, TypeId>,
    alloca_stats: CodegenAllocaStats,

    locals: HashMap<kernc_utils::SymbolId, PointerValue<'ctx>>,
    loop_targets: Vec<(
        crate::llvm_api::BasicBlock<'ctx>,
        crate::llvm_api::BasicBlock<'ctx>,
    )>,
    asm_dialect: InlineAsmDialect,

    def_mono_map: HashMap<(DefId, Vec<TypeId>), MonoId>,
    pure_enum_tag_map: HashMap<(DefId, Vec<TypeId>), TypeId>,
    adt_union_map: HashMap<MonoId, MonoId>,
    anon_struct_map: HashMap<TypeId, MonoId>,
    anon_union_map: HashMap<TypeId, MonoId>,
    anon_enum_map: HashMap<TypeId, MonoId>,
    split_sections_for_gc: bool,
}

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    fn collect_ir_instruction_stats(&self) -> (IrInstructionStats, Vec<IrFunctionStats>) {
        let mut stats = IrInstructionStats::default();
        let mut hot_functions = Vec::new();
        let mut current_function = self.module.get_first_function();
        while let Some(function) = current_function {
            stats.functions += 1;
            let mut function_stats = IrFunctionStats {
                name: function.name(),
                ..IrFunctionStats::default()
            };
            let mut current_block = function.get_first_basic_block();
            while let Some(block) = current_block {
                stats.basic_blocks += 1;
                function_stats.basic_blocks += 1;
                let mut current_instruction = block.get_first_instruction();
                while let Some(instruction) = current_instruction {
                    stats.instructions += 1;
                    function_stats.instructions += 1;
                    match instruction.get_opcode() {
                        LLVMOpcode::LLVMAlloca => {
                            stats.allocas += 1;
                            function_stats.allocas += 1;
                        }
                        LLVMOpcode::LLVMLoad => {
                            stats.loads += 1;
                            function_stats.loads += 1;
                        }
                        LLVMOpcode::LLVMStore => {
                            stats.stores += 1;
                            function_stats.stores += 1;
                        }
                        LLVMOpcode::LLVMGetElementPtr => {
                            stats.geps += 1;
                            function_stats.geps += 1;
                        }
                        LLVMOpcode::LLVMCall | LLVMOpcode::LLVMInvoke | LLVMOpcode::LLVMCallBr => {
                            stats.calls += 1;
                            function_stats.calls += 1;
                        }
                        LLVMOpcode::LLVMPHI => {
                            stats.phis += 1;
                            function_stats.phis += 1;
                        }
                        LLVMOpcode::LLVMBr => {
                            stats.branches += 1;
                            function_stats.branches += 1;
                        }
                        LLVMOpcode::LLVMSwitch => stats.switches += 1,
                        LLVMOpcode::LLVMRet => {
                            stats.returns += 1;
                            function_stats.returns += 1;
                        }
                        LLVMOpcode::LLVMICmp | LLVMOpcode::LLVMFCmp => {
                            stats.compares += 1;
                            function_stats.compares += 1;
                        }
                        _ => {}
                    }
                    current_instruction = instruction.get_next_instruction();
                }
                current_block = block.get_next_basic_block();
            }
            if function_stats.basic_blocks != 0 {
                hot_functions.push(function_stats);
            }
            current_function = function.get_next_function();
        }
        hot_functions.sort_by(|lhs, rhs| {
            rhs.instructions
                .cmp(&lhs.instructions)
                .then_with(|| rhs.loads.cmp(&lhs.loads))
                .then_with(|| rhs.stores.cmp(&lhs.stores))
                .then_with(|| lhs.name.cmp(&rhs.name))
        });
        hot_functions.truncate(8);
        (stats, hot_functions)
    }

    fn collect_remaining_alloca_stats(&self) -> CodegenAllocaStats {
        let mut stats = CodegenAllocaStats::default();
        let mut current_function = self.module.get_first_function();
        while let Some(function) = current_function {
            let mut current_block = function.get_first_basic_block();
            while let Some(block) = current_block {
                let mut current_instruction = block.get_first_instruction();
                while let Some(instruction) = current_instruction {
                    if instruction.get_opcode() == LLVMOpcode::LLVMAlloca {
                        accumulate_alloca_site(&mut stats, &instruction.name());
                    }
                    current_instruction = instruction.get_next_instruction();
                }
                current_block = block.get_next_basic_block();
            }
            current_function = function.get_next_function();
        }

        stats
    }

    fn collect_remaining_alloca_names(&self) -> Vec<AllocaNameStat> {
        let mut counts = HashMap::<String, usize>::new();
        let mut current_function = self.module.get_first_function();
        while let Some(function) = current_function {
            let mut current_block = function.get_first_basic_block();
            while let Some(block) = current_block {
                let mut current_instruction = block.get_first_instruction();
                while let Some(instruction) = current_instruction {
                    if instruction.get_opcode() == LLVMOpcode::LLVMAlloca {
                        *counts.entry(instruction.name()).or_default() += 1;
                    }
                    current_instruction = instruction.get_next_instruction();
                }
                current_block = block.get_next_basic_block();
            }
            current_function = function.get_next_function();
        }

        let mut stats = counts
            .into_iter()
            .map(|(name, count)| AllocaNameStat { name, count })
            .collect::<Vec<_>>();
        stats.sort_by(|lhs, rhs| rhs.count.cmp(&lhs.count).then_with(|| lhs.name.cmp(&rhs.name)));
        stats.truncate(8);
        stats
    }

    pub fn new(
        context: &'ctx LlvmContext,
        module_name: &str,
        sess: &'a mut Session,
        type_registry: &'a TypeRegistry,
        split_sections_for_gc: bool,
    ) -> Self {
        Self {
            context,
            builder: context.create_builder(),
            module: context.create_module(module_name),
            sess,
            type_registry,
            structs: HashMap::new(),
            struct_fields: HashMap::new(),
            union_ids: std::collections::HashSet::new(),
            globals: HashMap::new(),
            functions: HashMap::new(),
            function_ret_tys: HashMap::new(),
            alloca_stats: CodegenAllocaStats::default(),
            locals: HashMap::new(),
            loop_targets: Vec::new(),
            asm_dialect: InlineAsmDialect::Intel,
            def_mono_map: HashMap::new(),
            pure_enum_tag_map: HashMap::new(),
            adt_union_map: HashMap::new(),
            anon_struct_map: HashMap::new(),
            anon_union_map: HashMap::new(),
            anon_enum_map: HashMap::new(),
            split_sections_for_gc,
        }
    }

    fn target_uses_coff_sections(&self) -> bool {
        self.sess.target.triple.to_string().contains("windows")
    }

    fn target_uses_macho_sections(&self) -> bool {
        let triple = self.sess.target.triple.to_string();
        triple.contains("darwin") || triple.contains("macosx")
    }

    fn sanitize_symbol_for_section(symbol: &str) -> String {
        symbol
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '$') {
                    ch
                } else {
                    '_'
                }
            })
            .collect()
    }

    fn gc_text_section_for_symbol(&self, symbol: &str) -> Option<String> {
        if !self.split_sections_for_gc || self.target_uses_macho_sections() {
            return None;
        }

        let symbol = Self::sanitize_symbol_for_section(symbol);
        Some(if self.target_uses_coff_sections() {
            format!(".text${symbol}")
        } else {
            format!(".text.{symbol}")
        })
    }

    fn gc_data_section_for_symbol(&self, symbol: &str, is_constant: bool) -> Option<String> {
        if !self.split_sections_for_gc || self.target_uses_macho_sections() {
            return None;
        }

        let symbol = Self::sanitize_symbol_for_section(symbol);
        Some(if self.target_uses_coff_sections() {
            if is_constant {
                format!(".rdata${symbol}")
            } else {
                format!(".data${symbol}")
            }
        } else if is_constant {
            format!(".rodata.{symbol}")
        } else {
            format!(".data.{symbol}")
        })
    }

    pub fn compile(&mut self, module: &MastModule) -> CodegenReport {
        let mut report = CodegenReport::default();

        let prepare_started = Instant::now();
        self.def_mono_map = module.def_mono_map.clone();
        self.pure_enum_tag_map = module.pure_enum_tag_map.clone();
        self.adt_union_map = module.adt_union_map.clone();
        self.anon_struct_map = module.anon_struct_map.clone();
        self.anon_union_map = module.anon_union_map.clone();
        self.anon_enum_map = module.anon_enum_map.clone();
        self.function_ret_tys = module
            .functions
            .iter()
            .map(|function| (function.id, function.ret_ty))
            .collect();
        report.timings.push(CodegenTiming {
            name: "  codegen_prepare",
            duration: prepare_started.elapsed(),
        });

        let declare_structs_started = Instant::now();
        self.declare_structs(&module.structs);
        report.timings.push(CodegenTiming {
            name: "  codegen_declare_structs",
            duration: declare_structs_started.elapsed(),
        });

        let declare_globals_started = Instant::now();
        self.declare_globals(&module.globals);
        report.timings.push(CodegenTiming {
            name: "  codegen_declare_globals",
            duration: declare_globals_started.elapsed(),
        });

        let declare_functions_started = Instant::now();
        self.declare_functions(&module.functions);
        report.timings.push(CodegenTiming {
            name: "  codegen_declare_functions",
            duration: declare_functions_started.elapsed(),
        });

        let compile_globals_started = Instant::now();
        for global in &module.globals {
            self.compile_global(global);
        }
        report.timings.push(CodegenTiming {
            name: "  codegen_compile_globals",
            duration: compile_globals_started.elapsed(),
        });

        let compile_functions_started = Instant::now();
        for function in &module.functions {
            if function.body.is_some() {
                self.compile_function(function);
            }
        }
        report.timings.push(CodegenTiming {
            name: "  codegen_compile_functions",
            duration: compile_functions_started.elapsed(),
        });
        let (ir_stats, ir_hot_functions) = self.collect_ir_instruction_stats();
        report.ir_stats = ir_stats;
        report.ir_hot_functions = ir_hot_functions;
        report.alloca_stats = self.alloca_stats;

        report
    }

    pub fn set_asm_dialect(&mut self, dialect: InlineAsmDialect) {
        self.asm_dialect = dialect;
    }

    fn current_block_is_terminated(&self) -> bool {
        self.builder
            .get_insert_block()
            .and_then(|block| block.get_terminator())
            .is_some()
    }

    fn expr_terminated_fallback(
        &mut self,
        llvm_ty: crate::types::BasicTypeEnum<'ctx>,
    ) -> Option<crate::values::BasicValueEnum<'ctx>> {
        if self.current_block_is_terminated() {
            Some(self.get_undef_val(llvm_ty))
        } else {
            None
        }
    }

    pub fn print_ir(&self) -> Result<(), String> {
        let ir = self.module.ir_string()?;
        print!("{}", ir);
        Ok(())
    }

    pub fn emit_to_file(
        &self,
        target_triple_str: &str,
        output_path: &str,
        opt_level: OptLevel,
    ) -> Result<EmitObjectReport, String> {
        if target_triple_str.contains("windows") {
            return self.emit_to_file_windows(target_triple_str, output_path, opt_level);
        }

        let mut report = EmitObjectReport::default();
        let init_started = Instant::now();
        initialize_llvm_targets();
        report.timings.push(EmitObjectTiming {
            name: "  emit_init_llvm",
            duration: init_started.elapsed(),
        });
        let triple = CString::new(target_triple_str).map_err(|_| {
            format!("Target triple contains an interior NUL byte: {target_triple_str:?}")
        })?;
        let setup_started = Instant::now();
        let target_machine = create_target_machine(&triple, opt_level)?;
        let target_data = unsafe { LLVMCreateTargetDataLayout(target_machine) };
        unsafe {
            LLVMSetModuleDataLayout(self.module.as_mut_ptr(), target_data);
            LLVMSetTarget(self.module.as_mut_ptr(), triple.as_ptr());
        }
        report.timings.push(EmitObjectTiming {
            name: "  emit_setup",
            duration: setup_started.elapsed(),
        });

        let verify_started = Instant::now();
        if let Err(err) = self.module.verify() {
            eprintln!("LLVM IR Verification Failed:\n{}", err);
            let _ = self.print_ir();
            unsafe {
                LLVMDisposeTargetData(target_data);
                LLVMDisposeTargetMachine(target_machine);
            }
            return Err("Invalid LLVM IR generated".to_string());
        }
        report.timings.push(EmitObjectTiming {
            name: "  emit_verify",
            duration: verify_started.elapsed(),
        });

        let cleanup_before_stats = self.collect_ir_instruction_stats().0;
        let optimize_started = Instant::now();
        self.run_ir_cleanup_passes(target_machine, opt_level)?;
        report.timings.push(EmitObjectTiming {
            name: "  emit_opt_ir",
            duration: optimize_started.elapsed(),
        });
        let cleanup_after_stats = self.collect_ir_instruction_stats().0;
        report.ir_cleanup_stats = Some(IrCleanupStats {
            before: cleanup_before_stats,
            after: cleanup_after_stats,
        });
        report.remaining_alloca_stats = Some(self.collect_remaining_alloca_stats());
        report.remaining_alloca_names = self.collect_remaining_alloca_names();

        let mut output = output_path.as_bytes().to_vec();
        output.push(0);
        let mut err = ptr::null_mut();
        let backend_started = Instant::now();
        let emit_result = unsafe {
            LLVMTargetMachineEmitToFile(
                target_machine,
                self.module.as_mut_ptr(),
                output.as_mut_ptr() as *mut _,
                LLVMCodeGenFileType::LLVMObjectFile,
                &mut err,
            )
        };
        report.timings.push(EmitObjectTiming {
            name: "  emit_backend",
            duration: backend_started.elapsed(),
        });
        unsafe {
            LLVMDisposeTargetData(target_data);
            LLVMDisposeTargetMachine(target_machine);
        }

        if emit_result != 0 {
            return Err(take_llvm_message(err));
        }

        Ok(report)
    }

    fn emit_to_file_windows(
        &self,
        target_triple_str: &str,
        output_path: &str,
        opt_level: OptLevel,
    ) -> Result<EmitObjectReport, String> {
        let mut report = EmitObjectReport::default();
        let init_started = Instant::now();
        initialize_llvm_targets();
        report.timings.push(EmitObjectTiming {
            name: "  emit_init_llvm",
            duration: init_started.elapsed(),
        });
        let triple = CString::new(target_triple_str).map_err(|_| {
            format!("Target triple contains an interior NUL byte: {target_triple_str:?}")
        })?;
        let cpu = CString::new("generic").unwrap();
        let features = CString::new("").unwrap();

        let mut target = ptr::null_mut();
        let mut err = ptr::null_mut();

        let target_lookup_started = Instant::now();
        unsafe {
            if LLVMGetTargetFromTriple(triple.as_ptr(), &mut target, &mut err) != 0 {
                return Err(take_llvm_message(err));
            }
        }
        report.timings.push(EmitObjectTiming {
            name: "  emit_target_lookup",
            duration: target_lookup_started.elapsed(),
        });

        let setup_started = Instant::now();
        let target_machine =
            create_target_machine_from_parts(target, &triple, &cpu, &features, opt_level)?;
        let target_data = unsafe { LLVMCreateTargetDataLayout(target_machine) };

        // Keep the Windows module target explicit and set the triple through
        // the raw LLVM C API so we fully control ownership/lifetime here.
        unsafe {
            LLVMSetModuleDataLayout(self.module.as_mut_ptr(), target_data);
            LLVMSetTarget(self.module.as_mut_ptr(), triple.as_ptr());
        }
        report.timings.push(EmitObjectTiming {
            name: "  emit_setup",
            duration: setup_started.elapsed(),
        });

        let verify_started = Instant::now();
        if let Err(err) = self.module.verify() {
            eprintln!("LLVM IR Verification Failed:\n{}", err);
            let _ = self.print_ir();
            unsafe {
                LLVMDisposeTargetData(target_data);
                LLVMDisposeTargetMachine(target_machine);
            }
            return Err("Invalid LLVM IR generated".to_string());
        }
        report.timings.push(EmitObjectTiming {
            name: "  emit_verify",
            duration: verify_started.elapsed(),
        });

        let cleanup_before_stats = self.collect_ir_instruction_stats().0;
        let optimize_started = Instant::now();
        self.run_ir_cleanup_passes(target_machine, opt_level)?;
        report.timings.push(EmitObjectTiming {
            name: "  emit_opt_ir",
            duration: optimize_started.elapsed(),
        });
        let cleanup_after_stats = self.collect_ir_instruction_stats().0;
        report.ir_cleanup_stats = Some(IrCleanupStats {
            before: cleanup_before_stats,
            after: cleanup_after_stats,
        });
        report.remaining_alloca_stats = Some(self.collect_remaining_alloca_stats());
        report.remaining_alloca_names = self.collect_remaining_alloca_names();

        // Fast path: plain ASCII paths are safely representable through LLVM's narrow-path API.
        // Keep the memory-buffer fallback for Unicode paths and for direct-write failures.
        if output_path.is_ascii() {
            let mut output = output_path.as_bytes().to_vec();
            output.push(0);
            let backend_started = Instant::now();
            let direct_result = unsafe {
                LLVMTargetMachineEmitToFile(
                    target_machine,
                    self.module.as_mut_ptr(),
                    output.as_mut_ptr() as *mut _,
                    LLVMCodeGenFileType::LLVMObjectFile,
                    &mut err,
                )
            };
            report.timings.push(EmitObjectTiming {
                name: "  emit_backend",
                duration: backend_started.elapsed(),
            });

            if direct_result == 0 {
                unsafe {
                    LLVMDisposeTargetData(target_data);
                    LLVMDisposeTargetMachine(target_machine);
                }
                return Ok(report);
            }

            let _ = take_llvm_message(err);
            err = ptr::null_mut();
        }

        // LLVM's Windows file-emission path still goes through narrow paths here.
        // Emit to memory and let Rust write the bytes so Unicode output paths work.
        let mut mem_buf = ptr::null_mut();
        let backend_started = Instant::now();
        let result = unsafe {
            LLVMTargetMachineEmitToMemoryBuffer(
                target_machine,
                self.module.as_mut_ptr(),
                LLVMCodeGenFileType::LLVMObjectFile,
                &mut err,
                &mut mem_buf,
            )
        };
        report.timings.push(EmitObjectTiming {
            name: "  emit_backend",
            duration: backend_started.elapsed(),
        });

        if result != 0 {
            unsafe {
                LLVMDisposeTargetData(target_data);
                LLVMDisposeTargetMachine(target_machine);
            }
            return Err(take_llvm_message(err));
        }

        let write_result = unsafe {
            let bytes = std::slice::from_raw_parts(
                LLVMGetBufferStart(mem_buf) as *const u8,
                LLVMGetBufferSize(mem_buf),
            );
            let write_started = Instant::now();
            let result = std::fs::write(output_path, bytes);
            report.timings.push(EmitObjectTiming {
                name: "  emit_write",
                duration: write_started.elapsed(),
            });
            result
        }
        .map_err(|e| format!("Failed to write object file `{}`: {}", output_path, e));

        unsafe {
            LLVMDisposeMemoryBuffer(mem_buf);
            LLVMDisposeTargetData(target_data);
            LLVMDisposeTargetMachine(target_machine);
        }

        write_result.map(|_| report)
    }

    fn resolve_symbol(&self, sym: kernc_utils::SymbolId) -> &str {
        self.sess.interner.resolve(sym).unwrap_or("<unknown>")
    }

    fn run_ir_cleanup_passes(
        &self,
        target_machine: LLVMTargetMachineRef,
        opt_level: OptLevel,
    ) -> Result<(), String> {
        if matches!(opt_level, OptLevel::O0) {
            return Ok(());
        }

        let mem2reg_passes = CString::new("mem2reg").unwrap();
        let aggregate_cleanup_passes = CString::new("sroa,mem2reg").unwrap();
        let options = unsafe { LLVMCreatePassBuilderOptions() };
        let mut current_function = self.module.get_first_function();
        while let Some(function) = current_function {
            if function.get_first_basic_block().is_some() && function_contains_alloca(function) {
                let passes = if function_contains_aggregate_alloca(function) {
                    aggregate_cleanup_passes.as_ptr()
                } else {
                    mem2reg_passes.as_ptr()
                };
                let err = unsafe {
                    LLVMRunPassesOnFunction(
                        function.as_value_ref(),
                        passes,
                        target_machine,
                        options,
                    )
                };
                if !err.is_null() {
                    unsafe { LLVMDisposePassBuilderOptions(options) };
                    return Err(take_llvm_error(err));
                }
            }
            current_function = function.get_next_function();
        }
        unsafe { LLVMDisposePassBuilderOptions(options) };
        Ok(())
    }
}

fn function_contains_alloca(function: FunctionValue<'_>) -> bool {
    let mut current_block = function.get_first_basic_block();
    while let Some(block) = current_block {
        let mut current_instruction = block.get_first_instruction();
        while let Some(instruction) = current_instruction {
            if instruction.get_opcode() == LLVMOpcode::LLVMAlloca {
                return true;
            }
            current_instruction = instruction.get_next_instruction();
        }
        current_block = block.get_next_basic_block();
    }

    false
}

fn function_contains_aggregate_alloca(function: FunctionValue<'_>) -> bool {
    let mut current_block = function.get_first_basic_block();
    while let Some(block) = current_block {
        let mut current_instruction = block.get_first_instruction();
        while let Some(instruction) = current_instruction {
            if instruction.get_opcode() == LLVMOpcode::LLVMAlloca {
                let allocated_ty = instruction.get_allocated_type();
                if matches!(
                    allocated_ty,
                    crate::types::BasicTypeEnum::ArrayType(_)
                        | crate::types::BasicTypeEnum::StructType(_)
                ) {
                    return true;
                }
            }
            current_instruction = instruction.get_next_instruction();
        }
        current_block = block.get_next_basic_block();
    }

    false
}

pub(crate) fn accumulate_alloca_site(stats: &mut CodegenAllocaStats, name: &str) {
    if name.is_empty() {
        stats.unnamed += 1;
    } else if name.starts_with("arg_") {
        stats.params += 1;
    } else if name.starts_with("let_") {
        stats.lets += 1;
    } else if name.starts_with("tmp_addrof") {
        stats.addr_of_temps += 1;
    } else if name.starts_with("tmp_materialized_lvalue") {
        stats.materialized_lvalues += 1;
    } else if name.starts_with("tmp_array_for_slice") {
        stats.array_to_slice_temps += 1;
    } else if name.starts_with("union_init") {
        stats.union_inits += 1;
    } else if name.starts_with("data_union_init") {
        stats.data_union_inits += 1;
    } else {
        stats.other += 1;
    }
}

fn llvm_raw_opt_level(opt_level: OptLevel) -> LLVMCodeGenOptLevel {
    match opt_level {
        OptLevel::O0 => LLVMCodeGenOptLevel::LLVMCodeGenLevelNone,
        OptLevel::O1 => LLVMCodeGenOptLevel::LLVMCodeGenLevelLess,
        OptLevel::O2 => LLVMCodeGenOptLevel::LLVMCodeGenLevelDefault,
        OptLevel::O3 => LLVMCodeGenOptLevel::LLVMCodeGenLevelAggressive,
    }
}

fn initialize_llvm_targets() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| unsafe {
        let _ = LLVM_InitializeNativeTarget();
        let _ = LLVM_InitializeNativeAsmPrinter();
        let _ = LLVM_InitializeNativeAsmParser();
        LLVM_InitializeAllTargetInfos();
        LLVM_InitializeAllTargets();
        LLVM_InitializeAllTargetMCs();
        LLVM_InitializeAllAsmPrinters();
        LLVM_InitializeAllAsmParsers();
    });
}

fn create_target_machine(
    triple: &CString,
    opt_level: OptLevel,
) -> Result<LLVMTargetMachineRef, String> {
    let cpu = CString::new("generic").unwrap();
    let features = CString::new("").unwrap();

    let mut target = ptr::null_mut();
    let mut err = ptr::null_mut();
    unsafe {
        if LLVMGetTargetFromTriple(triple.as_ptr(), &mut target, &mut err) != 0 {
            return Err(take_llvm_message(err));
        }
    }

    create_target_machine_from_parts(target, triple, &cpu, &features, opt_level)
}

fn create_target_machine_from_parts(
    target: LLVMTargetRef,
    triple: &CString,
    cpu: &CString,
    features: &CString,
    opt_level: OptLevel,
) -> Result<LLVMTargetMachineRef, String> {
    let target_machine = unsafe {
        LLVMCreateTargetMachine(
            target,
            triple.as_ptr(),
            cpu.as_ptr(),
            features.as_ptr(),
            llvm_raw_opt_level(opt_level),
            LLVMRelocMode::LLVMRelocDefault,
            LLVMCodeModel::LLVMCodeModelDefault,
        )
    };
    if target_machine.is_null() {
        Err("Failed to create target machine".to_string())
    } else {
        Ok(target_machine)
    }
}

fn take_llvm_message(message: *mut std::ffi::c_char) -> String {
    if message.is_null() {
        return "Unknown LLVM error".to_string();
    }

    unsafe {
        let text = CStr::from_ptr(message).to_string_lossy().into_owned();
        LLVMDisposeMessage(message);
        text
    }
}

fn take_llvm_error(error: LLVMErrorRef) -> String {
    if error.is_null() {
        return "Unknown LLVM error".to_string();
    }

    unsafe {
        let message = LLVMGetErrorMessage(error);
        let text = if message.is_null() {
            "Unknown LLVM error".to_string()
        } else {
            CStr::from_ptr(message).to_string_lossy().into_owned()
        };
        LLVMDisposeErrorMessage(message);
        text
    }
}
