use super::*;

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
fn parallel_target_link_jobs_exclude_thinlto_final_links() {
    let root = temp_dir("craft-exec-parallel-link-thinlto");
    let native_dir = root.join("native");
    let thin_dir = root.join("thin");
    fs::create_dir_all(native_dir.join("src")).unwrap();
    fs::create_dir_all(thin_dir.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        r#"
[workspace]
members = ["native", "thin"]
"#,
    )
    .unwrap();
    fs::write(
        native_dir.join("Craft.toml"),
        r#"
[package]
name = "native"
version = "0.1.0"
kern = "0.6.7"

[profile.release]
lto = "none"

[[bin]]
name = "native"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        native_dir.join("src/main.rn"),
        "fn main() i32 { return 0; }\n",
    )
    .unwrap();
    fs::write(
        thin_dir.join("Craft.toml"),
        r#"
[package]
name = "thin"
version = "0.1.0"
kern = "0.6.7"

[profile.release]
lto = "thin"

[[bin]]
name = "thin"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        thin_dir.join("src/main.rn"),
        "fn main() i32 { return 0; }\n",
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
        &FeatureSelection {
            profile: crate::script::ProfileSelection::Release,
            ..FeatureSelection::default()
        },
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let compile_action_index = super::external::compile_actions_index(&action_plan.compile_actions);
    let jobs = parallel_target_link_jobs(&action_plan, &compile_action_index, &Default::default())
        .unwrap();

    assert_eq!(action_plan.link_actions.len(), 2);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].link_action.package_id.name, "native");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn parallel_target_compile_jobs_only_include_ready_local_libraries() {
    let root = temp_dir("craft-exec-parallel-compile-jobs");
    let util_dir = root.join("util");
    let extra_dir = root.join("extra");
    let app_dir = root.join("app");
    fs::create_dir_all(util_dir.join("src")).unwrap();
    fs::create_dir_all(extra_dir.join("src")).unwrap();
    fs::create_dir_all(app_dir.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        r#"
[workspace]
members = ["util", "extra", "app"]
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
        util_dir.join("src/lib.rn"),
        r#"
pub fn answer() i32 {
    return 42;
}
"#,
    )
    .unwrap();
    fs::write(
        extra_dir.join("Craft.toml"),
        r#"
[package]
name = "extra"
version = "0.1.0"
kern = "0.6.7"

[lib]
root = "src/lib.rn"
"#,
    )
    .unwrap();
    fs::write(
        extra_dir.join("src/lib.rn"),
        r#"
pub fn truth() bool {
    return true;
}
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

[dependencies]
util = { path = "../util" }
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("src/lib.rn"),
        r#"
pub fn value() i32 {
    return util.answer();
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
    let local_library_actions =
        super::external::local_library_actions(&action_plan.compile_actions);

    let initial_jobs =
        parallel_target_compile_jobs(&action_plan, &local_library_actions, &Default::default());
    assert_eq!(initial_jobs.len(), 2);
    assert!(
        initial_jobs
            .iter()
            .any(|job| job.compile_action.package_id.name == "util")
    );
    assert!(
        initial_jobs
            .iter()
            .any(|job| job.compile_action.package_id.name == "extra")
    );
    assert!(
        initial_jobs
            .iter()
            .all(|job| job.compile_action.package_id.name != "app")
    );

    let util_action = action_plan
        .compile_actions
        .iter()
        .find(|action| action.package_id.name == "util")
        .unwrap();
    let mut compiled = std::collections::BTreeSet::from([util_action.object_path.clone()]);
    let second_jobs = parallel_target_compile_jobs(&action_plan, &local_library_actions, &compiled);
    assert_eq!(second_jobs.len(), 2);
    assert!(
        second_jobs
            .iter()
            .any(|job| job.compile_action.package_id.name == "app")
    );
    assert!(
        second_jobs
            .iter()
            .any(|job| job.compile_action.package_id.name == "extra")
    );
    assert!(
        second_jobs
            .iter()
            .all(|job| job.compile_action.package_id.name != "util")
    );

    compiled.extend(
        second_jobs
            .iter()
            .map(|job| job.compile_action.object_path.clone()),
    );
    let final_jobs = parallel_target_compile_jobs(&action_plan, &local_library_actions, &compiled);
    assert!(final_jobs.is_empty());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn parallel_target_links_wait_for_local_library_dependencies() {
    let root = temp_dir("craft-exec-parallel-link-deps");
    let util_dir = root.join("util");
    let app_a_dir = root.join("app_a");
    let app_b_dir = root.join("app_b");
    fs::create_dir_all(util_dir.join("src")).unwrap();
    fs::create_dir_all(app_a_dir.join("src")).unwrap();
    fs::create_dir_all(app_b_dir.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        r#"
[workspace]
members = ["util", "app_a", "app_b"]
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
        util_dir.join("src/lib.rn"),
        r#"
pub fn answer() i32 {
    return 42;
}
"#,
    )
    .unwrap();

    for (name, dir) in [("app_a", &app_a_dir), ("app_b", &app_b_dir)] {
        fs::write(
            dir.join("Craft.toml"),
            format!(
                r#"
[package]
name = "{name}"
version = "0.1.0"
kern = "0.6.7"

[[bin]]
name = "{name}"
root = "src/main.rn"

[dependencies]
util = {{ path = "../util" }}
"#
            ),
        )
        .unwrap();
        fs::write(
            dir.join("src/main.rn"),
            r#"
fn main() i32 {
    return util.answer() - 42;
}
"#,
        )
        .unwrap();
    }

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

    let summary = build(&build_plan, &action_plan).unwrap();
    assert_eq!(summary.compile_actions, 3);
    assert_eq!(summary.link_actions, 2);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn release_build_links_against_thinlto_local_library_inputs() {
    let root = temp_dir("craft-exec-multi-object-local-lib");
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

[profile.release]
codegen-units = 2

[[bin]]
name = "app"
root = "src/main.rn"

[dependencies]
util = { path = "../util" }
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("src/main.rn"),
        r#"
fn main() i32 {
    return util.answer() - 3;
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

[profile.release]
codegen-units = 2

[lib]
root = "src/lib.rn"
"#,
    )
    .unwrap();
    fs::write(
        util_dir.join("src/lib.rn"),
        r#"
pub fn answer() i32 {
    return foo() + bar();
}

fn foo() i32 {
    return 1;
}

fn bar() i32 {
    return 2;
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
        &FeatureSelection {
            profile: crate::script::ProfileSelection::Release,
            ..FeatureSelection::default()
        },
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());

    let summary = build(&build_plan, &action_plan).unwrap();
    assert_eq!(summary.compile_actions, 2);
    assert_eq!(summary.link_actions, 1);

    let util_action = action_plan
        .compile_actions
        .iter()
        .find(|action| action.package_id.name == "util")
        .unwrap();
    let object_dir = super::multi_linker_input_dir(&util_action.object_path);
    assert!(util_action.object_path.is_file());
    if object_dir.is_dir() {
        assert!(
            fs::read_dir(&object_dir)
                .unwrap()
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .any(|path| {
                    path.extension().and_then(|ext| ext.to_str()) == Some("o")
                        && super::has_llvm_bitcode_magic(&path)
                })
        );
    } else {
        assert!(super::has_llvm_bitcode_magic(&util_action.object_path));
    }

    let _ = fs::remove_dir_all(root);
}
