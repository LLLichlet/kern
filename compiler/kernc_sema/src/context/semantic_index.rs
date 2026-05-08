use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Default)]
pub(crate) struct SemanticIndexState {
    identifier_references: Vec<(Span, Span)>,
    semantic_definitions: BTreeMap<Span, SemanticDefinition>,
}

impl SemanticIndexState {
    pub(crate) fn clear(&mut self) {
        self.identifier_references.clear();
        self.semantic_definitions.clear();
    }
}

impl<'a> SemaContext<'a> {
    pub fn record_identifier_reference(&mut self, reference_span: Span, definition_span: Span) {
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
