use super::*;
use crate::compiler::AnalysisFlowResolvedUseKind;

pub(super) fn collect_node_facts(
    cfg: &AnalysisFlowCfg,
    node_uses: &[Vec<AnalysisFlowBindingId>],
    node_defs: &[Vec<AnalysisFlowBindingId>],
    node_def_kinds: &[Option<AnalysisFlowDefinitionKind>],
) -> Vec<AnalysisFlowNodeFacts> {
    cfg.nodes
        .iter()
        .map(|node| AnalysisFlowNodeFacts {
            node_id: node.id,
            use_binding_ids: sort_binding_ids(&node_uses[node.id.index()]),
            define_binding_ids: sort_binding_ids(&node_defs[node.id.index()]),
            definition_kind: node_def_kinds[node.id.index()],
        })
        .collect()
}

pub(super) fn collect_definition_facts(
    node_facts: &[AnalysisFlowNodeFacts],
    node_value_uses: &[Vec<AnalysisFlowBindingId>],
    node_copy_sources: &[Option<AnalysisFlowBindingId>],
) -> Vec<AnalysisFlowDefinitionFacts> {
    let mut definition_facts = Vec::new();

    for facts in node_facts {
        let Some(kind) = facts.definition_kind else {
            continue;
        };

        for binding_id in &facts.define_binding_ids {
            definition_facts.push(AnalysisFlowDefinitionFacts {
                definition: AnalysisFlowDefinitionRef {
                    binding_id: *binding_id,
                    node_id: facts.node_id,
                },
                kind,
                use_binding_ids: sort_binding_ids(&node_value_uses[facts.node_id.index()]),
                copy_source_binding_id: node_copy_sources[facts.node_id.index()],
            });
        }
    }

    definition_facts.sort_by_key(|facts| (facts.definition.binding_id, facts.definition.node_id));
    definition_facts
}

pub(super) fn collect_node_transfers(
    node_facts: &[AnalysisFlowNodeFacts],
) -> Vec<AnalysisFlowNodeTransfer> {
    node_facts
        .iter()
        .map(|facts| AnalysisFlowNodeTransfer {
            node_id: facts.node_id,
            use_binding_ids: facts.use_binding_ids.clone(),
            kill_binding_ids: facts.define_binding_ids.clone(),
            generate_definitions: facts
                .define_binding_ids
                .iter()
                .copied()
                .map(|binding_id| AnalysisFlowDefinitionRef {
                    binding_id,
                    node_id: facts.node_id,
                })
                .collect(),
        })
        .collect()
}

pub(super) fn collect_use_defs(
    node_facts: &[AnalysisFlowNodeFacts],
    reaching_definitions: &[AnalysisFlowReaching],
) -> Vec<AnalysisFlowUseDef> {
    let reaching_by_node_id = reaching_definitions
        .iter()
        .map(|state| (state.node_id, state))
        .collect::<HashMap<_, _>>();

    let mut use_defs = Vec::new();
    for facts in node_facts {
        let Some(reaching) = reaching_by_node_id.get(&facts.node_id).copied() else {
            continue;
        };

        for binding_id in &facts.use_binding_ids {
            let mut reaching_for_binding = reaching
                .reaching_in
                .iter()
                .filter(|definition| definition.binding_id == *binding_id)
                .cloned()
                .collect::<Vec<_>>();
            reaching_for_binding.sort_by_key(|definition| definition.node_id);

            use_defs.push(AnalysisFlowUseDef {
                node_id: facts.node_id,
                binding_id: *binding_id,
                reaching_definitions: reaching_for_binding,
            });
        }
    }

    use_defs.sort_by_key(|use_def| (use_def.node_id, use_def.binding_id));
    use_defs
}

pub(super) fn collect_def_uses(
    node_transfers: &[AnalysisFlowNodeTransfer],
    use_defs: &[AnalysisFlowUseDef],
) -> Vec<AnalysisFlowDefUse> {
    let mut def_use_map = node_transfers
        .iter()
        .flat_map(|transfer| transfer.generate_definitions.iter().cloned())
        .map(|definition| (definition, Vec::<AnalysisFlowNodeId>::new()))
        .collect::<HashMap<_, _>>();

    for use_def in use_defs {
        for reaching_definition in &use_def.reaching_definitions {
            let Some(use_node_ids) = def_use_map.get_mut(reaching_definition) else {
                continue;
            };
            if use_node_ids.last() != Some(&use_def.node_id) {
                use_node_ids.push(use_def.node_id);
            }
        }
    }

    let mut def_uses = def_use_map
        .into_iter()
        .map(|(definition, mut use_node_ids)| {
            use_node_ids.sort();
            use_node_ids.dedup();
            AnalysisFlowDefUse {
                definition,
                use_node_ids,
            }
        })
        .collect::<Vec<_>>();
    def_uses.sort_by_key(|def_use| (def_use.definition.binding_id, def_use.definition.node_id));
    def_uses
}

pub(super) fn collect_resolved_uses(
    use_defs: &[AnalysisFlowUseDef],
) -> Vec<AnalysisFlowResolvedUse> {
    let mut resolved_uses = use_defs
        .iter()
        .map(|use_def| {
            let kind = match use_def.reaching_definitions.len() {
                0 => AnalysisFlowResolvedUseKind::Missing,
                1 => AnalysisFlowResolvedUseKind::Unique,
                _ => AnalysisFlowResolvedUseKind::Ambiguous,
            };

            AnalysisFlowResolvedUse {
                node_id: use_def.node_id,
                binding_id: use_def.binding_id,
                kind,
                candidate_definitions: use_def.reaching_definitions.clone(),
            }
        })
        .collect::<Vec<_>>();
    resolved_uses.sort_by_key(|resolved| (resolved.node_id, resolved.binding_id));
    resolved_uses
}

pub(super) fn collect_single_source_uses(
    resolved_uses: &[AnalysisFlowResolvedUse],
    definition_facts: &[AnalysisFlowDefinitionFacts],
) -> Vec<AnalysisFlowSingleSourceUse> {
    let definition_by_ref = definition_facts
        .iter()
        .map(|facts| (facts.definition, facts))
        .collect::<HashMap<_, _>>();

    let mut single_source_uses = Vec::new();
    for resolved in resolved_uses {
        if resolved.kind != AnalysisFlowResolvedUseKind::Unique {
            continue;
        }
        let Some(definition) = resolved.candidate_definitions.first().copied() else {
            continue;
        };
        let Some(definition_facts) = definition_by_ref.get(&definition).copied() else {
            continue;
        };

        single_source_uses.push(AnalysisFlowSingleSourceUse {
            node_id: resolved.node_id,
            binding_id: resolved.binding_id,
            definition,
            definition_kind: definition_facts.kind,
            copy_source_binding_id: definition_facts.copy_source_binding_id,
        });
    }

    single_source_uses.sort_by_key(|single| (single.node_id, single.binding_id));
    single_source_uses
}

pub(super) fn compute_reaching_definitions(
    cfg: &AnalysisFlowCfg,
    node_transfers: &[AnalysisFlowNodeTransfer],
) -> Vec<AnalysisFlowReaching> {
    let mut predecessors = vec![Vec::<AnalysisFlowNodeId>::new(); cfg.nodes.len()];
    for edge in &cfg.edges {
        predecessors[edge.to.index()].push(edge.from);
    }

    let mut reaching_in =
        vec![HashSet::<(AnalysisFlowBindingId, AnalysisFlowNodeId)>::new(); cfg.nodes.len()];
    let mut reaching_out =
        vec![HashSet::<(AnalysisFlowBindingId, AnalysisFlowNodeId)>::new(); cfg.nodes.len()];

    let mut changed = true;
    while changed {
        changed = false;
        for node in &cfg.nodes {
            let node_index = node.id.index();

            let mut next_in = HashSet::new();
            for predecessor in &predecessors[node_index] {
                next_in.extend(reaching_out[predecessor.index()].iter().copied());
            }

            let kill_bindings = node_transfers[node_index]
                .kill_binding_ids
                .iter()
                .copied()
                .collect::<HashSet<_>>();
            let mut next_out = next_in
                .iter()
                .copied()
                .filter(|(binding_id, _)| !kill_bindings.contains(binding_id))
                .collect::<HashSet<_>>();
            next_out.extend(
                node_transfers[node_index]
                    .generate_definitions
                    .iter()
                    .map(|definition| (definition.binding_id, definition.node_id)),
            );

            if next_in != reaching_in[node_index] || next_out != reaching_out[node_index] {
                reaching_in[node_index] = next_in;
                reaching_out[node_index] = next_out;
                changed = true;
            }
        }
    }

    cfg.nodes
        .iter()
        .map(|node| AnalysisFlowReaching {
            node_id: node.id,
            reaching_in: sort_definition_refs(&reaching_in[node.id.index()]),
            reaching_out: sort_definition_refs(&reaching_out[node.id.index()]),
        })
        .collect()
}

pub(super) fn collect_binding_summaries(
    bindings: &[FlowBindingFacts],
    cfg: &AnalysisFlowCfg,
    node_facts: &[AnalysisFlowNodeFacts],
    liveness: &[AnalysisFlowLiveness],
) -> Vec<AnalysisFlowBindingSummary> {
    let liveness_by_node_id = liveness
        .iter()
        .map(|state| (state.node_id, state))
        .collect::<HashMap<_, _>>();

    bindings
        .iter()
        .map(|binding| {
            let mut definition_node_ids = cfg
                .nodes
                .iter()
                .filter(|node| {
                    node_facts[node.id.index()]
                        .define_binding_ids
                        .contains(&binding.id)
                })
                .map(|node| node.id)
                .collect::<Vec<_>>();
            definition_node_ids.sort();

            let mut use_node_ids = cfg
                .nodes
                .iter()
                .filter(|node| {
                    node_facts[node.id.index()]
                        .use_binding_ids
                        .contains(&binding.id)
                })
                .map(|node| node.id)
                .collect::<Vec<_>>();
            use_node_ids.sort();

            let mut live_node_ids = cfg
                .nodes
                .iter()
                .filter_map(|node| {
                    let state = liveness_by_node_id.get(&node.id)?;
                    (state.live_in.contains(&binding.id) || state.live_out.contains(&binding.id))
                        .then_some(node.id)
                })
                .collect::<Vec<_>>();
            live_node_ids.sort();

            AnalysisFlowBindingSummary {
                binding_id: binding.id,
                definition_node_ids,
                use_node_ids,
                live_node_ids,
            }
        })
        .collect()
}

pub(super) fn compute_liveness(
    cfg: &AnalysisFlowCfg,
    node_transfers: &[AnalysisFlowNodeTransfer],
) -> Vec<AnalysisFlowLiveness> {
    let mut successors = vec![Vec::<AnalysisFlowNodeId>::new(); cfg.nodes.len()];
    for edge in &cfg.edges {
        successors[edge.from.index()].push(edge.to);
    }

    let mut live_in = vec![HashSet::<AnalysisFlowBindingId>::new(); cfg.nodes.len()];
    let mut live_out = vec![HashSet::<AnalysisFlowBindingId>::new(); cfg.nodes.len()];

    let mut changed = true;
    while changed {
        changed = false;
        for node_index in (0..cfg.nodes.len()).rev() {
            let mut next_out = HashSet::new();
            for successor in &successors[node_index] {
                next_out.extend(live_in[successor.index()].iter().copied());
            }

            let mut next_in = node_transfers[node_index]
                .use_binding_ids
                .iter()
                .copied()
                .collect::<HashSet<_>>();
            for value in &next_out {
                if !node_transfers[node_index].kill_binding_ids.contains(value) {
                    next_in.insert(*value);
                }
            }

            if next_out != live_out[node_index] || next_in != live_in[node_index] {
                live_out[node_index] = next_out;
                live_in[node_index] = next_in;
                changed = true;
            }
        }
    }

    cfg.nodes
        .iter()
        .map(|node| {
            let mut node_live_in = live_in[node.id.index()].iter().copied().collect::<Vec<_>>();
            node_live_in.sort();
            let mut node_live_out = live_out[node.id.index()]
                .iter()
                .copied()
                .collect::<Vec<_>>();
            node_live_out.sort();
            AnalysisFlowLiveness {
                node_id: node.id,
                live_in: node_live_in,
                live_out: node_live_out,
            }
        })
        .collect()
}

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
            let liveness_by_node_id = owner
                .liveness
                .iter()
                .map(|state| (state.node_id, state))
                .collect::<HashMap<_, _>>();
            for node in &owner.cfg.nodes {
                let Some(facts) = owner.node_facts.get(node.id.index()) else {
                    continue;
                };
                let Some(kind) = facts.definition_kind else {
                    continue;
                };
                let Some(liveness) = liveness_by_node_id.get(&node.id).copied() else {
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

fn sort_binding_ids(values: &[AnalysisFlowBindingId]) -> Vec<AnalysisFlowBindingId> {
    let mut ids = values.to_vec();
    ids.sort();
    ids.dedup();
    ids
}

fn maybe_push_dead_store(
    node: &AnalysisFlowCfgNode,
    binding: &FlowBindingFacts,
    kind: AnalysisFlowDefinitionKind,
    liveness: &AnalysisFlowLiveness,
    ctx: &SemaContext<'_>,
    dead_stores: &mut Vec<AnalysisDeadStore>,
) {
    if binding.kind == AnalysisFlowBindingKind::Static || binding.reference_spans.is_empty() {
        return;
    }
    if liveness.live_out.contains(&binding.id) {
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

fn sort_definition_refs(
    values: &HashSet<(AnalysisFlowBindingId, AnalysisFlowNodeId)>,
) -> Vec<AnalysisFlowDefinitionRef> {
    let mut refs = values
        .iter()
        .copied()
        .map(|(binding_id, node_id)| AnalysisFlowDefinitionRef {
            binding_id,
            node_id,
        })
        .collect::<Vec<_>>();
    refs.sort_by_key(|reference| (reference.binding_id, reference.node_id));
    refs
}
