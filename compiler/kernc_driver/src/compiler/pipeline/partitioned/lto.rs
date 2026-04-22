use super::*;
use crate::compiler::codegen_units;
use crate::compiler::{TempDirGuard, TempFileGuard};
use kernc_utils::config::OptLevel;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

impl CompilerDriver {
    pub(super) fn compile_partitioned_units_thin_lto(
        &self,
        pipeline: &mut CompilePipelineContext<'_, '_>,
        mast_module: &kernc_mast::MastModule,
        codegen_unit_plans: &[codegen_units::CodegenUnitPlan],
    ) -> Option<CompileReport> {
        type CompletedThinLtoUnit = (usize, String, Vec<u8>, CodegenReport, EmitObjectReport);

        let started = Instant::now();
        // ThinLTO still starts from fully partitioned MIR units. The "thin" part happens after
        // each unit has been lowered and emitted as its own bitcode module.
        let mir_unit_batch =
            match self.partitioned_mir_unit_reports(mast_module, codegen_unit_plans) {
                Ok(batch) => batch,
                Err(err) => {
                    eprintln!("Error: failed to build partitioned MIR units for ThinLTO: {err}");
                    return None;
                }
            };
        let emit_multi_linker_input_dir = self.options.driver_mode.emits_linker_input()
            && self.options.emit_multi_linker_input_dir;
        if self.options.driver_mode.emits_linker_input()
            && emit_multi_linker_input_dir
            && !self.emit_thin_lto_bitcode_linker_input()
        {
            return self.compile_partitioned_units_preserve_native_linker_inputs(
                pipeline,
                mast_module,
                codegen_unit_plans,
                started,
                mir_unit_batch.wall_duration,
            );
        }

        let asm_dialect = self.codegen_asm_dialect();
        let worker_count = Self::codegen_worker_count(mir_unit_batch.reports.len());
        let mut pending = mir_unit_batch.reports.iter().collect::<Vec<_>>();
        let mut completed = Vec::<CompletedThinLtoUnit>::with_capacity(pending.len());
        while !pending.is_empty() {
            let take = worker_count.min(pending.len());
            let chunk = pending.drain(..take).collect::<Vec<_>>();
            // Limit the number of simultaneously-live LLVM contexts to `worker_count` so large
            // builds can still parallelize codegen without blowing up memory usage.
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
                        codegen.set_debug_info(self.options.debug_info, opt_level != OptLevel::O0);
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
        if self.emit_thin_lto_bitcode_linker_input() {
            // In linker-input mode we sometimes preserve the ThinLTO-ready bitcode itself so a
            // downstream linker/plugin can drive the final ThinLTO step later.
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

        // After LLVM materializes post-LTO object files, the remaining pipeline is the same as
        // ordinary multi-object linking: optionally preserve them or feed them to the linker.
        let module_indices = thin_modules
            .iter()
            .enumerate()
            .map(|(index, module)| (module.identifier.as_str(), index))
            .collect::<std::collections::HashMap<_, _>>();
        let mut object_paths = object_outputs
            .into_iter()
            .enumerate()
            .map(|(index, object)| match object {
                kernc_codegen::ThinLtoObject {
                    identifier,
                    kind: kernc_codegen::ThinLtoObjectKind::File(path),
                } => {
                    let Some(&original_index) = module_indices.get(identifier.as_str()) else {
                        return Err(format!(
                            "ThinLTO returned object output for unknown module `{identifier}`"
                        ));
                    };
                    Ok((original_index, path))
                }
                kernc_codegen::ThinLtoObject {
                    kind: kernc_codegen::ThinLtoObjectKind::Buffer(_),
                    ..
                } => Err(format!(
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

    fn compile_partitioned_units_preserve_native_linker_inputs(
        &self,
        pipeline: &mut CompilePipelineContext<'_, '_>,
        mast_module: &kernc_mast::MastModule,
        codegen_unit_plans: &[codegen_units::CodegenUnitPlan],
        codegen_units_started: Instant,
        mir_units_duration: Duration,
    ) -> Option<CompileReport> {
        if !self.prepare_multi_linker_input_output_dir() {
            return None;
        }

        // This fallback is for "preserve linker inputs" mode when the user asked for native
        // objects on disk instead of ThinLTO bitcode. We keep the partitioning but skip the
        // ThinLTO post-link object materialization step entirely.
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
                    eprintln!("Error: LLVM failed to generate intermediate file: {err}");
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
            duration: codegen_units_started.elapsed(),
        });
        pipeline.phase_timings.push(PhaseTiming {
            name: "  mir_units",
            duration: mir_units_duration,
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

        let merged_output_path = if pipeline.target.is_windows {
            PathBuf::from(self.make_multi_linker_input_dir_path())
                .join("merged.lib")
                .to_string_lossy()
                .to_string()
        } else {
            self.make_multi_linker_input_codegen_unit_path("merged")
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
        if !merged {
            return None;
        }

        // The merged relocatable object is the artifact recorded in the manifest. The per-unit
        // preserved objects are just staging inputs and can be cleaned up immediately after.
        for object_path in &object_paths {
            if object_path == &merged_output_path {
                continue;
            }
            if let Err(err) = fs::remove_file(object_path) {
                eprintln!(
                    "Error: Failed to discard temporary preserved linker input `{}`: {}",
                    object_path, err
                );
                return None;
            }
        }

        if let Err(err) =
            self.write_multi_linker_input_manifest(std::slice::from_ref(&merged_output_path))
        {
            eprintln!(
                "Error: Failed to record preserved linker inputs `{}`: {}",
                self.options.output_file, err
            );
            return None;
        }

        Self::print_buffered_diagnostics(pipeline.sema.sess);
        Some(Self::build_compile_report(
            pipeline.report,
            std::mem::take(pipeline.phase_timings),
            codegen_report,
            Some(emit_report),
        ))
    }

    pub(super) fn compile_partitioned_units_full_lto(
        &self,
        pipeline: &mut CompilePipelineContext<'_, '_>,
        mast_module: &kernc_mast::MastModule,
        codegen_unit_plans: &[codegen_units::CodegenUnitPlan],
    ) -> Option<CompileReport> {
        type CompletedFullLtoUnit = (usize, String, Vec<u8>, CodegenReport);

        let started = Instant::now();
        // Full LTO shares the same front half as ThinLTO: emit each partition independently,
        // then merge everything back into one LLVM module for whole-program optimization.
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
            // Workers emit bitcode instead of objects because the driver will deserialize these
            // units into one shared LLVM context before the final emission step.
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
                        codegen.set_debug_info(
                            self.options.debug_info,
                            self.options.opt_level != OptLevel::O0,
                        );
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
        merged_codegen.set_debug_info(
            self.options.debug_info,
            self.options.opt_level != OptLevel::O0,
        );
        for (_, unit_name, bitcode, unit_report) in completed {
            Self::absorb_codegen_report(&mut codegen_report, unit_report);
            // Parse each unit into the merged context first; only then can LLVM run cross-unit
            // inlining, dead stripping, and internalization as if this were a single module.
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
}
