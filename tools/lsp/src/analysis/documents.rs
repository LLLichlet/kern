use super::*;

impl AnalysisEngine {
    #[cfg(test)]
    pub fn open_document(&mut self, params: DidOpenTextDocumentParams) -> AnalysisOutcome {
        match self.open_document_state(params) {
            DocumentSyncAction::ScheduleTarget(uri) => self.analyze_document(&uri),
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

        self.documents.insert(
            uri.clone(),
            OpenDocument {
                is_dirty: Self::document_differs_from_disk(&path, &doc.text),
                text_hash: hash_source_text(&doc.text),
                path,
                version: doc.version,
                text: doc.text,
            },
        );
        self.invalidate_open_path_index();
        self.invalidate_dirty_document_snapshot();
        self.invalidate_render_caches();

        DocumentSyncAction::ScheduleTarget(uri)
    }

    #[cfg(test)]
    pub fn change_document(&mut self, params: DidChangeTextDocumentParams) -> AnalysisOutcome {
        match self.change_document_state(params) {
            DocumentSyncAction::ScheduleTarget(uri) => self.analyze_document(&uri),
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

        DocumentSyncAction::ScheduleTarget(params.text_document.uri)
    }

    #[cfg(test)]
    pub fn close_document(&mut self, params: DidCloseTextDocumentParams) -> AnalysisOutcome {
        match self.close_document_state(params) {
            DocumentSyncAction::ScheduleTarget(uri) => self.analyze_document(&uri),
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

    pub fn refresh_workspace(&mut self) -> Vec<(String, AnalysisOutcome)> {
        self.project_cache.get_mut().clear();
        self.driver_cache.get_mut().clear();
        self.invalidate_artifact_cache();
        self.invalidate_render_caches();
        self.documents
            .keys()
            .cloned()
            .map(|uri| {
                let outcome = self.analyze_document(&uri);
                (uri, outcome)
            })
            .collect()
    }

    pub fn document_uris(&self) -> Vec<String> {
        self.documents.keys().cloned().collect()
    }

    pub fn analyze_document_uri(&self, target_uri: &str) -> AnalysisOutcome {
        self.analyze_document(target_uri)
    }
}
