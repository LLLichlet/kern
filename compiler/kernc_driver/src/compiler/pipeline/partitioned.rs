//! Partitioned codegen pipeline.
//!
//! Partitioned compilation plans multiple codegen units, materializes imports
//! between units, emits each unit independently, and optionally feeds the
//! resulting bitcode/object artifacts into ThinLTO.

use super::*;
use crate::compiler::TempFileGuard;
use crate::compiler::codegen_units;
use std::time::Duration;

mod lto;
mod units;

impl CompilerDriver {
    pub(super) fn compile_partitioned_units(
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
