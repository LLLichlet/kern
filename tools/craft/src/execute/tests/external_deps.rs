use super::*;

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
