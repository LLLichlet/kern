use crate::llvm_api::{
    Builder, Context as LlvmContext, FunctionValue, GlobalValue, InlineAsmDialect,
    Module as LlvmModule, PointerValue, StructType,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use kernc_mir::{MirLocalId, MirModule, MirStruct};
use kernc_mono::MonoId;
use kernc_sema::def::DefId;
use kernc_sema::ty::{TypeId, TypeRegistry};
use kernc_utils::{Session, SymbolId};
use llvm_sys::LLVMOpcode;

mod abi;
mod aggregate;
mod alloca;
mod debug_info;
mod decl;
mod emit;
mod math;
mod mir;
mod refs;
mod simd_shared;
mod types;

use debug_info::DebugInfoState;

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
    alloca_builder: Builder<'ctx>,
    module: LlvmModule<'ctx>,

    sess: &'a mut Session,
    type_registry: &'a TypeRegistry,

    structs: HashMap<MonoId, StructType<'ctx>>,
    mir_structs: HashMap<MonoId, MirStruct>,
    struct_fields: HashMap<MonoId, Vec<SymbolId>>,
    union_ids: std::collections::HashSet<MonoId>,
    globals: HashMap<MonoId, GlobalValue<'ctx>>,
    global_tys: HashMap<MonoId, TypeId>,
    functions: HashMap<MonoId, FunctionValue<'ctx>>,
    function_ret_tys: HashMap<MonoId, TypeId>,
    retained_globals: Vec<PointerValue<'ctx>>,
    string_literal_counter: usize,
    alloca_stats: CodegenAllocaStats,

    locals: HashMap<kernc_utils::SymbolId, PointerValue<'ctx>>,
    mir_locals: HashMap<MirLocalId, PointerValue<'ctx>>,
    loop_targets: Vec<(
        crate::llvm_api::BasicBlock<'ctx>,
        crate::llvm_api::BasicBlock<'ctx>,
    )>,
    asm_dialect: InlineAsmDialect,

    def_mono_map: HashMap<(DefId, Vec<kernc_sema::ty::GenericArg>), MonoId>,
    pure_enum_tag_map: HashMap<(DefId, Vec<kernc_sema::ty::GenericArg>), TypeId>,
    adt_union_map: HashMap<MonoId, MonoId>,
    anon_struct_map: HashMap<TypeId, MonoId>,
    anon_union_map: HashMap<TypeId, MonoId>,
    anon_enum_map: HashMap<TypeId, MonoId>,
    split_sections_for_gc: bool,
    preserve_llvm_value_names: bool,
    debug_info_enabled: bool,
    debug_info_is_optimized: bool,
    debug_info: Option<DebugInfoState<'ctx>>,
}

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    fn prepare_from_mir(&mut self, module: &MirModule) {
        self.mir_structs = module
            .structs
            .iter()
            .cloned()
            .map(|mir_struct| (mir_struct.id, mir_struct))
            .collect();
        self.def_mono_map = module.mono.def_mono_map.clone();
        self.pure_enum_tag_map = module.mono.pure_enum_tag_map.clone();
        self.adt_union_map = module.mono.adt_union_map.clone();
        self.anon_struct_map = module.mono.anon_struct_map.clone();
        self.anon_union_map = module.mono.anon_union_map.clone();
        self.anon_enum_map = module.mono.anon_enum_map.clone();
        self.function_ret_tys = module
            .functions
            .iter()
            .map(|function| (function.id, function.ret_ty))
            .collect();
        self.global_tys = module
            .globals
            .iter()
            .map(|global| (global.id, global.ty))
            .collect();
    }

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
        stats.sort_by(|lhs, rhs| {
            rhs.count
                .cmp(&lhs.count)
                .then_with(|| lhs.name.cmp(&rhs.name))
        });
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
        let preserve_llvm_value_names = sess.preserve_llvm_value_names;
        Self {
            context,
            builder: context.create_builder(),
            alloca_builder: context.create_builder(),
            module: context.create_module(module_name),
            sess,
            type_registry,
            structs: HashMap::new(),
            mir_structs: HashMap::new(),
            struct_fields: HashMap::new(),
            union_ids: std::collections::HashSet::new(),
            globals: HashMap::new(),
            global_tys: HashMap::new(),
            functions: HashMap::new(),
            function_ret_tys: HashMap::new(),
            retained_globals: Vec::new(),
            string_literal_counter: 0,
            alloca_stats: CodegenAllocaStats::default(),
            locals: HashMap::new(),
            mir_locals: HashMap::new(),
            loop_targets: Vec::new(),
            asm_dialect: InlineAsmDialect::Intel,
            def_mono_map: HashMap::new(),
            pure_enum_tag_map: HashMap::new(),
            adt_union_map: HashMap::new(),
            anon_struct_map: HashMap::new(),
            anon_union_map: HashMap::new(),
            anon_enum_map: HashMap::new(),
            split_sections_for_gc,
            preserve_llvm_value_names,
            debug_info_enabled: false,
            debug_info_is_optimized: false,
            debug_info: None,
        }
    }

    pub(crate) fn llvm_name<'b>(&self, preferred: &'b str) -> std::borrow::Cow<'b, str> {
        if self.preserve_llvm_value_names {
            if preferred.as_bytes().contains(&0) {
                std::borrow::Cow::Owned(preferred.replace('\0', "_"))
            } else {
                std::borrow::Cow::Borrowed(preferred)
            }
        } else {
            std::borrow::Cow::Borrowed("")
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

    fn emit_retained_globals_metadata(&mut self) {
        if self.retained_globals.is_empty() {
            return;
        }

        let ptr_ty = self.context.ptr_type(crate::llvm_api::AddressSpace(0));
        let llvm_used = self.module.add_global(
            ptr_ty.array_type(self.retained_globals.len() as u32).into(),
            None,
            "llvm.used",
        );
        llvm_used.set_linkage(crate::llvm_api::Linkage::Appending);
        llvm_used.set_section(Some("llvm.metadata"));
        llvm_used.set_constant(true);
        llvm_used.set_initializer(&ptr_ty.const_array(&self.retained_globals));
    }

    pub fn compile_mir(&mut self, module: &MirModule, collect_diagnostics: bool) -> CodegenReport {
        let mut report = CodegenReport::default();

        let prepare_started = Instant::now();
        self.prepare_from_mir(module);
        report.timings.push(CodegenTiming {
            name: "  codegen_prepare",
            duration: prepare_started.elapsed(),
        });

        let declare_structs_started = Instant::now();
        self.declare_mir_structs(&module.structs);
        report.timings.push(CodegenTiming {
            name: "  codegen_declare_structs",
            duration: declare_structs_started.elapsed(),
        });

        let declare_globals_started = Instant::now();
        self.declare_mir_globals(&module.globals);
        report.timings.push(CodegenTiming {
            name: "  codegen_declare_globals",
            duration: declare_globals_started.elapsed(),
        });

        let declare_functions_started = Instant::now();
        self.declare_mir_functions(&module.functions);
        report.timings.push(CodegenTiming {
            name: "  codegen_declare_functions",
            duration: declare_functions_started.elapsed(),
        });
        let compile_globals_started = Instant::now();
        for global in &module.globals {
            if global.is_extern || global.init.is_none() {
                continue;
            }
            self.compile_mir_global(global);
        }
        report.timings.push(CodegenTiming {
            name: "  codegen_compile_globals",
            duration: compile_globals_started.elapsed(),
        });

        let compile_functions_started = Instant::now();
        for function in &module.functions {
            if function.body.is_none() {
                continue;
            }
            self.compile_mir_function(function);
        }
        report.timings.push(CodegenTiming {
            name: "  codegen_compile_functions",
            duration: compile_functions_started.elapsed(),
        });
        self.emit_retained_globals_metadata();
        self.finalize_debug_info();
        if collect_diagnostics {
            let (ir_stats, ir_hot_functions) = self.collect_ir_instruction_stats();
            report.ir_stats = ir_stats;
            report.ir_hot_functions = ir_hot_functions;
            report.alloca_stats = self.alloca_stats;
        }

        report
    }

    pub fn set_asm_dialect(&mut self, dialect: InlineAsmDialect) {
        self.asm_dialect = dialect;
    }

    pub fn set_debug_info(&mut self, enabled: bool, is_optimized: bool) {
        self.debug_info_enabled = enabled;
        self.debug_info_is_optimized = is_optimized;
        if !enabled {
            self.debug_info = None;
        }
    }

    pub fn into_module(self) -> LlvmModule<'ctx> {
        self.module
    }

    pub fn link_module(&mut self, module: LlvmModule<'ctx>) -> Result<(), String> {
        self.module.link_in(module)
    }

    pub fn session(&self) -> &Session {
        self.sess
    }

    fn current_block_is_terminated(&self) -> bool {
        self.builder
            .get_insert_block()
            .and_then(|block| block.get_terminator())
            .is_some()
    }

    fn resolve_symbol(&self, sym: kernc_utils::SymbolId) -> &str {
        self.sess.interner.resolve(sym).unwrap_or("<unknown>")
    }
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
