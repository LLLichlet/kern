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
kern = "0.7.0"

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
kern = "0.7.0"

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
kern = "0.7.0"

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
kern = "0.7.0"

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
kern = "0.7.0"

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
fn builds_library_package_with_runtime_section_without_requiring_main() {
    let root = temp_dir("craft-exec-lib-runtime");
    fs::create_dir_all(root.join("src")).unwrap();
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
bundle = "std"

[lib]
root = "src/lib.rn"
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
kern = "0.7.0"

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
kern = "0.7.0"

[lib]
root = "src/lib.rn"
"#,
    )
    .unwrap();
    fs::create_dir_all(util_dir.join("src")).unwrap();
    fs::write(
        util_dir.join("src/lib.rn"),
        r#"
use base.coll;

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
