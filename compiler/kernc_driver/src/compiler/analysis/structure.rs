use super::*;
use crate::compiler::completion;

impl CompilerDriver {
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
                let file_id = match &ctx.defs[mod_id.0 as usize] {
                    kernc_sema::def::Def::Module(module_def) => module_def.file_id,
                    _ => kernc_utils::FileId(0),
                };
                let source_path = ctx
                    .sess
                    .source_manager
                    .get_file_path(file_id)
                    .map(|path| normalize_driver_path(path))
                    .unwrap_or_default();
                let path = module_analysis_path_from_source(&source_path, ast);
                ParsedModule {
                    file_id,
                    source_path,
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

    pub(in crate::compiler) fn load_asts<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        input_file: &str,
        collect_docs: bool,
    ) -> Option<LoadedAstArtifact> {
        self.load_asts_impl(ctx, input_file, collect_docs, &CancellationToken::new())
            .ok()
            .flatten()
    }

    pub(super) fn load_asts_cancelable<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        input_file: &str,
        collect_docs: bool,
        cancellation: &CancellationToken,
    ) -> Result<Option<LoadedAstArtifact>, Canceled> {
        self.load_asts_impl(ctx, input_file, collect_docs, cancellation)
    }

    fn load_asts_impl<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        input_file: &str,
        collect_docs: bool,
        cancellation: &CancellationToken,
    ) -> Result<Option<LoadedAstArtifact>, Canceled> {
        cancellation.check()?;
        let mut loader =
            ModuleLoader::new_cancelable(ctx, &self.frontend, collect_docs, cancellation);
        let root_name = loader
            .ctx
            .intern(self.options.root_module_name.as_deref().unwrap_or("root"));
        let root_loaded = loader.try_load_root(input_file, root_name)?;
        if root_loaded.is_none() {
            return Ok(None);
        }
        if !Self::report_diagnostics_if_errors(loader.ctx) {
            return Ok(None);
        }

        cancellation.check()?;
        loader.ctx.inject_alias_roots();
        Ok(Some(LoadedAstArtifact {
            asts: std::mem::take(&mut loader.asts),
            phase_timings: loader.phase_timings(),
        }))
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

    pub(in crate::compiler) fn try_analyze_structure_cancelable(
        &self,
        mut session: Session,
        input_file: &str,
        source_overrides: &SourceOverrides,
        cancellation: &CancellationToken,
    ) -> Result<Result<StructureArtifact, Box<Session>>, Canceled> {
        cancellation.check()?;
        self.sync_source_overrides(source_overrides);
        if let Some(structure) = self.cached_structure_artifact(input_file, source_overrides) {
            cancellation.check()?;
            return Ok(Ok(self.finalize_structure_artifact(
                input_file,
                source_overrides,
                structure,
            )));
        }
        cancellation.check()?;
        if let Some(imported) =
            self.cached_imported_structure_artifact(input_file, source_overrides)
        {
            if let Some(structure) =
                self.build_typed_structure_cancelable(&imported, cancellation)?
            {
                cancellation.check()?;
                return Ok(Ok(self.finalize_structure_artifact(
                    input_file,
                    source_overrides,
                    structure,
                )));
            }

            let structure = self.compute_structure_artifact_into_session_cancelable(
                &mut session,
                input_file,
                cancellation,
            )?;
            return Ok(structure
                .map(|structure| {
                    self.finalize_structure_artifact(input_file, source_overrides, structure)
                })
                .ok_or_else(|| Box::new(session)));
        }
        cancellation.check()?;
        let computed = if let Some(reused) = self
            .try_reuse_clean_typed_structure_artifact_cancelable(
                input_file,
                source_overrides,
                cancellation,
            )? {
            Some(reused)
        } else {
            self.compute_structure_artifact_into_session_cancelable(
                &mut session,
                input_file,
                cancellation,
            )?
        };
        cancellation.check()?;
        Ok(computed
            .map(|structure| {
                self.finalize_structure_artifact(input_file, source_overrides, structure)
            })
            .ok_or_else(|| Box::new(session)))
    }

    pub(in crate::compiler) fn analyze_diagnostic_structure_cancelable(
        &self,
        mut session: Session,
        input_file: &str,
        source_overrides: &SourceOverrides,
        cancellation: &CancellationToken,
    ) -> Result<Result<StructureArtifact, Box<Session>>, Canceled> {
        cancellation.check()?;
        self.sync_source_overrides(source_overrides);
        Ok(self
            .compute_diagnostic_structure_artifact_into_session_cancelable(
                &mut session,
                input_file,
                cancellation,
            )?
            .ok_or_else(|| Box::new(session)))
    }

    #[cfg(test)]
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

    pub(super) fn analyze_collected_structure_cancelable(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
        cancellation: &CancellationToken,
    ) -> Result<Option<CollectedStructureArtifact>, Canceled> {
        cancellation.check()?;
        let mut session = Session::new();
        session.apply_options(&self.options);
        match self.try_analyze_collected_structure_cancelable(
            session,
            input_file,
            source_overrides,
            cancellation,
        )? {
            Ok(collected) => Ok(Some(self.finalize_collected_structure_artifact(
                input_file,
                source_overrides,
                collected,
            ))),
            Err(_) => Ok(None),
        }
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

    pub(super) fn try_analyze_collected_structure_cancelable(
        &self,
        mut session: Session,
        input_file: &str,
        source_overrides: &SourceOverrides,
        cancellation: &CancellationToken,
    ) -> Result<Result<CollectedStructureArtifact, Box<Session>>, Canceled> {
        cancellation.check()?;
        self.sync_source_overrides(source_overrides);
        if let Some(collected) =
            self.cached_collected_structure_artifact(input_file, source_overrides)
        {
            cancellation.check()?;
            return Ok(Ok(self.finalize_collected_structure_artifact(
                input_file,
                source_overrides,
                collected,
            )));
        }

        let cache_key = self.structure_cache_key(input_file, source_overrides);
        let collected = match self.collected_artifacts.try_get_with(
            self.frontend.db(),
            "driver_collected_structure_artifact",
            cache_key,
            || {
                if let Some(reused) = self.try_reuse_clean_collected_structure_artifact_cancelable(
                    input_file,
                    source_overrides,
                    cancellation,
                )? {
                    return Ok(Ok(Some(reused)));
                }
                self.compute_collected_structure_artifact_into_session_cancelable(
                    &mut session,
                    input_file,
                    cancellation,
                )
                .map(Ok)
            },
        )? {
            Ok(collected) => collected,
            Err(_) => self.compute_collected_structure_artifact_into_session_cancelable(
                &mut session,
                input_file,
                cancellation,
            )?,
        };
        cancellation.check()?;

        Ok(collected
            .map(|collected| {
                self.finalize_collected_structure_artifact(input_file, source_overrides, collected)
            })
            .ok_or_else(|| Box::new(session)))
    }

    pub(super) fn try_analyze_imported_structure_cancelable(
        &self,
        mut session: Session,
        input_file: &str,
        source_overrides: &SourceOverrides,
        cancellation: &CancellationToken,
    ) -> Result<Result<ImportedStructureArtifact, Box<Session>>, Canceled> {
        cancellation.check()?;
        self.sync_source_overrides(source_overrides);
        let cache_key = self.structure_cache_key(input_file, source_overrides);
        let imported = match self.imported_artifacts.try_get_with(
            self.frontend.db(),
            "driver_imported_structure_artifact",
            cache_key,
            || {
                if let Some(reused) = self.try_reuse_clean_imported_structure_artifact_cancelable(
                    input_file,
                    source_overrides,
                    cancellation,
                )? {
                    return Ok(Ok(Some(reused)));
                }
                self.compute_imported_structure_artifact_into_session_cancelable(
                    &mut session,
                    input_file,
                    cancellation,
                )
                .map(Ok)
            },
        )? {
            Ok(imported) => imported,
            Err(_) => self.compute_imported_structure_artifact_into_session_cancelable(
                &mut session,
                input_file,
                cancellation,
            )?,
        };
        cancellation.check()?;

        Ok(imported.ok_or_else(|| Box::new(session)))
    }

    pub(super) fn compute_collected_structure_artifact_into_session_cancelable(
        &self,
        session: &mut Session,
        input_file: &str,
        cancellation: &CancellationToken,
    ) -> Result<Option<CollectedStructureArtifact>, Canceled> {
        cancellation.check()?;
        let mut ctx = self.build_sema_context(session);
        let loaded = self.load_asts_cancelable(&mut ctx, input_file, true, cancellation)?;
        let Some(loaded) = loaded else {
            return Ok(None);
        };
        cancellation.check()?;
        self.build_collected_structure_from_context_cancelable(&mut ctx, loaded.asts, cancellation)
    }

    pub(super) fn compute_imported_structure_artifact_into_session_cancelable(
        &self,
        session: &mut Session,
        input_file: &str,
        cancellation: &CancellationToken,
    ) -> Result<Option<ImportedStructureArtifact>, Canceled> {
        cancellation.check()?;
        let mut ctx = self.build_sema_context(session);
        let loaded = self.load_asts_cancelable(&mut ctx, input_file, true, cancellation)?;
        let Some(loaded) = loaded else {
            return Ok(None);
        };
        let asts = loaded.asts;
        cancellation.check()?;
        if !self.run_collect_phase_cancelable(&mut ctx, &asts, cancellation)? {
            return Ok(None);
        }
        cancellation.check()?;
        let symbols = self.collect_analysis_symbols(&ctx, &asts);
        cancellation.check()?;
        if !self.run_import_phase_cancelable(&mut ctx, cancellation)? {
            return Ok(None);
        }
        cancellation.check()?;
        let completion_model = self.collect_structure_completion_model(&ctx, &asts);
        let snapshot = ctx.into_structure_snapshot();
        let session = std::mem::take(session);

        Ok(Some(ImportedStructureArtifact {
            session,
            asts,
            symbols,
            snapshot,
            completion_model,
        }))
    }

    pub(super) fn try_reuse_clean_typed_structure_artifact(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<StructureArtifact> {
        self.try_reuse_clean_typed_structure_artifact_cancelable(
            input_file,
            source_overrides,
            &CancellationToken::new(),
        )
        .expect("fresh cancellation token cannot be canceled")
    }

    pub(super) fn try_reuse_clean_typed_structure_artifact_cancelable(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
        cancellation: &CancellationToken,
    ) -> Result<Option<StructureArtifact>, Canceled> {
        cancellation.check()?;
        if source_overrides.is_empty() {
            return Ok(None);
        }

        let Some(clean_structure) = self.cached_clean_structure_artifact(input_file) else {
            return Ok(None);
        };
        let parsed = self
            .try_parse_modules_cancelable(
                clean_structure.session.clone(),
                input_file,
                source_overrides,
                cancellation,
            )?
            .ok();
        let Some(parsed) = parsed else {
            return Ok(None);
        };
        cancellation.check()?;

        let mut session = parsed.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(clean_structure.snapshot.clone());
        if !self.rebind_body_only_modules_cancelable(
            &mut ctx,
            &clean_structure.session,
            &clean_structure.asts,
            &parsed,
            cancellation,
        )? {
            return Ok(None);
        }
        cancellation.check()?;

        let Some(asts) =
            self.reused_asts_cancelable(&clean_structure.asts, &parsed, cancellation)?
        else {
            return Ok(None);
        };
        let symbols = self.collect_analysis_symbols(&ctx, &asts);
        cancellation.check()?;
        let completion_model = self.collect_structure_completion_model(&ctx, &asts);
        cancellation.check()?;
        let trait_impl_stubs = self.collect_trait_impl_stubs(&ctx);
        cancellation.check()?;
        let snapshot = ctx.structure_snapshot();
        drop(ctx);

        self.record_body_only_structure_reuse();
        Ok(Some(StructureArtifact {
            session,
            asts,
            symbols,
            snapshot,
            completion_model,
            trait_impl_stubs,
        }))
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

    pub(super) fn try_reuse_clean_collected_structure_artifact_cancelable(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
        cancellation: &CancellationToken,
    ) -> Result<Option<CollectedStructureArtifact>, Canceled> {
        cancellation.check()?;
        if source_overrides.is_empty() {
            return Ok(None);
        }

        let Some(clean_collected) = self.cached_clean_collected_structure_artifact(input_file)
        else {
            return Ok(None);
        };
        let parsed = self
            .try_parse_modules_cancelable(
                clean_collected.session.clone(),
                input_file,
                source_overrides,
                cancellation,
            )?
            .ok();
        let Some(parsed) = parsed else {
            return Ok(None);
        };
        cancellation.check()?;

        let mut session = parsed.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(clean_collected.snapshot.clone());
        if !self.rebind_body_only_modules_cancelable(
            &mut ctx,
            &clean_collected.session,
            &clean_collected.asts,
            &parsed,
            cancellation,
        )? {
            return Ok(None);
        }
        cancellation.check()?;

        let Some(asts) =
            self.reused_asts_cancelable(&clean_collected.asts, &parsed, cancellation)?
        else {
            return Ok(None);
        };
        let symbols = self.collect_analysis_symbols(&ctx, &asts);
        cancellation.check()?;
        let snapshot = ctx.structure_snapshot();
        drop(ctx);

        self.record_body_only_collected_reuse();
        Ok(Some(CollectedStructureArtifact {
            session,
            asts,
            symbols,
            snapshot,
        }))
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

    pub(super) fn try_reuse_clean_imported_structure_artifact_cancelable(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
        cancellation: &CancellationToken,
    ) -> Result<Option<ImportedStructureArtifact>, Canceled> {
        cancellation.check()?;
        if source_overrides.is_empty() {
            return Ok(None);
        }

        let Some(clean_imported) = self.cached_clean_imported_structure_artifact(input_file) else {
            return Ok(None);
        };
        let parsed = self
            .try_parse_modules_cancelable(
                clean_imported.session.clone(),
                input_file,
                source_overrides,
                cancellation,
            )?
            .ok();
        let Some(parsed) = parsed else {
            return Ok(None);
        };
        cancellation.check()?;

        let mut session = parsed.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(clean_imported.snapshot.clone());
        if !self.rebind_body_only_modules_cancelable(
            &mut ctx,
            &clean_imported.session,
            &clean_imported.asts,
            &parsed,
            cancellation,
        )? {
            return Ok(None);
        }
        cancellation.check()?;

        let Some(asts) =
            self.reused_asts_cancelable(&clean_imported.asts, &parsed, cancellation)?
        else {
            return Ok(None);
        };
        let symbols = self.collect_analysis_symbols(&ctx, &asts);
        cancellation.check()?;
        let completion_model = self.collect_structure_completion_model(&ctx, &asts);
        cancellation.check()?;
        let snapshot = ctx.structure_snapshot();
        drop(ctx);

        self.record_body_only_imported_reuse();
        Ok(Some(ImportedStructureArtifact {
            session,
            asts,
            symbols,
            snapshot,
            completion_model,
        }))
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
        let mut type_resolution_phase_timings = Vec::new();
        let type_resolution_started = std::time::Instant::now();
        let type_resolution_ok = self.run_type_resolution_phase_with_timings(
            &mut ctx,
            collect_docs,
            Some(&mut type_resolution_phase_timings),
        );
        phase_timings.push(PhaseTiming {
            name: "  structure_type_resolution",
            duration: type_resolution_started.elapsed(),
        });
        phase_timings.extend(type_resolution_phase_timings);
        if !type_resolution_ok {
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
        let trait_impl_stubs = self.collect_trait_impl_stubs(&ctx);
        let snapshot = ctx.into_structure_snapshot();
        let session = std::mem::take(session);

        Some(StructureArtifact {
            session,
            asts,
            symbols,
            snapshot,
            completion_model,
            trait_impl_stubs,
        })
    }

    pub(super) fn compute_structure_artifact_into_session_cancelable(
        &self,
        session: &mut Session,
        input_file: &str,
        cancellation: &CancellationToken,
    ) -> Result<Option<StructureArtifact>, Canceled> {
        cancellation.check()?;
        let mut ctx = self.build_sema_context(session);
        let loaded = self.load_asts_cancelable(&mut ctx, input_file, true, cancellation)?;
        let Some(loaded) = loaded else {
            return Ok(None);
        };
        let asts = loaded.asts;
        cancellation.check()?;
        if !self.run_collect_phase_cancelable(&mut ctx, &asts, cancellation)? {
            return Ok(None);
        }
        cancellation.check()?;
        let symbols = self.collect_analysis_symbols(&ctx, &asts);
        cancellation.check()?;
        if !self.run_import_phase_cancelable(&mut ctx, cancellation)? {
            return Ok(None);
        }
        cancellation.check()?;
        if !self.run_type_resolution_phase_with_timings_cancelable(
            &mut ctx,
            true,
            None,
            cancellation,
        )? {
            return Ok(None);
        }
        cancellation.check()?;

        let completion_model = self.collect_structure_completion_model(&ctx, &asts);
        let trait_impl_stubs = self.collect_trait_impl_stubs(&ctx);
        let snapshot = ctx.into_structure_snapshot();
        let session = std::mem::take(session);

        Ok(Some(StructureArtifact {
            session,
            asts,
            symbols,
            snapshot,
            completion_model,
            trait_impl_stubs,
        }))
    }

    fn compute_diagnostic_structure_artifact_into_session_cancelable(
        &self,
        session: &mut Session,
        input_file: &str,
        cancellation: &CancellationToken,
    ) -> Result<Option<StructureArtifact>, Canceled> {
        cancellation.check()?;
        let mut ctx = self.build_sema_context(session);
        let loaded = self.load_asts_cancelable(&mut ctx, input_file, true, cancellation)?;
        let Some(loaded) = loaded else {
            return Ok(None);
        };
        let asts = loaded.asts;
        cancellation.check()?;
        if !self.run_collect_phase_cancelable(&mut ctx, &asts, cancellation)? {
            return Ok(None);
        }
        cancellation.check()?;
        let symbols = self.collect_analysis_symbols(&ctx, &asts);
        cancellation.check()?;
        if !self.run_import_phase_cancelable(&mut ctx, cancellation)? {
            return Ok(None);
        }
        cancellation.check()?;
        let _ = self.run_type_resolution_phase_with_timings_cancelable(
            &mut ctx,
            true,
            None,
            cancellation,
        );
        cancellation.check()?;

        let completion_model = self.collect_structure_completion_model(&ctx, &asts);
        let trait_impl_stubs = self.collect_trait_impl_stubs(&ctx);
        let snapshot = ctx.into_structure_snapshot();
        let session = std::mem::take(session);

        Ok(Some(StructureArtifact {
            session,
            asts,
            symbols,
            snapshot,
            completion_model,
            trait_impl_stubs,
        }))
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

    pub(super) fn reused_asts_cancelable(
        &self,
        clean_asts: &[(DefId, ast::Module)],
        parsed: &ParsedModuleArtifact,
        cancellation: &CancellationToken,
    ) -> Result<Option<Vec<(DefId, ast::Module)>>, Canceled> {
        if clean_asts.len() != parsed.modules.len() {
            return Ok(None);
        }

        let parsed_modules = self.index_parsed_modules(parsed);
        let mut asts = Vec::with_capacity(clean_asts.len());
        for (module_id, clean_module) in clean_asts {
            cancellation.check()?;
            let Some(parsed_module) = parsed_modules.get(Path::new(clean_module.path.as_str()))
            else {
                return Ok(None);
            };
            asts.push((*module_id, parsed_module.ast.clone()));
        }
        Ok(Some(asts))
    }

    pub(super) fn index_clean_modules_cancelable<'a>(
        &self,
        defs: &[kernc_sema::def::Def],
        clean_session: &Session,
        clean_asts: &'a [(DefId, ast::Module)],
        cancellation: &CancellationToken,
    ) -> Result<Option<std::collections::BTreeMap<PathBuf, (DefId, &'a ast::Module)>>, Canceled>
    {
        let mut modules = std::collections::BTreeMap::new();
        for (module_id, module_ast) in clean_asts {
            cancellation.check()?;
            let path = module_analysis_path(clean_session, defs, *module_id, module_ast);
            if path.as_os_str().is_empty() {
                return Ok(None);
            }
            modules.insert(path, (*module_id, module_ast));
        }
        Ok(Some(modules))
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
        session: Session,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Result<ParsedModuleArtifact, Box<Session>> {
        self.try_parse_modules_cancelable(
            session,
            input_file,
            source_overrides,
            &CancellationToken::new(),
        )
        .expect("fresh cancellation token cannot be canceled")
    }

    pub(super) fn try_parse_modules_cancelable(
        &self,
        mut session: Session,
        input_file: &str,
        source_overrides: &SourceOverrides,
        cancellation: &CancellationToken,
    ) -> Result<Result<ParsedModuleArtifact, Box<Session>>, Canceled> {
        cancellation.check()?;
        self.sync_source_overrides(source_overrides);
        let mut ctx = self.build_sema_context(&mut session);
        let loaded = self.load_asts_cancelable(&mut ctx, input_file, true, cancellation)?;
        let Some(loaded) = loaded else {
            drop(ctx);
            return Ok(Err(Box::new(session)));
        };
        cancellation.check()?;
        let modules = loaded
            .asts
            .into_iter()
            .map(|(mod_id, ast)| {
                let file_id = match &ctx.defs[mod_id.0 as usize] {
                    kernc_sema::def::Def::Module(module_def) => module_def.file_id,
                    _ => kernc_utils::FileId(0),
                };
                let source_path = ctx
                    .sess
                    .source_manager
                    .get_file_path(file_id)
                    .map(|path| normalize_driver_path(path))
                    .unwrap_or_default();
                let path = module_analysis_path_from_source(&source_path, &ast);
                ParsedModule {
                    file_id,
                    source_path,
                    path,
                    body_regions: completion::module_body_completion_regions(&ast),
                    ast,
                }
            })
            .collect();
        drop(ctx);

        cancellation.check()?;
        Ok(Ok(ParsedModuleArtifact { session, modules }))
    }

    pub(in crate::compiler) fn build_collected_structure_from_context_cancelable(
        &self,
        ctx: &mut SemaContext<'_>,
        asts: Vec<(DefId, ast::Module)>,
        cancellation: &CancellationToken,
    ) -> Result<Option<CollectedStructureArtifact>, Canceled> {
        if !self.run_collect_phase_cancelable(ctx, &asts, cancellation)? {
            return Ok(None);
        }
        cancellation.check()?;
        let symbols = self.collect_analysis_symbols(ctx, &asts);

        cancellation.check()?;
        Ok(Some(CollectedStructureArtifact {
            session: ctx.sess.clone(),
            asts,
            symbols,
            snapshot: ctx.structure_snapshot(),
        }))
    }

    pub(in crate::compiler) fn build_imported_structure_cancelable(
        &self,
        collected: &CollectedStructureArtifact,
        cancellation: &CancellationToken,
    ) -> Result<Option<ImportedStructureArtifact>, Canceled> {
        let mut session = collected.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(collected.snapshot.clone());
        if !self.run_import_phase_cancelable(&mut ctx, cancellation)? {
            return Ok(None);
        }
        cancellation.check()?;
        let asts = collected.asts.clone();
        let completion_model = self.collect_structure_completion_model(&ctx, &asts);
        cancellation.check()?;
        let snapshot = ctx.structure_snapshot();
        drop(ctx);

        Ok(Some(ImportedStructureArtifact {
            session,
            asts,
            symbols: collected.symbols.clone(),
            snapshot,
            completion_model,
        }))
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
        self.build_typed_structure_cancelable(imported, &CancellationToken::new())
            .expect("fresh cancellation token cannot be canceled")
    }

    pub(super) fn build_typed_structure_cancelable(
        &self,
        imported: &ImportedStructureArtifact,
        cancellation: &CancellationToken,
    ) -> Result<Option<StructureArtifact>, Canceled> {
        let mut session = imported.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(imported.snapshot.clone());
        if !self.run_type_resolution_phase_with_timings_cancelable(
            &mut ctx,
            true,
            None,
            cancellation,
        )? {
            return Ok(None);
        }
        cancellation.check()?;

        let asts = imported.asts.clone();
        let completion_model = self.collect_structure_completion_model(&ctx, &asts);
        cancellation.check()?;
        let trait_impl_stubs = self.collect_trait_impl_stubs(&ctx);
        cancellation.check()?;
        let snapshot = ctx.structure_snapshot();
        drop(ctx);

        Ok(Some(StructureArtifact {
            session,
            asts,
            symbols: imported.symbols.clone(),
            snapshot,
            completion_model,
            trait_impl_stubs,
        }))
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
        self.run_collect_phase_cancelable(ctx, asts, &CancellationToken::new())
            .expect("fresh cancellation token cannot be canceled")
    }

    pub(super) fn run_collect_phase_cancelable<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        asts: &[(DefId, ast::Module)],
        cancellation: &CancellationToken,
    ) -> Result<bool, Canceled> {
        let mut collector = Collector::new(ctx);
        for (mod_id, ast) in asts {
            cancellation.check()?;
            collector.collect_ast_cancelable(*mod_id, ast, cancellation)?;
        }
        cancellation.check()?;
        Ok(Self::report_diagnostics_if_errors(collector.context()))
    }

    pub(super) fn run_collect_phase_owned<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        asts: Vec<(DefId, ast::Module)>,
    ) -> bool {
        self.run_collect_phase_owned_cancelable(ctx, asts, &CancellationToken::new())
            .expect("fresh cancellation token cannot be canceled")
    }

    pub(super) fn run_collect_phase_owned_cancelable<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        asts: Vec<(DefId, ast::Module)>,
        cancellation: &CancellationToken,
    ) -> Result<bool, Canceled> {
        let mut collector = Collector::new(ctx);
        for (mod_id, ast) in asts {
            cancellation.check()?;
            collector.collect_ast_owned_cancelable(mod_id, ast, cancellation)?;
        }
        cancellation.check()?;
        Ok(Self::report_diagnostics_if_errors(collector.context()))
    }

    pub(super) fn run_import_phase<'a>(&self, ctx: &mut SemaContext<'a>) -> bool {
        self.run_import_phase_cancelable(ctx, &CancellationToken::new())
            .expect("fresh cancellation token cannot be canceled")
    }

    pub(super) fn run_import_phase_cancelable<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        cancellation: &CancellationToken,
    ) -> Result<bool, Canceled> {
        let mut import_resolver = ImportResolver::new(ctx);
        import_resolver.resolve_all_cancelable(cancellation)?;
        cancellation.check()?;
        Ok(Self::report_diagnostics_if_errors(
            import_resolver.context(),
        ))
    }

    pub(super) fn run_type_resolution_phase<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        lint_docs_enabled: bool,
    ) -> bool {
        self.run_type_resolution_phase_with_timings(ctx, lint_docs_enabled, None)
    }

    pub(super) fn run_type_resolution_phase_with_timings<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        lint_docs_enabled: bool,
        phase_timings: Option<&mut Vec<PhaseTiming>>,
    ) -> bool {
        self.run_type_resolution_phase_with_timings_cancelable(
            ctx,
            lint_docs_enabled,
            phase_timings,
            &CancellationToken::new(),
        )
        .expect("fresh cancellation token cannot be canceled")
    }

    pub(super) fn run_type_resolution_phase_with_timings_cancelable<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        lint_docs_enabled: bool,
        phase_timings: Option<&mut Vec<PhaseTiming>>,
        cancellation: &CancellationToken,
    ) -> Result<bool, Canceled> {
        let mut type_resolver = TypeResolver::new(ctx);
        type_resolver.resolve_all_cancelable(cancellation)?;
        let type_resolution_timings = type_resolver.phase_timings();
        let ctx = type_resolver.into_context();
        if !Self::report_diagnostics_if_errors(ctx) {
            return Ok(false);
        }
        cancellation.check()?;
        if let Some(phase_timings) = phase_timings {
            phase_timings.extend(
                type_resolution_timings
                    .into_iter()
                    .map(|timing| PhaseTiming {
                        name: timing.name,
                        duration: timing.duration,
                    }),
            );
        }
        if !self.configure_program_entry(ctx) {
            return Ok(false);
        }
        cancellation.check()?;
        if lint_docs_enabled {
            lint_docs(ctx);
        }
        cancellation.check()?;
        Ok(true)
    }
}
