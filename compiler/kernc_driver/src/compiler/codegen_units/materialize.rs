//! Codegen-unit materialization.
//!
//! Given a plan, materialization copies owned bodies into a unit and adds
//! declarations for imported functions/globals that are referenced by that unit.

use super::refs::build_item_refs;
use super::*;

pub(in crate::compiler) fn materialize_codegen_unit(
    module: &MastModule,
    unit: &CodegenUnitPlan,
) -> MastModule {
    let refs = build_item_refs(module);
    let (decl_function_ids, decl_global_ids) = collect_needed_declarations(module, unit, &refs);

    let functions = module
        .functions
        .iter()
        .filter_map(|function| {
            let owned = unit.function_ids.contains(&function.id);
            let imported = unit.imported_function_ids.contains(&function.id);
            let included = owned
                || imported
                || decl_function_ids.contains(&function.id)
                || function.body.is_none();
            included.then(|| {
                materialize_function(
                    function,
                    owned,
                    imported,
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
        mono: module.mono.clone(),
    }
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
    stack.extend(
        unit.imported_function_ids
            .iter()
            .copied()
            .map(ItemKey::Function),
    );
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

fn materialize_function(
    function: &MastFunction,
    owned: bool,
    imported: bool,
    promoted: bool,
) -> MastFunction {
    let mut function = function.clone();
    if owned {
        if promoted {
            function.linkage = MastLinkage::External;
        }
        return function;
    }

    if imported {
        function.linkage = MastLinkage::Internal;
        function.is_extern = false;
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
