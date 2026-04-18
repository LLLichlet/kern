use super::*;
use std::collections::HashMap;

impl CompilerDriver {
    pub(super) fn build_compile_report(
        context: CompileReportContext<'_>,
        phase_timings: Vec<PhaseTiming>,
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
            loaded_sources: context.loaded_sources.to_vec(),
            phase_timings,
            cache_stats: context.cache_stats,
            lower_cache_stats: Some(context.lower_cache_stats),
            mast_workload: Some(context.mast_workload),
            mir_workload: Some(context.mir_workload),
            codegen_plan: context.codegen_plan.clone(),
            ir_instruction_stats: context
                .collect_codegen_diagnostics
                .then_some(codegen_report.ir_stats),
            ir_cleanup_stats: context
                .collect_codegen_diagnostics
                .then_some(ir_cleanup_stats)
                .flatten(),
            remaining_alloca_stats: context
                .collect_codegen_diagnostics
                .then_some(remaining_alloca_stats)
                .flatten(),
            remaining_alloca_names: if context.collect_codegen_diagnostics {
                remaining_alloca_names
            } else {
                Default::default()
            },
            ir_hot_functions: if context.collect_codegen_diagnostics {
                codegen_report.ir_hot_functions
            } else {
                Default::default()
            },
            codegen_alloca_stats: if context.collect_codegen_diagnostics {
                codegen_report.alloca_stats
            } else {
                Default::default()
            },
        }
    }

    pub(super) fn absorb_codegen_report(into: &mut CodegenReport, other: CodegenReport) {
        into.timings.extend(other.timings);
        into.ir_stats = Self::sum_ir_instruction_stats(into.ir_stats, other.ir_stats);
        into.alloca_stats = Self::sum_alloca_stats(into.alloca_stats, other.alloca_stats);
        into.ir_hot_functions.extend(other.ir_hot_functions);
        Self::truncate_hot_functions(&mut into.ir_hot_functions);
    }

    pub(super) fn absorb_emit_report(into: &mut EmitObjectReport, other: EmitObjectReport) {
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

    pub(in crate::compiler) fn report_diagnostics_if_errors(ctx: &mut SemaContext<'_>) -> bool {
        !ctx.has_errors()
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

    pub(super) fn print_mir_workload(mir_workload: Option<&kernc_mir::MirWorkloadStats>) {
        let Some(stats) = mir_workload else {
            return;
        };

        println!("MIR workload:");
        for (name, value) in [
            ("  globals", stats.globals),
            ("  globals_with_init", stats.globals_with_init),
            ("  functions", stats.functions),
            ("  function_bodies", stats.function_bodies),
            ("  extern_functions", stats.extern_functions),
            ("  locals", stats.locals),
            ("  param_locals", stats.param_locals),
            ("  let_locals", stats.let_locals),
            ("  blocks", stats.blocks),
            ("  instructions", stats.instructions),
            ("  let_instructions", stats.let_instructions),
            ("  assign_instructions", stats.assign_instructions),
            ("  memory_instructions", stats.memory_instructions),
            ("  atomic_store_instrs", stats.atomic_store_instructions),
            ("  fence_instructions", stats.fence_instructions),
            ("  eval_instructions", stats.eval_instructions),
            ("  defer_instructions", stats.defer_instructions),
            ("  use_rvalues", stats.use_rvalues),
            ("  call_rvalues", stats.call_rvalues),
            ("  aggregate_rvalues", stats.aggregate_rvalues),
            ("  projection_rvalues", stats.projection_rvalues),
            ("  unary_rvalues", stats.unary_rvalues),
            ("  binary_rvalues", stats.binary_rvalues),
            ("  cast_rvalues", stats.cast_rvalues),
            ("  bit_intrinsic_rvals", stats.bit_intrinsic_rvalues),
            ("  atomic_load_rvals", stats.atomic_load_rvalues),
            ("  atomic_cas_rvalues", stats.atomic_cas_rvalues),
            ("  atomic_rmw_rvalues", stats.atomic_rmw_rvalues),
            ("  address_of_rvalues", stats.address_of_rvalues),
            ("  load_rvalues", stats.load_rvalues),
            ("  direct_calls", stats.direct_calls),
            ("  indirect_calls", stats.indirect_calls),
            ("  gotos", stats.gotos),
            ("  branches", stats.branches),
            ("  switches", stats.switches),
            ("  returns", stats.returns),
            ("  unreachable_terms", stats.unreachable_terminators),
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
        println!("  {:<18} {}", "  total_workload", report.total_workload);
        println!(
            "  {:<18} {}",
            "  min_cluster_workload", report.min_cluster_workload
        );
        println!(
            "  {:<18} {}",
            "  max_cluster_workload", report.max_cluster_workload
        );
        println!(
            "  {:<18} {}",
            "  min_unit_workload", report.min_unit_workload
        );
        println!(
            "  {:<18} {}",
            "  max_unit_workload", report.max_unit_workload
        );
        println!(
            "  {:<18} {}",
            "  promoted_functions", report.promoted_function_count
        );
        println!(
            "  {:<18} {}",
            "  promoted_globals", report.promoted_global_count
        );
        println!(
            "  {:<18} {}",
            "  imported_functions", report.imported_function_count
        );
        if let Some(import_plan) = &report.import_plan {
            println!(
                "  {:<18} {}",
                "  import_candidates", import_plan.candidate_function_count
            );
            println!(
                "  {:<18} {}",
                "  import_accepted", import_plan.accepted_candidate_count
            );
            println!(
                "  {:<18} {}",
                "  import_rej_budget", import_plan.rejected_for_budget_count
            );
            println!("  {:<18} {}", "  import_budget", import_plan.total_budget);
            println!(
                "  {:<18} {}",
                "  min_import_budget", import_plan.min_unit_budget
            );
            println!(
                "  {:<18} {}",
                "  max_import_budget", import_plan.max_unit_budget
            );
            println!(
                "  {:<18} {}",
                "  import_score_total", import_plan.total_candidate_score
            );
            println!(
                "  {:<18} {}",
                "  import_score_used", import_plan.imported_score
            );
            println!(
                "  {:<18} {}",
                "  import_workload", import_plan.imported_workload
            );
        }
        let fallback = match &report.fallback_reason {
            Some(CodegenPlanFallback::RequestedSingleUnit) => "requested_single_unit".to_string(),
            Some(CodegenPlanFallback::ContainsControlFlowAsm { function_name }) => {
                format!("contains_control_flow_asm({function_name})")
            }
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
}
