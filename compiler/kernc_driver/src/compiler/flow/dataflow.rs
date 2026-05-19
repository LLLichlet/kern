//! Flow dataflow-derived diagnostics.
//!
//! This module uses reaching definitions and liveness to identify dead stores
//! and other flow-sensitive facts that are easier to report after CFG analysis.

use super::*;
use std::collections::{HashMap, HashSet};

impl FlowModel {
    pub(in crate::compiler) fn collect_dead_stores(
        &self,
        ctx: &SemaContext<'_>,
        _references: &[(Span, Span)],
    ) -> Vec<AnalysisDeadStore> {
        let mut dead_stores = Vec::new();

        for owner in &self.owners {
            let binding_by_id = owner
                .bindings
                .iter()
                .map(|binding| (binding.id, binding))
                .collect::<HashMap<_, _>>();
            for node in &owner.cfg.nodes {
                let Some(facts) = owner.node_facts.get(node.id.index()) else {
                    continue;
                };
                let Some(kind) = facts.definition_kind else {
                    continue;
                };
                let Some(liveness) = owner.computed_liveness.as_ref() else {
                    continue;
                };

                for binding_id in &facts.define_binding_ids {
                    let Some(binding) = binding_by_id.get(binding_id).copied() else {
                        continue;
                    };
                    maybe_push_dead_store(node, binding, kind, liveness, ctx, &mut dead_stores);
                }
            }
        }

        let mut seen = HashSet::new();
        dead_stores.retain(|store| seen.insert((store.span, store.binding_id, store.kind)));
        dead_stores
    }
}

fn maybe_push_dead_store(
    node: &AnalysisFlowCfgNode,
    binding: &FlowBindingFacts,
    kind: AnalysisFlowDefinitionKind,
    liveness: &ComputedLiveness,
    ctx: &SemaContext<'_>,
    dead_stores: &mut Vec<AnalysisDeadStore>,
) {
    if binding.kind == AnalysisFlowBindingKind::Static || binding.reference_spans.is_empty() {
        return;
    }
    if liveness.live_out_contains(node.id, binding.id) {
        return;
    }

    let name = ctx
        .sess
        .source_manager
        .slice_source(binding.definition_span)
        .trim()
        .to_string();
    if name.is_empty() || name == "_" {
        return;
    }

    dead_stores.push(AnalysisDeadStore {
        span: node.span,
        node_id: node.id,
        binding_id: binding.id,
        binding_definition_span: binding.definition_span,
        kind: dead_store_kind(kind),
        name,
    });
}

fn dead_store_kind(kind: AnalysisFlowDefinitionKind) -> AnalysisDeadStoreKind {
    match kind {
        AnalysisFlowDefinitionKind::Initializer => AnalysisDeadStoreKind::Initializer,
        AnalysisFlowDefinitionKind::Assignment => AnalysisDeadStoreKind::Assignment,
    }
}
