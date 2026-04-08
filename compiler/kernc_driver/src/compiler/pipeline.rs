use super::codegen_units::{
    CodegenPlanFallback, CodegenPlanReport, materialize_codegen_unit,
    plan_codegen_units_with_report,
};
#[cfg(test)]
use super::flow::FlowModel;
use super::{
    CompileCacheStats, CompileReport, CompilerDriver, PhaseTiming, SourceOverrides,
    StructureArtifact, StructureCacheKey,
};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kernc_codegen::{
    AllocaNameStat, CodeGenerator, CodegenAllocaStats, CodegenReport, Context, EmitObjectReport,
    InlineAsmDialect, IrCleanupStats, IrFunctionStats, IrInstructionStats,
};
use kernc_db::Memo;
use kernc_lower::Lowerer;
use kernc_sema::SemaContext;
use kernc_sema::def::DefId;
use kernc_utils::Session;
use kernc_utils::config::{AsmDialect, CompileOptions, DriverMode};

use crate::frontend::FrontendDatabase;
use crate::metadata;

struct LoweredModuleReport {
    module: kernc_mast::MastModule,
    phase_timings: Vec<PhaseTiming>,
    cache_stats: kernc_lower::LowerCacheStats,
}

struct CodegenUnitArtifacts {
    index: usize,
    object_path: String,
    codegen_report: CodegenReport,
    emit_report: EmitObjectReport,
}

struct CodegenUnitBatch {
    artifacts: Vec<CodegenUnitArtifacts>,
    wall_duration: Duration,
}

impl CompilerDriver {
    pub fn new(options: CompileOptions) -> Self {
        Self {
            options,
            frontend: FrontendDatabase::new(),
            compile_structure_artifacts: Memo::new(),
            collected_artifacts: Memo::new(),
            imported_artifacts: Memo::new(),
            structure_artifacts: Memo::new(),
            cache_counters: std::sync::Arc::new(Default::default()),
        }
    }

    pub fn compile(&self) -> bool {
        match self.compile_with_report() {
            Some(report) => {
                if self.options.report_timings {
                    Self::print_phase_timings(&report.phase_timings);
                    Self::print_cache_stats(report.cache_stats);
                    Self::print_lower_cache_stats(report.lower_cache_stats);
                    Self::print_mast_workload(report.mast_workload.as_ref());
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
        let cache_snapshot = self.cache_counter_snapshot();
        let mut phase_timings = Vec::new();
        if self.options.driver_mode == DriverMode::LinkOnly {
            let linked = Self::measure_phase(&mut phase_timings, "link", || self.link_only());
            return linked.then(|| CompileReport {
                loaded_sources: Vec::new(),
                phase_timings,
                cache_stats: self.cache_stats_since(cache_snapshot),
                lower_cache_stats: None,
                mast_workload: None,
                codegen_plan: None,
                ir_instruction_stats: None,
                ir_cleanup_stats: None,
                remaining_alloca_stats: None,
                remaining_alloca_names: Vec::new(),
                ir_hot_functions: Vec::new(),
                codegen_alloca_stats: Default::default(),
            });
        }

        let Some(input_file) = self.options.input_file.as_deref() else {
            eprintln!("Error: compile mode requires a source input.");
            return None;
        };

        let structure = Self::measure_phase(&mut phase_timings, "analyze_structure", || {
            self.analyze_compile_structure(input_file, &SourceOverrides::new())
        })?;
        phase_timings.extend(structure.phase_timings.iter().copied());
        let mut session = structure.session.clone();

        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        let body_pipeline = self.run_body_pipeline_with_report(&mut ctx)?;
        phase_timings.extend(body_pipeline.phase_timings.iter().copied());
        let loaded_sources = ctx
            .sess
            .source_manager
            .files()
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();

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

        if let Some(metadata_output) = self.options.metadata_output.as_deref()
            && let Err(err) = Self::measure_phase(&mut phase_timings, "emit_kmeta", || {
                metadata::emit_package_metadata(
                    &ctx,
                    Path::new(metadata_output),
                    self.options
                        .metadata_package_name
                        .as_deref()
                        .or(self.options.root_module_name.as_deref())
                        .unwrap_or("root"),
                    self.options.metadata_package_version.as_deref(),
                )
            })
        {
            eprintln!("Error: Failed to emit kmeta snapshot: {}", err);
            return None;
        }

        let target = self.normalized_target();
        let module_name = self.module_name_for_codegen(input_file);
        let codegen_plan_started = Instant::now();
        let codegen_plan = if self.options.driver_mode == DriverMode::EmitLlvmIr {
            None
        } else {
            Some(plan_codegen_units_with_report(
                &mast_module,
                self.options.codegen_units,
            ))
        };
        phase_timings.push(PhaseTiming {
            name: "  codegen_plan",
            duration: codegen_plan_started.elapsed(),
        });
        let codegen_plan_report = codegen_plan.as_ref().map(|plan| plan.report.clone());
        let codegen_unit_plans = codegen_plan.map(|plan| plan.units).unwrap_or_default();

        let lower_cache_stats = lowered.cache_stats;
        let cache_stats = self.cache_stats_since(cache_snapshot);

        if codegen_unit_plans.is_empty() {
            let codegen_ctx = Context::create();
            let mut codegen = CodeGenerator::new(
                &codegen_ctx,
                &module_name,
                &mut *ctx.sess,
                &ctx.type_registry,
                self.options.split_sections_for_gc,
            );

            codegen.set_asm_dialect(match self.options.asm_dialect {
                AsmDialect::Intel => InlineAsmDialect::Intel,
                AsmDialect::Att => InlineAsmDialect::ATT,
            });
            let codegen_report = Self::measure_phase(&mut phase_timings, "codegen", || {
                codegen.compile(&mast_module)
            });
            phase_timings.extend(codegen_report.timings.iter().map(|timing| PhaseTiming {
                name: timing.name,
                duration: timing.duration,
            }));

            if self.options.driver_mode == DriverMode::EmitLlvmIr {
                return match Self::measure_phase(&mut phase_timings, "emit_llvm_ir", || {
                    codegen.print_ir()
                }) {
                    Ok(()) => {
                        Self::print_buffered_diagnostics(ctx.sess);
                        Some(Self::build_compile_report(
                            loaded_sources,
                            phase_timings,
                            cache_stats,
                            lower_cache_stats,
                            mast_workload,
                            codegen_plan_report.clone(),
                            codegen_report,
                            None,
                        ))
                    }
                    Err(err) => {
                        eprintln!("Error: Failed to print LLVM IR: {}", err);
                        None
                    }
                };
            }

            let link_input_path = self.prepare_link_input_path(&target);
            let _guard = self.temp_link_input_guard(&link_input_path);

            let emit_report = match Self::measure_phase(&mut phase_timings, "emit_object", || {
                codegen.emit_to_file(&target.triple, &link_input_path, self.options.opt_level)
            }) {
                Ok(report) => report,
                Err(err) => {
                    eprintln!("Error: LLVM failed to generate intermediate file: {}", err);
                    return None;
                }
            };
            phase_timings.extend(emit_report.timings.iter().map(|timing| PhaseTiming {
                name: timing.name,
                duration: timing.duration,
            }));

            if self.options.driver_mode.emits_linker_input() {
                Self::print_buffered_diagnostics(ctx.sess);
                if self.options.report_progress {
                    println!(
                        "Successfully emitted linker input to `{}`",
                        self.options.output_file
                    );
                }
                return Some(Self::build_compile_report(
                    loaded_sources,
                    phase_timings,
                    cache_stats,
                    lower_cache_stats,
                    mast_workload,
                    codegen_plan_report.clone(),
                    codegen_report,
                    Some(emit_report),
                ));
            }

            let linked = Self::measure_phase(&mut phase_timings, "link", || {
                self.run_link_command(Some(&link_input_path), &target, "Successfully compiled")
            });
            if linked {
                Self::print_buffered_diagnostics(ctx.sess);
            }
            return linked.then_some(Self::build_compile_report(
                loaded_sources,
                phase_timings,
                cache_stats,
                lower_cache_stats,
                mast_workload,
                codegen_plan_report.clone(),
                codegen_report,
                Some(emit_report),
            ));
        }

        let object_guards = codegen_unit_plans
            .iter()
            .map(|unit| super::TempFileGuard {
                path: self.make_temp_codegen_unit_path(&unit.name),
            })
            .collect::<Vec<_>>();
        let unit_batch = match self.codegen_unit_artifacts(
            &mast_module,
            &codegen_unit_plans,
            &module_name,
            &target.triple,
            ctx.sess,
            &ctx.type_registry,
        ) {
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

        phase_timings.push(PhaseTiming {
            name: "codegen_units",
            duration: unit_batch.wall_duration,
        });
        phase_timings.extend(codegen_report.timings.iter().map(|timing| PhaseTiming {
            name: timing.name,
            duration: timing.duration,
        }));
        phase_timings.extend(emit_report.timings.iter().map(|timing| PhaseTiming {
            name: timing.name,
            duration: timing.duration,
        }));

        if self.options.driver_mode.emits_linker_input() {
            let merged_output_path = self.make_temp_relocatable_merge_path();
            let merged_output_guard = super::TempFileGuard {
                path: merged_output_path.clone(),
            };
            let merged = Self::measure_phase(&mut phase_timings, "merge_object", || {
                self.run_relocatable_link_command(
                    &object_paths,
                    &target,
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
            Self::print_buffered_diagnostics(ctx.sess);
            return Some(Self::build_compile_report(
                loaded_sources,
                phase_timings,
                cache_stats,
                lower_cache_stats,
                mast_workload,
                codegen_plan_report.clone(),
                codegen_report,
                Some(emit_report),
            ));
        }

        let linked = Self::measure_phase(&mut phase_timings, "link", || {
            self.run_link_command_with_inputs(&object_paths, &target, "Successfully compiled")
        });
        drop(object_guards);
        if linked {
            Self::print_buffered_diagnostics(ctx.sess);
        }
        linked.then_some(Self::build_compile_report(
            loaded_sources,
            phase_timings,
            cache_stats,
            lower_cache_stats,
            mast_workload,
            codegen_plan_report,
            codegen_report,
            Some(emit_report),
        ))
    }

    fn codegen_unit_artifacts(
        &self,
        mast_module: &kernc_mast::MastModule,
        codegen_unit_plans: &[super::codegen_units::CodegenUnitPlan],
        module_name: &str,
        target_triple: &str,
        session: &Session,
        type_registry: &kernc_sema::ty::TypeRegistry,
    ) -> Result<CodegenUnitBatch, String> {
        let started = Instant::now();
        let worker_count = Self::codegen_worker_count(codegen_unit_plans.len());
        if worker_count <= 1 {
            return self.codegen_unit_artifacts_serial(
                mast_module,
                codegen_unit_plans,
                module_name,
                target_triple,
                session,
                type_registry,
            );
        }

        let asm_dialect = match self.options.asm_dialect {
            AsmDialect::Intel => InlineAsmDialect::Intel,
            AsmDialect::Att => InlineAsmDialect::ATT,
        };
        let mut pending = codegen_unit_plans
            .iter()
            .enumerate()
            .map(|(index, unit)| {
                (
                    index,
                    unit.name.clone(),
                    format!("{}_{}", module_name, unit.name),
                    self.make_temp_codegen_unit_path(&unit.name),
                    materialize_codegen_unit(mast_module, unit),
                )
            })
            .collect::<Vec<_>>();
        let mut completed = Vec::with_capacity(pending.len());

        while !pending.is_empty() {
            let take = worker_count.min(pending.len());
            let chunk = pending.drain(..take).collect::<Vec<_>>();
            let mut chunk_results = std::thread::scope(|scope| {
                let mut handles = Vec::with_capacity(chunk.len());
                for (index, unit_name, llvm_module_name, object_path, unit_module) in chunk {
                    let mut worker_session = session.clone();
                    let worker_registry = type_registry.clone();
                    let target_triple = target_triple.to_string();
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
                        let codegen_report = codegen.compile(&unit_module);
                        let emit_report =
                            codegen.emit_to_file(&target_triple, &object_path, opt_level)?;
                        Ok::<_, String>((
                            index,
                            unit_name,
                            object_path,
                            codegen_report,
                            emit_report,
                        ))
                    }));
                }

                let mut results = Vec::with_capacity(handles.len());
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
        codegen_unit_plans: &[super::codegen_units::CodegenUnitPlan],
        module_name: &str,
        target_triple: &str,
        session: &Session,
        type_registry: &kernc_sema::ty::TypeRegistry,
    ) -> Result<CodegenUnitBatch, String> {
        let started = Instant::now();
        let mut artifacts = Vec::with_capacity(codegen_unit_plans.len());
        for (index, unit) in codegen_unit_plans.iter().enumerate() {
            let unit_module = materialize_codegen_unit(mast_module, unit);
            let mut worker_session = session.clone();
            let codegen_ctx = Context::create();
            let mut codegen = CodeGenerator::new(
                &codegen_ctx,
                &format!("{}_{}", module_name, unit.name),
                &mut worker_session,
                type_registry,
                self.options.split_sections_for_gc,
            );
            codegen.set_asm_dialect(match self.options.asm_dialect {
                AsmDialect::Intel => InlineAsmDialect::Intel,
                AsmDialect::Att => InlineAsmDialect::ATT,
            });
            let codegen_report = codegen.compile(&unit_module);
            let object_path = self.make_temp_codegen_unit_path(&unit.name);
            let emit_report =
                codegen.emit_to_file(target_triple, &object_path, self.options.opt_level)?;
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

    fn build_compile_report(
        loaded_sources: Vec<PathBuf>,
        phase_timings: Vec<PhaseTiming>,
        cache_stats: CompileCacheStats,
        lower_cache_stats: kernc_lower::LowerCacheStats,
        mast_workload: kernc_mast::MastWorkloadStats,
        codegen_plan: Option<CodegenPlanReport>,
        codegen_report: CodegenReport,
        emit_report: Option<EmitObjectReport>,
    ) -> CompileReport {
        let (ir_cleanup_stats, remaining_alloca_stats, remaining_alloca_names) =
            if let Some(emit_report) = emit_report {
                (
                    emit_report.ir_cleanup_stats,
                    emit_report.remaining_alloca_stats,
                    emit_report.remaining_alloca_names,
                )
            } else {
                (None, None, Vec::new())
            };

        CompileReport {
            loaded_sources,
            phase_timings,
            cache_stats,
            lower_cache_stats: Some(lower_cache_stats),
            mast_workload: Some(mast_workload),
            codegen_plan,
            ir_instruction_stats: Some(codegen_report.ir_stats),
            ir_cleanup_stats,
            remaining_alloca_stats,
            remaining_alloca_names,
            ir_hot_functions: codegen_report.ir_hot_functions,
            codegen_alloca_stats: codegen_report.alloca_stats,
        }
    }

    fn absorb_codegen_report(into: &mut CodegenReport, other: CodegenReport) {
        into.timings.extend(other.timings);
        into.ir_stats = Self::sum_ir_instruction_stats(into.ir_stats, other.ir_stats);
        into.alloca_stats = Self::sum_alloca_stats(into.alloca_stats, other.alloca_stats);
        into.ir_hot_functions.extend(other.ir_hot_functions);
        Self::truncate_hot_functions(&mut into.ir_hot_functions);
    }

    fn absorb_emit_report(into: &mut EmitObjectReport, other: EmitObjectReport) {
        into.timings.extend(other.timings);
        into.ir_cleanup_stats = match (into.ir_cleanup_stats, other.ir_cleanup_stats) {
            (Some(lhs), Some(rhs)) => Some(IrCleanupStats {
                before: Self::sum_ir_instruction_stats(lhs.before, rhs.before),
                after: Self::sum_ir_instruction_stats(lhs.after, rhs.after),
            }),
            (None, rhs) => rhs,
            (lhs, None) => lhs,
        };
        into.remaining_alloca_stats =
            match (into.remaining_alloca_stats, other.remaining_alloca_stats) {
                (Some(lhs), Some(rhs)) => Some(Self::sum_alloca_stats(lhs, rhs)),
                (None, rhs) => rhs,
                (lhs, None) => lhs,
            };
        Self::absorb_alloca_name_stats(
            &mut into.remaining_alloca_names,
            other.remaining_alloca_names,
        );
    }

    fn sum_ir_instruction_stats(
        lhs: IrInstructionStats,
        rhs: IrInstructionStats,
    ) -> IrInstructionStats {
        IrInstructionStats {
            functions: lhs.functions + rhs.functions,
            basic_blocks: lhs.basic_blocks + rhs.basic_blocks,
            instructions: lhs.instructions + rhs.instructions,
            allocas: lhs.allocas + rhs.allocas,
            loads: lhs.loads + rhs.loads,
            stores: lhs.stores + rhs.stores,
            geps: lhs.geps + rhs.geps,
            calls: lhs.calls + rhs.calls,
            phis: lhs.phis + rhs.phis,
            branches: lhs.branches + rhs.branches,
            switches: lhs.switches + rhs.switches,
            returns: lhs.returns + rhs.returns,
            compares: lhs.compares + rhs.compares,
        }
    }

    fn sum_alloca_stats(lhs: CodegenAllocaStats, rhs: CodegenAllocaStats) -> CodegenAllocaStats {
        CodegenAllocaStats {
            params: lhs.params + rhs.params,
            lets: lhs.lets + rhs.lets,
            addr_of_temps: lhs.addr_of_temps + rhs.addr_of_temps,
            materialized_lvalues: lhs.materialized_lvalues + rhs.materialized_lvalues,
            array_to_slice_temps: lhs.array_to_slice_temps + rhs.array_to_slice_temps,
            union_inits: lhs.union_inits + rhs.union_inits,
            data_union_inits: lhs.data_union_inits + rhs.data_union_inits,
            unnamed: lhs.unnamed + rhs.unnamed,
            other: lhs.other + rhs.other,
        }
    }

    fn absorb_alloca_name_stats(into: &mut Vec<AllocaNameStat>, other: Vec<AllocaNameStat>) {
        let mut counts = HashMap::<String, usize>::new();
        for stat in into.iter().chain(other.iter()) {
            *counts.entry(stat.name.clone()).or_default() += stat.count;
        }

        let mut merged = counts
            .into_iter()
            .map(|(name, count)| AllocaNameStat { name, count })
            .collect::<Vec<_>>();
        merged.sort_by(|lhs, rhs| {
            rhs.count
                .cmp(&lhs.count)
                .then_with(|| lhs.name.cmp(&rhs.name))
        });
        merged.truncate(8);
        *into = merged;
    }

    fn truncate_hot_functions(hot_functions: &mut Vec<IrFunctionStats>) {
        hot_functions.sort_by(|lhs, rhs| {
            rhs.instructions
                .cmp(&lhs.instructions)
                .then_with(|| rhs.loads.cmp(&lhs.loads))
                .then_with(|| rhs.stores.cmp(&lhs.stores))
                .then_with(|| lhs.name.cmp(&rhs.name))
        });
        hot_functions.truncate(8);
    }

    pub fn analyze<'a>(
        &self,
        session: &'a mut Session,
        input_file: &str,
    ) -> Option<SemaContext<'a>> {
        self.analyze_with_overrides(session, input_file, &SourceOverrides::new())
    }

    pub fn analyze_with_overrides<'a>(
        &self,
        session: &'a mut Session,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<SemaContext<'a>> {
        let structure = self.analyze_structure(input_file, source_overrides)?;
        *session = structure.session.clone();

        let mut ctx = self.build_sema_context(session);
        ctx.restore_structure(structure.snapshot.clone());
        if !self.run_body_pipeline(&mut ctx) {
            return None;
        }

        Some(ctx)
    }

    pub fn analyze_structure(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<StructureArtifact> {
        let mut session = Session::new();
        session.apply_options(&self.options);
        self.try_analyze_structure(session, input_file, source_overrides)
            .ok()
    }

    #[cfg(test)]
    pub(super) fn lower_module<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
    ) -> Option<kernc_mast::MastModule> {
        let references = ctx.identifier_references().to_vec();
        let module_item_definition_spans = self.module_item_definition_spans(ctx);
        let flow_model = FlowModel::collect(ctx, &module_item_definition_spans, &references);
        let flow_lowering_hints = flow_model.lowering_hints(ctx);
        let reachable_items = self
            .compute_module_item_reachability(ctx, &references, &flow_model)
            .lowered_reachable;
        self.lower_module_with_flow(ctx, &flow_lowering_hints, &reachable_items)
    }

    #[cfg(test)]
    pub(super) fn lower_module_with_flow<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        flow_lowering_hints: &kernc_lower::FlowLoweringHints,
        reachable_items: &std::collections::HashSet<DefId>,
    ) -> Option<kernc_mast::MastModule> {
        self.lower_module_with_flow_report(ctx, flow_lowering_hints, reachable_items)
            .map(|report| report.module)
    }

    fn lower_module_with_flow_report<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        flow_lowering_hints: &kernc_lower::FlowLoweringHints,
        reachable_items: &std::collections::HashSet<DefId>,
    ) -> Option<LoweredModuleReport> {
        let mut lowerer = Lowerer::new(ctx);
        lowerer.set_reachable_module_items(reachable_items.clone());
        lowerer.set_flow_lowering_hints(flow_lowering_hints.clone());
        let report = lowerer.lower_all_with_report();
        if !Self::report_diagnostics_if_errors(lowerer.context()) {
            return None;
        }
        Some(LoweredModuleReport {
            module: report.module,
            phase_timings: report
                .phase_timings
                .into_iter()
                .map(|timing| PhaseTiming {
                    name: timing.name,
                    duration: timing.duration,
                })
                .collect(),
            cache_stats: report.cache_stats,
        })
    }

    pub(super) fn module_name_for_codegen(&self, input_file: &str) -> String {
        Path::new(input_file)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("kern_module")
            .to_string()
    }

    pub(super) fn report_diagnostics_if_errors(ctx: &mut SemaContext<'_>) -> bool {
        if ctx.has_errors() {
            ctx.sess.print_diagnostics();
            return false;
        }
        true
    }

    pub(super) fn print_buffered_diagnostics(session: &Session) {
        if !session.diagnostics.is_empty() {
            session.print_diagnostics();
        }
    }

    pub(super) fn measure_phase<T, F>(
        phase_timings: &mut Vec<PhaseTiming>,
        name: &'static str,
        f: F,
    ) -> T
    where
        F: FnOnce() -> T,
    {
        let started = Instant::now();
        let value = f();
        phase_timings.push(PhaseTiming {
            name,
            duration: started.elapsed(),
        });
        value
    }

    pub(super) fn print_phase_timings(phase_timings: &[PhaseTiming]) {
        if phase_timings.is_empty() {
            return;
        }

        println!("Phase timings:");
        for phase in phase_timings {
            println!(
                "  {:<18} {}",
                phase.name,
                Self::format_duration(phase.duration)
            );
        }
        let total = phase_timings
            .iter()
            .filter(|phase| !phase.name.starts_with(' '))
            .map(|phase| phase.duration)
            .sum::<Duration>();
        println!("  {:<18} {}", "total", Self::format_duration(total));
    }

    pub(super) fn print_cache_stats(cache_stats: CompileCacheStats) {
        if cache_stats.is_empty() {
            return;
        }

        println!("Cache stats:");
        for (name, value) in [
            ("  compile_hit", cache_stats.compile_structure_hits),
            ("  compile_miss", cache_stats.compile_structure_misses),
            ("  structure_hit", cache_stats.structure_hits),
            ("  structure_miss", cache_stats.structure_misses),
            ("  imported_hit", cache_stats.imported_hits),
            ("  imported_miss", cache_stats.imported_misses),
            ("  collected_hit", cache_stats.collected_hits),
            ("  collected_miss", cache_stats.collected_misses),
            ("  frontend_parse", cache_stats.fresh_frontend_parses),
        ] {
            println!("  {:<18} {}", name, value);
        }
    }

    pub(super) fn print_lower_cache_stats(lower_cache_stats: Option<kernc_lower::LowerCacheStats>) {
        let Some(lower_cache_stats) = lower_cache_stats else {
            return;
        };
        if lower_cache_stats.is_empty() {
            return;
        }

        println!("Lowering cache stats:");
        for (name, value) in [
            ("  mono_fn_hit", lower_cache_stats.mono_function_hits),
            ("  mono_fn_miss", lower_cache_stats.mono_function_misses),
            ("  mono_struct_hit", lower_cache_stats.mono_struct_hits),
            ("  mono_struct_miss", lower_cache_stats.mono_struct_misses),
            ("  mono_data_hit", lower_cache_stats.mono_data_hits),
            ("  mono_data_miss", lower_cache_stats.mono_data_misses),
        ] {
            println!("  {:<18} {}", name, value);
        }
    }

    pub(super) fn print_mast_workload(mast_workload: Option<&kernc_mast::MastWorkloadStats>) {
        let Some(stats) = mast_workload else {
            return;
        };

        println!("MAST workload:");
        for (name, value) in [
            ("  structs", stats.structs),
            ("  globals", stats.globals),
            ("  globals_with_init", stats.globals_with_init),
            ("  functions", stats.functions),
            ("  function_bodies", stats.function_bodies),
            ("  extern_functions", stats.extern_functions),
            ("  blocks", stats.blocks),
            ("  statements", stats.statements),
            ("  let_statements", stats.let_statements),
            ("  expr_statements", stats.expr_statements),
            ("  defers", stats.defers),
            ("  expressions", stats.expressions),
            ("  calls", stats.calls),
            ("  branches", stats.branches),
            ("  loops", stats.loops),
            ("  switches", stats.switches),
            ("  returns", stats.returns),
            ("  assignments", stats.assignments),
        ] {
            println!("  {:<18} {}", name, value);
        }
    }

    pub(super) fn print_codegen_plan(codegen_plan: Option<&CodegenPlanReport>) {
        let Some(report) = codegen_plan else {
            return;
        };

        println!("Codegen plan:");
        println!("  {:<18} {}", "  requested_units", report.requested_units);
        println!("  {:<18} {}", "  roots", report.root_count);
        println!("  {:<18} {}", "  clusters", report.cluster_count);
        println!("  {:<18} {}", "  planned_units", report.planned_units);
        let fallback = match &report.fallback_reason {
            Some(CodegenPlanFallback::RequestedSingleUnit) => "requested_single_unit".to_string(),
            Some(CodegenPlanFallback::NameCollision { item_kind, name }) => {
                format!("name_collision({item_kind}:{name})")
            }
            Some(CodegenPlanFallback::TooFewRoots) => "too_few_roots".to_string(),
            Some(CodegenPlanFallback::TooFewTargetUnits) => "too_few_target_units".to_string(),
            Some(CodegenPlanFallback::TooFewMaterializedUnits) => {
                "too_few_materialized_units".to_string()
            }
            None => "planned".to_string(),
        };
        println!("  {:<18} {}", "  fallback", fallback);
    }

    pub(super) fn print_ir_instruction_stats(
        ir_instruction_stats: Option<&kernc_codegen::IrInstructionStats>,
    ) {
        let Some(stats) = ir_instruction_stats else {
            return;
        };

        println!("IR instruction stats:");
        for (name, value) in [
            ("  functions", stats.functions),
            ("  basic_blocks", stats.basic_blocks),
            ("  instructions", stats.instructions),
            ("  allocas", stats.allocas),
            ("  loads", stats.loads),
            ("  stores", stats.stores),
            ("  geps", stats.geps),
            ("  calls", stats.calls),
            ("  phis", stats.phis),
            ("  branches", stats.branches),
            ("  switches", stats.switches),
            ("  returns", stats.returns),
            ("  compares", stats.compares),
        ] {
            println!("  {:<18} {}", name, value);
        }
    }

    pub(super) fn print_ir_cleanup_stats(ir_cleanup_stats: Option<&kernc_codegen::IrCleanupStats>) {
        let Some(stats) = ir_cleanup_stats else {
            return;
        };

        if stats.before == stats.after {
            return;
        }

        println!("IR cleanup stats:");
        for (name, before, after) in [
            (
                "  instructions",
                stats.before.instructions,
                stats.after.instructions,
            ),
            ("  allocas", stats.before.allocas, stats.after.allocas),
            ("  loads", stats.before.loads, stats.after.loads),
            ("  stores", stats.before.stores, stats.after.stores),
            ("  geps", stats.before.geps, stats.after.geps),
            ("  phis", stats.before.phis, stats.after.phis),
            ("  branches", stats.before.branches, stats.after.branches),
            ("  returns", stats.before.returns, stats.after.returns),
            ("  compares", stats.before.compares, stats.after.compares),
        ] {
            let delta = after as i64 - before as i64;
            println!("  {:<18} {} -> {} ({:+})", name, before, after, delta);
        }
    }

    pub(super) fn print_ir_hot_functions(ir_hot_functions: &[kernc_codegen::IrFunctionStats]) {
        if ir_hot_functions.is_empty() {
            return;
        }

        println!("IR hot functions:");
        for function in ir_hot_functions {
            let name = if function.name.is_empty() {
                "<anonymous>"
            } else {
                function.name.as_str()
            };
            println!(
                "  {}: inst={} bb={} alloca={} load={} store={} gep={} call={} phi={} br={} ret={} cmp={}",
                name,
                function.instructions,
                function.basic_blocks,
                function.allocas,
                function.loads,
                function.stores,
                function.geps,
                function.calls,
                function.phis,
                function.branches,
                function.returns,
                function.compares,
            );
        }
    }

    pub(super) fn print_codegen_alloca_stats(
        codegen_alloca_stats: kernc_codegen::CodegenAllocaStats,
    ) {
        if codegen_alloca_stats == kernc_codegen::CodegenAllocaStats::default() {
            return;
        }

        println!("Codegen alloca stats:");
        for (name, value) in [
            ("  params", codegen_alloca_stats.params),
            ("  lets", codegen_alloca_stats.lets),
            ("  addr_of_temps", codegen_alloca_stats.addr_of_temps),
            (
                "  materialized_lvalues",
                codegen_alloca_stats.materialized_lvalues,
            ),
            (
                "  array_to_slice_temps",
                codegen_alloca_stats.array_to_slice_temps,
            ),
            ("  union_inits", codegen_alloca_stats.union_inits),
            ("  data_union_inits", codegen_alloca_stats.data_union_inits),
            ("  unnamed", codegen_alloca_stats.unnamed),
            ("  other", codegen_alloca_stats.other),
        ] {
            println!("  {:<22} {}", name, value);
        }
    }

    pub(super) fn print_remaining_alloca_stats(
        remaining_alloca_stats: Option<kernc_codegen::CodegenAllocaStats>,
    ) {
        let Some(stats) = remaining_alloca_stats else {
            return;
        };
        if stats == kernc_codegen::CodegenAllocaStats::default() {
            return;
        }

        println!("Remaining alloca stats:");
        for (name, value) in [
            ("  params", stats.params),
            ("  lets", stats.lets),
            ("  addr_of_temps", stats.addr_of_temps),
            ("  materialized_lvalues", stats.materialized_lvalues),
            ("  array_to_slice_temps", stats.array_to_slice_temps),
            ("  union_inits", stats.union_inits),
            ("  data_union_inits", stats.data_union_inits),
            ("  unnamed", stats.unnamed),
            ("  other", stats.other),
        ] {
            println!("  {:<22} {}", name, value);
        }
    }

    pub(super) fn print_remaining_alloca_names(
        remaining_alloca_names: &[kernc_codegen::AllocaNameStat],
    ) {
        if remaining_alloca_names.is_empty() {
            return;
        }

        println!("Remaining alloca names:");
        for stat in remaining_alloca_names {
            let name = if stat.name.is_empty() {
                "<unnamed>"
            } else {
                stat.name.as_str()
            };
            println!("  {:<22} {}", name, stat.count);
        }
    }

    pub(super) fn format_duration(duration: Duration) -> String {
        if duration.as_secs() >= 1 {
            format!("{:.3}s", duration.as_secs_f64())
        } else if duration.as_millis() >= 1 {
            format!("{:.3}ms", duration.as_secs_f64() * 1_000.0)
        } else if duration.as_micros() >= 1 {
            format!("{:.3}us", duration.as_secs_f64() * 1_000_000.0)
        } else {
            format!("{}ns", duration.as_nanos())
        }
    }

    pub(super) fn sync_source_overrides(&self, source_overrides: &SourceOverrides) {
        self.frontend.sync_source_overrides(source_overrides);
    }

    pub(super) fn structure_cache_key(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> StructureCacheKey {
        let mut overrides = source_overrides
            .iter()
            .map(|(path, text)| (normalize_cache_path(path), hash_text(text)))
            .collect::<Vec<_>>();
        overrides.sort();

        StructureCacheKey {
            input_file: normalize_cache_path(Path::new(input_file)),
            overrides,
        }
    }

    #[cfg(test)]
    pub(crate) fn uncached_parse_count(&self) -> usize {
        self.frontend.uncached_parse_count()
    }
}

fn hash_text(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn normalize_cache_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
