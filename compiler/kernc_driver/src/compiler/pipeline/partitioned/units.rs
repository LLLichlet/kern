//! Partitioned codegen-unit emission.
//!
//! This module builds each planned codegen unit, picks object/bitcode output
//! paths, applies the effective inline-assembly dialect, and records per-unit
//! codegen/emit reports for the partitioned pipeline.

use super::*;
use crate::compiler::codegen_units;
use kernc_utils::config::OptLevel;
use std::fs;
use std::path::Path;
use std::time::Instant;

impl CompilerDriver {
    pub(super) fn partitioned_mir_unit_reports(
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
                    Ok(PartitionedMirUnitReport {
                        index,
                        unit_name: unit.name.clone(),
                        mir_report: kernc_mir_lower::try_build_from_mast(&unit_module)
                            .map_err(format_mir_lower_error)?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
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
                            kernc_mir_lower::try_build_from_mast(&unit_module)
                                .map_err(format_mir_lower_error)?,
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

    pub(super) fn codegen_unit_artifacts(
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

        let asm_dialect = match self
            .options
            .asm_dialect
            .effective_for_target(&self.options.target)
        {
            AsmDialect::Intel => InlineAsmDialect::Intel,
            AsmDialect::Att => InlineAsmDialect::ATT,
            // `effective_for_target` resolves Auto before this conversion.
            AsmDialect::Auto => unreachable!("effective_for_target must resolve `auto`"),
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
                    let code_model = self.options.code_model;
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
                        codegen.set_debug_info(self.options.debug_info, opt_level != OptLevel::O0);
                        let mir_report = kernc_mir_lower::build_from_mast(&unit_module);
                        let codegen_report = codegen
                            .compile_mir(&mir_report.module, build_context.collect_diagnostics);
                        let emit_report = codegen.emit_to_file(
                            &target_triple,
                            &object_path,
                            opt_level,
                            code_model,
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
            codegen.set_asm_dialect(
                match self
                    .options
                    .asm_dialect
                    .effective_for_target(&self.options.target)
                {
                    AsmDialect::Intel => InlineAsmDialect::Intel,
                    AsmDialect::Att => InlineAsmDialect::ATT,
                    AsmDialect::Auto => unreachable!("effective_for_target must resolve `auto`"),
                },
            );
            codegen.set_debug_info(
                self.options.debug_info,
                self.options.opt_level != OptLevel::O0,
            );
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
                self.options.code_model,
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

    pub(super) fn codegen_worker_count(unit_count: usize) -> usize {
        if unit_count <= 1 {
            return 1;
        }
        std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1)
            .min(unit_count)
    }

    pub(super) fn prepare_multi_linker_input_output_dir(&self) -> bool {
        let dir = self.make_multi_linker_input_dir_path();
        Self::prepare_clean_output_dir(Path::new(&dir), "linker-input directory")
    }

    pub(super) fn write_multi_linker_input_manifest(
        &self,
        object_paths: &[String],
    ) -> std::io::Result<()> {
        let mut contents = String::from("version=1\n");
        for object_path in object_paths {
            contents.push_str("linker_input=");
            contents.push_str(object_path);
            contents.push('\n');
        }
        fs::write(&self.options.output_file, contents)
    }

    pub(super) fn make_temp_thin_lto_output_dir_path(&self) -> String {
        format!("{}.tmp.thinlto.d", self.options.output_file)
    }

    pub(super) fn ensure_output_dir(path: &Path, label: &str) -> bool {
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

    pub(super) fn prepare_clean_output_dir(path: &Path, label: &str) -> bool {
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

fn format_mir_lower_error(error: kernc_mir_lower::MirLowerError) -> String {
    format!("{} at {:?}", error.message, error.span)
}
