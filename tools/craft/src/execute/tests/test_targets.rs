use super::*;

#[test]
fn builds_and_executes_test_units() {
    let root = temp_dir("craft-exec-test");
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.0"

[test]
roots = ["tests/smoke.rn"]
"#,
    )
    .unwrap();
    fs::write(
        root.join("tests/smoke.rn"),
        r#"
fn main() i32 {
return 0;
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
        crate::script::ScriptCommand::Test,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Test).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let test_units = build_plan.packages[0]
        .units
        .iter()
        .filter(|unit| unit.target_kind == crate::plan::TargetKind::Test)
        .collect::<Vec<_>>();

    let summary = test(&build_plan, &action_plan, &test_units).unwrap();
    assert_eq!(summary.executed, 1);
    assert!(!root.join(".gitignore").exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn builds_and_executes_release_thinlto_test_units() {
    let root = temp_dir("craft-exec-test-release-thin");
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.0"

[profile.release]
opt = 3
codegen-units = 2

[test]
roots = ["tests/smoke.rn"]
"#,
    )
    .unwrap();
    fs::write(
        root.join("tests/smoke.rn"),
        r#"
fn main() i32 {
return 0;
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
        crate::script::ScriptCommand::Test,
        &FeatureSelection {
            profile: crate::script::ProfileSelection::Release,
            ..Default::default()
        },
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Test).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let test_units = build_plan.packages[0]
        .units
        .iter()
        .filter(|unit| unit.target_kind == crate::plan::TargetKind::Test)
        .collect::<Vec<_>>();

    let summary = test(&build_plan, &action_plan, &test_units).unwrap();
    assert_eq!(summary.executed, 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn tests_can_import_their_own_package_library() {
    let root = temp_dir("craft-exec-test-self-lib");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.0"

[lib]
root = "src/lib.rn"

[test]
roots = ["tests/smoke.rn"]
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/lib.rn"),
        r#"
pub fn answer() i32 {
return 42;
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("tests/smoke.rn"),
        r#"
use demo.answer;

fn main() i32 {
if (answer() == 42) {
    return 0;
}
return 1;
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
        crate::script::ScriptCommand::Test,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Test).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let test_units = build_plan.packages[0]
        .units
        .iter()
        .filter(|unit| unit.target_kind == crate::plan::TargetKind::Test)
        .collect::<Vec<_>>();

    let summary = test(&build_plan, &action_plan, &test_units).unwrap();
    assert_eq!(summary.executed, 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn package_runtime_applies_to_test_targets() {
    let root = temp_dir("craft-exec-test-runtime-defaults");
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.0"

[runtime]
entry = "rt"
libc = false
bundle = "base"

[test]
roots = ["tests/smoke.rn"]
"#,
    )
    .unwrap();
    fs::write(
        root.join("tests/smoke.rn"),
        r#"
fn main() i32 {
return 0;
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
        crate::script::ScriptCommand::Test,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Test).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let test_units = build_plan.packages[0]
        .units
        .iter()
        .filter(|unit| unit.target_kind == crate::plan::TargetKind::Test)
        .collect::<Vec<_>>();

    let summary = test(&build_plan, &action_plan, &test_units).unwrap();
    assert_eq!(summary.executed, 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn workspace_member_tests_run_from_member_package_root() {
    let root = temp_dir("craft-exec-test-member-cwd");
    let app_dir = root.join("app");
    fs::create_dir_all(app_dir.join("tests")).unwrap();
    fs::create_dir_all(app_dir.join("fixtures")).unwrap();
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
kern = "0.7.0"

[test]
roots = ["tests/cwd.rn"]
"#,
    )
    .unwrap();
    fs::write(app_dir.join("fixtures/ok.txt"), "ok\n").unwrap();
    fs::write(
        app_dir.join("tests/cwd.rn"),
        r#"
use std.fs;
use base.mem.alloc.{Allocator, GPA};
use sys.mem.Page;

fn main() i32 {
let mut page = Page.{};
let mut gpa = GPA.{ backing: *mut Allocator.{ page..& } };
defer gpa..&.deinit();
let alloc = *mut Allocator.{ gpa..& };

let found = match (fs.exists(alloc, "fixtures/ok.txt")) {
    .{ Ok: value } => value,
    .{ Err: _ } => false,
};
if (found) {
    return 0;
}
return 1;
}
"#,
    )
    .unwrap();

    let manifest_path = root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path).unwrap();
    let members = workspace::load_members(&manifest_path, &manifest).unwrap();
    let elaboration = plan(
        &manifest_path,
        &manifest,
        &members,
        true,
        crate::script::ScriptCommand::Test,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Test).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let test_units = build_plan
        .packages
        .iter()
        .find(|package| package.package_id.name == "app")
        .unwrap()
        .units
        .iter()
        .filter(|unit| unit.target_kind == crate::plan::TargetKind::Test)
        .collect::<Vec<_>>();

    let summary = test(&build_plan, &action_plan, &test_units).unwrap();
    assert_eq!(summary.executed, 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn workspace_member_tests_receive_package_and_workspace_root_env() {
    let root = temp_dir("craft-exec-test-member-env");
    let app_dir = root.join("app");
    fs::create_dir_all(app_dir.join("tests")).unwrap();
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
kern = "0.7.0"

[test]
roots = ["tests/env.rn"]
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("tests/env.rn"),
        r#"
use std.env;
use base.mem.alloc.{Allocator, GPA};
use sys.mem.Page;

fn main() i32 {
let mut page = Page.{};
let mut gpa = GPA.{ backing: *mut Allocator.{ page..& } };
defer gpa..&.deinit();
let alloc = *mut Allocator.{ gpa..& };

let mut workspace_root = match (env.get(alloc, "CRAFT_WORKSPACE_ROOT")) {
    .{ Some: value } => value,
    .None => return 1,
};
defer workspace_root..&.deinit(alloc);

let mut package_root = match (env.get(alloc, "CRAFT_PACKAGE_ROOT")) {
    .{ Some: value } => value,
    .None => return 2,
};
defer package_root..&.deinit(alloc);

if (!package_root.&.ends_with("/app") and !package_root.&.ends_with("\\app")) {
    return 3;
}
if (workspace_root.&.eq(package_root.&.as_str())) {
    return 4;
}
return 0;
}
"#,
    )
    .unwrap();

    let manifest_path = root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path).unwrap();
    let members = workspace::load_members(&manifest_path, &manifest).unwrap();
    let elaboration = plan(
        &manifest_path,
        &manifest,
        &members,
        true,
        crate::script::ScriptCommand::Test,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Test).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let test_units = build_plan
        .packages
        .iter()
        .find(|package| package.package_id.name == "app")
        .unwrap()
        .units
        .iter()
        .filter(|unit| unit.target_kind == crate::plan::TargetKind::Test)
        .collect::<Vec<_>>();

    let summary = test(&build_plan, &action_plan, &test_units).unwrap();
    assert_eq!(summary.executed, 1);

    let _ = fs::remove_dir_all(root);
}
