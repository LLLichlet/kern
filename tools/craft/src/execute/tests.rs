use super::{build, parallel_target_link_jobs, run, test, validate_package_metadata_root};
use crate::build_plan;
use crate::elaborate::{FeatureSelection, plan};
use crate::manifest::Manifest;
use crate::workspace;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn release_build_dead_strips_unused_std_sections() {
    let root = temp_dir("craft-exec-dead-strip");
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "hello"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "hello"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/main.rn"),
        r#"
use std.io;

fn main() i32 {
    io.println("hello", .{});
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
        false,
        crate::script::ScriptCommand::Build,
        &FeatureSelection {
            profile: crate::script::ProfileSelection::Release,
            ..FeatureSelection::default()
        },
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());

    let summary = build(&build_plan, &action_plan).unwrap();
    assert_eq!(summary.link_actions, 1);

    let executable = action_plan
        .link_actions
        .iter()
        .find(|action| {
            action.package_id.name == "hello" && action.target_kind == crate::plan::TargetKind::Bin
        })
        .map(|action| action.artifact_path.clone())
        .expect("expected binary artifact path");
    assert!(
        executable.is_file(),
        "missing executable `{}`",
        executable.display()
    );

    let nm_output = Command::new("nm")
        .arg("--defined-only")
        .arg(&executable)
        .output()
        .expect("failed to run nm");
    assert!(
        nm_output.status.success(),
        "nm failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&nm_output.stdout),
        String::from_utf8_lossy(&nm_output.stderr)
    );

    let symbols = String::from_utf8_lossy(&nm_output.stdout);
    for unexpected in [
        "_K3std2fs4path9normalize",
        "_K3std2fs4file4copy",
        "_K3std3env9get_posix",
        "_K3std4time5linux11sleep_nanos",
    ] {
        assert!(
            !symbols.contains(unexpected),
            "unexpected unused std symbol `{unexpected}` survived release link:\n{symbols}"
        );
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn builds_and_runs_hosted_package_with_local_library_dependency() {
    let root = temp_dir("craft-exec-run");
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
util = { path = "../util" }
"#,
    )
    .unwrap();
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::write(
        app_dir.join("src/main.rn"),
        r#"
fn main() i32 {
if (util.answer() == 42) {
    return 0;
}
return 1;
}
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
    fs::create_dir_all(util_dir.join("src")).unwrap();
    fs::write(
        util_dir.join("src/lib.rn"),
        r#"
pub fn answer() i32 {
return 42;
}
"#,
    )
    .unwrap();

    let manifest_path = root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path).unwrap();
    let members = crate::workspace::load_members(&manifest_path, &manifest).unwrap();
    let elaboration = plan(
        &manifest_path,
        &manifest,
        &members,
        true,
        crate::script::ScriptCommand::Run,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let unit = build_plan
        .packages
        .iter()
        .find(|package| package.package_id.name == "app")
        .unwrap()
        .units
        .iter()
        .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
        .unwrap();

    let summary = run(&build_plan, &action_plan, unit).unwrap();
    assert!(summary.executable.is_file());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn builds_and_runs_hosted_package_with_renamed_local_library_dependency() {
    let root = temp_dir("craft-exec-run-alias");
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
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::write(
        app_dir.join("src/main.rn"),
        r#"
fn main() i32 {
if (foo.answer() == 42) {
    return 0;
}
return 1;
}
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
    fs::create_dir_all(util_dir.join("src")).unwrap();
    fs::write(
        util_dir.join("src/lib.rn"),
        r#"
pub fn answer() i32 {
return 42;
}
"#,
    )
    .unwrap();

    let manifest_path = root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path).unwrap();
    let members = crate::workspace::load_members(&manifest_path, &manifest).unwrap();
    let elaboration = plan(
        &manifest_path,
        &manifest,
        &members,
        true,
        crate::script::ScriptCommand::Run,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let unit = build_plan
        .packages
        .iter()
        .find(|package| package.package_id.name == "app")
        .unwrap()
        .units
        .iter()
        .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
        .unwrap();

    let summary = run(&build_plan, &action_plan, unit).unwrap();
    assert!(summary.executable.is_file());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn builds_hosted_package_when_dependency_emits_generic_std_instantiations() {
    let root = temp_dir("craft-exec-generic-std-linkage");
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
util = { path = "../util" }
"#,
    )
    .unwrap();
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::write(
        app_dir.join("src/main.rn"),
        r#"
fn main() i32 {
if (util.is_truthy("true")) {
    return 0;
}
return 1;
}
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
    fs::create_dir_all(util_dir.join("src")).unwrap();
    fs::write(
        util_dir.join("src/lib.rn"),
        r#"
pub fn is_truthy(value: []u8) bool {
return value.eq("true");
}
"#,
    )
    .unwrap();

    let manifest_path = root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path).unwrap();
    let members = crate::workspace::load_members(&manifest_path, &manifest).unwrap();
    let elaboration = plan(
        &manifest_path,
        &manifest,
        &members,
        true,
        crate::script::ScriptCommand::Build,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());

    let summary = build(&build_plan, &action_plan).unwrap();
    assert_eq!(summary.compile_actions, 2);
    assert_eq!(summary.link_actions, 1);

    let _ = fs::remove_dir_all(root);
}

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
kern = "0.6.7"

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
    let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(gitignore.contains(".craft/"));

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
kern = "0.6.7"

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
kern = "0.6.7"

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
kern = "0.6.7"

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

#[test]
fn builds_compile_and_link_actions() {
    let root = temp_dir("craft-exec-build");
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
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
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
        crate::script::ScriptCommand::Build,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());

    let summary = build(&build_plan, &action_plan).unwrap();
    assert_eq!(summary.compile_actions, 1);
    assert_eq!(summary.link_actions, 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn incremental_build_skips_unchanged_actions() {
    let root = temp_dir("craft-exec-incremental-skip");
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
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
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
        crate::script::ScriptCommand::Build,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());

    let first = build(&build_plan, &action_plan).unwrap();
    assert_eq!(first.compile_actions, 1);
    assert_eq!(first.link_actions, 1);
    assert!(first.action_cache_stats.compile_misses > 0);
    assert_eq!(first.action_cache_stats.link_misses, 1);
    assert_eq!(first.action_cache_stats.compile_hits, 0);
    assert_eq!(first.action_cache_stats.link_hits, 0);

    let second = build(&build_plan, &action_plan).unwrap();
    assert_eq!(second.compile_actions, 0);
    assert_eq!(second.link_actions, 0);
    assert_eq!(second.action_cache_stats.compile_misses, 0);
    assert_eq!(second.action_cache_stats.link_misses, 0);
    assert!(second.action_cache_stats.compile_hits > 0);
    assert_eq!(second.action_cache_stats.link_hits, 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn incremental_build_rebuilds_only_changed_workspace_actions() {
    let root = temp_dir("craft-exec-incremental-workspace");
    let app_dir = root.join("app");
    let util_dir = root.join("util");
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::create_dir_all(util_dir.join("src")).unwrap();

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
        app_dir.join("src/main.rn"),
        r#"
fn main() i32 {
return util.answer();
}
"#,
    )
    .unwrap();
    fs::write(
        util_dir.join("src/lib.rn"),
        r#"
pub fn answer() i32 {
return 41;
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
        crate::script::ScriptCommand::Build,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());

    let first = build(&build_plan, &action_plan).unwrap();
    assert_eq!(first.compile_actions, 2);
    assert_eq!(first.link_actions, 1);
    assert!(first.action_cache_stats.compile_misses > 0);
    assert_eq!(first.action_cache_stats.link_misses, 1);

    fs::write(
        app_dir.join("src/main.rn"),
        r#"
fn main() i32 {
return util.answer() + 1;
}
"#,
    )
    .unwrap();
    let app_changed = build(&build_plan, &action_plan).unwrap();
    assert_eq!(app_changed.compile_actions, 1);
    assert_eq!(app_changed.link_actions, 1);
    assert!(app_changed.action_cache_stats.compile_hits > 0);
    assert!(app_changed.action_cache_stats.compile_misses > 0);
    assert_eq!(app_changed.action_cache_stats.link_hits, 0);
    assert_eq!(app_changed.action_cache_stats.link_misses, 1);

    fs::write(
        util_dir.join("src/lib.rn"),
        r#"
pub fn answer() i32 {
return 42;
}
"#,
    )
    .unwrap();
    let dep_changed = build(&build_plan, &action_plan).unwrap();
    assert_eq!(dep_changed.compile_actions, 2);
    assert_eq!(dep_changed.link_actions, 1);
    assert!(dep_changed.action_cache_stats.compile_hits > 0);
    assert!(dep_changed.action_cache_stats.compile_misses > 0);
    assert_eq!(dep_changed.action_cache_stats.link_hits, 0);
    assert_eq!(dep_changed.action_cache_stats.link_misses, 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn incremental_build_rebuilds_when_runtime_manifest_options_change() {
    let root = temp_dir("craft-exec-incremental-runtime-manifest");
    fs::create_dir_all(root.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "hello"
version = "0.1.0"
kern = "0.6.7"

[runtime]
entry = "crt"
provider = "toolchain"
libc = true
bundle = "std"

[[bin]]
name = "hello"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();

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

    let first = build(&build_plan, &action_plan).unwrap();
    assert_eq!(first.compile_actions, 1);
    assert_eq!(first.link_actions, 1);
    assert!(first.action_cache_stats.compile_misses > 0);
    assert_eq!(first.action_cache_stats.link_misses, 1);

    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "hello"
version = "0.1.0"
kern = "0.6.7"

[runtime]
entry = "rt"
provider = "toolchain"
libc = true
bundle = "std"

[[bin]]
name = "hello"
root = "src/main.rn"
"#,
    )
    .unwrap();

    let rebuilt = build(&build_plan, &action_plan).unwrap();
    assert_eq!(rebuilt.compile_actions, 1);
    assert_eq!(rebuilt.link_actions, 1);
    assert!(rebuilt.action_cache_stats.compile_misses > 0);
    assert_eq!(rebuilt.action_cache_stats.link_misses, 1);
    assert_eq!(rebuilt.action_cache_stats.link_hits, 0);

    let cached = build(&build_plan, &action_plan).unwrap();
    assert_eq!(cached.compile_actions, 0);
    assert_eq!(cached.link_actions, 0);
    assert_eq!(cached.action_cache_stats.compile_misses, 0);
    assert_eq!(cached.action_cache_stats.link_misses, 0);
    assert!(cached.action_cache_stats.compile_hits > 0);
    assert_eq!(cached.action_cache_stats.link_hits, 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn parallel_target_link_jobs_skip_post_link_outputs() {
    let root = temp_dir("craft-exec-parallel-jobs");
    let plain_dir = root.join("plain");
    let staged_dir = root.join("staged");
    fs::create_dir_all(plain_dir.join("src")).unwrap();
    fs::create_dir_all(staged_dir.join("src")).unwrap();
    fs::create_dir_all(staged_dir.join("assets")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        r#"
[workspace]
members = ["plain", "staged"]
"#,
    )
    .unwrap();
    fs::write(
        plain_dir.join("Craft.toml"),
        r#"
[package]
name = "plain"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "plain"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        plain_dir.join("src/main.rn"),
        "fn main() i32 { return 0; }\n",
    )
    .unwrap();
    fs::write(
        staged_dir.join("Craft.toml"),
        r#"
[package]
name = "staged"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "staged"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        staged_dir.join("src/main.rn"),
        "fn main() i32 { return 0; }\n",
    )
    .unwrap();
    fs::write(staged_dir.join("assets/data.txt"), "data\n").unwrap();
    fs::write(
        staged_dir.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let _ = b.copy_package_file_to_artifact("assets/data.txt", "bundle/data.txt");
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
        crate::script::ScriptCommand::Build,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let compile_action_index = super::external::compile_actions_index(&action_plan.compile_actions);
    let jobs = parallel_target_link_jobs(&action_plan, &compile_action_index, &Default::default())
        .unwrap();

    assert_eq!(action_plan.link_actions.len(), 2);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].link_action.package_id.name, "plain");

    let _ = fs::remove_dir_all(root);
}

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
kern = "0.6.7"

[[bin]]
name = "hello"
root = "src/main.rn"
"#,
        )
        .unwrap();
        fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();

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
kern = "0.6.7"

[profile.release]
opt = {opt}

[[bin]]
name = "hello"
root = "src/main.rn"
"#
            ),
        )
        .unwrap();
        fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();

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
kern = "0.6.7"

[profile.release]
opt = 3
codegen-units = {codegen_units}

[[bin]]
name = "hello"
root = "src/main.rn"
"#
            ),
        )
        .unwrap();
        fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();

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
fn builds_package_with_direct_external_path_dependency() {
    let root = temp_dir("craft-exec-external-direct");
    let log_root = root.join("vendor").join("log");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(log_root.join("src")).unwrap();

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

[dependencies]
log = { path = "vendor/log", version = "1" }
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        r#"
fn main() i32 {
if (log.answer() == 42) {
    return 0;
}
return 1;
}
"#,
    )
    .unwrap();
    fs::write(
        log_root.join("Craft.toml"),
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
        log_root.join("src/lib.rn"),
        r#"
pub fn answer() i32 {
return 42;
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

    let summary = build(&build_plan, &action_plan).unwrap();
    assert_eq!(summary.compile_actions, 2);
    assert_eq!(summary.link_actions, 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn builds_package_with_direct_external_git_dependency_in_release_profile() {
    let root = temp_dir("craft-exec-external-git-release");
    let repo = root.join("log.git");
    fs::create_dir_all(root.join("src")).unwrap();
    init_git_package(
        &repo,
        r#"
[package]
name = "log"
version = "1"
kern = "0.6.7"

[lib]
root = "src/lib.rn"
"#,
        r#"
pub fn answer() i32 {
return 42;
}
"#,
    );

    fs::write(
        root.join("Craft.toml"),
        format!(
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "app"
root = "src/main.rn"

[dependencies]
log = {{ git = "{}", branch = "main", version = "1" }}
"#,
            toml_string_literal(&repo)
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        r#"
fn main() i32 {
if (log.answer() == 42) {
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
        crate::script::ScriptCommand::Build,
        &FeatureSelection {
            profile: crate::script::ProfileSelection::Release,
            ..Default::default()
        },
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());

    let summary = build(&build_plan, &action_plan).unwrap();
    assert_eq!(summary.compile_actions, 2);
    assert_eq!(summary.link_actions, 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn builds_and_runs_hosted_package_with_transitive_external_path_dependency() {
    let root = temp_dir("craft-exec-external-transitive");
    let log_root = root.join("vendor").join("log");
    let corelog_root = log_root.join("vendor").join("corelog");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(log_root.join("src")).unwrap();
    fs::create_dir_all(corelog_root.join("src")).unwrap();

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

[dependencies]
log = { path = "vendor/log", version = "1" }
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        r#"
fn main() i32 {
if (log.answer() == 42) {
    return 0;
}
return 1;
}
"#,
    )
    .unwrap();
    fs::write(
        log_root.join("Craft.toml"),
        r#"
[package]
name = "log"
version = "1"
kern = "0.6.7"

[lib]
root = "src/lib.rn"

[dependencies]
corelog = { path = "vendor/corelog", version = "1" }
"#,
    )
    .unwrap();
    fs::write(
        log_root.join("src/lib.rn"),
        r#"
pub fn answer() i32 {
return corelog.base() + 1;
}
"#,
    )
    .unwrap();
    fs::write(
        corelog_root.join("Craft.toml"),
        r#"
[package]
name = "corelog"
version = "1"
kern = "0.6.7"

[lib]
root = "src/lib.rn"
"#,
    )
    .unwrap();
    fs::write(
        corelog_root.join("src/lib.rn"),
        r#"
pub fn base() i32 {
return 41;
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
        crate::script::ScriptCommand::Run,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
        .unwrap();

    let summary = run(&build_plan, &action_plan, unit).unwrap();
    assert!(summary.executable.is_file());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn builds_and_runs_external_package_with_nested_path_dependency() {
    let root = temp_dir("craft-exec-external-package-local-source");
    let log_root = root.join("vendor").join("log");
    let corelog_root = log_root.join("vendor-nested").join("corelog");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(log_root.join("src")).unwrap();
    fs::create_dir_all(corelog_root.join("src")).unwrap();

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

[dependencies]
log = { path = "vendor/log", version = "1" }
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        r#"
fn main() i32 {
if (log.answer() == 42) {
    return 0;
}
return 1;
}
"#,
    )
    .unwrap();
    fs::write(
        log_root.join("Craft.toml"),
        r#"
[package]
name = "log"
version = "1"
kern = "0.6.7"

[lib]
root = "src/lib.rn"

[dependencies]
corelog = { path = "vendor-nested/corelog", version = "1" }
"#,
    )
    .unwrap();
    fs::write(
        log_root.join("src/lib.rn"),
        r#"
pub fn answer() i32 {
return corelog.base() + 1;
}
"#,
    )
    .unwrap();
    fs::write(
        corelog_root.join("Craft.toml"),
        r#"
[package]
name = "corelog"
version = "1"
kern = "0.6.7"

[lib]
root = "src/lib.rn"
"#,
    )
    .unwrap();
    fs::write(
        corelog_root.join("src/lib.rn"),
        r#"
pub fn base() i32 {
return 41;
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
        crate::script::ScriptCommand::Run,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
        .unwrap();

    let summary = run(&build_plan, &action_plan, unit).unwrap();
    assert!(summary.executable.is_file());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn builds_and_runs_hosted_package_with_generated_source_from_build_script() {
    let root = temp_dir("craft-exec-generated-source");
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
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let path = b.emit_generated(
    "src/main.rn",
    "fn main() i32 { return 0; }\n"
);
b.set_source_root(path);
b.define_bool("generated", true);
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
        crate::script::ScriptCommand::Run,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
        .unwrap();

    let summary = run(&build_plan, &action_plan, unit).unwrap();
    assert!(summary.executable.is_file());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn builds_and_runs_hosted_package_with_copied_generated_source_from_build_script() {
    let root = temp_dir("craft-exec-copied-source");
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
root = "src/main.rn"
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
let path = b.copy_package_file("templates/main.rn", "src/main.rn");
b.set_source_root(path);
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
        crate::script::ScriptCommand::Run,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
        .unwrap();

    let summary = run(&build_plan, &action_plan, unit).unwrap();
    assert!(summary.executable.is_file());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn builds_and_runs_hosted_package_with_post_link_artifact_stage_outputs() {
    let root = temp_dir("craft-exec-post-link-stage");
    fs::create_dir_all(root.join("src")).unwrap();
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
        root.join("src").join("main.rn"),
        "fn main() i32 { return 0; }\n",
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
        crate::script::ScriptCommand::Run,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
        .unwrap();
    let link_action = action_plan
        .link_actions
        .iter()
        .find(|action| {
            action.package_id.name == "demo" && action.target_kind == crate::plan::TargetKind::Bin
        })
        .unwrap();
    let link_nodes = action_plan.artifact_output_nodes_for_link_action(link_action);

    let summary = run(&build_plan, &action_plan, unit).unwrap();
    assert!(summary.executable.is_file());
    assert!(Path::new(&link_nodes[0].output).exists());
    assert!(Path::new(&link_nodes[1].output).exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn builds_and_runs_hosted_package_with_post_link_directory_stage_outputs() {
    let root = temp_dir("craft-exec-post-link-dir");
    fs::create_dir_all(root.join("src")).unwrap();
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
        root.join("src").join("main.rn"),
        "fn main() i32 { return 0; }\n",
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
        crate::script::ScriptCommand::Run,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
        .unwrap();
    let link_action = action_plan
        .link_actions
        .iter()
        .find(|action| {
            action.package_id.name == "demo" && action.target_kind == crate::plan::TargetKind::Bin
        })
        .unwrap();
    let link_nodes = action_plan.artifact_output_nodes_for_link_action(link_action);

    let summary = run(&build_plan, &action_plan, unit).unwrap();
    assert!(summary.executable.is_file());
    assert!(
        Path::new(&link_nodes[0].output)
            .join("config.json")
            .exists()
    );
    assert!(
        Path::new(&link_nodes[0].output)
            .join("images")
            .join("logo.txt")
            .exists()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn builds_and_runs_hosted_package_with_generated_source_from_host_tool() {
    let root = temp_dir("craft-exec-host-tool-generated");
    let app_dir = root.join("app");
    let tool_dir = root.join("tool");
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::create_dir_all(tool_dir.join("src")).unwrap();
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
root = "src/placeholder.rn"

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
let generated = b.emit_generated_from_tool("codegen", "codegen", "src/main.rn", .{});
b.set_source_root(generated);
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
    fs::write(
        tool_dir.join("src").join("main.rn"),
        r#"
use std.io;
use std.io.Writer;

fn main() i32 {
let mut out = io.stdout();
let writer = *mut Writer.{ out..& };
let _ = writer.write("fn main() i32 { return 0; }\n");
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
        crate::script::ScriptCommand::Run,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan =
        crate::build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let unit = build_plan
        .packages
        .iter()
        .find(|package| {
            package.domain == crate::graph::BuildDomain::Target && package.package_id.name == "app"
        })
        .unwrap()
        .units
        .iter()
        .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
        .unwrap();

    let summary = run(&build_plan, &action_plan, unit).unwrap();
    assert!(summary.executable.is_file());
    let crate::build_plan::SourceRootBinding::AbsolutePath(source_root) = &unit.source_root else {
        panic!("expected generated source root to be an absolute path binding");
    };
    assert!(Path::new(source_root).is_file());
    let generated = fs::read_to_string(source_root).unwrap();
    assert!(generated.contains("fn main() i32"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn builds_and_runs_hosted_package_with_explicit_staged_dependencies() {
    let root = temp_dir("craft-exec-staged-dependencies");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "app"
root = "src/placeholder.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let helper = b.stage_generated("tmp/main.template.rn", "fn main() i32 { return 0; }\n");
let source = b.stage_copy_output(helper, "src/main.rn");
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
        crate::script::ScriptCommand::Run,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan =
        crate::build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
        .unwrap();

    let summary = run(&build_plan, &action_plan, unit).unwrap();
    assert!(summary.executable.is_file());
    let crate::build_plan::SourceRootBinding::BuildOutput { path, .. } = &unit.source_root else {
        panic!("expected staged generated source root to bind to a build output");
    };
    assert!(Path::new(path).is_file());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn builds_and_runs_hosted_package_with_generated_source_from_external_host_tool() {
    let root = temp_dir("craft-exec-external-host-tool-generated");
    let tool_root = root.join("vendor").join("codegen");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(tool_root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "app"
root = "src/placeholder.rn"

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
let generated = b.emit_generated_from_tool("codegen", "codegen", "src/main.rn", .{});
b.set_source_root(generated);
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
    fs::write(
        tool_root.join("src").join("main.rn"),
        r#"
use std.io;
use std.io.Writer;

fn main() i32 {
let mut out = io.stdout();
let writer = *mut Writer.{ out..& };
let _ = writer.write("fn main() i32 { return 0; }\n");
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
        crate::script::ScriptCommand::Run,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan =
        crate::build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
        .unwrap();

    let summary = run(&build_plan, &action_plan, unit).unwrap();
    assert!(summary.executable.is_file());
    let crate::build_plan::SourceRootBinding::AbsolutePath(source_root) = &unit.source_root else {
        panic!("expected generated source root to be an absolute path binding");
    };
    assert!(Path::new(source_root).is_file());
    let generated = fs::read_to_string(source_root).unwrap();
    assert!(generated.contains("fn main() i32"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn rejects_kmeta_package_with_mismatched_declared_identity() {
    let root = temp_dir("craft-exec-kmeta-identity");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("Kmeta.toml"),
        r#"
format_version = 2
kind = "source_snapshot"
package_name = "other"
package_version = "2.0.0"
root_module_name = "other"
entry_module_path = "src/init.rn"
"#,
    )
    .unwrap();

    let err = validate_package_metadata_root(&root, "util", Some("1.0.0")).unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("declares package `other` but `util` was required"),
        "unexpected error: {message}"
    );

    let _ = fs::remove_dir_all(root);
}

fn init_git_package(repo: &Path, manifest: &str, lib_source: &str) {
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(repo.join("Craft.toml"), manifest).unwrap();
    fs::write(repo.join("src/lib.rn"), lib_source).unwrap();
    run_git(repo, ["init", "--initial-branch=main"]);
    run_git(repo, ["config", "user.name", "Craft Tests"]);
    run_git(
        repo,
        ["config", "user.email", "craft-tests@example.invalid"],
    );
    run_git(repo, ["add", "."]);
    run_git(repo, ["commit", "-m", "initial"]);
}

fn toml_string_literal(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\")
}

fn run_git<const N: usize>(cwd: &Path, args: [&str; N]) {
    let output = Command::new("git")
        .args(["-c", "commit.gpgsign=false"])
        .args(["-c", "tag.gpgSign=false"])
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
