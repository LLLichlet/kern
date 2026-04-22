use super::*;
use crate::compiler::completion;

impl CompilerDriver {
    pub(super) fn try_parse_modules_with_frontend_cache(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<ParsedModuleArtifact> {
        let mut session = Session::new();
        session.apply_options(&self.options);
        self.try_parse_modules(session, input_file, source_overrides)
            .ok()
    }

    pub(super) fn analyze_outline_from_structure(
        &self,
        structure: &StructureArtifact,
    ) -> AnalysisOutline {
        AnalysisOutline {
            session: structure.session.clone(),
            symbols: structure.symbols.clone(),
        }
    }

    pub(super) fn analyze_outline_from_imported(
        &self,
        imported: &ImportedStructureArtifact,
    ) -> AnalysisOutline {
        AnalysisOutline {
            session: imported.session.clone(),
            symbols: imported.symbols.clone(),
        }
    }

    pub(super) fn analyze_outline_from_collected(
        &self,
        collected: &CollectedStructureArtifact,
    ) -> AnalysisOutline {
        AnalysisOutline {
            session: collected.session.clone(),
            symbols: collected.symbols.clone(),
        }
    }

    pub(super) fn surface_from_structure(
        &self,
        structure: &StructureArtifact,
    ) -> AnalysisSurfaceArtifact {
        AnalysisSurfaceArtifact {
            session: structure.session.clone(),
            symbols: structure.symbols.clone(),
            completion_model: structure.completion_model.clone(),
        }
    }

    pub(super) fn surface_from_imported(
        &self,
        imported: &ImportedStructureArtifact,
    ) -> AnalysisSurfaceArtifact {
        AnalysisSurfaceArtifact {
            session: imported.session.clone(),
            symbols: imported.symbols.clone(),
            completion_model: imported.completion_model.clone(),
        }
    }

    pub(super) fn analyze_outline_from_parsed(
        &self,
        parsed: &ParsedModuleArtifact,
    ) -> AnalysisOutline {
        AnalysisOutline {
            session: parsed.session.clone(),
            symbols: self.collect_parsed_module_symbols(&parsed.session, &parsed.modules),
        }
    }

    pub(super) fn parsed_modules_from_structure(
        &self,
        structure: &StructureArtifact,
    ) -> ParsedModuleArtifact {
        self.parsed_modules_from_snapshot(
            structure.session.clone(),
            structure.asts.clone(),
            structure.snapshot.clone(),
        )
    }

    pub(super) fn parsed_modules_from_imported(
        &self,
        imported: &ImportedStructureArtifact,
    ) -> ParsedModuleArtifact {
        self.parsed_modules_from_snapshot(
            imported.session.clone(),
            imported.asts.clone(),
            imported.snapshot.clone(),
        )
    }

    pub(super) fn parsed_modules_from_collected(
        &self,
        collected: &CollectedStructureArtifact,
    ) -> ParsedModuleArtifact {
        self.parsed_modules_from_snapshot(
            collected.session.clone(),
            collected.asts.clone(),
            collected.snapshot.clone(),
        )
    }

    pub(super) fn parsed_modules_from_snapshot(
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
                    body_regions: completion::module_body_completion_regions(ast),
                    ast: ast.clone(),
                }
            })
            .collect();
        drop(ctx);

        ParsedModuleArtifact { session, modules }
    }

    pub(in crate::compiler) fn build_sema_context<'a>(
        &self,
        session: &'a mut Session,
    ) -> SemaContext<'a> {
        let mut ctx = SemaContext::new(session);
        ctx.resolution.module_aliases = self.options.module_aliases.clone();
        ctx.resolution.module_interface_aliases = self.options.module_interface_aliases.clone();
        ctx.resolution.current_package_name = self
            .options
            .metadata_package_name
            .as_deref()
            .map(|name| ctx.intern(name));

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

    pub(in crate::compiler) fn try_analyze_structure(
        &self,
        mut session: Session,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Result<StructureArtifact, Box<Session>> {
        self.sync_source_overrides(source_overrides);
        if let Some(structure) = self.cached_structure_artifact(input_file, source_overrides) {
            return Ok(self.finalize_structure_artifact(input_file, source_overrides, structure));
        }
        if let Some(imported) =
            self.cached_imported_structure_artifact(input_file, source_overrides)
        {
            if let Some(structure) = self.build_typed_structure(&imported) {
                return Ok(self.finalize_structure_artifact(
                    input_file,
                    source_overrides,
                    structure,
                ));
            }

            let structure = self.compute_structure_artifact_into_session(&mut session, input_file);
            return structure
                .map(|structure| {
                    self.finalize_structure_artifact(input_file, source_overrides, structure)
                })
                .ok_or_else(|| Box::new(session));
        }
        let cache_key = self.structure_cache_key(input_file, source_overrides);
        match self.structure_artifacts.get_with(
            self.frontend.db(),
            "driver_structure_artifact",
            cache_key,
            || {
                Ok(self
                    .try_reuse_clean_typed_structure_artifact(input_file, source_overrides)
                    .or_else(|| {
                        self.compute_structure_artifact_into_session(&mut session, input_file)
                    }))
            },
        ) {
            Ok(Some(structure)) => {
                Ok(self.finalize_structure_artifact(input_file, source_overrides, structure))
            }
            Ok(None) => {
                let structure =
                    self.compute_structure_artifact_into_session(&mut session, input_file);
                structure
                    .map(|structure| {
                        self.finalize_structure_artifact(input_file, source_overrides, structure)
                    })
                    .ok_or_else(|| Box::new(session))
            }
            Err(_) => {
                let structure =
                    self.compute_structure_artifact_into_session(&mut session, input_file);
                structure
                    .map(|structure| {
                        self.finalize_structure_artifact(input_file, source_overrides, structure)
                    })
                    .ok_or_else(|| Box::new(session))
            }
        }
    }

    #[allow(dead_code)]
    pub(in crate::compiler) fn analyze_compile_structure(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<CompileStructureArtifact> {
        let mut session = Session::new();
        session.apply_options(&self.options);
        self.try_analyze_compile_structure(session, input_file, source_overrides)
            .ok()
    }

    pub(in crate::compiler) fn try_analyze_compile_structure(
        &self,
        mut session: Session,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Result<CompileStructureArtifact, Box<Session>> {
        self.sync_source_overrides(source_overrides);
        if let Some(structure) =
            self.cached_compile_structure_artifact(input_file, source_overrides)
        {
            return Ok(structure);
        }
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
            if let Some(structure) = self.build_compile_structure_from_imported(&imported) {
                return Ok(structure);
            }

            let structure =
                self.compute_compile_structure_artifact_into_session(&mut session, input_file);
            return structure.ok_or_else(|| Box::new(session));
        }
        if let Some(collected) =
            self.cached_collected_structure_artifact(input_file, source_overrides)
        {
            if let Some(structure) = self.build_compile_structure(&collected) {
                return Ok(structure);
            }

            let structure =
                self.compute_compile_structure_artifact_into_session(&mut session, input_file);
            return structure.ok_or_else(|| Box::new(session));
        }

        let cache_key = self.structure_cache_key(input_file, source_overrides);
        match self.compile_structure_artifacts.get_with(
            self.frontend.db(),
            "driver_compile_structure_artifact",
            cache_key,
            || Ok(self.compute_compile_structure_artifact_into_session(&mut session, input_file)),
        ) {
            Ok(Some(structure)) => Ok(structure),
            Ok(None) => {
                let structure =
                    self.compute_compile_structure_artifact_into_session(&mut session, input_file);
                structure.ok_or_else(|| Box::new(session))
            }
            Err(_) => {
                let structure =
                    self.compute_compile_structure_artifact_into_session(&mut session, input_file);
                structure.ok_or_else(|| Box::new(session))
            }
        }
    }

    pub(super) fn cached_compile_structure_artifact(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<CompileStructureArtifact> {
        self.sync_source_overrides(source_overrides);
        let cache_key = self.structure_cache_key(input_file, source_overrides);
        let cached = self
            .compile_structure_artifacts
            .get_cached(
                self.frontend.db(),
                "driver_compile_structure_artifact",
                cache_key,
            )
            .ok()
            .flatten()
            .flatten();
        if cached.is_some() {
            self.record_compile_structure_cache_hit();
        } else {
            self.record_compile_structure_cache_miss();
        }
        cached
    }

    pub(super) fn analyze_collected_structure(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<CollectedStructureArtifact> {
        let mut session = Session::new();
        session.apply_options(&self.options);
        self.try_analyze_collected_structure(session, input_file, source_overrides)
            .ok()
            .map(|collected| {
                self.finalize_collected_structure_artifact(input_file, source_overrides, collected)
            })
    }

    pub(super) fn cached_structure_artifact(
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

    pub(super) fn cached_clean_structure_artifact(
        &self,
        input_file: &str,
    ) -> Option<StructureArtifact> {
        let normalized = normalize_driver_path(Path::new(input_file));
        self.clean_structure_reuse_artifacts
            .lock()
            .unwrap()
            .get(&normalized)
            .cloned()
    }

    pub(super) fn cached_clean_collected_structure_artifact(
        &self,
        input_file: &str,
    ) -> Option<CollectedStructureArtifact> {
        let normalized = normalize_driver_path(Path::new(input_file));
        self.clean_collected_reuse_artifacts
            .lock()
            .unwrap()
            .get(&normalized)
            .cloned()
    }

    pub(super) fn cached_clean_imported_structure_artifact(
        &self,
        input_file: &str,
    ) -> Option<ImportedStructureArtifact> {
        let normalized = normalize_driver_path(Path::new(input_file));
        self.clean_imported_reuse_artifacts
            .lock()
            .unwrap()
            .get(&normalized)
            .cloned()
    }

    pub(super) fn cached_collected_structure_artifact(
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

    pub(super) fn cached_imported_structure_artifact(
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

    pub(super) fn try_analyze_collected_structure(
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
            || {
                Ok(self
                    .try_reuse_clean_collected_structure_artifact(input_file, source_overrides)
                    .or_else(|| {
                        self.compute_collected_structure_artifact_into_session(
                            &mut session,
                            input_file,
                        )
                    }))
            },
        ) {
            Ok(Some(collected)) => Ok(self.finalize_collected_structure_artifact(
                input_file,
                source_overrides,
                collected,
            )),
            Ok(None) => {
                let collected = self
                    .compute_collected_structure_artifact_into_session(&mut session, input_file);
                collected
                    .map(|collected| {
                        self.finalize_collected_structure_artifact(
                            input_file,
                            source_overrides,
                            collected,
                        )
                    })
                    .ok_or_else(|| Box::new(session))
            }
            Err(_) => {
                let collected = self
                    .compute_collected_structure_artifact_into_session(&mut session, input_file);
                collected
                    .map(|collected| {
                        self.finalize_collected_structure_artifact(
                            input_file,
                            source_overrides,
                            collected,
                        )
                    })
                    .ok_or_else(|| Box::new(session))
            }
        }
    }

    pub(super) fn try_analyze_imported_structure(
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
            || {
                Ok(self
                    .try_reuse_clean_imported_structure_artifact(input_file, source_overrides)
                    .or_else(|| {
                        self.compute_imported_structure_artifact_into_session(
                            &mut session,
                            input_file,
                        )
                    }))
            },
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

    pub(super) fn compute_collected_structure_artifact_into_session(
        &self,
        session: &mut Session,
        input_file: &str,
    ) -> Option<CollectedStructureArtifact> {
        let mut ctx = self.build_sema_context(session);
        let loaded = self.load_asts(&mut ctx, input_file, true)?;
        self.build_collected_structure_from_context(&mut ctx, loaded.asts)
    }

    pub(super) fn compute_imported_structure_artifact_into_session(
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

    pub(super) fn try_reuse_clean_typed_structure_artifact(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<StructureArtifact> {
        if source_overrides.is_empty() {
            return None;
        }

        let clean_structure = self.cached_clean_structure_artifact(input_file)?;
        let parsed = self
            .try_parse_modules(
                clean_structure.session.clone(),
                input_file,
                source_overrides,
            )
            .ok()?;

        let mut session = parsed.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(clean_structure.snapshot.clone());
        if !self.rebind_body_only_modules(
            &mut ctx,
            &clean_structure.session,
            &clean_structure.asts,
            &parsed,
        ) {
            return None;
        }

        let asts = self.reused_asts(&clean_structure.asts, &parsed)?;
        let symbols = self.collect_analysis_symbols(&ctx, &asts);
        let completion_model = self.collect_structure_completion_model(&ctx, &asts);
        let snapshot = ctx.structure_snapshot();
        drop(ctx);

        self.record_body_only_structure_reuse();
        Some(StructureArtifact {
            session,
            asts,
            symbols,
            snapshot,
            completion_model,
        })
    }

    pub(super) fn finalize_structure_artifact(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
        structure: StructureArtifact,
    ) -> StructureArtifact {
        if source_overrides.is_empty() {
            let normalized = normalize_driver_path(Path::new(input_file));
            self.clean_structure_reuse_artifacts
                .lock()
                .unwrap()
                .insert(normalized, structure.clone());
        }
        structure
    }

    pub(super) fn try_reuse_clean_collected_structure_artifact(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<CollectedStructureArtifact> {
        if source_overrides.is_empty() {
            return None;
        }

        let clean_collected = self.cached_clean_collected_structure_artifact(input_file)?;
        let parsed = self
            .try_parse_modules(
                clean_collected.session.clone(),
                input_file,
                source_overrides,
            )
            .ok()?;

        let mut session = parsed.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(clean_collected.snapshot.clone());
        if !self.rebind_body_only_modules(
            &mut ctx,
            &clean_collected.session,
            &clean_collected.asts,
            &parsed,
        ) {
            return None;
        }

        let asts = self.reused_asts(&clean_collected.asts, &parsed)?;
        let symbols = self.collect_analysis_symbols(&ctx, &asts);
        let snapshot = ctx.structure_snapshot();
        drop(ctx);

        self.record_body_only_collected_reuse();
        Some(CollectedStructureArtifact {
            session,
            asts,
            symbols,
            snapshot,
        })
    }

    pub(super) fn finalize_collected_structure_artifact(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
        collected: CollectedStructureArtifact,
    ) -> CollectedStructureArtifact {
        if source_overrides.is_empty() {
            let normalized = normalize_driver_path(Path::new(input_file));
            self.clean_collected_reuse_artifacts
                .lock()
                .unwrap()
                .insert(normalized, collected.clone());
        }
        collected
    }

    pub(super) fn try_reuse_clean_imported_structure_artifact(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<ImportedStructureArtifact> {
        if source_overrides.is_empty() {
            return None;
        }

        let clean_imported = self.cached_clean_imported_structure_artifact(input_file)?;
        let parsed = self
            .try_parse_modules(clean_imported.session.clone(), input_file, source_overrides)
            .ok()?;

        let mut session = parsed.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(clean_imported.snapshot.clone());
        if !self.rebind_body_only_modules(
            &mut ctx,
            &clean_imported.session,
            &clean_imported.asts,
            &parsed,
        ) {
            return None;
        }

        let asts = self.reused_asts(&clean_imported.asts, &parsed)?;
        let symbols = self.collect_analysis_symbols(&ctx, &asts);
        let completion_model = self.collect_structure_completion_model(&ctx, &asts);
        let snapshot = ctx.structure_snapshot();
        drop(ctx);

        self.record_body_only_imported_reuse();
        Some(ImportedStructureArtifact {
            session,
            asts,
            symbols,
            snapshot,
            completion_model,
        })
    }

    pub(super) fn finalize_imported_structure_artifact(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
        imported: ImportedStructureArtifact,
    ) -> ImportedStructureArtifact {
        if source_overrides.is_empty() {
            let normalized = normalize_driver_path(Path::new(input_file));
            self.clean_imported_reuse_artifacts
                .lock()
                .unwrap()
                .insert(normalized, imported.clone());
        }
        imported
    }

    pub(super) fn compute_compile_structure_artifact_into_session(
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
        if !measure_body_phase(&mut phase_timings, "  structure_collect", || {
            self.run_collect_phase_owned(&mut ctx, loaded.asts)
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

    pub(super) fn compute_structure_artifact_into_session(
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

    pub(super) fn imported_structure_from_typed(
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

    pub(super) fn reused_asts(
        &self,
        clean_asts: &[(DefId, ast::Module)],
        parsed: &ParsedModuleArtifact,
    ) -> Option<Vec<(DefId, ast::Module)>> {
        if clean_asts.len() != parsed.modules.len() {
            return None;
        }

        let parsed_modules = self.index_parsed_modules(parsed);
        let mut asts = Vec::with_capacity(clean_asts.len());
        for (module_id, clean_module) in clean_asts {
            let parsed_module = parsed_modules.get(Path::new(clean_module.path.as_str()))?;
            asts.push((*module_id, parsed_module.ast.clone()));
        }
        Some(asts)
    }

    pub(super) fn index_clean_modules<'a>(
        &self,
        defs: &[kernc_sema::def::Def],
        clean_session: &Session,
        clean_asts: &'a [(DefId, ast::Module)],
    ) -> Option<std::collections::BTreeMap<PathBuf, (DefId, &'a ast::Module)>> {
        let mut modules = std::collections::BTreeMap::new();
        for (module_id, module_ast) in clean_asts {
            let path = clean_session
                .source_manager
                .get_file_path(module_file_id(defs, *module_id))?;
            modules.insert(normalize_driver_path(path), (*module_id, module_ast));
        }
        Some(modules)
    }

    pub(super) fn index_parsed_modules<'a>(
        &self,
        parsed: &'a ParsedModuleArtifact,
    ) -> std::collections::BTreeMap<&'a Path, &'a ParsedModule> {
        parsed
            .modules
            .iter()
            .map(|parsed_module| (parsed_module.path.as_path(), parsed_module))
            .collect()
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
                    body_regions: completion::module_body_completion_regions(&ast),
                    ast,
                }
            })
            .collect();
        drop(ctx);

        Ok(ParsedModuleArtifact { session, modules })
    }

    pub(super) fn build_collected_structure_from_context(
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

    pub(super) fn build_imported_structure(
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

    pub(super) fn build_compile_structure(
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

    pub(super) fn build_typed_structure(
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

    pub(super) fn build_compile_structure_from_imported(
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

    pub(super) fn run_collect_phase<'a>(
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

    pub(super) fn run_collect_phase_owned<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        asts: Vec<(DefId, ast::Module)>,
    ) -> bool {
        let mut collector = Collector::new(ctx);
        for (mod_id, ast) in asts {
            collector.collect_ast_owned(mod_id, ast);
        }
        Self::report_diagnostics_if_errors(collector.context())
    }

    pub(super) fn run_import_phase<'a>(&self, ctx: &mut SemaContext<'a>) -> bool {
        let mut import_resolver = ImportResolver::new(ctx);
        import_resolver.resolve_all();
        Self::report_diagnostics_if_errors(import_resolver.context())
    }

    pub(super) fn run_type_resolution_phase<'a>(
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
}
