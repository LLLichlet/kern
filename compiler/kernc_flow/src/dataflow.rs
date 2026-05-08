use crate::{
    AnalysisFlowBindingId, AnalysisFlowBindingSummary, AnalysisFlowCfg, AnalysisFlowDefUse,
    AnalysisFlowDefinitionFacts, AnalysisFlowDefinitionKind, AnalysisFlowDefinitionRef,
    AnalysisFlowLiveness, AnalysisFlowNodeFacts, AnalysisFlowNodeId, AnalysisFlowNodeTransfer,
    AnalysisFlowReaching, AnalysisFlowResolvedUse, AnalysisFlowResolvedUseKind,
    AnalysisFlowSingleSourceUse, AnalysisFlowUseDef,
};
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
        let mut predecessors = vec![Vec::<AnalysisFlowNodeId>::new(); cfg.nodes.len()];
        let mut successors = vec![Vec::<AnalysisFlowNodeId>::new(); cfg.nodes.len()];
        for edge in &cfg.edges {
            predecessors[edge.to.index()].push(edge.from);
            successors[edge.from.index()].push(edge.to);
        }

        Self {
            predecessors,
            successors,
        }
    }
}

pub fn collect_node_facts(
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

pub fn collect_definition_facts(
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

pub fn collect_node_transfers(
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

pub fn collect_use_defs(
    node_facts: &[AnalysisFlowNodeFacts],
    reaching: &ComputedReaching,
) -> Vec<AnalysisFlowUseDef> {
    let mut use_defs = Vec::new();
    for facts in node_facts {
        for binding_id in &facts.use_binding_ids {
            let mut reaching_for_binding = Vec::new();
            if let Some(indices) = reaching.binding_definition_indices.get(binding_id) {
                let reaching_in = &reaching.reaching_in[facts.node_id.index()];
                for &index in indices {
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
    use_defs
}

pub fn collect_def_uses(
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

pub fn collect_resolved_uses(use_defs: &[AnalysisFlowUseDef]) -> Vec<AnalysisFlowResolvedUse> {
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

pub fn collect_single_source_uses(
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

pub fn compute_reaching_definitions(
    topology: &CfgTopology,
    node_transfers: &[AnalysisFlowNodeTransfer],
) -> ComputedReaching {
    let mut domain = node_transfers
        .iter()
        .flat_map(|transfer| transfer.generate_definitions.iter().copied())
        .collect::<Vec<_>>();
    domain.sort_by_key(|definition| (definition.binding_id, definition.node_id));
    domain.dedup();

    let definition_index = domain
        .iter()
        .enumerate()
        .map(|(index, definition)| (*definition, index))
        .collect::<HashMap<_, _>>();
    let mut binding_definition_indices = HashMap::<AnalysisFlowBindingId, Vec<usize>>::new();
    for (index, definition) in domain.iter().copied().enumerate() {
        binding_definition_indices
            .entry(definition.binding_id)
            .or_default()
            .push(index);
    }

    let kill_bits = node_transfers
        .iter()
        .map(|transfer| {
            let indices = transfer
                .kill_binding_ids
                .iter()
                .flat_map(|binding_id| {
                    binding_definition_indices
                        .get(binding_id)
                        .into_iter()
                        .flat_map(|indices| indices.iter().copied())
                })
                .collect::<Vec<_>>();
            DenseBitSet::from_indices(domain.len(), &indices)
        })
        .collect::<Vec<_>>();
    let gen_bits = node_transfers
        .iter()
        .map(|transfer| {
            let indices = transfer
                .generate_definitions
                .iter()
                .filter_map(|definition| definition_index.get(definition).copied())
                .collect::<Vec<_>>();
            DenseBitSet::from_indices(domain.len(), &indices)
        })
        .collect::<Vec<_>>();

    let node_count = topology.predecessors.len();
    let mut reaching_in = vec![DenseBitSet::new(domain.len()); node_count];
    let mut reaching_out = vec![DenseBitSet::new(domain.len()); node_count];
    let mut next_in = DenseBitSet::new(domain.len());
    let mut next_out = DenseBitSet::new(domain.len());
    let mut queued = vec![true; node_count];
    let mut worklist = (0..node_count).collect::<Vec<_>>();
    let mut cursor = 0;

    while let Some(&node_index) = worklist.get(cursor) {
        cursor += 1;
        queued[node_index] = false;
        next_in.clear();
        for predecessor in &topology.predecessors[node_index] {
            next_in.union_with(&reaching_out[predecessor.index()]);
        }

        next_out.copy_from(&next_in);
        next_out.subtract(&kill_bits[node_index]);
        next_out.union_with(&gen_bits[node_index]);

        if next_in != reaching_in[node_index] || next_out != reaching_out[node_index] {
            reaching_in[node_index].copy_from(&next_in);
            reaching_out[node_index].copy_from(&next_out);
            for successor in &topology.successors[node_index] {
                let successor_index = successor.index();
                if !queued[successor_index] {
                    queued[successor_index] = true;
                    worklist.push(successor_index);
                }
            }
        }
    }

    ComputedReaching {
        domain,
        binding_definition_indices,
        reaching_in,
        reaching_out,
    }
}

pub fn materialize_reaching_definitions(
    cfg: &AnalysisFlowCfg,
    reaching: &ComputedReaching,
) -> Vec<AnalysisFlowReaching> {
    cfg.nodes
        .iter()
        .map(|node| AnalysisFlowReaching {
            node_id: node.id,
            reaching_in: reaching.reaching_in[node.id.index()]
                .collect_definitions(&reaching.domain),
            reaching_out: reaching.reaching_out[node.id.index()]
                .collect_definitions(&reaching.domain),
        })
        .collect()
}

pub fn collect_binding_summaries(
    binding_count: usize,
    cfg: &AnalysisFlowCfg,
    node_facts: &[AnalysisFlowNodeFacts],
    liveness: &ComputedLiveness,
) -> Vec<AnalysisFlowBindingSummary> {
    let mut summaries = (0..binding_count)
        .map(|binding_index| AnalysisFlowBindingSummary {
            binding_id: AnalysisFlowBindingId(binding_index),
            definition_node_ids: Vec::new(),
            use_node_ids: Vec::new(),
            live_node_ids: Vec::new(),
        })
        .collect::<Vec<_>>();

    for node in &cfg.nodes {
        let facts = &node_facts[node.id.index()];
        for binding_id in &facts.define_binding_ids {
            summaries[binding_id.index()]
                .definition_node_ids
                .push(node.id);
        }
        for binding_id in &facts.use_binding_ids {
            summaries[binding_id.index()].use_node_ids.push(node.id);
        }
    }

    for node in &cfg.nodes {
        let node_index = node.id.index();

        liveness.live_in[node_index].for_each_set_bit(|binding_index| {
            let binding_id = AnalysisFlowBindingId(binding_index);
            summaries[binding_id.index()].live_node_ids.push(node.id);
        });
        liveness.live_out[node_index].for_each_set_bit(|binding_index| {
            let binding_id = AnalysisFlowBindingId(binding_index);
            let live_node_ids = &mut summaries[binding_id.index()].live_node_ids;
            if live_node_ids.last() != Some(&node.id) {
                live_node_ids.push(node.id);
            }
        });
    }

    for summary in &mut summaries {
        summary.definition_node_ids.sort();
        summary.use_node_ids.sort();
        summary.live_node_ids.sort();
        summary.live_node_ids.dedup();
    }

    summaries
}

pub fn compute_liveness(
    topology: &CfgTopology,
    node_transfers: &[AnalysisFlowNodeTransfer],
) -> ComputedLiveness {
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

    let use_bits = node_transfers
        .iter()
        .map(|transfer| DenseBitSet::from_bindings(binding_count, &transfer.use_binding_ids))
        .collect::<Vec<_>>();
    let kill_bits = node_transfers
        .iter()
        .map(|transfer| DenseBitSet::from_bindings(binding_count, &transfer.kill_binding_ids))
        .collect::<Vec<_>>();

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
        cursor += 1;
        queued[node_index] = false;
        next_out.clear();
        for successor in &topology.successors[node_index] {
            next_out.union_with(&live_in[successor.index()]);
        }

        next_in.copy_from(&use_bits[node_index]);
        carried.copy_from(&next_out);
        carried.subtract(&kill_bits[node_index]);
        next_in.union_with(&carried);

        if next_out != live_out[node_index] || next_in != live_in[node_index] {
            live_out[node_index].copy_from(&next_out);
            live_in[node_index].copy_from(&next_in);
            for predecessor in &topology.predecessors[node_index] {
                let predecessor_index = predecessor.index();
                if !queued[predecessor_index] {
                    queued[predecessor_index] = true;
                    worklist.push(predecessor_index);
                }
            }
        }
    }

    ComputedLiveness { live_in, live_out }
}

pub fn materialize_liveness(
    cfg: &AnalysisFlowCfg,
    liveness: &ComputedLiveness,
) -> Vec<AnalysisFlowLiveness> {
    cfg.nodes
        .iter()
        .map(|node| AnalysisFlowLiveness {
            node_id: node.id,
            live_in: liveness.live_in[node.id.index()].collect_bindings(),
            live_out: liveness.live_out[node.id.index()].collect_bindings(),
        })
        .collect()
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

    fn from_bindings(bit_len: usize, bindings: &[AnalysisFlowBindingId]) -> Self {
        let mut set = Self::new(bit_len);
        for binding in bindings {
            set.set(binding.index());
        }
        set
    }

    fn from_indices(bit_len: usize, indices: &[usize]) -> Self {
        let mut set = Self::new(bit_len);
        for &index in indices {
            set.set(index);
        }
        set
    }

    fn set(&mut self, index: usize) {
        let word = index / 64;
        let bit = index % 64;
        if let Some(slot) = self.words.get_mut(word) {
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

    fn for_each_set_bit(&self, mut f: impl FnMut(usize)) {
        for (word_index, word) in self.words.iter().copied().enumerate() {
            let mut bits = word;
            while bits != 0 {
                let bit_index = bits.trailing_zeros() as usize;
                f(word_index * 64 + bit_index);
                bits &= bits - 1;
            }
        }
    }

    fn collect_bindings(&self) -> Vec<AnalysisFlowBindingId> {
        let mut bindings = Vec::new();
        self.for_each_set_bit(|index| bindings.push(AnalysisFlowBindingId(index)));
        bindings
    }

    fn collect_definitions(
        &self,
        domain: &[AnalysisFlowDefinitionRef],
    ) -> Vec<AnalysisFlowDefinitionRef> {
        let mut definitions = Vec::new();
        for (word_index, word) in self.words.iter().copied().enumerate() {
            let mut bits = word;
            while bits != 0 {
                let bit_index = bits.trailing_zeros() as usize;
                let domain_index = word_index * 64 + bit_index;
                if let Some(definition) = domain.get(domain_index).copied() {
                    definitions.push(definition);
                }
                bits &= bits - 1;
            }
        }
        definitions
    }
}
