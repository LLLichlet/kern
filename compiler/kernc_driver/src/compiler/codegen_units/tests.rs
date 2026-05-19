//! Codegen-unit planning tests.
//!
//! These tests build compact MAST/MIR summaries and assert ownership, imports,
//! fallback planning, and materialized per-unit modules.

use super::materialize_codegen_unit;
use super::plan::{plan_codegen_units, plan_codegen_units_with_report};
use super::plan_codegen_units_with_mir_summary;
use crate::CodegenPlanFallback;
use kernc_mast::{
    MastAsmBlock, MastBlock, MastExpr, MastExprKind, MastFunction, MastGlobal, MastInlineHint,
    MastLinkage, MastModule, MastParam, MastStmt,
};
use kernc_mono::{MonoId, MonoModuleMetadata};
use kernc_sema::ty::TypeId;

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
        span: kernc_utils::Span::default(),
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
        inline_hint: MastInlineHint::None,
        attributes: Vec::new(),
    }
}

fn inline_function(id: u32, name: &str, linkage: MastLinkage, body: Vec<MastExpr>) -> MastFunction {
    let mut function = function(id, name, linkage, body);
    function.inline_hint = MastInlineHint::Inline;
    function
}

fn internal_global(id: u32, name: &str) -> MastGlobal {
    global(id, name, MastLinkage::Internal)
}

fn link_once_global(id: u32, name: &str) -> MastGlobal {
    global(id, name, MastLinkage::LinkOnceOdr)
}

fn global(id: u32, name: &str, linkage: MastLinkage) -> MastGlobal {
    MastGlobal {
        id: MonoId(id),
        name: name.to_string(),
        span: kernc_utils::Span::default(),
        linkage,
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
        mono: MonoModuleMetadata::default(),
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
        mono: MonoModuleMetadata::default(),
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
        mono: MonoModuleMetadata::default(),
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
        mono: MonoModuleMetadata::default(),
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
        mono: MonoModuleMetadata::default(),
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
    assert_eq!(outcome.report.imported_function_count, 0);
    assert!(outcome.report.import_plan.is_none());
    assert!(outcome.report.fallback_reason.is_none());
}

#[test]
fn link_once_odr_items_do_not_become_partition_roots() {
    let root_a = function(1, "a", MastLinkage::External, vec![call(10)]);
    let root_b = function(2, "b", MastLinkage::External, Vec::new());
    let generic_helper = function(10, "generic_helper", MastLinkage::LinkOnceOdr, Vec::new());
    let module = MastModule {
        name: "demo".to_string(),
        structs: Vec::new(),
        globals: Vec::new(),
        functions: vec![root_a, root_b, generic_helper],
        mono: MonoModuleMetadata::default(),
    };

    let outcome = plan_codegen_units_with_report(&module, 3);

    assert_eq!(outcome.report.root_count, 2);
    assert_eq!(outcome.units.len(), 2);
}

#[test]
fn shared_link_once_odr_functions_are_owned_without_internal_promotion() {
    let shared = function(10, "generic_helper", MastLinkage::LinkOnceOdr, Vec::new());
    let root_a = function(1, "a", MastLinkage::External, vec![call(10)]);
    let root_b = function(2, "b", MastLinkage::External, vec![call(10)]);
    let module = MastModule {
        name: "demo".to_string(),
        structs: Vec::new(),
        globals: Vec::new(),
        functions: vec![root_a, root_b, shared],
        mono: MonoModuleMetadata::default(),
    };

    let units = plan_codegen_units(&module, 2);
    assert_eq!(units.len(), 2);

    let owner = units
        .iter()
        .find(|unit| unit.function_ids.contains(&MonoId(10)))
        .expect("shared linkonce helper must be owned by one unit");
    assert!(!owner.promoted_function_ids.contains(&MonoId(10)));

    let owner_materialized = materialize_codegen_unit(&module, owner);
    let owned_shared = owner_materialized
        .functions
        .iter()
        .find(|function| function.id == MonoId(10))
        .expect("owner must keep the shared linkonce helper body");
    assert!(owned_shared.body.is_some());
    assert_eq!(owned_shared.linkage, MastLinkage::LinkOnceOdr);

    let importer = units
        .iter()
        .find(|unit| !unit.function_ids.contains(&MonoId(10)))
        .expect("other unit must reference the shared linkonce helper");
    let importer_materialized = materialize_codegen_unit(&module, importer);
    let imported_shared = importer_materialized
        .functions
        .iter()
        .find(|function| function.id == MonoId(10))
        .expect("importer must see a declaration for the shared linkonce helper");
    assert!(imported_shared.body.is_none());
    assert!(imported_shared.is_extern);
    assert_eq!(imported_shared.linkage, MastLinkage::External);
}

#[test]
fn owned_link_once_odr_globals_materialize_as_exported_definitions() {
    let root_a = function(
        1,
        "a",
        MastLinkage::External,
        vec![void_expr(MastExprKind::GlobalRef(MonoId(10)))],
    );
    let root_b = function(
        2,
        "b",
        MastLinkage::External,
        vec![void_expr(MastExprKind::GlobalRef(MonoId(10)))],
    );
    let shared = link_once_global(10, "GENERIC");
    let module = MastModule {
        name: "demo".to_string(),
        structs: Vec::new(),
        globals: vec![shared],
        functions: vec![root_a, root_b],
        mono: MonoModuleMetadata::default(),
    };

    let units = plan_codegen_units(&module, 2);
    let owner = units
        .iter()
        .find(|unit| unit.global_ids.contains(&MonoId(10)))
        .expect("shared linkonce global must be owned by one unit");
    let owned_shared = materialize_codegen_unit(&module, owner)
        .globals
        .into_iter()
        .find(|global| global.id == MonoId(10))
        .expect("owner must keep the shared linkonce global init");
    assert!(owned_shared.init.is_some());
    assert_eq!(owned_shared.linkage, MastLinkage::LinkOnceOdr);
}

#[test]
fn mir_summary_can_drive_codegen_unit_workload_estimates() {
    let root_a = function(1, "a", MastLinkage::External, vec![call(2), call(2)]);
    let root_b = function(2, "b", MastLinkage::External, Vec::new());
    let module = MastModule {
        name: "demo".to_string(),
        structs: Vec::new(),
        globals: Vec::new(),
        functions: vec![root_a, root_b],
        mono: MonoModuleMetadata::default(),
    };

    let mir_report = kernc_mir_lower::build_from_mast(&module);
    let outcome = plan_codegen_units_with_mir_summary(&module, &mir_report.summary, 2);

    assert_eq!(outcome.report.root_count, 2);
    assert_eq!(outcome.report.cluster_count, 2);
    assert_eq!(outcome.report.planned_units, 2);
    assert_eq!(outcome.report.total_workload, 8);
    assert_eq!(outcome.report.min_cluster_workload, 2);
    assert_eq!(outcome.report.max_cluster_workload, 6);
    let import_plan = outcome
        .report
        .import_plan
        .as_ref()
        .expect("summary-driven planning should produce an import report");
    assert_eq!(import_plan.candidate_function_count, 0);
    assert_eq!(import_plan.accepted_candidate_count, 0);
    assert_eq!(import_plan.rejected_for_budget_count, 0);
    assert_eq!(import_plan.total_budget, 8);
    assert_eq!(import_plan.min_unit_budget, 4);
    assert_eq!(import_plan.max_unit_budget, 4);
    assert_eq!(import_plan.total_candidate_score, 0);
    assert_eq!(import_plan.imported_score, 0);
    assert_eq!(import_plan.imported_workload, 0);
}

#[test]
fn summary_planner_imports_small_inline_shared_helpers_across_units() {
    let shared = inline_function(10, "shared", MastLinkage::Internal, Vec::new());
    let root_a = function(1, "a", MastLinkage::External, vec![call(10)]);
    let root_b = function(2, "b", MastLinkage::External, vec![call(10)]);
    let module = MastModule {
        name: "demo".to_string(),
        structs: Vec::new(),
        globals: Vec::new(),
        functions: vec![root_a, root_b, shared],
        mono: MonoModuleMetadata::default(),
    };

    let mir_report = kernc_mir_lower::build_from_mast(&module);
    let outcome = plan_codegen_units_with_mir_summary(&module, &mir_report.summary, 2);

    assert_eq!(outcome.report.imported_function_count, 1);
    let import_plan = outcome
        .report
        .import_plan
        .as_ref()
        .expect("summary-driven planning should produce an import report");
    assert_eq!(import_plan.candidate_function_count, 1);
    assert_eq!(import_plan.accepted_candidate_count, 1);
    assert_eq!(import_plan.rejected_for_budget_count, 0);
    assert_eq!(import_plan.total_budget, 8);
    assert_eq!(import_plan.min_unit_budget, 4);
    assert_eq!(import_plan.max_unit_budget, 4);
    assert_eq!(import_plan.total_candidate_score, 50);
    assert_eq!(import_plan.imported_score, 50);
    assert_eq!(import_plan.imported_workload, 2);

    let importer = outcome
        .units
        .iter()
        .find(|unit| unit.imported_function_ids.contains(&MonoId(10)))
        .expect("one unit should import the shared inline helper body");
    let imported = materialize_codegen_unit(&module, importer)
        .functions
        .into_iter()
        .find(|function| function.id == MonoId(10))
        .expect("imported helper should be materialized");
    assert!(imported.body.is_some());
    assert_eq!(imported.linkage, MastLinkage::Internal);
    assert!(!imported.is_extern);
}

#[test]
fn summary_planner_can_import_small_inline_link_once_odr_helpers() {
    let shared = inline_function(10, "shared", MastLinkage::LinkOnceOdr, Vec::new());
    let root_a = function(1, "a", MastLinkage::External, vec![call(10)]);
    let root_b = function(2, "b", MastLinkage::External, vec![call(10)]);
    let module = MastModule {
        name: "demo".to_string(),
        structs: Vec::new(),
        globals: Vec::new(),
        functions: vec![root_a, root_b, shared],
        mono: MonoModuleMetadata::default(),
    };

    let mir_report = kernc_mir_lower::build_from_mast(&module);
    let outcome = plan_codegen_units_with_mir_summary(&module, &mir_report.summary, 2);

    assert_eq!(outcome.report.imported_function_count, 1);
    let importer = outcome
        .units
        .iter()
        .find(|unit| unit.imported_function_ids.contains(&MonoId(10)))
        .expect("one unit should import the shared linkonce helper body");
    let imported = materialize_codegen_unit(&module, importer)
        .functions
        .into_iter()
        .find(|function| function.id == MonoId(10))
        .expect("imported linkonce helper should be materialized");
    assert!(imported.body.is_some());
    assert_eq!(imported.linkage, MastLinkage::Internal);
    assert!(!imported.is_extern);
}

#[test]
fn summary_planner_imports_recursive_inline_helper_closure() {
    let shared_leaf = inline_function(11, "shared_leaf", MastLinkage::Internal, Vec::new());
    let shared_root = inline_function(10, "shared_root", MastLinkage::Internal, vec![call(11)]);
    let root_a = function(1, "a", MastLinkage::External, vec![call(10)]);
    let root_b = function(
        2,
        "b",
        MastLinkage::External,
        vec![call(10), call(10), call(10), call(10), call(10)],
    );
    let module = MastModule {
        name: "demo".to_string(),
        structs: Vec::new(),
        globals: Vec::new(),
        functions: vec![root_a, root_b, shared_root, shared_leaf],
        mono: MonoModuleMetadata::default(),
    };

    let mir_report = kernc_mir_lower::build_from_mast(&module);
    let outcome = plan_codegen_units_with_mir_summary(&module, &mir_report.summary, 2);

    assert_eq!(outcome.report.imported_function_count, 2);
    let import_plan = outcome
        .report
        .import_plan
        .as_ref()
        .expect("summary-driven planning should produce an import report");
    assert_eq!(import_plan.candidate_function_count, 1);
    assert_eq!(import_plan.accepted_candidate_count, 1);
    assert_eq!(import_plan.rejected_for_budget_count, 0);
    assert_eq!(import_plan.total_budget, 11);
    assert_eq!(import_plan.min_unit_budget, 5);
    assert_eq!(import_plan.max_unit_budget, 6);
    assert_eq!(import_plan.total_candidate_score, 84);
    assert_eq!(import_plan.imported_score, 84);
    assert_eq!(import_plan.imported_workload, 6);

    let importer = outcome
        .units
        .iter()
        .find(|unit| unit.imported_function_ids.contains(&MonoId(10)))
        .expect("one unit should import the shared inline helper body");
    assert!(importer.imported_function_ids.contains(&MonoId(11)));

    let materialized = materialize_codegen_unit(&module, importer);
    let imported_root = materialized
        .functions
        .iter()
        .find(|function| function.id == MonoId(10))
        .expect("imported shared root should be materialized");
    assert!(imported_root.body.is_some());

    let imported_leaf = materialized
        .functions
        .iter()
        .find(|function| function.id == MonoId(11))
        .expect("imported shared leaf should be materialized");
    assert!(imported_leaf.body.is_some());
}

#[test]
fn summary_planner_skips_inline_imports_that_exceed_unit_budget() {
    let shared_14 = inline_function(14, "shared_14", MastLinkage::Internal, Vec::new());
    let shared_13 = inline_function(13, "shared_13", MastLinkage::Internal, vec![call(14)]);
    let shared_12 = inline_function(12, "shared_12", MastLinkage::Internal, vec![call(13)]);
    let shared_11 = inline_function(11, "shared_11", MastLinkage::Internal, vec![call(12)]);
    let shared_10 = inline_function(10, "shared_10", MastLinkage::Internal, vec![call(11)]);
    let root_a = function(1, "a", MastLinkage::External, vec![call(10)]);
    let root_b = function(2, "b", MastLinkage::External, vec![call(10)]);
    let module = MastModule {
        name: "demo".to_string(),
        structs: Vec::new(),
        globals: Vec::new(),
        functions: vec![
            root_a, root_b, shared_10, shared_11, shared_12, shared_13, shared_14,
        ],
        mono: MonoModuleMetadata::default(),
    };

    let mir_report = kernc_mir_lower::build_from_mast(&module);
    let outcome = plan_codegen_units_with_mir_summary(&module, &mir_report.summary, 2);

    assert_eq!(outcome.report.imported_function_count, 0);
    let import_plan = outcome
        .report
        .import_plan
        .as_ref()
        .expect("summary-driven planning should produce an import report");
    assert_eq!(import_plan.candidate_function_count, 1);
    assert_eq!(import_plan.accepted_candidate_count, 0);
    assert_eq!(import_plan.rejected_for_budget_count, 1);
    assert_eq!(import_plan.total_budget, 15);
    assert_eq!(import_plan.min_unit_budget, 4);
    assert_eq!(import_plan.max_unit_budget, 11);
    assert_eq!(import_plan.total_candidate_score, 6);
    assert_eq!(import_plan.imported_score, 0);
    assert_eq!(import_plan.imported_workload, 0);
    assert!(
        outcome
            .units
            .iter()
            .all(|unit| unit.imported_function_ids.is_empty())
    );
}

#[test]
fn summary_planner_disables_thin_partitioning_for_control_flow_asm_bodies() {
    let asm_root = function(
        1,
        "_start",
        MastLinkage::External,
        vec![void_expr(MastExprKind::Asm(MastAsmBlock {
            asm_template: "call kern_entry_linux".to_string(),
            constraints: "~{memory}".to_string(),
            input_args: Vec::new(),
            output_ptrs: Vec::new(),
            output_tys: Vec::new(),
            is_volatile: true,
        }))],
    );
    let helper = function(2, "kern_entry_linux", MastLinkage::External, Vec::new());
    let module = MastModule {
        name: "demo".to_string(),
        structs: Vec::new(),
        globals: Vec::new(),
        functions: vec![asm_root, helper],
        mono: MonoModuleMetadata::default(),
    };

    let mir_report = kernc_mir_lower::build_from_mast(&module);
    let outcome = plan_codegen_units_with_mir_summary(&module, &mir_report.summary, 2);

    assert!(outcome.units.is_empty());
    assert_eq!(
        outcome.report.fallback_reason,
        Some(CodegenPlanFallback::ContainsControlFlowAsm {
            function_name: "_start".to_string(),
        })
    );
}
