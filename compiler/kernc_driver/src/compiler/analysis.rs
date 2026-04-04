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
    CollectedStructureArtifact, CompilerDriver, ImportedStructureArtifact, ParsedModule,
    ParsedModuleArtifact, SourceOverrides, StructureArtifact, TargetedAnalysisReport,
};
use crate::doc::{lint_docs, render_hover_markdown};
use crate::loader::ModuleLoader;
use kernc_ast as ast;
use kernc_sema::checker::TypeckDriver;
use kernc_sema::def::{DefId, Visibility};
use kernc_sema::passes::{Collector, ImportResolver, LinkageChecker, TypeResolver};
use kernc_sema::scope::ScopeId;
use kernc_sema::{BuiltinInjector, SemaContext, SemanticDefinition, SemanticSymbolKind};
use kernc_utils::{NodeId, Session, Span};
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct FunctionBodyReusePlan {
    worklist: Vec<(DefId, ScopeId)>,
    replaced_spans: Vec<AnalysisSpanReplacement>,
}

pub(in crate::compiler) struct ModuleItemReachability {
    nodes: std::collections::HashMap<DefId, ReachabilityItemNode>,
    pub(in crate::compiler) reachable: std::collections::HashSet<DefId>,
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
    ) -> Option<Vec<(DefId, ast::Module)>> {
        let mut loader = ModuleLoader::new(ctx, &self.frontend);
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
        Some(std::mem::take(&mut loader.asts))
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
        self.structure_artifacts
            .get_cached(self.frontend.db(), "driver_structure_artifact", cache_key)
            .ok()
            .flatten()
            .flatten()
    }

    fn cached_collected_structure_artifact(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<CollectedStructureArtifact> {
        self.sync_source_overrides(source_overrides);
        let cache_key = self.structure_cache_key(input_file, source_overrides);
        self.collected_artifacts
            .get_cached(
                self.frontend.db(),
                "driver_collected_structure_artifact",
                cache_key,
            )
            .ok()
            .flatten()
            .flatten()
    }

    fn cached_imported_structure_artifact(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<ImportedStructureArtifact> {
        self.sync_source_overrides(source_overrides);
        let cache_key = self.structure_cache_key(input_file, source_overrides);
        self.imported_artifacts
            .get_cached(
                self.frontend.db(),
                "driver_imported_structure_artifact",
                cache_key,
            )
            .ok()
            .flatten()
            .flatten()
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
        let Some(asts) = self.load_asts(&mut ctx, input_file) else {
            return None;
        };
        self.build_collected_structure_from_context(&mut ctx, asts)
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
        let Some(asts) = self.load_asts(&mut ctx, input_file) else {
            return None;
        };
        let collected = self.build_collected_structure_from_context(&mut ctx, asts)?;
        drop(ctx);

        self.build_imported_structure(&collected)
    }

    fn compute_structure_artifact(&self, input_file: &str) -> Option<StructureArtifact> {
        let mut session = Session::new();
        session.apply_options(&self.options);
        self.compute_structure_artifact_into_session(&mut session, input_file)
    }

    fn compute_structure_artifact_into_session(
        &self,
        session: &mut Session,
        input_file: &str,
    ) -> Option<StructureArtifact> {
        let mut ctx = self.build_sema_context(session);
        let Some(asts) = self.load_asts(&mut ctx, input_file) else {
            return None;
        };
        let collected = self.build_collected_structure_from_context(&mut ctx, asts)?;
        drop(ctx);

        let imported = self.build_imported_structure(&collected)?;
        self.build_typed_structure(&imported)
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
        let Some(asts) = self.load_asts(&mut ctx, input_file) else {
            return Err(Box::new(session));
        };
        let modules = asts
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
        let mut typeck = TypeckDriver::new(ctx);
        let globals = typeck.global_worklist();
        typeck.resolve_global_worklist(&globals);
        let worklist = typeck.body_worklist();
        typeck.check_body_worklist(&worklist);
        let ctx = typeck.into_context();
        if !Self::report_diagnostics_if_errors(ctx) {
            return false;
        }
        let references = ctx.identifier_references().to_vec();
        let flow_model = self.collect_flow_model_from_raw_references(ctx, &references);
        self.emit_unused_private_item_warnings(ctx, &references, &flow_model);
        self.emit_unused_binding_warnings(ctx, &flow_model);
        self.emit_dead_store_warnings(ctx, &references, &flow_model);

        let mut linkage_checker = LinkageChecker::new(ctx);
        linkage_checker.check_all();
        Self::report_diagnostics_if_errors(linkage_checker.context())
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
        if !self.run_collect_phase(ctx, asts.clone()) {
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

    fn build_typed_structure(
        &self,
        imported: &ImportedStructureArtifact,
    ) -> Option<StructureArtifact> {
        let mut session = imported.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(imported.snapshot.clone());
        if !self.run_type_resolution_phase(&mut ctx) {
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

    fn run_collect_phase<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        asts: Vec<(DefId, ast::Module)>,
    ) -> bool {
        let mut collector = Collector::new(ctx);
        for (mod_id, ast) in asts {
            collector.collect_ast(mod_id, &ast);
        }
        Self::report_diagnostics_if_errors(collector.context())
    }

    fn run_import_phase<'a>(&self, ctx: &mut SemaContext<'a>) -> bool {
        let mut import_resolver = ImportResolver::new(ctx);
        import_resolver.resolve_all();
        Self::report_diagnostics_if_errors(import_resolver.context())
    }

    fn run_type_resolution_phase<'a>(&self, ctx: &mut SemaContext<'a>) -> bool {
        let mut type_resolver = TypeResolver::new(ctx);
        type_resolver.resolve_all();
        if !Self::report_diagnostics_if_errors(type_resolver.context()) {
            return false;
        }

        let ctx = type_resolver.into_context();
        lint_docs(ctx);
        true
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

fn span_contains(outer: Span, inner: Span) -> bool {
    outer.file == inner.file && outer.start <= inner.start && inner.end <= outer.end
}
