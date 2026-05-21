//! Execution tests for runtime package cache reuse.

use super::*;
use kernc_driver::CodegenPlanFallback;
use kernc_utils::config::{CodeModel, LtoMode};

#[test]
fn runtime_packages_are_reused_across_fresh_workspaces() {
    let cache_root = temp_dir("craft-runtime-cache-shared");
    let root_a = temp_dir("craft-runtime-cache-a");
    let root_b = temp_dir("craft-runtime-cache-b");

    let build_workspace = |root: &Path| {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            r#"
[package]
name = "hello"
version = "0.1.0"
kern = "0.8.1"

[[bin]]
name = "hello"
root = "src/main.kn"
"#,
        )
        .unwrap();
        fs::write(root.join("src/main.kn"), "fn main() i32 { return 0; }\n").unwrap();

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
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        build(&build_plan, &action_plan).unwrap()
    };

    let (first, second) =
        super::runtime_packages::with_test_runtime_cache_root(cache_root.clone(), || {
            (build_workspace(&root_a), build_workspace(&root_b))
        });

    assert_eq!(first.compile_actions, 1);
    assert_eq!(first.link_actions, 1);
    assert_eq!(first.action_cache_stats.compile_hits, 0);
    assert!(first.action_cache_stats.compile_misses > 0);

    assert_eq!(second.compile_actions, 1);
    assert_eq!(second.link_actions, 1);
    assert!(second.action_cache_stats.compile_hits > 0);
    assert!(second.action_cache_stats.compile_misses > 0);
    assert_eq!(second.action_cache_stats.link_hits, 0);
    assert_eq!(second.action_cache_stats.link_misses, 1);

    let _ = fs::remove_dir_all(cache_root);
    let _ = fs::remove_dir_all(root_a);
    let _ = fs::remove_dir_all(root_b);
}

#[test]
fn runtime_packages_respect_profile_opt_level() {
    let cache_root = temp_dir("craft-runtime-cache-opt-shared");
    let root_o1 = temp_dir("craft-runtime-cache-o1");
    let root_o3 = temp_dir("craft-runtime-cache-o3");

    let build_workspace = |root: &Path, opt: u8| {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            format!(
                r#"
[package]
name = "hello"
version = "0.1.0"
kern = "0.8.1"

[profile.release]
opt = {opt}

[[bin]]
name = "hello"
root = "src/main.kn"
"#
            ),
        )
        .unwrap();
        fs::write(root.join("src/main.kn"), "fn main() i32 { return 0; }\n").unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Build,
            &FeatureSelection {
                profile: crate::script::ProfileSelection::Release,
                ..FeatureSelection::default()
            },
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        build(&build_plan, &action_plan).unwrap()
    };

    let (first, second) =
        super::runtime_packages::with_test_runtime_cache_root(cache_root.clone(), || {
            (build_workspace(&root_o1, 1), build_workspace(&root_o3, 3))
        });

    assert_eq!(first.compile_actions, 1);
    assert_eq!(first.link_actions, 1);
    assert_eq!(first.action_cache_stats.compile_hits, 0);
    assert!(first.action_cache_stats.compile_misses > 0);

    assert_eq!(second.compile_actions, 1);
    assert_eq!(second.link_actions, 1);
    assert_eq!(second.action_cache_stats.compile_hits, 0);
    assert!(second.action_cache_stats.compile_misses > 0);

    let _ = fs::remove_dir_all(cache_root);
    let _ = fs::remove_dir_all(root_o1);
    let _ = fs::remove_dir_all(root_o3);
}

#[test]
fn runtime_packages_respect_profile_codegen_units() {
    let cache_root = temp_dir("craft-runtime-cache-cgu-shared");
    let root_cgu1 = temp_dir("craft-runtime-cache-cgu1");
    let root_cgu3 = temp_dir("craft-runtime-cache-cgu3");

    let build_workspace = |root: &Path, codegen_units: usize| {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Craft.toml"),
            format!(
                r#"
[package]
name = "hello"
version = "0.1.0"
kern = "0.8.1"

[profile.release]
opt = 3
codegen-units = {codegen_units}

[[bin]]
name = "hello"
root = "src/main.kn"
"#
            ),
        )
        .unwrap();
        fs::write(root.join("src/main.kn"), "fn main() i32 { return 0; }\n").unwrap();

        let manifest_path = root.join("Craft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Build,
            &FeatureSelection {
                profile: crate::script::ProfileSelection::Release,
                ..FeatureSelection::default()
            },
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        build(&build_plan, &action_plan).unwrap()
    };

    let (first, second) =
        super::runtime_packages::with_test_runtime_cache_root(cache_root.clone(), || {
            (
                build_workspace(&root_cgu1, 1),
                build_workspace(&root_cgu3, 3),
            )
        });

    assert_eq!(first.compile_actions, 1);
    assert_eq!(first.link_actions, 1);
    assert_eq!(first.action_cache_stats.compile_hits, 0);
    assert!(first.action_cache_stats.compile_misses > 0);

    assert_eq!(second.compile_actions, 1);
    assert_eq!(second.link_actions, 1);
    assert_eq!(second.action_cache_stats.compile_hits, 0);
    assert!(second.action_cache_stats.compile_misses > 0);

    let _ = fs::remove_dir_all(cache_root);
    let _ = fs::remove_dir_all(root_cgu1);
    let _ = fs::remove_dir_all(root_cgu3);
}

#[test]
fn runtime_packages_preserve_multi_object_outputs_for_release_codegen_units() {
    let cache_root = temp_dir("craft-runtime-cache-multio-shared");
    let root = temp_dir("craft-runtime-cache-multio-workspace");
    let profile = crate::script::ScriptProfile {
        name: "release".to_string(),
        opt: 3,
        debug: false,
        codegen_units: 2,
        lto_mode: LtoMode::Thin,
        code_model: CodeModel::Default,
    };

    let summary = super::runtime_packages::with_test_runtime_cache_root(cache_root.clone(), || {
        build_release_hello_workspace(
            &root,
            "[profile.release]\nopt = 3\ncodegen-units = 2\nlto = \"thin\"",
        )
    });

    assert_eq!(summary.compile_actions, 1);
    assert_eq!(summary.link_actions, 1);
    let std_codegen_plan = summary
        .action_timings
        .iter()
        .find(|timing| timing.label.starts_with("std ("))
        .and_then(|timing| timing.codegen_plan.as_ref())
        .expect("runtime std compile should report a codegen plan");
    let std_fell_back_for_control_flow_asm = matches!(
        std_codegen_plan.fallback_reason,
        Some(CodegenPlanFallback::ContainsControlFlowAsm { .. })
    );

    let profile_root = cache_root.join(super::runtime_profile_key(&profile));
    let std_object = profile_root
        .join("obj")
        .join("std")
        .join("lib")
        .join("std.o");
    let std_object_dir = super::multi_linker_input_dir(&std_object);
    assert!(std_object.is_file());
    let linker_inputs = super::linker_input_paths_for_primary_output(&std_object).unwrap();
    if cfg!(windows) {
        assert!(!std_object_dir.is_dir());
        assert_eq!(linker_inputs.len(), 1);
    } else if std_fell_back_for_control_flow_asm {
        assert!(!std_object_dir.is_dir());
        assert_eq!(linker_inputs, vec![std_object.clone()]);
    } else if cfg!(target_os = "linux") {
        assert!(
            std_object_dir.is_dir(),
            "expected multi-object runtime std output without codegen fallback; plan: {std_codegen_plan:#?}"
        );
        assert!(linker_inputs.len() > 1);
    } else {
        if std_object_dir.is_dir() {
            assert!(linker_inputs.len() > 1);
        } else {
            assert_eq!(linker_inputs, vec![std_object.clone()]);
        }
    }
    assert!(
        linker_inputs
            .iter()
            .all(|path| super::has_llvm_bitcode_magic(path)),
        "expected preserved runtime linker inputs to stay as ThinLTO bitcode"
    );

    let rt_entry = profile_root
        .join("obj")
        .join("rt")
        .join("entry")
        .join("rt_entry_freestanding.o");
    assert!(rt_entry.is_file());
    assert!(
        !super::has_llvm_bitcode_magic(&rt_entry),
        "expected platform rt entry shim to remain a concrete object file"
    );

    let _ = fs::remove_dir_all(cache_root);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn runtime_packages_support_parallel_builds_with_shared_cache() {
    let cache_root = temp_dir("craft-runtime-cache-parallel-shared");
    let root_a = temp_dir("craft-runtime-cache-parallel-a");
    let root_b = temp_dir("craft-runtime-cache-parallel-b");

    let cache_root_a = cache_root.clone();
    let root_a_for_worker = root_a.clone();
    let worker_a = thread::spawn(move || {
        super::runtime_packages::with_test_runtime_cache_root(cache_root_a, || {
            build_release_hello_workspace(
                &root_a_for_worker,
                "[profile.release]\nopt = 3\ncodegen-units = 3",
            )
        })
    });

    let cache_root_b = cache_root.clone();
    let root_b_for_worker = root_b.clone();
    let worker_b = thread::spawn(move || {
        super::runtime_packages::with_test_runtime_cache_root(cache_root_b, || {
            build_release_hello_workspace(
                &root_b_for_worker,
                "[profile.release]\nopt = 3\ncodegen-units = 3",
            )
        })
    });

    let first = worker_a.join().expect("first runtime-cache build panicked");
    let second = worker_b
        .join()
        .expect("second runtime-cache build panicked");

    assert_eq!(first.compile_actions, 1);
    assert_eq!(first.link_actions, 1);
    assert_eq!(second.compile_actions, 1);
    assert_eq!(second.link_actions, 1);

    let _ = fs::remove_dir_all(cache_root);
    let _ = fs::remove_dir_all(root_a);
    let _ = fs::remove_dir_all(root_b);
}
