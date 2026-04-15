use super::*;
use crate::compiler::codegen_units;
use crate::compiler::{TempDirGuard, TempFileGuard};
use kernc_utils::config::LinkerInputFlavor;
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

        let Some(input_file) = self.options.input_file.as_deref() else {
            eprintln!("Error: compile mode requires a source input.");
            return None;
        };

        let structure = Self::measure_phase(&mut phase_timings, "analyze_structure", || {
            self.analyze_compile_structure(input_file, &SourceOverrides::new())
        })?;
        let crate::compiler::CompileStructureArtifact {
            session,
            snapshot,
            phase_timings: structure_phase_timings,
        } = structure;
        phase_timings.extend(structure_phase_timings);
        let mut session = session;

        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(snapshot);
        let body_pipeline = self.run_body_pipeline_with_report(&mut ctx)?;
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
        })?;
        phase_timings.extend(lowered.phase_timings.iter().copied());
        let mast_module = lowered.module;
        let mast_workload = mast_module.workload_stats();
        let mir_started = Instant::now();
        let mir_report = kernc_mir_lower::build_from_mast(&mast_module);
        let mir_workload = mir_report.workload;
        phase_timings.push(PhaseTiming {
            name: "  mir_build",
            duration: mir_started.elapsed(),
        });

        let target = self.normalized_target();
        let module_name = self.module_name_for_codegen(input_file);
        let codegen_plan_started = Instant::now();
        let codegen_plan = Some(match self.options.lto_mode {
            LtoMode::Thin => plan_codegen_units_with_mir_summary(
                &mast_module,
                &mir_report.summary,
                self.options.codegen_units,
            ),
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
                        self.options.emit_llvm_stage,
                        pipeline.report.collect_codegen_diagnostics,
                    )
                })
            } else if self.emit_thin_lto_bitcode_linker_input() {
                Self::measure_phase(pipeline.phase_timings, "emit_bitcode", || {
                    let (bitcode, report) = codegen.emit_thin_lto_bitcode(
                        &pipeline.target.triple,
                        self.options.opt_level,
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

    fn codegen_asm_dialect(&self) -> InlineAsmDialect {
        match self.options.asm_dialect {
            AsmDialect::Intel => InlineAsmDialect::Intel,
            AsmDialect::Att => InlineAsmDialect::ATT,
        }
    }

    fn compile_partitioned_units(
        &self,
        pipeline: &mut CompilePipelineContext<'_, '_>,
        mast_module: &kernc_mast::MastModule,
        codegen_unit_plans: &[codegen_units::CodegenUnitPlan],
    ) -> Option<CompileReport> {
        if self.options.lto_mode == LtoMode::Full {
            return self.compile_partitioned_units_full_lto(
                pipeline,
                mast_module,
                codegen_unit_plans,
            );
        }
        if self.options.lto_mode == LtoMode::Thin {
            return self.compile_partitioned_units_thin_lto(
                pipeline,
                mast_module,
                codegen_unit_plans,
            );
        }

        let emit_multi_linker_input_dir = self.options.driver_mode.emits_linker_input()
            && self.options.emit_multi_linker_input_dir;
        if emit_multi_linker_input_dir && !self.prepare_multi_linker_input_output_dir() {
            return None;
        }
        let object_guards = if emit_multi_linker_input_dir {
            Vec::new()
        } else {
            codegen_unit_plans
                .iter()
                .map(|unit| TempFileGuard {
                    path: self.make_temp_codegen_unit_path(&unit.name),
                })
                .collect::<Vec<_>>()
        };
        let build_context = CodegenUnitBuildContext {
            module_name: pipeline.module_name,
            target_triple: &pipeline.target.triple,
            session: pipeline.sema.sess,
            type_registry: &pipeline.sema.type_registry,
            collect_diagnostics: pipeline.report.collect_codegen_diagnostics,
        };
        let unit_batch =
            match self.codegen_unit_artifacts(mast_module, codegen_unit_plans, &build_context) {
                Ok(artifacts) => artifacts,
                Err(err) => {
                    eprintln!("Error: LLVM failed to generate intermediate file: {}", err);
                    drop(object_guards);
                    return None;
                }
            };
        let mut codegen_report = CodegenReport::default();
        let mut emit_report = EmitObjectReport::default();
        let mut object_paths = vec![String::new(); unit_batch.artifacts.len()];
        for artifact in unit_batch.artifacts {
            Self::absorb_codegen_report(&mut codegen_report, artifact.codegen_report);
            Self::absorb_emit_report(&mut emit_report, artifact.emit_report);
            object_paths[artifact.index] = artifact.object_path;
        }

        pipeline.phase_timings.push(PhaseTiming {
            name: "codegen_units",
            duration: unit_batch.wall_duration,
        });
        pipeline
            .phase_timings
            .extend(codegen_report.timings.iter().map(|timing| PhaseTiming {
                name: timing.name,
                duration: timing.duration,
            }));
        pipeline
            .phase_timings
            .extend(emit_report.timings.iter().map(|timing| PhaseTiming {
                name: timing.name,
                duration: timing.duration,
            }));

        if self.options.driver_mode.emits_linker_input() {
            if emit_multi_linker_input_dir {
                if let Err(err) = self.write_multi_linker_input_manifest(&object_paths) {
                    eprintln!(
                        "Error: Failed to record preserved linker inputs `{}`: {}",
                        self.options.output_file, err
                    );
                    return None;
                }
                Self::print_buffered_diagnostics(pipeline.sema.sess);
                return Some(Self::build_compile_report(
                    pipeline.report,
                    std::mem::take(pipeline.phase_timings),
                    codegen_report,
                    Some(emit_report),
                ));
            }

            let merged_output_path = self.make_temp_relocatable_merge_path();
            let merged_output_guard = TempFileGuard {
                path: merged_output_path.clone(),
            };
            let merged = Self::measure_phase(pipeline.phase_timings, "merge_object", || {
                self.run_relocatable_link_command(
                    &object_paths,
                    pipeline.target,
                    &merged_output_path,
                    &self.options.output_file,
                    "Successfully emitted linker input",
                )
            });
            drop(object_guards);
            if !merged {
                drop(merged_output_guard);
                return None;
            }
            if let Err(err) = std::fs::rename(&merged_output_path, &self.options.output_file) {
                eprintln!(
                    "Error: Failed to stage merged linker input `{}`: {}",
                    self.options.output_file, err
                );
                drop(merged_output_guard);
                return None;
            }
            drop(merged_output_guard);
            Self::print_buffered_diagnostics(pipeline.sema.sess);
            return Some(Self::build_compile_report(
                pipeline.report,
                std::mem::take(pipeline.phase_timings),
                codegen_report,
                Some(emit_report),
            ));
        }

        let linked = Self::measure_phase(pipeline.phase_timings, "link", || {
            self.run_link_command_with_inputs(
                &object_paths,
                pipeline.target,
                "Successfully compiled",
            )
        });
        drop(object_guards);
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

    fn compile_partitioned_units_thin_lto(
        &self,
        pipeline: &mut CompilePipelineContext<'_, '_>,
        mast_module: &kernc_mast::MastModule,
        codegen_unit_plans: &[codegen_units::CodegenUnitPlan],
    ) -> Option<CompileReport> {
        type CompletedThinLtoUnit = (usize, String, Vec<u8>, CodegenReport, EmitObjectReport);

        let started = Instant::now();
        let mir_unit_batch =
            match self.partitioned_mir_unit_reports(mast_module, codegen_unit_plans) {
                Ok(batch) => batch,
                Err(err) => {
                    eprintln!("Error: failed to build partitioned MIR units for ThinLTO: {err}");
                    return None;
                }
            };
        let asm_dialect = self.codegen_asm_dialect();
        let worker_count = Self::codegen_worker_count(mir_unit_batch.reports.len());
        let mut pending = mir_unit_batch.reports.iter().collect::<Vec<_>>();
        let mut completed = Vec::<CompletedThinLtoUnit>::with_capacity(pending.len());
        while !pending.is_empty() {
            let take = worker_count.min(pending.len());
            let chunk = pending.drain(..take).collect::<Vec<_>>();
            let mut chunk_results = match std::thread::scope(|scope| {
                let mut handles = Vec::<
                    std::thread::ScopedJoinHandle<'_, Result<CompletedThinLtoUnit, String>>,
                >::with_capacity(chunk.len());
                for unit in chunk {
                    let mut worker_session = pipeline.sema.sess.clone();
                    let worker_registry = pipeline.sema.type_registry.clone();
                    let module_name = format!("{}_{}", pipeline.module_name, unit.unit_name);
                    let target_triple = pipeline.target.triple.clone();
                    let split_sections_for_gc = self.options.split_sections_for_gc;
                    let opt_level = self.options.opt_level;
                    let collect_diagnostics = pipeline.report.collect_codegen_diagnostics;
                    handles.push(scope.spawn(move || {
                        let codegen_ctx = Context::create();
                        let mut codegen = CodeGenerator::new(
                            &codegen_ctx,
                            &module_name,
                            &mut worker_session,
                            &worker_registry,
                            split_sections_for_gc,
                        );
                        codegen.set_asm_dialect(asm_dialect);
                        let codegen_report =
                            codegen.compile_mir(&unit.mir_report.module, collect_diagnostics);
                        let (bitcode, emit_report) = codegen.emit_thin_lto_bitcode(
                            &target_triple,
                            opt_level,
                            collect_diagnostics,
                        )?;
                        Ok::<_, String>((
                            unit.index,
                            unit.unit_name.clone(),
                            bitcode,
                            codegen_report,
                            emit_report,
                        ))
                    }));
                }

                let mut results = Vec::with_capacity(handles.len());
                for handle in handles {
                    let result = handle
                        .join()
                        .map_err(|_| "parallel ThinLTO LLVM worker panicked".to_string())??;
                    results.push(result);
                }
                Ok::<_, String>(results)
            }) {
                Ok(results) => results,
                Err(err) => {
                    eprintln!("Error: LLVM failed to build a ThinLTO codegen unit: {err}");
                    return None;
                }
            };
            completed.append(&mut chunk_results);
        }
        completed.sort_by_key(|(index, _, _, _, _)| *index);

        let mut codegen_report = CodegenReport::default();
        let mut emit_report = EmitObjectReport::default();
        let thin_modules = completed
            .into_iter()
            .map(|(_, unit_name, bitcode, unit_report, unit_emit_report)| {
                Self::absorb_codegen_report(&mut codegen_report, unit_report);
                Self::absorb_emit_report(&mut emit_report, unit_emit_report);
                ThinLtoModule {
                    identifier: format!("{}_{}", pipeline.module_name, unit_name),
                    bitcode,
                }
            })
            .collect::<Vec<_>>();

        pipeline.phase_timings.push(PhaseTiming {
            name: "codegen_units",
            duration: started.elapsed(),
        });
        pipeline.phase_timings.push(PhaseTiming {
            name: "  mir_units",
            duration: mir_unit_batch.wall_duration,
        });
        pipeline
            .phase_timings
            .extend(codegen_report.timings.iter().map(|timing| PhaseTiming {
                name: timing.name,
                duration: timing.duration,
            }));
        pipeline
            .phase_timings
            .extend(emit_report.timings.iter().map(|timing| PhaseTiming {
                name: timing.name,
                duration: timing.duration,
            }));

        let thin_lto_started = Instant::now();
        let emit_multi_linker_input_dir = self.options.driver_mode.emits_linker_input()
            && self.options.emit_multi_linker_input_dir;
        if self.emit_thin_lto_bitcode_linker_input() {
            if emit_multi_linker_input_dir && !self.prepare_multi_linker_input_output_dir() {
                return None;
            }
            let mut link_input_paths = Vec::with_capacity(thin_modules.len());
            for (index, module) in thin_modules.iter().enumerate() {
                let link_input_path = if emit_multi_linker_input_dir {
                    self.make_multi_linker_input_codegen_unit_path(&format!("thin{index}"))
                } else {
                    self.options.output_file.clone()
                };
                if let Err(err) = fs::write(&link_input_path, &module.bitcode) {
                    eprintln!(
                        "Error: Failed to materialize ThinLTO linker input `{}`: {}",
                        link_input_path, err
                    );
                    return None;
                }
                link_input_paths.push(link_input_path);
            }
            if emit_multi_linker_input_dir
                && let Err(err) = self.write_multi_linker_input_manifest(&link_input_paths)
            {
                eprintln!(
                    "Error: Failed to record preserved linker inputs `{}`: {}",
                    self.options.output_file, err
                );
                return None;
            }
            Self::print_buffered_diagnostics(pipeline.sema.sess);
            return Some(Self::build_compile_report(
                pipeline.report,
                std::mem::take(pipeline.phase_timings),
                codegen_report,
                Some(emit_report),
            ));
        }

        let thin_lto_output_dir = self.make_temp_thin_lto_output_dir_path();
        if !Self::prepare_clean_output_dir(
            Path::new(&thin_lto_output_dir),
            "ThinLTO object directory",
        ) {
            return None;
        }
        let thin_lto_cache_dir = self.make_thin_lto_cache_dir_path();
        if !Self::ensure_output_dir(Path::new(&thin_lto_cache_dir), "ThinLTO cache directory") {
            return None;
        }
        let thin_lto_output_guard = TempDirGuard {
            path: thin_lto_output_dir.clone(),
        };
        let object_outputs = match run_thin_lto(
            &thin_modules,
            &kernc_codegen::ThinLtoOptions {
                generated_objects_dir: Some(PathBuf::from(&thin_lto_output_dir)),
                cache_dir: Some(PathBuf::from(&thin_lto_cache_dir)),
            },
        ) {
            Ok(objects) => objects,
            Err(err) => {
                eprintln!("Error: LLVM ThinLTO failed during post-link processing: {err}");
                return None;
            }
        };
        pipeline.phase_timings.push(PhaseTiming {
            name: "thin_lto",
            duration: thin_lto_started.elapsed(),
        });

        let mut object_paths = object_outputs
            .into_iter()
            .enumerate()
            .map(|(index, object)| match object {
                kernc_codegen::ThinLtoObject::File(path) => Ok((index, path)),
                kernc_codegen::ThinLtoObject::Buffer(_) => Err(format!(
                    "ThinLTO returned an unexpected in-memory object for output #{index}"
                )),
            })
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_else(|err| {
                eprintln!("Error: {err}");
                Vec::new()
            });
        if object_paths.is_empty() {
            return None;
        }

        if emit_multi_linker_input_dir && !self.prepare_multi_linker_input_output_dir() {
            return None;
        }
        if emit_multi_linker_input_dir {
            let mut preserved_paths = Vec::with_capacity(object_paths.len());
            for (index, object_path) in &object_paths {
                let preserved_path =
                    self.make_multi_linker_input_codegen_unit_path(&format!("thin{index}"));
                if let Err(err) = fs::copy(object_path, &preserved_path) {
                    eprintln!(
                        "Error: Failed to preserve ThinLTO object `{}` as `{}`: {}",
                        object_path.display(),
                        preserved_path,
                        err
                    );
                    return None;
                }
                preserved_paths.push(preserved_path);
            }
            object_paths = preserved_paths
                .into_iter()
                .enumerate()
                .map(|(index, path)| (index, PathBuf::from(path)))
                .collect();
        }

        if self.options.driver_mode.emits_linker_input() {
            if emit_multi_linker_input_dir {
                let manifest_paths = object_paths
                    .iter()
                    .map(|(_, path)| path.to_string_lossy().to_string())
                    .collect::<Vec<_>>();
                if let Err(err) = self.write_multi_linker_input_manifest(&manifest_paths) {
                    eprintln!(
                        "Error: Failed to record preserved linker inputs `{}`: {}",
                        self.options.output_file, err
                    );
                    return None;
                }
                Self::print_buffered_diagnostics(pipeline.sema.sess);
                drop(thin_lto_output_guard);
                return Some(Self::build_compile_report(
                    pipeline.report,
                    std::mem::take(pipeline.phase_timings),
                    codegen_report,
                    Some(emit_report),
                ));
            }

            let merged_output_path = self.make_temp_relocatable_merge_path();
            let merged_output_guard = TempFileGuard {
                path: merged_output_path.clone(),
            };
            let merged = Self::measure_phase(pipeline.phase_timings, "merge_object", || {
                let inputs = object_paths
                    .iter()
                    .map(|(_, path)| path.to_string_lossy().to_string())
                    .collect::<Vec<_>>();
                self.run_relocatable_link_command(
                    &inputs,
                    pipeline.target,
                    &merged_output_path,
                    &self.options.output_file,
                    "Successfully emitted linker input",
                )
            });
            if !merged {
                drop(merged_output_guard);
                return None;
            }
            if let Err(err) = std::fs::rename(&merged_output_path, &self.options.output_file) {
                eprintln!(
                    "Error: Failed to stage merged linker input `{}`: {}",
                    self.options.output_file, err
                );
                drop(merged_output_guard);
                return None;
            }
            drop(merged_output_guard);
            Self::print_buffered_diagnostics(pipeline.sema.sess);
            drop(thin_lto_output_guard);
            return Some(Self::build_compile_report(
                pipeline.report,
                std::mem::take(pipeline.phase_timings),
                codegen_report,
                Some(emit_report),
            ));
        }

        let linked = Self::measure_phase(pipeline.phase_timings, "link", || {
            let inputs = object_paths
                .iter()
                .map(|(_, path)| path.to_string_lossy().to_string())
                .collect::<Vec<_>>();
            self.run_link_command_with_inputs(&inputs, pipeline.target, "Successfully compiled")
        });
        if linked {
            Self::print_buffered_diagnostics(pipeline.sema.sess);
        }
        drop(thin_lto_output_guard);
        linked.then_some(Self::build_compile_report(
            pipeline.report,
            std::mem::take(pipeline.phase_timings),
            codegen_report,
            Some(emit_report),
        ))
    }

    fn compile_partitioned_units_full_lto(
        &self,
        pipeline: &mut CompilePipelineContext<'_, '_>,
        mast_module: &kernc_mast::MastModule,
        codegen_unit_plans: &[codegen_units::CodegenUnitPlan],
    ) -> Option<CompileReport> {
        type CompletedFullLtoUnit = (usize, String, Vec<u8>, CodegenReport);

        let started = Instant::now();
        let mir_unit_batch =
            match self.partitioned_mir_unit_reports(mast_module, codegen_unit_plans) {
                Ok(batch) => batch,
                Err(err) => {
                    eprintln!("Error: failed to build partitioned MIR units for full LTO: {err}");
                    return None;
                }
            };
        let asm_dialect = self.codegen_asm_dialect();
        let worker_count = Self::codegen_worker_count(mir_unit_batch.reports.len());
        let mut pending = mir_unit_batch.reports.iter().collect::<Vec<_>>();
        let mut completed = Vec::<CompletedFullLtoUnit>::with_capacity(pending.len());
        while !pending.is_empty() {
            let take = worker_count.min(pending.len());
            let chunk = pending.drain(..take).collect::<Vec<_>>();
            let mut chunk_results = match std::thread::scope(|scope| {
                let mut handles = Vec::<
                    std::thread::ScopedJoinHandle<'_, Result<CompletedFullLtoUnit, String>>,
                >::with_capacity(chunk.len());
                for unit in chunk {
                    let mut worker_session = pipeline.sema.sess.clone();
                    let worker_registry = pipeline.sema.type_registry.clone();
                    let module_name = format!("{}_{}", pipeline.module_name, unit.unit_name);
                    let split_sections_for_gc = self.options.split_sections_for_gc;
                    let collect_diagnostics = pipeline.report.collect_codegen_diagnostics;
                    handles.push(scope.spawn(move || {
                        let codegen_ctx = Context::create();
                        let mut codegen = CodeGenerator::new(
                            &codegen_ctx,
                            &module_name,
                            &mut worker_session,
                            &worker_registry,
                            split_sections_for_gc,
                        );
                        codegen.set_asm_dialect(asm_dialect);
                        let codegen_report =
                            codegen.compile_mir(&unit.mir_report.module, collect_diagnostics);
                        let bitcode = codegen.into_module().bitcode()?;
                        Ok::<_, String>((
                            unit.index,
                            unit.unit_name.clone(),
                            bitcode,
                            codegen_report,
                        ))
                    }));
                }

                let mut results = Vec::with_capacity(handles.len());
                for handle in handles {
                    let result = handle
                        .join()
                        .map_err(|_| "parallel full-LTO LLVM worker panicked".to_string())??;
                    results.push(result);
                }
                Ok::<_, String>(results)
            }) {
                Ok(results) => results,
                Err(err) => {
                    eprintln!("Error: LLVM failed to build a full-LTO codegen unit: {err}");
                    return None;
                }
            };
            completed.append(&mut chunk_results);
        }
        completed.sort_by_key(|(index, _, _, _)| *index);

        let codegen_ctx = Context::create();
        let mut codegen_report = CodegenReport::default();
        let mut link_duration = Duration::default();
        if completed.is_empty() {
            eprintln!("Error: full LTO requires at least one materialized codegen unit.");
            return None;
        }
        let mut merged_session = pipeline.sema.sess.clone();
        let mut merged_codegen = CodeGenerator::new(
            &codegen_ctx,
            &format!("{}_full_lto", pipeline.module_name),
            &mut merged_session,
            &pipeline.sema.type_registry,
            self.options.split_sections_for_gc,
        );
        merged_codegen.set_asm_dialect(self.codegen_asm_dialect());
        for (_, unit_name, bitcode, unit_report) in completed {
            Self::absorb_codegen_report(&mut codegen_report, unit_report);
            let unit_module = match codegen_ctx
                .parse_bitcode_module(&format!("{}_{}", pipeline.module_name, unit_name), &bitcode)
            {
                Ok(module) => module,
                Err(err) => {
                    eprintln!(
                        "Error: LLVM failed to deserialize codegen unit `{}` for full LTO: {}",
                        unit_name, err
                    );
                    return None;
                }
            };
            let link_started = Instant::now();
            if let Err(err) = merged_codegen.link_module(unit_module) {
                eprintln!(
                    "Error: LLVM failed to link codegen unit `{}` into the full-LTO module: {}",
                    unit_name, err
                );
                return None;
            }
            link_duration += link_started.elapsed();
        }

        pipeline.phase_timings.push(PhaseTiming {
            name: "codegen_units",
            duration: started.elapsed(),
        });
        pipeline.phase_timings.push(PhaseTiming {
            name: "  mir_units",
            duration: mir_unit_batch.wall_duration,
        });
        if !link_duration.is_zero() {
            pipeline.phase_timings.push(PhaseTiming {
                name: "lto_link",
                duration: link_duration,
            });
        }
        pipeline
            .phase_timings
            .extend(codegen_report.timings.iter().map(|timing| PhaseTiming {
                name: timing.name,
                duration: timing.duration,
            }));
        let link_input_path = self.prepare_link_input_path(pipeline.target);
        let _guard = self.temp_link_input_guard(&link_input_path);
        let emit_result = if self.options.driver_mode == DriverMode::EmitLlvmIr {
            Self::measure_phase(pipeline.phase_timings, "emit_llvm_ir", || {
                merged_codegen.emit_llvm_ir(
                    &pipeline.target.triple,
                    self.options.opt_level,
                    self.options.emit_llvm_stage,
                    pipeline.report.collect_codegen_diagnostics,
                )
            })
        } else {
            Self::measure_phase(pipeline.phase_timings, "emit_object", || {
                merged_codegen.emit_to_file(
                    &pipeline.target.triple,
                    &link_input_path,
                    self.options.opt_level,
                    pipeline.report.collect_codegen_diagnostics,
                )
            })
        };
        drop(merged_codegen);

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
            Self::print_buffered_diagnostics(&merged_session);
            return Some(Self::build_compile_report(
                pipeline.report,
                std::mem::take(pipeline.phase_timings),
                codegen_report,
                Some(emit_report),
            ));
        }

        if self.options.driver_mode.emits_linker_input() {
            Self::print_buffered_diagnostics(&merged_session);
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
            Self::print_buffered_diagnostics(&merged_session);
        }
        linked.then_some(Self::build_compile_report(
            pipeline.report,
            std::mem::take(pipeline.phase_timings),
            codegen_report,
            Some(emit_report),
        ))
    }

    fn partitioned_mir_unit_reports(
        &self,
        mast_module: &kernc_mast::MastModule,
        codegen_unit_plans: &[codegen_units::CodegenUnitPlan],
    ) -> Result<MirUnitBatch, String> {
        type CompletedMirUnit = (usize, String, kernc_mir_lower::MirBuildReport);

        let started = Instant::now();
        let worker_count = Self::codegen_worker_count(codegen_unit_plans.len());
        if worker_count <= 1 {
            let reports = codegen_unit_plans
                .iter()
                .enumerate()
                .map(|(index, unit)| {
                    let unit_module = materialize_codegen_unit(mast_module, unit);
                    PartitionedMirUnitReport {
                        index,
                        unit_name: unit.name.clone(),
                        mir_report: kernc_mir_lower::build_from_mast(&unit_module),
                    }
                })
                .collect::<Vec<_>>();
            return Ok(MirUnitBatch {
                reports,
                wall_duration: started.elapsed(),
            });
        }

        let mut pending = codegen_unit_plans.iter().enumerate().collect::<Vec<_>>();
        let mut completed = Vec::with_capacity(pending.len());

        while !pending.is_empty() {
            let take = worker_count.min(pending.len());
            let chunk = pending.drain(..take).collect::<Vec<_>>();
            let mut chunk_results = std::thread::scope(|scope| {
                let mut handles = Vec::<
                    std::thread::ScopedJoinHandle<'_, Result<CompletedMirUnit, String>>,
                >::with_capacity(chunk.len());
                for (index, unit) in chunk {
                    handles.push(scope.spawn(move || {
                        let unit_module = materialize_codegen_unit(mast_module, unit);
                        Ok::<_, String>((
                            index,
                            unit.name.clone(),
                            kernc_mir_lower::build_from_mast(&unit_module),
                        ))
                    }));
                }

                let mut results = Vec::with_capacity(handles.len());
                for handle in handles {
                    let result = handle
                        .join()
                        .map_err(|_| "parallel full-LTO MIR worker panicked".to_string())??;
                    results.push(result);
                }
                Ok::<_, String>(results)
            })?;
            completed.extend(
                chunk_results
                    .drain(..)
                    .map(|(index, unit_name, mir_report)| PartitionedMirUnitReport {
                        index,
                        unit_name,
                        mir_report,
                    }),
            );
        }

        completed.sort_by_key(|report| report.index);
        Ok(MirUnitBatch {
            reports: completed,
            wall_duration: started.elapsed(),
        })
    }

    fn codegen_unit_artifacts(
        &self,
        mast_module: &kernc_mast::MastModule,
        codegen_unit_plans: &[codegen_units::CodegenUnitPlan],
        build_context: &CodegenUnitBuildContext<'_>,
    ) -> Result<CodegenUnitBatch, String> {
        type PendingCodegenUnit = (usize, String, String, String, kernc_mast::MastModule);
        type CompletedCodegenUnit = (usize, String, String, CodegenReport, EmitObjectReport);

        let started = Instant::now();
        let worker_count = Self::codegen_worker_count(codegen_unit_plans.len());
        if worker_count <= 1 {
            return self.codegen_unit_artifacts_serial(
                mast_module,
                codegen_unit_plans,
                build_context,
            );
        }

        let asm_dialect = match self.options.asm_dialect {
            AsmDialect::Intel => InlineAsmDialect::Intel,
            AsmDialect::Att => InlineAsmDialect::ATT,
        };
        let mut pending: Vec<PendingCodegenUnit> = codegen_unit_plans
            .iter()
            .enumerate()
            .map(|(index, unit)| {
                let object_path = if self.options.driver_mode.emits_linker_input()
                    && self.options.emit_multi_linker_input_dir
                {
                    self.make_multi_linker_input_codegen_unit_path(&unit.name)
                } else {
                    self.make_temp_codegen_unit_path(&unit.name)
                };
                (
                    index,
                    unit.name.clone(),
                    format!("{}_{}", build_context.module_name, unit.name),
                    object_path,
                    materialize_codegen_unit(mast_module, unit),
                )
            })
            .collect::<Vec<_>>();
        let mut completed = Vec::with_capacity(pending.len());

        while !pending.is_empty() {
            let take = worker_count.min(pending.len());
            let chunk: Vec<PendingCodegenUnit> = pending.drain(..take).collect();
            let mut chunk_results: Vec<CompletedCodegenUnit> = std::thread::scope(|scope| {
                let mut handles = Vec::<
                    std::thread::ScopedJoinHandle<'_, Result<CompletedCodegenUnit, String>>,
                >::with_capacity(chunk.len());
                for (index, unit_name, llvm_module_name, object_path, unit_module) in chunk {
                    let mut worker_session = build_context.session.clone();
                    let worker_registry = build_context.type_registry.clone();
                    let target_triple = build_context.target_triple.to_string();
                    let split_sections_for_gc = self.options.split_sections_for_gc;
                    let opt_level = self.options.opt_level;
                    handles.push(scope.spawn(move || {
                        let codegen_ctx = Context::create();
                        let mut codegen = CodeGenerator::new(
                            &codegen_ctx,
                            &llvm_module_name,
                            &mut worker_session,
                            &worker_registry,
                            split_sections_for_gc,
                        );
                        codegen.set_asm_dialect(asm_dialect);
                        let mir_report = kernc_mir_lower::build_from_mast(&unit_module);
                        let codegen_report = codegen
                            .compile_mir(&mir_report.module, build_context.collect_diagnostics);
                        let emit_report = codegen.emit_to_file(
                            &target_triple,
                            &object_path,
                            opt_level,
                            build_context.collect_diagnostics,
                        )?;
                        Ok::<_, String>((
                            index,
                            unit_name,
                            object_path,
                            codegen_report,
                            emit_report,
                        ))
                    }));
                }

                let mut results: Vec<CompletedCodegenUnit> = Vec::with_capacity(handles.len());
                for handle in handles {
                    let result = handle
                        .join()
                        .map_err(|_| "parallel CGU worker panicked".to_string())??;
                    results.push(result);
                }
                Ok::<_, String>(results)
            })?;
            completed.extend(chunk_results.drain(..).map(
                |(index, _unit_name, object_path, codegen_report, emit_report)| {
                    CodegenUnitArtifacts {
                        index,
                        object_path,
                        codegen_report,
                        emit_report,
                    }
                },
            ));
        }

        completed.sort_by_key(|artifact| artifact.index);
        Ok(CodegenUnitBatch {
            artifacts: completed,
            wall_duration: started.elapsed(),
        })
    }

    fn codegen_unit_artifacts_serial(
        &self,
        mast_module: &kernc_mast::MastModule,
        codegen_unit_plans: &[codegen_units::CodegenUnitPlan],
        build_context: &CodegenUnitBuildContext<'_>,
    ) -> Result<CodegenUnitBatch, String> {
        let started = Instant::now();
        let mut artifacts = Vec::with_capacity(codegen_unit_plans.len());
        for (index, unit) in codegen_unit_plans.iter().enumerate() {
            let unit_module = materialize_codegen_unit(mast_module, unit);
            let mut worker_session = build_context.session.clone();
            let codegen_ctx = Context::create();
            let mut codegen = CodeGenerator::new(
                &codegen_ctx,
                &format!("{}_{}", build_context.module_name, unit.name),
                &mut worker_session,
                build_context.type_registry,
                self.options.split_sections_for_gc,
            );
            codegen.set_asm_dialect(match self.options.asm_dialect {
                AsmDialect::Intel => InlineAsmDialect::Intel,
                AsmDialect::Att => InlineAsmDialect::ATT,
            });
            let mir_report = kernc_mir_lower::build_from_mast(&unit_module);
            let codegen_report =
                codegen.compile_mir(&mir_report.module, build_context.collect_diagnostics);
            let object_path = if self.options.driver_mode.emits_linker_input()
                && self.options.emit_multi_linker_input_dir
            {
                self.make_multi_linker_input_codegen_unit_path(&unit.name)
            } else {
                self.make_temp_codegen_unit_path(&unit.name)
            };
            let emit_report = codegen.emit_to_file(
                build_context.target_triple,
                &object_path,
                self.options.opt_level,
                build_context.collect_diagnostics,
            )?;
            artifacts.push(CodegenUnitArtifacts {
                index,
                object_path,
                codegen_report,
                emit_report,
            });
        }
        Ok(CodegenUnitBatch {
            artifacts,
            wall_duration: started.elapsed(),
        })
    }

    fn codegen_worker_count(unit_count: usize) -> usize {
        if unit_count <= 1 {
            return 1;
        }
        std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1)
            .min(unit_count)
    }

    fn emit_thin_lto_bitcode_linker_input(&self) -> bool {
        self.options.driver_mode.emits_linker_input()
            && matches!(
                self.options.linker_input_flavor,
                LinkerInputFlavor::ThinLtoBitcode
            )
    }

    fn prepare_multi_linker_input_output_dir(&self) -> bool {
        let dir = self.make_multi_linker_input_dir_path();
        Self::prepare_clean_output_dir(Path::new(&dir), "linker-input directory")
    }

    fn write_multi_linker_input_manifest(&self, object_paths: &[String]) -> std::io::Result<()> {
        let mut contents = String::from("version=1\n");
        for object_path in object_paths {
            contents.push_str("linker_input=");
            contents.push_str(object_path);
            contents.push('\n');
        }
        fs::write(&self.options.output_file, contents)
    }

    fn make_temp_thin_lto_output_dir_path(&self) -> String {
        format!("{}.tmp.thinlto.d", self.options.output_file)
    }

    pub(in crate::compiler) fn make_thin_lto_cache_dir_path(&self) -> String {
        format!("{}.thinlto-cache.d", self.options.output_file)
    }

    fn ensure_output_dir(path: &Path, label: &str) -> bool {
        if path.is_file() && fs::remove_file(path).is_err() {
            eprintln!(
                "Error: Failed to remove stale {} `{}`.",
                label,
                path.display()
            );
            return false;
        }
        if let Err(err) = fs::create_dir_all(path) {
            eprintln!(
                "Error: Failed to create {} `{}`: {}",
                label,
                path.display(),
                err
            );
            return false;
        }
        true
    }

    fn prepare_clean_output_dir(path: &Path, label: &str) -> bool {
        if path.is_file() && fs::remove_file(path).is_err() {
            eprintln!(
                "Error: Failed to remove stale {} `{}`.",
                label,
                path.display()
            );
            return false;
        }
        if path.is_dir() && fs::remove_dir_all(path).is_err() {
            eprintln!(
                "Error: Failed to remove stale {} `{}`.",
                label,
                path.display()
            );
            return false;
        }
        Self::ensure_output_dir(path, label)
    }
}

struct PartitionedMirUnitReport {
    index: usize,
    unit_name: String,
    mir_report: kernc_mir_lower::MirBuildReport,
}

struct MirUnitBatch {
    reports: Vec<PartitionedMirUnitReport>,
    wall_duration: Duration,
}
