mod lints;
mod reuse;
mod surface;

use self::reuse::{
    classify_function_body_decl_changes, module_file_id, module_source_changed,
    modules_match_ignoring_body_only, normalize_driver_path, rebind_module_defs,
};
use super::completion::CompletionModel;
use super::flow::FlowModel;
use super::signature::SignatureModel;
use super::{
    AnalysisArtifact, AnalysisHover, AnalysisOutline, AnalysisReference, AnalysisReport,
    AnalysisSemanticEntry, AnalysisSemanticKind, AnalysisSemanticRole, AnalysisSpanReplacement,
    AnalysisSurfaceArtifact, AnalysisSymbol, AnalysisSymbolKind, AnalysisUnusedBinding,
    AnalysisUnusedBindingKind, AnalysisUnusedItem, AnalysisUnusedItemKind,
    CollectedStructureArtifact, CompileStructureArtifact, CompilerDriver,
    ImportedStructureArtifact, ParsedModule, ParsedModuleArtifact, PhaseTiming, SourceOverrides,
    StructureArtifact, TargetedAnalysisReport,
};
use crate::doc::{lint_docs, render_hover_markdown};
use crate::loader::ModuleLoader;
use kernc_ast as ast;
use kernc_sema::checker::TypeckDriver;
use kernc_sema::def::{Def, DefId, FunctionDef, Visibility};
use kernc_sema::passes::{Collector, ImportResolver, LinkageChecker, TypeResolver};
use kernc_sema::scope::ScopeId;
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_sema::{BuiltinInjector, SemaContext, SemanticDefinition, SemanticSymbolKind};
use kernc_utils::{NodeId, Session, Span};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug)]
struct FunctionBodyReusePlan {
    worklist: Vec<(DefId, ScopeId)>,
    replaced_spans: Vec<AnalysisSpanReplacement>,
}

pub(super) struct LoadedAstArtifact {
    asts: Vec<(DefId, ast::Module)>,
    phase_timings: Vec<PhaseTiming>,
}

pub(super) struct BodyPipelineReport {
    pub(super) flow_lowering_hints: kernc_lower::FlowLoweringHints,
    pub(super) lowered_module_items: std::collections::HashSet<DefId>,
    pub(super) phase_timings: Vec<PhaseTiming>,
}

pub(in crate::compiler) struct ModuleItemReachability {
    nodes: std::collections::HashMap<DefId, ReachabilityItemNode>,
    pub(in crate::compiler) reachable: std::collections::HashSet<DefId>,
    pub(in crate::compiler) lowered_reachable: std::collections::HashSet<DefId>,
}

#[derive(Debug, Clone, Copy)]
enum ReachabilityItemKind {
    Function,
    Constant,
    Static,
}

#[derive(Debug, Clone, Copy)]
struct ReachabilityItemNode {
    def_id: DefId,
    name_span: Span,
    kind: ReachabilityItemKind,
    is_root: bool,
    is_lower_root: bool,
    is_warnable: bool,
}

impl CompilerDriver {
    pub fn analyze_report(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> AnalysisReport {
        let mut session = Session::new();
        let succeeded = {
            let ctx = self.analyze_with_overrides(&mut session, input_file, source_overrides);
            ctx.is_some()
        };

        AnalysisReport { session, succeeded }
    }

    pub fn analyze_artifact(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> AnalysisArtifact {
        let mut session = Session::new();
        session.apply_options(&self.options);

        match self.try_analyze_structure(session, input_file, source_overrides) {
            Ok(structure) => self.analyze_artifact_from_structure(&structure),
            Err(session) => self.empty_analysis_artifact(*session),
        }
    }

    pub fn analyze_imported_structure(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<ImportedStructureArtifact> {
        if let Some(structure) = self.cached_structure_artifact(input_file, source_overrides) {
            return Some(self.imported_structure_from_typed(&structure));
        }

        if let Some(imported) =
            self.cached_imported_structure_artifact(input_file, source_overrides)
        {
            return Some(imported);
        }

        if let Some(collected) =
            self.cached_collected_structure_artifact(input_file, source_overrides)
        {
            return self.build_imported_structure(&collected);
        }

        let mut session = Session::new();
        session.apply_options(&self.options);
        self.try_analyze_imported_structure(session, input_file, source_overrides)
            .ok()
    }

    pub fn analyze_surface(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<AnalysisSurfaceArtifact> {
        if let Some(structure) = self.cached_structure_artifact(input_file, source_overrides) {
            return Some(self.surface_from_structure(&structure));
        }

        let imported = self.analyze_imported_structure(input_file, source_overrides)?;
        Some(self.surface_from_imported(&imported))
    }

    pub fn analyze_outline(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> AnalysisOutline {
        if let Some(structure) = self.cached_structure_artifact(input_file, source_overrides) {
            return self.analyze_outline_from_structure(&structure);
        }

        if let Some(imported) =
            self.cached_imported_structure_artifact(input_file, source_overrides)
        {
            return self.analyze_outline_from_imported(&imported);
        }

        if let Some(collected) =
            self.cached_collected_structure_artifact(input_file, source_overrides)
        {
            return self.analyze_outline_from_collected(&collected);
        }

        if let Some(collected) = self.analyze_collected_structure(input_file, source_overrides) {
            return self.analyze_outline_from_collected(&collected);
        }

        match self.try_parse_modules_with_frontend_cache(input_file, source_overrides) {
            Some(parsed) => self.analyze_outline_from_parsed(&parsed),
            None => {
                let mut session = Session::new();
                session.apply_options(&self.options);
                AnalysisOutline {
                    session,
                    symbols: Vec::new(),
                }
            }
        }
    }

    pub fn parse_modules(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<ParsedModuleArtifact> {
        if let Some(structure) = self.cached_structure_artifact(input_file, source_overrides) {
            return Some(self.parsed_modules_from_structure(&structure));
        }

        if let Some(imported) =
            self.cached_imported_structure_artifact(input_file, source_overrides)
        {
            return Some(self.parsed_modules_from_imported(&imported));
        }

        if let Some(collected) =
            self.cached_collected_structure_artifact(input_file, source_overrides)
        {
            return Some(self.parsed_modules_from_collected(&collected));
        }

        if let Some(collected) = self.analyze_collected_structure(input_file, source_overrides) {
            return Some(self.parsed_modules_from_collected(&collected));
        }

        self.try_parse_modules_with_frontend_cache(input_file, source_overrides)
    }

    fn try_parse_modules_with_frontend_cache(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<ParsedModuleArtifact> {
        let mut session = Session::new();
        session.apply_options(&self.options);
        self.try_parse_modules(session, input_file, source_overrides)
            .ok()
    }

    pub fn analyze_artifact_from_structure(
        &self,
        structure: &StructureArtifact,
    ) -> AnalysisArtifact {
        let mut session = structure.session.clone();
        let analysis_asts = structure.asts.clone();

        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        let succeeded = self.run_body_pipeline(&mut ctx);
        let symbols = self.collect_analysis_symbols(&ctx, &analysis_asts);
        let references = ctx
            .identifier_references()
            .iter()
            .map(|(reference_span, definition_span)| AnalysisReference {
                reference_span: *reference_span,
                definition_span: *definition_span,
            })
            .collect::<Vec<_>>();
        let raw_references = references
            .iter()
            .map(|reference| (reference.reference_span, reference.definition_span))
            .collect::<Vec<_>>();
        let hovers = self.collect_analysis_hovers(&ctx);
        let semantic_entries = self.collect_analysis_semantic_entries(&symbols, &ctx, &references);
        let completion_model = self.collect_completion_model(&mut ctx, &analysis_asts);
        let signature_model = self.collect_signature_model(&mut ctx, &analysis_asts);
        let flow_model = self.collect_flow_model(&ctx, &references);
        let unused_items = self.collect_unused_private_items(&ctx, &raw_references, &flow_model);
        let unused_bindings = self.collect_unused_bindings(&ctx, &flow_model);
        let dead_stores = self.collect_dead_stores(&ctx, &raw_references, &flow_model);
        let resolved_globals = self.collect_resolved_globals(&ctx);
        drop(ctx);

        AnalysisArtifact {
            session,
            succeeded,
            symbols,
            references,
            hovers,
            semantic_entries,
            asts: analysis_asts,
            resolved_globals,
            completion_model,
            signature_model,
            flow_model,
            unused_items,
            unused_bindings,
            dead_stores,
        }
    }

    pub fn analyze_report_from_structure_and_parsed(
        &self,
        structure: &StructureArtifact,
        parsed: &ParsedModuleArtifact,
    ) -> Option<AnalysisReport> {
        let mut session = parsed.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        if !self.rebind_body_only_modules(&mut ctx, structure, parsed) {
            return None;
        }
        let succeeded = self.run_body_pipeline(&mut ctx);
        drop(ctx);

        Some(AnalysisReport { session, succeeded })
    }

    pub fn analyze_report_with_function_body_reuse(
        &self,
        clean_artifact: &AnalysisArtifact,
        structure: &StructureArtifact,
        parsed: &ParsedModuleArtifact,
    ) -> Option<TargetedAnalysisReport> {
        let mut session = parsed.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        self.apply_resolved_globals(&mut ctx, &clean_artifact.resolved_globals);

        let plan = self.build_function_body_reuse_plan(&ctx, &clean_artifact.asts, parsed)?;
        if plan.worklist.is_empty() {
            return None;
        }
        if !self.rebind_body_only_modules(&mut ctx, structure, parsed) {
            return None;
        }

        let mut typeck = TypeckDriver::new(&mut ctx);
        typeck.check_body_worklist(&plan.worklist);
        let ctx = typeck.into_context();
        let references = self.merge_targeted_identifier_references(
            clean_artifact,
            &plan.replaced_spans,
            ctx.identifier_references(),
        );
        let flow_model = self.collect_flow_model_from_raw_references(ctx, &references);
        self.emit_unused_private_item_warnings(ctx, &references, &flow_model);
        self.emit_unused_binding_warnings(ctx, &flow_model);
        self.emit_dead_store_warnings(ctx, &references, &flow_model);
        let succeeded = Self::report_diagnostics_if_errors(ctx);

        Some(TargetedAnalysisReport {
            report: AnalysisReport { session, succeeded },
            replaced_spans: plan.replaced_spans,
        })
    }

    pub fn analyze_outline_from_structure(&self, structure: &StructureArtifact) -> AnalysisOutline {
        AnalysisOutline {
            session: structure.session.clone(),
            symbols: structure.symbols.clone(),
        }
    }

    fn analyze_outline_from_imported(
        &self,
        imported: &ImportedStructureArtifact,
    ) -> AnalysisOutline {
        AnalysisOutline {
            session: imported.session.clone(),
            symbols: imported.symbols.clone(),
        }
    }

    fn analyze_outline_from_collected(
        &self,
        collected: &CollectedStructureArtifact,
    ) -> AnalysisOutline {
        AnalysisOutline {
            session: collected.session.clone(),
            symbols: collected.symbols.clone(),
        }
    }

    fn surface_from_structure(&self, structure: &StructureArtifact) -> AnalysisSurfaceArtifact {
        AnalysisSurfaceArtifact {
            session: structure.session.clone(),
            symbols: structure.symbols.clone(),
            completion_model: structure.completion_model.clone(),
        }
    }

    fn surface_from_imported(
        &self,
        imported: &ImportedStructureArtifact,
    ) -> AnalysisSurfaceArtifact {
        AnalysisSurfaceArtifact {
            session: imported.session.clone(),
            symbols: imported.symbols.clone(),
            completion_model: imported.completion_model.clone(),
        }
    }

    pub fn analyze_outline_from_parsed(&self, parsed: &ParsedModuleArtifact) -> AnalysisOutline {
        AnalysisOutline {
            session: parsed.session.clone(),
            symbols: self.collect_parsed_module_symbols(&parsed.session, &parsed.modules),
        }
    }

    fn parsed_modules_from_structure(&self, structure: &StructureArtifact) -> ParsedModuleArtifact {
        self.parsed_modules_from_snapshot(
            structure.session.clone(),
            structure.asts.clone(),
            structure.snapshot.clone(),
        )
    }

    fn parsed_modules_from_imported(
        &self,
        imported: &ImportedStructureArtifact,
    ) -> ParsedModuleArtifact {
        self.parsed_modules_from_snapshot(
            imported.session.clone(),
            imported.asts.clone(),
            imported.snapshot.clone(),
        )
    }

    fn parsed_modules_from_collected(
        &self,
        collected: &CollectedStructureArtifact,
    ) -> ParsedModuleArtifact {
        self.parsed_modules_from_snapshot(
            collected.session.clone(),
            collected.asts.clone(),
            collected.snapshot.clone(),
        )
    }

    fn parsed_modules_from_snapshot(
        &self,
        mut session: Session,
        asts: Vec<(DefId, ast::Module)>,
        snapshot: kernc_sema::SemaStructureSnapshot,
    ) -> ParsedModuleArtifact {
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(snapshot);
        let modules = asts
            .iter()
            .map(|(mod_id, ast)| {
                let name = match &ctx.defs[mod_id.0 as usize] {
                    kernc_sema::def::Def::Module(module_def) => {
                        ctx.resolve(module_def.name).to_string()
                    }
                    _ => "<unknown>".to_string(),
                };
                let file_id = match &ctx.defs[mod_id.0 as usize] {
                    kernc_sema::def::Def::Module(module_def) => module_def.file_id,
                    _ => kernc_utils::FileId(0),
                };
                let path = ctx
                    .sess
                    .source_manager
                    .get_file_path(file_id)
                    .map(|path| normalize_driver_path(path))
                    .unwrap_or_default();
                ParsedModule {
                    name,
                    file_id,
                    path,
                    body_regions: super::completion::module_body_completion_regions(ast),
                    ast: ast.clone(),
                }
            })
            .collect();
        drop(ctx);

        ParsedModuleArtifact { session, modules }
    }

    pub(super) fn build_sema_context<'a>(&self, session: &'a mut Session) -> SemaContext<'a> {
        let mut ctx = SemaContext::new(session);
        ctx.module_aliases = self.options.module_aliases.clone();
        ctx.module_interface_aliases = self.options.module_interface_aliases.clone();

        let mut builtin = BuiltinInjector::new(&mut ctx);
        builtin.inject();
        ctx
    }

    pub(super) fn load_asts<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        input_file: &str,
        collect_docs: bool,
    ) -> Option<LoadedAstArtifact> {
        let mut loader = ModuleLoader::new(ctx, &self.frontend, collect_docs);
        let root_name = loader
            .ctx
            .intern(self.options.root_module_name.as_deref().unwrap_or("root"));
        if loader.load_root(input_file, root_name).is_none() {
            loader.ctx.sess.print_diagnostics();
            return None;
        }
        if !Self::report_diagnostics_if_errors(loader.ctx) {
            return None;
        }

        loader.ctx.inject_alias_roots();
        Some(LoadedAstArtifact {
            asts: std::mem::take(&mut loader.asts),
            phase_timings: loader.phase_timings(),
        })
    }

    pub(super) fn try_analyze_structure(
        &self,
        mut session: Session,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Result<StructureArtifact, Box<Session>> {
        self.sync_source_overrides(source_overrides);
        if let Some(structure) = self.cached_structure_artifact(input_file, source_overrides) {
            return Ok(structure);
        }
        if let Some(imported) =
            self.cached_imported_structure_artifact(input_file, source_overrides)
        {
            return self
                .build_typed_structure(&imported)
                .ok_or_else(|| Box::new(session));
        }
        let cache_key = self.structure_cache_key(input_file, source_overrides);
        match self.structure_artifacts.get_with(
            self.frontend.db(),
            "driver_structure_artifact",
            cache_key,
            || Ok(self.compute_structure_artifact(input_file)),
        ) {
            Ok(Some(structure)) => Ok(structure),
            Ok(None) => {
                let structure =
                    self.compute_structure_artifact_into_session(&mut session, input_file);
                structure.ok_or_else(|| Box::new(session))
            }
            Err(_) => {
                let structure =
                    self.compute_structure_artifact_into_session(&mut session, input_file);
                structure.ok_or_else(|| Box::new(session))
            }
        }
    }

    pub(super) fn analyze_compile_structure(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<CompileStructureArtifact> {
        let mut session = Session::new();
        session.apply_options(&self.options);
        self.try_analyze_compile_structure(session, input_file, source_overrides)
            .ok()
    }

    fn try_analyze_compile_structure(
        &self,
        mut session: Session,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Result<CompileStructureArtifact, Box<Session>> {
        self.sync_source_overrides(source_overrides);
        if let Some(structure) = self.cached_structure_artifact(input_file, source_overrides) {
            return Ok(CompileStructureArtifact {
                session: structure.session,
                snapshot: structure.snapshot,
                phase_timings: Vec::new(),
            });
        }
        if let Some(imported) =
            self.cached_imported_structure_artifact(input_file, source_overrides)
        {
            return self
                .build_compile_structure_from_imported(&imported)
                .ok_or_else(|| Box::new(session));
        }
        if let Some(collected) =
            self.cached_collected_structure_artifact(input_file, source_overrides)
        {
            return self
                .build_compile_structure(&collected)
                .ok_or_else(|| Box::new(session));
        }

        let structure =
            self.compute_compile_structure_artifact_into_session(&mut session, input_file);
        structure.ok_or_else(|| Box::new(session))
    }

    fn analyze_collected_structure(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<CollectedStructureArtifact> {
        let mut session = Session::new();
        session.apply_options(&self.options);
        self.try_analyze_collected_structure(session, input_file, source_overrides)
            .ok()
    }

    fn cached_structure_artifact(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<StructureArtifact> {
        self.sync_source_overrides(source_overrides);
        let cache_key = self.structure_cache_key(input_file, source_overrides);
        let cached = self
            .structure_artifacts
            .get_cached(self.frontend.db(), "driver_structure_artifact", cache_key)
            .ok()
            .flatten()
            .flatten();
        if cached.is_some() {
            self.record_structure_cache_hit();
        } else {
            self.record_structure_cache_miss();
        }
        cached
    }

    fn cached_collected_structure_artifact(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<CollectedStructureArtifact> {
        self.sync_source_overrides(source_overrides);
        let cache_key = self.structure_cache_key(input_file, source_overrides);
        let cached = self
            .collected_artifacts
            .get_cached(
                self.frontend.db(),
                "driver_collected_structure_artifact",
                cache_key,
            )
            .ok()
            .flatten()
            .flatten();
        if cached.is_some() {
            self.record_collected_cache_hit();
        } else {
            self.record_collected_cache_miss();
        }
        cached
    }

    fn cached_imported_structure_artifact(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<ImportedStructureArtifact> {
        self.sync_source_overrides(source_overrides);
        let cache_key = self.structure_cache_key(input_file, source_overrides);
        let cached = self
            .imported_artifacts
            .get_cached(
                self.frontend.db(),
                "driver_imported_structure_artifact",
                cache_key,
            )
            .ok()
            .flatten()
            .flatten();
        if cached.is_some() {
            self.record_imported_cache_hit();
        } else {
            self.record_imported_cache_miss();
        }
        cached
    }

    fn try_analyze_collected_structure(
        &self,
        mut session: Session,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Result<CollectedStructureArtifact, Box<Session>> {
        self.sync_source_overrides(source_overrides);
        let cache_key = self.structure_cache_key(input_file, source_overrides);
        match self.collected_artifacts.get_with(
            self.frontend.db(),
            "driver_collected_structure_artifact",
            cache_key,
            || Ok(self.compute_collected_structure_artifact(input_file)),
        ) {
            Ok(Some(collected)) => Ok(collected),
            Ok(None) => {
                let collected = self
                    .compute_collected_structure_artifact_into_session(&mut session, input_file);
                collected.ok_or_else(|| Box::new(session))
            }
            Err(_) => {
                let collected = self
                    .compute_collected_structure_artifact_into_session(&mut session, input_file);
                collected.ok_or_else(|| Box::new(session))
            }
        }
    }

    fn try_analyze_imported_structure(
        &self,
        mut session: Session,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Result<ImportedStructureArtifact, Box<Session>> {
        self.sync_source_overrides(source_overrides);
        let cache_key = self.structure_cache_key(input_file, source_overrides);
        match self.imported_artifacts.get_with(
            self.frontend.db(),
            "driver_imported_structure_artifact",
            cache_key,
            || Ok(self.compute_imported_structure_artifact(input_file)),
        ) {
            Ok(Some(imported)) => Ok(imported),
            Ok(None) => {
                let imported =
                    self.compute_imported_structure_artifact_into_session(&mut session, input_file);
                imported.ok_or_else(|| Box::new(session))
            }
            Err(_) => {
                let imported =
                    self.compute_imported_structure_artifact_into_session(&mut session, input_file);
                imported.ok_or_else(|| Box::new(session))
            }
        }
    }

    fn compute_collected_structure_artifact(
        &self,
        input_file: &str,
    ) -> Option<CollectedStructureArtifact> {
        let mut session = Session::new();
        session.apply_options(&self.options);
        self.compute_collected_structure_artifact_into_session(&mut session, input_file)
    }

    fn compute_collected_structure_artifact_into_session(
        &self,
        session: &mut Session,
        input_file: &str,
    ) -> Option<CollectedStructureArtifact> {
        let mut ctx = self.build_sema_context(session);
        let loaded = self.load_asts(&mut ctx, input_file, true)?;
        self.build_collected_structure_from_context(&mut ctx, loaded.asts)
    }

    fn compute_imported_structure_artifact(
        &self,
        input_file: &str,
    ) -> Option<ImportedStructureArtifact> {
        let mut session = Session::new();
        session.apply_options(&self.options);
        self.compute_imported_structure_artifact_into_session(&mut session, input_file)
    }

    fn compute_imported_structure_artifact_into_session(
        &self,
        session: &mut Session,
        input_file: &str,
    ) -> Option<ImportedStructureArtifact> {
        let mut ctx = self.build_sema_context(session);
        let loaded = self.load_asts(&mut ctx, input_file, true)?;
        let asts = loaded.asts;
        if !self.run_collect_phase(&mut ctx, &asts) {
            return None;
        }
        let symbols = self.collect_analysis_symbols(&ctx, &asts);
        if !self.run_import_phase(&mut ctx) {
            return None;
        }
        let completion_model = self.collect_structure_completion_model(&ctx, &asts);
        let snapshot = ctx.into_structure_snapshot();
        let session = std::mem::take(session);

        Some(ImportedStructureArtifact {
            session,
            asts,
            symbols,
            snapshot,
            completion_model,
        })
    }

    fn compute_structure_artifact(&self, input_file: &str) -> Option<StructureArtifact> {
        let mut session = Session::new();
        session.apply_options(&self.options);
        self.compute_structure_artifact_into_session(&mut session, input_file)
    }

    fn compute_compile_structure_artifact_into_session(
        &self,
        session: &mut Session,
        input_file: &str,
    ) -> Option<CompileStructureArtifact> {
        let mut phase_timings = Vec::new();
        let mut ctx = self.build_sema_context(session);
        let collect_docs = self.options.metadata_output.is_some();
        let loaded = measure_body_phase(&mut phase_timings, "  structure_load_asts", || {
            self.load_asts(&mut ctx, input_file, collect_docs)
        })?;
        phase_timings.extend(loaded.phase_timings);
        let asts = loaded.asts;
        if !measure_body_phase(&mut phase_timings, "  structure_collect", || {
            self.run_collect_phase(&mut ctx, &asts)
        }) {
            return None;
        }
        if !measure_body_phase(&mut phase_timings, "  structure_import", || {
            self.run_import_phase(&mut ctx)
        }) {
            return None;
        }
        if !measure_body_phase(&mut phase_timings, "  structure_type_resolution", || {
            self.run_type_resolution_phase(&mut ctx, collect_docs)
        }) {
            return None;
        }

        let snapshot = measure_body_phase(&mut phase_timings, "  structure_snapshot", || {
            ctx.into_structure_snapshot()
        });
        let session = std::mem::take(session);

        Some(CompileStructureArtifact {
            session,
            snapshot,
            phase_timings,
        })
    }

    fn compute_structure_artifact_into_session(
        &self,
        session: &mut Session,
        input_file: &str,
    ) -> Option<StructureArtifact> {
        let mut ctx = self.build_sema_context(session);
        let loaded = self.load_asts(&mut ctx, input_file, true)?;
        let asts = loaded.asts;
        if !self.run_collect_phase(&mut ctx, &asts) {
            return None;
        }
        let symbols = self.collect_analysis_symbols(&ctx, &asts);
        if !self.run_import_phase(&mut ctx) {
            return None;
        }
        if !self.run_type_resolution_phase(&mut ctx, true) {
            return None;
        }

        let completion_model = self.collect_structure_completion_model(&ctx, &asts);
        let snapshot = ctx.into_structure_snapshot();
        let session = std::mem::take(session);

        Some(StructureArtifact {
            session,
            asts,
            symbols,
            snapshot,
            completion_model,
        })
    }

    fn imported_structure_from_typed(
        &self,
        structure: &StructureArtifact,
    ) -> ImportedStructureArtifact {
        ImportedStructureArtifact {
            session: structure.session.clone(),
            asts: structure.asts.clone(),
            symbols: structure.symbols.clone(),
            snapshot: structure.snapshot.clone(),
            completion_model: structure.completion_model.clone(),
        }
    }

    pub(super) fn try_parse_modules(
        &self,
        mut session: Session,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Result<ParsedModuleArtifact, Box<Session>> {
        self.sync_source_overrides(source_overrides);
        let mut ctx = self.build_sema_context(&mut session);
        let Some(loaded) = self.load_asts(&mut ctx, input_file, true) else {
            return Err(Box::new(session));
        };
        let modules = loaded
            .asts
            .into_iter()
            .map(|(mod_id, ast)| {
                let name = match &ctx.defs[mod_id.0 as usize] {
                    kernc_sema::def::Def::Module(module_def) => {
                        ctx.resolve(module_def.name).to_string()
                    }
                    _ => "<unknown>".to_string(),
                };
                let file_id = match &ctx.defs[mod_id.0 as usize] {
                    kernc_sema::def::Def::Module(module_def) => module_def.file_id,
                    _ => kernc_utils::FileId(0),
                };
                let path = ctx
                    .sess
                    .source_manager
                    .get_file_path(file_id)
                    .map(|path| normalize_driver_path(path))
                    .unwrap_or_default();
                ParsedModule {
                    name,
                    file_id,
                    path,
                    body_regions: super::completion::module_body_completion_regions(&ast),
                    ast,
                }
            })
            .collect();
        drop(ctx);

        Ok(ParsedModuleArtifact { session, modules })
    }

    pub(super) fn run_body_pipeline<'a>(&self, ctx: &mut SemaContext<'a>) -> bool {
        self.run_body_pipeline_with_report(ctx).is_some()
    }

    pub(super) fn run_body_pipeline_with_report<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
    ) -> Option<BodyPipelineReport> {
        let mut phase_timings = Vec::new();
        let mut typeck = TypeckDriver::new(ctx);
        let globals = typeck.global_worklist();
        measure_body_phase(&mut phase_timings, "typeck_globals", || {
            typeck.resolve_global_worklist(&globals);
        });
        let worklist = typeck.body_worklist();
        let _ = measure_body_phase(&mut phase_timings, "typeck_bodies", || {
            typeck.check_body_worklist(&worklist)
        });
        phase_timings.extend(
            typeck
                .body_phase_timings()
                .into_iter()
                .map(|timing| PhaseTiming {
                    name: timing.name,
                    duration: timing.duration,
                }),
        );
        let ctx = typeck.into_context();
        if !Self::report_diagnostics_if_errors(ctx) {
            return None;
        }
        let references = ctx.identifier_references().to_vec();
        let flow_model = measure_body_phase(&mut phase_timings, "flow", || {
            self.collect_compile_flow_model_from_raw_references(ctx, &references)
        });
        phase_timings.extend(flow_model.phase_timings().iter().copied().map(|timing| {
            PhaseTiming {
                name: timing.name,
                duration: timing.duration,
            }
        }));
        let flow_lowering_hints = flow_model.lowering_hints(ctx);
        let reachability = self.compute_module_item_reachability(ctx, &references, &flow_model);
        let lowered_module_items = reachability.lowered_reachable.clone();
        measure_body_phase(&mut phase_timings, "warn_unused_items", || {
            self.emit_unused_private_item_warnings_with_reachability(ctx, &reachability);
        });
        measure_body_phase(&mut phase_timings, "warn_unused_bindings", || {
            self.emit_unused_binding_warnings(ctx, &flow_model);
        });
        measure_body_phase(&mut phase_timings, "warn_dead_stores", || {
            self.emit_dead_store_warnings(ctx, &references, &flow_model);
        });

        let mut linkage_checker = LinkageChecker::new(ctx);
        measure_body_phase(&mut phase_timings, "linkage", || {
            linkage_checker.check_all();
        });
        let ctx = linkage_checker.context();
        if !Self::report_diagnostics_if_errors(ctx) {
            return None;
        }

        Some(BodyPipelineReport {
            flow_lowering_hints,
            lowered_module_items,
            phase_timings,
        })
    }

    fn merge_targeted_identifier_references(
        &self,
        clean_artifact: &AnalysisArtifact,
        replaced_spans: &[AnalysisSpanReplacement],
        dirty_references: &[(Span, Span)],
    ) -> Vec<(Span, Span)> {
        let mut merged = clean_artifact
            .references
            .iter()
            .filter(|reference| {
                !replaced_spans
                    .iter()
                    .any(|replacement| span_contains(replacement.clean, reference.reference_span))
            })
            .map(|reference| (reference.reference_span, reference.definition_span))
            .collect::<std::collections::BTreeSet<_>>();

        merged.extend(dirty_references.iter().copied());
        merged.into_iter().collect()
    }

    fn empty_analysis_artifact(&self, session: Session) -> AnalysisArtifact {
        AnalysisArtifact {
            session,
            succeeded: false,
            symbols: Vec::new(),
            references: Vec::new(),
            hovers: Vec::new(),
            semantic_entries: Vec::new(),
            asts: Vec::new(),
            resolved_globals: Vec::new(),
            completion_model: CompletionModel::default(),
            signature_model: SignatureModel::default(),
            flow_model: FlowModel::default(),
            unused_items: Vec::new(),
            unused_bindings: Vec::new(),
            dead_stores: Vec::new(),
        }
    }

    fn build_collected_structure_from_context(
        &self,
        ctx: &mut SemaContext<'_>,
        asts: Vec<(DefId, ast::Module)>,
    ) -> Option<CollectedStructureArtifact> {
        if !self.run_collect_phase(ctx, &asts) {
            return None;
        }
        let symbols = self.collect_analysis_symbols(ctx, &asts);

        Some(CollectedStructureArtifact {
            session: ctx.sess.clone(),
            asts,
            symbols,
            snapshot: ctx.structure_snapshot(),
        })
    }

    fn build_imported_structure(
        &self,
        collected: &CollectedStructureArtifact,
    ) -> Option<ImportedStructureArtifact> {
        let mut session = collected.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(collected.snapshot.clone());
        if !self.run_import_phase(&mut ctx) {
            return None;
        }
        let asts = collected.asts.clone();
        let completion_model = self.collect_structure_completion_model(&ctx, &asts);
        let snapshot = ctx.structure_snapshot();
        drop(ctx);

        Some(ImportedStructureArtifact {
            session,
            asts,
            symbols: collected.symbols.clone(),
            snapshot,
            completion_model,
        })
    }

    fn build_compile_structure(
        &self,
        collected: &CollectedStructureArtifact,
    ) -> Option<CompileStructureArtifact> {
        let mut session = collected.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(collected.snapshot.clone());
        if !self.run_import_phase(&mut ctx) {
            return None;
        }
        if !self.run_type_resolution_phase(&mut ctx, self.options.metadata_output.is_some()) {
            return None;
        }

        let snapshot = ctx.into_structure_snapshot();
        let session = std::mem::take(&mut session);

        Some(CompileStructureArtifact {
            session,
            snapshot,
            phase_timings: Vec::new(),
        })
    }

    fn build_typed_structure(
        &self,
        imported: &ImportedStructureArtifact,
    ) -> Option<StructureArtifact> {
        let mut session = imported.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(imported.snapshot.clone());
        if !self.run_type_resolution_phase(&mut ctx, true) {
            return None;
        }

        let asts = imported.asts.clone();
        let completion_model = self.collect_structure_completion_model(&ctx, &asts);
        let snapshot = ctx.structure_snapshot();
        drop(ctx);

        Some(StructureArtifact {
            session,
            asts,
            symbols: imported.symbols.clone(),
            snapshot,
            completion_model,
        })
    }

    fn build_compile_structure_from_imported(
        &self,
        imported: &ImportedStructureArtifact,
    ) -> Option<CompileStructureArtifact> {
        let mut session = imported.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(imported.snapshot.clone());
        if !self.run_type_resolution_phase(&mut ctx, self.options.metadata_output.is_some()) {
            return None;
        }

        let snapshot = ctx.into_structure_snapshot();
        let session = std::mem::take(&mut session);

        Some(CompileStructureArtifact {
            session,
            snapshot,
            phase_timings: Vec::new(),
        })
    }

    fn run_collect_phase<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        asts: &[(DefId, ast::Module)],
    ) -> bool {
        let mut collector = Collector::new(ctx);
        for (mod_id, ast) in asts {
            collector.collect_ast(*mod_id, ast);
        }
        Self::report_diagnostics_if_errors(collector.context())
    }

    fn run_import_phase<'a>(&self, ctx: &mut SemaContext<'a>) -> bool {
        let mut import_resolver = ImportResolver::new(ctx);
        import_resolver.resolve_all();
        Self::report_diagnostics_if_errors(import_resolver.context())
    }

    fn run_type_resolution_phase<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        lint_docs_enabled: bool,
    ) -> bool {
        let mut type_resolver = TypeResolver::new(ctx);
        type_resolver.resolve_all();
        if !Self::report_diagnostics_if_errors(type_resolver.context()) {
            return false;
        }

        let ctx = type_resolver.into_context();
        if !self.configure_program_entry(ctx) {
            return false;
        }
        if lint_docs_enabled {
            lint_docs(ctx);
        }
        true
    }

    fn configure_program_entry(&self, ctx: &mut SemaContext<'_>) -> bool {
        if !ctx.program_entry_enabled() {
            return true;
        }

        self.synthesize_program_main_adapter(ctx);
        Self::report_diagnostics_if_errors(ctx)
    }

    fn synthesize_program_main_adapter(&self, ctx: &mut SemaContext<'_>) {
        let Some(root_module_id) = ctx.root_module else {
            return;
        };
        let Some((root_items, root_scope_id)) =
            ctx.defs
                .get(root_module_id.0 as usize)
                .and_then(|def| match def {
                    Def::Module(module) => Some((module.items.clone(), module.scope_id)),
                    _ => None,
                })
        else {
            return;
        };

        let main_name = ctx.intern("main");
        let main_argv_ty = ctx.main_argv_ptr_ty();

        let entry_main =
            root_items
                .iter()
                .find_map(|item_id| match &ctx.defs[item_id.0 as usize] {
                    Def::Function(function)
                        if function.parent == Some(root_module_id)
                            && function.name == main_name =>
                    {
                        Some(function.clone())
                    }
                    _ => None,
                });

        let Some(entry_main) = entry_main else {
            ctx.struct_error(Span::default(), "program entry mode requires a root `main` function")
                .with_hint("declare either `fn main() i32` or `fn main(argc: i32, argv: **u8) i32` in the root module")
                .emit();
            return;
        };

        let Some(main_arity_uses_args) =
            Self::validate_program_main(ctx, &entry_main, main_argv_ty)
        else {
            return;
        };

        let adapter_id = DefId(ctx.defs.len() as u32);
        let adapter_name = ctx.intern("__kern_main_adapter");
        let argc_name = ctx.intern("argc");
        let argv_name = ctx.intern("argv");
        let span = entry_main.name_span;
        let argc_pattern = ast::BindingPattern {
            name: argc_name,
            name_span: span,
            is_mut: false,
            span,
        };
        let argv_pattern = ast::BindingPattern {
            name: argv_name,
            name_span: span,
            is_mut: false,
            span,
        };
        let argc_type_node = Self::i32_type_node(ctx, span);
        let argv_type_node = Self::main_argv_type_node(ctx, span);
        let ret_type_node = Self::i32_type_node(ctx, span);
        let ptr_u8_ty = ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::U8,
        });
        ctx.node_types.insert(argc_type_node.id, TypeId::I32);
        Self::record_main_argv_type_nodes(ctx, &argv_type_node, main_argv_ty, ptr_u8_ty);
        ctx.node_types.insert(ret_type_node.id, TypeId::I32);
        let call_args = if main_arity_uses_args {
            vec![
                ast::Expr {
                    id: ctx.next_node_id(),
                    span,
                    kind: ast::ExprKind::Identifier(argc_name),
                },
                ast::Expr {
                    id: ctx.next_node_id(),
                    span,
                    kind: ast::ExprKind::Identifier(argv_name),
                },
            ]
        } else {
            Vec::new()
        };
        let body = ast::Expr {
            id: ctx.next_node_id(),
            span,
            kind: ast::ExprKind::Call {
                callee: Box::new(ast::Expr {
                    id: ctx.next_node_id(),
                    span,
                    kind: ast::ExprKind::Identifier(main_name),
                }),
                args: call_args,
            },
        };
        let adapter_sig = ctx.type_registry.intern(TypeKind::Function {
            params: vec![TypeId::I32, main_argv_ty],
            ret: TypeId::I32,
            is_variadic: false,
        });

        ctx.add_def(Def::Function(FunctionDef {
            id: adapter_id,
            name: adapter_name,
            name_span: span,
            vis: Visibility::Private,
            parent: Some(root_module_id),
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            params: vec![
                ast::FuncParam {
                    pattern: argc_pattern,
                    type_node: argc_type_node,
                    span,
                },
                ast::FuncParam {
                    pattern: argv_pattern,
                    type_node: argv_type_node,
                    span,
                },
            ],
            ret_type: ret_type_node,
            body: Some(Box::new(body)),
            is_const: false,
            is_extern: true,
            is_variadic: false,
            is_intrinsic: false,
            span,
            resolved_sig: Some(adapter_sig),
            docs: None,
            attributes: Vec::new(),
        }));
        ctx.register_def_owner(adapter_id, Some(root_module_id), Some(root_scope_id));

        if let Def::Module(module) = &mut ctx.defs[root_module_id.0 as usize] {
            module.items.push(adapter_id);
        }
    }

    fn validate_program_main(
        ctx: &mut SemaContext<'_>,
        main: &FunctionDef,
        main_argv_ty: TypeId,
    ) -> Option<bool> {
        if main.is_extern {
            ctx.struct_error(
                main.name_span,
                "program `main` must not be declared `extern`",
            )
            .with_hint("`main` is a language-level entry function when `runtime_entry != none`")
            .emit();
            return None;
        }

        if main.is_const {
            ctx.emit_error(main.name_span, "program `main` cannot be `const`");
            return None;
        }

        if !main.generics.is_empty() {
            ctx.emit_error(main.name_span, "program `main` cannot be generic");
            return None;
        }

        if main.body.is_none() {
            ctx.emit_error(main.name_span, "program `main` must have a body");
            return None;
        }

        let sig_ty = main.resolved_sig.unwrap_or(TypeId::ERROR);
        if sig_ty == TypeId::ERROR {
            return None;
        }

        let TypeKind::Function {
            params,
            ret,
            is_variadic,
        } = ctx.type_registry.get(sig_ty).clone()
        else {
            return None;
        };

        if is_variadic {
            ctx.emit_error(main.name_span, "program `main` cannot be variadic");
            return None;
        }

        if ret != TypeId::I32 {
            ctx.struct_error(main.ret_type.span, "program `main` must return `i32`")
                .with_hint("legal entry forms are `fn main() i32` and `fn main(argc: i32, argv: **u8) i32`")
                .emit();
            return None;
        }

        match params.as_slice() {
            [] => Some(false),
            [argc_ty, argv_ty] if *argc_ty == TypeId::I32 && *argv_ty == main_argv_ty => Some(true),
            [_, _] => {
                ctx.struct_error(
                    main.params[0].type_node.span,
                    "program `main` accepts only `(i32, **u8)` when it has parameters",
                )
                .with_hint("legal entry forms are `fn main() i32` and `fn main(argc: i32, argv: **u8) i32`")
                .emit();
                None
            }
            _ => {
                ctx.struct_error(
                    main.name_span,
                    "program `main` accepts either zero parameters or exactly `(i32, **u8)`",
                )
                .with_hint("legal entry forms are `fn main() i32` and `fn main(argc: i32, argv: **u8) i32`")
                .emit();
                None
            }
        }
    }

    fn i32_type_node(ctx: &mut SemaContext<'_>, span: Span) -> ast::TypeNode {
        ast::TypeNode {
            id: ctx.next_node_id(),
            span,
            kind: ast::TypeKind::Path {
                segments: vec![ctx.intern("i32")],
                segment_spans: vec![span],
                generics: Vec::new(),
            },
        }
    }

    fn u8_type_node(ctx: &mut SemaContext<'_>, span: Span) -> ast::TypeNode {
        ast::TypeNode {
            id: ctx.next_node_id(),
            span,
            kind: ast::TypeKind::Path {
                segments: vec![ctx.intern("u8")],
                segment_spans: vec![span],
                generics: Vec::new(),
            },
        }
    }

    fn main_argv_type_node(ctx: &mut SemaContext<'_>, span: Span) -> ast::TypeNode {
        ast::TypeNode {
            id: ctx.next_node_id(),
            span,
            kind: ast::TypeKind::Pointer {
                is_mut: false,
                elem: Box::new(ast::TypeNode {
                    id: ctx.next_node_id(),
                    span,
                    kind: ast::TypeKind::Pointer {
                        is_mut: false,
                        elem: Box::new(Self::u8_type_node(ctx, span)),
                    },
                }),
            },
        }
    }

    fn record_main_argv_type_nodes(
        ctx: &mut SemaContext<'_>,
        type_node: &ast::TypeNode,
        argv_ty: TypeId,
        ptr_u8_ty: TypeId,
    ) {
        ctx.node_types.insert(type_node.id, argv_ty);

        let ast::TypeKind::Pointer { elem, .. } = &type_node.kind else {
            return;
        };
        ctx.node_types.insert(elem.id, ptr_u8_ty);

        if let ast::TypeKind::Pointer { elem: inner, .. } = &elem.kind {
            ctx.node_types.insert(inner.id, TypeId::U8);
        }
    }

    fn collect_flow_model(
        &self,
        ctx: &SemaContext<'_>,
        references: &[AnalysisReference],
    ) -> FlowModel {
        let raw_references = references
            .iter()
            .map(|reference| (reference.reference_span, reference.definition_span))
            .collect::<Vec<_>>();
        self.collect_flow_model_from_raw_references(ctx, &raw_references)
    }

    fn collect_flow_model_from_raw_references(
        &self,
        ctx: &SemaContext<'_>,
        references: &[(Span, Span)],
    ) -> FlowModel {
        let module_item_definition_spans = self.module_item_definition_spans(ctx);
        FlowModel::collect(ctx, &module_item_definition_spans, references)
    }

    fn collect_compile_flow_model_from_raw_references(
        &self,
        ctx: &SemaContext<'_>,
        references: &[(Span, Span)],
    ) -> FlowModel {
        let module_item_definition_spans = self.module_item_definition_spans(ctx);
        FlowModel::collect_for_compile(ctx, &module_item_definition_spans, references)
    }

    fn rebind_body_only_modules(
        &self,
        ctx: &mut SemaContext<'_>,
        structure: &StructureArtifact,
        parsed: &ParsedModuleArtifact,
    ) -> bool {
        let mut clean_modules = Vec::with_capacity(structure.asts.len());
        for (module_id, module_ast) in &structure.asts {
            let Some(path) = structure
                .session
                .source_manager
                .get_file_path(module_file_id(&ctx.defs, *module_id))
            else {
                return false;
            };
            clean_modules.push((normalize_driver_path(path), *module_id, module_ast));
        }

        if clean_modules.len() != parsed.modules.len() {
            return false;
        }

        for parsed_module in &parsed.modules {
            let Some(path) = parsed
                .session
                .source_manager
                .get_file_path(parsed_module.file_id)
            else {
                return false;
            };
            let normalized = normalize_driver_path(path);
            let Some((module_id, clean_module)) =
                clean_modules
                    .iter()
                    .find_map(|(path, module_id, module_ast)| {
                        (path == &normalized).then_some((*module_id, *module_ast))
                    })
            else {
                return false;
            };

            let clean_file_id = module_file_id(&ctx.defs, module_id);
            let module_changed = module_source_changed(
                &structure.session,
                clean_file_id,
                &parsed.session,
                parsed_module.file_id,
            );
            if module_changed && !modules_match_ignoring_body_only(clean_module, &parsed_module.ast)
            {
                return false;
            }

            if !rebind_module_defs(ctx, module_id, parsed_module) {
                return false;
            }
        }

        true
    }

    fn apply_resolved_globals(
        &self,
        ctx: &mut SemaContext<'_>,
        globals: &[super::ResolvedGlobalType],
    ) {
        for global in globals {
            let _ = ctx
                .scopes
                .update_type_in_scope(global.scope_id, global.name, global.ty);
        }
    }

    fn collect_resolved_globals(&self, ctx: &SemaContext<'_>) -> Vec<super::ResolvedGlobalType> {
        let mut globals = Vec::new();

        for def in &ctx.defs {
            let kernc_sema::def::Def::Module(module) = def else {
                continue;
            };

            for item_id in &module.items {
                let kernc_sema::def::Def::Global(global) = &ctx.defs[item_id.0 as usize] else {
                    continue;
                };
                let Some(info) = ctx.scopes.resolve_in(module.scope_id, global.name) else {
                    continue;
                };
                if info.type_id == kernc_sema::ty::TypeId::ERROR {
                    continue;
                }

                globals.push(super::ResolvedGlobalType {
                    scope_id: module.scope_id,
                    name: global.name,
                    ty: info.type_id,
                });
            }
        }

        globals
    }

    fn build_function_body_reuse_plan(
        &self,
        ctx: &SemaContext<'_>,
        clean_asts: &[(DefId, ast::Module)],
        parsed: &ParsedModuleArtifact,
    ) -> Option<FunctionBodyReusePlan> {
        let mut clean_modules = Vec::with_capacity(clean_asts.len());
        for (module_id, module_ast) in clean_asts {
            let path = ctx
                .sess
                .source_manager
                .get_file_path(module_file_id(&ctx.defs, *module_id))?;
            clean_modules.push((normalize_driver_path(path), *module_id, module_ast));
        }

        let mut worklist = Vec::new();
        let mut replaced_spans = Vec::new();

        for parsed_module in &parsed.modules {
            let path = parsed
                .session
                .source_manager
                .get_file_path(parsed_module.file_id)?;
            let normalized = normalize_driver_path(path);
            let (module_id, clean_module) =
                clean_modules
                    .iter()
                    .find_map(|(path, module_id, module_ast)| {
                        (path == &normalized).then_some((*module_id, *module_ast))
                    })?;

            let clean_file_id = module_file_id(&ctx.defs, module_id);
            let module_changed = module_source_changed(
                ctx.sess,
                clean_file_id,
                &parsed.session,
                parsed_module.file_id,
            );
            if !module_changed {
                continue;
            }

            let module_scope = match &ctx.defs[module_id.0 as usize] {
                kernc_sema::def::Def::Module(module) => module.scope_id,
                _ => return None,
            };
            let module_items = match &ctx.defs[module_id.0 as usize] {
                kernc_sema::def::Def::Module(module) => module.items.clone(),
                _ => return None,
            };

            let mut item_iter = module_items.iter();
            if !classify_function_body_decl_changes(
                clean_module,
                &parsed_module.ast,
                &mut item_iter,
                module_scope,
                &mut worklist,
                &mut replaced_spans,
            ) {
                return None;
            }
            if item_iter.next().is_some() {
                return None;
            }
        }

        Some(FunctionBodyReusePlan {
            worklist,
            replaced_spans,
        })
    }
}

fn measure_body_phase<T, F>(phase_timings: &mut Vec<PhaseTiming>, name: &'static str, f: F) -> T
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

fn span_contains(outer: Span, inner: Span) -> bool {
    outer.file == inner.file && outer.start <= inner.start && inner.end <= outer.end
}
