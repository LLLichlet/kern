use super::*;
use std::collections::{BTreeMap, BTreeSet};

impl AnalysisEngine {
    pub(crate) fn warm_workspace_symbol_indexes_cancelable(
        &self,
        workspace_roots: Vec<PathBuf>,
        cancellation: CancellationToken,
    ) -> Result<(usize, usize), String> {
        let snapshot = self.snapshot(workspace_roots, cancellation);
        match self.warm_workspace_symbol_indexes_in_snapshot_cancelable(&snapshot) {
            Ok(indexed_targets) => Ok((indexed_targets, 0)),
            Err(err) if snapshot.cancellation.is_canceled() => Err(err),
            Err(_) => Ok((0, 1)),
        }
    }

    fn warm_workspace_symbol_indexes_in_snapshot_cancelable(
        &self,
        snapshot: &AnalysisSnapshot,
    ) -> Result<usize, String> {
        snapshot.check_canceled()?;
        let mut indexed_targets = 0;

        if !snapshot.workspace_roots().is_empty() {
            let mut saw_project = false;
            for workspace_root in snapshot.workspace_roots() {
                snapshot.check_canceled()?;
                let Some(project) = self.project_for_path(workspace_root)? else {
                    continue;
                };
                saw_project = true;
                let targets = project
                    .workspace_targets(&self.settings.compile_options)
                    .map_err(|err| {
                        format!(
                            "workspace symbol project indexing failed for `{}`: {err}",
                            project.manifest_path().display()
                        )
                    })?;
                for resolved in targets {
                    snapshot.check_canceled()?;
                    let context = self.analysis_context_for_resolved_and_dirty(
                        resolved,
                        snapshot.dirty_documents(),
                        snapshot.cancellation.clone(),
                    )?;
                    self.surface_symbol_index_for_context(
                        &context,
                        snapshot.uri_by_normalized_path(),
                    )?;
                    indexed_targets += 1;
                }
            }
            if saw_project {
                self.record_analysis_tier(AnalysisTier::Surface);
                return Ok(indexed_targets);
            }
        }

        for document in snapshot.documents.values() {
            snapshot.check_canceled()?;
            let resolved = self.resolve_analysis_for_snapshot_document(snapshot, document)?;
            let context = self.analysis_context_for_resolved_and_dirty(
                resolved,
                snapshot.dirty_documents(),
                snapshot.cancellation.clone(),
            )?;
            self.surface_symbol_index_for_context(&context, snapshot.uri_by_normalized_path())?;
            indexed_targets += 1;
        }

        self.record_analysis_tier(AnalysisTier::Surface);
        Ok(indexed_targets)
    }

    #[cfg(test)]
    pub(crate) fn warm_workspace_symbol_indexes_with_cancellation_for_testing(
        &self,
        workspace_roots: Vec<PathBuf>,
        cancellation: CancellationToken,
    ) -> Result<(usize, usize), String> {
        self.warm_workspace_symbol_indexes_cancelable(workspace_roots, cancellation)
    }

    #[cfg(test)]
    pub fn workspace_symbols(&self, query: &str) -> Result<Vec<IdeWorkspaceSymbol>, String> {
        let snapshot = self.snapshot(Vec::new(), CancellationToken::new());
        self.workspace_symbols_in_snapshot(&snapshot, query)
    }

    pub fn workspace_symbols_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        query: &str,
    ) -> Result<Vec<IdeWorkspaceSymbol>, String> {
        snapshot.check_canceled()?;
        let needle = query.trim().to_ascii_lowercase();
        let mut symbols = Vec::new();

        if !snapshot.workspace_roots().is_empty() {
            let mut saw_project = false;
            for workspace_root in snapshot.workspace_roots() {
                snapshot.check_canceled()?;
                let Some(project) = self.project_for_path(workspace_root)? else {
                    continue;
                };
                saw_project = true;
                let targets = project
                    .workspace_targets(&self.settings.compile_options)
                    .map_err(|err| {
                        format!(
                            "workspace symbol project analysis failed for `{}`: {err}",
                            project.manifest_path().display()
                        )
                    })?;
                for resolved in targets {
                    snapshot.check_canceled()?;
                    let context = self.analysis_context_for_resolved_and_dirty(
                        resolved,
                        snapshot.dirty_documents(),
                        snapshot.cancellation.clone(),
                    )?;
                    let index = self.surface_symbol_index_for_context(
                        &context,
                        snapshot.uri_by_normalized_path(),
                    )?;
                    for symbol in index.workspace_symbols.iter() {
                        snapshot.check_canceled()?;
                        if workspace_symbol_matches_query(symbol, &needle) {
                            symbols.push(symbol.clone());
                        }
                    }
                }
            }
            if saw_project {
                symbols.sort_by(workspace_symbol_order);
                symbols.dedup_by(workspace_symbol_same_location);
                self.record_analysis_tier(AnalysisTier::Surface);
                return Ok(symbols);
            }
        }

        for document in snapshot.documents.values() {
            snapshot.check_canceled()?;
            let resolved = self.resolve_analysis_for_snapshot_document(snapshot, document)?;
            let context = self.analysis_context_for_resolved_and_dirty(
                resolved,
                snapshot.dirty_documents(),
                snapshot.cancellation.clone(),
            )?;
            let index =
                self.surface_symbol_index_for_context(&context, snapshot.uri_by_normalized_path())?;
            for symbol in index.workspace_symbols.iter() {
                snapshot.check_canceled()?;
                if workspace_symbol_matches_query(symbol, &needle) {
                    symbols.push(symbol.clone());
                }
            }
        }

        symbols.sort_by(workspace_symbol_order);
        symbols.dedup_by(workspace_symbol_same_location);
        self.record_analysis_tier(AnalysisTier::Surface);
        Ok(symbols)
    }

    fn surface_symbol_index_for_context(
        &self,
        context: &AnalysisRequestContext,
        uri_by_path: &BTreeMap<PathBuf, String>,
    ) -> Result<Arc<SurfaceSymbolIndex>, String> {
        context.check_canceled()?;
        {
            let mut workspace_index = self.workspace_index.lock().unwrap();
            if let Some(index) = workspace_index
                .symbol_indexes
                .get(&context.cache_key)
                .cloned()
            {
                self.record_cache_hit(AnalysisCacheTraceKind::WorkspaceSymbolIndex);
                workspace_index
                    .targets
                    .entry(context.cache_key.clone())
                    .or_insert_with(|| WorkspaceIndexTarget::from_resolved(&context.resolved));
                return Ok(index);
            }
        }
        self.record_cache_miss(AnalysisCacheTraceKind::WorkspaceSymbolIndex);

        let Some(surface) = self.analyze_surface_artifact_for_context(context)? else {
            let index = Arc::new(SurfaceSymbolIndex {
                document_symbols_by_path: BTreeMap::new(),
                workspace_symbols: Arc::new(Vec::new()),
            });
            self.prune_cache_family_for_insert(&context.cache_key);
            let mut workspace_index = self.workspace_index.lock().unwrap();
            workspace_index
                .symbol_indexes
                .insert(context.cache_key.clone(), Arc::clone(&index));
            workspace_index.targets.insert(
                context.cache_key.clone(),
                WorkspaceIndexTarget::from_resolved(&context.resolved),
            );
            self.record_cache_store(AnalysisCacheTraceKind::WorkspaceSymbolIndex);
            return Ok(index);
        };
        let index = Arc::new(surface_symbol_index_from_artifact_cancelable(
            &surface,
            uri_by_path,
            &context.cancellation,
        )?);
        self.prune_cache_family_for_insert(&context.cache_key);
        let mut workspace_index = self.workspace_index.lock().unwrap();
        workspace_index
            .symbol_indexes
            .insert(context.cache_key.clone(), Arc::clone(&index));
        workspace_index.targets.insert(
            context.cache_key.clone(),
            WorkspaceIndexTarget::from_resolved(&context.resolved),
        );
        self.record_cache_store(AnalysisCacheTraceKind::WorkspaceSymbolIndex);
        Ok(index)
    }

    fn semantic_query_offset(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        position: &IdePosition,
    ) -> Result<Option<usize>, String> {
        let Some(target_doc) = snapshot.document(uri) else {
            return Err("requested semantic query for a document that is not open".to_string());
        };
        let file = snapshot.document_source_file(uri).ok_or_else(|| {
            "requested semantic query for a document that is not open".to_string()
        })?;
        let Some(offset) = position_to_byte_offset(&file, position) else {
            return Ok(None);
        };
        if self
            .lexical_index_for_document(uri, target_doc)
            .contains(offset)
        {
            self.record_analysis_tier(AnalysisTier::Lexical);
            return Ok(None);
        }
        Ok(Some(offset))
    }

    #[cfg(test)]
    pub fn document_symbols(&self, uri: &str) -> Result<Vec<IdeDocumentSymbol>, String> {
        let snapshot = self.snapshot(Vec::new(), CancellationToken::new());
        self.document_symbols_in_snapshot(&snapshot, uri)
    }

    pub fn document_symbols_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
    ) -> Result<Vec<IdeDocumentSymbol>, String> {
        snapshot.check_canceled()?;
        let context = self.resolve_analysis_context_for_snapshot(snapshot, uri)?;
        let surface =
            if snapshot.dirty_documents().is_clean() || !context.resolved.input_file.is_file() {
                self.analyze_surface_artifact_for_context(&context)?
                    .map(|surface| (surface, context.cache_key.clone()))
                    .or_else(|| {
                        self.analyze_clean_surface_for_context(&context)
                            .ok()
                            .flatten()
                            .map(|surface| (surface, AnalysisCacheKey::clean(&context.resolved)))
                    })
            } else {
                self.analyze_clean_surface_for_context(&context)?
                    .map(|surface| (surface, AnalysisCacheKey::clean(&context.resolved)))
            };
        let Some((surface, symbol_analysis_key)) = surface else {
            return Ok(Vec::new());
        };

        let Some(target_doc) = snapshot.document(uri) else {
            return Err("requested document symbols for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let index = {
            let mut workspace_index = self.workspace_index.lock().unwrap();
            if let Some(index) = workspace_index
                .symbol_indexes
                .get(&symbol_analysis_key)
                .cloned()
            {
                workspace_index
                    .targets
                    .entry(symbol_analysis_key.clone())
                    .or_insert_with(|| WorkspaceIndexTarget::from_resolved(&context.resolved));
                index
            } else {
                drop(workspace_index);
                let index = Arc::new(surface_symbol_index_from_artifact_cancelable(
                    &surface,
                    snapshot.uri_by_normalized_path(),
                    &snapshot.cancellation,
                )?);
                self.prune_cache_family_for_insert(&symbol_analysis_key);
                let mut workspace_index = self.workspace_index.lock().unwrap();
                workspace_index
                    .symbol_indexes
                    .insert(symbol_analysis_key.clone(), Arc::clone(&index));
                workspace_index.targets.insert(
                    symbol_analysis_key,
                    WorkspaceIndexTarget::from_resolved(&context.resolved),
                );
                index
            }
        };

        self.record_analysis_tier(AnalysisTier::Surface);
        Ok(index
            .document_symbols_by_path
            .get(&target_path)
            .map(|symbols| symbols.as_ref().clone())
            .unwrap_or_default())
    }

    #[cfg(test)]
    pub fn code_lenses(&self, uri: &str) -> Result<Vec<IdeCodeLens>, String> {
        let snapshot = self.snapshot(Vec::new(), CancellationToken::new());
        self.code_lenses_in_snapshot(&snapshot, uri)
    }

    pub fn code_lenses_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
    ) -> Result<Vec<IdeCodeLens>, String> {
        snapshot.check_canceled()?;
        let Some(document) = snapshot.document(uri) else {
            return Err("requested code lenses for a document that is not open".to_string());
        };
        let Some(project) = self.project_for_path(&document.path)? else {
            return Ok(Vec::new());
        };

        let target_path = normalize_path(&document.path);
        let Some(file) = snapshot.document_source_file(uri) else {
            return Ok(Vec::new());
        };
        let range = IdeRange {
            start: IdePosition {
                line: 0,
                character: 0,
            },
            end: IdePosition {
                line: 0,
                character: first_line_end_character(&file),
            },
        };

        let mut lenses = Vec::new();
        for target in project.analysis_targets().map_err(|err| {
            format!(
                "code lens project analysis failed for `{}`: {err}",
                project.manifest_path().display()
            )
        })? {
            snapshot.check_canceled()?;
            if normalize_path(&target.root) != target_path {
                continue;
            }
            let manifest = target.manifest_path.to_string_lossy().to_string();
            match target.kind {
                craft::plan::TargetKind::Test => {
                    let Some(name) = target.name else {
                        continue;
                    };
                    lenses.push(IdeCodeLens {
                        range: range.clone(),
                        title: format!("Run Test {name}"),
                        command: "kern.craft.testTarget".to_string(),
                        arguments: vec![serde_json::json!({
                            "manifestPath": manifest,
                            "targetName": name,
                        })],
                    });
                }
                craft::plan::TargetKind::Lib
                | craft::plan::TargetKind::Bin
                | craft::plan::TargetKind::Example => {
                    let label = target
                        .name
                        .as_ref()
                        .map(|name| format!("{} {name}", target.kind.as_str()))
                        .unwrap_or_else(|| target.kind.as_str().to_string());
                    lenses.push(IdeCodeLens {
                        range: range.clone(),
                        title: format!("Build {label}"),
                        command: "kern.craft.buildPackage".to_string(),
                        arguments: vec![serde_json::json!({
                            "manifestPath": manifest,
                            "targetKind": target.kind.as_str(),
                            "targetName": target.name,
                        })],
                    });
                }
            }
        }
        lenses.sort_by(|lhs, rhs| {
            (
                lhs.range.start.line,
                lhs.range.start.character,
                lhs.title.as_str(),
                lhs.command.as_str(),
            )
                .cmp(&(
                    rhs.range.start.line,
                    rhs.range.start.character,
                    rhs.title.as_str(),
                    rhs.command.as_str(),
                ))
        });
        Ok(lenses)
    }

    #[cfg(test)]
    pub fn goto_definition(
        &self,
        uri: &str,
        position: impl IntoIdePosition,
    ) -> Result<Option<IdeLocation>, String> {
        let snapshot = self.snapshot(Vec::new(), CancellationToken::new());
        self.goto_definition_in_snapshot(&snapshot, uri, position)
    }

    pub fn goto_definition_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        position: impl IntoIdePosition,
    ) -> Result<Option<IdeLocation>, String> {
        self.goto_definition_like_in_snapshot(snapshot, uri, position, "definition")
    }

    pub fn goto_declaration_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        position: impl IntoIdePosition,
    ) -> Result<Option<IdeLocation>, String> {
        self.goto_definition_like_in_snapshot(snapshot, uri, position, "declaration")
    }

    fn goto_definition_like_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        position: impl IntoIdePosition,
        query_name: &str,
    ) -> Result<Option<IdeLocation>, String> {
        let position = position.into_ide_position();
        if self
            .semantic_query_offset(snapshot, uri, &position)?
            .is_none()
        {
            return Ok(None);
        }
        let artifact = self
            .analyze_interactive_navigation_artifact_for_snapshot(snapshot, uri)
            .map_err(|message| format!("{query_name} analysis failed: {message}"))?;
        let Some(target_doc) = snapshot.document(uri) else {
            return Err(format!(
                "requested {query_name} for a document that is not open"
            ));
        };
        let target_path = normalize_path(&target_doc.path);

        Ok(find_definition_location(
            &artifact.session,
            &artifact.hovers,
            &artifact.semantic_entries,
            &target_path,
            &position,
            snapshot.uri_by_normalized_path(),
        ))
    }

    #[cfg(test)]
    pub fn references(
        &self,
        uri: &str,
        position: impl IntoIdePosition,
        include_declaration: bool,
    ) -> Result<Vec<IdeLocation>, String> {
        let snapshot = self.snapshot(Vec::new(), CancellationToken::new());
        self.references_in_snapshot(&snapshot, uri, position, include_declaration)
    }

    pub fn references_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        position: impl IntoIdePosition,
        include_declaration: bool,
    ) -> Result<Vec<IdeLocation>, String> {
        let position = position.into_ide_position();
        if self
            .semantic_query_offset(snapshot, uri, &position)?
            .is_none()
        {
            return Ok(Vec::new());
        }
        let artifact = self
            .analyze_interactive_navigation_artifact_for_snapshot(snapshot, uri)
            .map_err(|message| format!("references analysis failed: {message}"))?;
        let Some(target_doc) = snapshot.document(uri) else {
            return Err("requested references for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let Some(definition_span) = navigation_definition_span_for_position(
            &artifact.session,
            &artifact.hovers,
            &artifact.semantic_entries,
            &target_path,
            &position,
        ) else {
            return Ok(Vec::new());
        };

        let Some(definition_key) = span_identity_key(&artifact.session, definition_span) else {
            return find_reference_locations_cancelable(
                ReferenceLocationQuery {
                    session: &artifact.session,
                    hovers: &artifact.hovers,
                    definition_links: &artifact.definition_links,
                    semantic_entries: &artifact.semantic_entries,
                    target_path: &target_path,
                    position: &position,
                    include_declaration,
                    uri_by_path: snapshot.uri_by_normalized_path(),
                },
                &snapshot.cancellation,
            );
        };

        if let Some(workspace_locations) = self.workspace_reference_locations(
            snapshot,
            target_doc,
            &definition_key,
            include_declaration,
        )? {
            return Ok(workspace_locations);
        }

        find_reference_locations_for_definition_cancelable(
            KnownReferenceLocationQuery {
                session: &artifact.session,
                definition_links: &artifact.definition_links,
                semantic_entries: &artifact.semantic_entries,
                definition_span,
                include_declaration,
                uri_by_path: snapshot.uri_by_normalized_path(),
            },
            &snapshot.cancellation,
        )
    }

    fn workspace_reference_locations(
        &self,
        snapshot: &AnalysisSnapshot,
        target_doc: &OpenDocument,
        definition_key: &SpanIdentityKey,
        include_declaration: bool,
    ) -> Result<Option<Vec<IdeLocation>>, String> {
        let Some(project) = self.project_for_path(&target_doc.path)? else {
            return Ok(None);
        };
        let targets = project
            .workspace_targets(&self.settings.compile_options)
            .map_err(|err| {
                format!(
                    "workspace references project analysis failed for `{}`: {err}",
                    project.manifest_path().display()
                )
            })?;
        if targets.len() <= 1 {
            return Ok(None);
        }

        let mut locations = Vec::new();
        let mut seen_contexts = BTreeSet::new();
        for resolved in targets {
            snapshot.check_canceled()?;
            let context = self.analysis_context_for_resolved_and_dirty(
                resolved,
                snapshot.dirty_documents(),
                snapshot.cancellation.clone(),
            )?;
            if !seen_contexts.insert(context.cache_key.clone()) {
                continue;
            }
            let artifact = self
                .analyze_interactive_navigation_artifact_for_context(&context)
                .map_err(|message| format!("workspace references analysis failed: {message}"))?;
            let Some(definition_span) = find_definition_span_by_identity_key_cancelable(
                &artifact.session,
                &artifact.semantic_entries,
                definition_key,
                &snapshot.cancellation,
            )?
            else {
                continue;
            };
            let target_locations = find_reference_locations_for_definition_cancelable(
                KnownReferenceLocationQuery {
                    session: &artifact.session,
                    definition_links: &artifact.definition_links,
                    semantic_entries: &artifact.semantic_entries,
                    definition_span,
                    include_declaration,
                    uri_by_path: snapshot.uri_by_normalized_path(),
                },
                &snapshot.cancellation,
            )?;
            locations.extend(target_locations);
        }

        locations.sort_by(workspace_location_order);
        locations.dedup();
        Ok(Some(locations))
    }

    pub fn implementation_locations_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        position: impl IntoIdePosition,
    ) -> Result<Vec<IdeLocation>, String> {
        let position = position.into_ide_position();
        if self
            .semantic_query_offset(snapshot, uri, &position)?
            .is_none()
        {
            return Ok(Vec::new());
        }
        let artifact = self
            .analyze_interactive_navigation_artifact_for_snapshot(snapshot, uri)
            .map_err(|message| format!("implementation analysis failed: {message}"))?;
        let Some(target_doc) = snapshot.document(uri) else {
            return Err("requested implementation for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);

        Ok(find_implementation_locations(
            &artifact.session,
            &artifact.hovers,
            &artifact.definition_links,
            &artifact.semantic_entries,
            &target_path,
            &position,
            snapshot.uri_by_normalized_path(),
        ))
    }

    pub fn prepare_call_hierarchy_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        position: impl IntoIdePosition,
    ) -> Result<Option<IdeCallHierarchyItem>, String> {
        let position = position.into_ide_position();
        if self
            .semantic_query_offset(snapshot, uri, &position)?
            .is_none()
        {
            return Ok(None);
        }
        let artifact = self
            .analyze_interactive_navigation_artifact_for_snapshot(snapshot, uri)
            .map_err(|message| format!("call hierarchy analysis failed: {message}"))?;
        let Some(target_doc) = snapshot.document(uri) else {
            return Err("requested call hierarchy for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);

        Ok(find_call_hierarchy_item(
            &artifact.session,
            &artifact.hovers,
            &artifact.semantic_entries,
            &target_path,
            &position,
            snapshot.uri_by_normalized_path(),
        ))
    }

    pub fn call_hierarchy_incoming_calls_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        item_uri: &str,
        item_range: impl IntoIdeRange,
    ) -> Result<Vec<IdeCallHierarchyIncomingCall>, String> {
        let item_range = item_range.into_ide_range();
        let artifact = self
            .analyze_interactive_navigation_artifact_for_snapshot(snapshot, item_uri)
            .map_err(|message| format!("call hierarchy analysis failed: {message}"))?;

        Ok(find_call_hierarchy_incoming_calls(
            &artifact.session,
            &artifact.semantic_entries,
            &artifact.calls,
            item_uri,
            &item_range,
            snapshot.uri_by_normalized_path(),
        ))
    }

    pub fn call_hierarchy_outgoing_calls_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        item_uri: &str,
        item_range: impl IntoIdeRange,
    ) -> Result<Vec<IdeCallHierarchyOutgoingCall>, String> {
        let item_range = item_range.into_ide_range();
        let artifact = self
            .analyze_interactive_navigation_artifact_for_snapshot(snapshot, item_uri)
            .map_err(|message| format!("call hierarchy analysis failed: {message}"))?;

        Ok(find_call_hierarchy_outgoing_calls(
            &artifact.session,
            &artifact.semantic_entries,
            &artifact.calls,
            item_uri,
            &item_range,
            snapshot.uri_by_normalized_path(),
        ))
    }

    pub fn goto_type_definition_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        position: impl IntoIdePosition,
    ) -> Result<Option<IdeLocation>, String> {
        let position = position.into_ide_position();
        if self
            .semantic_query_offset(snapshot, uri, &position)?
            .is_none()
        {
            return Ok(None);
        }
        let artifact = self
            .analyze_interactive_navigation_artifact_for_snapshot(snapshot, uri)
            .map_err(|message| format!("type definition analysis failed: {message}"))?;
        let Some(target_doc) = snapshot.document(uri) else {
            return Err("requested type definition for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);

        Ok(find_type_definition_location(
            &artifact.session,
            &artifact.semantic_entries,
            &target_path,
            &position,
            snapshot.uri_by_normalized_path(),
        ))
    }

    #[cfg(test)]
    pub fn document_highlights(
        &self,
        uri: &str,
        position: impl IntoIdePosition,
    ) -> Result<Vec<IdeDocumentHighlight>, String> {
        let snapshot = self.snapshot(Vec::new(), CancellationToken::new());
        self.document_highlights_in_snapshot(&snapshot, uri, position)
    }

    pub fn document_highlights_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        position: impl IntoIdePosition,
    ) -> Result<Vec<IdeDocumentHighlight>, String> {
        let position = position.into_ide_position();
        if self
            .semantic_query_offset(snapshot, uri, &position)?
            .is_none()
        {
            return Ok(Vec::new());
        }
        let artifact = self
            .analyze_interactive_navigation_artifact_for_snapshot(snapshot, uri)
            .map_err(|message| format!("document highlights analysis failed: {message}"))?;
        let Some(target_doc) = snapshot.document(uri) else {
            return Err(
                "requested document highlights for a document that is not open".to_string(),
            );
        };
        let target_path = normalize_path(&target_doc.path);

        Ok(find_document_highlights(
            &artifact.session,
            &artifact.definition_links,
            &artifact.semantic_entries,
            &artifact.hovers,
            &target_path,
            &position,
        ))
    }

    #[cfg(test)]
    pub fn hover(
        &self,
        uri: &str,
        position: impl IntoIdePosition,
    ) -> Result<Option<IdeHover>, String> {
        let snapshot = self.snapshot(Vec::new(), CancellationToken::new());
        self.hover_in_snapshot(&snapshot, uri, position)
    }

    pub fn hover_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        position: impl IntoIdePosition,
    ) -> Result<Option<IdeHover>, String> {
        let position = position.into_ide_position();
        if self
            .semantic_query_offset(snapshot, uri, &position)?
            .is_none()
        {
            return Ok(None);
        }
        let artifact = self
            .analyze_interactive_navigation_artifact_for_snapshot(snapshot, uri)
            .map_err(|message| format!("hover analysis failed: {message}"))?;
        let Some(target_doc) = snapshot.document(uri) else {
            return Err("requested hover for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);

        Ok(find_hover(
            &artifact.session,
            &artifact.hovers,
            &artifact.semantic_entries,
            &target_path,
            &position,
        ))
    }

    #[cfg(test)]
    pub fn signature_help(
        &self,
        uri: &str,
        position: impl IntoIdePosition,
    ) -> Result<Option<IdeSignatureHelp>, String> {
        let snapshot = self.snapshot(Vec::new(), CancellationToken::new());
        self.signature_help_in_snapshot(&snapshot, uri, position)
    }

    pub fn signature_help_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        position: impl IntoIdePosition,
    ) -> Result<Option<IdeSignatureHelp>, String> {
        let position = position.into_ide_position();
        let Some(offset) = self.semantic_query_offset(snapshot, uri, &position)? else {
            return Ok(None);
        };
        let artifact = self
            .analyze_interactive_artifact_for_snapshot(snapshot, uri)
            .map_err(|message| format!("signature help analysis failed: {message}"))?;
        let Some(target_doc) = snapshot.document(uri) else {
            return Err("requested signature help for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);

        Ok(artifact
            .signature_help(&target_path, offset)
            .map(analysis_signature_help_to_ide_help))
    }

    #[cfg(test)]
    pub fn completion(
        &self,
        uri: &str,
        position: impl IntoIdePosition,
    ) -> Result<Vec<IdeCompletionItem>, String> {
        let snapshot = self.snapshot(Vec::new(), CancellationToken::new());
        self.completion_in_snapshot(&snapshot, uri, position)
    }

    pub fn completion_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        position: impl IntoIdePosition,
    ) -> Result<Vec<IdeCompletionItem>, String> {
        let position = position.into_ide_position();
        let Some(target_doc) = snapshot.document(uri) else {
            return Err("requested completion for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let file = snapshot
            .document_source_file(uri)
            .ok_or_else(|| "requested completion for a document that is not open".to_string())?;
        let Some(offset) = position_to_byte_offset(&file, &position) else {
            return Ok(Vec::new());
        };
        if self
            .lexical_index_for_document(uri, target_doc)
            .contains(offset)
        {
            self.record_analysis_tier(AnalysisTier::Lexical);
            return Ok(Vec::new());
        }
        let prefix = completion_prefix(&target_doc.text, offset);
        let has_call_paren = has_following_call_paren(&target_doc.text, offset);
        let context = completion_context(&target_doc.text, offset);
        let member_access = completion_is_member_access(&target_doc.text, offset);
        if member_access && !completion_member_access_has_receiver(&target_doc.text, offset) {
            self.record_analysis_tier(AnalysisTier::Lexical);
            return Ok(Vec::new());
        }
        if completion_is_binding_name_context(&target_doc.text, offset) {
            self.record_analysis_tier(AnalysisTier::Lexical);
            let mut labels = keyword_completion_labels(prefix, context, member_access);
            if labels.is_empty() {
                labels = fallback_keyword_completion_labels(context, member_access);
            }
            return Ok(labels.into_iter().map(keyword_completion_item).collect());
        }

        let analysis_context = self.resolve_analysis_context_for_snapshot(snapshot, uri)?;
        let is_dirty = !analysis_context.dirty_documents.is_clean();
        let surface = if is_dirty {
            self.analyze_clean_surface_for_context(&analysis_context)?
        } else {
            self.analyze_surface_artifact_for_context(&analysis_context)?
        };
        let mut items = if let Some(surface) = surface {
            if !surface.requires_body_completion(&target_path, offset) {
                self.record_analysis_tier(AnalysisTier::Surface);
                surface.completion_items(&target_path, offset)
            } else {
                let artifact = if is_dirty {
                    self.record_analysis_tier(AnalysisTier::CleanSemantic);
                    self.analyze_clean_artifact_for_context(&analysis_context)?
                } else {
                    self.record_analysis_tier(AnalysisTier::CleanSemantic);
                    self.analyze_artifact_for_context(&analysis_context)?
                };
                artifact.completion_items(&target_path, offset)
            }
        } else {
            let artifact = if is_dirty {
                self.record_analysis_tier(AnalysisTier::CleanSemantic);
                self.analyze_clean_artifact_for_context(&analysis_context)?
            } else {
                self.record_analysis_tier(AnalysisTier::CleanSemantic);
                self.analyze_artifact_for_context(&analysis_context)?
            };
            artifact.completion_items(&target_path, offset)
        };
        if !prefix.is_empty() {
            items.retain(|item| item.label.starts_with(prefix));
        }
        if has_call_paren {
            for item in &mut items {
                if item.insert_text.is_some() {
                    item.insert_text = None;
                }
            }
        }
        items.sort_by(|left, right| {
            completion_sort_key(left, prefix, context)
                .cmp(&completion_sort_key(right, prefix, context))
        });

        let mut completions = items
            .into_iter()
            .map(analysis_completion_to_ide_item)
            .collect::<Vec<_>>();
        for item in &mut completions {
            if item.documentation.is_some() {
                item.resolve_data = Some(CompletionResolveData {
                    uri: uri.to_string(),
                    version: target_doc.version,
                    position: position.clone().into(),
                    label: item.label.clone(),
                });
            }
        }
        let mut seen_labels = completions
            .iter()
            .map(|item| item.label.clone())
            .collect::<BTreeSet<_>>();
        for keyword in keyword_completion_labels(prefix, context, member_access) {
            if seen_labels.insert(keyword.to_string()) {
                completions.push(keyword_completion_item(keyword));
            }
        }

        Ok(completions)
    }

    pub fn resolve_completion_item_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        data: &CompletionResolveData,
    ) -> Result<Option<IdeCompletionItem>, String> {
        let Some(document) = snapshot.document(&data.uri) else {
            return Ok(None);
        };
        if document.version != data.version {
            return Ok(None);
        }
        let items = self.completion_in_snapshot(snapshot, &data.uri, data.position.clone())?;
        Ok(items.into_iter().find(|item| item.label == data.label))
    }

    #[cfg(test)]
    pub fn prepare_rename(
        &self,
        uri: &str,
        position: impl IntoIdePosition,
    ) -> Result<Option<IdePrepareRenameResult>, String> {
        let snapshot = self.snapshot(Vec::new(), CancellationToken::new());
        self.prepare_rename_in_snapshot(&snapshot, uri, position)
    }

    pub fn prepare_rename_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        position: impl IntoIdePosition,
    ) -> Result<Option<IdePrepareRenameResult>, String> {
        let position = position.into_ide_position();
        if self
            .semantic_query_offset(snapshot, uri, &position)?
            .is_none()
        {
            return Ok(None);
        }
        let artifact = self
            .analyze_interactive_navigation_artifact_for_snapshot(snapshot, uri)
            .map_err(|message| format!("prepareRename analysis failed: {message}"))?;
        let Some(target_doc) = snapshot.document(uri) else {
            return Err("requested prepareRename for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let Some(target) = find_rename_target(
            &artifact.session,
            &artifact.hovers,
            &artifact.semantic_entries,
            &target_path,
            &position,
        ) else {
            return Ok(None);
        };

        Ok(Some(IdePrepareRenameResult {
            range: span_to_range(&artifact.session, target.query_span).into(),
            placeholder: target.placeholder,
        }))
    }

    #[cfg(test)]
    pub fn rename(
        &self,
        uri: &str,
        position: impl IntoIdePosition,
        new_name: &str,
    ) -> Result<IdeWorkspaceEdit, String> {
        let snapshot = self.snapshot(Vec::new(), CancellationToken::new());
        self.rename_in_snapshot(&snapshot, uri, position, new_name)
    }

    pub fn rename_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        position: impl IntoIdePosition,
        new_name: &str,
    ) -> Result<IdeWorkspaceEdit, String> {
        let position = position.into_ide_position();
        if !is_valid_identifier(new_name) {
            return Err(format!("`{}` is not a valid Kern identifier", new_name));
        }
        if self
            .semantic_query_offset(snapshot, uri, &position)?
            .is_none()
        {
            return Err("rename target is not a supported identifier".to_string());
        }

        let artifact = self
            .analyze_interactive_navigation_artifact_for_snapshot(snapshot, uri)
            .map_err(|message| format!("rename analysis failed: {message}"))?;
        let Some(target_doc) = snapshot.document(uri) else {
            return Err("requested rename for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let Some(target) = find_rename_target(
            &artifact.session,
            &artifact.hovers,
            &artifact.semantic_entries,
            &target_path,
            &position,
        ) else {
            return Err("rename target is not a supported identifier".to_string());
        };

        let changes = build_rename_changes(
            &artifact.session,
            &artifact.definition_links,
            &artifact.semantic_entries,
            &target,
            new_name,
            snapshot.uri_by_normalized_path(),
        );

        Ok(IdeWorkspaceEdit { changes })
    }

    #[cfg(test)]
    pub fn semantic_tokens(&self, uri: &str) -> Result<IdeSemanticTokens, String> {
        let snapshot = self.snapshot(Vec::new(), CancellationToken::new());
        self.semantic_tokens_in_snapshot(&snapshot, uri)
    }

    pub fn semantic_tokens_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
    ) -> Result<IdeSemanticTokens, String> {
        let Some(target_doc) = snapshot.document(uri) else {
            return Err("requested semantic tokens for a document that is not open".to_string());
        };
        let file = snapshot.document_source_file(uri).ok_or_else(|| {
            "requested semantic tokens for a document that is not open".to_string()
        })?;

        let context = self.resolve_analysis_context_for_snapshot(snapshot, uri)?;
        let target_path = normalize_path(&target_doc.path);
        let token_key = SemanticTokensCacheKey {
            analysis: context.cache_key.clone(),
            target_path: target_path.clone(),
            document_version: target_doc.version,
        };
        if let Some(tokens) = self.semantic_tokens_cache.lock().unwrap().get(&token_key) {
            self.record_cache_hit(AnalysisCacheTraceKind::SemanticTokens);
            self.record_analysis_tier(if context.dirty_documents.is_clean() {
                AnalysisTier::CleanSemantic
            } else {
                AnalysisTier::Lexical
            });
            return Ok(tokens.clone());
        }
        self.record_cache_miss(AnalysisCacheTraceKind::SemanticTokens);

        let tokens = if !context.dirty_documents.is_clean() {
            self.record_analysis_tier(AnalysisTier::Lexical);
            semantic::lexical_semantic_tokens_cancelable(&file, &snapshot.cancellation)?
        } else {
            self.record_analysis_tier(AnalysisTier::CleanSemantic);
            let artifact = self.analyze_navigation_artifact_for_context(&context)?;
            semantic::semantic_tokens_cancelable(
                semantic::SemanticArtifactView {
                    session: &artifact.session,
                    symbols: &artifact.symbols,
                    references: &artifact.references,
                    hovers: &artifact.hovers,
                    semantic_entries: &artifact.semantic_entries,
                },
                &file,
                &target_path,
                &snapshot.cancellation,
            )?
        };
        self.semantic_tokens_cache
            .lock()
            .unwrap()
            .insert(token_key, tokens.clone());
        self.record_cache_store(AnalysisCacheTraceKind::SemanticTokens);
        Ok(tokens)
    }

    pub fn semantic_tokens_range_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        range: impl IntoIdeRange,
    ) -> Result<IdeSemanticTokens, String> {
        let range = range.into_ide_range();
        snapshot.check_canceled()?;
        let tokens = self.semantic_tokens_in_snapshot(snapshot, uri)?;
        semantic::filter_semantic_tokens_to_range_cancelable(
            &tokens,
            &range,
            &snapshot.cancellation,
        )
    }

    #[cfg(test)]
    pub fn inlay_hints(
        &self,
        uri: &str,
        range: impl IntoIdeRange,
    ) -> Result<Vec<IdeInlayHint>, String> {
        let snapshot = self.snapshot(Vec::new(), CancellationToken::new());
        self.inlay_hints_in_snapshot(&snapshot, uri, range)
    }

    pub fn inlay_hints_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        range: impl IntoIdeRange,
    ) -> Result<Vec<IdeInlayHint>, String> {
        let range = range.into_ide_range();
        let Some(target_doc) = snapshot.document(uri) else {
            return Err("requested inlay hints for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let artifact = self
            .analyze_interactive_navigation_artifact_for_snapshot(snapshot, uri)
            .map_err(|message| format!("inlay hint analysis failed: {message}"))?;

        self.record_analysis_tier(AnalysisTier::CleanSemantic);
        Ok(artifact
            .type_hints
            .iter()
            .filter_map(|hint| {
                let path = artifact
                    .session
                    .source_manager
                    .get_file_path(hint.span.file)?;
                (normalize_path(path) == target_path).then_some(hint)
            })
            .map(|hint| analysis_type_hint_to_ide_hint(&artifact.session, hint))
            .filter(|hint| {
                let hint_range = IdeRange {
                    start: hint.position.clone(),
                    end: hint.position.clone(),
                };
                ranges_overlap(&hint_range, &range)
            })
            .collect())
    }

    #[cfg(test)]
    pub fn code_actions(
        &self,
        uri: &str,
        range: impl IntoIdeRange,
    ) -> Result<Vec<IdeCodeAction>, String> {
        let snapshot = self.snapshot(Vec::new(), CancellationToken::new());
        self.code_actions_in_snapshot(&snapshot, uri, range)
    }

    pub fn code_actions_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        range: impl IntoIdeRange,
    ) -> Result<Vec<IdeCodeAction>, String> {
        self.code_actions_in_snapshot_with_deferred(snapshot, uri, range, true)
    }

    fn code_actions_in_snapshot_with_deferred(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        range: impl IntoIdeRange,
        defer_heavy_actions: bool,
    ) -> Result<Vec<IdeCodeAction>, String> {
        let range = range.into_ide_range();
        let analysis_context = self.resolve_analysis_context_for_snapshot(snapshot, uri)?;
        let Some(target_doc) = snapshot.document(uri) else {
            return Err("requested code actions for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let (diagnostics_session, artifact) = if analysis_context.dirty_documents.is_clean() {
            self.record_analysis_tier(AnalysisTier::CleanSemantic);
            let artifact = self.analyze_artifact_for_context(&analysis_context)?;
            (artifact.session.clone(), Some(artifact))
        } else {
            self.record_analysis_tier(AnalysisTier::ParseOnly);
            (
                self.parse_open_document_session_for_snapshot(snapshot, uri)?,
                None,
            )
        };

        let mut actions = Vec::new();
        let mut seen = BTreeSet::new();
        for diagnostic in &diagnostics_session.diagnostics {
            let Some(path) = diagnostics_session
                .source_manager
                .get_file_path(diagnostic.primary_span.file)
            else {
                continue;
            };
            if normalize_path(path) != target_path {
                continue;
            }

            let ide_diagnostic =
                convert_diagnostic_for_document(&diagnostics_session, diagnostic, target_doc);
            if !ide_ranges_overlap(&ide_diagnostic.range, &range) {
                continue;
            }

            let action = if let Some(artifact) = &artifact {
                quick_fix_for_diagnostic(uri, artifact, diagnostic, ide_diagnostic.clone())
            } else {
                lightweight_quick_fix_for_diagnostic(uri, diagnostic, ide_diagnostic.clone())
            };
            let Some(action) = action else {
                continue;
            };
            let action = if defer_heavy_actions {
                code_action_with_resolve_data(action, uri, target_doc.version, &range)
            } else {
                action
            };

            let edit_key = action
                .edit
                .as_ref()
                .map(workspace_edit_key)
                .unwrap_or_else(|| {
                    action
                        .resolve_data
                        .as_ref()
                        .map(code_action_resolve_key)
                        .unwrap_or_default()
                });
            let dedup_key = (action.title.clone(), action.fix_id, edit_key);
            if seen.insert(dedup_key) {
                actions.push(action);
            }
        }

        Ok(actions)
    }

    pub fn resolve_code_action_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        data: &CodeActionResolveData,
    ) -> Result<Option<IdeCodeAction>, String> {
        let Some(document) = snapshot.document(&data.uri) else {
            return Ok(None);
        };
        if document.version != data.version {
            return Ok(None);
        }
        let actions = self.code_actions_in_snapshot_with_deferred(
            snapshot,
            &data.uri,
            data.range.clone(),
            false,
        )?;
        Ok(actions.into_iter().find(|action| {
            action.fix_id == Some(data.fix_id.as_str())
                && action.kind == Some(data.action_kind.as_str())
                && action.diagnostics.first().is_some_and(|diagnostic| {
                    diagnostic.range == data.diagnostic_range
                        && diagnostic.code == data.diagnostic_code
                })
        }))
    }
}

fn code_action_with_resolve_data(
    mut action: IdeCodeAction,
    uri: &str,
    version: i64,
    request_range: &IdeRange,
) -> IdeCodeAction {
    if action.fix_id.is_some_and(defer_code_action_fix)
        && let Some(diagnostic) = action.diagnostics.first()
        && let Some(action_kind) = action.kind
    {
        let fix_id = action.fix_id.expect("checked above");
        action.resolve_data = Some(CodeActionResolveData {
            uri: uri.to_string(),
            version,
            range: request_range.clone().into(),
            diagnostic_range: diagnostic.range.clone().into(),
            diagnostic_code: diagnostic.code.clone(),
            action_kind: action_kind.to_string(),
            fix_id: fix_id.to_string(),
        });
        action.edit = None;
    }
    action
}

fn defer_code_action_fix(fix_id: &str) -> bool {
    matches!(
        fix_id,
        "change-let-mut"
            | "rename-unused-binding-to-underscore"
            | "remove-dead-assignment"
            | "make-private-item-public"
            | "add-match-catch-all"
            | "remove-irrefutable-let-else"
    )
}

fn code_action_resolve_key(data: &CodeActionResolveData) -> String {
    format!(
        "{}|{}|{}:{}:{}:{}|{}:{}:{}:{}|{}|{}",
        data.fix_id,
        data.version,
        data.range.start.line,
        data.range.start.character,
        data.range.end.line,
        data.range.end.character,
        data.diagnostic_range.start.line,
        data.diagnostic_range.start.character,
        data.diagnostic_range.end.line,
        data.diagnostic_range.end.character,
        data.diagnostic_code.as_deref().unwrap_or(""),
        data.action_kind
    )
}

fn workspace_symbol_order(
    lhs: &IdeWorkspaceSymbol,
    rhs: &IdeWorkspaceSymbol,
) -> std::cmp::Ordering {
    let lhs_range = &lhs.location.range;
    let rhs_range = &rhs.location.range;
    (
        lhs.name.as_str(),
        lhs.location.uri.as_str(),
        lhs_range.start.line,
        lhs_range.start.character,
        lhs_range.end.line,
        lhs_range.end.character,
    )
        .cmp(&(
            rhs.name.as_str(),
            rhs.location.uri.as_str(),
            rhs_range.start.line,
            rhs_range.start.character,
            rhs_range.end.line,
            rhs_range.end.character,
        ))
}

fn workspace_symbol_matches_query(symbol: &IdeWorkspaceSymbol, query: &str) -> bool {
    query.is_empty() || symbol.name.to_ascii_lowercase().contains(query)
}

fn surface_symbol_index_from_artifact_cancelable(
    surface: &AnalysisSurfaceArtifact,
    uri_by_path: &BTreeMap<PathBuf, String>,
    cancellation: &CancellationToken,
) -> Result<SurfaceSymbolIndex, String> {
    let mut document_symbols_by_path = BTreeMap::<PathBuf, Vec<IdeDocumentSymbol>>::new();
    let mut workspace_symbols = Vec::new();

    for module_symbol in &surface.symbols {
        cancellation
            .check()
            .map_err(|_| "request was canceled".to_string())?;
        if let Some(path) = surface
            .session
            .source_manager
            .get_file_path(module_symbol.span.file)
        {
            let path = normalize_path(path);
            document_symbols_by_path.entry(path).or_default().extend(
                module_symbol
                    .children
                    .iter()
                    .map(|symbol| analysis_symbol_to_document_symbol(&surface.session, symbol)),
            );
        }
        analysis_symbol_to_workspace_symbols_cancelable(
            &surface.session,
            module_symbol,
            None,
            uri_by_path,
            &mut workspace_symbols,
            cancellation,
        )?;
    }

    cancellation
        .check()
        .map_err(|_| "request was canceled".to_string())?;
    workspace_symbols.sort_by(workspace_symbol_order);
    workspace_symbols.dedup_by(workspace_symbol_same_location);

    Ok(SurfaceSymbolIndex {
        document_symbols_by_path: document_symbols_by_path
            .into_iter()
            .map(|(path, symbols)| (path, Arc::new(symbols)))
            .collect(),
        workspace_symbols: Arc::new(workspace_symbols),
    })
}

fn workspace_symbol_same_location(
    lhs: &mut IdeWorkspaceSymbol,
    rhs: &mut IdeWorkspaceSymbol,
) -> bool {
    lhs.name == rhs.name && lhs.kind == rhs.kind && lhs.location == rhs.location
}

fn first_line_end_character(file: &SourceFile) -> u32 {
    let first_line = file.src.lines().next().unwrap_or("");
    first_line.encode_utf16().count() as u32
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SpanIdentityKey {
    path: PathBuf,
    start: usize,
    end: usize,
}

fn span_identity_key(session: &Session, span: Span) -> Option<SpanIdentityKey> {
    let path = session.source_manager.get_file_path(span.file)?;
    Some(SpanIdentityKey {
        path: normalize_path(path),
        start: span.start,
        end: span.end,
    })
}

fn find_definition_span_by_identity_key_cancelable(
    session: &Session,
    semantic_entries: &[kernc_driver::AnalysisSemanticEntry],
    key: &SpanIdentityKey,
    cancellation: &CancellationToken,
) -> Result<Option<Span>, String> {
    semantic_entries.iter().try_fold(None, |found, entry| {
        if found.is_some() {
            return Ok(found);
        }
        cancellation
            .check()
            .map_err(|_| "request was canceled".to_string())?;
        Ok(span_identity_key(session, entry.definition_span)
            .filter(|candidate| candidate == key)
            .map(|_| entry.definition_span))
    })
}

fn workspace_location_order(lhs: &IdeLocation, rhs: &IdeLocation) -> std::cmp::Ordering {
    let lhs_range = &lhs.range;
    let rhs_range = &rhs.range;
    (
        lhs.uri.as_str(),
        lhs_range.start.line,
        lhs_range.start.character,
        lhs_range.end.line,
        lhs_range.end.character,
    )
        .cmp(&(
            rhs.uri.as_str(),
            rhs_range.start.line,
            rhs_range.start.character,
            rhs_range.end.line,
            rhs_range.end.character,
        ))
}
