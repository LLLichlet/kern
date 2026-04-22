use super::{
    CodeGenerator, EmitObjectReport, EmitObjectTiming, IrCleanupStats, IrInstructionStats,
};
use kernc_utils::config::{LlvmIrStage, OptLevel};
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
use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::Mutex;
use std::time::Instant;

static THIN_LTO_BITCODE_EMIT_LOCK: Mutex<()> = Mutex::new(());

struct EmissionTargetMachine {
    machine: LLVMTargetMachineRef,
    target_data: llvm_sys::target::LLVMTargetDataRef,
}

impl Drop for EmissionTargetMachine {
    fn drop(&mut self) {
        unsafe {
            if !self.target_data.is_null() {
                LLVMDisposeTargetData(self.target_data);
            }
            if !self.machine.is_null() {
                LLVMDisposeTargetMachine(self.machine);
            }
        }
    }
}

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
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

        let resources =
            self.create_emission_target_machine(target_triple_str, opt_level, &mut report)?;
        self.verify_module_for_emission(&mut report)?;

        if stage == LlvmIrStage::Optimized {
            let cleanup_before_stats =
                collect_diagnostics.then(|| self.collect_ir_instruction_stats().0);
            let optimize_started = Instant::now();
            self.run_llvm_pass_pipeline(resources.machine, opt_level)?;
            report.timings.push(EmitObjectTiming {
                name: "  emit_opt_ir",
                duration: optimize_started.elapsed(),
            });
            self.record_cleanup_diagnostics(&mut report, cleanup_before_stats);
        }

        let print_started = Instant::now();
        let print_result = self.print_ir();
        report.timings.push(EmitObjectTiming {
            name: "  emit_print_ir",
            duration: print_started.elapsed(),
        });

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
        let resources =
            self.create_emission_target_machine(target_triple_str, opt_level, &mut report)?;
        self.verify_module_for_emission(&mut report)?;

        let cleanup_before_stats =
            collect_diagnostics.then(|| self.collect_ir_instruction_stats().0);
        let optimize_started = Instant::now();
        self.run_llvm_pass_pipeline(resources.machine, opt_level)?;
        report.timings.push(EmitObjectTiming {
            name: "  emit_opt_ir",
            duration: optimize_started.elapsed(),
        });
        self.record_cleanup_diagnostics(&mut report, cleanup_before_stats);

        let mut output = output_path.as_bytes().to_vec();
        output.push(0);
        let mut err = ptr::null_mut();
        let backend_started = Instant::now();
        let emit_result = unsafe {
            LLVMTargetMachineEmitToFile(
                resources.machine,
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
        let resources =
            self.create_emission_target_machine(target_triple_str, opt_level, &mut report)?;
        self.verify_module_for_emission(&mut report)?;

        let cleanup_before_stats =
            collect_diagnostics.then(|| self.collect_ir_instruction_stats().0);
        let optimize_started = Instant::now();
        self.run_llvm_thin_lto_prelink_pipeline(resources.machine, opt_level)?;
        report.timings.push(EmitObjectTiming {
            name: "  emit_thinlto_prelink",
            duration: optimize_started.elapsed(),
        });
        self.record_cleanup_diagnostics(&mut report, cleanup_before_stats);

        let serialize_started = Instant::now();
        let bitcode = self.module.bitcode();
        report.timings.push(EmitObjectTiming {
            name: "  emit_bitcode",
            duration: serialize_started.elapsed(),
        });

        bitcode.map(|bitcode| (bitcode, report))
    }

    fn create_emission_target_machine(
        &self,
        target_triple_str: &str,
        opt_level: OptLevel,
        report: &mut EmitObjectReport,
    ) -> Result<EmissionTargetMachine, String> {
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
        let machine = create_target_machine(&triple, opt_level)?;
        let target_data = unsafe { LLVMCreateTargetDataLayout(machine) };
        unsafe {
            LLVMSetModuleDataLayout(self.module.as_mut_ptr(), target_data);
            LLVMSetTarget(self.module.as_mut_ptr(), triple.as_ptr());
        }
        report.timings.push(EmitObjectTiming {
            name: "  emit_setup",
            duration: setup_started.elapsed(),
        });
        Ok(EmissionTargetMachine {
            machine,
            target_data,
        })
    }

    fn verify_module_for_emission(&self, report: &mut EmitObjectReport) -> Result<(), String> {
        let verify_started = Instant::now();
        if let Err(err) = self.module.verify() {
            eprintln!("LLVM IR Verification Failed:\n{}", err);
            let _ = self.print_ir();
            return Err("Invalid LLVM IR generated".to_string());
        }
        report.timings.push(EmitObjectTiming {
            name: "  emit_verify",
            duration: verify_started.elapsed(),
        });
        Ok(())
    }

    fn record_cleanup_diagnostics(
        &self,
        report: &mut EmitObjectReport,
        cleanup_before_stats: Option<IrInstructionStats>,
    ) {
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
        unsafe {
            LLVMSetModuleDataLayout(self.module.as_mut_ptr(), target_data);
            LLVMSetTarget(self.module.as_mut_ptr(), triple.as_ptr());
        }
        let resources = EmissionTargetMachine {
            machine: target_machine,
            target_data,
        };
        report.timings.push(EmitObjectTiming {
            name: "  emit_setup",
            duration: setup_started.elapsed(),
        });

        self.verify_module_for_emission(&mut report)?;

        let cleanup_before_stats =
            collect_diagnostics.then(|| self.collect_ir_instruction_stats().0);
        let optimize_started = Instant::now();
        self.run_llvm_pass_pipeline(resources.machine, opt_level)?;
        report.timings.push(EmitObjectTiming {
            name: "  emit_opt_ir",
            duration: optimize_started.elapsed(),
        });
        self.record_cleanup_diagnostics(&mut report, cleanup_before_stats);

        if output_path.is_ascii() {
            let mut output = output_path.as_bytes().to_vec();
            output.push(0);
            let backend_started = Instant::now();
            let direct_result = unsafe {
                LLVMTargetMachineEmitToFile(
                    resources.machine,
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
                return Ok(report);
            }

            let _ = take_llvm_message(err);
            err = ptr::null_mut();
        }

        let mut mem_buf = ptr::null_mut();
        let backend_started = Instant::now();
        let result = unsafe {
            LLVMTargetMachineEmitToMemoryBuffer(
                resources.machine,
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
        }

        write_result.map(|_| report)
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
