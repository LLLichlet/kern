mod body;
mod entry;
mod lints;
mod reuse;
mod structure;
mod surface;

pub(in crate::compiler) use self::reuse::module_analysis_path_from_source;
use self::reuse::{
    classify_function_body_decl_changes, module_analysis_path, module_file_id,
    module_source_changed, modules_match_ignoring_body_only, normalize_driver_path,
    rebind_module_defs,
};
use super::completion::CompletionModel;
use super::flow::FlowModel;
use super::signature::SignatureModel;
use super::{
    AnalysisArtifact, AnalysisCall, AnalysisCallKind, AnalysisDefinitionLink, AnalysisHover,
    AnalysisNavigationArtifact, AnalysisOutline, AnalysisReference, AnalysisReport,
    AnalysisSemanticEntry, AnalysisSemanticKind, AnalysisSemanticRole, AnalysisSpanReplacement,
    AnalysisSurfaceArtifact, AnalysisSymbol, AnalysisSymbolKind, AnalysisUnusedBinding,
    AnalysisUnusedBindingKind, AnalysisUnusedItem, AnalysisUnusedItemKind, Canceled,
    CancellationToken, CollectedStructureArtifact, CompileStructureArtifact, CompilerDriver,
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
pub(in crate::compiler) struct FunctionBodyReusePlan {
    worklist: Vec<(DefId, ScopeId)>,
    replaced_spans: Vec<AnalysisSpanReplacement>,
}

pub(super) struct LoadedAstArtifact {
    pub(in crate::compiler) asts: Vec<(DefId, ast::Module)>,
    pub(super) phase_timings: Vec<PhaseTiming>,
}

pub(in crate::compiler) struct BodyPipelineReport {
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
        cancellation: &CancellationToken,
    ) -> Result<AnalysisReport, Canceled> {
        cancellation.check()?;
        let mut session = Session::new();
        session.apply_options(&self.options);

        let report = match self.try_analyze_structure_cancelable(
            session,
            input_file,
            source_overrides,
            cancellation,
        )? {
            Ok(structure) => self.analyze_report_from_structure(&structure, cancellation)?,
            Err(session) => AnalysisReport {
                session: *session,
                succeeded: false,
            },
        };
        cancellation.check()?;
        Ok(report)
    }

    pub fn analyze_artifact(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
        cancellation: &CancellationToken,
    ) -> Result<AnalysisArtifact, Canceled> {
        cancellation.check()?;
        let mut session = Session::new();
        session.apply_options(&self.options);

        let structure = match self.try_analyze_structure_cancelable(
            session,
            input_file,
            source_overrides,
            cancellation,
        )? {
            Ok(structure) => structure,
            Err(session) => return Ok(self.empty_analysis_artifact(*session)),
        };
        cancellation.check()?;
        self.analyze_artifact_from_structure(&structure, cancellation)
    }

    pub fn analyze_navigation_artifact(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
        cancellation: &CancellationToken,
    ) -> Result<AnalysisNavigationArtifact, Canceled> {
        cancellation.check()?;
        let mut session = Session::new();
        session.apply_options(&self.options);

        let structure = match self.try_analyze_structure_cancelable(
            session,
            input_file,
            source_overrides,
            cancellation,
        )? {
            Ok(structure) => structure,
            Err(session) => return Ok(self.empty_analysis_navigation_artifact(*session)),
        };
        cancellation.check()?;
        self.analyze_navigation_artifact_from_structure(&structure, cancellation)
    }

    pub fn analyze_imported_structure(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
        cancellation: &CancellationToken,
    ) -> Result<Option<ImportedStructureArtifact>, Canceled> {
        cancellation.check()?;
        if let Some(structure) = self.cached_structure_artifact(input_file, source_overrides) {
            return Ok(Some(self.finalize_imported_structure_artifact(
                input_file,
                source_overrides,
                self.imported_structure_from_typed(&structure),
            )));
        }

        cancellation.check()?;
        if let Some(imported) =
            self.cached_imported_structure_artifact(input_file, source_overrides)
        {
            return Ok(Some(self.finalize_imported_structure_artifact(
                input_file,
                source_overrides,
                imported,
            )));
        }

        cancellation.check()?;
        if let Some(collected) =
            self.cached_collected_structure_artifact(input_file, source_overrides)
        {
            let imported = self
                .build_imported_structure_cancelable(&collected, cancellation)?
                .map(|imported| {
                    self.finalize_imported_structure_artifact(
                        input_file,
                        source_overrides,
                        imported,
                    )
                });
            cancellation.check()?;
            return Ok(imported);
        }

        cancellation.check()?;
        let mut session = Session::new();
        session.apply_options(&self.options);
        let imported = self
            .try_analyze_imported_structure_cancelable(
                session,
                input_file,
                source_overrides,
                cancellation,
            )?
            .ok()
            .map(|imported| {
                self.finalize_imported_structure_artifact(input_file, source_overrides, imported)
            });
        cancellation.check()?;
        Ok(imported)
    }

    pub fn analyze_surface(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
        cancellation: &CancellationToken,
    ) -> Result<Option<AnalysisSurfaceArtifact>, Canceled> {
        cancellation.check()?;
        if let Some(structure) = self.cached_structure_artifact(input_file, source_overrides) {
            cancellation.check()?;
            return Ok(Some(self.surface_from_structure(&structure)));
        }

        cancellation.check()?;
        let Some(imported) =
            self.analyze_imported_structure(input_file, source_overrides, cancellation)?
        else {
            return Ok(None);
        };
        cancellation.check()?;
        Ok(Some(self.surface_from_imported(&imported)))
    }

    pub fn analyze_outline(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
        cancellation: &CancellationToken,
    ) -> Result<AnalysisOutline, Canceled> {
        cancellation.check()?;
        if let Some(structure) = self.cached_structure_artifact(input_file, source_overrides) {
            return Ok(self.analyze_outline_from_structure(&structure));
        }

        cancellation.check()?;
        if let Some(imported) =
            self.cached_imported_structure_artifact(input_file, source_overrides)
        {
            return Ok(self.analyze_outline_from_imported(&imported));
        }

        cancellation.check()?;
        if let Some(collected) =
            self.cached_collected_structure_artifact(input_file, source_overrides)
        {
            return Ok(self.analyze_outline_from_collected(&collected));
        }

        cancellation.check()?;
        let outline = match self.analyze_collected_structure_cancelable(
            input_file,
            source_overrides,
            cancellation,
        )? {
            Some(collected) => self.analyze_outline_from_collected(&collected),
            None => {
                let mut session = Session::new();
                session.apply_options(&self.options);
                AnalysisOutline {
                    session,
                    symbols: Vec::new(),
                }
            }
        };
        cancellation.check()?;
        Ok(outline)
    }

    pub fn parse_modules(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
        cancellation: &CancellationToken,
    ) -> Result<Option<ParsedModuleArtifact>, Canceled> {
        cancellation.check()?;
        if let Some(structure) = self.cached_structure_artifact(input_file, source_overrides) {
            cancellation.check()?;
            return Ok(Some(self.parsed_modules_from_structure(&structure)));
        }

        cancellation.check()?;
        if let Some(imported) =
            self.cached_imported_structure_artifact(input_file, source_overrides)
        {
            cancellation.check()?;
            return Ok(Some(self.parsed_modules_from_imported(&imported)));
        }

        cancellation.check()?;
        if let Some(collected) =
            self.cached_collected_structure_artifact(input_file, source_overrides)
        {
            cancellation.check()?;
            return Ok(Some(self.parsed_modules_from_collected(&collected)));
        }

        cancellation.check()?;
        let parsed = self
            .analyze_collected_structure_cancelable(input_file, source_overrides, cancellation)?
            .map(|collected| self.parsed_modules_from_collected(&collected));
        cancellation.check()?;
        Ok(parsed)
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
