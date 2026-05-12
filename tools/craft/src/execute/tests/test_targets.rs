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
kern = "0.7.5"

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
fn executes_all_test_units_before_reporting_failures() {
    let root = temp_dir("craft-exec-test-aggregate-failures");
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.5"

[test]
roots = ["tests/alpha.rn", "tests/beta.rn", "tests/gamma.rn"]
"#,
    )
    .unwrap();
    fs::write(
        root.join("tests/alpha.rn"),
        r#"
fn main() i32 {
return 1;
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("tests/beta.rn"),
        r#"
fn main() i32 {
return 0;
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("tests/gamma.rn"),
        r#"
fn main() i32 {
return 3;
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

    assert_eq!(summary.executed, 3);
    assert_eq!(summary.failures.len(), 2);
    assert!(
        summary
            .failures
            .iter()
            .any(|failure| failure.label.contains("alpha"))
    );
    assert!(
        summary
            .failures
            .iter()
            .any(|failure| failure.label.contains("gamma"))
    );

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
kern = "0.7.5"

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
kern = "0.7.5"

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
kern = "0.7.5"

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
kern = "0.7.5"

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
use base.mem.alloc.{Allocator, gpa};
use std.mem.Page;

fn main() i32 {
let mut page = Page.{};
let mut gpa = gpa().on(&mut Allocator.{ page..& });
defer gpa..&.deinit();
let alloc = &mut Allocator.{ gpa..& };

let found = match ("fixtures/ok.txt".path().exists(alloc)) {
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
kern = "0.7.5"

[test]
roots = ["tests/env.rn"]
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("tests/env.rn"),
        r#"
use std.env;
use base.mem.alloc.{Allocator, gpa};
use std.mem.Page;

fn main() i32 {
let mut page = Page.{};
let mut gpa = gpa().on(&mut Allocator.{ page..& });
defer gpa..&.deinit();
let alloc = &mut Allocator.{ gpa..& };

let mut workspace_root = match ("CRAFT_WORKSPACE_ROOT".env().get(alloc)) {
    .{ Ok: .{ Some: value } } => value,
    .{ Ok: .None } => return 1,
    .{ Err: _ } => return 5,
};
defer workspace_root..&.deinit(alloc);

let mut package_root = match ("CRAFT_PACKAGE_ROOT".env().get(alloc)) {
    .{ Ok: .{ Some: value } } => value,
    .{ Ok: .None } => return 2,
    .{ Err: _ } => return 6,
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

#[test]
fn test_targets_receive_name_and_temporary_directory_env() {
    let root = temp_dir("craft-exec-test-env");
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.5"

[test]
roots = ["tests/env.rn"]
"#,
    )
    .unwrap();
    fs::write(
        root.join("tests/env.rn"),
        r#"
use std.env;
use std.fs;
use base.mem.alloc.{Allocator, gpa};
use std.mem.Page;

fn main() i32 {
let mut page = Page.{};
let mut gpa = gpa().on(&mut Allocator.{ page..& });
defer gpa..&.deinit();
let alloc = &mut Allocator.{ gpa..& };

let mut name = match ("CRAFT_TEST_NAME".env().get(alloc)) {
    .{ Ok: .{ Some: value } } => value,
    .{ Ok: .None } => return 1,
    .{ Err: _ } => return 2,
};
defer name..&.deinit(alloc);
if (name.& != "env") {
    return 3;
}

let mut tmp = match ("CRAFT_TEST_TMPDIR".env().get(alloc)) {
    .{ Ok: .{ Some: value } } => value,
    .{ Ok: .None } => return 4,
    .{ Err: _ } => return 5,
};
defer tmp..&.deinit(alloc);
let is_dir = match (tmp.&.path().is_dir(alloc)) {
    .{ Ok: value } => value,
    .{ Err: _ } => return 6,
};
if (!is_dir) {
    return 7;
}

let mut marker = match (fs.join(alloc, tmp.&.as_str(), "marker.txt")) {
    .{ Ok: value } => value,
    .{ Err: _ } => return 8,
};
defer marker..&.deinit(alloc);
let wrote = match (marker.&.as_str().path().write_all(alloc, "ok")) {
    .{ Ok: value } => value,
    .{ Err: _ } => return 9,
};
if (wrote != 2) {
    return 10;
}
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
