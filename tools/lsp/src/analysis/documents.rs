use super::*;
use std::path::Path;

impl AnalysisEngine {
    #[cfg(test)]
    pub fn open_document(&mut self, params: impl IntoIdeOpenDocument) -> AnalysisOutcome {
        match self.open_document_state(params.into_ide_open_document()) {
            DocumentSyncAction::ScheduleTarget { uri, mode, .. } => match mode {
                DiagnosticsAnalysisMode::Structure => self.analyze_document_structure(&uri),
                DiagnosticsAnalysisMode::Full => self.analyze_document(&uri),
            },
            DocumentSyncAction::Immediate(outcome) => outcome,
        }
    }

    pub fn open_document_state(&mut self, doc: impl IntoIdeOpenDocument) -> DocumentSyncAction {
        let doc = doc.into_ide_open_document();
        let uri = doc.uri.clone();
        let Some(path) = uri_to_analysis_path(&uri) else {
            return DocumentSyncAction::Immediate(single_server_diagnostic(
                uri,
                "only file:// and untitled: URIs are supported",
            ));
        };

        let is_dirty = Self::document_differs_from_disk(&path, &doc.text);
        let text_hash = hash_source_text(&doc.text);
        self.documents.insert(
            uri.clone(),
            OpenDocument {
                is_dirty,
                text_hash,
                path: path.clone(),
                version: doc.version,
                text: doc.text,
            },
        );
        self.invalidate_open_path_index();
        self.invalidate_dirty_document_snapshot();
        self.invalidate_lexical_cache_for_document(&uri);
        self.retain_semantic_tokens_for_document_text(&path, text_hash);

        DocumentSyncAction::ScheduleTarget {
            uri,
            mode: DiagnosticsAnalysisMode::Structure,
            prewarm: !is_dirty,
        }
    }

    #[cfg(test)]
    pub fn change_document(&mut self, params: impl IntoIdeChangeDocument) -> AnalysisOutcome {
        match self.change_document_state(params.into_ide_change_document()) {
            DocumentSyncAction::ScheduleTarget { uri, mode, .. } => match mode {
                DiagnosticsAnalysisMode::Structure => self.analyze_document_structure(&uri),
                DiagnosticsAnalysisMode::Full => self.analyze_document(&uri),
            },
            DocumentSyncAction::Immediate(outcome) => outcome,
        }
    }

    pub fn change_document_state(
        &mut self,
        params: impl IntoIdeChangeDocument,
    ) -> DocumentSyncAction {
        let params = params.into_ide_change_document();
        let Some(doc) = self.documents.get_mut(&params.uri) else {
            return DocumentSyncAction::Immediate(single_server_diagnostic(
                params.uri,
                "received didChange for a document that is not open",
            ));
        };

        let mut updated_text = doc.text.clone();
        for change in params.changes {
            if let Err(message) = apply_content_change(&doc.path, &mut updated_text, &change) {
                return DocumentSyncAction::Immediate(single_server_diagnostic(
                    params.uri, message,
                ));
            }
        }

        doc.text = updated_text;
        doc.version = params.version;
        doc.is_dirty = Self::document_differs_from_disk(&doc.path, &doc.text);
        doc.text_hash = hash_source_text(&doc.text);
        let path = doc.path.clone();
        self.invalidate_dirty_document_snapshot();
        self.invalidate_render_caches_for_document(&params.uri, &path);

        DocumentSyncAction::ScheduleTarget {
            uri: params.uri,
            mode: DiagnosticsAnalysisMode::Structure,
            prewarm: false,
        }
    }

    #[cfg(test)]
    pub fn close_document(&mut self, params: impl IntoIdeCloseDocument) -> AnalysisOutcome {
        match self.close_document_state(params.into_ide_close_document()) {
            DocumentSyncAction::ScheduleTarget { uri, mode, .. } => match mode {
                DiagnosticsAnalysisMode::Structure => self.analyze_document_structure(&uri),
                DiagnosticsAnalysisMode::Full => self.analyze_document(&uri),
            },
            DocumentSyncAction::Immediate(outcome) => outcome,
        }
    }

    pub fn close_document_state(
        &mut self,
        params: impl IntoIdeCloseDocument,
    ) -> DocumentSyncAction {
        let params = params.into_ide_close_document();
        let closed_document = self.documents.remove(&params.uri);
        self.invalidate_open_path_index();
        self.invalidate_dirty_document_snapshot();
        if closed_document.is_some() {
            self.invalidate_lexical_cache_for_document(&params.uri);
        }
        DocumentSyncAction::Immediate(AnalysisOutcome {
            bundles: vec![DiagnosticBundle {
                uri: params.uri,
                diagnostics: Vec::new(),
            }],
        })
    }

    pub fn save_document_state(&mut self, uri: String) -> DocumentSyncAction {
        let Some(doc) = self.documents.get_mut(&uri) else {
            return DocumentSyncAction::Immediate(single_server_diagnostic(
                uri,
                "received didSave for a document that is not open",
            ));
        };

        let was_dirty = doc.is_dirty;
        let is_dirty = Self::document_differs_from_disk(&doc.path, &doc.text);
        doc.is_dirty = is_dirty;
        self.invalidate_dirty_document_snapshot();
        if was_dirty && !is_dirty {
            self.driver_cache.lock().unwrap().clear();
            self.invalidate_artifact_cache();
        }

        DocumentSyncAction::ScheduleTarget {
            uri,
            mode: if is_dirty {
                DiagnosticsAnalysisMode::Structure
            } else {
                DiagnosticsAnalysisMode::Full
            },
            prewarm: false,
        }
    }

    #[cfg(test)]
    pub fn reload_project_metadata_targets(&mut self) -> Vec<(String, DiagnosticsAnalysisMode)> {
        self.project_cache.lock().unwrap().clear();
        self.driver_cache.lock().unwrap().clear();
        self.refresh_workspace_targets()
    }

    pub(crate) fn reload_project_metadata_index_cancelable(
        &mut self,
        workspace_roots: Vec<PathBuf>,
        cancellation: CancellationToken,
    ) -> Result<WorkspaceIndexRefresh, String> {
        self.project_cache.lock().unwrap().clear();
        self.driver_cache.lock().unwrap().clear();
        self.refresh_workspace_index_cancelable(workspace_roots, cancellation)
    }

    #[cfg(test)]
    pub fn refresh_workspace_targets(&mut self) -> Vec<(String, DiagnosticsAnalysisMode)> {
        self.driver_cache.lock().unwrap().clear();
        self.invalidate_artifact_cache();
        self.invalidate_render_caches();
        self.workspace_refresh_targets()
    }

    pub(crate) fn refresh_workspace_index_cancelable(
        &mut self,
        workspace_roots: Vec<PathBuf>,
        cancellation: CancellationToken,
    ) -> Result<WorkspaceIndexRefresh, String> {
        self.driver_cache.lock().unwrap().clear();
        self.invalidate_artifact_cache();
        self.invalidate_render_caches();
        if cancellation.is_canceled() {
            return Err("request was canceled".to_string());
        }
        let targets = self.workspace_refresh_targets();
        let (indexed_targets, failed_targets) =
            self.warm_workspace_symbol_indexes_cancelable(workspace_roots, cancellation)?;
        let stats = self.finish_workspace_index_refresh(indexed_targets, failed_targets);
        Ok(WorkspaceIndexRefresh {
            targets,
            indexed_targets: stats.indexed_targets,
            failed_targets: stats.failed_targets,
            generation: stats.generation,
        })
    }

    fn workspace_refresh_targets(&self) -> Vec<(String, DiagnosticsAnalysisMode)> {
        self.documents
            .iter()
            .map(|(uri, _document)| (uri.clone(), DiagnosticsAnalysisMode::Structure))
            .collect()
    }

    pub fn watched_files_require_project_reload(uris: &[String]) -> bool {
        uris.iter()
            .filter_map(|uri| uri_to_file_path(uri))
            .any(|path| Self::watched_path_requires_project_reload(&path))
    }

    fn watched_path_requires_project_reload(path: &Path) -> bool {
        let file_name = path.file_name().and_then(|name| name.to_str());
        if matches!(file_name, Some("Craft.toml" | "Craft.lock")) {
            return true;
        }

        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "analysis.toml")
            && path
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == ".craft")
    }

    pub fn document_uris(&self) -> Vec<String> {
        self.documents.keys().cloned().collect()
    }

    pub fn analyze_document_uri(&self, target_uri: &str) -> AnalysisOutcome {
        self.analyze_document(target_uri)
    }

    pub fn analyze_document_structure_uri(&self, target_uri: &str) -> AnalysisOutcome {
        self.analyze_document_structure(target_uri)
    }
}
