//! Codegen-unit planning.
//!
//! Planning assigns roots and reachable helper items to units using either MIR
//! summary data or MAST workload estimates, then records imports needed for
//! cross-unit references.

use super::refs::build_item_refs;
use super::workload::{workload_for_function, workload_for_global};
use super::*;
use kernc_mir::{MirInlineHint, MirItemBodyRole, MirSummaryIndex};
use std::cmp::Ordering;

#[cfg(test)]
pub(super) fn plan_codegen_units(
    module: &MastModule,
    requested_units: usize,
) -> Vec<CodegenUnitPlan> {
    plan_codegen_units_with_report(module, requested_units).units
}

#[cfg(test)]
pub(in crate::compiler) fn plan_codegen_units_with_report(
    module: &MastModule,
    requested_units: usize,
) -> CodegenPlanOutcome {
    plan_codegen_units_impl(module, None, requested_units, false)
}

pub(in crate::compiler) fn plan_codegen_units_with_mir_summary(
    module: &MastModule,
    summary: &MirSummaryIndex,
    requested_units: usize,
) -> CodegenPlanOutcome {
    plan_codegen_units_impl(module, Some(summary), requested_units, true)
}

pub(in crate::compiler) fn plan_codegen_units_with_mir_workload(
    module: &MastModule,
    summary: &MirSummaryIndex,
    requested_units: usize,
) -> CodegenPlanOutcome {
    plan_codegen_units_impl(module, Some(summary), requested_units, false)
}

fn plan_codegen_units_impl(
    module: &MastModule,
    summary: Option<&MirSummaryIndex>,
    requested_units: usize,
    enable_imports: bool,
) -> CodegenPlanOutcome {
    let mut report = CodegenPlanReport {
        requested_units,
        root_count: 0,
        cluster_count: 0,
        planned_units: 0,
        total_workload: 0,
        min_cluster_workload: 0,
        max_cluster_workload: 0,
        min_unit_workload: 0,
        max_unit_workload: 0,
        promoted_function_count: 0,
        promoted_global_count: 0,
        imported_function_count: 0,
        import_plan: None,
        fallback_reason: None,
    };

    if requested_units <= 1 {
        report.fallback_reason = Some(CodegenPlanFallback::RequestedSingleUnit);
        return CodegenPlanOutcome {
            units: Vec::new(),
            report,
        };
    }
    if let Some(collision) = find_codegen_name_collision(module) {
        report.fallback_reason = Some(CodegenPlanFallback::NameCollision {
            item_kind: collision.item_kind,
            name: collision.name,
        });
        return CodegenPlanOutcome {
            units: Vec::new(),
            report,
        };
    }
    let isolated_control_flow_asm_functions = if enable_imports {
        summary
            .map(isolated_control_flow_asm_function_ids)
            .unwrap_or_default()
    } else {
        HashSet::new()
    };

    let functions_by_id = module
        .functions
        .iter()
        .map(|function| (function.id, function))
        .collect::<HashMap<_, _>>();
    let globals_by_id = module
        .globals
        .iter()
        .map(|global| (global.id, global))
        .collect::<HashMap<_, _>>();
    let refs = build_item_refs(module);
    let mut workloads = HashMap::new();

    for function in &module.functions {
        if function.body.is_some() {
            let workload = summary
                .and_then(|summary| summary.function(function.id))
                .map(|summary| summary.workload())
                .unwrap_or_else(|| workload_for_function(function));
            workloads.insert(ItemKey::Function(function.id), workload);
        }
    }

    for global in &module.globals {
        if global.init.is_some() {
            let workload = summary
                .and_then(|summary| summary.global(global.id))
                .map(|summary| summary.workload())
                .unwrap_or_else(|| workload_for_global(global));
            workloads.insert(ItemKey::Global(global.id), workload);
        }
    }

    let mut roots = refs
        .keys()
        .copied()
        .filter(|key| is_partition_root(*key, &functions_by_id, &globals_by_id))
        .collect::<Vec<_>>();
    roots.sort_by_key(|key| item_key_sort_key(*key));
    report.root_count = roots.len();
    if roots.len() <= 1 {
        report.fallback_reason = Some(CodegenPlanFallback::TooFewRoots);
        return CodegenPlanOutcome {
            units: Vec::new(),
            report,
        };
    }

    let root_index = roots
        .iter()
        .enumerate()
        .map(|(idx, key)| (*key, idx))
        .collect::<HashMap<_, _>>();
    let mut clusters = roots
        .iter()
        .map(|root| ClusterPlan {
            root_keys: vec![*root],
            function_ids: HashSet::new(),
            global_ids: HashSet::new(),
            promoted_function_ids: HashSet::new(),
            promoted_global_ids: HashSet::new(),
            workload: 0,
        })
        .collect::<Vec<_>>();

    for root in &roots {
        let Some(&root_idx) = root_index.get(root) else {
            continue;
        };
        let cluster = &mut clusters[root_idx];
        insert_item(root, &mut cluster.function_ids, &mut cluster.global_ids);
    }

    let mut internal_to_roots = HashMap::<ItemKey, Vec<usize>>::new();
    for root in &roots {
        let Some(&root_idx) = root_index.get(root) else {
            continue;
        };
        let reachable = reachable_internal_items(*root, &refs, &functions_by_id, &globals_by_id);
        for item in reachable {
            internal_to_roots.entry(item).or_default().push(root_idx);
        }
    }

    let shared_partitioned_functions = internal_to_roots
        .iter()
        .filter_map(|(item, owner_roots)| match item {
            ItemKey::Function(id) if owner_roots.len() > 1 => Some((*id, owner_roots.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();

    for (item, owner_roots) in internal_to_roots {
        let Some(&owner_idx) = owner_roots.iter().min() else {
            continue;
        };
        let owner_cluster = &mut clusters[owner_idx];
        insert_item(
            &item,
            &mut owner_cluster.function_ids,
            &mut owner_cluster.global_ids,
        );
        if owner_roots.len() > 1 && item_requires_promotion(item, &functions_by_id, &globals_by_id)
        {
            mark_promoted_item(
                item,
                &mut owner_cluster.promoted_function_ids,
                &mut owner_cluster.promoted_global_ids,
            );
        }
    }

    for cluster in &mut clusters {
        cluster.root_keys.sort_by_key(|key| item_key_sort_key(*key));
        cluster.workload = cluster
            .function_ids
            .iter()
            .map(|id| workloads.get(&ItemKey::Function(*id)).copied().unwrap_or(1))
            .sum::<usize>()
            + cluster
                .global_ids
                .iter()
                .map(|id| workloads.get(&ItemKey::Global(*id)).copied().unwrap_or(1))
                .sum::<usize>();
    }
    report.cluster_count = clusters.len();
    report.total_workload = clusters.iter().map(|cluster| cluster.workload).sum();
    report.min_cluster_workload = clusters
        .iter()
        .map(|cluster| cluster.workload)
        .min()
        .unwrap_or(0);
    report.max_cluster_workload = clusters
        .iter()
        .map(|cluster| cluster.workload)
        .max()
        .unwrap_or(0);
    report.promoted_function_count = clusters
        .iter()
        .map(|cluster| cluster.promoted_function_ids.len())
        .sum();
    report.promoted_global_count = clusters
        .iter()
        .map(|cluster| cluster.promoted_global_ids.len())
        .sum();

    clusters.sort_by(|lhs, rhs| {
        rhs.workload
            .cmp(&lhs.workload)
            .then_with(|| compare_item_key_slices(&lhs.root_keys, &rhs.root_keys))
    });

    let target_units = requested_units.min(clusters.len());
    if target_units <= 1 {
        report.fallback_reason = Some(CodegenPlanFallback::TooFewTargetUnits);
        return CodegenPlanOutcome {
            units: Vec::new(),
            report,
        };
    }

    let mut units = (0..target_units)
        .map(|idx| CodegenUnitPlan {
            name: format!("cgu{idx}"),
            root_keys: Vec::new(),
            function_ids: HashSet::new(),
            global_ids: HashSet::new(),
            imported_function_ids: HashSet::new(),
            promoted_function_ids: HashSet::new(),
            promoted_global_ids: HashSet::new(),
            workload: 0,
        })
        .collect::<Vec<_>>();

    for cluster in clusters {
        let Some((unit_idx, _)) = units.iter().enumerate().min_by(|(_, lhs), (_, rhs)| {
            lhs.workload
                .cmp(&rhs.workload)
                .then_with(|| lhs.name.cmp(&rhs.name))
        }) else {
            // This should be unreachable after the target-unit guard above, but
            // preserving a normal fallback keeps planning robust if the unit
            // allocation rules change independently later.
            report.fallback_reason = Some(CodegenPlanFallback::TooFewTargetUnits);
            return CodegenPlanOutcome {
                units: Vec::new(),
                report,
            };
        };
        let unit = &mut units[unit_idx];
        unit.root_keys.extend(cluster.root_keys);
        unit.function_ids.extend(cluster.function_ids);
        unit.global_ids.extend(cluster.global_ids);
        unit.promoted_function_ids
            .extend(cluster.promoted_function_ids);
        unit.promoted_global_ids.extend(cluster.promoted_global_ids);
        unit.workload += cluster.workload;
    }

    units.retain(|unit| !unit.function_ids.is_empty() || !unit.global_ids.is_empty());
    for unit in &mut units {
        unit.root_keys.sort_by_key(|key| item_key_sort_key(*key));
    }
    if enable_imports && let Some(summary) = summary {
        report.import_plan = Some(assign_imported_inline_functions(
            &mut units,
            &roots,
            &shared_partitioned_functions,
            summary,
            &functions_by_id,
            &workloads,
            &isolated_control_flow_asm_functions,
        ));
    }
    report.planned_units = units.len();
    report.imported_function_count = units
        .iter()
        .map(|unit| unit.imported_function_ids.len())
        .sum();
    report.min_unit_workload = units.iter().map(|unit| unit.workload).min().unwrap_or(0);
    report.max_unit_workload = units.iter().map(|unit| unit.workload).max().unwrap_or(0);
    if units.len() <= 1 {
        report.fallback_reason = Some(CodegenPlanFallback::TooFewMaterializedUnits);
        CodegenPlanOutcome {
            units: Vec::new(),
            report,
        }
    } else {
        CodegenPlanOutcome { units, report }
    }
}

fn assign_imported_inline_functions(
    units: &mut [CodegenUnitPlan],
    roots: &[ItemKey],
    shared_internal_functions: &[(MonoId, Vec<usize>)],
    summary: &MirSummaryIndex,
    functions_by_id: &HashMap<MonoId, &MastFunction>,
    workloads: &HashMap<ItemKey, usize>,
    isolated_control_flow_asm_functions: &HashSet<MonoId>,
) -> CodegenImportPlanReport {
    let root_to_unit = units
        .iter()
        .enumerate()
        .flat_map(|(unit_idx, unit)| {
            unit.root_keys
                .iter()
                .copied()
                .map(move |root| (root, unit_idx))
        })
        .collect::<HashMap<_, _>>();
    let shared_function_layout = shared_internal_functions
        .iter()
        .filter_map(|(function_id, owner_roots)| {
            let &owner_root_idx = owner_roots.iter().min()?;
            let owner_root = roots.get(owner_root_idx).copied()?;
            let &owner_unit_idx = root_to_unit.get(&owner_root)?;
            let importer_unit_indices = owner_roots
                .iter()
                .filter_map(|root_idx| roots.get(*root_idx).copied())
                .filter_map(|root| root_to_unit.get(&root).copied())
                .filter(|unit_idx| *unit_idx != owner_unit_idx)
                .collect::<HashSet<_>>();
            Some((
                *function_id,
                SharedFunctionLayout {
                    owner_unit_idx,
                    importer_unit_indices,
                },
            ))
        })
        .collect::<HashMap<_, _>>();
    let mut remaining_budgets = units.iter().map(import_budget_for_unit).collect::<Vec<_>>();
    let mut import_report = CodegenImportPlanReport {
        total_budget: remaining_budgets.iter().sum(),
        min_unit_budget: remaining_budgets.iter().copied().min().unwrap_or(0),
        max_unit_budget: remaining_budgets.iter().copied().max().unwrap_or(0),
        ..CodegenImportPlanReport::default()
    };
    let units_snapshot = units.to_vec();
    let import_context = ImportClosureContext {
        units: &units_snapshot,
        summary,
        functions_by_id,
        workloads,
        shared_function_layout: &shared_function_layout,
        isolated_control_flow_asm_functions,
    };

    for unit_idx in 0..units.len() {
        let unit = &units[unit_idx];
        let mut candidates = shared_function_layout
            .iter()
            .filter(|(_, layout)| layout.importer_unit_indices.contains(&unit_idx))
            .filter_map(|(function_id, _layout)| {
                let local_callsite_count =
                    local_direct_callsite_count(summary, &unit.function_ids, *function_id);
                if local_callsite_count == 0 {
                    return None;
                }
                let mut closure = HashSet::new();
                let mut visited = HashSet::new();
                let import_workload = collect_import_closure(
                    *function_id,
                    unit_idx,
                    &import_context,
                    &mut closure,
                    &mut visited,
                )?;
                let score = import_score(local_callsite_count, import_workload);
                Some(ImportCandidate {
                    id: *function_id,
                    local_callsite_count,
                    import_workload,
                    score,
                    closure,
                })
            })
            .collect::<Vec<_>>();
        import_report.candidate_function_count += candidates.len();
        import_report.total_candidate_score += candidates
            .iter()
            .map(|candidate| candidate.score)
            .sum::<usize>();
        candidates.sort_by(|lhs, rhs| {
            rhs.score
                .cmp(&lhs.score)
                .then_with(|| rhs.local_callsite_count.cmp(&lhs.local_callsite_count))
                .then_with(|| lhs.import_workload.cmp(&rhs.import_workload))
                .then_with(|| lhs.id.cmp(&rhs.id))
        });

        for candidate in candidates {
            if units[unit_idx].function_ids.contains(&candidate.id)
                || units[unit_idx]
                    .imported_function_ids
                    .contains(&candidate.id)
            {
                continue;
            }
            if candidate.import_workload > remaining_budgets[unit_idx] {
                import_report.rejected_for_budget_count += 1;
                continue;
            }
            import_report.accepted_candidate_count += 1;
            import_report.imported_score += candidate.score;
            import_report.imported_workload += candidate.import_workload;
            for function_id in candidate.closure {
                if units[unit_idx].imported_function_ids.insert(function_id) {
                    units[unit_idx].workload += workloads
                        .get(&ItemKey::Function(function_id))
                        .copied()
                        .unwrap_or(1);
                }
            }
            remaining_budgets[unit_idx] -= candidate.import_workload;
        }
    }
    import_report
}

fn local_direct_callsite_count(
    summary: &MirSummaryIndex,
    unit_function_ids: &HashSet<MonoId>,
    callee_id: MonoId,
) -> usize {
    unit_function_ids
        .iter()
        .filter_map(|function_id| summary.function(*function_id))
        .map(|function_summary| function_summary.refs.direct_callsite_count(callee_id))
        .sum()
}

fn import_score(local_callsite_count: usize, import_workload: usize) -> usize {
    local_callsite_count
        .saturating_mul(100)
        .div_ceil(import_workload.max(1))
}

fn should_import_function(function: &MastFunction, summary: &MirSummaryIndex) -> bool {
    let Some(function_summary) = summary.function(function.id) else {
        return false;
    };
    if !matches!(function_summary.inline_hint, MirInlineHint::Inline) {
        return false;
    }
    if function_summary.body_role != MirItemBodyRole::InternalBody {
        return false;
    }
    if function_summary.workload() > 8 {
        return false;
    }
    if function_summary.indirect_call_count != 0 {
        return false;
    }
    if !function_summary.refs.global_ids.is_empty() {
        return false;
    }
    true
}

fn import_budget_for_unit(unit: &CodegenUnitPlan) -> usize {
    unit.workload.max(1).div_ceil(2).clamp(4, 32)
}

struct ImportClosureContext<'a> {
    units: &'a [CodegenUnitPlan],
    summary: &'a MirSummaryIndex,
    functions_by_id: &'a HashMap<MonoId, &'a MastFunction>,
    workloads: &'a HashMap<ItemKey, usize>,
    shared_function_layout: &'a HashMap<MonoId, SharedFunctionLayout>,
    isolated_control_flow_asm_functions: &'a HashSet<MonoId>,
}

fn collect_import_closure(
    function_id: MonoId,
    importer_unit_idx: usize,
    context: &ImportClosureContext<'_>,
    closure: &mut HashSet<MonoId>,
    visited: &mut HashSet<MonoId>,
) -> Option<usize> {
    if !visited.insert(function_id) {
        return Some(0);
    }
    if context
        .isolated_control_flow_asm_functions
        .contains(&function_id)
    {
        return None;
    }
    let unit = &context.units[importer_unit_idx];
    if unit.function_ids.contains(&function_id) || unit.imported_function_ids.contains(&function_id)
    {
        return Some(0);
    }

    let function = context.functions_by_id.get(&function_id).copied()?;
    if !should_import_function(function, context.summary) {
        return None;
    }
    let layout = context.shared_function_layout.get(&function_id)?;
    if layout.owner_unit_idx == importer_unit_idx {
        return Some(0);
    }
    if !layout.importer_unit_indices.contains(&importer_unit_idx) {
        return None;
    }

    let function_summary = context.summary.function(function_id)?;
    let mut import_workload = 0;
    for callee_id in &function_summary.refs.direct_callee_ids {
        if *callee_id == function_id {
            continue;
        }
        if let Some(callee_summary) = context.summary.function(*callee_id)
            && callee_summary.body_role == MirItemBodyRole::InternalBody
        {
            import_workload +=
                collect_import_closure(*callee_id, importer_unit_idx, context, closure, visited)?;
        }
    }

    if closure.insert(function_id) {
        import_workload += context
            .workloads
            .get(&ItemKey::Function(function_id))
            .copied()
            .unwrap_or(1);
    }
    Some(import_workload)
}

#[derive(Debug, Clone)]
struct SharedFunctionLayout {
    owner_unit_idx: usize,
    importer_unit_indices: HashSet<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImportCandidate {
    id: MonoId,
    local_callsite_count: usize,
    import_workload: usize,
    score: usize,
    closure: HashSet<MonoId>,
}

fn find_codegen_name_collision(module: &MastModule) -> Option<CodegenNameCollision> {
    let mut struct_names = HashSet::new();
    for item in &module.structs {
        if !struct_names.insert(item.name.as_str()) {
            return Some(CodegenNameCollision {
                item_kind: "struct",
                name: item.name.clone(),
            });
        }
    }

    let mut function_names = HashSet::new();
    for item in &module.functions {
        if !function_names.insert(item.name.as_str()) {
            return Some(CodegenNameCollision {
                item_kind: "function",
                name: item.name.clone(),
            });
        }
    }

    let mut global_names = HashSet::new();
    for item in &module.globals {
        if !global_names.insert(item.name.as_str()) {
            return Some(CodegenNameCollision {
                item_kind: "global",
                name: item.name.clone(),
            });
        }
    }

    None
}

fn is_partition_root(
    key: ItemKey,
    functions_by_id: &HashMap<MonoId, &MastFunction>,
    globals_by_id: &HashMap<MonoId, &MastGlobal>,
) -> bool {
    match key {
        ItemKey::Function(id) => functions_by_id
            .get(&id)
            .is_some_and(|function| function.linkage == MastLinkage::External),
        ItemKey::Global(id) => globals_by_id
            .get(&id)
            .is_some_and(|global| global.linkage == MastLinkage::External),
    }
}

fn isolated_control_flow_asm_function_ids(summary: &MirSummaryIndex) -> HashSet<MonoId> {
    summary
        .functions
        .iter()
        .filter(|function| function.contains_control_flow_asm)
        .map(|function| function.id)
        .collect()
}

fn reachable_internal_items(
    root: ItemKey,
    refs: &HashMap<ItemKey, ItemRefs>,
    functions_by_id: &HashMap<MonoId, &MastFunction>,
    globals_by_id: &HashMap<MonoId, &MastGlobal>,
) -> HashSet<ItemKey> {
    let mut reachable = HashSet::new();
    let mut stack = vec![root];
    let mut visited = HashSet::new();

    while let Some(item) = stack.pop() {
        if !visited.insert(item) {
            continue;
        }
        let Some(item_refs) = refs.get(&item) else {
            continue;
        };

        for function_id in &item_refs.functions {
            let target = ItemKey::Function(*function_id);
            if is_partition_local_item(target, functions_by_id, globals_by_id)
                && reachable.insert(target)
            {
                stack.push(target);
            }
        }
        for global_id in &item_refs.globals {
            let target = ItemKey::Global(*global_id);
            if is_partition_local_item(target, functions_by_id, globals_by_id)
                && reachable.insert(target)
            {
                stack.push(target);
            }
        }
    }

    reachable
}

fn is_partition_local_item(
    key: ItemKey,
    functions_by_id: &HashMap<MonoId, &MastFunction>,
    globals_by_id: &HashMap<MonoId, &MastGlobal>,
) -> bool {
    match key {
        ItemKey::Function(id) => functions_by_id.get(&id).is_some_and(|function| {
            matches!(
                function.linkage,
                MastLinkage::Internal | MastLinkage::LinkOnceOdr
            )
        }),
        ItemKey::Global(id) => globals_by_id.get(&id).is_some_and(|global| {
            matches!(
                global.linkage,
                MastLinkage::Internal | MastLinkage::LinkOnceOdr
            )
        }),
    }
}

fn item_requires_promotion(
    key: ItemKey,
    functions_by_id: &HashMap<MonoId, &MastFunction>,
    globals_by_id: &HashMap<MonoId, &MastGlobal>,
) -> bool {
    match key {
        ItemKey::Function(id) => functions_by_id
            .get(&id)
            .is_some_and(|function| function.linkage == MastLinkage::Internal),
        ItemKey::Global(id) => globals_by_id
            .get(&id)
            .is_some_and(|global| global.linkage == MastLinkage::Internal),
    }
}

fn insert_item(
    key: &ItemKey,
    function_ids: &mut HashSet<MonoId>,
    global_ids: &mut HashSet<MonoId>,
) {
    match key {
        ItemKey::Function(id) => {
            function_ids.insert(*id);
        }
        ItemKey::Global(id) => {
            global_ids.insert(*id);
        }
    }
}

fn mark_promoted_item(
    key: ItemKey,
    promoted_function_ids: &mut HashSet<MonoId>,
    promoted_global_ids: &mut HashSet<MonoId>,
) {
    match key {
        ItemKey::Function(id) => {
            promoted_function_ids.insert(id);
        }
        ItemKey::Global(id) => {
            promoted_global_ids.insert(id);
        }
    }
}

fn item_key_sort_key(key: ItemKey) -> (u8, u32) {
    match key {
        ItemKey::Function(id) => (0, id.0),
        ItemKey::Global(id) => (1, id.0),
    }
}

fn compare_item_key_slices(lhs: &[ItemKey], rhs: &[ItemKey]) -> Ordering {
    let mut lhs = lhs.iter().copied().map(item_key_sort_key);
    let mut rhs = rhs.iter().copied().map(item_key_sort_key);
    loop {
        match (lhs.next(), rhs.next()) {
            (Some(lhs_key), Some(rhs_key)) => match lhs_key.cmp(&rhs_key) {
                Ordering::Equal => {}
                ordering => return ordering,
            },
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => return Ordering::Equal,
        }
    }
}
