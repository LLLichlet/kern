//! Build-plan tests for package target layout and command filtering.

use super::*;

#[test]
fn artifact_layout_keeps_versions_out_of_local_package_directories() {
    let root = temp_dir("craft-build-plan-artifact-layout");
    fs::create_dir_all(root.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

[[bin]]
name = "demo"
root = "src/main.kn"
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
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let actions = build_plan.derive_actions(&crate::script::host_target());
    let artifact = &actions.link_actions[0].artifact_path;
    let artifact_text = artifact.to_string_lossy().replace('\\', "/");

    assert!(artifact_text.contains("/.craft/build/dev/target-"));
    assert!(artifact_text.contains("/out/demo/bin/demo"));
    assert!(!artifact_text.contains("demo-0.1.0"));
    assert_eq!(build_plan.packages[0].layout_key, "demo");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn external_package_layout_keys_keep_identity_without_visible_versions() {
    let first = PackageId {
        name: "codegen".to_string(),
        version: "1.2.3".to_string(),
        source: crate::graph::SourceId::PathDependency {
            path: "vendor/codegen".to_string(),
        },
    };
    let second = PackageId {
        name: "codegen".to_string(),
        version: "1.2.3".to_string(),
        source: crate::graph::SourceId::GitDependency {
            git: "https://example.invalid/codegen.git".to_string(),
            rev: None,
            branch: None,
            tag: Some("v1.2.3".to_string()),
        },
    };

    let first_key = package_layout_key(&first);
    let second_key = package_layout_key(&second);

    assert!(first_key.starts_with("codegen~"));
    assert!(second_key.starts_with("codegen~"));
    assert!(!first_key.contains("1.2.3"));
    assert!(!second_key.contains("1.2.3"));
    assert_ne!(first_key, second_key);
}

#[test]
fn target_layout_key_separates_environment_variants() {
    let gnu = crate::script::ScriptTarget {
        os: ScriptOs::Linux,
        arch: "x86_64".to_string(),
        vendor: "unknown".to_string(),
        env: "gnu".to_string(),
    };
    let musl = crate::script::ScriptTarget {
        env: "musl".to_string(),
        ..gnu.clone()
    };

    assert_eq!(gnu.layout_key(), "x86_64-unknown-linux-gnu");
    assert_eq!(musl.layout_key(), "x86_64-unknown-linux-musl");
    assert_ne!(gnu.layout_key(), musl.layout_key());
}

#[test]
fn derives_workspace_build_units_from_package_targets() {
    let root = temp_dir("craft-build-plan-targets");
    let app_dir = root.join("app");
    fs::create_dir_all(&app_dir).unwrap();

    fs::write(
        root.join("Craft.toml"),
        r#"
[workspace]
name = "workspace"
members = ["app"]
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("Craft.toml"),
        r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.8.0"

[lib]
root = "src/lib.kn"

[[bin]]
name = "app"
root = "src/main.kn"

[test]
roots = ["tests/smoke.kn"]
"#,
    )
    .unwrap();

    let manifest_path = root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path).unwrap();
    let members = load_members(&manifest_path, &manifest).unwrap();
    let elaboration = plan(
        &manifest_path,
        &manifest,
        &members,
        true,
        crate::script::ScriptCommand::Check,
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Check).unwrap();
    let actions = build_plan.derive_actions(&crate::script::host_target());

    assert_eq!(build_plan.unit_count(), 3);
    assert_eq!(actions.compile_count(), 3);
    assert_eq!(actions.link_count(), 2);
    let app_package = build_plan
        .packages
        .iter()
        .find(|package| package.package_id.name == "app")
        .unwrap();
    assert!(app_package.units.iter().any(|unit| {
        unit.target_kind == TargetKind::Lib
            && unit.artifact_kind == ArtifactKind::Library
            && unit.artifact_name == "app"
    }));
    assert!(app_package.units.iter().any(|unit| {
        unit.target_kind == TargetKind::Bin
            && unit.artifact_kind == ArtifactKind::Executable
            && unit.artifact_name == "app"
    }));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn package_closure_filter_keeps_selected_member_and_local_dependencies() {
    let root = temp_dir("craft-build-plan-package-closure");
    let app_dir = root.join("app");
    let util_dir = root.join("util");
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::create_dir_all(util_dir.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        r#"
[workspace]
name = "workspace"
members = ["app", "util"]
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("Craft.toml"),
        r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.8.0"

[[bin]]
name = "app"
root = "src/main.kn"

[dependencies]
util = { path = "../util" }
"#,
    )
    .unwrap();
    fs::write(app_dir.join("src/main.kn"), "fn main() i32 { return 0; }\n").unwrap();
    fs::write(
        util_dir.join("Craft.toml"),
        r#"
[package]
name = "util"
version = "0.1.0"
kern = "0.8.0"

[lib]
root = "src/lib.kn"
"#,
    )
    .unwrap();
    fs::write(
        util_dir.join("src/lib.kn"),
        "pub fn value() i32 { return 0; }\n",
    )
    .unwrap();

    let manifest_path = root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path).unwrap();
    let members = load_members(&manifest_path, &manifest).unwrap();
    let elaboration = plan(
        &manifest_path,
        &manifest,
        &members,
        true,
        crate::script::ScriptCommand::Build,
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let app_id = elaboration
        .packages
        .iter()
        .find(|package| package.package_id.name == "app")
        .unwrap()
        .package_id
        .clone();

    let filtered = build_plan.filtered_package_closure(&[(BuildDomain::Target, app_id)]);

    assert!(
        filtered
            .packages
            .iter()
            .any(|package| package.package_id.name == "app")
    );
    assert!(
        filtered
            .packages
            .iter()
            .any(|package| package.package_id.name == "util")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_run_and_test_only_include_relevant_target_kinds() {
    let root = temp_dir("craft-build-plan-command-target-filter");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::create_dir_all(root.join("examples")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

[lib]
root = "src/lib.kn"

[[bin]]
name = "demo"
root = "src/main.kn"

[test]
roots = ["tests/smoke.kn"]

[example]
roots = ["examples/sample.kn"]
"#,
    )
    .unwrap();

    let manifest_path = root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path).unwrap();

    let build_elaboration = plan(
        &manifest_path,
        &manifest,
        &[],
        false,
        crate::script::ScriptCommand::Build,
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&build_elaboration, crate::script::ScriptCommand::Build).unwrap();
    let build_kinds = build_plan.packages[0]
        .units
        .iter()
        .map(|unit| unit.target_kind)
        .collect::<Vec<_>>();
    assert_eq!(build_kinds, vec![TargetKind::Lib, TargetKind::Bin]);

    let run_elaboration = plan(
        &manifest_path,
        &manifest,
        &[],
        false,
        crate::script::ScriptCommand::Run,
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let run_plan = derive(&run_elaboration, crate::script::ScriptCommand::Run).unwrap();
    let run_kinds = run_plan.packages[0]
        .units
        .iter()
        .map(|unit| unit.target_kind)
        .collect::<Vec<_>>();
    assert_eq!(run_kinds, vec![TargetKind::Lib, TargetKind::Bin]);

    let test_elaboration = plan(
        &manifest_path,
        &manifest,
        &[],
        false,
        crate::script::ScriptCommand::Test,
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let test_plan = derive(&test_elaboration, crate::script::ScriptCommand::Test).unwrap();
    let test_kinds = test_plan.packages[0]
        .units
        .iter()
        .map(|unit| unit.target_kind)
        .collect::<Vec<_>>();
    assert_eq!(test_kinds, vec![TargetKind::Lib, TargetKind::Test]);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_can_include_examples_when_requested() {
    let root = temp_dir("craft-build-plan-build-examples");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("examples")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

[lib]
root = "src/lib.kn"

[example]
roots = ["examples/sample.kn"]
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
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();

    let build_plan = derive_with_options(
        &elaboration,
        crate::script::ScriptCommand::Build,
        DeriveOptions {
            include_examples: true,
        },
    )
    .unwrap();
    let build_kinds = build_plan.packages[0]
        .units
        .iter()
        .map(|unit| unit.target_kind)
        .collect::<Vec<_>>();
    assert_eq!(build_kinds, vec![TargetKind::Lib, TargetKind::Example]);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn run_can_select_examples_without_building_bins() {
    let root = temp_dir("craft-build-plan-run-examples");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("examples")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

[lib]
root = "src/lib.kn"

[[bin]]
name = "demo"
root = "src/main.kn"

[example]
roots = ["examples/sample.kn"]
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
        crate::script::ScriptCommand::Run,
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();

    let run_plan = derive_with_options(
        &elaboration,
        crate::script::ScriptCommand::Run,
        DeriveOptions {
            include_examples: true,
        },
    )
    .unwrap();
    let run_kinds = run_plan.packages[0]
        .units
        .iter()
        .map(|unit| unit.target_kind)
        .collect::<Vec<_>>();
    assert_eq!(run_kinds, vec![TargetKind::Lib, TargetKind::Example]);

    let _ = fs::remove_dir_all(root);
}
