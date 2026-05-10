use super::*;
use std::collections::BTreeSet;

impl AnalysisEngine {
    pub fn document_symbols(&self, uri: &str) -> Result<Vec<DocumentSymbol>, String> {
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
    ) -> Result<Option<Location>, String> {
        let artifact = match self.analyze_interactive_artifact(uri) {
            Ok(artifact) => artifact,
            Err(_) => return Ok(None),
        };
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
    ) -> Result<Vec<Location>, String> {
        let artifact = match self.analyze_interactive_artifact(uri) {
            Ok(artifact) => artifact,
            Err(_) => return Ok(Vec::new()),
        };
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
    ) -> Result<Vec<DocumentHighlight>, String> {
        let artifact = match self.analyze_interactive_artifact(uri) {
            Ok(artifact) => artifact,
            Err(_) => return Ok(Vec::new()),
        };
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

    pub fn hover(&self, uri: &str, position: Position) -> Result<Option<Hover>, String> {
        let artifact = match self.analyze_interactive_artifact(uri) {
            Ok(artifact) => artifact,
            Err(_) => return Ok(None),
        };
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
    ) -> Result<Option<SignatureHelp>, String> {
        let artifact = match self.analyze_interactive_artifact(uri) {
            Ok(artifact) => artifact,
            Err(_) => return Ok(None),
        };
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested signature help for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let file = kernc_utils::SourceFile::new(target_doc.path.clone(), target_doc.text.clone());
        let Some(offset) = position_to_byte_offset(&file, &position) else {
            return Ok(None);
        };

        Ok(artifact
            .signature_help(&target_path, offset)
            .map(analysis_signature_help_to_lsp_help))
    }

    pub fn completion(&self, uri: &str, position: Position) -> Result<Vec<CompletionItem>, String> {
        let Some(target_doc) = self.documents.get(uri) else {
            return Err("requested completion for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);
        let file = kernc_utils::SourceFile::new(target_doc.path.clone(), target_doc.text.clone());
        let Some(offset) = position_to_byte_offset(&file, &position) else {
            return Ok(Vec::new());
        };
        if completion_is_in_comment_or_literal(&target_doc.text, offset) {
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
            .map(analysis_completion_to_lsp_item)
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
    ) -> Result<Option<PrepareRenameResult>, String> {
        let artifact = match self.analyze_interactive_artifact(uri) {
            Ok(artifact) => artifact,
            Err(_) => return Ok(None),
        };
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

        Ok(Some(PrepareRenameResult {
            range: span_to_range(&artifact.session, target.query_span),
            placeholder: target.placeholder,
        }))
    }

    pub fn rename(
        &self,
        uri: &str,
        position: Position,
        new_name: &str,
    ) -> Result<WorkspaceEdit, String> {
        if !is_valid_identifier(new_name) {
            return Err(format!("`{}` is not a valid Kern identifier", new_name));
        }

        let artifact = self
            .analyze_interactive_artifact(uri)
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

        Ok(WorkspaceEdit { changes })
    }

    pub fn semantic_tokens(&self, uri: &str) -> Result<SemanticTokens, String> {
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

        let tokens = if !context.dirty_documents.is_clean()
            || !self
                .artifact_cache
                .borrow()
                .contains_key(&AnalysisCacheKey::clean(&context.resolved))
        {
            self.record_analysis_tier(AnalysisTier::Lexical);
            semantic::lexical_semantic_tokens(&file)
        } else {
            self.record_analysis_tier(AnalysisTier::CleanSemantic);
            let artifact = self.analyze_artifact_for_context(&context);
            semantic::semantic_tokens(&artifact, &file, &target_path)
        };
        self.semantic_tokens_cache
            .borrow_mut()
            .insert(token_key, tokens.clone());
        Ok(tokens)
    }

    pub fn code_actions(&self, uri: &str, range: Range) -> Result<Vec<CodeAction>, String> {
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

            let lsp_diagnostic =
                convert_diagnostic_for_document(&diagnostics_session, diagnostic, target_doc);
            if !ranges_overlap(&lsp_diagnostic.range, &range) {
                continue;
            }

            let action = if let Some(artifact) = &artifact {
                quick_fix_for_diagnostic(uri, artifact, diagnostic, lsp_diagnostic.clone())
            } else {
                lightweight_quick_fix_for_diagnostic(uri, diagnostic, lsp_diagnostic.clone())
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
