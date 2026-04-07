#[cfg(test)]
use super::flow::FlowModel;
use super::{
    CompileCacheStats, CompileReport, CompilerDriver, PhaseTiming, SourceOverrides,
    StructureArtifact, StructureCacheKey,
};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kernc_codegen::{CodeGenerator, Context, InlineAsmDialect};
use kernc_db::Memo;
use kernc_lower::Lowerer;
use kernc_sema::SemaContext;
use kernc_sema::def::DefId;
use kernc_utils::Session;
use kernc_utils::config::{AsmDialect, CompileOptions, DriverMode};

use crate::frontend::FrontendDatabase;
use crate::metadata;

impl CompilerDriver {
    pub fn new(options: CompileOptions) -> Self {
        Self {
            options,
            frontend: FrontendDatabase::new(),
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
                    Self::print_mast_workload(report.mast_workload.as_ref());
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
                mast_workload: None,
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

        let mast_module = Self::measure_phase(&mut phase_timings, "lower", || {
            self.lower_module_with_flow(
                &mut ctx,
                &body_pipeline.flow_lowering_hints,
                &body_pipeline.lowered_module_items,
            )
        })?;
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

        let codegen_ctx = Context::create();
        let mut codegen = CodeGenerator::new(
            &codegen_ctx,
            &self.module_name_for_codegen(input_file),
            &mut *ctx.sess,
            &ctx.type_registry,
        );

        codegen.set_asm_dialect(match self.options.asm_dialect {
            AsmDialect::Intel => InlineAsmDialect::Intel,
            AsmDialect::Att => InlineAsmDialect::ATT,
        });
        Self::measure_phase(&mut phase_timings, "codegen", || {
            codegen.compile(&mast_module)
        });

        if self.options.driver_mode == DriverMode::EmitLlvmIr {
            return match Self::measure_phase(&mut phase_timings, "emit_llvm_ir", || {
                codegen.print_ir()
            }) {
                Ok(()) => {
                    Self::print_buffered_diagnostics(ctx.sess);
                    Some(CompileReport {
                        loaded_sources,
                        phase_timings,
                        cache_stats: self.cache_stats_since(cache_snapshot),
                        mast_workload: Some(mast_workload),
                    })
                }
                Err(err) => {
                    eprintln!("Error: Failed to print LLVM IR: {}", err);
                    None
                }
            };
        }

        let target = self.normalized_target();
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
        phase_timings.extend(emit_report.timings.into_iter().map(|timing| PhaseTiming {
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
            return Some(CompileReport {
                loaded_sources,
                phase_timings,
                cache_stats: self.cache_stats_since(cache_snapshot),
                mast_workload: Some(mast_workload),
            });
        }

        let linked = Self::measure_phase(&mut phase_timings, "link", || {
            self.run_link_command(Some(&link_input_path), &target, "Successfully compiled")
        });
        if linked {
            Self::print_buffered_diagnostics(ctx.sess);
        }
        linked.then_some(CompileReport {
            loaded_sources,
            phase_timings,
            cache_stats: self.cache_stats_since(cache_snapshot),
            mast_workload: Some(mast_workload),
        })
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

    pub(super) fn lower_module_with_flow<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        flow_lowering_hints: &kernc_lower::FlowLoweringHints,
        reachable_items: &std::collections::HashSet<DefId>,
    ) -> Option<kernc_mast::MastModule> {
        let mut lowerer = Lowerer::new(ctx);
        lowerer.set_reachable_module_items(reachable_items.clone());
        lowerer.set_flow_lowering_hints(flow_lowering_hints.clone());
        let module = lowerer.lower_all();
        if !Self::report_diagnostics_if_errors(lowerer.context()) {
            return None;
        }
        Some(module)
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
