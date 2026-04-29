use super::*;
use crate::compiler::ResolvedGlobalType;

impl CompilerDriver {
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
        let definition_links = self.collect_analysis_definition_links(&ctx);
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
            definition_links,
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

    pub fn analyze_report_from_structure(&self, structure: &StructureArtifact) -> AnalysisReport {
        let mut session = structure.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        let succeeded = self.run_body_pipeline(&mut ctx);
        drop(ctx);

        AnalysisReport { session, succeeded }
    }

    pub fn analyze_report_from_structure_and_parsed(
        &self,
        structure: &StructureArtifact,
        parsed: &ParsedModuleArtifact,
    ) -> Option<AnalysisReport> {
        let mut session = parsed.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        if !self.rebind_body_only_modules(&mut ctx, &structure.session, &structure.asts, parsed) {
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
        if !self.rebind_body_only_modules(&mut ctx, &structure.session, &structure.asts, parsed) {
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

    pub(in crate::compiler) fn run_body_pipeline<'a>(&self, ctx: &mut SemaContext<'a>) -> bool {
        self.run_body_pipeline_with_report(ctx).is_some()
    }

    pub(in crate::compiler) fn run_body_pipeline_with_report<'a>(
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

    pub(super) fn empty_analysis_artifact(&self, session: Session) -> AnalysisArtifact {
        AnalysisArtifact {
            session,
            succeeded: false,
            symbols: Vec::new(),
            references: Vec::new(),
            hovers: Vec::new(),
            definition_links: Vec::new(),
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

    pub(super) fn rebind_body_only_modules(
        &self,
        ctx: &mut SemaContext<'_>,
        clean_session: &Session,
        clean_asts: &[(DefId, ast::Module)],
        parsed: &ParsedModuleArtifact,
    ) -> bool {
        if clean_asts.len() != parsed.modules.len() {
            return false;
        }

        let Some(clean_modules) = self.index_clean_modules(&ctx.defs, clean_session, clean_asts)
        else {
            return false;
        };

        for parsed_module in &parsed.modules {
            let Some((module_id, clean_module)) = clean_modules.get(parsed_module.path.as_path())
            else {
                return false;
            };

            let clean_file_id = module_file_id(&ctx.defs, *module_id);
            let module_changed = module_source_changed(
                clean_session,
                clean_file_id,
                &parsed.session,
                parsed_module.file_id,
            );
            if module_changed && !modules_match_ignoring_body_only(clean_module, &parsed_module.ast)
            {
                return false;
            }

            if !rebind_module_defs(ctx, *module_id, parsed_module) {
                return false;
            }
        }

        true
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

    fn build_function_body_reuse_plan(
        &self,
        ctx: &SemaContext<'_>,
        clean_asts: &[(DefId, ast::Module)],
        parsed: &ParsedModuleArtifact,
    ) -> Option<FunctionBodyReusePlan> {
        let clean_modules = self.index_clean_modules(&ctx.defs, ctx.sess, clean_asts)?;

        let mut worklist = Vec::new();
        let mut replaced_spans = Vec::new();

        for parsed_module in &parsed.modules {
            let &(module_id, clean_module) = clean_modules.get(parsed_module.path.as_path())?;

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
