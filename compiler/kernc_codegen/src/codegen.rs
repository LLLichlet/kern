use crate::llvm_api::{
    Builder, Context as LlvmContext, FunctionValue, GlobalValue, InlineAsmDialect,
    Module as LlvmModule, PointerValue, StructType,
};
use llvm_sys::core::{
    LLVMDisposeMemoryBuffer, LLVMDisposeMessage, LLVMGetBufferSize, LLVMGetBufferStart,
    LLVMSetTarget,
};
use llvm_sys::target::{LLVMDisposeTargetData, LLVMSetModuleDataLayout, LLVM_InitializeAllAsmParsers,
    LLVM_InitializeAllAsmPrinters, LLVM_InitializeAllTargetInfos, LLVM_InitializeAllTargetMCs,
    LLVM_InitializeAllTargets, LLVM_InitializeNativeAsmParser, LLVM_InitializeNativeAsmPrinter,
    LLVM_InitializeNativeTarget};
use llvm_sys::target_machine::{
    LLVMCodeGenFileType, LLVMCodeGenOptLevel, LLVMCodeModel, LLVMCreateTargetMachine,
    LLVMCreateTargetDataLayout, LLVMDisposeTargetMachine, LLVMGetTargetFromTriple, LLVMRelocMode,
    LLVMTargetMachineEmitToFile, LLVMTargetMachineEmitToMemoryBuffer, LLVMTargetMachineRef,
    LLVMTargetRef,
};
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
            asm_dialect: InlineAsmDialect::Intel,
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

    pub fn set_asm_dialect(&mut self, dialect: InlineAsmDialect) {
        self.asm_dialect = dialect;
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
    ) -> Result<(), String> {
        initialize_llvm_targets();

        if target_triple_str.contains("windows") {
            return self.emit_to_file_windows(target_triple_str, output_path, opt_level);
        }

        let triple = CString::new(target_triple_str).map_err(|_| {
            format!("Target triple contains an interior NUL byte: {target_triple_str:?}")
        })?;
        let target_machine = create_target_machine(&triple, opt_level)?;
        let target_data = unsafe { LLVMCreateTargetDataLayout(target_machine) };
        unsafe {
            LLVMSetModuleDataLayout(self.module.as_mut_ptr(), target_data);
            LLVMSetTarget(self.module.as_mut_ptr(), triple.as_ptr());
        }

        if let Err(err) = self.module.verify() {
            eprintln!("LLVM IR Verification Failed:\n{}", err);
            let _ = self.print_ir();
            unsafe {
                LLVMDisposeTargetData(target_data);
                LLVMDisposeTargetMachine(target_machine);
            }
            return Err("Invalid LLVM IR generated".to_string());
        }

        let mut output = output_path.as_bytes().to_vec();
        output.push(0);
        let mut err = ptr::null_mut();
        let emit_result = unsafe {
            LLVMTargetMachineEmitToFile(
                target_machine,
                self.module.as_mut_ptr(),
                output.as_mut_ptr() as *mut _,
                LLVMCodeGenFileType::LLVMObjectFile,
                &mut err,
            )
        };
        unsafe {
            LLVMDisposeTargetData(target_data);
            LLVMDisposeTargetMachine(target_machine);
        }

        if emit_result != 0 {
            return Err(take_llvm_message(err));
        }

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

        let target_machine = create_target_machine_from_parts(target, &triple, &cpu, &features, opt_level)?;
        let target_data = unsafe { LLVMCreateTargetDataLayout(target_machine) };

        // Keep the Windows module target explicit and set the triple through
        // the raw LLVM C API so we fully control ownership/lifetime here.
        unsafe {
            LLVMSetModuleDataLayout(self.module.as_mut_ptr(), target_data);
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
            std::fs::write(output_path, bytes)
        }
        .map_err(|e| format!("Failed to write object file `{}`: {}", output_path, e));

        unsafe {
            LLVMDisposeMemoryBuffer(mem_buf);
            LLVMDisposeTargetData(target_data);
            LLVMDisposeTargetMachine(target_machine);
        }

        write_result
    }

    fn resolve_symbol(&self, sym: kernc_utils::SymbolId) -> &str {
        self.sess.interner.resolve(sym).unwrap_or("<unknown>")
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
    unsafe {
        let _ = LLVM_InitializeNativeTarget();
        let _ = LLVM_InitializeNativeAsmPrinter();
        let _ = LLVM_InitializeNativeAsmParser();
        LLVM_InitializeAllTargetInfos();
        LLVM_InitializeAllTargets();
        LLVM_InitializeAllTargetMCs();
        LLVM_InitializeAllAsmPrinters();
        LLVM_InitializeAllAsmParsers();
    }
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
