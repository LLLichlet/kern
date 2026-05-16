use super::*;
use std::collections::BTreeSet;

impl AnalysisEngine {
    fn semantic_query_offset(
        &self,
        uri: &str,
        position: &Position,
    ) -> Result<Option<usize>, String> {
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested semantic query for a document that is not open".to_string());
        };
        let file = kernc_utils::SourceFile::new(target_doc.path.clone(), target_doc.text.clone());
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

    pub fn document_symbols(&self, uri: &str) -> Result<Vec<IdeDocumentSymbol>, String> {
        let context = self.resolve_analysis_context(uri)?;
        let surface =
            if context.dirty_documents.is_clean() || !context.resolved.input_file.is_file() {
                self.analyze_surface_artifact(uri)
                    .ok()
                    .or_else(|| self.analyze_clean_surface_for_context(&context))
            } else {
                self.analyze_clean_surface_for_context(&context)
            };
        let Some(surface) = surface else {
            return Ok(Vec::new());
        };
        self.record_analysis_tier(AnalysisTier::Surface);

        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested document symbols for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);

        let mut symbols = Vec::new();
        for module_symbol in &surface.symbols {
            let Some(path) = surface
                .session
                .source_manager
                .get_file_path(module_symbol.span.file)
            else {
                continue;
            };
            if normalize_path(path) == target_path {
                symbols.extend(
                    module_symbol
                        .children
                        .iter()
                        .map(|symbol| analysis_symbol_to_document_symbol(&surface.session, symbol)),
                );
            }
        }

        Ok(symbols)
    }

    pub fn goto_definition(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<Option<IdeLocation>, String> {
        if self.semantic_query_offset(uri, &position)?.is_none() {
            return Ok(None);
        }
        let artifact = self
            .analyze_interactive_navigation_artifact(uri)
            .map_err(|message| format!("definition analysis failed: {message}"))?;
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested definition for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let uri_by_path = self.uri_by_normalized_path();

        Ok(find_definition_location(
            &artifact.session,
            &artifact.hovers,
            &artifact.semantic_entries,
            &target_path,
            &position,
            &uri_by_path,
        ))
    }

    pub fn references(
        &self,
        uri: &str,
        position: Position,
        include_declaration: bool,
    ) -> Result<Vec<IdeLocation>, String> {
        if self.semantic_query_offset(uri, &position)?.is_none() {
            return Ok(Vec::new());
        }
        let artifact = self
            .analyze_interactive_navigation_artifact(uri)
            .map_err(|message| format!("references analysis failed: {message}"))?;
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested references for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let uri_by_path = self.uri_by_normalized_path();

        Ok(find_reference_locations(ReferenceLocationQuery {
            session: &artifact.session,
            hovers: &artifact.hovers,
            definition_links: &artifact.definition_links,
            semantic_entries: &artifact.semantic_entries,
            target_path: &target_path,
            position: &position,
            include_declaration,
            uri_by_path: &uri_by_path,
        }))
    }

    pub fn document_highlights(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<Vec<IdeDocumentHighlight>, String> {
        if self.semantic_query_offset(uri, &position)?.is_none() {
            return Ok(Vec::new());
        }
        let artifact = self
            .analyze_interactive_navigation_artifact(uri)
            .map_err(|message| format!("document highlights analysis failed: {message}"))?;
        let Some(target_doc) = self.documents.get(uri) else {
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

    pub fn hover(&self, uri: &str, position: Position) -> Result<Option<IdeHover>, String> {
        if self.semantic_query_offset(uri, &position)?.is_none() {
            return Ok(None);
        }
        let artifact = self
            .analyze_interactive_navigation_artifact(uri)
            .map_err(|message| format!("hover analysis failed: {message}"))?;
        let Some(target_doc) = self.documents.get(uri) else {
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

    pub fn signature_help(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<Option<IdeSignatureHelp>, String> {
        let Some(offset) = self.semantic_query_offset(uri, &position)? else {
            return Ok(None);
        };
        let artifact = self
            .analyze_interactive_artifact(uri)
            .map_err(|message| format!("signature help analysis failed: {message}"))?;
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested signature help for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);

        Ok(artifact
            .signature_help(&target_path, offset)
            .map(analysis_signature_help_to_ide_help))
    }

    pub fn completion(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<Vec<IdeCompletionItem>, String> {
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested completion for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let file = kernc_utils::SourceFile::new(target_doc.path.clone(), target_doc.text.clone());
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

        let analysis_context = self.resolve_analysis_context(uri)?;
        let is_dirty = !analysis_context.dirty_documents.is_clean();
        let surface = if is_dirty {
            self.analyze_clean_surface_for_context(&analysis_context)
        } else {
            self.analyze_surface_artifact(uri).ok()
        };
        let mut items = if let Some(surface) = surface {
            if !surface.requires_body_completion(&target_path, offset) {
                self.record_analysis_tier(AnalysisTier::Surface);
                surface.completion_items(&target_path, offset)
            } else {
                let artifact = if is_dirty {
                    self.record_analysis_tier(AnalysisTier::CleanSemantic);
                    self.analyze_clean_artifact_for_context(&analysis_context)
                } else {
                    self.record_analysis_tier(AnalysisTier::CleanSemantic);
                    self.analyze_artifact_for_context(&analysis_context)
                };
                artifact.completion_items(&target_path, offset)
            }
        } else {
            let artifact = if is_dirty {
                self.record_analysis_tier(AnalysisTier::CleanSemantic);
                self.analyze_clean_artifact_for_context(&analysis_context)
            } else {
                self.record_analysis_tier(AnalysisTier::CleanSemantic);
                self.analyze_artifact_for_context(&analysis_context)
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

    pub fn prepare_rename(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<Option<IdePrepareRenameResult>, String> {
        if self.semantic_query_offset(uri, &position)?.is_none() {
            return Ok(None);
        }
        let artifact = self
            .analyze_interactive_navigation_artifact(uri)
            .map_err(|message| format!("prepareRename analysis failed: {message}"))?;
        let Some(target_doc) = self.documents.get(uri) else {
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
            range: span_to_range(&artifact.session, target.query_span),
            placeholder: target.placeholder,
        }))
    }

    pub fn rename(
        &self,
        uri: &str,
        position: Position,
        new_name: &str,
    ) -> Result<IdeWorkspaceEdit, String> {
        if !is_valid_identifier(new_name) {
            return Err(format!("`{}` is not a valid Kern identifier", new_name));
        }
        if self.semantic_query_offset(uri, &position)?.is_none() {
            return Err("rename target is not a supported identifier".to_string());
        }

        let artifact = self
            .analyze_interactive_navigation_artifact(uri)
            .map_err(|message| format!("rename analysis failed: {message}"))?;
        let Some(target_doc) = self.documents.get(uri) else {
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
        let uri_by_path = self.uri_by_normalized_path();

        let changes = build_rename_changes(
            &artifact.session,
            &artifact.definition_links,
            &artifact.semantic_entries,
            &target,
            new_name,
            &uri_by_path,
        );

        Ok(IdeWorkspaceEdit { changes })
    }

    pub fn semantic_tokens(&self, uri: &str) -> Result<IdeSemanticTokens, String> {
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested semantic tokens for a document that is not open".to_string());
        };
        let file = kernc_utils::SourceFile::new(target_doc.path.clone(), target_doc.text.clone());

        let context = self.resolve_analysis_context(uri)?;
        let target_path = normalize_path(&target_doc.path);
        let token_key = SemanticTokensCacheKey {
            analysis: context.cache_key.clone(),
            target_path: target_path.clone(),
            document_version: target_doc.version,
        };
        if let Some(tokens) = self.semantic_tokens_cache.borrow().get(&token_key) {
            return Ok(tokens.clone());
        }

        let tokens = if !context.dirty_documents.is_clean() {
            self.record_analysis_tier(AnalysisTier::Lexical);
            semantic::lexical_semantic_tokens(&file)
        } else {
            self.record_analysis_tier(AnalysisTier::CleanSemantic);
            let artifact = self.analyze_navigation_artifact_for_context(&context);
            semantic::semantic_tokens(
                semantic::SemanticArtifactView {
                    session: &artifact.session,
                    symbols: &artifact.symbols,
                    references: &artifact.references,
                    hovers: &artifact.hovers,
                    semantic_entries: &artifact.semantic_entries,
                },
                &file,
                &target_path,
            )
        };
        self.semantic_tokens_cache
            .borrow_mut()
            .insert(token_key, tokens.clone());
        Ok(tokens)
    }

    pub fn inlay_hints(&self, uri: &str, range: Range) -> Result<Vec<IdeInlayHint>, String> {
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested inlay hints for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let artifact = self
            .analyze_interactive_navigation_artifact(uri)
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
                let hint_range = Range {
                    start: hint.position.clone(),
                    end: hint.position.clone(),
                };
                ranges_overlap(&hint_range, &range)
            })
            .collect())
    }

    pub fn code_actions(&self, uri: &str, range: Range) -> Result<Vec<IdeCodeAction>, String> {
        let analysis_context = self.resolve_analysis_context(uri)?;
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested code actions for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let (diagnostics_session, artifact) = if analysis_context.dirty_documents.is_clean() {
            self.record_analysis_tier(AnalysisTier::CleanSemantic);
            let artifact = self.analyze_artifact_for_context(&analysis_context);
            (artifact.session.clone(), Some(artifact))
        } else {
            self.record_analysis_tier(AnalysisTier::ParseOnly);
            (self.parse_open_document_session(uri)?, None)
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
            if !ranges_overlap(&ide_diagnostic.range, &range) {
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

            let edit_key = action
                .edit
                .as_ref()
                .map(workspace_edit_key)
                .unwrap_or_default();
            let dedup_key = (action.title.clone(), edit_key);
            if seen.insert(dedup_key) {
                actions.push(action);
            }
        }

        Ok(actions)
    }
}
