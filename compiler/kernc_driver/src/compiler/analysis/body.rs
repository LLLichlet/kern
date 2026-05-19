//! Full body analysis artifact construction.
//!
//! This module starts from a structure artifact, type-checks bodies, collects
//! flow/completion/reference/lint data, and returns the complete analysis model
//! used by editor features.

use super::*;
use crate::compiler::ResolvedGlobalType;

impl CompilerDriver {
    pub fn analyze_artifact_from_structure(
        &self,
        structure: &StructureArtifact,
        cancellation: &CancellationToken,
    ) -> Result<AnalysisArtifact, Canceled> {
        cancellation.check()?;
        let mut session = structure.session.clone();
        let analysis_asts = structure.asts.clone();

        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        cancellation.check()?;
        let succeeded = self.run_body_pipeline_cancelable(&mut ctx, cancellation)?;
        cancellation.check()?;
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
        let type_hints = self.collect_analysis_type_hints(&ctx, &analysis_asts);
        let definition_links = self.collect_analysis_definition_links(&ctx);
        let semantic_entries = self.collect_analysis_semantic_entries(&symbols, &ctx, &references);
        let completion_model = self.collect_completion_model(&mut ctx, &analysis_asts);
        let signature_model = self.collect_signature_model(&mut ctx, &analysis_asts);
        let flow_model = self.collect_flow_model_cancelable(&ctx, &references, cancellation)?;
        cancellation.check()?;
        let calls =
            self.collect_analysis_calls(&mut ctx, &analysis_asts, &semantic_entries, &flow_model);
        cancellation.check()?;
        let unused_items = self.collect_unused_private_items_cancelable(
            &ctx,
            &raw_references,
            &flow_model,
            cancellation,
        )?;
        cancellation.check()?;
        let unused_bindings =
            self.collect_unused_bindings_cancelable(&ctx, &flow_model, cancellation)?;
        cancellation.check()?;
        let dead_stores =
            self.collect_dead_stores_cancelable(&ctx, &raw_references, &flow_model, cancellation)?;
        let trait_impl_stubs = if structure.trait_impl_stubs.is_empty() {
            self.collect_trait_impl_stubs(&ctx)
        } else {
            structure.trait_impl_stubs.clone()
        };
        let resolved_globals = self.collect_resolved_globals(&ctx);
        drop(ctx);
        cancellation.check()?;

        Ok(AnalysisArtifact {
            session,
            succeeded,
            symbols,
            references,
            hovers,
            type_hints,
            definition_links,
            semantic_entries,
            calls,
            asts: analysis_asts,
            resolved_globals,
            completion_model,
            signature_model,
            flow_model,
            unused_items,
            unused_bindings,
            dead_stores,
            trait_impl_stubs,
        })
    }

    pub fn analyze_navigation_artifact_from_structure(
        &self,
        structure: &StructureArtifact,
        cancellation: &CancellationToken,
    ) -> Result<AnalysisNavigationArtifact, Canceled> {
        cancellation.check()?;
        let mut session = structure.session.clone();
        let analysis_asts = structure.asts.clone();

        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        cancellation.check()?;
        let succeeded = self.run_navigation_pipeline_cancelable(&mut ctx, cancellation)?;
        cancellation.check()?;
        let symbols = self.collect_analysis_symbols(&ctx, &analysis_asts);
        let references = ctx
            .identifier_references()
            .iter()
            .map(|(reference_span, definition_span)| AnalysisReference {
                reference_span: *reference_span,
                definition_span: *definition_span,
            })
            .collect::<Vec<_>>();
        let hovers = self.collect_analysis_hovers(&ctx);
        let type_hints = self.collect_analysis_type_hints(&ctx, &analysis_asts);
        let definition_links = self.collect_analysis_definition_links(&ctx);
        let semantic_entries = self.collect_analysis_semantic_entries(&symbols, &ctx, &references);
        let flow_model = self.collect_flow_model_cancelable(&ctx, &references, cancellation)?;
        cancellation.check()?;
        let calls =
            self.collect_analysis_calls(&mut ctx, &analysis_asts, &semantic_entries, &flow_model);
        drop(ctx);
        cancellation.check()?;

        Ok(AnalysisNavigationArtifact {
            session,
            succeeded,
            symbols,
            references,
            hovers,
            type_hints,
            definition_links,
            semantic_entries,
            calls,
        })
    }

    pub fn analyze_semantic_artifact_from_structure(
        &self,
        structure: &StructureArtifact,
        cancellation: &CancellationToken,
    ) -> Result<AnalysisSemanticArtifact, Canceled> {
        cancellation.check()?;
        let mut session = structure.session.clone();
        let analysis_asts = structure.asts.clone();

        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        cancellation.check()?;
        let succeeded = self.run_navigation_pipeline_cancelable(&mut ctx, cancellation)?;
        cancellation.check()?;
        let symbols = self.collect_analysis_symbols(&ctx, &analysis_asts);
        let references = ctx
            .identifier_references()
            .iter()
            .map(|(reference_span, definition_span)| AnalysisReference {
                reference_span: *reference_span,
                definition_span: *definition_span,
            })
            .collect::<Vec<_>>();
        let hovers = self.collect_analysis_hovers(&ctx);
        let type_hints = self.collect_analysis_type_hints(&ctx, &analysis_asts);
        let semantic_entries = self.collect_analysis_semantic_entries(&symbols, &ctx, &references);
        drop(ctx);
        cancellation.check()?;

        Ok(AnalysisSemanticArtifact {
            session,
            succeeded,
            symbols,
            references,
            hovers,
            type_hints,
            semantic_entries,
        })
    }

    pub fn analyze_semantic_artifact_from_structure_and_parsed(
        &self,
        structure: &StructureArtifact,
        parsed: &ParsedModuleArtifact,
        cancellation: &CancellationToken,
    ) -> Result<Option<AnalysisSemanticArtifact>, Canceled> {
        cancellation.check()?;
        let mut session = parsed.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        cancellation.check()?;
        if !self.rebind_body_only_modules_cancelable(
            &mut ctx,
            &structure.session,
            &structure.asts,
            parsed,
            cancellation,
        )? {
            return Ok(None);
        }
        let Some(analysis_asts) =
            self.reused_asts_cancelable(&structure.asts, parsed, cancellation)?
        else {
            return Ok(None);
        };
        cancellation.check()?;
        let succeeded = self.run_navigation_pipeline_cancelable(&mut ctx, cancellation)?;
        cancellation.check()?;
        let symbols = self.collect_analysis_symbols(&ctx, &analysis_asts);
        let references = ctx
            .identifier_references()
            .iter()
            .map(|(reference_span, definition_span)| AnalysisReference {
                reference_span: *reference_span,
                definition_span: *definition_span,
            })
            .collect::<Vec<_>>();
        let hovers = self.collect_analysis_hovers(&ctx);
        let type_hints = self.collect_analysis_type_hints(&ctx, &analysis_asts);
        let semantic_entries = self.collect_analysis_semantic_entries(&symbols, &ctx, &references);
        drop(ctx);
        cancellation.check()?;

        Ok(Some(AnalysisSemanticArtifact {
            session,
            succeeded,
            symbols,
            references,
            hovers,
            type_hints,
            semantic_entries,
        }))
    }

    pub fn analyze_semantic_token_artifact_from_structure(
        &self,
        structure: &StructureArtifact,
        target_path: &Path,
        cancellation: &CancellationToken,
    ) -> Result<AnalysisSemanticTokenArtifact, Canceled> {
        cancellation.check()?;
        let mut session = structure.session.clone();
        let analysis_asts = structure.asts.clone();

        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        cancellation.check()?;
        let succeeded =
            self.run_targeted_navigation_pipeline_cancelable(&mut ctx, target_path, cancellation)?;
        cancellation.check()?;
        let symbols = self.collect_analysis_symbols(&ctx, &analysis_asts);
        let references = ctx
            .identifier_references()
            .iter()
            .map(|(reference_span, definition_span)| AnalysisReference {
                reference_span: *reference_span,
                definition_span: *definition_span,
            })
            .collect::<Vec<_>>();
        let hovers = self.collect_analysis_hovers(&ctx);
        let semantic_entries = self.collect_analysis_semantic_entries(&symbols, &ctx, &references);
        drop(ctx);
        cancellation.check()?;

        Ok(AnalysisSemanticTokenArtifact {
            session,
            succeeded,
            symbols,
            references,
            hovers,
            semantic_entries,
        })
    }

    pub fn analyze_semantic_token_artifact_from_structure_and_parsed(
        &self,
        structure: &StructureArtifact,
        parsed: &ParsedModuleArtifact,
        target_path: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Option<AnalysisSemanticTokenArtifact>, Canceled> {
        cancellation.check()?;
        let mut session = parsed.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        cancellation.check()?;
        if !self.rebind_body_only_modules_cancelable(
            &mut ctx,
            &structure.session,
            &structure.asts,
            parsed,
            cancellation,
        )? {
            return Ok(None);
        }
        let Some(analysis_asts) =
            self.reused_asts_cancelable(&structure.asts, parsed, cancellation)?
        else {
            return Ok(None);
        };
        cancellation.check()?;
        let succeeded =
            self.run_targeted_navigation_pipeline_cancelable(&mut ctx, target_path, cancellation)?;
        cancellation.check()?;
        let symbols = self.collect_analysis_symbols(&ctx, &analysis_asts);
        let references = ctx
            .identifier_references()
            .iter()
            .map(|(reference_span, definition_span)| AnalysisReference {
                reference_span: *reference_span,
                definition_span: *definition_span,
            })
            .collect::<Vec<_>>();
        let hovers = self.collect_analysis_hovers(&ctx);
        let semantic_entries = self.collect_analysis_semantic_entries(&symbols, &ctx, &references);
        drop(ctx);
        cancellation.check()?;

        Ok(Some(AnalysisSemanticTokenArtifact {
            session,
            succeeded,
            symbols,
            references,
            hovers,
            semantic_entries,
        }))
    }

    pub fn analyze_report_from_structure(
        &self,
        structure: &StructureArtifact,
        cancellation: &CancellationToken,
    ) -> Result<AnalysisReport, Canceled> {
        cancellation.check()?;
        let mut session = structure.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        cancellation.check()?;
        let succeeded = self.run_body_pipeline_cancelable(&mut ctx, cancellation)?;
        drop(ctx);
        cancellation.check()?;

        Ok(AnalysisReport { session, succeeded })
    }

    pub fn analyze_structure_report(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> AnalysisReport {
        let mut session = Session::new();
        session.apply_options(&self.options);

        match self.try_analyze_structure(session, input_file, source_overrides) {
            Ok(structure) => AnalysisReport {
                session: structure.session,
                succeeded: true,
            },
            Err(session) => AnalysisReport {
                session: *session,
                succeeded: false,
            },
        }
    }

    pub fn parse_report(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> AnalysisReport {
        let mut session = Session::new();
        session.apply_options(&self.options);

        match self.try_parse_modules(session, input_file, source_overrides) {
            Ok(parsed) => AnalysisReport {
                session: parsed.session,
                succeeded: true,
            },
            Err(session) => AnalysisReport {
                session: *session,
                succeeded: false,
            },
        }
    }

    pub fn analyze_report_from_structure_and_parsed(
        &self,
        structure: &StructureArtifact,
        parsed: &ParsedModuleArtifact,
        cancellation: &CancellationToken,
    ) -> Result<Option<AnalysisReport>, Canceled> {
        cancellation.check()?;
        let mut session = parsed.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        cancellation.check()?;
        if !self.rebind_body_only_modules_cancelable(
            &mut ctx,
            &structure.session,
            &structure.asts,
            parsed,
            cancellation,
        )? {
            return Ok(None);
        }
        let succeeded = self.run_body_pipeline_cancelable(&mut ctx, cancellation)?;
        drop(ctx);
        cancellation.check()?;

        Ok(Some(AnalysisReport { session, succeeded }))
    }

    pub fn parsed_modules_match_structure_body_only(
        &self,
        structure: &StructureArtifact,
        parsed: &ParsedModuleArtifact,
    ) -> bool {
        let clean = structure
            .asts
            .iter()
            .map(|(_, module)| module)
            .collect::<Vec<_>>();
        self.modules_match_body_only(&structure.session, clean, parsed)
    }

    pub fn parsed_modules_match_body_only(
        &self,
        clean: &ParsedModuleArtifact,
        dirty: &ParsedModuleArtifact,
    ) -> bool {
        if clean.modules.len() != dirty.modules.len() {
            return false;
        }

        let dirty_modules = self.index_parsed_modules(dirty);
        for clean_module in &clean.modules {
            let dirty_module = dirty_modules
                .get(clean_module.path.as_path())
                .or_else(|| dirty_modules.get(Path::new(clean_module.ast.path.as_str())));
            let Some(dirty_module) = dirty_module else {
                return false;
            };

            let module_changed = clean_module.ast != dirty_module.ast;
            if module_changed
                && !modules_match_ignoring_body_only(
                    &clean.session,
                    &clean_module.ast,
                    &dirty.session,
                    &dirty_module.ast,
                )
            {
                return false;
            }
        }

        true
    }

    fn modules_match_body_only(
        &self,
        clean_session: &Session,
        clean: Vec<&ast::Module>,
        dirty: &ParsedModuleArtifact,
    ) -> bool {
        if clean.len() != dirty.modules.len() {
            return false;
        }

        let dirty_modules = self.index_parsed_modules(dirty);
        for clean_module in clean {
            let clean_path = normalize_driver_path(Path::new(clean_module.path.as_str()));
            let dirty_module = dirty_modules
                .get(clean_path.as_path())
                .or_else(|| dirty_modules.get(Path::new(clean_module.path.as_str())));
            let Some(dirty_module) = dirty_module else {
                return false;
            };

            let module_changed = clean_module != &dirty_module.ast;
            if module_changed
                && !modules_match_ignoring_body_only(
                    clean_session,
                    clean_module,
                    &dirty.session,
                    &dirty_module.ast,
                )
            {
                return false;
            }
        }

        true
    }

    pub fn analyze_report_with_function_body_reuse(
        &self,
        clean_artifact: &AnalysisArtifact,
        structure: &StructureArtifact,
        parsed: &ParsedModuleArtifact,
        cancellation: &CancellationToken,
    ) -> Result<Option<TargetedAnalysisReport>, Canceled> {
        cancellation.check()?;
        let mut session = parsed.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        self.apply_resolved_globals(&mut ctx, &clean_artifact.resolved_globals);
        cancellation.check()?;

        let Some(plan) = self.build_function_body_reuse_plan_cancelable(
            &ctx,
            &clean_artifact.asts,
            parsed,
            cancellation,
        )?
        else {
            return Ok(None);
        };
        if plan.worklist.is_empty() {
            return Ok(None);
        }
        if !self.rebind_body_only_modules_cancelable(
            &mut ctx,
            &structure.session,
            &structure.asts,
            parsed,
            cancellation,
        )? {
            return Ok(None);
        }
        cancellation.check()?;

        let mut typeck = TypeckDriver::new(&mut ctx);
        typeck.check_body_worklist_cancelable(&plan.worklist, cancellation)?;
        let ctx = typeck.into_context();
        cancellation.check()?;
        let references = self.merge_targeted_identifier_references(
            clean_artifact,
            &plan.replaced_spans,
            ctx.identifier_references(),
        );
        cancellation.check()?;
        let flow_model =
            self.collect_flow_model_from_raw_references_cancelable(ctx, &references, cancellation)?;
        cancellation.check()?;
        self.emit_unused_private_item_warnings_cancelable(
            ctx,
            &references,
            &flow_model,
            cancellation,
        )?;
        cancellation.check()?;
        self.emit_unused_binding_warnings_cancelable(ctx, &flow_model, cancellation)?;
        cancellation.check()?;
        self.emit_dead_store_warnings_cancelable(ctx, &references, &flow_model, cancellation)?;
        cancellation.check()?;
        let succeeded = Self::report_diagnostics_if_errors(ctx);

        Ok(Some(TargetedAnalysisReport {
            report: AnalysisReport { session, succeeded },
            replaced_spans: plan.replaced_spans,
        }))
    }

    pub(in crate::compiler) fn run_body_pipeline<'a>(&self, ctx: &mut SemaContext<'a>) -> bool {
        self.run_body_pipeline_with_report(ctx).is_some()
    }

    pub(in crate::compiler) fn run_body_pipeline_cancelable<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        cancellation: &CancellationToken,
    ) -> Result<bool, Canceled> {
        Ok(self
            .run_body_pipeline_with_report_cancelable(ctx, cancellation)?
            .is_some())
    }

    pub(in crate::compiler) fn run_navigation_pipeline_cancelable<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        cancellation: &CancellationToken,
    ) -> Result<bool, Canceled> {
        let mut typeck = TypeckDriver::new(ctx);
        let (globals, worklist) = typeck.worklists();
        cancellation.check()?;
        typeck.resolve_global_worklist_cancelable(&globals, cancellation)?;
        typeck.check_body_worklist_cancelable(&worklist, cancellation)?;
        let ctx = typeck.into_context();
        cancellation.check()?;
        Ok(Self::report_diagnostics_if_errors(ctx))
    }

    pub(in crate::compiler) fn run_targeted_navigation_pipeline_cancelable<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        target_path: &Path,
        cancellation: &CancellationToken,
    ) -> Result<bool, Canceled> {
        let target_path = normalize_driver_path(target_path);
        let mut typeck = TypeckDriver::new(ctx);
        let globals = typeck.global_worklist();
        let worklist = typeck.body_worklist_for_file(&target_path);
        cancellation.check()?;
        typeck.resolve_global_worklist_cancelable(&globals, cancellation)?;
        typeck.check_body_worklist_cancelable(&worklist, cancellation)?;
        let ctx = typeck.into_context();
        cancellation.check()?;
        Ok(Self::report_diagnostics_if_errors(ctx))
    }

    pub(in crate::compiler) fn run_body_pipeline_with_report<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
    ) -> Option<BodyPipelineReport> {
        self.run_body_pipeline_with_report_uncancelable(ctx)
    }

    fn run_body_pipeline_with_report_uncancelable<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
    ) -> Option<BodyPipelineReport> {
        let mut phase_timings = Vec::new();
        let mut typeck = TypeckDriver::new(ctx);
        let (globals, worklist) = typeck.worklists();
        measure_body_phase(&mut phase_timings, "typeck_globals", || {
            typeck.resolve_global_worklist(&globals);
        });
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
            self.emit_unused_private_item_warnings_with_reachability_cancelable(
                ctx,
                &reachability,
                &CancellationToken::new(),
            )
            .expect("fresh cancellation token cannot be canceled");
        });
        measure_body_phase(&mut phase_timings, "warn_unused_bindings", || {
            self.emit_unused_binding_warnings_cancelable(
                ctx,
                &flow_model,
                &CancellationToken::new(),
            )
            .expect("fresh cancellation token cannot be canceled");
        });
        measure_body_phase(&mut phase_timings, "warn_dead_stores", || {
            self.emit_dead_store_warnings_cancelable(
                ctx,
                &references,
                &flow_model,
                &CancellationToken::new(),
            )
            .expect("fresh cancellation token cannot be canceled");
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

    pub(in crate::compiler) fn run_body_pipeline_with_report_cancelable<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        cancellation: &CancellationToken,
    ) -> Result<Option<BodyPipelineReport>, Canceled> {
        let mut phase_timings = Vec::new();
        let mut typeck = TypeckDriver::new(ctx);
        let (globals, worklist) = typeck.worklists();
        cancellation.check()?;
        measure_body_phase(&mut phase_timings, "typeck_globals", || {
            typeck.resolve_global_worklist_cancelable(&globals, cancellation)
        })?;
        let _ = measure_body_phase(&mut phase_timings, "typeck_bodies", || {
            typeck.check_body_worklist_cancelable(&worklist, cancellation)
        })?;
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
        cancellation.check()?;
        if !Self::report_diagnostics_if_errors(ctx) {
            return Ok(None);
        }
        let references = ctx.identifier_references().to_vec();
        cancellation.check()?;
        let flow_model = measure_body_phase(&mut phase_timings, "flow", || {
            self.collect_compile_flow_model_from_raw_references_cancelable(
                ctx,
                &references,
                cancellation,
            )
        })?;
        cancellation.check()?;
        phase_timings.extend(flow_model.phase_timings().iter().copied().map(|timing| {
            PhaseTiming {
                name: timing.name,
                duration: timing.duration,
            }
        }));
        let flow_lowering_hints = flow_model.lowering_hints(ctx);
        cancellation.check()?;
        let reachability = self.compute_module_item_reachability_cancelable(
            ctx,
            &references,
            &flow_model,
            cancellation,
        )?;
        let lowered_module_items = reachability.lowered_reachable.clone();
        cancellation.check()?;
        measure_body_phase(&mut phase_timings, "warn_unused_items", || {
            self.emit_unused_private_item_warnings_with_reachability_cancelable(
                ctx,
                &reachability,
                cancellation,
            )
        })?;
        cancellation.check()?;
        measure_body_phase(&mut phase_timings, "warn_unused_bindings", || {
            self.emit_unused_binding_warnings_cancelable(ctx, &flow_model, cancellation)
        })?;
        cancellation.check()?;
        measure_body_phase(&mut phase_timings, "warn_dead_stores", || {
            self.emit_dead_store_warnings_cancelable(ctx, &references, &flow_model, cancellation)
        })?;
        cancellation.check()?;

        let mut linkage_checker = LinkageChecker::new(ctx);
        measure_body_phase(&mut phase_timings, "linkage", || {
            linkage_checker.check_all_cancelable(cancellation)
        })?;
        let ctx = linkage_checker.context();
        cancellation.check()?;
        if !Self::report_diagnostics_if_errors(ctx) {
            return Ok(None);
        }

        Ok(Some(BodyPipelineReport {
            flow_lowering_hints,
            lowered_module_items,
            phase_timings,
        }))
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

    pub(super) fn empty_analysis_artifact(&self, session: Session) -> AnalysisArtifact {
        AnalysisArtifact {
            session,
            succeeded: false,
            symbols: Vec::new(),
            references: Vec::new(),
            hovers: Vec::new(),
            type_hints: Vec::new(),
            definition_links: Vec::new(),
            semantic_entries: Vec::new(),
            calls: Vec::new(),
            asts: Vec::new(),
            resolved_globals: Vec::new(),
            completion_model: CompletionModel::default(),
            signature_model: SignatureModel::default(),
            flow_model: FlowModel::default(),
            unused_items: Vec::new(),
            unused_bindings: Vec::new(),
            dead_stores: Vec::new(),
            trait_impl_stubs: Vec::new(),
        }
    }

    pub(super) fn empty_analysis_navigation_artifact(
        &self,
        session: Session,
    ) -> AnalysisNavigationArtifact {
        AnalysisNavigationArtifact {
            session,
            succeeded: false,
            symbols: Vec::new(),
            references: Vec::new(),
            hovers: Vec::new(),
            type_hints: Vec::new(),
            definition_links: Vec::new(),
            semantic_entries: Vec::new(),
            calls: Vec::new(),
        }
    }

    pub(super) fn empty_analysis_semantic_artifact(
        &self,
        session: Session,
    ) -> AnalysisSemanticArtifact {
        AnalysisSemanticArtifact {
            session,
            succeeded: false,
            symbols: Vec::new(),
            references: Vec::new(),
            hovers: Vec::new(),
            type_hints: Vec::new(),
            semantic_entries: Vec::new(),
        }
    }

    pub(super) fn empty_analysis_semantic_token_artifact(
        &self,
        session: Session,
    ) -> AnalysisSemanticTokenArtifact {
        AnalysisSemanticTokenArtifact {
            session,
            succeeded: false,
            symbols: Vec::new(),
            references: Vec::new(),
            hovers: Vec::new(),
            semantic_entries: Vec::new(),
        }
    }

    fn collect_flow_model_cancelable(
        &self,
        ctx: &SemaContext<'_>,
        references: &[AnalysisReference],
        cancellation: &CancellationToken,
    ) -> Result<FlowModel, Canceled> {
        let raw_references = references
            .iter()
            .map(|reference| (reference.reference_span, reference.definition_span))
            .collect::<Vec<_>>();
        self.collect_flow_model_from_raw_references_cancelable(ctx, &raw_references, cancellation)
    }

    fn collect_flow_model_from_raw_references_cancelable(
        &self,
        ctx: &SemaContext<'_>,
        references: &[(Span, Span)],
        cancellation: &CancellationToken,
    ) -> Result<FlowModel, Canceled> {
        let module_item_definition_spans = self.module_item_definition_spans(ctx);
        FlowModel::collect_cancelable(ctx, &module_item_definition_spans, references, cancellation)
    }

    fn collect_compile_flow_model_from_raw_references(
        &self,
        ctx: &SemaContext<'_>,
        references: &[(Span, Span)],
    ) -> FlowModel {
        let module_item_definition_spans = self.module_item_definition_spans(ctx);
        FlowModel::collect_for_compile(ctx, &module_item_definition_spans, references)
    }

    fn collect_compile_flow_model_from_raw_references_cancelable(
        &self,
        ctx: &SemaContext<'_>,
        references: &[(Span, Span)],
        cancellation: &CancellationToken,
    ) -> Result<FlowModel, Canceled> {
        let module_item_definition_spans = self.module_item_definition_spans(ctx);
        FlowModel::collect_for_compile_cancelable(
            ctx,
            &module_item_definition_spans,
            references,
            cancellation,
        )
    }

    pub(super) fn rebind_body_only_modules_cancelable(
        &self,
        ctx: &mut SemaContext<'_>,
        clean_session: &Session,
        clean_asts: &[(DefId, ast::Module)],
        parsed: &ParsedModuleArtifact,
        cancellation: &CancellationToken,
    ) -> Result<bool, Canceled> {
        if clean_asts.len() != parsed.modules.len() {
            return Ok(false);
        }

        let Some(clean_modules) = self.index_clean_modules_cancelable(
            &ctx.defs,
            clean_session,
            clean_asts,
            cancellation,
        )?
        else {
            return Ok(false);
        };

        for parsed_module in &parsed.modules {
            cancellation.check()?;
            let Some((module_id, clean_module)) = clean_modules.get(parsed_module.path.as_path())
            else {
                return Ok(false);
            };

            let clean_file_id = module_file_id(&ctx.defs, *module_id);
            let module_changed = module_source_changed(
                clean_session,
                clean_file_id,
                &parsed.session,
                parsed_module.file_id,
            );
            if module_changed
                && !modules_match_ignoring_body_only(
                    clean_session,
                    clean_module,
                    &parsed.session,
                    &parsed_module.ast,
                )
            {
                return Ok(false);
            }

            if !rebind_module_defs(ctx, *module_id, parsed_module) {
                return Ok(false);
            }
        }

        Ok(true)
    }

    fn apply_resolved_globals(&self, ctx: &mut SemaContext<'_>, globals: &[ResolvedGlobalType]) {
        for global in globals {
            let _ = ctx
                .scopes
                .update_type_in_scope(global.scope_id, global.name, global.ty);
        }
    }

    fn collect_resolved_globals(&self, ctx: &SemaContext<'_>) -> Vec<ResolvedGlobalType> {
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

                globals.push(ResolvedGlobalType {
                    scope_id: module.scope_id,
                    name: global.name,
                    ty: info.type_id,
                });
            }
        }

        globals
    }

    fn build_function_body_reuse_plan_cancelable(
        &self,
        ctx: &SemaContext<'_>,
        clean_asts: &[(DefId, ast::Module)],
        parsed: &ParsedModuleArtifact,
        cancellation: &CancellationToken,
    ) -> Result<Option<FunctionBodyReusePlan>, Canceled> {
        let Some(clean_modules) =
            self.index_clean_modules_cancelable(&ctx.defs, ctx.sess, clean_asts, cancellation)?
        else {
            return Ok(None);
        };

        let mut worklist = Vec::new();
        let mut replaced_spans = Vec::new();

        for parsed_module in &parsed.modules {
            cancellation.check()?;
            let Some(&(module_id, clean_module)) = clean_modules.get(parsed_module.path.as_path())
            else {
                return Ok(None);
            };

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
                _ => return Ok(None),
            };
            let module_items = match &ctx.defs[module_id.0 as usize] {
                kernc_sema::def::Def::Module(module) => module.items.clone(),
                _ => return Ok(None),
            };

            let mut item_iter = module_items.iter();
            if !classify_function_body_decl_changes(
                ctx.sess,
                clean_module,
                &parsed.session,
                &parsed_module.ast,
                &mut item_iter,
                module_scope,
                &mut worklist,
                &mut replaced_spans,
            ) {
                return Ok(None);
            }
            if item_iter.next().is_some() {
                return Ok(None);
            }
        }

        Ok(Some(FunctionBodyReusePlan {
            worklist,
            replaced_spans,
        }))
    }
}
