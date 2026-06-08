//! Data-flow algorithms over `AnalysisFlowCfg`.
//!
//! The public crate exposes materialized vectors for tools, but these
//! algorithms operate on dense bitsets for speed.  Every long loop has a
//! cancellation check because LSP analysis may abandon work between edits.

use crate::{
    AnalysisFlowBindingId, AnalysisFlowBindingSummary, AnalysisFlowCfg, AnalysisFlowDefUse,
    AnalysisFlowDefinitionFacts, AnalysisFlowDefinitionKind, AnalysisFlowDefinitionRef,
    AnalysisFlowLiveness, AnalysisFlowNodeFacts, AnalysisFlowNodeId, AnalysisFlowNodeTransfer,
    AnalysisFlowReaching, AnalysisFlowResolvedUse, AnalysisFlowResolvedUseKind,
    AnalysisFlowSingleSourceUse, AnalysisFlowUseDef,
};
use kernc_utils::{Canceled, CancellationToken, expect_uncancelable};
use std::collections::HashMap;

pub struct ComputedReaching {
    domain: Vec<AnalysisFlowDefinitionRef>,
    binding_definition_indices: HashMap<AnalysisFlowBindingId, Vec<usize>>,
    reaching_in: Vec<DenseBitSet>,
    reaching_out: Vec<DenseBitSet>,
}

#[derive(Clone)]
pub struct ComputedLiveness {
    live_in: Vec<DenseBitSet>,
    live_out: Vec<DenseBitSet>,
}

impl ComputedLiveness {
    pub fn live_out_contains(
        &self,
        node_id: AnalysisFlowNodeId,
        binding_id: AnalysisFlowBindingId,
    ) -> bool {
        self.live_out
            .get(node_id.index())
            .is_some_and(|live_out| live_out.contains(binding_id.index()))
    }
}

pub struct CfgTopology {
    predecessors: Vec<Vec<AnalysisFlowNodeId>>,
    successors: Vec<Vec<AnalysisFlowNodeId>>,
}

impl CfgTopology {
    pub fn from_cfg(cfg: &AnalysisFlowCfg) -> Self {
        expect_uncancelable(
            Self::from_cfg_cancelable(cfg, &CancellationToken::new()),
            "building CFG topology",
        )
    }

    pub fn from_cfg_cancelable(
        cfg: &AnalysisFlowCfg,
        cancellation: &CancellationToken,
    ) -> Result<Self, Canceled> {
        let mut predecessors = vec![Vec::<AnalysisFlowNodeId>::new(); cfg.nodes.len()];
        let mut successors = vec![Vec::<AnalysisFlowNodeId>::new(); cfg.nodes.len()];
        for edge in &cfg.edges {
            cancellation.check()?;
            // Node IDs are dense indices by construction, so adjacency lists can
            // be addressed directly without an intermediate map.
            predecessors[edge.to.index()].push(edge.from);
            successors[edge.from.index()].push(edge.to);
        }

        Ok(Self {
            predecessors,
            successors,
        })
    }
}

pub fn collect_node_facts(
    cfg: &AnalysisFlowCfg,
    node_uses: &[Vec<AnalysisFlowBindingId>],
    node_defs: &[Vec<AnalysisFlowBindingId>],
    node_def_kinds: &[Option<AnalysisFlowDefinitionKind>],
) -> Vec<AnalysisFlowNodeFacts> {
    let result = collect_node_facts_cancelable(
        cfg,
        node_uses,
        node_defs,
        node_def_kinds,
        &CancellationToken::new(),
    );
    expect_uncancelable(result, "collecting flow node facts")
}

pub fn collect_node_facts_cancelable(
    cfg: &AnalysisFlowCfg,
    node_uses: &[Vec<AnalysisFlowBindingId>],
    node_defs: &[Vec<AnalysisFlowBindingId>],
    node_def_kinds: &[Option<AnalysisFlowDefinitionKind>],
    cancellation: &CancellationToken,
) -> Result<Vec<AnalysisFlowNodeFacts>, Canceled> {
    let mut facts = Vec::with_capacity(cfg.nodes.len());
    for node in &cfg.nodes {
        cancellation.check()?;
        facts.push(AnalysisFlowNodeFacts {
            node_id: node.id,
            use_binding_ids: sort_binding_ids(&node_uses[node.id.index()]),
            define_binding_ids: sort_binding_ids(&node_defs[node.id.index()]),
            definition_kind: node_def_kinds[node.id.index()],
        });
    }
    Ok(facts)
}

pub fn collect_definition_facts(
    node_facts: &[AnalysisFlowNodeFacts],
    node_value_uses: &[Vec<AnalysisFlowBindingId>],
    node_copy_sources: &[Option<AnalysisFlowBindingId>],
) -> Vec<AnalysisFlowDefinitionFacts> {
    let result = collect_definition_facts_cancelable(
        node_facts,
        node_value_uses,
        node_copy_sources,
        &CancellationToken::new(),
    );
    expect_uncancelable(result, "collecting flow definition facts")
}

pub fn collect_definition_facts_cancelable(
    node_facts: &[AnalysisFlowNodeFacts],
    node_value_uses: &[Vec<AnalysisFlowBindingId>],
    node_copy_sources: &[Option<AnalysisFlowBindingId>],
    cancellation: &CancellationToken,
) -> Result<Vec<AnalysisFlowDefinitionFacts>, Canceled> {
    let mut definition_facts = Vec::new();

    for facts in node_facts {
        cancellation.check()?;
        let Some(kind) = facts.definition_kind else {
            continue;
        };

        for binding_id in &facts.define_binding_ids {
            cancellation.check()?;
            // A definition remembers the value-side uses of the same node; this
            // lets later passes identify copy-forwarding opportunities.
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
    Ok(definition_facts)
}

pub fn collect_node_transfers(
    node_facts: &[AnalysisFlowNodeFacts],
) -> Vec<AnalysisFlowNodeTransfer> {
    expect_uncancelable(
        collect_node_transfers_cancelable(node_facts, &CancellationToken::new()),
        "collecting flow node transfers",
    )
}

pub fn collect_node_transfers_cancelable(
    node_facts: &[AnalysisFlowNodeFacts],
    cancellation: &CancellationToken,
) -> Result<Vec<AnalysisFlowNodeTransfer>, Canceled> {
    let mut transfers = Vec::with_capacity(node_facts.len());
    for facts in node_facts {
        cancellation.check()?;
        transfers.push(AnalysisFlowNodeTransfer {
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
        });
    }
    Ok(transfers)
}

pub fn collect_use_defs(
    node_facts: &[AnalysisFlowNodeFacts],
    reaching: &ComputedReaching,
) -> Vec<AnalysisFlowUseDef> {
    expect_uncancelable(
        collect_use_defs_cancelable(node_facts, reaching, &CancellationToken::new()),
        "collecting flow use-def links",
    )
}

pub fn collect_use_defs_cancelable(
    node_facts: &[AnalysisFlowNodeFacts],
    reaching: &ComputedReaching,
    cancellation: &CancellationToken,
) -> Result<Vec<AnalysisFlowUseDef>, Canceled> {
    let mut use_defs = Vec::new();
    for facts in node_facts {
        cancellation.check()?;
        for binding_id in &facts.use_binding_ids {
            cancellation.check()?;
            let mut reaching_for_binding = Vec::new();
            if let Some(indices) = reaching.binding_definition_indices.get(binding_id) {
                let reaching_in = &reaching.reaching_in[facts.node_id.index()];
                for &index in indices {
                    cancellation.check()?;
                    if reaching_in.contains(index) {
                        reaching_for_binding.push(reaching.domain[index]);
                    }
                }
            }
            reaching_for_binding.sort_by_key(|definition| definition.node_id);

            use_defs.push(AnalysisFlowUseDef {
                node_id: facts.node_id,
                binding_id: *binding_id,
                reaching_definitions: reaching_for_binding,
            });
        }
    }

    use_defs.sort_by_key(|use_def| (use_def.node_id, use_def.binding_id));
    Ok(use_defs)
}

pub fn collect_def_uses(
    node_transfers: &[AnalysisFlowNodeTransfer],
    use_defs: &[AnalysisFlowUseDef],
) -> Vec<AnalysisFlowDefUse> {
    expect_uncancelable(
        collect_def_uses_cancelable(node_transfers, use_defs, &CancellationToken::new()),
        "collecting flow def-use links",
    )
}

pub fn collect_def_uses_cancelable(
    node_transfers: &[AnalysisFlowNodeTransfer],
    use_defs: &[AnalysisFlowUseDef],
    cancellation: &CancellationToken,
) -> Result<Vec<AnalysisFlowDefUse>, Canceled> {
    let mut def_use_map = node_transfers
        .iter()
        .flat_map(|transfer| transfer.generate_definitions.iter().cloned())
        .map(|definition| (definition, Vec::<AnalysisFlowNodeId>::new()))
        .collect::<HashMap<_, _>>();

    for use_def in use_defs {
        cancellation.check()?;
        for reaching_definition in &use_def.reaching_definitions {
            cancellation.check()?;
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
    Ok(def_uses)
}

pub fn collect_resolved_uses(use_defs: &[AnalysisFlowUseDef]) -> Vec<AnalysisFlowResolvedUse> {
    expect_uncancelable(
        collect_resolved_uses_cancelable(use_defs, &CancellationToken::new()),
        "collecting resolved flow uses",
    )
}

pub fn collect_resolved_uses_cancelable(
    use_defs: &[AnalysisFlowUseDef],
    cancellation: &CancellationToken,
) -> Result<Vec<AnalysisFlowResolvedUse>, Canceled> {
    let mut resolved_uses = Vec::with_capacity(use_defs.len());
    for use_def in use_defs {
        cancellation.check()?;
        let kind = match use_def.reaching_definitions.len() {
            0 => AnalysisFlowResolvedUseKind::Missing,
            1 => AnalysisFlowResolvedUseKind::Unique,
            _ => AnalysisFlowResolvedUseKind::Ambiguous,
        };

        resolved_uses.push(AnalysisFlowResolvedUse {
            node_id: use_def.node_id,
            binding_id: use_def.binding_id,
            kind,
            candidate_definitions: use_def.reaching_definitions.clone(),
        });
    }
    resolved_uses.sort_by_key(|resolved| (resolved.node_id, resolved.binding_id));
    Ok(resolved_uses)
}

pub fn collect_single_source_uses(
    resolved_uses: &[AnalysisFlowResolvedUse],
    definition_facts: &[AnalysisFlowDefinitionFacts],
) -> Vec<AnalysisFlowSingleSourceUse> {
    let result = collect_single_source_uses_cancelable(
        resolved_uses,
        definition_facts,
        &CancellationToken::new(),
    );
    expect_uncancelable(result, "collecting single-source flow uses")
}

pub fn collect_single_source_uses_cancelable(
    resolved_uses: &[AnalysisFlowResolvedUse],
    definition_facts: &[AnalysisFlowDefinitionFacts],
    cancellation: &CancellationToken,
) -> Result<Vec<AnalysisFlowSingleSourceUse>, Canceled> {
    let definition_by_ref = definition_facts
        .iter()
        .map(|facts| (facts.definition, facts))
        .collect::<HashMap<_, _>>();

    let mut single_source_uses = Vec::new();
    for resolved in resolved_uses {
        cancellation.check()?;
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
    Ok(single_source_uses)
}

pub fn compute_reaching_definitions(
    topology: &CfgTopology,
    node_transfers: &[AnalysisFlowNodeTransfer],
) -> ComputedReaching {
    expect_uncancelable(
        compute_reaching_definitions_cancelable(
            topology,
            node_transfers,
            &CancellationToken::new(),
        ),
        "computing reaching definitions",
    )
}

pub fn compute_reaching_definitions_cancelable(
    topology: &CfgTopology,
    node_transfers: &[AnalysisFlowNodeTransfer],
    cancellation: &CancellationToken,
) -> Result<ComputedReaching, Canceled> {
    let mut domain = node_transfers
        .iter()
        .flat_map(|transfer| transfer.generate_definitions.iter().copied())
        .collect::<Vec<_>>();
    cancellation.check()?;
    // The domain is stable and sorted so materialized facts are deterministic
    // even when input CFG construction order changes slightly.
    domain.sort_by_key(|definition| (definition.binding_id, definition.node_id));
    domain.dedup();

    let definition_index = domain
        .iter()
        .enumerate()
        .map(|(index, definition)| (*definition, index))
        .collect::<HashMap<_, _>>();
    let mut binding_definition_indices = HashMap::<AnalysisFlowBindingId, Vec<usize>>::new();
    for (index, definition) in domain.iter().copied().enumerate() {
        cancellation.check()?;
        binding_definition_indices
            .entry(definition.binding_id)
            .or_default()
            .push(index);
    }

    let mut kill_bits = Vec::with_capacity(node_transfers.len());
    for transfer in node_transfers {
        cancellation.check()?;
        let indices = transfer
            .kill_binding_ids
            .iter()
            // Defining a binding kills all previous definitions for that
            // binding, not just definitions that reach this node.
            .flat_map(|binding_id| {
                binding_definition_indices
                    .get(binding_id)
                    .into_iter()
                    .flat_map(|indices| indices.iter().copied())
            })
            .collect::<Vec<_>>();
        kill_bits.push(DenseBitSet::from_indices_cancelable(
            domain.len(),
            &indices,
            cancellation,
        )?);
    }
    let mut gen_bits = Vec::with_capacity(node_transfers.len());
    for transfer in node_transfers {
        cancellation.check()?;
        let indices = transfer
            .generate_definitions
            .iter()
            .filter_map(|definition| definition_index.get(definition).copied())
            .collect::<Vec<_>>();
        gen_bits.push(DenseBitSet::from_indices_cancelable(
            domain.len(),
            &indices,
            cancellation,
        )?);
    }

    let node_count = topology.predecessors.len();
    let mut reaching_in = vec![DenseBitSet::new(domain.len()); node_count];
    let mut reaching_out = vec![DenseBitSet::new(domain.len()); node_count];
    let mut next_in = DenseBitSet::new(domain.len());
    let mut next_out = DenseBitSet::new(domain.len());
    let mut queued = vec![true; node_count];
    let mut worklist = (0..node_count).collect::<Vec<_>>();
    let mut cursor = 0;

    while let Some(&node_index) = worklist.get(cursor) {
        cancellation.check()?;
        cursor += 1;
        queued[node_index] = false;
        next_in.clear();
        for predecessor in &topology.predecessors[node_index] {
            cancellation.check()?;
            next_in.union_with(&reaching_out[predecessor.index()]);
        }

        next_out.copy_from(&next_in);
        next_out.subtract(&kill_bits[node_index]);
        next_out.union_with(&gen_bits[node_index]);

        if next_in != reaching_in[node_index] || next_out != reaching_out[node_index] {
            // Forward fixed-point iteration: when a node changes, only its
            // successors can observe new reaching definitions.
            reaching_in[node_index].copy_from(&next_in);
            reaching_out[node_index].copy_from(&next_out);
            for successor in &topology.successors[node_index] {
                cancellation.check()?;
                let successor_index = successor.index();
                if !queued[successor_index] {
                    queued[successor_index] = true;
                    worklist.push(successor_index);
                }
            }
        }
    }

    Ok(ComputedReaching {
        domain,
        binding_definition_indices,
        reaching_in,
        reaching_out,
    })
}

pub fn materialize_reaching_definitions(
    cfg: &AnalysisFlowCfg,
    reaching: &ComputedReaching,
) -> Vec<AnalysisFlowReaching> {
    expect_uncancelable(
        materialize_reaching_definitions_cancelable(cfg, reaching, &CancellationToken::new()),
        "materializing reaching definitions",
    )
}

pub fn materialize_reaching_definitions_cancelable(
    cfg: &AnalysisFlowCfg,
    reaching: &ComputedReaching,
    cancellation: &CancellationToken,
) -> Result<Vec<AnalysisFlowReaching>, Canceled> {
    let mut materialized = Vec::with_capacity(cfg.nodes.len());
    for node in &cfg.nodes {
        cancellation.check()?;
        materialized.push(AnalysisFlowReaching {
            node_id: node.id,
            reaching_in: reaching.reaching_in[node.id.index()]
                .collect_definitions_cancelable(&reaching.domain, cancellation)?,
            reaching_out: reaching.reaching_out[node.id.index()]
                .collect_definitions_cancelable(&reaching.domain, cancellation)?,
        });
    }
    Ok(materialized)
}

pub fn collect_binding_summaries(
    binding_count: usize,
    cfg: &AnalysisFlowCfg,
    node_facts: &[AnalysisFlowNodeFacts],
    liveness: &ComputedLiveness,
) -> Vec<AnalysisFlowBindingSummary> {
    let result = collect_binding_summaries_cancelable(
        binding_count,
        cfg,
        node_facts,
        liveness,
        &CancellationToken::new(),
    );
    expect_uncancelable(result, "collecting flow binding summaries")
}

pub fn collect_binding_summaries_cancelable(
    binding_count: usize,
    cfg: &AnalysisFlowCfg,
    node_facts: &[AnalysisFlowNodeFacts],
    liveness: &ComputedLiveness,
    cancellation: &CancellationToken,
) -> Result<Vec<AnalysisFlowBindingSummary>, Canceled> {
    let mut summaries = (0..binding_count)
        .map(|binding_index| AnalysisFlowBindingSummary {
            binding_id: AnalysisFlowBindingId(binding_index),
            definition_node_ids: Vec::new(),
            use_node_ids: Vec::new(),
            live_node_ids: Vec::new(),
        })
        .collect::<Vec<_>>();

    for node in &cfg.nodes {
        cancellation.check()?;
        let facts = &node_facts[node.id.index()];
        for binding_id in &facts.define_binding_ids {
            cancellation.check()?;
            summaries[binding_id.index()]
                .definition_node_ids
                .push(node.id);
        }
        for binding_id in &facts.use_binding_ids {
            cancellation.check()?;
            summaries[binding_id.index()].use_node_ids.push(node.id);
        }
    }

    for node in &cfg.nodes {
        cancellation.check()?;
        let node_index = node.id.index();

        liveness.live_in[node_index].for_each_set_bit_cancelable(
            cancellation,
            |binding_index| {
                let binding_id = AnalysisFlowBindingId(binding_index);
                summaries[binding_id.index()].live_node_ids.push(node.id);
                Ok(())
            },
        )?;
        liveness.live_out[node_index].for_each_set_bit_cancelable(
            cancellation,
            |binding_index| {
                let binding_id = AnalysisFlowBindingId(binding_index);
                let live_node_ids = &mut summaries[binding_id.index()].live_node_ids;
                if live_node_ids.last() != Some(&node.id) {
                    live_node_ids.push(node.id);
                }
                Ok(())
            },
        )?;
    }

    for summary in &mut summaries {
        cancellation.check()?;
        summary.definition_node_ids.sort();
        summary.use_node_ids.sort();
        summary.live_node_ids.sort();
        summary.live_node_ids.dedup();
    }

    Ok(summaries)
}

pub fn compute_liveness(
    topology: &CfgTopology,
    node_transfers: &[AnalysisFlowNodeTransfer],
) -> ComputedLiveness {
    expect_uncancelable(
        compute_liveness_cancelable(topology, node_transfers, &CancellationToken::new()),
        "computing liveness",
    )
}

pub fn compute_liveness_cancelable(
    topology: &CfgTopology,
    node_transfers: &[AnalysisFlowNodeTransfer],
    cancellation: &CancellationToken,
) -> Result<ComputedLiveness, Canceled> {
    let binding_count = node_transfers
        .iter()
        .flat_map(|transfer| {
            transfer
                .use_binding_ids
                .iter()
                .chain(transfer.kill_binding_ids.iter())
        })
        .map(|binding_id| binding_id.index() + 1)
        .max()
        .unwrap_or(0);

    // Liveness is over bindings rather than definitions, so the bit domain is
    // just the dense binding-id range observed in the transfer functions.
    let mut use_bits = Vec::with_capacity(node_transfers.len());
    let mut kill_bits = Vec::with_capacity(node_transfers.len());
    for transfer in node_transfers {
        cancellation.check()?;
        use_bits.push(DenseBitSet::from_bindings_cancelable(
            binding_count,
            &transfer.use_binding_ids,
            cancellation,
        )?);
        kill_bits.push(DenseBitSet::from_bindings_cancelable(
            binding_count,
            &transfer.kill_binding_ids,
            cancellation,
        )?);
    }

    let node_count = topology.successors.len();
    let mut live_in = vec![DenseBitSet::new(binding_count); node_count];
    let mut live_out = vec![DenseBitSet::new(binding_count); node_count];
    let mut next_in = DenseBitSet::new(binding_count);
    let mut next_out = DenseBitSet::new(binding_count);
    let mut carried = DenseBitSet::new(binding_count);
    let mut queued = vec![true; node_count];
    let mut worklist = (0..node_count).rev().collect::<Vec<_>>();
    let mut cursor = 0;

    while let Some(&node_index) = worklist.get(cursor) {
        cancellation.check()?;
        cursor += 1;
        queued[node_index] = false;
        next_out.clear();
        for successor in &topology.successors[node_index] {
            cancellation.check()?;
            next_out.union_with(&live_in[successor.index()]);
        }

        next_in.copy_from(&use_bits[node_index]);
        carried.copy_from(&next_out);
        carried.subtract(&kill_bits[node_index]);
        next_in.union_with(&carried);

        if next_out != live_out[node_index] || next_in != live_in[node_index] {
            // Backward fixed-point iteration: predecessors are the only nodes
            // affected by a changed live-in/live-out set.
            live_out[node_index].copy_from(&next_out);
            live_in[node_index].copy_from(&next_in);
            for predecessor in &topology.predecessors[node_index] {
                cancellation.check()?;
                let predecessor_index = predecessor.index();
                if !queued[predecessor_index] {
                    queued[predecessor_index] = true;
                    worklist.push(predecessor_index);
                }
            }
        }
    }

    Ok(ComputedLiveness { live_in, live_out })
}

pub fn materialize_liveness(
    cfg: &AnalysisFlowCfg,
    liveness: &ComputedLiveness,
) -> Vec<AnalysisFlowLiveness> {
    expect_uncancelable(
        materialize_liveness_cancelable(cfg, liveness, &CancellationToken::new()),
        "materializing liveness",
    )
}

pub fn materialize_liveness_cancelable(
    cfg: &AnalysisFlowCfg,
    liveness: &ComputedLiveness,
    cancellation: &CancellationToken,
) -> Result<Vec<AnalysisFlowLiveness>, Canceled> {
    let mut materialized = Vec::with_capacity(cfg.nodes.len());
    for node in &cfg.nodes {
        cancellation.check()?;
        materialized.push(AnalysisFlowLiveness {
            node_id: node.id,
            live_in: liveness.live_in[node.id.index()].collect_bindings_cancelable(cancellation)?,
            live_out: liveness.live_out[node.id.index()]
                .collect_bindings_cancelable(cancellation)?,
        });
    }
    Ok(materialized)
}

fn sort_binding_ids(values: &[AnalysisFlowBindingId]) -> Vec<AnalysisFlowBindingId> {
    let mut ids = values.to_vec();
    ids.sort();
    ids.dedup();
    ids
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DenseBitSet {
    words: Vec<u64>,
}

impl DenseBitSet {
    fn new(bit_len: usize) -> Self {
        Self {
            words: vec![0; bit_len.div_ceil(64)],
        }
    }

    fn from_bindings_cancelable(
        bit_len: usize,
        bindings: &[AnalysisFlowBindingId],
        cancellation: &CancellationToken,
    ) -> Result<Self, Canceled> {
        let mut set = Self::new(bit_len);
        for binding in bindings {
            cancellation.check()?;
            set.set(binding.index());
        }
        Ok(set)
    }

    fn from_indices_cancelable(
        bit_len: usize,
        indices: &[usize],
        cancellation: &CancellationToken,
    ) -> Result<Self, Canceled> {
        let mut set = Self::new(bit_len);
        for &index in indices {
            cancellation.check()?;
            set.set(index);
        }
        Ok(set)
    }

    fn set(&mut self, index: usize) {
        let word = index / 64;
        let bit = index % 64;
        if let Some(slot) = self.words.get_mut(word) {
            // Out-of-domain bits are ignored by construction callers, but this
            // guard keeps malformed recovery data from panicking in analysis.
            *slot |= 1u64 << bit;
        }
    }

    fn union_with(&mut self, other: &Self) {
        for (lhs, rhs) in self.words.iter_mut().zip(&other.words) {
            *lhs |= *rhs;
        }
    }

    fn clear(&mut self) {
        self.words.fill(0);
    }

    fn copy_from(&mut self, other: &Self) {
        debug_assert_eq!(self.words.len(), other.words.len());
        self.words.copy_from_slice(&other.words);
    }

    fn subtract(&mut self, other: &Self) {
        for (lhs, rhs) in self.words.iter_mut().zip(&other.words) {
            *lhs &= !*rhs;
        }
    }

    fn contains(&self, index: usize) -> bool {
        let word = index / 64;
        let bit = index % 64;
        self.words
            .get(word)
            .is_some_and(|word| (word & (1u64 << bit)) != 0)
    }

    fn for_each_set_bit_cancelable(
        &self,
        cancellation: &CancellationToken,
        mut f: impl FnMut(usize) -> Result<(), Canceled>,
    ) -> Result<(), Canceled> {
        for (word_index, word) in self.words.iter().copied().enumerate() {
            cancellation.check()?;
            let mut bits = word;
            while bits != 0 {
                cancellation.check()?;
                let bit_index = bits.trailing_zeros() as usize;
                f(word_index * 64 + bit_index)?;
                // Clear the lowest set bit in O(1), avoiding a scan over unset
                // positions in sparse data-flow sets.
                bits &= bits - 1;
            }
        }
        Ok(())
    }

    fn collect_bindings_cancelable(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<Vec<AnalysisFlowBindingId>, Canceled> {
        let mut bindings = Vec::new();
        self.for_each_set_bit_cancelable(cancellation, |index| {
            bindings.push(AnalysisFlowBindingId(index));
            Ok(())
        })?;
        Ok(bindings)
    }

    fn collect_definitions_cancelable(
        &self,
        domain: &[AnalysisFlowDefinitionRef],
        cancellation: &CancellationToken,
    ) -> Result<Vec<AnalysisFlowDefinitionRef>, Canceled> {
        let mut definitions = Vec::new();
        for (word_index, word) in self.words.iter().copied().enumerate() {
            cancellation.check()?;
            let mut bits = word;
            while bits != 0 {
                cancellation.check()?;
                let bit_index = bits.trailing_zeros() as usize;
                let domain_index = word_index * 64 + bit_index;
                if let Some(definition) = domain.get(domain_index).copied() {
                    definitions.push(definition);
                }
                bits &= bits - 1;
            }
        }
        Ok(definitions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AnalysisFlowCfgEdge, AnalysisFlowCfgEdgeKind, AnalysisFlowCfgNode, AnalysisFlowCfgNodeKind,
    };
    use kernc_utils::Span;

    fn linear_cfg(node_count: usize) -> AnalysisFlowCfg {
        let nodes = (0..node_count)
            .map(|index| AnalysisFlowCfgNode {
                id: AnalysisFlowNodeId(index),
                span: Span::default(),
                kind: if index == 0 {
                    AnalysisFlowCfgNodeKind::Entry
                } else if index + 1 == node_count {
                    AnalysisFlowCfgNodeKind::Exit
                } else {
                    AnalysisFlowCfgNodeKind::Eval
                },
                ast_node_id: None,
            })
            .collect::<Vec<_>>();
        let edges = (0..node_count.saturating_sub(1))
            .map(|index| AnalysisFlowCfgEdge {
                from: AnalysisFlowNodeId(index),
                to: AnalysisFlowNodeId(index + 1),
                kind: AnalysisFlowCfgEdgeKind::Next,
            })
            .collect::<Vec<_>>();

        AnalysisFlowCfg {
            entry: AnalysisFlowNodeId(0),
            exit: AnalysisFlowNodeId(node_count.saturating_sub(1)),
            nodes,
            edges,
        }
    }

    fn transfers(count: usize) -> Vec<AnalysisFlowNodeTransfer> {
        (0..count)
            .map(|index| {
                let binding_id = AnalysisFlowBindingId(index);
                let use_binding_ids = if index == 0 {
                    Vec::new()
                } else {
                    vec![AnalysisFlowBindingId(index - 1)]
                };
                AnalysisFlowNodeTransfer {
                    node_id: AnalysisFlowNodeId(index),
                    use_binding_ids,
                    kill_binding_ids: vec![binding_id],
                    generate_definitions: vec![AnalysisFlowDefinitionRef {
                        binding_id,
                        node_id: AnalysisFlowNodeId(index),
                    }],
                }
            })
            .collect()
    }

    #[test]
    fn reaching_definition_worklist_observes_cancellation() {
        let cfg = linear_cfg(12);
        let topology = CfgTopology::from_cfg(&cfg);
        let transfers = transfers(cfg.nodes.len());
        let cancellation = CancellationToken::with_check_budget_for_testing(8);

        let result = compute_reaching_definitions_cancelable(&topology, &transfers, &cancellation);

        assert_eq!(result.err(), Some(Canceled));
        assert!(cancellation.is_canceled());
    }

    #[test]
    fn liveness_worklist_observes_cancellation() {
        let cfg = linear_cfg(12);
        let topology = CfgTopology::from_cfg(&cfg);
        let transfers = transfers(cfg.nodes.len());
        let cancellation = CancellationToken::with_check_budget_for_testing(8);

        let result = compute_liveness_cancelable(&topology, &transfers, &cancellation);

        assert_eq!(result.err(), Some(Canceled));
        assert!(cancellation.is_canceled());
    }
}
