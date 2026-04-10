mod cfg;
mod control;
mod dataflow;

use self::control::collect_control_facts;
use self::dataflow::{
    CfgTopology, ComputedLiveness, collect_binding_summaries, collect_def_uses,
    collect_definition_facts, collect_node_facts, collect_node_transfers, collect_resolved_uses,
    collect_single_source_uses, collect_use_defs, compute_liveness, compute_reaching_definitions,
    materialize_liveness, materialize_reaching_definitions,
};
use super::{
    AnalysisDeadStore, AnalysisDeadStoreKind, AnalysisFlowBinding, AnalysisFlowBindingId,
    AnalysisFlowBindingKind, AnalysisFlowBindingSummary, AnalysisFlowCfg, AnalysisFlowCfgEdge,
    AnalysisFlowCfgEdgeKind, AnalysisFlowCfgNode, AnalysisFlowCfgNodeKind, AnalysisFlowDefUse,
    AnalysisFlowDefinitionFacts, AnalysisFlowDefinitionKind, AnalysisFlowDefinitionRef,
    AnalysisFlowLiveness, AnalysisFlowNodeEffects, AnalysisFlowNodeFacts, AnalysisFlowNodeId,
    AnalysisFlowNodeTransfer, AnalysisFlowOwner, AnalysisFlowOwnerKind, AnalysisFlowReaching,
    AnalysisFlowRegion, AnalysisFlowRegionKind, AnalysisFlowResolvedUse,
    AnalysisFlowSingleSourceUse, AnalysisFlowSummary, AnalysisFlowUseDef,
};
use kernc_ast as ast;
use kernc_lower::{
    FlowLoweringElisionHints, FlowLoweringForwardingHints, FlowLoweringHints,
    FlowLoweringOwnerHints,
};
use kernc_sema::SemaContext;
use kernc_sema::def::{Def, DefId};
use kernc_sema::semantic::SemanticSymbolKind;
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::Span;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlowTiming {
    pub name: &'static str,
    pub duration: Duration,
}

#[derive(Clone, Default)]
pub struct FlowModel {
    owners: Vec<FlowOwnerFacts>,
    owner_body_lookup_by_file: HashMap<kernc_utils::FileId, Vec<(Span, DefId)>>,
    referenced_item_edges: Vec<(DefId, DefId)>,
    phase_timings: Vec<FlowTiming>,
}

#[derive(Clone)]
struct FlowOwnerFacts {
    def_id: DefId,
    definition_span: Span,
    owner_span: Span,
    body_span: Span,
    kind: AnalysisFlowOwnerKind,
    referenced_def_ids: Vec<DefId>,
    referenced_definition_spans: Vec<Span>,
    cfg: AnalysisFlowCfg,
    node_facts: Vec<AnalysisFlowNodeFacts>,
    node_effects: Vec<AnalysisFlowNodeEffects>,
    node_transfers: Vec<AnalysisFlowNodeTransfer>,
    use_defs: Vec<AnalysisFlowUseDef>,
    def_uses: Vec<AnalysisFlowDefUse>,
    definition_facts: Vec<AnalysisFlowDefinitionFacts>,
    resolved_uses: Vec<AnalysisFlowResolvedUse>,
    single_source_uses: Vec<AnalysisFlowSingleSourceUse>,
    liveness: Vec<AnalysisFlowLiveness>,
    computed_liveness: Option<ComputedLiveness>,
    reaching_definitions: Vec<AnalysisFlowReaching>,
    control_regions: Vec<FlowRegionFacts>,
    summary: AnalysisFlowSummary,
    bindings: Vec<FlowBindingFacts>,
    binding_summaries: Vec<AnalysisFlowBindingSummary>,
}

#[derive(Default)]
struct FlowOwnerOptimizationFacts {
    elision: FlowOwnerElisionFacts,
    forwarding: FlowOwnerForwardingFacts,
}

#[derive(Default)]
struct FlowOwnerElisionFacts {
    pure_dead_initializer_expr_ids: HashSet<kernc_utils::NodeId>,
    pure_dead_assignment_expr_ids: HashSet<kernc_utils::NodeId>,
    elidable_binding_expr_ids: HashSet<kernc_utils::NodeId>,
}

#[derive(Default)]
struct FlowOwnerForwardingFacts {
    identifier_copy_sources: HashMap<kernc_utils::NodeId, String>,
    forwardable_binding_sources: HashMap<kernc_utils::NodeId, String>,
    forwardable_value_expr_ids: HashSet<kernc_utils::NodeId>,
}

struct FlowOwnerOptimizationContext<'a, 'ctx> {
    owner: &'a FlowOwnerFacts,
    ctx: &'a SemaContext<'ctx>,
    owner_exprs: HashMap<kernc_utils::NodeId, &'a ast::Expr>,
    simple_binding_let_expr_ids: HashMap<Span, kernc_utils::NodeId>,
    bindings_by_id: HashMap<AnalysisFlowBindingId, &'a FlowBindingFacts>,
    binding_summaries_by_id: HashMap<AnalysisFlowBindingId, &'a AnalysisFlowBindingSummary>,
}

impl FlowOwnerOptimizationFacts {
    fn is_empty(&self) -> bool {
        self.elision.is_empty() && self.forwarding.is_empty()
    }

    fn into_lowering_hints(self) -> FlowLoweringOwnerHints {
        FlowLoweringOwnerHints {
            elision: FlowLoweringElisionHints {
                pure_dead_initializer_expr_ids: self.elision.pure_dead_initializer_expr_ids,
                pure_dead_assignment_expr_ids: self.elision.pure_dead_assignment_expr_ids,
                elidable_binding_expr_ids: self.elision.elidable_binding_expr_ids,
            },
            forwarding: FlowLoweringForwardingHints {
                identifier_copy_sources: self.forwarding.identifier_copy_sources,
                forwardable_binding_sources: self.forwarding.forwardable_binding_sources,
                forwardable_value_expr_ids: self.forwarding.forwardable_value_expr_ids,
            },
        }
    }
}

impl FlowOwnerElisionFacts {
    fn is_empty(&self) -> bool {
        self.pure_dead_initializer_expr_ids.is_empty()
            && self.pure_dead_assignment_expr_ids.is_empty()
            && self.elidable_binding_expr_ids.is_empty()
    }
}

impl FlowOwnerForwardingFacts {
    fn is_empty(&self) -> bool {
        self.identifier_copy_sources.is_empty()
            && self.forwardable_binding_sources.is_empty()
            && self.forwardable_value_expr_ids.is_empty()
    }
}

impl<'a, 'ctx> FlowOwnerOptimizationContext<'a, 'ctx> {
    fn new(owner: &'a FlowOwnerFacts, ctx: &'a SemaContext<'ctx>) -> Self {
        Self {
            owner,
            ctx,
            owner_exprs: owner_expr_map(ctx, owner.def_id),
            simple_binding_let_expr_ids: owner_simple_binding_let_expr_ids(ctx, owner.def_id),
            bindings_by_id: owner
                .bindings
                .iter()
                .map(|binding| (binding.id, binding))
                .collect(),
            binding_summaries_by_id: owner
                .binding_summaries
                .iter()
                .map(|summary| (summary.binding_id, summary))
                .collect(),
        }
    }

    fn collect(self) -> FlowOwnerOptimizationFacts {
        FlowOwnerOptimizationFacts {
            elision: self.collect_elision_facts(),
            forwarding: self.collect_forwarding_facts(),
        }
    }

    fn collect_elision_facts(&self) -> FlowOwnerElisionFacts {
        let purity_ctx = FlowBindingPurityContext {
            ctx: self.ctx,
            definition_facts: &self.owner.definition_facts,
            bindings_by_id: &self.bindings_by_id,
            binding_summaries_by_id: &self.binding_summaries_by_id,
            owner_exprs: &self.owner_exprs,
            simple_binding_let_expr_ids: &self.simple_binding_let_expr_ids,
        };
        let def_use_by_definition = self
            .owner
            .def_uses
            .iter()
            .map(|def_use| (def_use.definition, def_use))
            .collect::<HashMap<_, _>>();
        let definition_groups = self.owner.definition_facts.iter().fold(
            HashMap::<AnalysisFlowNodeId, Vec<&AnalysisFlowDefinitionFacts>>::new(),
            |mut groups, facts| {
                groups
                    .entry(facts.definition.node_id)
                    .or_default()
                    .push(facts);
                groups
            },
        );

        let mut facts = FlowOwnerElisionFacts::default();
        for (node_id, definition_facts) in definition_groups {
            let all_dead = definition_facts.iter().all(|definition_facts| {
                def_use_by_definition
                    .get(&definition_facts.definition)
                    .is_some_and(|def_use| def_use.use_node_ids.is_empty())
            });
            if !all_dead {
                continue;
            }

            let Some(ast_node_id) = self
                .owner
                .cfg
                .nodes
                .get(node_id.index())
                .and_then(|node| node.ast_node_id)
            else {
                continue;
            };
            let Some(expr) = self.owner_exprs.get(&ast_node_id).copied() else {
                continue;
            };

            match definition_facts[0].kind {
                AnalysisFlowDefinitionKind::Initializer
                    if removable_initializer_is_pure(self.ctx, expr) =>
                {
                    facts.pure_dead_initializer_expr_ids.insert(ast_node_id);
                }
                AnalysisFlowDefinitionKind::Assignment
                    if removable_assignment_is_pure(self.ctx, expr) =>
                {
                    facts.pure_dead_assignment_expr_ids.insert(ast_node_id);
                }
                _ => {}
            }
        }

        for binding in &self.owner.bindings {
            let Some(let_expr_id) = self
                .simple_binding_let_expr_ids
                .get(&binding.definition_span)
                .copied()
            else {
                continue;
            };
            if purity_ctx.is_elidable_pure_binding(binding.id) {
                facts.elidable_binding_expr_ids.insert(let_expr_id);
            }
        }

        facts
    }

    fn collect_forwarding_facts(&self) -> FlowOwnerForwardingFacts {
        let purity_ctx = FlowBindingPurityContext {
            ctx: self.ctx,
            definition_facts: &self.owner.definition_facts,
            bindings_by_id: &self.bindings_by_id,
            binding_summaries_by_id: &self.binding_summaries_by_id,
            owner_exprs: &self.owner_exprs,
            simple_binding_let_expr_ids: &self.simple_binding_let_expr_ids,
        };
        let mut facts = FlowOwnerForwardingFacts::default();

        for binding in &self.owner.bindings {
            let Some(let_expr_id) = self
                .simple_binding_let_expr_ids
                .get(&binding.definition_span)
                .copied()
            else {
                continue;
            };

            if purity_ctx.is_forwardable_pure_value_binding(binding.id) {
                facts.forwardable_value_expr_ids.insert(let_expr_id);
            }

            let Some(source_binding_id) = resolve_immutable_copy_origin_binding(
                binding.id,
                &self.owner.definition_facts,
                &self.bindings_by_id,
                &self.binding_summaries_by_id,
            ) else {
                continue;
            };
            if source_binding_id == binding.id {
                continue;
            }

            let Some(source_name) = self.binding_source_name(source_binding_id) else {
                continue;
            };
            facts
                .forwardable_binding_sources
                .insert(let_expr_id, source_name);
        }

        for single_source in &self.owner.single_source_uses {
            let Some(node) = self.owner.cfg.nodes.get(single_source.node_id.index()) else {
                continue;
            };
            let Some(use_expr_id) = node.ast_node_id else {
                continue;
            };
            let Some(use_expr) = self.owner_exprs.get(&use_expr_id).copied() else {
                continue;
            };
            if !matches!(use_expr.kind, ast::ExprKind::Identifier(_)) {
                continue;
            }

            let Some(source_binding_id) = resolve_immutable_copy_origin_binding(
                single_source.binding_id,
                &self.owner.definition_facts,
                &self.bindings_by_id,
                &self.binding_summaries_by_id,
            ) else {
                continue;
            };
            let Some(source_name) = self.binding_source_name(source_binding_id) else {
                continue;
            };
            facts
                .identifier_copy_sources
                .insert(use_expr_id, source_name);
        }

        facts
    }

    fn binding_source_name(&self, binding_id: AnalysisFlowBindingId) -> Option<String> {
        let source_binding = self.bindings_by_id.get(&binding_id).copied()?;
        let source_name = self
            .ctx
            .sess
            .source_manager
            .slice_source(source_binding.definition_span)
            .trim()
            .to_string();
        if source_name.is_empty() {
            None
        } else {
            Some(source_name)
        }
    }
}

#[derive(Clone)]
struct FlowBindingFacts {
    id: AnalysisFlowBindingId,
    definition_span: Span,
    kind: AnalysisFlowBindingKind,
    is_mut: bool,
    reference_spans: Vec<Span>,
}

#[derive(Clone, Copy)]
struct FlowRegionFacts {
    span: Span,
    kind: AnalysisFlowRegionKind,
}

#[derive(Clone, Copy)]
struct PendingEdge {
    from: AnalysisFlowNodeId,
    kind: AnalysisFlowCfgEdgeKind,
}

#[derive(Clone, Copy)]
struct LoopContext {
    break_target: AnalysisFlowNodeId,
    continue_target: AnalysisFlowNodeId,
}

struct FlowCfgBuilder<'a> {
    nodes: Vec<AnalysisFlowCfgNode>,
    edges: Vec<AnalysisFlowCfgEdge>,
    incoming_counts: Vec<usize>,
    node_uses: Vec<Vec<AnalysisFlowBindingId>>,
    node_value_uses: Vec<Vec<AnalysisFlowBindingId>>,
    node_defs: Vec<Vec<AnalysisFlowBindingId>>,
    node_def_kinds: Vec<Option<AnalysisFlowDefinitionKind>>,
    node_copy_sources: Vec<Option<AnalysisFlowBindingId>>,
    node_effects: Vec<AnalysisFlowNodeEffects>,
    local_bindings_by_span: &'a HashMap<Span, AnalysisFlowBindingId>,
    reference_to_binding: &'a HashMap<Span, AnalysisFlowBindingId>,
    entry: AnalysisFlowNodeId,
    exit: AnalysisFlowNodeId,
}

struct FlowCfgBuildResult {
    cfg: AnalysisFlowCfg,
    node_uses: Vec<Vec<AnalysisFlowBindingId>>,
    node_value_uses: Vec<Vec<AnalysisFlowBindingId>>,
    node_defs: Vec<Vec<AnalysisFlowBindingId>>,
    node_def_kinds: Vec<Option<AnalysisFlowDefinitionKind>>,
    node_copy_sources: Vec<Option<AnalysisFlowBindingId>>,
    node_effects: Vec<AnalysisFlowNodeEffects>,
}

impl FlowModel {
    pub(in crate::compiler) fn phase_timings(&self) -> &[FlowTiming] {
        &self.phase_timings
    }

    pub fn collect(
        ctx: &SemaContext<'_>,
        module_item_definition_spans: &HashMap<DefId, Span>,
        references: &[(Span, Span)],
    ) -> Self {
        Self::collect_with_mode(ctx, module_item_definition_spans, references, true)
    }

    pub fn collect_for_compile(
        ctx: &SemaContext<'_>,
        module_item_definition_spans: &HashMap<DefId, Span>,
        references: &[(Span, Span)],
    ) -> Self {
        Self::collect_with_mode(ctx, module_item_definition_spans, references, false)
    }

    fn collect_with_mode(
        ctx: &SemaContext<'_>,
        module_item_definition_spans: &HashMap<DefId, Span>,
        references: &[(Span, Span)],
        include_analysis_details: bool,
    ) -> Self {
        let mut phase_totals = HashMap::<&'static str, Duration>::new();
        let record =
            |totals: &mut HashMap<&'static str, Duration>, name: &'static str, started: Instant| {
                *totals.entry(name).or_default() += started.elapsed();
            };
        let mut owners = Vec::new();

        for def in &ctx.defs {
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
                        body_span: global.value.span,
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
            dedup_preserving_order(&mut owner.referenced_def_ids);
            dedup_preserving_order(&mut owner.referenced_definition_spans);
            for binding in &mut owner.bindings {
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
                    owner.node_facts = collect_node_facts(
                        &owner.cfg,
                        &cfg_build.node_uses,
                        &cfg_build.node_defs,
                        &cfg_build.node_def_kinds,
                    );
                    record(&mut phase_totals, "  flow_node_facts", started);
                    let started = Instant::now();
                    let node_transfers = collect_node_transfers(&owner.node_facts);
                    record(&mut phase_totals, "  flow_node_transfers", started);
                    let topology = CfgTopology::from_cfg(&owner.cfg);
                    let started = Instant::now();
                    let computed_liveness = compute_liveness(&topology, &node_transfers);
                    record(&mut phase_totals, "  flow_liveness", started);
                    let started = Instant::now();
                    let computed_reaching =
                        compute_reaching_definitions(&topology, &node_transfers);
                    record(&mut phase_totals, "  flow_reaching", started);
                    let started = Instant::now();
                    let use_defs = collect_use_defs(&owner.node_facts, &computed_reaching);
                    record(&mut phase_totals, "  flow_use_defs", started);
                    let started = Instant::now();
                    owner.def_uses = collect_def_uses(&node_transfers, &use_defs);
                    record(&mut phase_totals, "  flow_def_uses", started);
                    let started = Instant::now();
                    owner.definition_facts = collect_definition_facts(
                        &owner.node_facts,
                        &cfg_build.node_value_uses,
                        &cfg_build.node_copy_sources,
                    );
                    record(&mut phase_totals, "  flow_definition_facts", started);
                    let started = Instant::now();
                    let resolved_uses = collect_resolved_uses(&use_defs);
                    record(&mut phase_totals, "  flow_resolved_uses", started);
                    let started = Instant::now();
                    owner.single_source_uses =
                        collect_single_source_uses(&resolved_uses, &owner.definition_facts);
                    record(&mut phase_totals, "  flow_single_source_uses", started);
                    let started = Instant::now();
                    owner.binding_summaries = collect_binding_summaries(
                        &owner.bindings,
                        &owner.cfg,
                        &owner.node_facts,
                        &computed_liveness,
                    );
                    record(&mut phase_totals, "  flow_binding_summaries", started);
                    owner.computed_liveness = Some(computed_liveness.clone());
                    if include_analysis_details {
                        let started = Instant::now();
                        let (control_regions, summary) = collect_control_facts(body);
                        record(&mut phase_totals, "  flow_control", started);
                        owner.node_effects = cfg_build.node_effects;
                        owner.node_transfers = node_transfers;
                        owner.reaching_definitions =
                            materialize_reaching_definitions(&owner.cfg, &computed_reaching);
                        owner.liveness = materialize_liveness(
                            &owner.cfg,
                            owner.computed_liveness.as_ref().unwrap(),
                        );
                        owner.use_defs = use_defs;
                        owner.resolved_uses = resolved_uses;
                        owner.control_regions = control_regions;
                        owner.summary = summary;
                    }
                }
                Def::Global(global) => {
                    let started = Instant::now();
                    let cfg_build = FlowCfgBuilder::build(
                        &global.value,
                        global.span,
                        &binding_ids_by_span,
                        &reference_to_binding,
                    );
                    record(&mut phase_totals, "  flow_cfg_build", started);
                    owner.cfg = cfg_build.cfg;
                    let started = Instant::now();
                    owner.node_facts = collect_node_facts(
                        &owner.cfg,
                        &cfg_build.node_uses,
                        &cfg_build.node_defs,
                        &cfg_build.node_def_kinds,
                    );
                    record(&mut phase_totals, "  flow_node_facts", started);
                    let started = Instant::now();
                    let node_transfers = collect_node_transfers(&owner.node_facts);
                    record(&mut phase_totals, "  flow_node_transfers", started);
                    let topology = CfgTopology::from_cfg(&owner.cfg);
                    let started = Instant::now();
                    let computed_liveness = compute_liveness(&topology, &node_transfers);
                    record(&mut phase_totals, "  flow_liveness", started);
                    let started = Instant::now();
                    let computed_reaching =
                        compute_reaching_definitions(&topology, &node_transfers);
                    record(&mut phase_totals, "  flow_reaching", started);
                    let started = Instant::now();
                    let use_defs = collect_use_defs(&owner.node_facts, &computed_reaching);
                    record(&mut phase_totals, "  flow_use_defs", started);
                    let started = Instant::now();
                    owner.def_uses = collect_def_uses(&node_transfers, &use_defs);
                    record(&mut phase_totals, "  flow_def_uses", started);
                    let started = Instant::now();
                    owner.definition_facts = collect_definition_facts(
                        &owner.node_facts,
                        &cfg_build.node_value_uses,
                        &cfg_build.node_copy_sources,
                    );
                    record(&mut phase_totals, "  flow_definition_facts", started);
                    let started = Instant::now();
                    let resolved_uses = collect_resolved_uses(&use_defs);
                    record(&mut phase_totals, "  flow_resolved_uses", started);
                    let started = Instant::now();
                    owner.single_source_uses =
                        collect_single_source_uses(&resolved_uses, &owner.definition_facts);
                    record(&mut phase_totals, "  flow_single_source_uses", started);
                    let started = Instant::now();
                    owner.binding_summaries = collect_binding_summaries(
                        &owner.bindings,
                        &owner.cfg,
                        &owner.node_facts,
                        &computed_liveness,
                    );
                    record(&mut phase_totals, "  flow_binding_summaries", started);
                    owner.computed_liveness = Some(computed_liveness.clone());
                    if include_analysis_details {
                        let started = Instant::now();
                        let (control_regions, summary) = collect_control_facts(&global.value);
                        record(&mut phase_totals, "  flow_control", started);
                        owner.node_effects = cfg_build.node_effects;
                        owner.node_transfers = node_transfers;
                        owner.reaching_definitions =
                            materialize_reaching_definitions(&owner.cfg, &computed_reaching);
                        owner.liveness = materialize_liveness(
                            &owner.cfg,
                            owner.computed_liveness.as_ref().unwrap(),
                        );
                        owner.use_defs = use_defs;
                        owner.resolved_uses = resolved_uses;
                        owner.control_regions = control_regions;
                        owner.summary = summary;
                    }
                }
                _ => {}
            }
        }

        let owner_body_lookup_by_file = build_owner_def_lookup_by_file(&owners);
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
        Self {
            owners,
            owner_body_lookup_by_file,
            referenced_item_edges,
            phase_timings,
        }
    }

    pub fn owner_def_id(&self, reference_span: Span) -> Option<DefId> {
        find_owner_def_id(&self.owner_body_lookup_by_file, reference_span)
    }

    pub fn referenced_item_edges(&self) -> &[(DefId, DefId)] {
        &self.referenced_item_edges
    }

    pub fn public_owners(&self) -> Vec<AnalysisFlowOwner> {
        self.owners
            .iter()
            .map(|owner| AnalysisFlowOwner {
                definition_span: owner.definition_span,
                body_span: owner.body_span,
                kind: owner.kind,
                referenced_definition_spans: owner.referenced_definition_spans.clone(),
                cfg: owner.cfg.clone(),
                node_facts: owner.node_facts.clone(),
                node_effects: owner.node_effects.clone(),
                node_transfers: owner.node_transfers.clone(),
                use_defs: owner.use_defs.clone(),
                def_uses: owner.def_uses.clone(),
                definition_facts: owner.definition_facts.clone(),
                resolved_uses: owner.resolved_uses.clone(),
                single_source_uses: owner.single_source_uses.clone(),
                liveness: owner.liveness.clone(),
                reaching_definitions: owner.reaching_definitions.clone(),
                control_regions: owner
                    .control_regions
                    .iter()
                    .map(|region| AnalysisFlowRegion {
                        span: region.span,
                        kind: region.kind,
                    })
                    .collect(),
                summary: owner.summary,
                bindings: owner
                    .bindings
                    .iter()
                    .map(|binding| AnalysisFlowBinding {
                        id: binding.id,
                        definition_span: binding.definition_span,
                        kind: binding.kind,
                        is_mut: binding.is_mut,
                        reference_spans: binding.reference_spans.clone(),
                    })
                    .collect(),
                binding_summaries: owner.binding_summaries.clone(),
            })
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

fn collect_owner_optimization_facts(
    owner: &FlowOwnerFacts,
    ctx: &SemaContext<'_>,
) -> FlowOwnerOptimizationFacts {
    FlowOwnerOptimizationContext::new(owner, ctx).collect()
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

fn owner_expr_map<'a>(
    ctx: &'a SemaContext<'_>,
    def_id: DefId,
) -> HashMap<kernc_utils::NodeId, &'a ast::Expr> {
    let mut exprs = HashMap::new();

    match &ctx.defs[def_id.0 as usize] {
        Def::Function(function) => {
            if let Some(body) = function.body.as_ref() {
                collect_owner_exprs(body, &mut exprs);
            }
        }
        Def::Global(global) => {
            collect_owner_exprs(&global.value, &mut exprs);
        }
        _ => {}
    }

    exprs
}

fn owner_simple_binding_let_expr_ids(
    ctx: &SemaContext<'_>,
    def_id: DefId,
) -> HashMap<Span, kernc_utils::NodeId> {
    let mut expr_ids = HashMap::new();

    match &ctx.defs[def_id.0 as usize] {
        Def::Function(function) => {
            if let Some(body) = function.body.as_ref() {
                collect_simple_binding_let_expr_ids(body, &mut expr_ids);
            }
        }
        Def::Global(global) => {
            collect_simple_binding_let_expr_ids(&global.value, &mut expr_ids);
        }
        _ => {}
    }

    expr_ids
}

fn collect_owner_exprs<'a>(
    expr: &'a ast::Expr,
    exprs: &mut HashMap<kernc_utils::NodeId, &'a ast::Expr>,
) {
    exprs.insert(expr.id, expr);

    match &expr.kind {
        ast::ExprKind::Let {
            init, else_branch, ..
        } => {
            collect_owner_exprs(init, exprs);
            if let Some(else_branch) = else_branch {
                collect_owner_exprs(else_branch, exprs);
            }
        }
        ast::ExprKind::Static { init, .. } => collect_owner_exprs(init, exprs),
        ast::ExprKind::Binary { lhs, rhs, .. } => {
            collect_owner_exprs(lhs, exprs);
            collect_owner_exprs(rhs, exprs);
        }
        ast::ExprKind::Unary { operand, .. } => collect_owner_exprs(operand, exprs),
        ast::ExprKind::FieldAccess { lhs, .. } => collect_owner_exprs(lhs, exprs),
        ast::ExprKind::IndexAccess { lhs, index, .. } => {
            collect_owner_exprs(lhs, exprs);
            collect_owner_exprs(index, exprs);
        }
        ast::ExprKind::Call { callee, args } => {
            collect_owner_exprs(callee, exprs);
            for arg in args {
                collect_owner_exprs(arg, exprs);
            }
        }
        ast::ExprKind::DataInit { literal, .. } => match literal {
            ast::DataLiteralKind::Struct(fields) => {
                for field in fields {
                    collect_owner_exprs(&field.value, exprs);
                }
            }
            ast::DataLiteralKind::Array(items) => {
                for item in items {
                    collect_owner_exprs(item, exprs);
                }
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                collect_owner_exprs(value, exprs);
                collect_owner_exprs(count, exprs);
            }
            ast::DataLiteralKind::Scalar(value) => collect_owner_exprs(value, exprs),
        },
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_owner_exprs(cond, exprs);
            collect_owner_exprs(then_branch, exprs);
            if let Some(else_branch) = else_branch {
                collect_owner_exprs(else_branch, exprs);
            }
        }
        ast::ExprKind::Match { target, arms } => {
            collect_owner_exprs(target, exprs);
            for arm in arms {
                collect_owner_exprs(&arm.body, exprs);
            }
        }
        ast::ExprKind::Block { stmts, result } => {
            for stmt in stmts {
                match &stmt.kind {
                    ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                        collect_owner_exprs(expr, exprs);
                    }
                }
            }
            if let Some(result) = result {
                collect_owner_exprs(result, exprs);
            }
        }
        ast::ExprKind::For {
            init,
            cond,
            post,
            body,
        } => {
            if let Some(init) = init {
                collect_owner_exprs(init, exprs);
            }
            if let Some(cond) = cond {
                collect_owner_exprs(cond, exprs);
            }
            if let Some(post) = post {
                collect_owner_exprs(post, exprs);
            }
            collect_owner_exprs(body, exprs);
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_owner_exprs(lhs, exprs);
            if let Some(start) = start {
                collect_owner_exprs(start, exprs);
            }
            if let Some(end) = end {
                collect_owner_exprs(end, exprs);
            }
        }
        ast::ExprKind::Defer { expr } => collect_owner_exprs(expr, exprs),
        ast::ExprKind::Return(value) => {
            if let Some(value) = value {
                collect_owner_exprs(value, exprs);
            }
        }
        ast::ExprKind::Assign { lhs, rhs, .. } => {
            collect_owner_exprs(lhs, exprs);
            collect_owner_exprs(rhs, exprs);
        }
        ast::ExprKind::As { lhs, .. } => collect_owner_exprs(lhs, exprs),
        ast::ExprKind::GenericInstantiation { target, .. } => collect_owner_exprs(target, exprs),
        ast::ExprKind::Closure { captures, body, .. } => {
            for capture in captures {
                collect_owner_exprs(&capture.value, exprs);
            }
            collect_owner_exprs(body, exprs);
        }
        ast::ExprKind::Integer(_)
        | ast::ExprKind::Float(_)
        | ast::ExprKind::Bool(_)
        | ast::ExprKind::Char(_)
        | ast::ExprKind::ByteChar(_)
        | ast::ExprKind::String(_)
        | ast::ExprKind::Identifier(_)
        | ast::ExprKind::EnumLiteral { .. }
        | ast::ExprKind::SelfValue
        | ast::ExprKind::Undef
        | ast::ExprKind::Infer
        | ast::ExprKind::Break
        | ast::ExprKind::Continue => {}
    }
}

fn collect_simple_binding_let_expr_ids(
    expr: &ast::Expr,
    expr_ids: &mut HashMap<Span, kernc_utils::NodeId>,
) {
    if let ast::ExprKind::Let {
        pattern,
        else_branch,
        ..
    } = &expr.kind
        && else_branch.is_none()
        && let ast::PatternKind::Binding(binding) = &pattern.pattern.kind
    {
        expr_ids.insert(binding.name_span, expr.id);
    }

    match &expr.kind {
        ast::ExprKind::Let {
            init, else_branch, ..
        } => {
            collect_simple_binding_let_expr_ids(init, expr_ids);
            if let Some(else_branch) = else_branch {
                collect_simple_binding_let_expr_ids(else_branch, expr_ids);
            }
        }
        ast::ExprKind::Static { init, .. } => collect_simple_binding_let_expr_ids(init, expr_ids),
        ast::ExprKind::Binary { lhs, rhs, .. } => {
            collect_simple_binding_let_expr_ids(lhs, expr_ids);
            collect_simple_binding_let_expr_ids(rhs, expr_ids);
        }
        ast::ExprKind::Unary { operand, .. } => {
            collect_simple_binding_let_expr_ids(operand, expr_ids);
        }
        ast::ExprKind::FieldAccess { lhs, .. } => {
            collect_simple_binding_let_expr_ids(lhs, expr_ids)
        }
        ast::ExprKind::IndexAccess { lhs, index, .. } => {
            collect_simple_binding_let_expr_ids(lhs, expr_ids);
            collect_simple_binding_let_expr_ids(index, expr_ids);
        }
        ast::ExprKind::Call { callee, args } => {
            collect_simple_binding_let_expr_ids(callee, expr_ids);
            for arg in args {
                collect_simple_binding_let_expr_ids(arg, expr_ids);
            }
        }
        ast::ExprKind::DataInit { literal, .. } => match literal {
            ast::DataLiteralKind::Struct(fields) => {
                for field in fields {
                    collect_simple_binding_let_expr_ids(&field.value, expr_ids);
                }
            }
            ast::DataLiteralKind::Array(items) => {
                for item in items {
                    collect_simple_binding_let_expr_ids(item, expr_ids);
                }
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                collect_simple_binding_let_expr_ids(value, expr_ids);
                collect_simple_binding_let_expr_ids(count, expr_ids);
            }
            ast::DataLiteralKind::Scalar(value) => {
                collect_simple_binding_let_expr_ids(value, expr_ids);
            }
        },
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_simple_binding_let_expr_ids(cond, expr_ids);
            collect_simple_binding_let_expr_ids(then_branch, expr_ids);
            if let Some(else_branch) = else_branch {
                collect_simple_binding_let_expr_ids(else_branch, expr_ids);
            }
        }
        ast::ExprKind::Match { target, arms } => {
            collect_simple_binding_let_expr_ids(target, expr_ids);
            for arm in arms {
                collect_simple_binding_let_expr_ids(&arm.body, expr_ids);
            }
        }
        ast::ExprKind::Block { stmts, result } => {
            for stmt in stmts {
                match &stmt.kind {
                    ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                        collect_simple_binding_let_expr_ids(expr, expr_ids);
                    }
                }
            }
            if let Some(result) = result {
                collect_simple_binding_let_expr_ids(result, expr_ids);
            }
        }
        ast::ExprKind::For {
            init,
            cond,
            post,
            body,
        } => {
            if let Some(init) = init {
                collect_simple_binding_let_expr_ids(init, expr_ids);
            }
            if let Some(cond) = cond {
                collect_simple_binding_let_expr_ids(cond, expr_ids);
            }
            if let Some(post) = post {
                collect_simple_binding_let_expr_ids(post, expr_ids);
            }
            collect_simple_binding_let_expr_ids(body, expr_ids);
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_simple_binding_let_expr_ids(lhs, expr_ids);
            if let Some(start) = start {
                collect_simple_binding_let_expr_ids(start, expr_ids);
            }
            if let Some(end) = end {
                collect_simple_binding_let_expr_ids(end, expr_ids);
            }
        }
        ast::ExprKind::Defer { expr } => collect_simple_binding_let_expr_ids(expr, expr_ids),
        ast::ExprKind::Return(value) => {
            if let Some(value) = value {
                collect_simple_binding_let_expr_ids(value, expr_ids);
            }
        }
        ast::ExprKind::Assign { lhs, rhs, .. } => {
            collect_simple_binding_let_expr_ids(lhs, expr_ids);
            collect_simple_binding_let_expr_ids(rhs, expr_ids);
        }
        ast::ExprKind::As { lhs, .. } => collect_simple_binding_let_expr_ids(lhs, expr_ids),
        ast::ExprKind::GenericInstantiation { target, .. } => {
            collect_simple_binding_let_expr_ids(target, expr_ids);
        }
        ast::ExprKind::Closure { captures, body, .. } => {
            for capture in captures {
                collect_simple_binding_let_expr_ids(&capture.value, expr_ids);
            }
            collect_simple_binding_let_expr_ids(body, expr_ids);
        }
        ast::ExprKind::Integer(_)
        | ast::ExprKind::Float(_)
        | ast::ExprKind::Bool(_)
        | ast::ExprKind::Char(_)
        | ast::ExprKind::ByteChar(_)
        | ast::ExprKind::String(_)
        | ast::ExprKind::Identifier(_)
        | ast::ExprKind::EnumLiteral { .. }
        | ast::ExprKind::SelfValue
        | ast::ExprKind::Undef
        | ast::ExprKind::Infer
        | ast::ExprKind::Break
        | ast::ExprKind::Continue => {}
    }
}

fn removable_initializer_is_pure(ctx: &SemaContext<'_>, expr: &ast::Expr) -> bool {
    let ast::ExprKind::Let { init, .. } = &expr.kind else {
        return false;
    };
    expr_is_strictly_pure(ctx, init)
}

fn removable_assignment_is_pure(ctx: &SemaContext<'_>, expr: &ast::Expr) -> bool {
    let ast::ExprKind::Assign { lhs, op, rhs } = &expr.kind else {
        return false;
    };
    matches!(lhs.kind, ast::ExprKind::Identifier(_))
        && *op == ast::AssignmentOperator::Assign
        && expr_is_strictly_pure(ctx, rhs)
}

fn expr_is_strictly_pure(ctx: &SemaContext<'_>, expr: &ast::Expr) -> bool {
    match &expr.kind {
        ast::ExprKind::Integer(_)
        | ast::ExprKind::Float(_)
        | ast::ExprKind::Bool(_)
        | ast::ExprKind::Char(_)
        | ast::ExprKind::ByteChar(_)
        | ast::ExprKind::String(_)
        | ast::ExprKind::Identifier(_)
        | ast::ExprKind::EnumLiteral { .. }
        | ast::ExprKind::SelfValue
        | ast::ExprKind::Undef
        | ast::ExprKind::Infer => true,
        ast::ExprKind::Unary { op, operand } => {
            !matches!(
                op,
                ast::UnaryOperator::PointerDeRef
                    | ast::UnaryOperator::AddressOf
                    | ast::UnaryOperator::MutAddressOf
            ) && expr_is_strictly_pure(ctx, operand)
        }
        ast::ExprKind::Binary { lhs, rhs, .. } => {
            expr_is_strictly_pure(ctx, lhs) && expr_is_strictly_pure(ctx, rhs)
        }
        ast::ExprKind::DataInit { literal, .. } => {
            let ty = ctx
                .node_types
                .get(&expr.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let norm_ty = ctx.type_registry.normalize(ty);

            if !matches!(ctx.type_registry.get(norm_ty), TypeKind::Primitive(_)) {
                return false;
            }

            match literal {
                ast::DataLiteralKind::Struct(fields) => fields
                    .iter()
                    .all(|field| expr_is_strictly_pure(ctx, &field.value)),
                ast::DataLiteralKind::Array(items) => {
                    items.iter().all(|item| expr_is_strictly_pure(ctx, item))
                }
                ast::DataLiteralKind::Repeat { value, count } => {
                    expr_is_strictly_pure(ctx, value) && expr_is_strictly_pure(ctx, count)
                }
                ast::DataLiteralKind::Scalar(value) => expr_is_strictly_pure(ctx, value),
            }
        }
        ast::ExprKind::As { lhs, .. } => expr_is_strictly_pure(ctx, lhs),
        ast::ExprKind::GenericInstantiation { target, .. } => expr_is_strictly_pure(ctx, target),
        ast::ExprKind::Closure { captures, .. } => captures
            .iter()
            .all(|capture| expr_is_strictly_pure(ctx, &capture.value)),
        ast::ExprKind::FieldAccess { .. }
        | ast::ExprKind::IndexAccess { .. }
        | ast::ExprKind::Call { .. }
        | ast::ExprKind::If { .. }
        | ast::ExprKind::Match { .. }
        | ast::ExprKind::Block { .. }
        | ast::ExprKind::For { .. }
        | ast::ExprKind::SliceOp { .. }
        | ast::ExprKind::Defer { .. }
        | ast::ExprKind::Return(_)
        | ast::ExprKind::Assign { .. }
        | ast::ExprKind::Let { .. }
        | ast::ExprKind::Static { .. }
        | ast::ExprKind::Break
        | ast::ExprKind::Continue => false,
    }
}

fn resolve_immutable_copy_origin_binding(
    binding_id: AnalysisFlowBindingId,
    definition_facts: &[AnalysisFlowDefinitionFacts],
    bindings_by_id: &HashMap<AnalysisFlowBindingId, &FlowBindingFacts>,
    binding_summaries_by_id: &HashMap<AnalysisFlowBindingId, &AnalysisFlowBindingSummary>,
) -> Option<AnalysisFlowBindingId> {
    let mut current = binding_id;

    loop {
        let binding = bindings_by_id.get(&current).copied()?;
        if binding.kind == AnalysisFlowBindingKind::Parameter && !binding.is_mut {
            return Some(current);
        }
        if binding.kind != AnalysisFlowBindingKind::Variable || binding.is_mut {
            return None;
        }

        let summary = binding_summaries_by_id.get(&current).copied()?;
        if summary.definition_node_ids.len() != 1 {
            return None;
        }

        let definition_facts = definition_facts.iter().find(|facts| {
            facts.definition.binding_id == current
                && facts.definition.node_id == summary.definition_node_ids[0]
                && facts.kind == AnalysisFlowDefinitionKind::Initializer
        })?;

        let source_binding_id = definition_facts.copy_source_binding_id?;
        if source_binding_id == current {
            return None;
        }
        current = source_binding_id;
    }
}

struct FlowBindingPurityContext<'a, 'ctx> {
    ctx: &'a SemaContext<'ctx>,
    definition_facts: &'a [AnalysisFlowDefinitionFacts],
    bindings_by_id: &'a HashMap<AnalysisFlowBindingId, &'a FlowBindingFacts>,
    binding_summaries_by_id: &'a HashMap<AnalysisFlowBindingId, &'a AnalysisFlowBindingSummary>,
    owner_exprs: &'a HashMap<kernc_utils::NodeId, &'a ast::Expr>,
    simple_binding_let_expr_ids: &'a HashMap<Span, kernc_utils::NodeId>,
}

impl FlowBindingPurityContext<'_, '_> {
    fn is_elidable_pure_binding(&self, binding_id: AnalysisFlowBindingId) -> bool {
        let Some(binding) = self.bindings_by_id.get(&binding_id).copied() else {
            return false;
        };
        if binding.kind != AnalysisFlowBindingKind::Variable || binding.is_mut {
            return false;
        }

        let Some(summary) = self.binding_summaries_by_id.get(&binding_id).copied() else {
            return false;
        };
        if !summary.use_node_ids.is_empty() || summary.definition_node_ids.len() != 1 {
            return false;
        }

        let Some(let_expr_id) = self
            .simple_binding_let_expr_ids
            .get(&binding.definition_span)
            .copied()
        else {
            return false;
        };
        let Some(let_expr) = self.owner_exprs.get(&let_expr_id).copied() else {
            return false;
        };

        removable_initializer_is_pure(self.ctx, let_expr)
    }

    fn is_forwardable_pure_value_binding(&self, binding_id: AnalysisFlowBindingId) -> bool {
        let mut visiting = HashSet::new();
        let mut memo = HashMap::new();
        self.is_forwardable_pure_value_binding_inner(binding_id, &mut visiting, &mut memo)
    }

    fn is_forwardable_pure_value_binding_inner(
        &self,
        binding_id: AnalysisFlowBindingId,
        visiting: &mut HashSet<AnalysisFlowBindingId>,
        memo: &mut HashMap<AnalysisFlowBindingId, bool>,
    ) -> bool {
        if let Some(result) = memo.get(&binding_id).copied() {
            return result;
        }
        if !visiting.insert(binding_id) {
            return false;
        }

        let result = match self.bindings_by_id.get(&binding_id).copied() {
            Some(binding) if binding.kind == AnalysisFlowBindingKind::Parameter => !binding.is_mut,
            Some(binding)
                if binding.kind == AnalysisFlowBindingKind::Variable && !binding.is_mut =>
            {
                match self.binding_summaries_by_id.get(&binding_id).copied() {
                    Some(summary) if summary.definition_node_ids.len() == 1 => {
                        match self
                            .simple_binding_let_expr_ids
                            .get(&binding.definition_span)
                            .copied()
                        {
                            Some(let_expr_id) => {
                                match self.owner_exprs.get(&let_expr_id).copied() {
                                    Some(let_expr)
                                        if removable_initializer_is_pure(self.ctx, let_expr) =>
                                    {
                                        let definition =
                                            self.definition_facts.iter().find(|facts| {
                                                facts.definition.binding_id == binding_id
                                                    && facts.definition.node_id
                                                        == summary.definition_node_ids[0]
                                                    && facts.kind
                                                        == AnalysisFlowDefinitionKind::Initializer
                                            });
                                        definition.is_some_and(|facts| {
                                            facts.use_binding_ids.iter().all(|used_binding_id| {
                                                self.is_forwardable_pure_value_binding_inner(
                                                    *used_binding_id,
                                                    visiting,
                                                    memo,
                                                )
                                            })
                                        })
                                    }
                                    _ => false,
                                }
                            }
                            None => false,
                        }
                    }
                    _ => false,
                }
            }
            _ => false,
        };

        visiting.remove(&binding_id);
        memo.insert(binding_id, result);
        result
    }
}
