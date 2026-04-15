use super::*;

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

    let changed_runtime = CompileOptions {
        runtime_entry: kernc_utils::config::RuntimeEntry::Rt,
        ..CompileOptions::default()
    };
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
fn analyze_only_skips_object_emission() {
    let root = std::env::temp_dir().join(format!(
        "kern_analyze_only_no_object_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let source = root.join("main.rn");
    let object = root.join("main.o");
    fs::write(&source, "fn main() i32 { return 1; }").unwrap();

    let report = CompilerDriver::new(CompileOptions {
        input_file: Some(source.to_string_lossy().to_string()),
        output_file: object.to_string_lossy().to_string(),
        driver_mode: DriverMode::AnalyzeOnly,
        report_progress: false,
        ..CompileOptions::default()
    })
    .compile_with_report()
    .expect("analyze-only should succeed");

    assert!(report.lower_cache_stats.is_none());
    assert!(!object.exists(), "analyze-only must not emit linker input");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_only_still_emits_metadata() {
    let root = std::env::temp_dir().join(format!(
        "kern_analyze_only_metadata_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let source = root.join("lib.rn");
    let object = root.join("lib.o");
    let metadata = root.join("meta");
    fs::write(&source, "pub fn answer() i32 { return 42; }").unwrap();

    CompilerDriver::new(CompileOptions {
        input_file: Some(source.to_string_lossy().to_string()),
        output_file: object.to_string_lossy().to_string(),
        metadata_output: Some(metadata.to_string_lossy().to_string()),
        metadata_package_name: Some("demo".to_string()),
        metadata_package_version: Some("0.1.0".to_string()),
        root_module_name: Some("demo".to_string()),
        driver_mode: DriverMode::AnalyzeOnly,
        report_progress: false,
        ..CompileOptions::default()
    })
    .compile_with_report()
    .expect("analyze-only metadata emission should succeed");

    assert!(
        metadata.join(crate::KMETA_MANIFEST_FILE).is_file(),
        "analyze-only library compile should emit kmeta"
    );
    assert!(!object.exists(), "analyze-only must not emit linker input");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_only_reports_semantic_errors() {
    let root = std::env::temp_dir().join(format!(
        "kern_analyze_only_semantic_error_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let source = root.join("main.rn");
    let object = root.join("main.o");
    fs::write(&source, "fn main() i32 { return missing_symbol; }").unwrap();

    let report = CompilerDriver::new(CompileOptions {
        input_file: Some(source.to_string_lossy().to_string()),
        output_file: object.to_string_lossy().to_string(),
        driver_mode: DriverMode::AnalyzeOnly,
        report_progress: false,
        ..CompileOptions::default()
    })
    .compile_with_report();

    assert!(
        report.is_none(),
        "analyze-only must still reject semantic errors"
    );

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
    assert!(
        without_diagnostics
            .phase_timings
            .iter()
            .all(|phase| !phase.name.starts_with("  lower_") && !phase.name.starts_with("    expr_"))
    );

    let with_diagnostics = CompilerDriver::new(CompileOptions {
        report_timings: true,
        ..base_options
    })
    .compile_with_report()
    .expect("compile with timing diagnostics should succeed");
    assert!(with_diagnostics.ir_instruction_stats.is_some());
    assert!(with_diagnostics.ir_cleanup_stats.is_some());
    assert!(with_diagnostics.remaining_alloca_stats.is_some());
    assert!(
        with_diagnostics
            .phase_timings
            .iter()
            .any(|phase| phase.name.starts_with("  lower_"))
    );
    assert!(
        with_diagnostics
            .phase_timings
            .iter()
            .any(|phase| phase.name.starts_with("    expr_"))
    );

    let _ = fs::remove_dir_all(&root);
}
