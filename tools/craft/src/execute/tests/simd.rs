use super::*;
use kernc_driver::CompilerDriver;
use std::collections::BTreeMap;

#[test]
fn craft_bin_compile_options_support_builtin_simd_types() {
    let cache_root = temp_dir("craft-simd-runtime-cache");
    let root = temp_dir("craft-exec-simd-bin");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "simd_probe"
version = "0.1.0"
kern = "0.7.6"

[[bin]]
name = "simd_probe"
root = "src/main.kn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.kn"),
        r#"
fn main() i32 {
    let v = u8x16.{ 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1 };
    return v.[0] as i32;
}
"#,
    )
    .unwrap();

    let manifest_path = root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path).unwrap();
    let elaboration = plan(
        &manifest_path,
        &manifest,
        &[],
        false,
        crate::script::ScriptCommand::Build,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let local_library_actions =
        super::external::local_library_actions(&action_plan.compile_actions);
    let action = action_plan
        .compile_actions
        .iter()
        .find(|action| action.target_kind == crate::plan::TargetKind::Bin)
        .unwrap();

    let mut built_std_packages = BTreeMap::new();
    let mut driver_families = BTreeMap::new();
    let mut execution_summary = super::super::ExecutionSummary::default();
    let mut manifest_runtime_options = BTreeMap::new();

    super::runtime_packages::with_test_runtime_cache_root(cache_root.clone(), || {
        super::super::ensure_std_packages_for_actions(
            &root,
            &action_plan.compile_actions,
            crate::script::ScriptCommand::Build,
            &mut built_std_packages,
            &mut driver_families,
            &mut execution_summary,
        )
        .unwrap();

        let options = super::super::compile_action_options(
            crate::script::ScriptCommand::Build,
            action,
            &local_library_actions,
            &built_std_packages,
            &BTreeMap::new(),
            &mut manifest_runtime_options,
            false,
        )
        .unwrap();

        fs::create_dir_all(
            Path::new(&options.output_file)
                .parent()
                .expect("compile output should have a parent directory"),
        )
        .unwrap();

        assert!(
            CompilerDriver::new(options).compile_with_report().is_some(),
            "craft-generated compile options should accept builtin SIMD vector types",
        );
    });

    let _ = fs::remove_dir_all(cache_root);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn craft_build_supports_builtin_simd_types_in_bin_targets() {
    let cache_root = temp_dir("craft-simd-runtime-cache-build");
    let root = temp_dir("craft-exec-simd-build");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "simd_probe"
version = "0.1.0"
kern = "0.7.6"

[[bin]]
name = "simd_probe"
root = "src/main.kn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.kn"),
        r#"
fn main() i32 {
    let v = u8x16.{ 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1 };
    return v.[0] as i32;
}
"#,
    )
    .unwrap();

    let manifest_path = root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path).unwrap();
    let elaboration = plan(
        &manifest_path,
        &manifest,
        &[],
        false,
        crate::script::ScriptCommand::Build,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());

    let summary =
        super::super::runtime_packages::with_test_runtime_cache_root(cache_root.clone(), || {
            build(&build_plan, &action_plan)
        });
    assert!(
        summary.is_ok(),
        "craft build should accept builtin SIMD vector types in bin targets: {summary:?}",
    );

    let _ = fs::remove_dir_all(cache_root);
    let _ = fs::remove_dir_all(root);
}
