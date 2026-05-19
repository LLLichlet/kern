// Flow model collection.
//
// Collection walks semantic definitions, lowers function bodies into flow CFGs,
// computes reaching definitions, liveness, use-def links, control regions, and
// phase timings for editor/compile diagnostics.

use super::control::collect_control_facts;
use super::optimize::collect_owner_optimization_facts;

use super::*;
use crate::compiler::AnalysisFlowOwner;
use kernc_flow::FlowLoweringHints;
use kernc_flow::{
    CfgTopology, collect_binding_summaries_cancelable, collect_def_uses_cancelable,
    collect_definition_facts_cancelable, collect_node_facts_cancelable,
    collect_node_transfers_cancelable, collect_resolved_uses_cancelable,
    collect_single_source_uses_cancelable, collect_use_defs_cancelable,
    compute_liveness_cancelable, compute_reaching_definitions_cancelable,
    materialize_liveness_cancelable, materialize_reaching_definitions_cancelable,
};
use kernc_sema::SemaContext;
use kernc_sema::def::{Def, DefId};
use kernc_sema::semantic::SemanticSymbolKind;
use kernc_utils::Span;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

impl FlowModel {
    pub(in crate::compiler) fn phase_timings(&self) -> &[FlowTiming] {
        &self.phase_timings
    }

    #[cfg(test)]
    pub fn collect(
        ctx: &SemaContext<'_>,
        module_item_definition_spans: &HashMap<DefId, Span>,
        references: &[(Span, Span)],
    ) -> Self {
        Self::collect_cancelable(
            ctx,
            module_item_definition_spans,
            references,
            &CancellationToken::new(),
        )
        .expect("fresh cancellation token cannot be canceled")
    }

    pub fn collect_cancelable(
        ctx: &SemaContext<'_>,
        module_item_definition_spans: &HashMap<DefId, Span>,
        references: &[(Span, Span)],
        cancellation: &CancellationToken,
    ) -> Result<Self, Canceled> {
        Self::collect_with_mode(
            ctx,
            module_item_definition_spans,
            references,
            true,
            cancellation,
        )
    }

    pub fn collect_for_compile(
        ctx: &SemaContext<'_>,
        module_item_definition_spans: &HashMap<DefId, Span>,
        references: &[(Span, Span)],
    ) -> Self {
        Self::collect_for_compile_cancelable(
            ctx,
            module_item_definition_spans,
            references,
            &CancellationToken::new(),
        )
        .expect("fresh cancellation token cannot be canceled")
    }

    pub fn collect_for_compile_cancelable(
        ctx: &SemaContext<'_>,
        module_item_definition_spans: &HashMap<DefId, Span>,
        references: &[(Span, Span)],
        cancellation: &CancellationToken,
    ) -> Result<Self, Canceled> {
        Self::collect_with_mode(
            ctx,
            module_item_definition_spans,
            references,
            false,
            cancellation,
        )
    }

    fn collect_with_mode(
        ctx: &SemaContext<'_>,
        module_item_definition_spans: &HashMap<DefId, Span>,
        references: &[(Span, Span)],
        include_analysis_details: bool,
        cancellation: &CancellationToken,
    ) -> Result<Self, Canceled> {
        cancellation.check()?;
        let mut phase_totals = HashMap::<&'static str, Duration>::new();
        let record =
            |totals: &mut HashMap<&'static str, Duration>, name: &'static str, started: Instant| {
                *totals.entry(name).or_default() += started.elapsed();
            };
        let mut owners = Vec::new();

        for def in &ctx.defs {
            cancellation.check()?;
            match def {
                Def::Function(function) => {
                    let started = Instant::now();
                    if function.is_imported || function.is_intrinsic {
                        continue;
                    }
                    let Some(parent) = function.parent else {
                        continue;
                    };
                    if !matches!(ctx.defs.get(parent.0 as usize), Some(Def::Module(_))) {
                        continue;
                    }
                    let Some(body_span) = function.body.as_ref().map(|body| body.span) else {
                        continue;
                    };
                    owners.push(FlowOwnerFacts {
                        def_id: function.id,
                        definition_span: function.name_span,
                        owner_span: function.span,
                        body_span,
                        kind: AnalysisFlowOwnerKind::Function,
                        referenced_def_ids: Vec::new(),
                        referenced_definition_spans: Vec::new(),
                        cfg: AnalysisFlowCfg {
                            entry: AnalysisFlowNodeId(0),
                            exit: AnalysisFlowNodeId(0),
                            nodes: Vec::new(),
                            edges: Vec::new(),
                        },
                        node_facts: Vec::new(),
                        node_effects: Vec::new(),
                        node_transfers: Vec::new(),
                        use_defs: Vec::new(),
                        def_uses: Vec::new(),
                        definition_facts: Vec::new(),
                        resolved_uses: Vec::new(),
                        single_source_uses: Vec::new(),
                        liveness: Vec::new(),
                        computed_liveness: None,
                        reaching_definitions: Vec::new(),
                        control_regions: Vec::new(),
                        summary: AnalysisFlowSummary::default(),
                        bindings: Vec::new(),
                        binding_summaries: Vec::new(),
                    });
                    record(&mut phase_totals, "  flow_collect_owners", started);
                }
                Def::Global(global) => {
                    let started = Instant::now();
                    if global.is_imported {
                        continue;
                    }
                    let Some(&definition_span) = module_item_definition_spans.get(&global.id)
                    else {
                        continue;
                    };
                    owners.push(FlowOwnerFacts {
                        def_id: global.id,
                        definition_span,
                        owner_span: global.span,
                        body_span: global
                            .value
                            .as_ref()
                            .map(|value| value.span)
                            .unwrap_or(global.span),
                        kind: if global.is_static {
                            AnalysisFlowOwnerKind::Static
                        } else {
                            AnalysisFlowOwnerKind::Constant
                        },
                        referenced_def_ids: Vec::new(),
                        referenced_definition_spans: Vec::new(),
                        cfg: AnalysisFlowCfg {
                            entry: AnalysisFlowNodeId(0),
                            exit: AnalysisFlowNodeId(0),
                            nodes: Vec::new(),
                            edges: Vec::new(),
                        },
                        node_facts: Vec::new(),
                        node_effects: Vec::new(),
                        node_transfers: Vec::new(),
                        use_defs: Vec::new(),
                        def_uses: Vec::new(),
                        definition_facts: Vec::new(),
                        resolved_uses: Vec::new(),
                        single_source_uses: Vec::new(),
                        liveness: Vec::new(),
                        computed_liveness: None,
                        reaching_definitions: Vec::new(),
                        control_regions: Vec::new(),
                        summary: AnalysisFlowSummary::default(),
                        bindings: Vec::new(),
                        binding_summaries: Vec::new(),
                    });
                    record(&mut phase_totals, "  flow_collect_owners", started);
                }
                _ => {}
            }
        }

        let started = Instant::now();
        let owner_scope_lookup_by_file =
            build_owner_lookup_by_file(&owners, |owner| owner.owner_span);
        record(&mut phase_totals, "  flow_owner_lookup", started);
        let started = Instant::now();
        for definition in ctx.semantic_definitions() {
            cancellation.check()?;
            let Some((kind, is_mut)) = flow_binding_kind(definition.kind, definition.is_mut) else {
                continue;
            };
            let Some(owner_index) =
                find_owner_lookup_index(&owner_scope_lookup_by_file, definition.span)
            else {
                continue;
            };
            let owner = &mut owners[owner_index];

            owner.bindings.push(FlowBindingFacts {
                id: AnalysisFlowBindingId(0),
                definition_span: definition.span,
                kind,
                is_mut,
                reference_spans: Vec::new(),
            });
        }
        record(&mut phase_totals, "  flow_bindings", started);

        let started = Instant::now();
        cancellation.check()?;
        let item_by_definition_span = module_item_definition_spans
            .iter()
            .map(|(&def_id, &span)| (span, def_id))
            .collect::<HashMap<_, _>>();
        let local_binding_by_definition_span = owners
            .iter()
            .enumerate()
            .flat_map(|(owner_index, owner)| {
                owner
                    .bindings
                    .iter()
                    .enumerate()
                    .map(move |(binding_index, binding)| {
                        (binding.definition_span, (owner_index, binding_index))
                    })
            })
            .collect::<HashMap<_, _>>();
        record(&mut phase_totals, "  flow_reference_index", started);

        let started = Instant::now();
        let owner_body_lookup_by_file =
            build_owner_lookup_by_file(&owners, |owner| owner.body_span);
        for (reference_span, definition_span) in references {
            cancellation.check()?;
            if let Some(&(owner_index, binding_index)) =
                local_binding_by_definition_span.get(definition_span)
            {
                owners[owner_index].bindings[binding_index]
                    .reference_spans
                    .push(*reference_span);
            }

            if let Some(&referenced_def_id) = item_by_definition_span.get(definition_span) {
                let Some(owner_index) =
                    find_owner_lookup_index(&owner_body_lookup_by_file, *reference_span)
                else {
                    continue;
                };
                let owner = &mut owners[owner_index];
                owner.referenced_def_ids.push(referenced_def_id);
                owner.referenced_definition_spans.push(*definition_span);
            }
        }
        record(&mut phase_totals, "  flow_attach_references", started);

        let started = Instant::now();
        for owner in &mut owners {
            cancellation.check()?;
            dedup_preserving_order(&mut owner.referenced_def_ids);
            dedup_preserving_order(&mut owner.referenced_definition_spans);
            for binding in &mut owner.bindings {
                cancellation.check()?;
                dedup_preserving_order(&mut binding.reference_spans);
            }
            owner
                .bindings
                .sort_by_key(|binding| binding.definition_span);
            for (binding_index, binding) in owner.bindings.iter_mut().enumerate() {
                binding.id = AnalysisFlowBindingId(binding_index);
            }
        }
        record(&mut phase_totals, "  flow_finalize_bindings", started);

        for owner in &mut owners {
            cancellation.check()?;
            let binding_ids_by_span = owner
                .bindings
                .iter()
                .map(|binding| (binding.definition_span, binding.id))
                .collect::<HashMap<_, _>>();
            let reference_to_binding = owner
                .bindings
                .iter()
                .flat_map(|binding| {
                    binding
                        .reference_spans
                        .iter()
                        .copied()
                        .map(move |reference_span| (reference_span, binding.id))
                })
                .collect::<HashMap<_, _>>();
            match &ctx.defs[owner.def_id.0 as usize] {
                Def::Function(function) => {
                    let Some(body) = function.body.as_ref() else {
                        continue;
                    };
                    cancellation.check()?;
                    let started = Instant::now();
                    let cfg_build = FlowCfgBuilder::build(
                        body,
                        function.span,
                        &binding_ids_by_span,
                        &reference_to_binding,
                    );
                    record(&mut phase_totals, "  flow_cfg_build", started);
                    owner.cfg = cfg_build.cfg;
                    let started = Instant::now();
                    owner.node_facts = collect_node_facts_cancelable(
                        &owner.cfg,
                        &cfg_build.node_uses,
                        &cfg_build.node_defs,
                        &cfg_build.node_def_kinds,
                        cancellation,
                    )?;
                    record(&mut phase_totals, "  flow_node_facts", started);
                    let started = Instant::now();
                    let node_transfers =
                        collect_node_transfers_cancelable(&owner.node_facts, cancellation)?;
                    record(&mut phase_totals, "  flow_node_transfers", started);
                    let topology = CfgTopology::from_cfg_cancelable(&owner.cfg, cancellation)?;
                    let started = Instant::now();
                    let computed_liveness =
                        compute_liveness_cancelable(&topology, &node_transfers, cancellation)?;
                    record(&mut phase_totals, "  flow_liveness", started);
                    let started = Instant::now();
                    let computed_reaching = compute_reaching_definitions_cancelable(
                        &topology,
                        &node_transfers,
                        cancellation,
                    )?;
                    record(&mut phase_totals, "  flow_reaching", started);
                    let started = Instant::now();
                    let use_defs = collect_use_defs_cancelable(
                        &owner.node_facts,
                        &computed_reaching,
                        cancellation,
                    )?;
                    record(&mut phase_totals, "  flow_use_defs", started);
                    let started = Instant::now();
                    owner.def_uses =
                        collect_def_uses_cancelable(&node_transfers, &use_defs, cancellation)?;
                    record(&mut phase_totals, "  flow_def_uses", started);
                    let started = Instant::now();
                    owner.definition_facts = collect_definition_facts_cancelable(
                        &owner.node_facts,
                        &cfg_build.node_value_uses,
                        &cfg_build.node_copy_sources,
                        cancellation,
                    )?;
                    record(&mut phase_totals, "  flow_definition_facts", started);
                    let started = Instant::now();
                    let resolved_uses = collect_resolved_uses_cancelable(&use_defs, cancellation)?;
                    record(&mut phase_totals, "  flow_resolved_uses", started);
                    let started = Instant::now();
                    owner.single_source_uses = collect_single_source_uses_cancelable(
                        &resolved_uses,
                        &owner.definition_facts,
                        cancellation,
                    )?;
                    record(&mut phase_totals, "  flow_single_source_uses", started);
                    let started = Instant::now();
                    owner.binding_summaries = collect_binding_summaries_cancelable(
                        owner.bindings.len(),
                        &owner.cfg,
                        &owner.node_facts,
                        &computed_liveness,
                        cancellation,
                    )?;
                    record(&mut phase_totals, "  flow_binding_summaries", started);
                    owner.computed_liveness = Some(computed_liveness.clone());
                    if include_analysis_details {
                        cancellation.check()?;
                        let started = Instant::now();
                        let (control_regions, summary) = collect_control_facts(body);
                        record(&mut phase_totals, "  flow_control", started);
                        owner.node_effects = cfg_build.node_effects;
                        owner.node_transfers = node_transfers;
                        owner.reaching_definitions = materialize_reaching_definitions_cancelable(
                            &owner.cfg,
                            &computed_reaching,
                            cancellation,
                        )?;
                        // `computed_liveness` is stored just above so consumers can inspect the
                        // raw fixed-point result; the materialized form uses the same value.
                        owner.liveness = materialize_liveness_cancelable(
                            &owner.cfg,
                            owner.computed_liveness.as_ref().unwrap(),
                            cancellation,
                        )?;
                        owner.use_defs = use_defs;
                        owner.resolved_uses = resolved_uses;
                        owner.control_regions = control_regions;
                        owner.summary = summary;
                    }
                }
                Def::Global(global) => {
                    let Some(value) = global.value.as_ref() else {
                        continue;
                    };
                    cancellation.check()?;
                    let started = Instant::now();
                    let cfg_build = FlowCfgBuilder::build(
                        value,
                        global.span,
                        &binding_ids_by_span,
                        &reference_to_binding,
                    );
                    record(&mut phase_totals, "  flow_cfg_build", started);
                    owner.cfg = cfg_build.cfg;
                    let started = Instant::now();
                    owner.node_facts = collect_node_facts_cancelable(
                        &owner.cfg,
                        &cfg_build.node_uses,
                        &cfg_build.node_defs,
                        &cfg_build.node_def_kinds,
                        cancellation,
                    )?;
                    record(&mut phase_totals, "  flow_node_facts", started);
                    let started = Instant::now();
                    let node_transfers =
                        collect_node_transfers_cancelable(&owner.node_facts, cancellation)?;
                    record(&mut phase_totals, "  flow_node_transfers", started);
                    let topology = CfgTopology::from_cfg_cancelable(&owner.cfg, cancellation)?;
                    let started = Instant::now();
                    let computed_liveness =
                        compute_liveness_cancelable(&topology, &node_transfers, cancellation)?;
                    record(&mut phase_totals, "  flow_liveness", started);
                    let started = Instant::now();
                    let computed_reaching = compute_reaching_definitions_cancelable(
                        &topology,
                        &node_transfers,
                        cancellation,
                    )?;
                    record(&mut phase_totals, "  flow_reaching", started);
                    let started = Instant::now();
                    let use_defs = collect_use_defs_cancelable(
                        &owner.node_facts,
                        &computed_reaching,
                        cancellation,
                    )?;
                    record(&mut phase_totals, "  flow_use_defs", started);
                    let started = Instant::now();
                    owner.def_uses =
                        collect_def_uses_cancelable(&node_transfers, &use_defs, cancellation)?;
                    record(&mut phase_totals, "  flow_def_uses", started);
                    let started = Instant::now();
                    owner.definition_facts = collect_definition_facts_cancelable(
                        &owner.node_facts,
                        &cfg_build.node_value_uses,
                        &cfg_build.node_copy_sources,
                        cancellation,
                    )?;
                    record(&mut phase_totals, "  flow_definition_facts", started);
                    let started = Instant::now();
                    let resolved_uses = collect_resolved_uses_cancelable(&use_defs, cancellation)?;
                    record(&mut phase_totals, "  flow_resolved_uses", started);
                    let started = Instant::now();
                    owner.single_source_uses = collect_single_source_uses_cancelable(
                        &resolved_uses,
                        &owner.definition_facts,
                        cancellation,
                    )?;
                    record(&mut phase_totals, "  flow_single_source_uses", started);
                    let started = Instant::now();
                    owner.binding_summaries = collect_binding_summaries_cancelable(
                        owner.bindings.len(),
                        &owner.cfg,
                        &owner.node_facts,
                        &computed_liveness,
                        cancellation,
                    )?;
                    record(&mut phase_totals, "  flow_binding_summaries", started);
                    owner.computed_liveness = Some(computed_liveness.clone());
                    if include_analysis_details {
                        cancellation.check()?;
                        let started = Instant::now();
                        let (control_regions, summary) = collect_control_facts(value);
                        record(&mut phase_totals, "  flow_control", started);
                        owner.node_effects = cfg_build.node_effects;
                        owner.node_transfers = node_transfers;
                        owner.reaching_definitions = materialize_reaching_definitions_cancelable(
                            &owner.cfg,
                            &computed_reaching,
                            cancellation,
                        )?;
                        owner.liveness = materialize_liveness_cancelable(
                            &owner.cfg,
                            owner.computed_liveness.as_ref().unwrap(),
                            cancellation,
                        )?;
                        owner.use_defs = use_defs;
                        owner.resolved_uses = resolved_uses;
                        owner.control_regions = control_regions;
                        owner.summary = summary;
                    }
                }
                _ => {}
            }
        }

        cancellation.check()?;
        let owner_body_lookup_by_file = build_owner_def_lookup_by_file(&owners);
        let owner_lookup_by_def_id = owners
            .iter()
            .enumerate()
            .map(|(index, owner)| (owner.def_id, index))
            .collect();
        let referenced_item_edges = owners
            .iter()
            .flat_map(|owner| {
                owner
                    .referenced_def_ids
                    .iter()
                    .map(move |&referenced_def_id| (owner.def_id, referenced_def_id))
            })
            .collect();
        let mut phase_timings = phase_totals
            .into_iter()
            .map(|(name, duration)| FlowTiming { name, duration })
            .collect::<Vec<_>>();
        phase_timings.sort_by_key(|timing| timing.name);
        Ok(Self {
            owners,
            owner_body_lookup_by_file,
            owner_lookup_by_def_id,
            referenced_item_edges,
            phase_timings,
        })
    }

    pub fn owner_def_id(&self, reference_span: Span) -> Option<DefId> {
        find_owner_def_id(&self.owner_body_lookup_by_file, reference_span)
    }

    pub fn referenced_item_edges(&self) -> &[(DefId, DefId)] {
        &self.referenced_item_edges
    }

    pub(in crate::compiler) fn function_value_facts(
        &self,
        owner_def_id: DefId,
    ) -> Option<FlowFunctionValueFacts<'_>> {
        let owner_index = self.owner_lookup_by_def_id.get(&owner_def_id).copied()?;
        let owner = self.owners.get(owner_index)?;
        Some(FlowFunctionValueFacts::new(owner))
    }

    pub fn public_owners(&self) -> Vec<AnalysisFlowOwner> {
        self.owners
            .iter()
            .map(FlowOwnerFacts::public_view)
            .collect()
    }

    pub(in crate::compiler) fn lowering_hints(&self, ctx: &SemaContext<'_>) -> FlowLoweringHints {
        let mut hints = FlowLoweringHints::default();

        for owner in &self.owners {
            let owner_facts = collect_owner_optimization_facts(owner, ctx);
            if owner_facts.is_empty() {
                continue;
            }

            hints.insert_owner(owner.def_id, owner_facts.into_lowering_hints());
        }

        hints
    }
}

fn flow_binding_kind(
    kind: SemanticSymbolKind,
    is_mut: bool,
) -> Option<(AnalysisFlowBindingKind, bool)> {
    match kind {
        SemanticSymbolKind::Variable => Some((AnalysisFlowBindingKind::Variable, is_mut)),
        SemanticSymbolKind::Parameter => Some((AnalysisFlowBindingKind::Parameter, is_mut)),
        SemanticSymbolKind::Static => Some((AnalysisFlowBindingKind::Static, is_mut)),
        _ => None,
    }
}

fn dedup_preserving_order<T: Copy + Eq + std::hash::Hash>(values: &mut Vec<T>) {
    let mut seen = HashSet::new();
    values.retain(|value| seen.insert(*value));
}

fn build_owner_lookup_by_file<F>(
    owners: &[FlowOwnerFacts],
    span_of: F,
) -> HashMap<kernc_utils::FileId, Vec<(Span, usize)>>
where
    F: Fn(&FlowOwnerFacts) -> Span,
{
    let mut by_file = HashMap::<kernc_utils::FileId, Vec<(Span, usize)>>::new();
    for (owner_index, owner) in owners.iter().enumerate() {
        let span = span_of(owner);
        by_file
            .entry(span.file)
            .or_default()
            .push((span, owner_index));
    }
    for entries in by_file.values_mut() {
        entries.sort_by_key(|(span, _)| (span.start, span.end));
    }
    by_file
}

fn build_owner_def_lookup_by_file(
    owners: &[FlowOwnerFacts],
) -> HashMap<kernc_utils::FileId, Vec<(Span, DefId)>> {
    let mut by_file = HashMap::<kernc_utils::FileId, Vec<(Span, DefId)>>::new();
    for owner in owners {
        by_file
            .entry(owner.body_span.file)
            .or_default()
            .push((owner.body_span, owner.def_id));
    }
    for entries in by_file.values_mut() {
        entries.sort_by_key(|(span, _)| (span.start, span.end));
    }
    by_file
}

fn find_owner_lookup_index(
    lookup: &HashMap<kernc_utils::FileId, Vec<(Span, usize)>>,
    span: Span,
) -> Option<usize> {
    let entries = lookup.get(&span.file)?;
    let candidate_index = entries.partition_point(|(owner_span, _)| owner_span.start <= span.start);
    if candidate_index == 0 {
        return None;
    }
    let (owner_span, owner_index) = entries[candidate_index - 1];
    span_contains(owner_span, span).then_some(owner_index)
}

fn find_owner_def_id(
    lookup: &HashMap<kernc_utils::FileId, Vec<(Span, DefId)>>,
    span: Span,
) -> Option<DefId> {
    let entries = lookup.get(&span.file)?;
    let candidate_index = entries.partition_point(|(owner_span, _)| owner_span.start <= span.start);
    if candidate_index == 0 {
        return None;
    }
    let (owner_span, def_id) = entries[candidate_index - 1];
    span_contains(owner_span, span).then_some(def_id)
}

fn span_contains(outer: Span, inner: Span) -> bool {
    outer.file == inner.file && outer.start <= inner.start && inner.end <= outer.end
}
