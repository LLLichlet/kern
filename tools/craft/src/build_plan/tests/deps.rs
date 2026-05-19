//! Build-plan tests for local, external, and build-tool dependencies.

use super::*;

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
kern = "0.7.6"

[[bin]]
name = "app"
root = "src/main.kn"

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
kern = "0.7.6"

[lib]
root = "src/lib.kn"
"#,
    )
    .unwrap();
    fs::write(
        log_dir.join("Craft.toml"),
        r#"
[package]
name = "log"
version = "1"
kern = "0.7.6"

[lib]
root = "src/lib.kn"
"#,
    )
    .unwrap();
    fs::write(
        log_dir.join("src/lib.kn"),
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
kern = "0.7.6"

[[bin]]
name = "app"
root = "src/main.kn"

[dependencies]
util = { path = "../util" }
log = { path = "../vendor/log", version = "1" }

[build-dependencies]
util_build = { path = "../util", export = "util" }
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
kern = "0.7.6"

[lib]
root = "src/lib.kn"
"#,
    )
    .unwrap();
    fs::write(
        log_dir.join("Craft.toml"),
        r#"
[package]
name = "log"
version = "1"
kern = "0.7.6"

[lib]
root = "src/lib.kn"
"#,
    )
    .unwrap();
    fs::write(
        log_dir.join("src/lib.kn"),
        "pub fn answer() i32 { return 42; }\n",
    )
    .unwrap();
    fs::write(
        cc_dir.join("Craft.toml"),
        r#"
[package]
name = "cc"
version = "1"
kern = "0.7.6"

[lib]
root = "src/lib.kn"
"#,
    )
    .unwrap();
    fs::write(
        cc_dir.join("src/lib.kn"),
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
name = "workspace"
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
kern = "0.7.6"

[[bin]]
name = "app"
root = "src/main.kn"

[build-dependencies]
codegen = { path = "../tool", export = "tool" }
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("build.kn"),
        r#"
use craft.builder;

pub fn build(b: &mut builder.Builder) void {
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
kern = "0.7.6"

[[bin]]
name = "codegen"
root = "src/main.kn"
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
name = "workspace"
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
kern = "0.7.6"

[[bin]]
name = "app"
root = "src/main.kn"

[build-dependencies]
tools = { path = "../tool", export = "tool" }
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("build.kn"),
        r#"
use craft.builder;

pub fn build(b: &mut builder.Builder) void {
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
kern = "0.7.6"

[[bin]]
name = "alpha"
root = "src/alpha.kn"

[[bin]]
name = "beta"
root = "src/beta.kn"
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
kern = "0.7.6"

[[bin]]
name = "app"
root = "src/main.kn"

[build-dependencies]
codegen = { path = "vendor/codegen", version = "1" }
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.kn"),
        r#"
use craft.builder;

pub fn build(b: &mut builder.Builder) void {
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
kern = "0.7.6"

[[bin]]
name = "codegen"
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
kern = "0.7.6"

[[bin]]
name = "app"
root = "src/main.kn"

[dependencies]
foo = { path = "../util", export = "util" }
"#,
    )
    .unwrap();
    fs::write(
        util_dir.join("Craft.toml"),
        r#"
[package]
name = "util"
version = "0.1.0"
kern = "0.7.6"

[lib]
root = "src/lib.kn"
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
