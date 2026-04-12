mod body;
mod entry;
mod lints;
mod reuse;
mod structure;
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
use kernc_flow::FlowLoweringHints;
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
    pub(super) flow_lowering_hints: FlowLoweringHints,
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
            return Some(self.finalize_imported_structure_artifact(
                input_file,
                source_overrides,
                self.imported_structure_from_typed(&structure),
            ));
        }

        if let Some(imported) =
            self.cached_imported_structure_artifact(input_file, source_overrides)
        {
            return Some(self.finalize_imported_structure_artifact(
                input_file,
                source_overrides,
                imported,
            ));
        }

        if let Some(collected) =
            self.cached_collected_structure_artifact(input_file, source_overrides)
        {
            return self.build_imported_structure(&collected).map(|imported| {
                self.finalize_imported_structure_artifact(input_file, source_overrides, imported)
            });
        }

        let mut session = Session::new();
        session.apply_options(&self.options);
        self.try_analyze_imported_structure(session, input_file, source_overrides)
            .ok()
            .map(|imported| {
                self.finalize_imported_structure_artifact(input_file, source_overrides, imported)
            })
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
