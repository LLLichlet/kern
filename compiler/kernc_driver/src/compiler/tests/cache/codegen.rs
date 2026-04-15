use super::*;

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
fn compile_only_thin_lto_bitcode_preserves_prelink_inputs() {
    let root = std::env::temp_dir().join(format!(
        "kern_thinlto_bitcode_compile_only_{}_{}",
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
        linker_input_flavor: kernc_utils::config::LinkerInputFlavor::ThinLtoBitcode,
        emit_multi_linker_input_dir: true,
        report_progress: false,
        ..CompileOptions::default()
    });
    driver
        .compile_with_report()
        .expect("ThinLTO bitcode compile-only emission should succeed");

    let linker_inputs = manifest_object_paths(&object);
    assert_eq!(linker_inputs.len(), 2);
    assert!(
        linker_inputs
            .iter()
            .all(|path| has_llvm_bitcode_magic(std::path::Path::new(path))),
        "expected preserved linker inputs to stay serialized as LLVM bitcode"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn compile_only_thin_lto_object_preserves_native_objects() {
    let root = std::env::temp_dir().join(format!(
        "kern_thinlto_object_compile_only_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let object = root.join("main.o");
    let thin_lto_cache_dir = root.join("main.o.thinlto-cache.d");
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
        emit_multi_linker_input_dir: true,
        report_progress: false,
        ..CompileOptions::default()
    });
    driver
        .compile_with_report()
        .expect("ThinLTO object compile-only emission should succeed");

    let linker_inputs = manifest_object_paths(&object);
    assert_eq!(linker_inputs.len(), 2);
    assert!(
        linker_inputs
            .iter()
            .all(|path| !has_llvm_bitcode_magic(std::path::Path::new(path))),
        "expected preserved linker inputs to be native objects instead of LLVM bitcode"
    );
    assert!(
        nm_reports_object(&linker_inputs),
        "expected preserved ThinLTO objects to be inspectable by `nm`"
    );
    assert!(thin_lto_cache_dir.is_dir());
    assert!(
        fs::read_dir(&thin_lto_cache_dir)
            .unwrap()
            .any(|entry| entry.is_ok()),
        "expected ThinLTO cache dir to contain cached outputs"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn compile_and_link_thin_lto_multi_cgu_produces_runnable_binary() {
    let root = std::env::temp_dir().join(format!(
        "kern_thinlto_link_exec_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let executable = root.join("main.out");
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

fn main() i32 {
    return left(3) + right(-2) - 5;
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

    let mut options = CompileOptions {
        input_file: Some(main.to_string_lossy().to_string()),
        output_file: executable.to_string_lossy().to_string(),
        runtime_entry: RuntimeEntry::Rt,
        library_bundle: LibraryBundle::Std,
        driver_mode: DriverMode::CompileAndLink,
        codegen_units: 2,
        lto_mode: LtoMode::Thin,
        report_progress: false,
        ..CompileOptions::default()
    };
    kernc_utils::config::apply_configured_library_aliases(&mut options);
    kernc_utils::config::inject_driver_condition_defines(&mut options);
    let driver = CompilerDriver::new(options);
    driver
        .compile_with_report()
        .expect("multi-CGU ThinLTO compile-and-link should succeed");

    let status = Command::new(&executable).status().unwrap();
    assert_eq!(status.code(), Some(0));

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
        emit_multi_linker_input_dir: true,
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
fn compile_only_preserve_objects_keeps_base_runtime_generic_definitions() {
    let root = std::env::temp_dir().join(format!(
        "kern_base_multi_object_defs_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let object = root.join("base.o");
    let metadata = root.join("base-meta");
    let base_root = kernc_utils::config::resolve_base_path();
    let source = base_root.join("init.rn");

    let mut options = CompileOptions {
        input_file: Some(source.to_string_lossy().to_string()),
        output_file: object.to_string_lossy().to_string(),
        metadata_output: Some(metadata.to_string_lossy().to_string()),
        metadata_package_name: Some("base".to_string()),
        root_module_name: Some("base".to_string()),
        driver_mode: DriverMode::CompileOnly,
        opt_level: kernc_utils::config::OptLevel::O3,
        codegen_units: 4,
        lto_mode: LtoMode::Thin,
        emit_multi_linker_input_dir: true,
        library_bundle: kernc_utils::config::LibraryBundle::Base,
        split_sections_for_gc: true,
        report_progress: false,
        ..CompileOptions::default()
    };
    options
        .module_aliases
        .insert("base".to_string(), base_root.to_string_lossy().to_string());
    let driver = CompilerDriver::new(options);
    driver
        .compile_with_report()
        .expect("base compile-only with preserved objects should succeed");

    let object_paths = manifest_object_paths(&object);
    assert!(
        !object_paths.is_empty(),
        "expected ThinLTO to preserve at least one object in the manifest"
    );

    let nm_output = Command::new("nm")
        .arg("-A")
        .args(&object_paths)
        .output()
        .expect("nm should inspect preserved CGU objects");
    assert!(
        nm_output.status.success(),
        "nm failed: {}",
        String::from_utf8_lossy(&nm_output.stderr)
    );
    let stdout = String::from_utf8(nm_output.stdout).unwrap();

    for symbol in [
        "_K4base4coll4list36PmutQ4base4coll4list4ListEI7unknownE15ensure_capacityI2u8E",
        "_K4base3mem6layout9layout_ofI2u8E",
    ] {
        assert!(
            stdout
                .lines()
                .any(|line| !line.contains(":                 U ") && line.contains(symbol)),
            "expected preserved CGU objects to define `{symbol}`, nm output was:\n{stdout}"
        );
    }

    assert!(
        [
            "_K4base4coll5slice5query8Sunknown11starts_withI2u8E",
            "_K4base4coll5slice5query8Sunknown9ends_withI2u8E",
            "_K4base4coll5slice5query8Sunknown4findI2u8E",
            "_K4base4coll5slice5query8Sunknown8containsI2u8E",
            "_K4base4coll5slice5query8Sunknown7lex_cmpI2u8E",
        ]
        .into_iter()
        .any(|symbol| {
            stdout
                .lines()
                .any(|line| !line.contains(":                 U ") && line.contains(symbol))
        }),
        "expected preserved CGU objects to define at least one generic slice query helper, nm output was:\n{stdout}"
    );

    let _ = fs::remove_dir_all(&root);
}
