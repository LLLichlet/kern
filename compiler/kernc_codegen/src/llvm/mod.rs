use inkwell::builder::Builder;
use inkwell::context::Context as LlvmContext;
use inkwell::llvm_sys::core::{
    LLVMDisposeMemoryBuffer, LLVMDisposeMessage, LLVMGetBufferSize, LLVMGetBufferStart,
    LLVMSetTarget,
};
use inkwell::llvm_sys::target_machine::{
    LLVMCodeGenFileType, LLVMCodeGenOptLevel, LLVMCodeModel, LLVMCreateTargetMachine,
    LLVMDisposeTargetMachine, LLVMGetTargetFromTriple, LLVMRelocMode,
    LLVMTargetMachineEmitToMemoryBuffer,
};
use inkwell::module::Module as LlvmModule;
use inkwell::targets::{CodeModel, FileType, InitializationConfig, RelocMode, Target};
use inkwell::types::StructType;
use inkwell::values::{FunctionValue, GlobalValue, PointerValue};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::ptr;

use kernc_mast::*;
use kernc_sema::def::DefId;
use kernc_sema::ty::{TypeId, TypeRegistry};
use kernc_utils::config::OptLevel;
use kernc_utils::{Session, SymbolId};

mod block;
mod decl;
mod expr;
mod types;

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

    locals: HashMap<kernc_utils::SymbolId, PointerValue<'ctx>>,
    loop_targets: Vec<(
        inkwell::basic_block::BasicBlock<'ctx>,
        inkwell::basic_block::BasicBlock<'ctx>,
    )>,
    asm_dialect: inkwell::InlineAsmDialect,

    def_mono_map: HashMap<(DefId, Vec<TypeId>), MonoId>,
    pure_enum_tag_map: HashMap<(DefId, Vec<TypeId>), TypeId>,
    adt_union_map: HashMap<MonoId, MonoId>,
    anon_struct_map: HashMap<TypeId, MonoId>,
    anon_union_map: HashMap<TypeId, MonoId>,
    anon_enum_map: HashMap<TypeId, MonoId>,
}

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub fn new(
        context: &'ctx LlvmContext,
        module_name: &str,
        sess: &'a mut Session,
        type_registry: &'a TypeRegistry,
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
            locals: HashMap::new(),
            loop_targets: Vec::new(),
            asm_dialect: inkwell::InlineAsmDialect::Intel,
            def_mono_map: HashMap::new(),
            pure_enum_tag_map: HashMap::new(),
            adt_union_map: HashMap::new(),
            anon_struct_map: HashMap::new(),
            anon_union_map: HashMap::new(),
            anon_enum_map: HashMap::new(),
        }
    }

    pub fn compile(&mut self, module: &MastModule) {
        self.def_mono_map = module.def_mono_map.clone();
        self.pure_enum_tag_map = module.pure_enum_tag_map.clone();
        self.adt_union_map = module.adt_union_map.clone();
        self.anon_struct_map = module.anon_struct_map.clone();
        self.anon_union_map = module.anon_union_map.clone();
        self.anon_enum_map = module.anon_enum_map.clone();

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

    pub fn set_asm_dialect(&mut self, dialect: inkwell::InlineAsmDialect) {
        self.asm_dialect = dialect;
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
        Target::initialize_native(&InitializationConfig::default())
            .map_err(|e| format!("Failed to initialize native target: {}", e))?;
        Target::initialize_all(&InitializationConfig::default());

        if target_triple_str.contains("windows") {
            return self.emit_to_file_windows(target_triple_str, output_path, opt_level);
        }

        let triple = inkwell::targets::TargetTriple::create(target_triple_str);
        let target = Target::from_triple(&triple).map_err(|e| e.to_string())?;
        let target_machine = target
            .create_target_machine(
                &triple,
                "generic",
                "",
                llvm_opt_level(opt_level),
                RelocMode::Default,
                CodeModel::Default,
            )
            .ok_or("Failed to create target machine")?;

        let target_data = target_machine.get_target_data();
        self.module.set_data_layout(&target_data.get_data_layout());
        self.module.set_triple(&triple);

        if let Err(err) = self.module.verify() {
            eprintln!("LLVM IR Verification Failed:\n{}", err);
            self.print_ir();
            return Err("Invalid LLVM IR generated".to_string());
        }

        target_machine
            .write_to_file(
                &self.module,
                FileType::Object,
                std::path::Path::new(output_path),
            )
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    fn emit_to_file_windows(
        &self,
        target_triple_str: &str,
        output_path: &str,
        opt_level: OptLevel,
    ) -> Result<(), String> {
        let triple = CString::new(target_triple_str).map_err(|_| {
            format!("Target triple contains an interior NUL byte: {target_triple_str:?}")
        })?;
        let cpu = CString::new("generic").unwrap();
        let features = CString::new("").unwrap();

        let mut target = ptr::null_mut();
        let mut err = ptr::null_mut();

        unsafe {
            if LLVMGetTargetFromTriple(triple.as_ptr(), &mut target, &mut err) != 0 {
                return Err(take_llvm_message(err));
            }
        }

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
            return Err("Failed to create target machine".to_string());
        }

        // Keep the Windows module target explicit, but avoid the unstable inkwell
        // TargetTriple wrapper/drop path by setting it with the raw C API.
        unsafe {
            LLVMSetTarget(self.module.as_mut_ptr(), triple.as_ptr());
        }

        // LLVM's Windows file-emission path still goes through narrow paths here.
        // Emit to memory and let Rust write the bytes so Unicode output paths work.
        let mut mem_buf = ptr::null_mut();
        let result = unsafe {
            LLVMTargetMachineEmitToMemoryBuffer(
                target_machine,
                self.module.as_mut_ptr(),
                LLVMCodeGenFileType::LLVMObjectFile,
                &mut err,
                &mut mem_buf,
            )
        };

        if result != 0 {
            unsafe {
                LLVMDisposeTargetMachine(target_machine);
            }
            return Err(take_llvm_message(err));
        }

        let write_result = unsafe {
            let bytes = std::slice::from_raw_parts(
                LLVMGetBufferStart(mem_buf) as *const u8,
                LLVMGetBufferSize(mem_buf),
            );
            std::fs::write(output_path, bytes)
        }
        .map_err(|e| format!("Failed to write object file `{}`: {}", output_path, e));

        unsafe {
            LLVMDisposeMemoryBuffer(mem_buf);
            LLVMDisposeTargetMachine(target_machine);
        }

        write_result
    }

    fn resolve_symbol(&self, sym: kernc_utils::SymbolId) -> &str {
        self.sess.interner.resolve(sym).unwrap_or("<unknown>")
    }
}

fn llvm_opt_level(opt_level: OptLevel) -> inkwell::OptimizationLevel {
    match opt_level {
        OptLevel::O0 => inkwell::OptimizationLevel::None,
        OptLevel::O1 => inkwell::OptimizationLevel::Less,
        OptLevel::O2 => inkwell::OptimizationLevel::Default,
        OptLevel::O3 => inkwell::OptimizationLevel::Aggressive,
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
