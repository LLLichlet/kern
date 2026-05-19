//! Lightweight semantic index for editor-facing references and definitions.
//!
//! The index records only valid source spans so downstream tools do not need to
//! filter synthetic builtin spans, recovery placeholders, or unloaded files.

use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Default)]
pub(crate) struct SemanticIndexState {
    identifier_references: Vec<(Span, Span)>,
    semantic_definitions: BTreeMap<Span, SemanticDefinition>,
}

impl<'a> SemaContext<'a> {
    pub fn record_identifier_reference(&mut self, reference_span: Span, definition_span: Span) {
        if reference_span.end <= reference_span.start
            || self
                .sess
                .source_manager
                .get_file(reference_span.file)
                .is_none()
        {
            // Builtins and synthesized nodes often have default spans.  Skip
            // them so reference queries stay source-backed.
            return;
        }

        self.analysis
            .semantic_index
            .identifier_references
            .push((reference_span, definition_span));
    }

    pub fn identifier_references(&self) -> &[(Span, Span)] {
        &self.analysis.semantic_index.identifier_references
    }

    pub fn record_symbol_definition(
        &mut self,
        span: Span,
        kind: SemanticSymbolKind,
        is_mut: bool,
        is_pub: bool,
    ) {
        if span.end <= span.start || self.sess.source_manager.get_file(span.file).is_none() {
            return;
        }

        // Keep the first definition recorded for a span.  Multiple semantic
        // passes can observe the same source binding from different angles.
        self.analysis
            .semantic_index
            .semantic_definitions
            .entry(span)
            .or_insert(SemanticDefinition {
                span,
                kind,
                is_mut,
                is_pub,
            });
    }

    pub fn semantic_definitions(&self) -> impl Iterator<Item = &SemanticDefinition> {
        self.analysis.semantic_index.semantic_definitions.values()
    }
}
