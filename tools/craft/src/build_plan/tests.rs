use super::{
    ArtifactKind, DeriveOptions, GeneratedFileOrigin, SourceRootBinding, StagedActionKind,
    StagedActionPhase, artifact_path, derive, derive_with_options,
};
use crate::elaborate::plan;
use crate::graph::BuildDomain;
use crate::graph::PackageId;
use crate::manifest::Manifest;
use crate::plan::TargetKind;
use crate::script::ScriptOs;
use crate::workspace::load_members;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn os_variant_name(os: ScriptOs) -> &'static str {
    match os {
        ScriptOs::Unknown => "unknown",
        ScriptOs::Linux => "linux",
        ScriptOs::Windows => "windows",
        ScriptOs::Darwin => "darwin",
    }
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
kern = "0.6.7"

[lib]
root = "src/lib.rn"

[[bin]]
name = "app"
root = "src/main.rn"

[test]
roots = ["tests/smoke.rn"]
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
[package]
name = "rootpkg"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "rootpkg"
root = "src/main.rn"

[workspace]
members = ["app", "util"]
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
    fs::write(
        app_dir.join("Craft.toml"),
        r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "app"
root = "src/main.rn"

[dependencies]
util = { path = "../util" }
"#,
    )
    .unwrap();
    fs::write(app_dir.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
    fs::write(
        util_dir.join("Craft.toml"),
        r#"
[package]
name = "util"
version = "0.1.0"
kern = "0.6.7"

[lib]
root = "src/lib.rn"
"#,
    )
    .unwrap();
    fs::write(util_dir.join("src/lib.rn"), "pub fn value() i32 { return 0; }\n").unwrap();

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

    assert!(filtered.packages.iter().any(|package| package.package_id.name == "app"));
    assert!(filtered.packages.iter().any(|package| package.package_id.name == "util"));
    assert!(filtered
        .packages
        .iter()
        .all(|package| package.package_id.name != "rootpkg"));

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
kern = "0.6.7"

[lib]
root = "src/lib.rn"

[[bin]]
name = "demo"
root = "src/main.rn"

[test]
roots = ["tests/smoke.rn"]

[[example]]
name = "sample"
root = "examples/sample.rn"
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
kern = "0.6.7"

[lib]
root = "src/lib.rn"

[[example]]
name = "sample"
root = "examples/sample.rn"
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
fn carries_local_and_external_dependencies_into_build_units() {
    let root = temp_dir("craft-build-plan-deps");
    let app_dir = root.join("app");
    let util_dir = root.join("util");
    let log_dir = root.join("vendor").join("log");
    fs::create_dir_all(&app_dir).unwrap();
    fs::create_dir_all(&util_dir).unwrap();
    fs::create_dir_all(log_dir.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        r#"
[workspace]
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
kern = "0.6.7"

[[bin]]
name = "app"
root = "src/main.rn"

[dependencies]
util = { path = "../util" }
log = { path = "../vendor/log", version = "1" }
"#,
    )
    .unwrap();
    fs::write(
        util_dir.join("Craft.toml"),
        r#"
[package]
name = "util"
version = "0.1.0"
kern = "0.6.7"

[lib]
root = "src/lib.rn"
"#,
    )
    .unwrap();
    fs::write(
        log_dir.join("Craft.toml"),
        r#"
[package]
name = "log"
version = "1"
kern = "0.6.7"

[lib]
root = "src/lib.rn"
"#,
    )
    .unwrap();
    fs::write(
        log_dir.join("src/lib.rn"),
        "pub fn answer() i32 { return 42; }\n",
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

    let app_unit = build_plan
        .packages
        .iter()
        .find(|package| package.package_id.name == "app")
        .unwrap()
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();
    assert_eq!(app_unit.local_dependencies.len(), 1);
    assert_eq!(app_unit.local_dependencies[0].dependency_name, "util");
    assert_eq!(app_unit.local_dependencies[0].package_id.name, "util");
    assert_eq!(app_unit.external_dependencies.len(), 1);
    assert_eq!(app_unit.external_dependencies[0].dependency_name, "log");
    assert_eq!(
        app_unit.external_dependencies[0].package_id.package_name,
        "log"
    );
    assert_eq!(build_plan.local_dependency_edge_count(), 1);
    assert_eq!(build_plan.external_dependency_edge_count(), 1);
    let link = actions
        .link_actions
        .iter()
        .find(|action| action.package_id.name == "app" && action.target_kind == TargetKind::Bin)
        .unwrap();
    assert_eq!(link.local_library_objects.len(), 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn isolates_build_dependencies_from_target_units() {
    let root = temp_dir("craft-build-plan-build-deps");
    let app_dir = root.join("app");
    let util_dir = root.join("util");
    let log_dir = root.join("vendor").join("log");
    let cc_dir = root.join("vendor").join("cc");
    fs::create_dir_all(&app_dir).unwrap();
    fs::create_dir_all(&util_dir).unwrap();
    fs::create_dir_all(log_dir.join("src")).unwrap();
    fs::create_dir_all(cc_dir.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        r#"
[workspace]
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
kern = "0.6.7"

[[bin]]
name = "app"
root = "src/main.rn"

[dependencies]
util = { path = "../util" }
log = { path = "../vendor/log", version = "1" }

[build-dependencies]
util_build = { path = "../util", package = "util" }
cc = { path = "../vendor/cc", version = "1" }
"#,
    )
    .unwrap();
    fs::write(
        util_dir.join("Craft.toml"),
        r#"
[package]
name = "util"
version = "0.1.0"
kern = "0.6.7"

[lib]
root = "src/lib.rn"
"#,
    )
    .unwrap();
    fs::write(
        log_dir.join("Craft.toml"),
        r#"
[package]
name = "log"
version = "1"
kern = "0.6.7"

[lib]
root = "src/lib.rn"
"#,
    )
    .unwrap();
    fs::write(
        log_dir.join("src/lib.rn"),
        "pub fn answer() i32 { return 42; }\n",
    )
    .unwrap();
    fs::write(
        cc_dir.join("Craft.toml"),
        r#"
[package]
name = "cc"
version = "1"
kern = "0.6.7"

[lib]
root = "src/lib.rn"
"#,
    )
    .unwrap();
    fs::write(
        cc_dir.join("src/lib.rn"),
        "pub fn tool() i32 { return 1; }\n",
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
    let app_package = build_plan
        .packages
        .iter()
        .find(|package| package.package_id.name == "app")
        .unwrap();
    let app_unit = app_package
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();

    assert_eq!(app_unit.domain, crate::graph::BuildDomain::Target);
    assert_eq!(app_unit.local_dependencies.len(), 1);
    assert_eq!(app_unit.local_dependencies[0].dependency_name, "util");
    assert_eq!(app_unit.external_dependencies.len(), 1);
    assert_eq!(app_unit.external_dependencies[0].dependency_name, "log");

    assert_eq!(app_package.build_local_dependencies.len(), 1);
    assert_eq!(
        app_package.build_local_dependencies[0].dependency_name,
        "util_build"
    );
    assert_eq!(app_package.build_external_dependencies.len(), 1);
    assert_eq!(
        app_package.build_external_dependencies[0].dependency_name,
        "cc"
    );
    assert_eq!(build_plan.local_dependency_edge_count(), 1);
    assert_eq!(build_plan.external_dependency_edge_count(), 1);
    assert_eq!(build_plan.build_local_dependency_edge_count(), 1);
    assert_eq!(build_plan.build_external_dependency_edge_count(), 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_dependencies_create_host_tool_instances() {
    let root = temp_dir("craft-build-plan-host-tools");
    let app_dir = root.join("app");
    let tool_dir = root.join("tool");
    fs::create_dir_all(&app_dir).unwrap();
    fs::create_dir_all(&tool_dir).unwrap();

    fs::write(
        root.join("Craft.toml"),
        r#"
[workspace]
members = ["app", "tool"]
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("Craft.toml"),
        r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "app"
root = "src/main.rn"

[build-dependencies]
codegen = { path = "../tool", package = "tool" }
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
b.define_string("tool_path", b.tool_path("codegen", "codegen"));
}
"#,
    )
    .unwrap();
    fs::write(
        tool_dir.join("Craft.toml"),
        r#"
[package]
name = "tool"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "codegen"
root = "src/main.rn"
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
        crate::script::ScriptCommand::Build,
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let host_tool_package = build_plan
        .packages
        .iter()
        .find(|package| {
            package.domain == crate::graph::BuildDomain::Host && package.package_id.name == "tool"
        })
        .unwrap();
    assert!(host_tool_package.units.iter().any(|unit| {
        unit.domain == crate::graph::BuildDomain::Host
            && unit.target_kind == TargetKind::Bin
            && unit.target_name.as_deref() == Some("codegen")
    }));

    let app_package = build_plan
        .packages
        .iter()
        .find(|package| {
            package.domain == crate::graph::BuildDomain::Target && package.package_id.name == "app"
        })
        .unwrap();
    let app_unit = app_package
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();
    let expected_tool_path = artifact_path(
        &build_plan.workspace_root,
        &crate::script::host_target(),
        crate::graph::BuildDomain::Host,
        &host_tool_package.package_id,
        &host_tool_package.units[0].profile.name,
        TargetKind::Bin,
        "codegen",
    )
    .to_string_lossy()
    .to_string();
    assert_eq!(
        app_unit.define.get("tool_path"),
        Some(&crate::plan::PlanValue::String(expected_tool_path))
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_tool_lookup_supports_explicit_named_tools() {
    let root = temp_dir("craft-build-plan-named-tools");
    let app_dir = root.join("app");
    let tool_dir = root.join("tool");
    fs::create_dir_all(&app_dir).unwrap();
    fs::create_dir_all(&tool_dir).unwrap();

    fs::write(
        root.join("Craft.toml"),
        r#"
[workspace]
members = ["app", "tool"]
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("Craft.toml"),
        r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "app"
root = "src/main.rn"

[build-dependencies]
tools = { path = "../tool", package = "tool" }
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
b.define_string("selected_tool", b.tool_path("tools", "beta"));
}
"#,
    )
    .unwrap();
    fs::write(
        tool_dir.join("Craft.toml"),
        r#"
[package]
name = "tool"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "alpha"
root = "src/alpha.rn"

[[bin]]
name = "beta"
root = "src/beta.rn"
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
        crate::script::ScriptCommand::Build,
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let host_tool_package = build_plan
        .packages
        .iter()
        .find(|package| {
            package.domain == crate::graph::BuildDomain::Host && package.package_id.name == "tool"
        })
        .unwrap();
    let expected_tool_path = artifact_path(
        &build_plan.workspace_root,
        &crate::script::host_target(),
        crate::graph::BuildDomain::Host,
        &host_tool_package.package_id,
        &host_tool_package.units[0].profile.name,
        TargetKind::Bin,
        "beta",
    )
    .to_string_lossy()
    .to_string();
    let app_unit = build_plan
        .packages
        .iter()
        .find(|package| {
            package.domain == crate::graph::BuildDomain::Target && package.package_id.name == "app"
        })
        .unwrap()
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();

    assert_eq!(
        app_unit.define.get("selected_tool"),
        Some(&crate::plan::PlanValue::String(expected_tool_path))
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_tool_lookup_supports_external_build_dependency_tools() {
    let root = temp_dir("craft-build-plan-external-tools");
    let tool_root = root.join("vendor").join("codegen");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(&tool_root).unwrap();

    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "app"
root = "src/main.rn"

[build-dependencies]
codegen = { path = "vendor/codegen", version = "1" }
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
b.define_string("selected_tool", b.tool_path("codegen", "codegen"));
}
"#,
    )
    .unwrap();
    fs::write(
        tool_root.join("Craft.toml"),
        r#"
[package]
name = "codegen"
version = "1"
kern = "0.6.7"

[[bin]]
name = "codegen"
root = "src/main.rn"
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
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();
    let expected_tool_path = artifact_path(
        &root
            .join(".craft")
            .join("sources")
            .join("path")
            .join("vendor_codegen")
            .join("codegen"),
        &crate::script::host_target(),
        crate::graph::BuildDomain::Target,
        &PackageId {
            name: "codegen".to_string(),
            version: "1".to_string(),
            source: crate::graph::SourceId::PathDependency {
                path: "vendor/codegen".to_string(),
            },
        },
        "dev",
        TargetKind::Bin,
        "codegen",
    )
    .to_string_lossy()
    .to_string();

    assert_eq!(
        unit.define.get("selected_tool"),
        Some(&crate::plan::PlanValue::String(expected_tool_path))
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn preserves_dependency_aliases_in_build_units() {
    let root = temp_dir("craft-build-plan-alias");
    let app_dir = root.join("app");
    let util_dir = root.join("util");
    fs::create_dir_all(&app_dir).unwrap();
    fs::create_dir_all(&util_dir).unwrap();

    fs::write(
        root.join("Craft.toml"),
        r#"
[workspace]
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
kern = "0.6.7"

[[bin]]
name = "app"
root = "src/main.rn"

[dependencies]
foo = { path = "../util", package = "util" }
"#,
    )
    .unwrap();
    fs::write(
        util_dir.join("Craft.toml"),
        r#"
[package]
name = "util"
version = "0.1.0"
kern = "0.6.7"

[lib]
root = "src/lib.rn"
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

    let app_unit = build_plan
        .packages
        .iter()
        .find(|package| package.package_id.name == "app")
        .unwrap()
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();

    assert_eq!(app_unit.local_dependencies.len(), 1);
    assert_eq!(app_unit.local_dependencies[0].dependency_name, "foo");
    assert_eq!(app_unit.local_dependencies[0].package_id.name, "util");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn applies_build_script_link_directives_per_unit() {
    let root = temp_dir("craft-build-plan-script");
    let os_variant = os_variant_name(crate::script::host_target().os);
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.7"

[features]
default = ["simd"]
simd = []

[[bin]]
name = "demo"
root = "src/main.rn"

[test]
roots = ["tests/smoke.rn"]
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        format!(
            r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {{
if (b.feature_enabled("simd")) {{
    b.link_arg("-flto");
}}

if (b.target.os == .{os_variant}) {{
    b.link_arg("-Dtarget-os-match");
}}

if (b.unit.kind == .bin) {{
    b.link_framework("Security");
}}

if (b.unit.kind == .test) {{
    b.link_search("native/test");
}}
}}
"#
        ),
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
    let test_elaboration = plan(
        &manifest_path,
        &manifest,
        &[],
        false,
        crate::script::ScriptCommand::Test,
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&build_elaboration, crate::script::ScriptCommand::Build).unwrap();
    let build_actions = build_plan.derive_actions(&crate::script::host_target());
    let build_package = build_plan
        .packages
        .iter()
        .find(|package| package.package_id.name == "demo")
        .unwrap();
    let test_plan = derive(&test_elaboration, crate::script::ScriptCommand::Test).unwrap();
    let test_actions = test_plan.derive_actions(&crate::script::host_target());
    let test_package = test_plan
        .packages
        .iter()
        .find(|package| package.package_id.name == "demo")
        .unwrap();
    assert_eq!(
        build_package
            .build_script
            .as_ref()
            .map(|script| script.relative_path.as_str()),
        Some("build.rn")
    );

    let bin = build_package
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();
    assert!(bin.link.args.iter().any(|arg| arg == "-flto"));
    assert!(bin.link.args.iter().any(|arg| arg == "-Dtarget-os-match"));
    assert!(bin.link.frameworks.iter().any(|name| name == "Security"));

    let test = test_package
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Test)
        .unwrap();
    assert!(test.link.args.iter().any(|arg| arg == "-flto"));
    assert!(
        test.link
            .search_paths
            .iter()
            .any(|path| path == "native/test")
    );
    let bin_action = build_actions
        .link_actions
        .iter()
        .find(|action| action.package_id.name == "demo" && action.target_kind == TargetKind::Bin)
        .unwrap();
    assert!(
        bin_action
            .link
            .frameworks
            .iter()
            .any(|name| name == "Security")
    );
    assert!(bin_action.link.args.iter().any(|arg| arg == "-flto"));
    let test_action = test_actions
        .link_actions
        .iter()
        .find(|action| action.package_id.name == "demo" && action.target_kind == TargetKind::Test)
        .unwrap();
    assert!(
        test_action
            .link
            .search_paths
            .iter()
            .any(|path| path == "native/test")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_can_generate_sources_and_mutate_unit_cfg_define() {
    let root = temp_dir("craft-build-plan-generated");
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "demo"
root = "src/placeholder.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let root = b.emit_generated(
    "src/main.rn",
    "fn main() i32 { return 0; }\n"
);
b.set_source_root(root);
b.cfg_bool("generated", true);
b.define_string("entry", "generated");
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
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();
    let unit_nodes = build_plan.compile_input_nodes_for_unit(unit);

    let SourceRootBinding::AbsolutePath(source_root) = &unit.source_root else {
        panic!("expected generated source root to be an absolute path binding");
    };
    assert!(
        Path::new(source_root).is_absolute(),
        "expected generated source root to be absolute: {}",
        source_root
    );
    assert!(!Path::new(source_root).exists());
    assert_eq!(
        unit.cfg.get("generated"),
        Some(&crate::plan::PlanValue::Bool(true))
    );
    assert_eq!(
        unit.define.get("entry"),
        Some(&crate::plan::PlanValue::String("generated".to_string()))
    );
    assert_eq!(unit.generated_files.len(), 1);
    assert_eq!(unit.generated_files[0].origin, GeneratedFileOrigin::Emitted);
    assert_eq!(unit_nodes.len(), 1);
    assert!(matches!(
        &unit_nodes[0].kind,
        StagedActionKind::WriteFile { .. }
    ));
    assert_eq!(unit_nodes[0].phase, StagedActionPhase::PreCompile);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_can_copy_package_files_into_generated_root() {
    let root = temp_dir("craft-build-plan-copy");
    fs::create_dir_all(root.join("templates")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "demo"
root = "src/placeholder.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("templates").join("main.rn"),
        "fn main() i32 { return 0; }\n",
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let root = b.copy_package_file("templates/main.rn", "src/main.rn");
b.set_source_root(root);
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
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();
    let unit_nodes = build_plan.compile_input_nodes_for_unit(unit);

    assert_eq!(unit.generated_files.len(), 1);
    assert_eq!(
        unit.generated_files[0].origin,
        GeneratedFileOrigin::Copied {
            source: "templates/main.rn".to_string()
        }
    );
    assert_eq!(unit_nodes.len(), 1);
    assert!(matches!(
        &unit_nodes[0].kind,
        StagedActionKind::CopyFile { source } if source == "templates/main.rn"
    ));
    assert_eq!(unit_nodes[0].phase, StagedActionPhase::PreCompile);
    let SourceRootBinding::AbsolutePath(source_root) = &unit.source_root else {
        panic!("expected copied generated source root to be an absolute path binding");
    };
    assert!(!Path::new(source_root).exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_can_model_explicit_staged_dependencies() {
    let root = temp_dir("craft-build-plan-staged-deps");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "demo"
root = "src/placeholder.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let helper = b.stage_generated("tmp/value.txt", "41\n");
let source = b.stage_generated("src/main.rn", "fn main() i32 { return 0; }\n");
b.depend(source, helper);
b.set_source_root_from(source);
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
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();
    let unit_nodes = build_plan.compile_input_nodes_for_unit(unit);

    assert_eq!(unit_nodes.len(), 2);
    let helper = unit_nodes
        .iter()
        .find(|action| action.output.ends_with("tmp/value.txt"))
        .unwrap();
    let source = unit_nodes
        .iter()
        .find(|action| action.output.ends_with("src/main.rn"))
        .unwrap();
    assert_eq!(source.depends_on, vec![helper.id]);
    assert!(matches!(
        &unit.source_root,
        SourceRootBinding::BuildOutput { id, path }
            if *id == source.id && path.replace('\\', "/").ends_with("src/main.rn")
    ));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_can_stage_post_link_artifact_outputs() {
    let root = temp_dir("craft-build-plan-post-link");
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("assets").join("config.json"),
        "{ \"mode\": \"demo\" }\n",
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let _ = b.copy_package_file_to_artifact("assets/config.json", "config/config.json");
let _ = b.emit_artifact_file("notes/build.txt", "built by craft\n");
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
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();
    let link_action = action_plan
        .link_actions
        .iter()
        .find(|action| action.package_id.name == "demo" && action.target_kind == TargetKind::Bin)
        .unwrap();
    let unit_nodes = build_plan.artifact_output_nodes_for_unit(unit);
    let link_nodes = action_plan.artifact_output_nodes_for_link_action(link_action);

    assert_eq!(unit_nodes.len(), 2);
    assert!(
        unit_nodes
            .iter()
            .all(|action| action.phase == StagedActionPhase::PostLink)
    );
    assert_eq!(link_nodes.len(), 2);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_can_stage_post_link_directory_copies() {
    let root = temp_dir("craft-build-plan-post-link-dir");
    fs::create_dir_all(root.join("assets").join("images")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("assets").join("config.json"),
        "{ \"mode\": \"demo\" }\n",
    )
    .unwrap();
    fs::write(
        root.join("assets").join("images").join("logo.txt"),
        "logo\n",
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let _ = b.copy_package_dir_to_artifact("assets", "bundle/assets");
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
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();
    let link_action = action_plan
        .link_actions
        .iter()
        .find(|action| action.package_id.name == "demo" && action.target_kind == TargetKind::Bin)
        .unwrap();
    let unit_nodes = build_plan.artifact_output_nodes_for_unit(unit);
    let link_nodes = action_plan.artifact_output_nodes_for_link_action(link_action);

    assert_eq!(unit_nodes.len(), 1);
    assert!(matches!(
        &unit_nodes[0].kind,
        StagedActionKind::CopyDirectory { source } if source == "assets"
    ));
    assert_eq!(unit_nodes[0].phase, StagedActionPhase::PostLink);
    assert_eq!(link_nodes.len(), 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_receives_host_target_and_domain_context() {
    let root = temp_dir("craft-build-plan-domain-context");
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
b.define_string("host_arch", b.host.arch);
b.define_string("target_arch", b.target.arch);

match (b.unit.domain) {
    .host => b.link_arg("-host-unit"),
    .target => b.link_arg("-target-unit"),
}
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
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();

    assert_eq!(unit.domain, crate::graph::BuildDomain::Target);
    assert_eq!(
        unit.define.get("host_arch"),
        Some(&crate::plan::PlanValue::String(
            crate::script::host_target().arch.to_string()
        ))
    );
    assert_eq!(
        unit.define.get("target_arch"),
        Some(&crate::plan::PlanValue::String(
            crate::script::host_target().arch.to_string()
        ))
    );
    assert!(unit.link.args.iter().any(|arg| arg == "-target-unit"));

    let _ = fs::remove_dir_all(root);
}
