mod compile;
mod partitioned;
mod reporting;

use super::codegen_units::{
    CodegenPlanFallback, CodegenPlanReport, materialize_codegen_unit,
    plan_codegen_units_with_mir_summary, plan_codegen_units_with_mir_workload,
};
#[cfg(test)]
use super::flow::FlowModel;
use super::{
    CompileCacheStats, CompileReport, CompilerDriver, LinkTarget, PhaseTiming, SourceOverrides,
    StructureArtifact, StructureCacheKey,
};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kernc_codegen::{
    AllocaNameStat, CodeGenerator, CodegenAllocaStats, CodegenReport, Context, EmitObjectReport,
    InlineAsmDialect, IrCleanupStats, IrFunctionStats, IrInstructionStats, ThinLtoModule,
    run_thin_lto,
};
use kernc_db::Memo;
use kernc_flow::FlowLoweringHints;
use kernc_lower::Lowerer;
use kernc_sema::SemaContext;
use kernc_sema::def::DefId;
use kernc_utils::Session;
use kernc_utils::config::{AsmDialect, CompileOptions, DriverMode, LtoMode};

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

#[derive(Clone, Copy)]
struct CompileReportContext<'a> {
    loaded_sources: &'a [PathBuf],
    cache_stats: CompileCacheStats,
    lower_cache_stats: kernc_lower::LowerCacheStats,
    mast_workload: kernc_mast::MastWorkloadStats,
    mir_workload: kernc_mir::MirWorkloadStats,
    codegen_plan: &'a Option<CodegenPlanReport>,
    collect_codegen_diagnostics: bool,
}

struct CompilePipelineContext<'a, 'ctx> {
    sema: &'a mut SemaContext<'ctx>,
    phase_timings: &'a mut Vec<PhaseTiming>,
    target: &'a LinkTarget,
    module_name: &'a str,
    report: CompileReportContext<'a>,
}

#[derive(Clone, Copy)]
struct CodegenUnitBuildContext<'a> {
    module_name: &'a str,
    target_triple: &'a str,
    session: &'a Session,
    type_registry: &'a kernc_sema::ty::TypeRegistry,
    collect_diagnostics: bool,
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
            clean_collected_reuse_artifacts: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            clean_imported_reuse_artifacts: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            clean_structure_reuse_artifacts: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            cache_counters: std::sync::Arc::new(Default::default()),
        }
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
        let StructureArtifact {
            session: restored_session,
            snapshot,
            ..
        } = structure;
        *session = restored_session;

        let mut ctx = self.build_sema_context(session);
        ctx.restore_structure(snapshot);
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
        flow_lowering_hints: &FlowLoweringHints,
        reachable_items: &std::collections::HashSet<DefId>,
    ) -> Option<kernc_mast::MastModule> {
        self.lower_module_with_flow_report(ctx, flow_lowering_hints, reachable_items)
            .map(|report| report.module)
    }

    fn lower_module_with_flow_report<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        flow_lowering_hints: &FlowLoweringHints,
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
    normalize_platform_path(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
}

fn normalize_platform_path(path: PathBuf) -> PathBuf {
    let path = strip_windows_verbatim_prefix(path);
    strip_macos_private_var_prefix(path)
}

#[cfg(windows)]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix("\\\\?\\UNC\\") {
        return PathBuf::from(format!("\\\\{stripped}"));
    }
    if let Some(stripped) = raw.strip_prefix("\\\\?\\") {
        return PathBuf::from(stripped);
    }
    path
}

#[cfg(not(windows))]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    path
}

#[cfg(target_os = "macos")]
fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix("/private/var/") {
        return PathBuf::from(format!("/var/{stripped}"));
    }
    if raw == "/private/var" {
        return PathBuf::from("/var");
    }
    path
}

#[cfg(not(target_os = "macos"))]
fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
    path
}
