use crate::llvm_api::{
    Builder, Context as LlvmContext, DICompileUnit, DIFile, DISubprogram, DIType, DebugInfoBuilder,
    FunctionValue, GlobalValue, InlineAsmDialect, Module as LlvmModule, ModuleFlagBehavior,
    PointerValue, StructType,
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
    LLVMCreatePassBuilderOptions, LLVMDisposePassBuilderOptions, LLVMRunPasses,
};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::path::Path;
use std::ptr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use kernc_mir::{MirFunction, MirLocalId, MirModule, MirStruct};
use kernc_mono::MonoId;
use kernc_sema::def::DefId;
use kernc_sema::ty::{PrimitiveType, TypeId, TypeKind, TypeRegistry};
use kernc_utils::config::{LlvmIrStage, OptLevel};
use kernc_utils::{FileId, Session, Span, SymbolId};
use llvm_sys::LLVMOpcode;

mod abi;
mod aggregate;
mod alloca;
mod decl;
mod math;
mod mir;
mod refs;
mod simd_shared;
mod types;

// LLVM's ThinLTO prelink bitcode emission is not robust under concurrent
// execution in the current in-process pipeline. Keep the rest of multi-CGU
// lowering/codegen parallel, but serialize the prelink/bitcode handoff itself
// so release-thin stays stable until LLVM-side concurrency is proven sound.
static THIN_LTO_BITCODE_EMIT_LOCK: Mutex<()> = Mutex::new(());

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

struct DebugInfoState<'ctx> {
    builder: DebugInfoBuilder<'ctx>,
    compile_unit: Option<DICompileUnit<'ctx>>,
    primary_file: Option<DIFile<'ctx>>,
    files: HashMap<FileId, DIFile<'ctx>>,
    subprograms: HashMap<MonoId, DISubprogram<'ctx>>,
    types: HashMap<TypeId, DIType<'ctx>>,
    finalized: bool,
}

pub struct CodeGenerator<'ctx, 'a> {
    context: &'ctx LlvmContext,
    builder: Builder<'ctx>,
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
        Self {
            context,
            builder: context.create_builder(),
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
            debug_info_enabled: false,
            debug_info_is_optimized: false,
            debug_info: None,
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

    fn ensure_debug_info_state(&mut self) -> Option<&mut DebugInfoState<'ctx>> {
        if !self.debug_info_enabled {
            return None;
        }
        if self.debug_info.is_none() {
            let version = self
                .context
                .i32_type()
                .const_int(self.context.debug_metadata_version() as u64, false);
            self.module.add_basic_value_flag(
                "Debug Info Version",
                ModuleFlagBehavior::Warning,
                version,
            );
            if self.target_uses_coff_sections() {
                let codeview = self.context.i32_type().const_int(1, false);
                self.module
                    .add_basic_value_flag("CodeView", ModuleFlagBehavior::Warning, codeview);
            }
            self.debug_info = Some(DebugInfoState {
                builder: self.module.create_debug_info_builder(),
                compile_unit: None,
                primary_file: None,
                files: HashMap::new(),
                subprograms: HashMap::new(),
                types: HashMap::new(),
                finalized: false,
            });
        }
        self.debug_info.as_mut()
    }

    fn debug_file_parts(path: &Path) -> (String, String) {
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown.rn")
            .to_string();
        let directory = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_string_lossy()
            .into_owned();
        (filename, directory)
    }

    fn debug_source_location(&mut self, span: Span) -> Option<(DIFile<'ctx>, u32, u32)> {
        if !self.debug_info_enabled || span == Span::default() {
            return None;
        }
        let location = self.sess.source_manager.lookup_location(span)?;
        let path = self
            .sess
            .source_manager
            .get_file_path(location.file_id)
            .cloned()
            .unwrap_or_default();
        let (filename, directory) = Self::debug_file_parts(&path);
        let state = self.ensure_debug_info_state()?;
        let file = if let Some(file) = state.files.get(&location.file_id).copied() {
            file
        } else {
            let file = state.builder.create_file(&filename, &directory);
            state.files.insert(location.file_id, file);
            file
        };
        Some((
            file,
            location.line.min(u32::MAX as usize) as u32,
            location.col.min(u32::MAX as usize) as u32,
        ))
    }

    fn debug_pointer_bytes(&self) -> u64 {
        self.sess.target.pointer_size
    }

    fn debug_pointer_bits(&self) -> u64 {
        self.debug_pointer_bytes() * 8
    }

    fn debug_align_to(offset: u64, align: u64) -> u64 {
        if align <= 1 {
            offset
        } else {
            (offset + align - 1) & !(align - 1)
        }
    }

    fn debug_primitive_align_bytes(&self, primitive: PrimitiveType) -> u64 {
        match primitive {
            PrimitiveType::Void | PrimitiveType::Never => 1,
            PrimitiveType::Bool | PrimitiveType::I8 | PrimitiveType::U8 => 1,
            PrimitiveType::I16 | PrimitiveType::U16 => 2,
            PrimitiveType::I32 | PrimitiveType::U32 | PrimitiveType::F32 => 4,
            PrimitiveType::I64 | PrimitiveType::U64 | PrimitiveType::F64 => 8,
            PrimitiveType::ISize | PrimitiveType::USize | PrimitiveType::Str => {
                self.debug_pointer_bytes()
            }
            PrimitiveType::I128 | PrimitiveType::U128 => 16,
        }
    }

    fn debug_primitive_size_bytes(&self, primitive: PrimitiveType) -> u64 {
        match primitive {
            PrimitiveType::Void | PrimitiveType::Never => 0,
            PrimitiveType::Bool | PrimitiveType::I8 | PrimitiveType::U8 => 1,
            PrimitiveType::I16 | PrimitiveType::U16 => 2,
            PrimitiveType::I32 | PrimitiveType::U32 | PrimitiveType::F32 => 4,
            PrimitiveType::I64 | PrimitiveType::U64 | PrimitiveType::F64 => 8,
            PrimitiveType::ISize | PrimitiveType::USize | PrimitiveType::Str => {
                self.debug_pointer_bytes()
            }
            PrimitiveType::I128 | PrimitiveType::U128 => 16,
        }
    }

    fn debug_has_packed_attr(&self, attrs: &[kernc_ast::MetaItem]) -> bool {
        attrs.iter().any(|attr| {
            matches!(attr, kernc_ast::MetaItem::Marker(id) if self.resolve_symbol(*id) == "packed")
        })
    }

    fn debug_mir_struct_id_for_type(&self, ty: TypeId) -> Option<MonoId> {
        let norm = self.type_registry.normalize(ty);
        match self.type_registry.get(norm).clone() {
            TypeKind::Def(def_id, args) | TypeKind::Enum(def_id, args) => {
                self.def_mono_map.get(&(def_id, args)).copied()
            }
            TypeKind::EnumPayload(def_id, args) => self
                .def_mono_map
                .get(&(def_id, args))
                .and_then(|wrapper_id| self.adt_union_map.get(wrapper_id))
                .copied(),
            TypeKind::AnonymousStruct(..) => self.anon_struct_map.get(&norm).copied(),
            TypeKind::AnonymousUnion(..) => self.anon_union_map.get(&norm).copied(),
            TypeKind::AnonymousEnum(..) => self.anon_enum_map.get(&norm).copied(),
            TypeKind::AnonymousEnumPayload(enum_ty) => {
                let enum_ty = self.type_registry.normalize(enum_ty);
                self.anon_enum_map
                    .get(&enum_ty)
                    .and_then(|wrapper_id| self.adt_union_map.get(wrapper_id))
                    .copied()
            }
            _ => None,
        }
    }

    fn debug_mir_struct_for_type(&self, ty: TypeId) -> Option<&MirStruct> {
        let struct_id = self.debug_mir_struct_id_for_type(ty)?;
        self.mir_structs.get(&struct_id)
    }

    fn debug_cached_type(&self, ty: TypeId) -> Option<DIType<'ctx>> {
        self.debug_info
            .as_ref()
            .and_then(|state| state.types.get(&ty).copied())
    }

    fn debug_cache_type(&mut self, ty: TypeId, di_ty: DIType<'ctx>) {
        if let Some(state) = self.ensure_debug_info_state() {
            state.types.insert(ty, di_ty);
        }
    }

    fn debug_type_scope(&mut self) -> Option<(DICompileUnit<'ctx>, DIFile<'ctx>)> {
        let is_optimized = self.debug_info_is_optimized;
        let producer = format!("kernc {}", env!("CARGO_PKG_VERSION"));
        let state = self.ensure_debug_info_state()?;
        let file = if let Some(file) = state.primary_file {
            file
        } else if let Some(file) = state.files.values().next().copied() {
            state.primary_file = Some(file);
            file
        } else {
            let file = state.builder.create_file("unknown.rn", ".");
            state.primary_file = Some(file);
            file
        };
        let unit = if let Some(unit) = state.compile_unit {
            unit
        } else {
            let unit = state
                .builder
                .create_compile_unit(file, &producer, is_optimized);
            state.compile_unit = Some(unit);
            unit
        };
        Some((unit, file))
    }

    fn debug_mir_struct_layout(
        &mut self,
        mir_struct: &MirStruct,
    ) -> (u64, u64, Vec<(String, TypeId, u64, u64, u32)>) {
        let packed = self.debug_has_packed_attr(&mir_struct.attributes);
        if mir_struct.is_union {
            let align_bytes = if packed {
                1
            } else {
                mir_struct.union_align.max(1) as u64
            };
            let size_bytes = if packed {
                mir_struct.union_size as u64
            } else {
                Self::debug_align_to(mir_struct.union_size as u64, align_bytes)
            };
            let mut members = Vec::with_capacity(mir_struct.fields.len());
            for field in &mir_struct.fields {
                let field_size_bits = self.debug_type_size_bytes(field.ty) * 8;
                let field_align_bits = (if packed {
                    1
                } else {
                    self.debug_type_align_bytes(field.ty).max(1)
                } * 8) as u32;
                members.push((
                    self.resolve_symbol(field.name).to_string(),
                    field.ty,
                    0,
                    field_size_bits,
                    field_align_bits,
                ));
            }
            return (size_bytes.max(1), align_bytes.max(1), members);
        }

        let mut offset_bytes = 0;
        let mut struct_align_bytes = if packed { 1 } else { 1 };
        let mut members = Vec::with_capacity(mir_struct.fields.len());
        for field in &mir_struct.fields {
            let field_align_bytes = if packed {
                1
            } else {
                self.debug_type_align_bytes(field.ty).max(1)
            };
            let field_size_bytes = self.debug_type_size_bytes(field.ty);
            if !packed {
                struct_align_bytes = struct_align_bytes.max(field_align_bytes);
                offset_bytes = Self::debug_align_to(offset_bytes, field_align_bytes);
            }
            members.push((
                self.resolve_symbol(field.name).to_string(),
                field.ty,
                offset_bytes * 8,
                field_size_bytes * 8,
                (field_align_bytes * 8) as u32,
            ));
            offset_bytes += field_size_bytes;
        }

        let size_bytes = Self::debug_align_to(offset_bytes, struct_align_bytes.max(1));
        (size_bytes, struct_align_bytes.max(1), members)
    }

    fn debug_type_align_bytes(&mut self, ty: TypeId) -> u64 {
        let norm = self.type_registry.normalize(ty);
        match self.type_registry.get(norm).clone() {
            TypeKind::Primitive(primitive) => self.debug_primitive_align_bytes(primitive),
            TypeKind::Pointer { .. }
            | TypeKind::VolatilePtr { .. }
            | TypeKind::Function { .. }
            | TypeKind::FnDef(..)
            | TypeKind::Slice { .. }
            | TypeKind::TraitObject(..) => self.debug_pointer_bytes(),
            TypeKind::Simd { elem, lanes } => {
                if elem == TypeId::BOOL {
                    1
                } else {
                    let elem_align = self.debug_type_align_bytes(elem);
                    let elem_size = self.debug_type_size_bytes(elem);
                    elem_align.max(elem_size.saturating_mul(lanes as u64))
                }
            }
            TypeKind::Array { elem, .. } | TypeKind::ArrayInfer { elem, .. } => {
                self.debug_type_align_bytes(elem).max(1)
            }
            TypeKind::ClosureInterface { .. } => 1,
            TypeKind::AnonymousState { captures, .. } => {
                let mut offset_bytes = 0;
                let mut struct_align_bytes = 1;
                for capture in captures {
                    let capture_align = self.debug_type_align_bytes(capture).max(1);
                    struct_align_bytes = struct_align_bytes.max(capture_align);
                    offset_bytes = Self::debug_align_to(offset_bytes, capture_align);
                    offset_bytes += self.debug_type_size_bytes(capture);
                }
                struct_align_bytes.max(1)
            }
            TypeKind::Def(..)
            | TypeKind::Enum(..)
            | TypeKind::EnumPayload(..)
            | TypeKind::AnonymousStruct(..)
            | TypeKind::AnonymousUnion(..)
            | TypeKind::AnonymousEnum(..)
            | TypeKind::AnonymousEnumPayload(_) => self
                .debug_mir_struct_for_type(norm)
                .cloned()
                .map(|mir_struct| self.debug_mir_struct_layout(&mir_struct).1)
                .unwrap_or(1),
            TypeKind::Projection { .. }
            | TypeKind::Alias(..)
            | TypeKind::Param(_)
            | TypeKind::Associated(..)
            | TypeKind::Module(_)
            | TypeKind::TypeVar(_)
            | TypeKind::Error => 1,
        }
    }

    fn debug_type_size_bytes(&mut self, ty: TypeId) -> u64 {
        let norm = self.type_registry.normalize(ty);
        match self.type_registry.get(norm).clone() {
            TypeKind::Primitive(primitive) => self.debug_primitive_size_bytes(primitive),
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let elem = self.type_registry.normalize(elem);
                if matches!(
                    self.type_registry.get(elem),
                    TypeKind::TraitObject(..) | TypeKind::ClosureInterface { .. }
                ) {
                    self.debug_pointer_bytes() * 2
                } else {
                    self.debug_pointer_bytes()
                }
            }
            TypeKind::Function { .. } | TypeKind::FnDef(..) => self.debug_pointer_bytes(),
            TypeKind::Slice { .. } | TypeKind::TraitObject(..) => self.debug_pointer_bytes() * 2,
            TypeKind::Simd { elem, lanes } => {
                if elem == TypeId::BOOL {
                    (lanes as u64).div_ceil(8)
                } else {
                    self.debug_type_size_bytes(elem)
                        .saturating_mul(lanes as u64)
                }
            }
            TypeKind::Array { elem, len, .. } => self
                .const_generic_usize(len, Span::default())
                .map(|len| self.debug_type_size_bytes(elem).saturating_mul(len))
                .unwrap_or(0),
            TypeKind::ArrayInfer { .. } | TypeKind::ClosureInterface { .. } => 0,
            TypeKind::AnonymousState { captures, .. } => {
                let mut offset_bytes = 0;
                let mut struct_align_bytes = 1;
                for capture in captures {
                    let capture_align = self.debug_type_align_bytes(capture).max(1);
                    struct_align_bytes = struct_align_bytes.max(capture_align);
                    offset_bytes = Self::debug_align_to(offset_bytes, capture_align);
                    offset_bytes += self.debug_type_size_bytes(capture);
                }
                Self::debug_align_to(offset_bytes, struct_align_bytes.max(1))
            }
            TypeKind::Def(..)
            | TypeKind::Enum(..)
            | TypeKind::EnumPayload(..)
            | TypeKind::AnonymousStruct(..)
            | TypeKind::AnonymousUnion(..)
            | TypeKind::AnonymousEnum(..)
            | TypeKind::AnonymousEnumPayload(_) => self
                .debug_mir_struct_for_type(norm)
                .cloned()
                .map(|mir_struct| self.debug_mir_struct_layout(&mir_struct).0)
                .unwrap_or(0),
            TypeKind::Projection { .. }
            | TypeKind::Alias(..)
            | TypeKind::Param(_)
            | TypeKind::Associated(..)
            | TypeKind::Module(_)
            | TypeKind::TypeVar(_)
            | TypeKind::Error => 0,
        }
    }

    fn debug_type_name(&self, ty: TypeId) -> String {
        let norm = self.type_registry.normalize(ty);
        match self.type_registry.get(norm).clone() {
            TypeKind::Primitive(primitive) => match primitive {
                PrimitiveType::Void => "void".to_string(),
                PrimitiveType::Bool => "bool".to_string(),
                PrimitiveType::I8 => "i8".to_string(),
                PrimitiveType::I16 => "i16".to_string(),
                PrimitiveType::I32 => "i32".to_string(),
                PrimitiveType::I64 => "i64".to_string(),
                PrimitiveType::I128 => "i128".to_string(),
                PrimitiveType::ISize => "isize".to_string(),
                PrimitiveType::U8 => "u8".to_string(),
                PrimitiveType::U16 => "u16".to_string(),
                PrimitiveType::U32 => "u32".to_string(),
                PrimitiveType::U64 => "u64".to_string(),
                PrimitiveType::U128 => "u128".to_string(),
                PrimitiveType::USize => "usize".to_string(),
                PrimitiveType::F32 => "f32".to_string(),
                PrimitiveType::F64 => "f64".to_string(),
                PrimitiveType::Str => "str".to_string(),
                PrimitiveType::Never => "never".to_string(),
            },
            TypeKind::Pointer { is_mut, elem } => {
                format!(
                    "*{}{}",
                    if is_mut { "mut " } else { "" },
                    self.debug_type_name(elem)
                )
            }
            TypeKind::VolatilePtr { is_mut, elem } => {
                format!(
                    "^{}{}",
                    if is_mut { "mut " } else { "" },
                    self.debug_type_name(elem)
                )
            }
            TypeKind::Array { elem, len } => format!("[{}]{}", len, self.debug_type_name(elem)),
            TypeKind::Slice { is_mut, elem } => {
                format!(
                    "[]{}{}",
                    if is_mut { "mut " } else { "" },
                    self.debug_type_name(elem)
                )
            }
            TypeKind::Function { .. } => "fn".to_string(),
            TypeKind::ClosureInterface { .. } => "Fn".to_string(),
            TypeKind::TraitObject(..) => "trait-object".to_string(),
            TypeKind::Simd { elem, lanes } => format!("{}x{}", self.debug_type_name(elem), lanes),
            TypeKind::AnonymousState { .. } => "<closure-state>".to_string(),
            TypeKind::Def(..)
            | TypeKind::Enum(..)
            | TypeKind::EnumPayload(..)
            | TypeKind::AnonymousStruct(..)
            | TypeKind::AnonymousUnion(..)
            | TypeKind::AnonymousEnum(..)
            | TypeKind::AnonymousEnumPayload(_) => self
                .debug_mir_struct_for_type(norm)
                .map(|mir_struct| mir_struct.name.clone())
                .unwrap_or_else(|| "<unnamed>".to_string()),
            TypeKind::Projection { .. }
            | TypeKind::Alias(..)
            | TypeKind::Param(_)
            | TypeKind::Associated(..)
            | TypeKind::FnDef(..)
            | TypeKind::Module(_)
            | TypeKind::TypeVar(_)
            | TypeKind::ArrayInfer { .. }
            | TypeKind::Error => "<unnamed>".to_string(),
        }
    }

    fn debug_basic_type_encoding(
        primitive: PrimitiveType,
    ) -> Option<(u64, llvm_sys::debuginfo::LLVMDWARFTypeEncoding)> {
        // DWARF DW_ATE_* encodings.
        const DW_ATE_ADDRESS: u32 = 0x01;
        const DW_ATE_BOOLEAN: u32 = 0x02;
        const DW_ATE_FLOAT: u32 = 0x04;
        const DW_ATE_SIGNED: u32 = 0x05;
        const DW_ATE_UNSIGNED: u32 = 0x07;
        match primitive {
            PrimitiveType::Bool => Some((8, DW_ATE_BOOLEAN)),
            PrimitiveType::I8 => Some((8, DW_ATE_SIGNED)),
            PrimitiveType::I16 => Some((16, DW_ATE_SIGNED)),
            PrimitiveType::I32 => Some((32, DW_ATE_SIGNED)),
            PrimitiveType::I64 => Some((64, DW_ATE_SIGNED)),
            PrimitiveType::I128 => Some((128, DW_ATE_SIGNED)),
            PrimitiveType::ISize => Some((0, DW_ATE_SIGNED)),
            PrimitiveType::U8 => Some((8, DW_ATE_UNSIGNED)),
            PrimitiveType::U16 => Some((16, DW_ATE_UNSIGNED)),
            PrimitiveType::U32 => Some((32, DW_ATE_UNSIGNED)),
            PrimitiveType::U64 => Some((64, DW_ATE_UNSIGNED)),
            PrimitiveType::U128 => Some((128, DW_ATE_UNSIGNED)),
            PrimitiveType::USize => Some((0, DW_ATE_UNSIGNED)),
            PrimitiveType::F32 => Some((32, DW_ATE_FLOAT)),
            PrimitiveType::F64 => Some((64, DW_ATE_FLOAT)),
            PrimitiveType::Str => Some((0, DW_ATE_ADDRESS)),
            PrimitiveType::Void | PrimitiveType::Never => None,
        }
    }

    fn debug_build_named_composite_type(
        &mut self,
        norm: TypeId,
        mir_struct: MirStruct,
    ) -> Option<DIType<'ctx>> {
        const DW_TAG_STRUCTURE_TYPE: u32 = 0x13;
        const DW_TAG_UNION_TYPE: u32 = 0x17;

        let (scope, file) = self.debug_type_scope()?;
        let name = mir_struct.name.clone();
        let unique_id = format!("kern.debug.{name}.{:?}", norm);
        let (size_bytes, align_bytes, members) = self.debug_mir_struct_layout(&mir_struct);
        let placeholder = {
            let state = self.ensure_debug_info_state()?;
            state.builder.create_replaceable_composite_type(
                if mir_struct.is_union {
                    DW_TAG_UNION_TYPE
                } else {
                    DW_TAG_STRUCTURE_TYPE
                },
                scope,
                &name,
                file,
                size_bytes * 8,
                (align_bytes * 8) as u32,
                &unique_id,
            )
        };
        self.debug_cache_type(norm, placeholder);

        let mut member_types = Vec::with_capacity(members.len());
        for (member_name, member_ty, offset_bits, size_bits, align_bits) in members {
            let field_di_ty = self.debug_type(member_ty)?;
            let member_di = {
                let state = self.ensure_debug_info_state()?;
                state.builder.create_member_type(
                    scope,
                    &member_name,
                    file,
                    size_bits,
                    align_bits,
                    offset_bits,
                    field_di_ty,
                )
            };
            member_types.push(member_di);
        }

        let composite_ty = {
            let state = self.ensure_debug_info_state()?;
            if mir_struct.is_union {
                state.builder.create_union_type(
                    scope,
                    &name,
                    file,
                    size_bytes * 8,
                    (align_bytes * 8) as u32,
                    &member_types,
                    &unique_id,
                )
            } else {
                state.builder.create_struct_type(
                    scope,
                    &name,
                    file,
                    size_bytes * 8,
                    (align_bytes * 8) as u32,
                    &member_types,
                    &unique_id,
                )
            }
        };
        let state = self.ensure_debug_info_state()?;
        state
            .builder
            .replace_all_uses_with(placeholder, composite_ty);
        state.types.insert(norm, composite_ty);
        Some(composite_ty)
    }

    fn debug_build_fat_pointer_type(
        &mut self,
        norm: TypeId,
        data_pointee: DIType<'ctx>,
        meta_name: &str,
    ) -> Option<DIType<'ctx>> {
        let (scope, file) = self.debug_type_scope()?;
        let name = self.debug_type_name(norm);
        let pointer_bits = self.debug_pointer_bits();
        let data_ptr_ty = {
            let state = self.ensure_debug_info_state()?;
            state.builder.create_pointer_type(
                data_pointee,
                pointer_bits,
                pointer_bits as u32,
                "data_ptr",
            )
        };
        let meta_ty = self.debug_type(TypeId::USIZE)?;
        let members = {
            let state = self.ensure_debug_info_state()?;
            vec![
                state.builder.create_member_type(
                    scope,
                    "data_ptr",
                    file,
                    pointer_bits,
                    pointer_bits as u32,
                    0,
                    data_ptr_ty,
                ),
                state.builder.create_member_type(
                    scope,
                    meta_name,
                    file,
                    pointer_bits,
                    pointer_bits as u32,
                    pointer_bits,
                    meta_ty,
                ),
            ]
        };
        let composite_ty = {
            let state = self.ensure_debug_info_state()?;
            state.builder.create_struct_type(
                scope,
                &name,
                file,
                pointer_bits * 2,
                pointer_bits as u32,
                &members,
                &format!("kern.debug.{name}.{:?}", norm),
            )
        };
        self.debug_cache_type(norm, composite_ty);
        Some(composite_ty)
    }

    fn debug_build_anonymous_state_type(
        &mut self,
        norm: TypeId,
        captures: Vec<TypeId>,
    ) -> Option<DIType<'ctx>> {
        let (scope, file) = self.debug_type_scope()?;
        let name = self.debug_type_name(norm);
        let mut offset_bits = 0;
        let mut members = Vec::with_capacity(captures.len());
        for (index, capture_ty) in captures.into_iter().enumerate() {
            let capture_align_bits = (self.debug_type_align_bytes(capture_ty).max(1) * 8) as u32;
            offset_bits = Self::debug_align_to(offset_bits, capture_align_bits as u64);
            let capture_size_bits = self.debug_type_size_bytes(capture_ty) * 8;
            let capture_di_ty = self.debug_type(capture_ty)?;
            let member = {
                let state = self.ensure_debug_info_state()?;
                state.builder.create_member_type(
                    scope,
                    &format!("capture{index}"),
                    file,
                    capture_size_bits,
                    capture_align_bits,
                    offset_bits,
                    capture_di_ty,
                )
            };
            members.push(member);
            offset_bits += capture_size_bits;
        }
        let size_bits = self.debug_type_size_bytes(norm) * 8;
        let align_bits = (self.debug_type_align_bytes(norm) * 8) as u32;
        let composite_ty = {
            let state = self.ensure_debug_info_state()?;
            state.builder.create_struct_type(
                scope,
                &name,
                file,
                size_bits,
                align_bits,
                &members,
                &format!("kern.debug.{name}.{:?}", norm),
            )
        };
        self.debug_cache_type(norm, composite_ty);
        Some(composite_ty)
    }

    fn debug_type(&mut self, ty: TypeId) -> Option<DIType<'ctx>> {
        if !self.debug_info_enabled {
            return None;
        }
        let norm = self.type_registry.normalize(ty);
        if let Some(di_ty) = self.debug_cached_type(norm) {
            return Some(di_ty);
        }

        let di_ty = match self.type_registry.get(norm).clone() {
            TypeKind::Primitive(primitive) => {
                let name = self.debug_type_name(norm);
                let type_info = Self::debug_basic_type_encoding(primitive);
                let pointer_bits = self.debug_pointer_bits();
                let state = self.ensure_debug_info_state()?;
                if let Some((mut bits, encoding)) = type_info {
                    if bits == 0 {
                        bits = pointer_bits;
                    }
                    state.builder.create_basic_type(&name, bits, encoding)
                } else {
                    state.builder.create_unspecified_type(&name)
                }
            }
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let elem = self.type_registry.normalize(elem);
                if matches!(self.type_registry.get(elem), TypeKind::TraitObject(..)) {
                    let data_pointee = self.debug_type(TypeId::VOID)?;
                    return self.debug_build_fat_pointer_type(norm, data_pointee, "vtable");
                }
                if matches!(
                    self.type_registry.get(elem),
                    TypeKind::ClosureInterface { .. }
                ) {
                    let data_pointee = self.debug_type(TypeId::VOID)?;
                    return self.debug_build_fat_pointer_type(norm, data_pointee, "code_ptr");
                }

                let pointee = self.debug_type(elem)?;
                let name = self.debug_type_name(norm);
                let pointer_bits = self.debug_pointer_bits();
                let state = self.ensure_debug_info_state()?;
                state
                    .builder
                    .create_pointer_type(pointee, pointer_bits, pointer_bits as u32, &name)
            }
            TypeKind::Slice { elem, .. } => {
                let data_pointee = self.debug_type(elem)?;
                return self.debug_build_fat_pointer_type(norm, data_pointee, "len");
            }
            TypeKind::TraitObject(..) => {
                let data_pointee = self.debug_type(TypeId::VOID)?;
                return self.debug_build_fat_pointer_type(norm, data_pointee, "vtable");
            }
            TypeKind::Array { elem, len, .. } => {
                let elem_di_ty = self.debug_type(elem)?;
                let len = self.const_generic_usize(len, Span::default())?;
                let size_bits = self.debug_type_size_bytes(norm) * 8;
                let align_bits = (self.debug_type_align_bytes(norm) * 8) as u32;
                let state = self.ensure_debug_info_state()?;
                state
                    .builder
                    .create_array_type(elem_di_ty, size_bits, align_bits, len as i64)
            }
            TypeKind::Simd { elem, lanes } => {
                let elem_di_ty = self.debug_type(elem)?;
                let size_bits = self.debug_type_size_bytes(norm) * 8;
                let align_bits = (self.debug_type_align_bytes(norm) * 8) as u32;
                let state = self.ensure_debug_info_state()?;
                state
                    .builder
                    .create_array_type(elem_di_ty, size_bits, align_bits, lanes as i64)
            }
            TypeKind::AnonymousState { captures, .. } => {
                return self.debug_build_anonymous_state_type(norm, captures);
            }
            TypeKind::Def(..)
            | TypeKind::Enum(..)
            | TypeKind::EnumPayload(..)
            | TypeKind::AnonymousStruct(..)
            | TypeKind::AnonymousUnion(..)
            | TypeKind::AnonymousEnum(..)
            | TypeKind::AnonymousEnumPayload(_) => {
                if let Some(mir_struct) = self.debug_mir_struct_for_type(norm).cloned() {
                    return self.debug_build_named_composite_type(norm, mir_struct);
                }
                let name = self.debug_type_name(norm);
                let state = self.ensure_debug_info_state()?;
                state.builder.create_unspecified_type(&name)
            }
            _ => {
                let name = self.debug_type_name(norm);
                let state = self.ensure_debug_info_state()?;
                state.builder.create_unspecified_type(&name)
            }
        };
        self.debug_cache_type(norm, di_ty);
        Some(di_ty)
    }

    fn declare_debug_local(
        &mut self,
        function: &MirFunction,
        local: &kernc_mir::MirLocal,
        storage: PointerValue<'ctx>,
        entry_block: crate::llvm_api::BasicBlock<'ctx>,
        arg_no: Option<u32>,
    ) {
        let name = self.resolve_symbol(local.name).to_string();
        if name == "<unknown>" {
            return;
        }
        let span = if local.span == Span::default() {
            function.span
        } else {
            local.span
        };
        let Some((file, line, column)) = self.debug_source_location(span) else {
            return;
        };
        let Some(di_ty) = self.debug_type(local.ty) else {
            return;
        };
        let Some(subprogram) = self
            .debug_info
            .as_ref()
            .and_then(|state| state.subprograms.get(&function.id).copied())
        else {
            return;
        };
        let context = self.context;
        let state = self
            .ensure_debug_info_state()
            .expect("debug info state must exist");
        let location = state
            .builder
            .create_debug_location(context, line, column, subprogram);
        let variable = match arg_no {
            Some(arg_no) => state
                .builder
                .create_parameter_variable(subprogram, &name, arg_no, file, line, di_ty),
            None => state
                .builder
                .create_auto_variable(subprogram, &name, file, line, di_ty, 0),
        };
        let expression = state.builder.create_expression();
        let _ = state.builder.insert_declare_at_end(
            storage,
            variable,
            expression,
            location,
            entry_block,
        );
    }

    fn debug_compile_unit(&mut self, file: DIFile<'ctx>) -> Option<DICompileUnit<'ctx>> {
        let is_optimized = self.debug_info_is_optimized;
        let producer = format!("kernc {}", env!("CARGO_PKG_VERSION"));
        let state = self.ensure_debug_info_state()?;
        if state.primary_file.is_none() {
            state.primary_file = Some(file);
        }
        if let Some(unit) = state.compile_unit {
            return Some(unit);
        }
        let unit = state
            .builder
            .create_compile_unit(file, &producer, is_optimized);
        state.compile_unit = Some(unit);
        Some(unit)
    }

    fn attach_debug_info_to_function(
        &mut self,
        function: &MirFunction,
        llvm_func: FunctionValue<'ctx>,
    ) {
        let Some((file, line, _column)) = self.debug_source_location(function.span) else {
            return;
        };
        let Some(compile_unit) = self.debug_compile_unit(file) else {
            return;
        };
        let is_optimized = self.debug_info_is_optimized;
        let is_local_to_unit = matches!(function.linkage, kernc_mir::MirLinkage::Internal);
        let state = self
            .ensure_debug_info_state()
            .expect("debug info state must exist");
        let subroutine_type = state.builder.create_subroutine_type(file);
        let subprogram = state.builder.create_function(
            compile_unit,
            file,
            &function.name,
            &function.name,
            line,
            line,
            subroutine_type,
            is_local_to_unit,
            is_optimized,
        );
        llvm_func.set_subprogram(subprogram);
        state.subprograms.insert(function.id, subprogram);
    }

    fn set_function_debug_location(&mut self, function: &MirFunction) {
        self.set_debug_location_for_span(function, function.span);
    }

    fn set_debug_location_for_span(&mut self, function: &MirFunction, span: Span) {
        let Some(subprogram) = self
            .debug_info
            .as_ref()
            .and_then(|state| state.subprograms.get(&function.id).copied())
        else {
            return;
        };
        let Some((_, line, column)) = self.debug_source_location(span) else {
            return;
        };
        let context = self.context;
        let state = self
            .ensure_debug_info_state()
            .expect("debug info state must exist");
        let location = state
            .builder
            .create_debug_location(context, line, column, subprogram);
        self.builder.set_current_debug_location(location);
    }

    fn clear_function_debug_location(&mut self) {
        self.builder.clear_current_debug_location();
    }

    fn finalize_debug_info(&mut self) {
        if let Some(state) = self.debug_info.as_mut()
            && !state.finalized
        {
            state.builder.finalize();
            state.finalized = true;
        }
    }

    pub fn print_ir(&self) -> Result<(), String> {
        let ir = self.module.ir_string()?;
        print!("{}", ir);
        Ok(())
    }

    pub fn emit_llvm_ir(
        &self,
        target_triple_str: &str,
        opt_level: OptLevel,
        stage: LlvmIrStage,
        collect_diagnostics: bool,
    ) -> Result<EmitObjectReport, String> {
        let mut report = EmitObjectReport::default();
        if stage == LlvmIrStage::Raw {
            let print_started = Instant::now();
            self.print_ir()?;
            report.timings.push(EmitObjectTiming {
                name: "  emit_print_ir",
                duration: print_started.elapsed(),
            });
            return Ok(report);
        }

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

        if stage == LlvmIrStage::Optimized {
            let cleanup_before_stats =
                collect_diagnostics.then(|| self.collect_ir_instruction_stats().0);
            let optimize_started = Instant::now();
            self.run_llvm_pass_pipeline(target_machine, opt_level)?;
            report.timings.push(EmitObjectTiming {
                name: "  emit_opt_ir",
                duration: optimize_started.elapsed(),
            });
            if let Some(cleanup_before_stats) = cleanup_before_stats {
                let cleanup_after_stats = self.collect_ir_instruction_stats().0;
                report.ir_cleanup_stats = Some(IrCleanupStats {
                    before: cleanup_before_stats,
                    after: cleanup_after_stats,
                });
                report.remaining_alloca_stats = Some(self.collect_remaining_alloca_stats());
                report.remaining_alloca_names = self.collect_remaining_alloca_names();
            }
        }

        let print_started = Instant::now();
        let print_result = self.print_ir();
        report.timings.push(EmitObjectTiming {
            name: "  emit_print_ir",
            duration: print_started.elapsed(),
        });

        unsafe {
            LLVMDisposeTargetData(target_data);
            LLVMDisposeTargetMachine(target_machine);
        }

        print_result.map(|_| report)
    }

    pub fn emit_to_file(
        &self,
        target_triple_str: &str,
        output_path: &str,
        opt_level: OptLevel,
        collect_diagnostics: bool,
    ) -> Result<EmitObjectReport, String> {
        if target_triple_str.contains("windows") {
            return self.emit_to_file_windows(
                target_triple_str,
                output_path,
                opt_level,
                collect_diagnostics,
            );
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

        let cleanup_before_stats =
            collect_diagnostics.then(|| self.collect_ir_instruction_stats().0);
        let optimize_started = Instant::now();
        self.run_llvm_pass_pipeline(target_machine, opt_level)?;
        report.timings.push(EmitObjectTiming {
            name: "  emit_opt_ir",
            duration: optimize_started.elapsed(),
        });
        if let Some(cleanup_before_stats) = cleanup_before_stats {
            let cleanup_after_stats = self.collect_ir_instruction_stats().0;
            report.ir_cleanup_stats = Some(IrCleanupStats {
                before: cleanup_before_stats,
                after: cleanup_after_stats,
            });
            report.remaining_alloca_stats = Some(self.collect_remaining_alloca_stats());
            report.remaining_alloca_names = self.collect_remaining_alloca_names();
        }

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

    pub fn emit_thin_lto_bitcode(
        &self,
        target_triple_str: &str,
        opt_level: OptLevel,
        collect_diagnostics: bool,
    ) -> Result<(Vec<u8>, EmitObjectReport), String> {
        let _thin_lto_emit_guard = THIN_LTO_BITCODE_EMIT_LOCK
            .lock()
            .map_err(|_| "ThinLTO bitcode emit lock was poisoned".to_string())?;
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

        let cleanup_before_stats =
            collect_diagnostics.then(|| self.collect_ir_instruction_stats().0);
        let optimize_started = Instant::now();
        self.run_llvm_thin_lto_prelink_pipeline(target_machine, opt_level)?;
        report.timings.push(EmitObjectTiming {
            name: "  emit_thinlto_prelink",
            duration: optimize_started.elapsed(),
        });
        if let Some(cleanup_before_stats) = cleanup_before_stats {
            let cleanup_after_stats = self.collect_ir_instruction_stats().0;
            report.ir_cleanup_stats = Some(IrCleanupStats {
                before: cleanup_before_stats,
                after: cleanup_after_stats,
            });
            report.remaining_alloca_stats = Some(self.collect_remaining_alloca_stats());
            report.remaining_alloca_names = self.collect_remaining_alloca_names();
        }

        let serialize_started = Instant::now();
        let bitcode = self.module.bitcode();
        report.timings.push(EmitObjectTiming {
            name: "  emit_bitcode",
            duration: serialize_started.elapsed(),
        });

        unsafe {
            LLVMDisposeTargetData(target_data);
            LLVMDisposeTargetMachine(target_machine);
        }

        bitcode.map(|bitcode| (bitcode, report))
    }

    fn emit_to_file_windows(
        &self,
        target_triple_str: &str,
        output_path: &str,
        opt_level: OptLevel,
        collect_diagnostics: bool,
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

        let cleanup_before_stats =
            collect_diagnostics.then(|| self.collect_ir_instruction_stats().0);
        let optimize_started = Instant::now();
        self.run_llvm_pass_pipeline(target_machine, opt_level)?;
        report.timings.push(EmitObjectTiming {
            name: "  emit_opt_ir",
            duration: optimize_started.elapsed(),
        });
        if let Some(cleanup_before_stats) = cleanup_before_stats {
            let cleanup_after_stats = self.collect_ir_instruction_stats().0;
            report.ir_cleanup_stats = Some(IrCleanupStats {
                before: cleanup_before_stats,
                after: cleanup_after_stats,
            });
            report.remaining_alloca_stats = Some(self.collect_remaining_alloca_stats());
            report.remaining_alloca_names = self.collect_remaining_alloca_names();
        }

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

    fn run_llvm_pass_pipeline(
        &self,
        target_machine: LLVMTargetMachineRef,
        opt_level: OptLevel,
    ) -> Result<(), String> {
        let Some(pass_pipeline) = llvm_module_pass_pipeline(opt_level) else {
            return Ok(());
        };
        self.run_llvm_pipeline(target_machine, &pass_pipeline)
    }

    fn run_llvm_thin_lto_prelink_pipeline(
        &self,
        target_machine: LLVMTargetMachineRef,
        opt_level: OptLevel,
    ) -> Result<(), String> {
        let pass_pipeline = llvm_thin_lto_prelink_pipeline(opt_level);
        self.run_llvm_pipeline(target_machine, &pass_pipeline)
    }

    fn run_llvm_pipeline(
        &self,
        target_machine: LLVMTargetMachineRef,
        pass_pipeline: &CString,
    ) -> Result<(), String> {
        let options = unsafe { LLVMCreatePassBuilderOptions() };
        let err = unsafe {
            LLVMRunPasses(
                self.module.as_mut_ptr(),
                pass_pipeline.as_ptr(),
                target_machine,
                options,
            )
        };
        if !err.is_null() {
            unsafe { LLVMDisposePassBuilderOptions(options) };
            return Err(take_llvm_error(err));
        }
        unsafe { LLVMDisposePassBuilderOptions(options) };
        Ok(())
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

fn llvm_raw_opt_level(opt_level: OptLevel) -> LLVMCodeGenOptLevel {
    match opt_level {
        OptLevel::O0 => LLVMCodeGenOptLevel::LLVMCodeGenLevelNone,
        OptLevel::O1 => LLVMCodeGenOptLevel::LLVMCodeGenLevelLess,
        OptLevel::O2 => LLVMCodeGenOptLevel::LLVMCodeGenLevelDefault,
        OptLevel::O3 => LLVMCodeGenOptLevel::LLVMCodeGenLevelAggressive,
    }
}

fn llvm_module_pass_pipeline(opt_level: OptLevel) -> Option<CString> {
    let pipeline = match opt_level {
        OptLevel::O0 => return None,
        OptLevel::O1 => "always-inline,default<O1>",
        OptLevel::O2 => "always-inline,default<O2>",
        OptLevel::O3 => "always-inline,default<O3>",
    };
    Some(CString::new(pipeline).unwrap())
}

fn llvm_thin_lto_prelink_pipeline(opt_level: OptLevel) -> CString {
    let pipeline = match opt_level {
        OptLevel::O0 => "thinlto-pre-link<O0>",
        OptLevel::O1 => "thinlto-pre-link<O1>",
        OptLevel::O2 => "thinlto-pre-link<O2>",
        OptLevel::O3 => "thinlto-pre-link<O3>",
    };
    CString::new(pipeline).unwrap()
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
