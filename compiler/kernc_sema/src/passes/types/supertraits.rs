use super::*;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TraitSupertraitEdge {
    source_trait_id: DefId,
    supertrait_index: usize,
    span: Span,
    target_trait_id: DefId,
}

struct SupertraitCycleSearch<'a> {
    visited: &'a mut HashSet<DefId>,
    stack: &'a mut Vec<DefId>,
    stack_edges: &'a mut Vec<TraitSupertraitEdge>,
    on_stack: &'a mut HashMap<DefId, usize>,
    reported_cycles: &'a mut HashSet<Vec<u32>>,
    edges_to_break: &'a mut HashSet<(DefId, usize)>,
}

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    pub(super) fn validate_supertrait_graph(&mut self) {
        let trait_ids = self
            .ctx
            .defs
            .iter()
            .filter_map(|def| match def {
                Def::Trait(trait_def) => Some(trait_def.id),
                _ => None,
            })
            .collect::<Vec<_>>();

        let mut visited = HashSet::new();
        let mut stack = Vec::new();
        let mut stack_edges = Vec::new();
        let mut on_stack = HashMap::new();
        let mut reported_cycles = HashSet::new();
        let mut edges_to_break = HashSet::new();

        for trait_id in trait_ids {
            let mut search = SupertraitCycleSearch {
                visited: &mut visited,
                stack: &mut stack,
                stack_edges: &mut stack_edges,
                on_stack: &mut on_stack,
                reported_cycles: &mut reported_cycles,
                edges_to_break: &mut edges_to_break,
            };
            self.find_supertrait_cycles_from(trait_id, &mut search);
        }

        for (trait_id, supertrait_index) in edges_to_break {
            let Some(Def::Trait(trait_def)) = self.ctx.defs.get_mut(trait_id.0 as usize) else {
                continue;
            };
            if let Some(supertrait_ty) = trait_def.resolved_supertraits.get_mut(supertrait_index) {
                *supertrait_ty = TypeId::ERROR;
            }
        }
    }

    fn find_supertrait_cycles_from(
        &mut self,
        trait_id: DefId,
        search: &mut SupertraitCycleSearch<'_>,
    ) {
        if !search.visited.insert(trait_id) {
            return;
        }

        search.on_stack.insert(trait_id, search.stack.len());
        search.stack.push(trait_id);

        for edge in self.trait_supertrait_edges(trait_id) {
            if let Some(&cycle_start) = search.on_stack.get(&edge.target_trait_id) {
                let mut cycle_edges = search.stack_edges[cycle_start..].to_vec();
                cycle_edges.push(edge);

                let cycle_trait_ids = search.stack[cycle_start..].to_vec();
                let cycle_key = canonical_cycle_key(&cycle_trait_ids);
                if search.reported_cycles.insert(cycle_key) {
                    self.report_supertrait_cycle(&cycle_trait_ids, &cycle_edges);
                }
                for cycle_edge in cycle_edges {
                    search
                        .edges_to_break
                        .insert((cycle_edge.source_trait_id, cycle_edge.supertrait_index));
                }
                continue;
            }

            if search.visited.contains(&edge.target_trait_id) {
                continue;
            }

            search.stack_edges.push(edge);
            self.find_supertrait_cycles_from(edge.target_trait_id, search);
            search.stack_edges.pop();
        }

        search.stack.pop();
        search.on_stack.remove(&trait_id);
    }

    fn trait_supertrait_edges(&mut self, trait_id: DefId) -> Vec<TraitSupertraitEdge> {
        let Some(Def::Trait(trait_def)) = self.ctx.defs.get(trait_id.0 as usize).cloned() else {
            return Vec::new();
        };

        trait_def
            .resolved_supertraits
            .iter()
            .enumerate()
            .filter_map(|(index, &supertrait_ty)| {
                let supertrait_norm = self.ctx.type_registry.normalize(supertrait_ty);
                let TypeKind::TraitObject(target_trait_id, _, _) =
                    self.ctx.type_registry.get(supertrait_norm)
                else {
                    return None;
                };

                Some(TraitSupertraitEdge {
                    source_trait_id: trait_id,
                    supertrait_index: index,
                    span: trait_def
                        .supertraits
                        .get(index)
                        .map(|supertrait| supertrait.span)
                        .unwrap_or(trait_def.span),
                    target_trait_id: *target_trait_id,
                })
            })
            .collect()
    }

    fn report_supertrait_cycle(
        &mut self,
        cycle_trait_ids: &[DefId],
        cycle_edges: &[TraitSupertraitEdge],
    ) {
        if cycle_trait_ids.is_empty() || cycle_edges.is_empty() {
            return;
        }

        let cycle_names = cycle_trait_ids
            .iter()
            .map(|&trait_id| self.trait_name(trait_id))
            .collect::<Vec<_>>();
        let mut cycle_chain = cycle_names.clone();
        cycle_chain.push(cycle_names[0].clone());
        let edge_labels = cycle_edges
            .iter()
            .map(|edge| {
                let source_name = self.trait_name(edge.source_trait_id);
                let target_name = self.trait_name(edge.target_trait_id);
                (
                    edge.span,
                    format!("`{source_name}` depends on `{target_name}` here"),
                )
            })
            .collect::<Vec<_>>();
        let growth_hints = cycle_edges
            .iter()
            .filter_map(|edge| {
                let Def::Trait(trait_def) = &self.ctx.defs[edge.source_trait_id.0 as usize] else {
                    return None;
                };
                self.ctx
                    .compare_paterson_supertrait_against_generics(
                        &trait_def.generics,
                        trait_def.resolved_supertraits[edge.supertrait_index],
                    )
                    .map(|issue| {
                        format!(
                            "cycle edge `{} -> {}` is non-decreasing: {}",
                            self.trait_name(edge.source_trait_id),
                            self.trait_name(edge.target_trait_id),
                            self.ctx.describe_paterson_issue(&issue)
                        )
                    })
            })
            .collect::<Vec<_>>();

        let primary_edge = cycle_edges[0];
        let mut diag = self
            .ctx
            .struct_error(
                primary_edge.span,
                "trait supertrait hierarchy contains a cycle",
            )
            .with_hint(format!("cycle: {}", cycle_chain.join(" -> ")))
            .with_hint(
                "supertrait hierarchies must be acyclic so trait proof search and vtable layout remain finite",
            )
            .with_hint(
                "supertraits are stricter than impl prerequisites: the hierarchy itself must form a DAG, so even an equal-size cycle is rejected",
            );

        for (span, label) in edge_labels {
            diag = diag.with_span_label(span, label);
        }
        for hint in growth_hints {
            diag = diag.with_hint(hint);
        }

        diag.emit();
    }

    fn trait_name(&self, trait_id: DefId) -> String {
        self.ctx
            .defs
            .get(trait_id.0 as usize)
            .and_then(|def| match def {
                Def::Trait(trait_def) => Some(self.ctx.resolve(trait_def.name).to_string()),
                _ => None,
            })
            .unwrap_or_else(|| format!("<trait {}>", trait_id.0))
    }
}

fn canonical_cycle_key(cycle_trait_ids: &[DefId]) -> Vec<u32> {
    if cycle_trait_ids.is_empty() {
        return Vec::new();
    }

    let raw = cycle_trait_ids
        .iter()
        .map(|trait_id| trait_id.0)
        .collect::<Vec<_>>();
    let len = raw.len();
    let mut best = raw.clone();
    for start in 1..len {
        let rotated = (0..len).map(|offset| raw[(start + offset) % len]).collect();
        if rotated < best {
            best = rotated;
        }
    }
    best
}
