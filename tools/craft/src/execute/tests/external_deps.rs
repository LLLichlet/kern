//! Execution tests for external git/path dependencies.

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
kern = "0.7.6"

[[bin]]
name = "app"
root = "src/main.kn"

[dependencies]
log = { path = "vendor/log", version = "1" }
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.kn"),
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
kern = "0.7.6"

[lib]
root = "src/lib.kn"
"#,
    )
    .unwrap();
    fs::write(
        log_root.join("src/lib.kn"),
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
kern = "0.7.6"

[lib]
root = "src/lib.kn"
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
kern = "0.7.6"

[[bin]]
name = "app"
root = "src/main.kn"

[dependencies]
log = {{ git = "{}", branch = "main", version = "1" }}
"#,
            toml_string_literal(&repo)
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/main.kn"),
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
fn builds_package_from_git_workspace_export() {
    let root = temp_dir("craft-exec-external-git-workspace-export");
    let repo = root.join("json-kern.git");
    let json_dir = repo.join("json");
    fs::create_dir_all(json_dir.join("src")).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        repo.join("Craft.toml"),
        format!(
            r#"
[workspace]
name = "json-kern"
members = ["json"]

[workspace.package]
version = "1"
kern = "0.7.6"
description = "JSON workspace"
license = "MIT"
authors = ["Craft Tests <craft-tests@example.invalid>"]
readme = "README.md"
repository = "{}"

[workspace.exports]
json = {{ member = "json" }}
"#,
            toml_string_literal(&repo)
        ),
    )
    .unwrap();
    fs::write(repo.join("README.md"), "# json workspace\n").unwrap();
    fs::write(
        json_dir.join("Craft.toml"),
        format!(
            r#"
[package]
name = "json"
version = "1"
kern = "0.7.6"
description = "JSON package"
license = "MIT"
authors = ["Craft Tests <craft-tests@example.invalid>"]
readme = "../README.md"
repository = "{}"

[lib]
root = "src/lib.kn"
"#,
            toml_string_literal(&repo)
        ),
    )
    .unwrap();
    fs::write(
        json_dir.join("src/lib.kn"),
        r#"
pub fn answer() i32 {
return 42;
}
"#,
    )
    .unwrap();
    write_publish_artifacts(&repo);
    run_git(&repo, ["init", "--initial-branch=main"]);
    run_git(&repo, ["config", "user.name", "Craft Tests"]);
    run_git(
        &repo,
        ["config", "user.email", "craft-tests@example.invalid"],
    );
    run_git(&repo, ["add", "."]);
    run_git(&repo, ["commit", "-m", "initial"]);

    fs::write(
        root.join("Craft.toml"),
        format!(
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7.6"

[[bin]]
name = "app"
root = "src/main.kn"

[dependencies]
json = {{ git = "{}", branch = "main", version = "1" }}
"#,
            toml_string_literal(&repo)
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/main.kn"),
        r#"
fn main() i32 {
if (json.answer() == 42) {
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
fn release_build_preserves_thinlto_bitcode_for_external_git_library() {
    let root = temp_dir("craft-exec-external-git-thinlto");
    let repo = root.join("log.git");
    fs::create_dir_all(root.join("src")).unwrap();
    init_git_package(
        &repo,
        r#"
[package]
name = "log"
version = "1"
kern = "0.7.6"

[profile.release]
opt = 3
codegen-units = 2
lto = "thin"

[lib]
root = "src/lib.kn"
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
kern = "0.7.6"

[profile.release]
opt = 3
codegen-units = 2
lto = "thin"

[[bin]]
name = "app"
root = "src/main.kn"

[dependencies]
log = {{ git = "{}", branch = "main", version = "1" }}
"#,
            toml_string_literal(&repo)
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/main.kn"),
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
    let feature_selection = FeatureSelection {
        profile: crate::script::ProfileSelection::Release,
        ..Default::default()
    };
    let elaboration = plan(
        &manifest_path,
        &manifest,
        &[],
        false,
        crate::script::ScriptCommand::Build,
        &feature_selection,
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());

    let summary = build(&build_plan, &action_plan).unwrap();
    assert_eq!(summary.compile_actions, 2);
    assert_eq!(summary.link_actions, 1);

    let deps = external::requested_external_dependencies(&action_plan);
    assert_eq!(deps.len(), 1);
    let source_config = super::super::load_source_config(&build_plan).unwrap();
    let loaded = external::load_external_package_actions(
        &source_config,
        &build_plan.workspace_root,
        &deps[0],
        crate::script::ScriptCommand::Build,
        feature_selection.profile,
    )
    .unwrap();
    let lib_action = loaded
        .action_plan
        .compile_actions
        .iter()
        .find(|action| {
            action.package_id.name == "log" && action.target_kind == crate::plan::TargetKind::Lib
        })
        .expect("expected external library compile action");
    let linker_inputs = linker_input_paths_for_primary_output(&lib_action.object_path).unwrap();
    assert!(
        linker_inputs
            .iter()
            .all(|path| super::has_llvm_bitcode_magic(path)),
        "expected external release library to preserve ThinLTO bitcode inputs, got: {:?}",
        linker_inputs
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn release_build_refreshes_updated_external_git_thinlto_library() {
    let root = temp_dir("craft-exec-external-git-thinlto-refresh");
    let repo = root.join("log.git");
    fs::create_dir_all(root.join("src")).unwrap();
    init_git_package(
        &repo,
        r#"
[package]
name = "log"
version = "1"
kern = "0.7.6"

[profile.release]
opt = 3
codegen-units = 2
lto = "thin"

[lib]
root = "src/lib.kn"
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
kern = "0.7.6"

[profile.release]
opt = 3
codegen-units = 2
lto = "thin"

[[bin]]
name = "app"
root = "src/main.kn"

[dependencies]
log = {{ git = "{}", branch = "main", version = "1" }}
"#,
            toml_string_literal(&repo)
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/main.kn"),
        r#"
fn main() i32 {
return log.answer() - 42;
}
"#,
    )
    .unwrap();

    let manifest_path = root.join("Craft.toml");
    let feature_selection = FeatureSelection {
        profile: crate::script::ProfileSelection::Release,
        ..Default::default()
    };

    let manifest = Manifest::load(&manifest_path).unwrap();
    let elaboration = plan(
        &manifest_path,
        &manifest,
        &[],
        false,
        crate::script::ScriptCommand::Build,
        &feature_selection,
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let first_summary = build(&build_plan, &action_plan).unwrap();
    assert_eq!(first_summary.compile_actions, 2);
    assert_eq!(first_summary.link_actions, 1);

    let executable = action_plan
        .link_actions
        .iter()
        .find(|action| action.package_id.name == "app")
        .expect("expected app link action")
        .artifact_path
        .clone();
    let first_output = super::run_binary_with_retry(&executable, 0);
    assert!(first_output.status.success());

    fs::write(
        repo.join("src/lib.kn"),
        r#"
pub fn answer() i32 {
return 43;
}
"#,
    )
    .unwrap();
    commit_git_package(&repo, "update answer");

    let manifest = Manifest::load(&manifest_path).unwrap();
    let elaboration = plan(
        &manifest_path,
        &manifest,
        &[],
        false,
        crate::script::ScriptCommand::Build,
        &feature_selection,
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let second_summary = build(&build_plan, &action_plan).unwrap();
    assert!(
        second_summary.compile_actions >= 1,
        "expected updated external ThinLTO dependency to trigger at least one compile miss"
    );
    assert_eq!(second_summary.link_actions, 1);

    let second_executable = action_plan
        .link_actions
        .iter()
        .find(|action| action.package_id.name == "app")
        .expect("expected app link action")
        .artifact_path
        .clone();
    let second_output = super::run_binary_with_retry(&second_executable, 1);
    assert_eq!(second_output.status.code(), Some(1));

    let deps = external::requested_external_dependencies(&action_plan);
    assert_eq!(deps.len(), 1);
    let source_config = super::super::load_source_config(&build_plan).unwrap();
    let loaded = external::load_external_package_actions(
        &source_config,
        &build_plan.workspace_root,
        &deps[0],
        crate::script::ScriptCommand::Build,
        feature_selection.profile,
    )
    .unwrap();
    let lib_action = loaded
        .action_plan
        .compile_actions
        .iter()
        .find(|action| {
            action.package_id.name == "log" && action.target_kind == crate::plan::TargetKind::Lib
        })
        .expect("expected external library compile action");
    let linker_inputs = linker_input_paths_for_primary_output(&lib_action.object_path).unwrap();
    assert!(
        linker_inputs
            .iter()
            .all(|path| super::has_llvm_bitcode_magic(path)),
        "expected refreshed external release library to preserve ThinLTO bitcode inputs, got: {:?}",
        linker_inputs
    );

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
kern = "0.7.6"

[[bin]]
name = "app"
root = "src/main.kn"

[dependencies]
log = { path = "vendor/log", version = "1" }
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.kn"),
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
kern = "0.7.6"

[lib]
root = "src/lib.kn"

[dependencies]
corelog = { path = "vendor/corelog", version = "1" }
"#,
    )
    .unwrap();
    fs::write(
        log_root.join("src/lib.kn"),
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
kern = "0.7.6"

[lib]
root = "src/lib.kn"
"#,
    )
    .unwrap();
    fs::write(
        corelog_root.join("src/lib.kn"),
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
kern = "0.7.6"

[[bin]]
name = "app"
root = "src/main.kn"

[dependencies]
log = { path = "vendor/log", version = "1" }
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.kn"),
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
kern = "0.7.6"

[lib]
root = "src/lib.kn"

[dependencies]
corelog = { path = "vendor-nested/corelog", version = "1" }
"#,
    )
    .unwrap();
    fs::write(
        log_root.join("src/lib.kn"),
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
kern = "0.7.6"

[lib]
root = "src/lib.kn"
"#,
    )
    .unwrap();
    fs::write(
        corelog_root.join("src/lib.kn"),
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
