//! Execution tests for local package dependencies and runtime behavior.

use super::*;

#[test]
fn release_build_dead_strips_unused_std_sections() {
    let root = temp_dir("craft-exec-dead-strip");
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "hello"
version = "0.1.0"
kern = "0.7.8"

[[bin]]
name = "hello"
root = "src/main.kn"
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/main.kn"),
        r#"
use std.io;

fn main() i32 {
    "hello".println();
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

    let nm_output = Command::new(symbol_dump_tool())
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
kern = "0.7.8"

[[bin]]
name = "app"
root = "src/main.kn"

[dependencies]
util = { path = "../util" }
"#,
    )
    .unwrap();
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::write(
        app_dir.join("src/main.kn"),
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
kern = "0.7.8"

[lib]
root = "src/lib.kn"
"#,
    )
    .unwrap();
    fs::create_dir_all(util_dir.join("src")).unwrap();
    fs::write(
        util_dir.join("src/lib.kn"),
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
    validate_package_metadata_root(
        &root
            .join(".craft")
            .join("build")
            .join("dev")
            .join("target")
            .join("meta")
            .join("util-0.1.0"),
        "util",
        Some("0.1.0"),
    )
    .unwrap();

    let _ = fs::remove_dir_all(root);
}

#[test]
fn builds_local_dependency_public_aggregate_const_without_link_symbol() {
    let root = temp_dir("craft-exec-public-aggregate-const");
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
kern = "0.7.8"

[[bin]]
name = "app"
root = "src/main.kn"

[dependencies]
util = { path = "../util" }
"#,
    )
    .unwrap();
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::write(
        app_dir.join("src/main.kn"),
        r#"
fn main() i32 {
    let c = util.WHITE;
    if (c.r == 255) {
        if (c.g == 255) {
            if (c.b == 255) {
                if (c.a == 255) {
                    return 0;
                }
            }
        }
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
kern = "0.7.8"

[lib]
root = "src/lib.kn"
"#,
    )
    .unwrap();
    fs::create_dir_all(util_dir.join("src")).unwrap();
    fs::write(
        util_dir.join("src/lib.kn"),
        r#"
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
};

pub const WHITE = Color.{ r: 255, g: 255, b: 255, a: 255 };
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
    assert_eq!(summary.link_actions, 1);

    let util_object = action_plan
        .compile_actions
        .iter()
        .find(|action| {
            action.package_id.name == "util" && action.target_kind == crate::plan::TargetKind::Lib
        })
        .map(|action| action.object_path.clone())
        .expect("expected util library object path");
    let nm_output = Command::new(symbol_dump_tool())
        .arg("--defined-only")
        .arg(&util_object)
        .output()
        .expect("failed to run nm");
    assert!(
        nm_output.status.success(),
        "nm failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&nm_output.stdout),
        String::from_utf8_lossy(&nm_output.stderr)
    );

    let symbols = String::from_utf8_lossy(&nm_output.stdout);
    assert!(
        !symbols.contains("_K4util5WHITE"),
        "public const lowered to a linkable global symbol:\n{symbols}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn builds_and_runs_package_with_resource_c_source() {
    let root = temp_dir("craft-exec-resource-cc");
    fs::create_dir_all(root.join("vendor/native/src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.8"

[[bin]]
name = "app"
root = "src/main.kn"

[resources]
native = { path = "vendor/native" }
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.kn"),
        r#"
use craft.builder;

pub fn build(b: &mut builder.Builder) void {
    let obj = b.cc_resource_config("native", "src/add.c", .{
        include_dirs: .{"src"},
        defines: .{},
        args: .{},
        dependencies: .{},
    });
    let _ = obj;
}
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/main.kn"),
        r#"
extern {
    fn native_add7() i32;
}

fn main() i32 {
    if (native_add7() == 7) {
        return 0;
    }
    return 1;
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("vendor/native/src/add.c"),
        "int native_add7(void) { return 7; }\n",
    )
    .unwrap();

    let manifest_path = root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path).unwrap();
    let members = crate::workspace::load_members(&manifest_path, &manifest).unwrap();
    let elaboration = plan(
        &manifest_path,
        &manifest,
        &members,
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
kern = "0.7.8"

[[bin]]
name = "app"
root = "src/main.kn"

[dependencies]
foo = { path = "../util", export = "util" }
"#,
    )
    .unwrap();
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::write(
        app_dir.join("src/main.kn"),
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
kern = "0.7.8"

[lib]
root = "src/lib.kn"
"#,
    )
    .unwrap();
    fs::create_dir_all(util_dir.join("src")).unwrap();
    fs::write(
        util_dir.join("src/lib.kn"),
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
fn builds_library_package_with_runtime_section_without_requiring_main() {
    let root = temp_dir("craft-exec-lib-runtime");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.8"

[runtime]
entry = "rt"
libc = false
bundle = "std"

[lib]
root = "src/lib.kn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/lib.kn"),
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
    assert_eq!(summary.link_actions, 0);
    assert_eq!(summary.compile_actions, 1);

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
kern = "0.7.8"

[[bin]]
name = "app"
root = "src/main.kn"

[dependencies]
util = { path = "../util" }
"#,
    )
    .unwrap();
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::write(
        app_dir.join("src/main.kn"),
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
kern = "0.7.8"

[lib]
root = "src/lib.kn"
"#,
    )
    .unwrap();
    fs::create_dir_all(util_dir.join("src")).unwrap();
    fs::write(
        util_dir.join("src/lib.kn"),
        r#"
use base.coll;

pub fn is_truthy(value: &[u8]) bool {
return value == "true";
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
