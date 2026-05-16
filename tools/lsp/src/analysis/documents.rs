use super::*;
use std::path::Path;

impl AnalysisEngine {
    #[cfg(test)]
    pub fn open_document(&mut self, params: DidOpenTextDocumentParams) -> AnalysisOutcome {
        match self.open_document_state(params) {
            DocumentSyncAction::ScheduleTarget { uri, mode } => match mode {
                DiagnosticsAnalysisMode::Structure => self.analyze_document_structure(&uri),
                DiagnosticsAnalysisMode::Full => self.analyze_document(&uri),
            },
            DocumentSyncAction::Immediate(outcome) => outcome,
        }
    }

    pub fn open_document_state(&mut self, params: DidOpenTextDocumentParams) -> DocumentSyncAction {
        let doc = params.text_document;
        let uri = doc.uri.clone();
        let Some(path) = uri_to_analysis_path(&uri) else {
            return DocumentSyncAction::Immediate(single_server_diagnostic(
                uri,
                "only file:// and untitled: URIs are supported",
            ));
        };

        let is_dirty = Self::document_differs_from_disk(&path, &doc.text);
        self.documents.insert(
            uri.clone(),
            OpenDocument {
                is_dirty,
                text_hash: hash_source_text(&doc.text),
                path,
                version: doc.version,
                text: doc.text,
            },
        );
        self.invalidate_open_path_index();
        self.invalidate_dirty_document_snapshot();
        self.invalidate_render_caches();

        DocumentSyncAction::ScheduleTarget {
            uri,
            mode: DiagnosticsAnalysisMode::Structure,
        }
    }

    #[cfg(test)]
    pub fn change_document(&mut self, params: DidChangeTextDocumentParams) -> AnalysisOutcome {
        match self.change_document_state(params) {
            DocumentSyncAction::ScheduleTarget { uri, mode } => match mode {
                DiagnosticsAnalysisMode::Structure => self.analyze_document_structure(&uri),
                DiagnosticsAnalysisMode::Full => self.analyze_document(&uri),
            },
            DocumentSyncAction::Immediate(outcome) => outcome,
        }
    }

    pub fn change_document_state(
        &mut self,
        params: DidChangeTextDocumentParams,
    ) -> DocumentSyncAction {
        let Some(doc) = self.documents.get_mut(&params.text_document.uri) else {
            return DocumentSyncAction::Immediate(single_server_diagnostic(
                params.text_document.uri,
                "received didChange for a document that is not open",
            ));
        };

        let mut updated_text = doc.text.clone();
        for change in params.content_changes {
            if let Err(message) = apply_content_change(&doc.path, &mut updated_text, &change) {
                return DocumentSyncAction::Immediate(single_server_diagnostic(
                    params.text_document.uri.clone(),
                    message,
                ));
            }
        }

        doc.text = updated_text;
        doc.version = params.text_document.version;
        doc.is_dirty = Self::document_differs_from_disk(&doc.path, &doc.text);
        doc.text_hash = hash_source_text(&doc.text);
        self.invalidate_dirty_document_snapshot();
        self.invalidate_render_caches();

        DocumentSyncAction::ScheduleTarget {
            uri: params.text_document.uri,
            mode: DiagnosticsAnalysisMode::Structure,
        }
    }

    #[cfg(test)]
    pub fn close_document(&mut self, params: DidCloseTextDocumentParams) -> AnalysisOutcome {
        match self.close_document_state(params) {
            DocumentSyncAction::ScheduleTarget { uri, mode } => match mode {
                DiagnosticsAnalysisMode::Structure => self.analyze_document_structure(&uri),
                DiagnosticsAnalysisMode::Full => self.analyze_document(&uri),
            },
            DocumentSyncAction::Immediate(outcome) => outcome,
        }
    }

    pub fn close_document_state(
        &mut self,
        params: DidCloseTextDocumentParams,
    ) -> DocumentSyncAction {
        let _was_dirty = self
            .documents
            .remove(&params.text_document.uri)
            .map(|doc| doc.is_dirty)
            .unwrap_or(false);
        self.invalidate_open_path_index();
        self.invalidate_dirty_document_snapshot();
        self.invalidate_render_caches();
        DocumentSyncAction::Immediate(AnalysisOutcome {
            bundles: vec![DiagnosticBundle {
                uri: params.text_document.uri,
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

        let is_dirty = Self::document_differs_from_disk(&doc.path, &doc.text);
        doc.is_dirty = is_dirty;
        self.invalidate_dirty_document_snapshot();

        DocumentSyncAction::ScheduleTarget {
            uri,
            mode: if is_dirty {
                DiagnosticsAnalysisMode::Structure
            } else {
                DiagnosticsAnalysisMode::Full
            },
        }
    }

    pub fn reload_project_metadata_targets(&mut self) -> Vec<(String, DiagnosticsAnalysisMode)> {
        self.project_cache.lock().unwrap().clear();
        self.driver_cache.lock().unwrap().clear();
        self.refresh_workspace_targets()
    }

    pub fn refresh_workspace_targets(&mut self) -> Vec<(String, DiagnosticsAnalysisMode)> {
        self.driver_cache.lock().unwrap().clear();
        self.invalidate_artifact_cache();
        self.invalidate_render_caches();
        self.documents
            .iter()
            .map(|(uri, document)| {
                let mode = if document.is_dirty {
                    DiagnosticsAnalysisMode::Structure
                } else {
                    DiagnosticsAnalysisMode::Full
                };
                (uri.clone(), mode)
            })
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
