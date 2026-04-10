use kernc_mast::{
    MastBlock, MastExpr, MastExprKind, MastFunction, MastGlobal, MastLinkage, MastModule, MastStmt,
    MonoId,
};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodegenPlanReport {
    pub requested_units: usize,
    pub root_count: usize,
    pub cluster_count: usize,
    pub planned_units: usize,
    pub total_workload: usize,
    pub min_cluster_workload: usize,
    pub max_cluster_workload: usize,
    pub min_unit_workload: usize,
    pub max_unit_workload: usize,
    pub promoted_function_count: usize,
    pub promoted_global_count: usize,
    pub fallback_reason: Option<CodegenPlanFallback>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodegenPlanFallback {
    RequestedSingleUnit,
    NameCollision {
        item_kind: &'static str,
        name: String,
    },
    TooFewRoots,
    TooFewTargetUnits,
    TooFewMaterializedUnits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CodegenPlanOutcome {
    pub(super) units: Vec<CodegenUnitPlan>,
    pub(super) report: CodegenPlanReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ItemKey {
    Function(MonoId),
    Global(MonoId),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ItemRefs {
    functions: HashSet<MonoId>,
    globals: HashSet<MonoId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CodegenUnitPlan {
    pub(super) name: String,
    pub(super) function_ids: HashSet<MonoId>,
    pub(super) global_ids: HashSet<MonoId>,
    pub(super) promoted_function_ids: HashSet<MonoId>,
    pub(super) promoted_global_ids: HashSet<MonoId>,
    pub(super) workload: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClusterPlan {
    root_keys: Vec<ItemKey>,
    function_ids: HashSet<MonoId>,
    global_ids: HashSet<MonoId>,
    promoted_function_ids: HashSet<MonoId>,
    promoted_global_ids: HashSet<MonoId>,
    workload: usize,
}

#[cfg(test)]
pub(super) fn plan_codegen_units(
    module: &MastModule,
    requested_units: usize,
) -> Vec<CodegenUnitPlan> {
    plan_codegen_units_with_report(module, requested_units).units
}

pub(super) fn plan_codegen_units_with_report(
    module: &MastModule,
    requested_units: usize,
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

    let mut refs = HashMap::new();
    let mut workloads = HashMap::new();

    for function in &module.functions {
        if function.body.is_some() {
            let key = ItemKey::Function(function.id);
            refs.insert(key, refs_for_function(function));
            workloads.insert(key, workload_for_function(function));
        }
    }

    for global in &module.globals {
        if global.init.is_some() {
            let key = ItemKey::Global(global.id);
            refs.insert(key, refs_for_global(global));
            workloads.insert(key, workload_for_global(global));
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
        if owner_roots.len() > 1 {
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
    report.min_cluster_workload = clusters.iter().map(|cluster| cluster.workload).min().unwrap_or(0);
    report.max_cluster_workload = clusters.iter().map(|cluster| cluster.workload).max().unwrap_or(0);
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
            function_ids: HashSet::new(),
            global_ids: HashSet::new(),
            promoted_function_ids: HashSet::new(),
            promoted_global_ids: HashSet::new(),
            workload: 0,
        })
        .collect::<Vec<_>>();

    for cluster in clusters {
        let (unit_idx, _) = units
            .iter()
            .enumerate()
            .min_by(|(_, lhs), (_, rhs)| {
                lhs.workload
                    .cmp(&rhs.workload)
                    .then_with(|| lhs.name.cmp(&rhs.name))
            })
            .expect("at least one codegen unit must exist");
        let unit = &mut units[unit_idx];
        unit.function_ids.extend(cluster.function_ids);
        unit.global_ids.extend(cluster.global_ids);
        unit.promoted_function_ids
            .extend(cluster.promoted_function_ids);
        unit.promoted_global_ids.extend(cluster.promoted_global_ids);
        unit.workload += cluster.workload;
    }

    units.retain(|unit| !unit.function_ids.is_empty() || !unit.global_ids.is_empty());
    report.planned_units = units.len();
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodegenNameCollision {
    item_kind: &'static str,
    name: String,
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

pub(super) fn materialize_codegen_unit(module: &MastModule, unit: &CodegenUnitPlan) -> MastModule {
    let refs = build_item_refs(module);
    let (decl_function_ids, decl_global_ids) = collect_needed_declarations(module, unit, &refs);

    let functions = module
        .functions
        .iter()
        .filter_map(|function| {
            let owned = unit.function_ids.contains(&function.id);
            let included =
                owned || decl_function_ids.contains(&function.id) || function.body.is_none();
            included.then(|| {
                materialize_function(
                    function,
                    owned,
                    unit.promoted_function_ids.contains(&function.id),
                )
            })
        })
        .collect();
    let globals = module
        .globals
        .iter()
        .filter_map(|global| {
            let owned = unit.global_ids.contains(&global.id);
            let included = owned || decl_global_ids.contains(&global.id) || global.init.is_none();
            included.then(|| {
                materialize_global(global, owned, unit.promoted_global_ids.contains(&global.id))
            })
        })
        .collect();

    MastModule {
        name: format!("{}_{}", module.name, unit.name),
        structs: module.structs.clone(),
        globals,
        functions,
        def_mono_map: module.def_mono_map.clone(),
        pure_enum_tag_map: module.pure_enum_tag_map.clone(),
        adt_union_map: module.adt_union_map.clone(),
        anon_struct_map: module.anon_struct_map.clone(),
        anon_union_map: module.anon_union_map.clone(),
        anon_enum_map: module.anon_enum_map.clone(),
    }
}

fn build_item_refs(module: &MastModule) -> HashMap<ItemKey, ItemRefs> {
    let mut refs = HashMap::new();

    for function in &module.functions {
        if function.body.is_some() {
            refs.insert(ItemKey::Function(function.id), refs_for_function(function));
        }
    }

    for global in &module.globals {
        if global.init.is_some() {
            refs.insert(ItemKey::Global(global.id), refs_for_global(global));
        }
    }

    refs
}

fn collect_needed_declarations(
    module: &MastModule,
    unit: &CodegenUnitPlan,
    refs: &HashMap<ItemKey, ItemRefs>,
) -> (HashSet<MonoId>, HashSet<MonoId>) {
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

    let mut decl_function_ids = HashSet::new();
    let mut decl_global_ids = HashSet::new();
    let mut visited = HashSet::new();
    let mut stack = Vec::new();

    stack.extend(unit.function_ids.iter().copied().map(ItemKey::Function));
    stack.extend(unit.global_ids.iter().copied().map(ItemKey::Global));

    while let Some(item) = stack.pop() {
        if !visited.insert(item) {
            continue;
        }
        let Some(item_refs) = refs.get(&item) else {
            continue;
        };

        for function_id in &item_refs.functions {
            let target = ItemKey::Function(*function_id);
            if unit.function_ids.contains(function_id) {
                stack.push(target);
            } else if is_materializable_declaration(target, &functions_by_id, &globals_by_id) {
                decl_function_ids.insert(*function_id);
            }
        }

        for global_id in &item_refs.globals {
            let target = ItemKey::Global(*global_id);
            if unit.global_ids.contains(global_id) {
                stack.push(target);
            } else if is_materializable_declaration(target, &functions_by_id, &globals_by_id) {
                decl_global_ids.insert(*global_id);
            }
        }
    }

    (decl_function_ids, decl_global_ids)
}

fn is_materializable_declaration(
    key: ItemKey,
    functions_by_id: &HashMap<MonoId, &MastFunction>,
    globals_by_id: &HashMap<MonoId, &MastGlobal>,
) -> bool {
    match key {
        ItemKey::Function(id) => functions_by_id
            .get(&id)
            .is_some_and(|function| function.body.is_some() || function.is_extern),
        ItemKey::Global(id) => globals_by_id
            .get(&id)
            .is_some_and(|global| global.init.is_some() || global.is_extern),
    }
}

fn materialize_function(function: &MastFunction, owned: bool, promoted: bool) -> MastFunction {
    let mut function = function.clone();
    if owned {
        if promoted {
            function.linkage = MastLinkage::External;
        }
        return function;
    }

    if function.body.is_none() {
        return function;
    }

    function.body = None;
    function.is_extern = true;
    function.linkage = MastLinkage::External;
    function
}

fn materialize_global(global: &MastGlobal, owned: bool, promoted: bool) -> MastGlobal {
    let mut global = global.clone();
    if owned {
        if promoted {
            global.linkage = MastLinkage::External;
        }
        return global;
    }

    if global.init.is_none() {
        return global;
    }

    global.init = None;
    global.is_extern = true;
    global.linkage = MastLinkage::External;
    global
}

fn is_partition_root(
    key: ItemKey,
    functions_by_id: &HashMap<MonoId, &MastFunction>,
    globals_by_id: &HashMap<MonoId, &MastGlobal>,
) -> bool {
    match key {
        ItemKey::Function(id) => functions_by_id
            .get(&id)
            .is_some_and(|function| function.linkage != MastLinkage::Internal),
        ItemKey::Global(id) => globals_by_id
            .get(&id)
            .is_some_and(|global| global.linkage != MastLinkage::Internal),
    }
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
            if is_internal_item(target, functions_by_id, globals_by_id) && reachable.insert(target)
            {
                stack.push(target);
            }
        }
        for global_id in &item_refs.globals {
            let target = ItemKey::Global(*global_id);
            if is_internal_item(target, functions_by_id, globals_by_id) && reachable.insert(target)
            {
                stack.push(target);
            }
        }
    }

    reachable
}

fn is_internal_item(
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

fn refs_for_function(function: &MastFunction) -> ItemRefs {
    let mut refs = ItemRefs::default();
    if let Some(body) = &function.body {
        collect_block_refs(body, &mut refs);
    }
    refs
}

fn refs_for_global(global: &MastGlobal) -> ItemRefs {
    let mut refs = ItemRefs::default();
    if let Some(init) = &global.init {
        collect_expr_refs(init, &mut refs);
    }
    refs
}

fn collect_block_refs(block: &MastBlock, refs: &mut ItemRefs) {
    for stmt in &block.stmts {
        match stmt {
            MastStmt::Let { init, .. } => collect_expr_refs(init, refs),
            MastStmt::Expr(expr) => collect_expr_refs(expr, refs),
        }
    }
    if let Some(result) = &block.result {
        collect_expr_refs(result, refs);
    }
    for defer in &block.defers {
        collect_expr_refs(defer, refs);
    }
}

fn collect_expr_refs(expr: &MastExpr, refs: &mut ItemRefs) {
    match &expr.kind {
        MastExprKind::GlobalRef(id) => {
            refs.globals.insert(*id);
        }
        MastExprKind::FuncRef(id) => {
            refs.functions.insert(*id);
        }
        MastExprKind::AddressOf(inner)
        | MastExprKind::Deref(inner)
        | MastExprKind::ExtractFatPtrData(inner)
        | MastExprKind::ExtractFatPtrMeta(inner)
        | MastExprKind::BitIntrinsic { operand: inner, .. }
        | MastExprKind::Cast { operand: inner, .. }
        | MastExprKind::Unary { operand: inner, .. } => collect_expr_refs(inner, refs),
        MastExprKind::StructInit { fields, .. } | MastExprKind::ArrayInit(fields) => {
            for field in fields {
                collect_expr_refs(field, refs);
            }
        }
        MastExprKind::UnionInit { value, .. } | MastExprKind::DataInit { payload: value, .. } => {
            collect_expr_refs(value, refs)
        }
        MastExprKind::FieldAccess { lhs, .. } => collect_expr_refs(lhs, refs),
        MastExprKind::IndexAccess { lhs, index }
        | MastExprKind::Binary {
            lhs, rhs: index, ..
        }
        | MastExprKind::Assign {
            lhs, rhs: index, ..
        } => {
            collect_expr_refs(lhs, refs);
            collect_expr_refs(index, refs);
        }
        MastExprKind::Call { callee, args } => {
            collect_expr_refs(callee, refs);
            for arg in args {
                collect_expr_refs(arg, refs);
            }
        }
        MastExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_refs(cond, refs);
            collect_block_refs(then_branch, refs);
            if let Some(else_branch) = else_branch {
                collect_block_refs(else_branch, refs);
            }
        }
        MastExprKind::Loop { body, latch } => {
            collect_block_refs(body, refs);
            if let Some(latch) = latch {
                collect_block_refs(latch, refs);
            }
        }
        MastExprKind::Switch {
            target,
            cases,
            default_case,
        } => {
            collect_expr_refs(target, refs);
            for case in cases {
                collect_block_refs(&case.body, refs);
            }
            if let Some(default_case) = default_case {
                collect_block_refs(default_case, refs);
            }
        }
        MastExprKind::Return(value) => {
            if let Some(value) = value {
                collect_expr_refs(value, refs);
            }
        }
        MastExprKind::AtomicLoad { ptr, .. } => collect_expr_refs(ptr, refs),
        MastExprKind::AtomicStore { ptr, value, .. }
        | MastExprKind::AtomicRmw { ptr, value, .. } => {
            collect_expr_refs(ptr, refs);
            collect_expr_refs(value, refs);
        }
        MastExprKind::AtomicCas {
            ptr,
            expected,
            desired,
            ..
        } => {
            collect_expr_refs(ptr, refs);
            collect_expr_refs(expected, refs);
            collect_expr_refs(desired, refs);
        }
        MastExprKind::Memcpy { dest, src, len } | MastExprKind::Memmove { dest, src, len } => {
            collect_expr_refs(dest, refs);
            collect_expr_refs(src, refs);
            collect_expr_refs(len, refs);
        }
        MastExprKind::Memset { dest, val, len } => {
            collect_expr_refs(dest, refs);
            collect_expr_refs(val, refs);
            collect_expr_refs(len, refs);
        }
        MastExprKind::ConstructFatPointer { data_ptr, meta } => {
            collect_expr_refs(data_ptr, refs);
            collect_expr_refs(meta, refs);
        }
        MastExprKind::Block(block) => collect_block_refs(block, refs),
        MastExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_expr_refs(lhs, refs);
            if let Some(start) = start {
                collect_expr_refs(start, refs);
            }
            if let Some(end) = end {
                collect_expr_refs(end, refs);
            }
        }
        MastExprKind::Asm(asm) => {
            for arg in &asm.input_args {
                collect_expr_refs(arg, refs);
            }
        }
        MastExprKind::Fence { .. }
        | MastExprKind::Undef
        | MastExprKind::Unreachable
        | MastExprKind::Trap
        | MastExprKind::Breakpoint
        | MastExprKind::Break
        | MastExprKind::Continue
        | MastExprKind::Integer(_)
        | MastExprKind::Float(_)
        | MastExprKind::Bool(_)
        | MastExprKind::StringLiteral(_)
        | MastExprKind::Var(_) => {}
    }
}

fn workload_for_function(function: &MastFunction) -> usize {
    function
        .body
        .as_ref()
        .map(block_workload)
        .unwrap_or(1)
        .max(1)
}

fn workload_for_global(global: &MastGlobal) -> usize {
    global.init.as_ref().map(expr_workload).unwrap_or(1).max(1)
}

fn block_workload(block: &MastBlock) -> usize {
    let mut weight = 1 + block.stmts.len() + block.defers.len();
    for stmt in &block.stmts {
        weight += match stmt {
            MastStmt::Let { init, .. } => expr_workload(init),
            MastStmt::Expr(expr) => expr_workload(expr),
        };
    }
    if let Some(result) = &block.result {
        weight += expr_workload(result);
    }
    for defer in &block.defers {
        weight += expr_workload(defer);
    }
    weight
}

fn expr_workload(expr: &MastExpr) -> usize {
    let mut weight = 1;
    match &expr.kind {
        MastExprKind::AddressOf(inner)
        | MastExprKind::Deref(inner)
        | MastExprKind::ExtractFatPtrData(inner)
        | MastExprKind::ExtractFatPtrMeta(inner)
        | MastExprKind::BitIntrinsic { operand: inner, .. }
        | MastExprKind::Cast { operand: inner, .. }
        | MastExprKind::Unary { operand: inner, .. } => weight += expr_workload(inner),
        MastExprKind::StructInit { fields, .. } | MastExprKind::ArrayInit(fields) => {
            for field in fields {
                weight += expr_workload(field);
            }
        }
        MastExprKind::UnionInit { value, .. } | MastExprKind::DataInit { payload: value, .. } => {
            weight += expr_workload(value);
        }
        MastExprKind::FieldAccess { lhs, .. } => weight += expr_workload(lhs),
        MastExprKind::IndexAccess { lhs, index }
        | MastExprKind::Binary {
            lhs, rhs: index, ..
        }
        | MastExprKind::Assign {
            lhs, rhs: index, ..
        } => {
            weight += expr_workload(lhs);
            weight += expr_workload(index);
        }
        MastExprKind::Call { callee, args } => {
            weight += 2 + expr_workload(callee);
            for arg in args {
                weight += expr_workload(arg);
            }
        }
        MastExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            weight += 3 + expr_workload(cond) + block_workload(then_branch);
            if let Some(else_branch) = else_branch {
                weight += block_workload(else_branch);
            }
        }
        MastExprKind::Loop { body, latch } => {
            weight += 3 + block_workload(body);
            if let Some(latch) = latch {
                weight += block_workload(latch);
            }
        }
        MastExprKind::Switch {
            target,
            cases,
            default_case,
        } => {
            weight += 4 + expr_workload(target);
            for case in cases {
                weight += block_workload(&case.body);
            }
            if let Some(default_case) = default_case {
                weight += block_workload(default_case);
            }
        }
        MastExprKind::Return(value) => {
            if let Some(value) = value {
                weight += expr_workload(value);
            }
        }
        MastExprKind::AtomicLoad { ptr, .. } => weight += expr_workload(ptr),
        MastExprKind::AtomicStore { ptr, value, .. }
        | MastExprKind::AtomicRmw { ptr, value, .. } => {
            weight += expr_workload(ptr) + expr_workload(value);
        }
        MastExprKind::AtomicCas {
            ptr,
            expected,
            desired,
            ..
        } => {
            weight += expr_workload(ptr) + expr_workload(expected) + expr_workload(desired);
        }
        MastExprKind::Memcpy { dest, src, len } | MastExprKind::Memmove { dest, src, len } => {
            weight += expr_workload(dest) + expr_workload(src) + expr_workload(len);
        }
        MastExprKind::Memset { dest, val, len } => {
            weight += expr_workload(dest) + expr_workload(val) + expr_workload(len);
        }
        MastExprKind::ConstructFatPointer { data_ptr, meta } => {
            weight += expr_workload(data_ptr) + expr_workload(meta);
        }
        MastExprKind::Block(block) => weight += block_workload(block),
        MastExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            weight += expr_workload(lhs);
            if let Some(start) = start {
                weight += expr_workload(start);
            }
            if let Some(end) = end {
                weight += expr_workload(end);
            }
        }
        MastExprKind::Asm(asm) => {
            weight += 2;
            for arg in &asm.input_args {
                weight += expr_workload(arg);
            }
        }
        MastExprKind::Fence { .. }
        | MastExprKind::Undef
        | MastExprKind::Unreachable
        | MastExprKind::Trap
        | MastExprKind::Breakpoint
        | MastExprKind::Break
        | MastExprKind::Continue
        | MastExprKind::Integer(_)
        | MastExprKind::Float(_)
        | MastExprKind::Bool(_)
        | MastExprKind::StringLiteral(_)
        | MastExprKind::Var(_)
        | MastExprKind::GlobalRef(_)
        | MastExprKind::FuncRef(_) => {}
    }
    weight
}

#[cfg(test)]
mod tests {
    use super::{materialize_codegen_unit, plan_codegen_units, plan_codegen_units_with_report};
    use kernc_mast::{
        MastBlock, MastExpr, MastExprKind, MastFunction, MastGlobal, MastLinkage, MastModule,
        MastParam, MastStmt, MonoId,
    };
    use kernc_sema::ty::TypeId;
    use std::collections::HashMap;

    fn void_expr(kind: MastExprKind) -> MastExpr {
        MastExpr::new(TypeId::VOID, kind, kernc_utils::Span::default())
    }

    fn call(id: u32) -> MastExpr {
        void_expr(MastExprKind::Call {
            callee: Box::new(void_expr(MastExprKind::FuncRef(MonoId(id)))),
            args: Vec::new(),
        })
    }

    fn function(id: u32, name: &str, linkage: MastLinkage, body: Vec<MastExpr>) -> MastFunction {
        MastFunction {
            id: MonoId(id),
            name: name.to_string(),
            linkage,
            params: Vec::<MastParam>::new(),
            ret_ty: TypeId::VOID,
            body: Some(MastBlock {
                stmts: body.into_iter().map(MastStmt::Expr).collect(),
                result: None,
                defers: Vec::new(),
            }),
            is_extern: false,
            is_variadic: false,
            attributes: Vec::new(),
        }
    }

    fn internal_global(id: u32, name: &str) -> MastGlobal {
        MastGlobal {
            id: MonoId(id),
            name: name.to_string(),
            linkage: MastLinkage::Internal,
            ty: TypeId::U32,
            is_mut: false,
            init: Some(MastExpr::new(
                TypeId::U32,
                MastExprKind::Integer(1),
                kernc_utils::Span::default(),
            )),
            is_extern: false,
            attributes: Vec::new(),
        }
    }

    #[test]
    fn shared_internal_functions_can_split_by_promoting_a_single_owner() {
        let shared = function(10, "shared", MastLinkage::Internal, Vec::new());
        let root_a = function(1, "a", MastLinkage::External, vec![call(10)]);
        let root_b = function(2, "b", MastLinkage::External, vec![call(10)]);
        let module = MastModule {
            name: "demo".to_string(),
            structs: Vec::new(),
            globals: Vec::new(),
            functions: vec![root_a, root_b, shared],
            def_mono_map: HashMap::new(),
            pure_enum_tag_map: HashMap::new(),
            adt_union_map: HashMap::new(),
            anon_struct_map: HashMap::new(),
            anon_union_map: HashMap::new(),
            anon_enum_map: HashMap::new(),
        };

        let units = plan_codegen_units(&module, 2);
        assert_eq!(units.len(), 2);

        let owner = units
            .iter()
            .find(|unit| unit.function_ids.contains(&MonoId(10)))
            .expect("shared helper must be owned by one unit");
        assert!(owner.promoted_function_ids.contains(&MonoId(10)));

        let owner_materialized = materialize_codegen_unit(&module, owner);
        let owned_shared = owner_materialized
            .functions
            .iter()
            .find(|function| function.id == MonoId(10))
            .expect("owner must keep the shared helper body");
        assert!(owned_shared.body.is_some());
        assert_eq!(owned_shared.linkage, MastLinkage::External);

        let importer = units
            .iter()
            .find(|unit| !unit.function_ids.contains(&MonoId(10)))
            .expect("other unit must import the shared helper");
        let importer_materialized = materialize_codegen_unit(&module, importer);
        let imported_shared = importer_materialized
            .functions
            .iter()
            .find(|function| function.id == MonoId(10))
            .expect("importer must see a declaration for the shared helper");
        assert!(imported_shared.body.is_none());
        assert!(imported_shared.is_extern);
        assert_eq!(imported_shared.linkage, MastLinkage::External);
    }

    #[test]
    fn shared_internal_globals_can_split_by_promoting_a_single_owner() {
        let root_a = function(
            1,
            "a",
            MastLinkage::External,
            vec![void_expr(MastExprKind::GlobalRef(MonoId(20)))],
        );
        let root_b = function(
            2,
            "b",
            MastLinkage::External,
            vec![void_expr(MastExprKind::GlobalRef(MonoId(20)))],
        );
        let shared = internal_global(20, "shared");
        let module = MastModule {
            name: "demo".to_string(),
            structs: Vec::new(),
            globals: vec![shared],
            functions: vec![root_a, root_b],
            def_mono_map: HashMap::new(),
            pure_enum_tag_map: HashMap::new(),
            adt_union_map: HashMap::new(),
            anon_struct_map: HashMap::new(),
            anon_union_map: HashMap::new(),
            anon_enum_map: HashMap::new(),
        };

        let units = plan_codegen_units(&module, 2);
        assert_eq!(units.len(), 2);

        let owner = units
            .iter()
            .find(|unit| unit.global_ids.contains(&MonoId(20)))
            .expect("shared global must be owned by one unit");
        assert!(owner.promoted_global_ids.contains(&MonoId(20)));

        let owner_materialized = materialize_codegen_unit(&module, owner);
        let owned_shared = owner_materialized
            .globals
            .iter()
            .find(|global| global.id == MonoId(20))
            .expect("owner must keep the shared global initializer");
        assert!(owned_shared.init.is_some());
        assert_eq!(owned_shared.linkage, MastLinkage::External);

        let importer = units
            .iter()
            .find(|unit| !unit.global_ids.contains(&MonoId(20)))
            .expect("other unit must import the shared global");
        let importer_materialized = materialize_codegen_unit(&module, importer);
        let imported_shared = importer_materialized
            .globals
            .iter()
            .find(|global| global.id == MonoId(20))
            .expect("importer must see a declaration for the shared global");
        assert!(imported_shared.init.is_none());
        assert!(imported_shared.is_extern);
        assert_eq!(imported_shared.linkage, MastLinkage::External);
    }

    #[test]
    fn duplicate_codegen_names_disable_partitioning() {
        let root_a = function(1, "dup", MastLinkage::External, Vec::new());
        let root_b = function(2, "dup", MastLinkage::External, Vec::new());
        let module = MastModule {
            name: "demo".to_string(),
            structs: Vec::new(),
            globals: Vec::new(),
            functions: vec![root_a, root_b],
            def_mono_map: HashMap::new(),
            pure_enum_tag_map: HashMap::new(),
            adt_union_map: HashMap::new(),
            anon_struct_map: HashMap::new(),
            anon_union_map: HashMap::new(),
            anon_enum_map: HashMap::new(),
        };

        let units = plan_codegen_units(&module, 2);
        assert!(units.is_empty());
    }

    #[test]
    fn independent_roots_split_and_only_needed_non_owned_defs_become_declarations() {
        let root_a = function(
            1,
            "a",
            MastLinkage::External,
            vec![void_expr(MastExprKind::GlobalRef(MonoId(20))), call(2)],
        );
        let root_b = function(2, "b", MastLinkage::External, Vec::new());
        let helper_global = internal_global(20, "helper");
        let module = MastModule {
            name: "demo".to_string(),
            structs: Vec::new(),
            globals: vec![helper_global],
            functions: vec![root_a, root_b],
            def_mono_map: HashMap::new(),
            pure_enum_tag_map: HashMap::new(),
            adt_union_map: HashMap::new(),
            anon_struct_map: HashMap::new(),
            anon_union_map: HashMap::new(),
            anon_enum_map: HashMap::new(),
        };

        let units = plan_codegen_units(&module, 2);
        assert_eq!(units.len(), 2);
        assert!(
            units
                .iter()
                .any(|unit| unit.function_ids.contains(&MonoId(1)))
        );
        assert!(
            units
                .iter()
                .any(|unit| unit.function_ids.contains(&MonoId(2)))
        );

        let unit_for_a = units
            .iter()
            .find(|unit| unit.function_ids.contains(&MonoId(1)))
            .unwrap();
        let materialized = materialize_codegen_unit(&module, unit_for_a);
        let decl_b = materialized
            .functions
            .iter()
            .find(|function| function.id == MonoId(2))
            .unwrap();
        assert!(decl_b.body.is_none());
        assert!(decl_b.is_extern);
        let helper = materialized
            .globals
            .iter()
            .find(|global| global.id == MonoId(20))
            .unwrap();
        assert!(helper.init.is_some());
    }

    #[test]
    fn report_tracks_cluster_workload_summary() {
        let root_a = function(1, "a", MastLinkage::External, vec![call(2), call(2)]);
        let root_b = function(2, "b", MastLinkage::External, Vec::new());
        let module = MastModule {
            name: "demo".to_string(),
            structs: Vec::new(),
            globals: Vec::new(),
            functions: vec![root_a, root_b],
            def_mono_map: HashMap::new(),
            pure_enum_tag_map: HashMap::new(),
            adt_union_map: HashMap::new(),
            anon_struct_map: HashMap::new(),
            anon_union_map: HashMap::new(),
            anon_enum_map: HashMap::new(),
        };

        let outcome = plan_codegen_units_with_report(&module, 2);

        assert_eq!(outcome.report.root_count, 2);
        assert_eq!(outcome.report.cluster_count, 2);
        assert_eq!(outcome.report.planned_units, 2);
        assert_eq!(outcome.report.total_workload, 12);
        assert_eq!(outcome.report.min_cluster_workload, 1);
        assert_eq!(outcome.report.max_cluster_workload, 11);
        assert_eq!(outcome.report.min_unit_workload, 1);
        assert_eq!(outcome.report.max_unit_workload, 11);
        assert_eq!(outcome.report.promoted_function_count, 0);
        assert_eq!(outcome.report.promoted_global_count, 0);
        assert!(outcome.report.fallback_reason.is_none());
    }
}
