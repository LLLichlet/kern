use super::*;
use crate::CodegenPlanFallback;

#[test]
fn structure_cache_reuses_loaded_frontend_modules_until_input_changes() {
    let root = std::env::temp_dir().join(format!(
        "kern_structure_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let mut dirty = SourceOverrides::new();
    dirty.insert(main.clone(), "fn main() i32 { return 2; }".to_string());
    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &dirty)
            .is_some()
    );
    assert!(driver.uncached_parse_count() > parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_structure_reuses_clean_typed_structure_for_body_only_overrides() {
    let root = std::env::temp_dir().join(format!(
        "kern_structure_body_only_reuse_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let clean = SourceOverrides::new();

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &clean)
            .is_some()
    );
    let reuse_count = driver.body_only_structure_reuse_count();
    let parse_count = driver.uncached_parse_count();

    let mut dirty = SourceOverrides::new();
    dirty.insert(main.clone(), "fn main() i32 { return 2; }".to_string());
    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &dirty)
            .is_some()
    );
    assert_eq!(driver.body_only_structure_reuse_count(), reuse_count + 1);

    let dirty_parse_count = driver.uncached_parse_count();
    assert!(dirty_parse_count > parse_count);

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &dirty)
            .is_some()
    );
    assert_eq!(driver.body_only_structure_reuse_count(), reuse_count + 1);
    assert_eq!(driver.uncached_parse_count(), dirty_parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_structure_does_not_reuse_clean_typed_structure_for_surface_changes() {
    let root = std::env::temp_dir().join(format!(
        "kern_structure_surface_change_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let clean = SourceOverrides::new();

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &clean)
            .is_some()
    );
    let reuse_count = driver.body_only_structure_reuse_count();

    let mut dirty = SourceOverrides::new();
    dirty.insert(main.clone(), "fn main() bool { return true; }".to_string());
    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &dirty)
            .is_some()
    );
    assert_eq!(driver.body_only_structure_reuse_count(), reuse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn parse_modules_reuses_cached_structure_without_extra_frontend_parse() {
    let root = std::env::temp_dir().join(format!(
        "kern_parse_modules_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    let parsed = driver
        .parse_modules(main.to_str().unwrap(), &overrides)
        .expect("parsed modules should be available from cached structure");
    assert!(!parsed.modules.is_empty());
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_structure_only_loads_referenced_alias_roots() {
    let root = std::env::temp_dir().join(format!(
        "kern_alias_root_references_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();

    let main = root.join("main.rn");
    let foo = root.join("foo");
    let bar = root.join("bar");
    fs::create_dir_all(&foo).unwrap();
    fs::create_dir_all(&bar).unwrap();

    fs::write(&main, "use foo.answer;\nfn main() i32 { return answer; }\n").unwrap();
    fs::write(foo.join("init.rn"), "pub const answer = 7;\n").unwrap();
    fs::write(bar.join("init.rn"), "pub const unused = 9;\n").unwrap();

    let mut options = CompileOptions::default();
    options
        .module_aliases
        .insert("foo".to_string(), foo.to_string_lossy().to_string());
    options
        .module_aliases
        .insert("bar".to_string(), bar.to_string_lossy().to_string());
    let driver = CompilerDriver::new(options);

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &SourceOverrides::new())
            .is_some()
    );
    assert_eq!(driver.uncached_parse_count(), 2);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn shared_incremental_state_reuses_frontend_cache_across_output_variants() {
    let root = std::env::temp_dir().join(format!(
        "kern_shared_incremental_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let options = CompileOptions {
        input_file: Some(main.to_string_lossy().to_string()),
        output_file: root.join("main.o").to_string_lossy().to_string(),
        ..CompileOptions::default()
    };
    let driver = CompilerDriver::new(options.clone());
    let overrides = SourceOverrides::new();

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    let shared = driver
        .share_incremental_state(CompileOptions {
            output_file: root.join("other.o").to_string_lossy().to_string(),
            ..options
        })
        .expect("output-only changes should preserve incremental compatibility");
    assert!(
        shared
            .analyze_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    assert_eq!(shared.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn shared_incremental_state_rejects_semantic_option_changes() {
    let driver = CompilerDriver::new(CompileOptions::default());
    let mut changed = CompileOptions::default();
    changed
        .custom_defines
        .insert("feature".to_string(), "enabled".to_string());

    assert!(driver.share_incremental_state(changed).is_none());

    let mut changed_runtime = CompileOptions::default();
    changed_runtime.runtime_entry = kernc_utils::config::RuntimeEntry::Rt;
    assert!(driver.share_incremental_state(changed_runtime).is_none());
}

#[test]
fn custom_runtime_entry_define_does_not_enable_program_entry_mode() {
    let root = std::env::temp_dir().join(format!(
        "kern_runtime_entry_define_cfg_only_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let source = root.join("main.rn");
    let object = root.join("main.o");
    fs::write(
        &source,
        concat!(
            "#[if(runtime_entry == \"rt\")]\n",
            "fn helper() i32 {\n",
            "    return 7;\n",
            "}\n",
            "\n",
            "fn exported() i32 {\n",
            "    return helper();\n",
            "}\n",
        ),
    )
    .unwrap();

    let mut options = CompileOptions {
        input_file: Some(source.to_string_lossy().to_string()),
        output_file: object.to_string_lossy().to_string(),
        driver_mode: DriverMode::CompileOnly,
        report_progress: false,
        ..CompileOptions::default()
    };
    options
        .custom_defines
        .insert("runtime_entry".to_string(), "rt".to_string());
    let driver = CompilerDriver::new(options);
    let report = driver.compile_with_report();
    assert!(
        report.is_some(),
        "custom cfg runtime_entry should not require a root main"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn compile_report_exposes_cache_hits_and_frontend_parse_deltas() {
    let root = std::env::temp_dir().join(format!(
        "kern_compile_cache_stats_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let object = root.join("main.o");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions {
        input_file: Some(main.to_string_lossy().to_string()),
        output_file: object.to_string_lossy().to_string(),
        driver_mode: DriverMode::CompileOnly,
        report_progress: false,
        ..CompileOptions::default()
    });

    let first = driver
        .compile_with_report()
        .expect("first compile should succeed");
    assert!(first.cache_stats.structure_misses > 0);
    assert!(first.cache_stats.fresh_frontend_parses > 0);

    let second = driver
        .compile_with_report()
        .expect("second compile should succeed");
    assert!(second.cache_stats.compile_structure_hits > 0);
    assert_eq!(second.cache_stats.fresh_frontend_parses, 0);

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &SourceOverrides::new())
            .is_some()
    );
    let third = driver
        .compile_with_report()
        .expect("structure-warmed compile should succeed");
    assert!(third.cache_stats.compile_structure_hits > 0);
    assert_eq!(third.cache_stats.fresh_frontend_parses, 0);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn compile_report_exposes_mir_workload_stats() {
    let root = std::env::temp_dir().join(format!(
        "kern_compile_mir_workload_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let object = root.join("main.o");
    fs::write(
        &main,
        concat!(
            "extern fn main(seed: i32) i32 {\n",
            "    let mut value = seed;\n",
            "    value = value + 1;\n",
            "    return value;\n",
            "}\n",
        ),
    )
    .unwrap();

    let report = CompilerDriver::new(CompileOptions {
        input_file: Some(main.to_string_lossy().to_string()),
        output_file: object.to_string_lossy().to_string(),
        driver_mode: DriverMode::CompileOnly,
        report_progress: false,
        ..CompileOptions::default()
    })
    .compile_with_report()
    .expect("compile should succeed");

    let mir = report
        .mir_workload
        .expect("compile report should include MIR workload");
    assert!(mir.functions >= 1);
    assert!(mir.function_bodies >= 1);
    assert!(mir.locals >= 1);
    assert!(mir.let_locals >= 1);
    assert!(mir.assign_instructions >= 1);
    assert!(mir.use_rvalues >= 1);
    assert!(mir.binary_rvalues >= 1);
    assert!(mir.returns >= 1);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn compile_report_only_collects_codegen_diagnostics_when_requested() {
    let root = std::env::temp_dir().join(format!(
        "kern_compile_codegen_diag_toggle_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let object = root.join("main.o");
    fs::write(&main, "fn main() i32 { return 1; }\n").unwrap();

    let base_options = CompileOptions {
        input_file: Some(main.to_string_lossy().to_string()),
        output_file: object.to_string_lossy().to_string(),
        driver_mode: DriverMode::CompileOnly,
        report_progress: false,
        ..CompileOptions::default()
    };

    let without_diagnostics = CompilerDriver::new(base_options.clone())
        .compile_with_report()
        .expect("compile without timing diagnostics should succeed");
    assert!(without_diagnostics.ir_instruction_stats.is_none());
    assert!(without_diagnostics.ir_cleanup_stats.is_none());
    assert!(without_diagnostics.remaining_alloca_stats.is_none());
    assert!(without_diagnostics.remaining_alloca_names.is_empty());
    assert!(without_diagnostics.ir_hot_functions.is_empty());
    assert_eq!(without_diagnostics.codegen_alloca_stats, Default::default());

    let with_diagnostics = CompilerDriver::new(CompileOptions {
        report_timings: true,
        ..base_options
    })
    .compile_with_report()
    .expect("compile with timing diagnostics should succeed");
    assert!(with_diagnostics.ir_instruction_stats.is_some());
    assert!(with_diagnostics.ir_cleanup_stats.is_some());
    assert!(with_diagnostics.remaining_alloca_stats.is_some());

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn compile_only_merges_multiple_codegen_units_into_a_linkable_object() {
    let root = std::env::temp_dir().join(format!(
        "kern_multi_cgu_compile_only_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let object = root.join("main.o");
    let executable = root.join("main.out");
    fs::write(
        &main,
        "\
extern fn main() i32 {
    return foo() + bar();
}

extern fn foo() i32 {
    return 1;
}

extern fn bar() i32 {
    return 2;
}
",
    )
    .unwrap();

    let compile_driver = CompilerDriver::new(CompileOptions {
        input_file: Some(main.to_string_lossy().to_string()),
        output_file: object.to_string_lossy().to_string(),
        driver_mode: DriverMode::CompileOnly,
        codegen_units: 2,
        report_progress: false,
        ..CompileOptions::default()
    });
    compile_driver
        .compile_with_report()
        .expect("multi-CGU compile-only should succeed");
    assert!(object.is_file());
    assert!(fs::metadata(&object).unwrap().len() > 0);

    let link_driver = CompilerDriver::new(CompileOptions {
        output_file: executable.to_string_lossy().to_string(),
        driver_mode: DriverMode::LinkOnly,
        linker_inputs: vec![object.to_string_lossy().to_string()],
        report_progress: false,
        ..CompileOptions::default()
    });
    assert!(link_driver.compile());

    let status = Command::new(&executable).status().unwrap();
    assert_eq!(status.code(), Some(3));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn compile_only_full_lto_merges_multiple_codegen_units_into_a_linkable_object() {
    let root = std::env::temp_dir().join(format!(
        "kern_multi_cgu_full_lto_compile_only_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let object = root.join("main.o");
    let executable = root.join("main.out");
    fs::write(
        &main,
        "\
extern fn main() i32 {
    return foo() + bar();
}

extern fn foo() i32 {
    return 1;
}

extern fn bar() i32 {
    return 2;
}
",
    )
    .unwrap();

    let compile_driver = CompilerDriver::new(CompileOptions {
        input_file: Some(main.to_string_lossy().to_string()),
        output_file: object.to_string_lossy().to_string(),
        driver_mode: DriverMode::CompileOnly,
        codegen_units: 2,
        lto_mode: LtoMode::Full,
        report_progress: false,
        ..CompileOptions::default()
    });
    compile_driver
        .compile_with_report()
        .expect("multi-CGU full-LTO compile-only should succeed");
    assert!(object.is_file());
    assert!(fs::metadata(&object).unwrap().len() > 0);

    let link_driver = CompilerDriver::new(CompileOptions {
        output_file: executable.to_string_lossy().to_string(),
        driver_mode: DriverMode::LinkOnly,
        linker_inputs: vec![object.to_string_lossy().to_string()],
        report_progress: false,
        ..CompileOptions::default()
    });
    assert!(link_driver.compile());

    let status = Command::new(&executable).status().unwrap();
    assert_eq!(status.code(), Some(3));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn compile_report_does_not_enable_summary_imports_without_thin_lto() {
    let root = std::env::temp_dir().join(format!(
        "kern_compile_no_thin_import_plan_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let object = root.join("main.o");
    fs::write(
        &main,
        "\
#[inline]
fn shared(seed: i32) i32 {
    if (seed > 0) {
        return seed;
    }
    if (seed < 0) {
        return -seed;
    }
    return 0;
}

extern fn left(seed: i32) i32 {
    return shared(seed);
}

extern fn right(seed: i32) i32 {
    return shared(seed);
}
",
    )
    .unwrap();

    let driver = CompilerDriver::new(CompileOptions {
        input_file: Some(main.to_string_lossy().to_string()),
        output_file: object.to_string_lossy().to_string(),
        driver_mode: DriverMode::CompileOnly,
        codegen_units: 2,
        report_progress: false,
        ..CompileOptions::default()
    });
    let report = driver
        .compile_with_report()
        .expect("multi-CGU compile-only without thin LTO should succeed");
    let codegen_plan = report
        .codegen_plan
        .expect("compile-only should record a codegen plan");

    assert_eq!(codegen_plan.root_count, 2);
    assert_eq!(codegen_plan.planned_units, 2);
    assert_eq!(codegen_plan.imported_function_count, 0);
    assert!(codegen_plan.import_plan.is_none());

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn compile_report_exposes_import_plan_stats_for_thin_lto_summary_imports() {
    let root = std::env::temp_dir().join(format!(
        "kern_compile_thin_import_plan_stats_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let object = root.join("main.o");
    fs::write(
        &main,
        "\
#[inline]
fn shared(seed: i32) i32 {
    if (seed > 0) {
        return seed;
    }
    if (seed < 0) {
        return -seed;
    }
    return 0;
}

extern fn left(seed: i32) i32 {
    return shared(seed);
}

extern fn right(seed: i32) i32 {
    return shared(seed);
}
",
    )
    .unwrap();

    let driver = CompilerDriver::new(CompileOptions {
        input_file: Some(main.to_string_lossy().to_string()),
        output_file: object.to_string_lossy().to_string(),
        driver_mode: DriverMode::CompileOnly,
        codegen_units: 2,
        lto_mode: LtoMode::Thin,
        report_progress: false,
        ..CompileOptions::default()
    });
    let report = driver
        .compile_with_report()
        .expect("multi-CGU thin-LTO compile-only with summary import should succeed");
    let codegen_plan = report
        .codegen_plan
        .expect("compile-only should record a codegen plan");
    let import_plan = codegen_plan
        .import_plan
        .as_ref()
        .expect("summary-driven multi-CGU plan should record import stats");

    assert_eq!(codegen_plan.root_count, 2);
    assert_eq!(codegen_plan.planned_units, 2);
    assert!(codegen_plan.fallback_reason.is_none());
    assert!(import_plan.total_budget > 0);
    assert!(import_plan.candidate_function_count >= import_plan.accepted_candidate_count);
    assert!(import_plan.total_candidate_score >= import_plan.imported_score);
    assert!(import_plan.imported_workload >= codegen_plan.imported_function_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn compile_only_preserve_objects_falls_back_when_program_has_single_external_root() {
    let root = std::env::temp_dir().join(format!(
        "kern_multi_cgu_preserve_objects_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let object = root.join("main.o");
    let object_dir = root.join("main.o.d");
    let executable = root.join("main.out");
    fs::write(
        &main,
        "\
extern fn main() i32 {
    return foo() + bar();
}

fn foo() i32 {
    return 1;
}

fn bar() i32 {
    return 2;
}
",
    )
    .unwrap();

    let compile_driver = CompilerDriver::new(CompileOptions {
        input_file: Some(main.to_string_lossy().to_string()),
        output_file: object.to_string_lossy().to_string(),
        driver_mode: DriverMode::CompileOnly,
        codegen_units: 2,
        emit_multi_object_dir: true,
        report_progress: false,
        ..CompileOptions::default()
    });
    let report = compile_driver
        .compile_with_report()
        .expect("compile-only with requested preserved objects should succeed");
    let codegen_plan = report
        .codegen_plan
        .expect("compile-only should record a codegen plan");
    assert_eq!(codegen_plan.root_count, 1);
    assert_eq!(codegen_plan.planned_units, 0);
    assert!(
        matches!(
            codegen_plan.fallback_reason,
            Some(CodegenPlanFallback::TooFewRoots)
        ),
        "expected single-root fallback, got report: {codegen_plan:#?}"
    );

    assert!(object.is_file());
    assert!(!object_dir.exists());

    let link_driver = CompilerDriver::new(CompileOptions {
        output_file: executable.to_string_lossy().to_string(),
        driver_mode: DriverMode::LinkOnly,
        linker_inputs: vec![object.to_string_lossy().to_string()],
        report_progress: false,
        ..CompileOptions::default()
    });
    assert!(link_driver.compile());

    let status = Command::new(&executable).status().unwrap();
    assert_eq!(status.code(), Some(3));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_outline_reuses_collected_cache_without_extra_frontend_parse() {
    let root = std::env::temp_dir().join(format!(
        "kern_outline_collected_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    let parsed = driver
        .parse_modules(main.to_str().unwrap(), &overrides)
        .expect("parsed modules should be available");
    assert!(!parsed.modules.is_empty());
    let parse_count = driver.uncached_parse_count();

    let outline = driver.analyze_outline(main.to_str().unwrap(), &overrides);
    assert!(!outline.symbols.is_empty());
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_outline_reuses_clean_collected_structure_for_body_only_overrides() {
    let root = std::env::temp_dir().join(format!(
        "kern_outline_body_only_reuse_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let clean = SourceOverrides::new();

    let parsed = driver
        .parse_modules(main.to_str().unwrap(), &clean)
        .expect("parsed modules should warm clean collected cache");
    assert!(!parsed.modules.is_empty());
    let reuse_count = driver.body_only_collected_reuse_count();
    let parse_count = driver.uncached_parse_count();

    let mut dirty = SourceOverrides::new();
    dirty.insert(main.clone(), "fn main() i32 { return 2; }".to_string());
    let outline = driver.analyze_outline(main.to_str().unwrap(), &dirty);
    assert!(!outline.symbols.is_empty());
    assert_eq!(driver.body_only_collected_reuse_count(), reuse_count + 1);

    let dirty_parse_count = driver.uncached_parse_count();
    assert!(dirty_parse_count > parse_count);

    let outline = driver.analyze_outline(main.to_str().unwrap(), &dirty);
    assert!(!outline.symbols.is_empty());
    assert_eq!(driver.body_only_collected_reuse_count(), reuse_count + 1);
    assert_eq!(driver.uncached_parse_count(), dirty_parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_outline_does_not_reuse_clean_collected_structure_for_surface_changes() {
    let root = std::env::temp_dir().join(format!(
        "kern_outline_surface_change_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let clean = SourceOverrides::new();

    let parsed = driver
        .parse_modules(main.to_str().unwrap(), &clean)
        .expect("parsed modules should warm clean collected cache");
    assert!(!parsed.modules.is_empty());
    let reuse_count = driver.body_only_collected_reuse_count();

    let mut dirty = SourceOverrides::new();
    dirty.insert(
        main.clone(),
        "fn main(value: i32) i32 { return value; }".to_string(),
    );
    let outline = driver.analyze_outline(main.to_str().unwrap(), &dirty);
    assert!(!outline.symbols.is_empty());
    assert_eq!(driver.body_only_collected_reuse_count(), reuse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn imported_structure_cache_reuses_loaded_frontend_modules_without_type_stage() {
    let root = std::env::temp_dir().join(format!(
        "kern_imported_structure_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    let imported = driver
        .analyze_imported_structure(main.to_str().unwrap(), &overrides)
        .expect("imported structure should be available");
    let parse_count = driver.uncached_parse_count();

    let items = imported.completion_items(main.as_path(), 0);
    assert!(!items.is_empty());

    let imported_again = driver
        .analyze_imported_structure(main.to_str().unwrap(), &overrides)
        .expect("imported structure should stay cached");
    assert!(
        !imported_again
            .completion_items(main.as_path(), 0)
            .is_empty()
    );
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_imported_structure_reuses_clean_imported_structure_for_body_only_overrides() {
    let root = std::env::temp_dir().join(format!(
        "kern_imported_body_only_reuse_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let clean = SourceOverrides::new();

    assert!(
        driver
            .analyze_imported_structure(main.to_str().unwrap(), &clean)
            .is_some()
    );
    let reuse_count = driver.body_only_imported_reuse_count();
    let parse_count = driver.uncached_parse_count();

    let mut dirty = SourceOverrides::new();
    dirty.insert(main.clone(), "fn main() i32 { return 2; }".to_string());
    assert!(
        driver
            .analyze_imported_structure(main.to_str().unwrap(), &dirty)
            .is_some()
    );
    assert_eq!(driver.body_only_imported_reuse_count(), reuse_count + 1);

    let dirty_parse_count = driver.uncached_parse_count();
    assert!(dirty_parse_count > parse_count);

    assert!(
        driver
            .analyze_imported_structure(main.to_str().unwrap(), &dirty)
            .is_some()
    );
    assert_eq!(driver.body_only_imported_reuse_count(), reuse_count + 1);
    assert_eq!(driver.uncached_parse_count(), dirty_parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_imported_structure_does_not_reuse_clean_imported_structure_for_surface_changes() {
    let root = std::env::temp_dir().join(format!(
        "kern_imported_surface_change_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let clean = SourceOverrides::new();

    assert!(
        driver
            .analyze_imported_structure(main.to_str().unwrap(), &clean)
            .is_some()
    );
    let reuse_count = driver.body_only_imported_reuse_count();

    let mut dirty = SourceOverrides::new();
    dirty.insert(
        main.clone(),
        "fn main(value: i32) i32 { return value; }".to_string(),
    );
    assert!(
        driver
            .analyze_imported_structure(main.to_str().unwrap(), &dirty)
            .is_some()
    );
    assert_eq!(driver.body_only_imported_reuse_count(), reuse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn surface_artifact_reuses_cached_imported_stage_without_extra_frontend_parse() {
    let root = std::env::temp_dir().join(format!(
        "kern_surface_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    assert!(
        driver
            .analyze_imported_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    let surface = driver
        .analyze_surface(main.to_str().unwrap(), &overrides)
        .expect("surface artifact should be derivable from imported cache");
    assert!(!surface.symbols.is_empty());
    assert!(!surface.completion_items(main.as_path(), 0).is_empty());
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_structure_reuses_cached_imported_stage_without_extra_frontend_parse() {
    let root = std::env::temp_dir().join(format!(
        "kern_structure_from_imported_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    assert!(
        driver
            .analyze_imported_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    let structure = driver
        .analyze_structure(main.to_str().unwrap(), &overrides)
        .expect("typed structure should be derivable from imported cache");
    assert!(!structure.completion_items(main.as_path(), 0).is_empty());
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn parse_modules_reuses_cached_imported_stage_without_extra_frontend_parse() {
    let root = std::env::temp_dir().join(format!(
        "kern_parse_from_imported_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    assert!(
        driver
            .analyze_imported_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    let parsed = driver
        .parse_modules(main.to_str().unwrap(), &overrides)
        .expect("parsed modules should be derivable from imported cache");
    assert!(!parsed.modules.is_empty());
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn parsed_modules_cache_preserves_body_completion_regions() {
    let root = std::env::temp_dir().join(format!(
        "kern_parse_body_regions_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = "fn main() i32 {\n    return 1;\n}\n";
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    assert!(
        driver
            .analyze_imported_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    let parsed = driver
        .parse_modules(main.to_str().unwrap(), &overrides)
        .expect("parsed modules should be derivable from imported cache");
    let body_offset = source.find("return").unwrap();
    let top_level_offset = source.find("fn").unwrap();

    assert!(parsed.requires_body_completion(main.as_path(), body_offset));
    assert!(!parsed.requires_body_completion(main.as_path(), top_level_offset));
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}
