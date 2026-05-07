use super::*;
use kernc_utils::config::{LinkerInputFlavor, OptLevel};
use std::fs;
use std::path::Path;

impl CompilerDriver {
    fn empty_compile_report(
        loaded_sources: Vec<PathBuf>,
        phase_timings: Vec<PhaseTiming>,
        cache_stats: CompileCacheStats,
    ) -> CompileReport {
        CompileReport {
            loaded_sources,
            phase_timings,
            cache_stats,
            lower_cache_stats: None,
            mast_workload: None,
            mir_workload: None,
            codegen_plan: None,
            ir_instruction_stats: None,
            ir_cleanup_stats: None,
            remaining_alloca_stats: None,
            remaining_alloca_names: Vec::new(),
            ir_hot_functions: Vec::new(),
            codegen_alloca_stats: Default::default(),
        }
    }

    fn emit_metadata_snapshot(
        &self,
        ctx: &SemaContext<'_>,
        phase_timings: &mut Vec<PhaseTiming>,
    ) -> Result<(), String> {
        let Some(metadata_output) = self.options.metadata_output.as_deref() else {
            return Ok(());
        };

        Self::measure_phase(phase_timings, "emit_kmeta", || {
            metadata::emit_package_metadata(
                ctx,
                Path::new(metadata_output),
                self.options
                    .metadata_package_name
                    .as_deref()
                    .or(self.options.root_module_name.as_deref())
                    .unwrap_or("root"),
                self.options.metadata_package_version.as_deref(),
            )
        })
    }

    pub fn compile(&self) -> bool {
        match self.compile_with_report() {
            Some(report) => {
                if self.options.report_timings {
                    Self::print_phase_timings(&report.phase_timings);
                    Self::print_cache_stats(report.cache_stats);
                    Self::print_lower_cache_stats(report.lower_cache_stats);
                    Self::print_mast_workload(report.mast_workload.as_ref());
                    Self::print_mir_workload(report.mir_workload.as_ref());
                    Self::print_codegen_plan(report.codegen_plan.as_ref());
                    Self::print_ir_instruction_stats(report.ir_instruction_stats.as_ref());
                    Self::print_ir_cleanup_stats(report.ir_cleanup_stats.as_ref());
                    Self::print_codegen_alloca_stats(report.codegen_alloca_stats);
                    Self::print_remaining_alloca_stats(report.remaining_alloca_stats);
                    Self::print_remaining_alloca_names(report.remaining_alloca_names.as_slice());
                    Self::print_ir_hot_functions(report.ir_hot_functions.as_slice());
                }
                true
            }
            None => false,
        }
    }

    pub fn compile_with_report(&self) -> Option<CompileReport> {
        if let Err(err) = kernc_utils::config::validate_compile_options(&self.options) {
            eprintln!("Error: {}", err);
            return None;
        }

        let cache_snapshot = self.cache_counter_snapshot();
        let mut phase_timings = Vec::new();
        if self.options.driver_mode == DriverMode::LinkOnly {
            let linked = Self::measure_phase(&mut phase_timings, "link", || self.link_only());
            return linked.then(|| {
                Self::empty_compile_report(
                    Vec::new(),
                    phase_timings,
                    self.cache_stats_since(cache_snapshot),
                )
            });
        }

        if self.options.driver_mode == DriverMode::CcCompile {
            let compiled =
                Self::measure_phase(&mut phase_timings, "cc_compile", || self.cc_compile_only());
            return compiled.then(|| {
                Self::empty_compile_report(
                    self.options
                        .input_file
                        .as_ref()
                        .map(PathBuf::from)
                        .into_iter()
                        .collect(),
                    phase_timings,
                    self.cache_stats_since(cache_snapshot),
                )
            });
        }

        let Some(input_file) = self.options.input_file.as_deref() else {
            eprintln!("Error: compile mode requires a source input.");
            return None;
        };

        let mut structure_session = Session::new();
        structure_session.apply_options(&self.options);
        let structure = match Self::measure_phase(&mut phase_timings, "analyze_structure", || {
            self.try_analyze_compile_structure(
                structure_session,
                input_file,
                &SourceOverrides::new(),
            )
        }) {
            Ok(structure) => structure,
            Err(session) => {
                Self::print_buffered_diagnostics(&session);
                return None;
            }
        };
        let crate::compiler::CompileStructureArtifact {
            session,
            snapshot,
            phase_timings: structure_phase_timings,
        } = structure;
        phase_timings.extend(structure_phase_timings);
        let mut session = session;

        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(snapshot);
        let Some(body_pipeline) = self.run_body_pipeline_with_report(&mut ctx) else {
            Self::print_buffered_diagnostics(ctx.sess);
            return None;
        };
        phase_timings.extend(body_pipeline.phase_timings.iter().copied());
        let loaded_sources = ctx
            .sess
            .source_manager
            .files()
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();

        if let Err(err) = self.emit_metadata_snapshot(&ctx, &mut phase_timings) {
            eprintln!("Error: Failed to emit kmeta snapshot: {}", err);
            return None;
        }

        if self.options.driver_mode == DriverMode::AnalyzeOnly {
            Self::print_buffered_diagnostics(ctx.sess);
            return Some(Self::empty_compile_report(
                loaded_sources,
                phase_timings,
                self.cache_stats_since(cache_snapshot),
            ));
        }

        let lowered = Self::measure_phase(&mut phase_timings, "lower", || {
            self.lower_module_with_flow_report(
                &mut ctx,
                &body_pipeline.flow_lowering_hints,
                &body_pipeline.lowered_module_items,
            )
        });
        let Some(lowered) = lowered else {
            Self::print_buffered_diagnostics(ctx.sess);
            return None;
        };
        phase_timings.extend(lowered.phase_timings.iter().copied());
        let mast_module = lowered.module;
        let mast_workload = mast_module.workload_stats();
        let mir_started = Instant::now();
        let mir_report = match kernc_mir_lower::try_build_from_mast(&mast_module) {
            Ok(report) => report,
            Err(error) => {
                ctx.sess.emit_error(error.span, error.message);
                Self::print_buffered_diagnostics(ctx.sess);
                return None;
            }
        };
        let mir_workload = mir_report.workload;
        phase_timings.push(PhaseTiming {
            name: "  mir_build",
            duration: mir_started.elapsed(),
        });

        let target = self.normalized_target();
        let module_name = self.module_name_for_codegen(input_file);
        let codegen_plan_started = Instant::now();
        let preserve_native_thin_linker_inputs = self.options.lto_mode == LtoMode::Thin
            && self.options.driver_mode.emits_linker_input()
            && self.options.emit_multi_linker_input_dir
            && !self.emit_thin_lto_bitcode_linker_input();
        let codegen_plan = Some(match self.options.lto_mode {
            LtoMode::Thin if !preserve_native_thin_linker_inputs => {
                plan_codegen_units_with_mir_summary(
                    &mast_module,
                    &mir_report.summary,
                    self.options.codegen_units,
                )
            }
            _ => plan_codegen_units_with_mir_workload(
                &mast_module,
                &mir_report.summary,
                self.options.codegen_units,
            ),
        });
        phase_timings.push(PhaseTiming {
            name: "  codegen_plan",
            duration: codegen_plan_started.elapsed(),
        });
        let codegen_plan_report = codegen_plan.as_ref().map(|plan| plan.report.clone());
        let codegen_unit_plans = codegen_plan.map(|plan| plan.units).unwrap_or_default();
        let collect_codegen_diagnostics = self.options.report_timings;

        let lower_cache_stats = lowered.cache_stats;
        let cache_stats = self.cache_stats_since(cache_snapshot);
        let report_context = CompileReportContext {
            loaded_sources: &loaded_sources,
            cache_stats,
            lower_cache_stats,
            mast_workload,
            mir_workload,
            codegen_plan: &codegen_plan_report,
            collect_codegen_diagnostics,
        };

        if codegen_unit_plans.is_empty() {
            let mut pipeline = CompilePipelineContext {
                sema: &mut ctx,
                phase_timings: &mut phase_timings,
                target: &target,
                module_name: &module_name,
                report: report_context,
            };
            return self.compile_single_unit(&mut pipeline, &mir_report.module);
        }

        let mut pipeline = CompilePipelineContext {
            sema: &mut ctx,
            phase_timings: &mut phase_timings,
            target: &target,
            module_name: &module_name,
            report: report_context,
        };
        self.compile_partitioned_units(&mut pipeline, &mast_module, &codegen_unit_plans)
    }

    fn compile_single_unit(
        &self,
        pipeline: &mut CompilePipelineContext<'_, '_>,
        mir_module: &kernc_mir::MirModule,
    ) -> Option<CompileReport> {
        let link_input_path = self.prepare_link_input_path(pipeline.target);
        let _guard = self.temp_link_input_guard(&link_input_path);
        let (codegen_report, emit_result) = {
            let codegen_ctx = Context::create();
            let mut codegen = CodeGenerator::new(
                &codegen_ctx,
                pipeline.module_name,
                &mut *pipeline.sema.sess,
                &pipeline.sema.type_registry,
                self.options.split_sections_for_gc,
            );
            codegen.set_asm_dialect(self.codegen_asm_dialect());
            codegen.set_debug_info(
                self.options.debug_info,
                self.options.opt_level != OptLevel::O0,
            );
            let codegen_report = Self::measure_phase(pipeline.phase_timings, "codegen", || {
                codegen.compile_mir(mir_module, pipeline.report.collect_codegen_diagnostics)
            });
            pipeline
                .phase_timings
                .extend(codegen_report.timings.iter().map(|timing| PhaseTiming {
                    name: timing.name,
                    duration: timing.duration,
                }));
            let emit_result = if self.options.driver_mode == DriverMode::EmitLlvmIr {
                Self::measure_phase(pipeline.phase_timings, "emit_llvm_ir", || {
                    codegen.emit_llvm_ir(
                        &pipeline.target.triple,
                        self.options.opt_level,
                        self.options.code_model,
                        self.options.emit_llvm_stage,
                        pipeline.report.collect_codegen_diagnostics,
                    )
                })
            } else if self.emit_thin_lto_bitcode_linker_input() {
                Self::measure_phase(pipeline.phase_timings, "emit_bitcode", || {
                    let (bitcode, report) = codegen.emit_thin_lto_bitcode(
                        &pipeline.target.triple,
                        self.options.opt_level,
                        self.options.code_model,
                        pipeline.report.collect_codegen_diagnostics,
                    )?;
                    fs::write(&link_input_path, bitcode)
                        .map_err(|err| format!("failed to stage ThinLTO linker input: {err}"))?;
                    Ok(report)
                })
            } else {
                Self::measure_phase(pipeline.phase_timings, "emit_object", || {
                    codegen.emit_to_file(
                        &pipeline.target.triple,
                        &link_input_path,
                        self.options.opt_level,
                        self.options.code_model,
                        pipeline.report.collect_codegen_diagnostics,
                    )
                })
            };
            (codegen_report, emit_result)
        };

        let emit_report = match emit_result {
            Ok(report) => report,
            Err(err) => {
                if self.options.driver_mode == DriverMode::EmitLlvmIr {
                    eprintln!("Error: Failed to print LLVM IR: {}", err);
                } else {
                    eprintln!("Error: LLVM failed to generate intermediate file: {}", err);
                }
                return None;
            }
        };
        pipeline
            .phase_timings
            .extend(emit_report.timings.iter().map(|timing| PhaseTiming {
                name: timing.name,
                duration: timing.duration,
            }));

        if self.options.driver_mode == DriverMode::EmitLlvmIr {
            Self::print_buffered_diagnostics(pipeline.sema.sess);
            return Some(Self::build_compile_report(
                pipeline.report,
                std::mem::take(pipeline.phase_timings),
                codegen_report,
                Some(emit_report),
            ));
        }

        if self.options.driver_mode.emits_linker_input() {
            Self::print_buffered_diagnostics(pipeline.sema.sess);
            if self.options.report_progress {
                println!(
                    "Successfully emitted linker input to `{}`",
                    self.options.output_file
                );
            }
            return Some(Self::build_compile_report(
                pipeline.report,
                std::mem::take(pipeline.phase_timings),
                codegen_report,
                Some(emit_report),
            ));
        }

        let linked = Self::measure_phase(pipeline.phase_timings, "link", || {
            self.run_link_command(
                Some(&link_input_path),
                pipeline.target,
                "Successfully compiled",
            )
        });
        if linked {
            Self::print_buffered_diagnostics(pipeline.sema.sess);
        }
        linked.then_some(Self::build_compile_report(
            pipeline.report,
            std::mem::take(pipeline.phase_timings),
            codegen_report,
            Some(emit_report),
        ))
    }

    pub(super) fn codegen_asm_dialect(&self) -> InlineAsmDialect {
        match self
            .options
            .asm_dialect
            .effective_for_target(&self.options.target)
        {
            AsmDialect::Intel => InlineAsmDialect::Intel,
            AsmDialect::Att => InlineAsmDialect::ATT,
            AsmDialect::Auto => unreachable!("effective_for_target must resolve `auto`"),
        }
    }

    pub(super) fn emit_thin_lto_bitcode_linker_input(&self) -> bool {
        self.options.driver_mode.emits_linker_input()
            && matches!(
                self.options.linker_input_flavor,
                LinkerInputFlavor::ThinLtoBitcode
            )
    }

    pub(in crate::compiler) fn make_thin_lto_cache_dir_path(&self) -> String {
        format!("{}.thinlto-cache.d", self.options.output_file)
    }
}
