use super::*;

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
kern = "0.7.1"

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
kern = "0.7.1"

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
fn removing_generated_helper_file_causes_rebuild_failure_instead_of_stale_success() {
    let root = temp_dir("craft-exec-stale-generated-cleanup");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.1"

[[bin]]
name = "demo"
root = "src/placeholder.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("src").join("placeholder.rn"),
        "fn main() i32 { return 1; }\n",
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let main = b.emit_generated(
    "src/main.rn",
    "mod helper;\nfn main() i32 { return helper.answer(); }\n"
);
let _ = b.emit_generated(
    "src/helper.rn",
    "pub/ fn answer() i32 { return 0; }\n"
);
b.set_source_root(main);
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
    let compile_action = action_plan
        .compile_actions
        .iter()
        .find(|action| {
            action.package_id.name == "demo" && action.target_kind == crate::plan::TargetKind::Bin
        })
        .unwrap();
    let helper_path = compile_action
        .generated_root_path
        .join("src")
        .join("helper.rn");
    let helper_state = crate::build_state::action_state_path(&helper_path);

    let first = build(&build_plan, &action_plan).unwrap();
    assert_eq!(first.compile_actions, 1);
    assert!(helper_path.is_file());
    assert!(helper_state.is_file());

    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let main = b.emit_generated(
    "src/main.rn",
    "mod helper;\nfn main() i32 { return helper.answer(); }\n"
);
b.set_source_root(main);
}
"#,
    )
    .unwrap();

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

    let err = build(&build_plan, &action_plan).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("compile failed"));
    assert!(!helper_path.exists());
    assert!(!helper_state.exists());

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
kern = "0.7.1"

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
fn removes_stale_artifact_outputs_when_build_script_plan_changes() {
    let root = temp_dir("craft-exec-stale-artifact-cleanup");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.1"

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
        "{ \"mode\": \"old\" }\n",
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let _ = b.copy_package_file_to_artifact("assets/config.json", "bundle/config.json");
let _ = b.emit_artifact_file("notes/old.txt", "old\n");
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
    let link_action = action_plan
        .link_actions
        .iter()
        .find(|action| {
            action.package_id.name == "demo" && action.target_kind == crate::plan::TargetKind::Bin
        })
        .unwrap();
    let artifact_root = link_action.artifact_root_path.clone();
    let old_note = artifact_root.join("notes").join("old.txt");
    let old_note_state = crate::build_state::action_state_path(&old_note);
    let old_bundle = artifact_root.join("bundle").join("config.json");
    let old_bundle_state = crate::build_state::action_state_path(&old_bundle);

    let first = build(&build_plan, &action_plan).unwrap();
    assert_eq!(first.link_actions, 1);
    assert!(old_note.is_file());
    assert!(old_note_state.is_file());
    assert!(old_bundle.is_file());
    assert!(old_bundle_state.is_file());

    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let _ = b.emit_artifact_file("notes/new.txt", "new\n");
}
"#,
    )
    .unwrap();

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
    let link_action = action_plan
        .link_actions
        .iter()
        .find(|action| {
            action.package_id.name == "demo" && action.target_kind == crate::plan::TargetKind::Bin
        })
        .unwrap();
    let new_note = link_action.artifact_root_path.join("notes").join("new.txt");
    let new_note_state = crate::build_state::action_state_path(&new_note);

    let second = build(&build_plan, &action_plan).unwrap();
    assert_eq!(second.compile_actions, 0);
    assert_eq!(second.link_actions, 0);
    assert!(!old_note.exists());
    assert!(!old_note_state.exists());
    assert!(!old_bundle.exists());
    assert!(!old_bundle_state.exists());
    assert!(!artifact_root.join("bundle").exists());
    assert_eq!(fs::read_to_string(&new_note).unwrap(), "new\n");
    assert!(new_note_state.is_file());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn stale_artifact_cleanup_preserves_kept_directory_output_cache_hits() {
    let root = temp_dir("craft-exec-stale-artifact-dir-cache");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("assets").join("images")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.1"

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
let _ = b.emit_artifact_file("notes/old.txt", "old\n");
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
    let link_action = action_plan
        .link_actions
        .iter()
        .find(|action| {
            action.package_id.name == "demo" && action.target_kind == crate::plan::TargetKind::Bin
        })
        .unwrap();
    let artifact_root = link_action.artifact_root_path.clone();
    let bundle_dir = artifact_root.join("bundle").join("assets");
    let old_note = artifact_root.join("notes").join("old.txt");

    let first = build(&build_plan, &action_plan).unwrap();
    assert_eq!(first.link_actions, 1);
    assert!(bundle_dir.join("config.json").is_file());
    assert!(bundle_dir.join("images").join("logo.txt").is_file());
    assert!(old_note.is_file());

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

    let second = build(&build_plan, &action_plan).unwrap();
    assert_eq!(second.compile_actions, 0);
    assert_eq!(second.link_actions, 0);
    assert_eq!(second.action_cache_stats.staged_misses, 0);
    assert!(second.action_cache_stats.staged_hits >= 1);
    assert!(!old_note.exists());
    assert!(bundle_dir.join("config.json").is_file());
    assert!(bundle_dir.join("images").join("logo.txt").is_file());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn directory_stage_rebuilds_when_source_adds_empty_subdirectory() {
    let root = temp_dir("craft-exec-dir-stage-empty-subdir");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.1"

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
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let link_action = action_plan
        .link_actions
        .iter()
        .find(|action| {
            action.package_id.name == "demo" && action.target_kind == crate::plan::TargetKind::Bin
        })
        .unwrap();
    let bundle_dir = link_action.artifact_root_path.join("bundle").join("assets");

    let first = build(&build_plan, &action_plan).unwrap();
    assert_eq!(first.action_cache_stats.staged_misses, 1);
    assert!(bundle_dir.join("config.json").is_file());

    fs::create_dir_all(root.join("assets").join("empty-dir")).unwrap();

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

    let second = build(&build_plan, &action_plan).unwrap();
    assert_eq!(second.compile_actions, 0);
    assert_eq!(second.link_actions, 0);
    assert!(second.action_cache_stats.staged_misses >= 1);
    assert!(bundle_dir.join("empty-dir").is_dir());

    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn post_link_directory_stage_rejects_symlink_entries() {
    use std::os::unix::fs::symlink;

    let root = temp_dir("craft-exec-post-link-dir-symlink");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.1"

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
    symlink(
        root.join("assets").join("config.json"),
        root.join("assets").join("config-link.json"),
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
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());

    let err = build(&build_plan, &action_plan).unwrap_err();
    assert!(err.to_string().contains("unsupported filesystem entry"));

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
kern = "0.7.1"

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
fn builds_and_runs_hosted_package_with_copied_primary_artifact_in_stage_tree() {
    let root = temp_dir("craft-exec-copy-primary-artifact");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.1"

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
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let artifact = b.primary_artifact();
let _ = b.copy_output_to_artifact(artifact, "bundle/demo");
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
    assert_eq!(link_nodes.len(), 1);
    let staged_copy = PathBuf::from(&link_nodes[0].output);
    assert!(staged_copy.is_file());
    assert_eq!(
        fs::read(&summary.executable).unwrap(),
        fs::read(&staged_copy).unwrap()
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
kern = "0.7.1"

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
kern = "0.7.1"

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
kern = "0.7.1"

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
kern = "0.7.1"

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
kern = "0.7.1"

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
fn builds_hosted_package_with_post_link_artifact_file_from_host_tool() {
    let root = temp_dir("craft-exec-artifact-file-from-tool");
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
kern = "0.7.1"

[[bin]]
name = "app"
root = "src/main.rn"

[build-dependencies]
tool = { path = "../tool", package = "tool" }
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let note = b.stage_artifact_file_from_tool("tool", "artifact-note", "notes/build.txt", .{});
let bundle = b.stage_copy_output_to_artifact(b.primary_artifact(), "bundle/app");
b.depend(note, bundle);
}
"#,
    )
    .unwrap();
    fs::write(app_dir.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
    fs::write(
        tool_dir.join("Craft.toml"),
        r#"
[package]
name = "tool"
version = "0.1.0"
kern = "0.7.1"

[[bin]]
name = "artifact-note"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        tool_dir.join("src/main.rn"),
        r#"
use std.io;
use std.io.Writer;

fn main() i32 {
let mut out = io.stdout();
let writer = *mut Writer.{ out..& };
let _ = writer.write("built by tool\n");
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
        crate::script::ScriptCommand::Build,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan =
        crate::build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let package = build_plan
        .packages
        .iter()
        .find(|package| package.package_id.name == "app")
        .unwrap();
    let unit = package
        .units
        .iter()
        .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
        .unwrap();
    let link_action = action_plan
        .link_actions
        .iter()
        .find(|action| {
            action.package_id.name == "app" && action.target_kind == crate::plan::TargetKind::Bin
        })
        .unwrap();
    let summary = run(&build_plan, &action_plan, unit).unwrap();

    assert!(summary.executable.is_file());
    let note_path = action_plan
        .artifact_output_nodes_for_link_action(link_action)
        .iter()
        .find(|node| node.output.ends_with("notes/build.txt"))
        .map(|node| PathBuf::from(&node.output))
        .unwrap();
    assert_eq!(fs::read_to_string(note_path).unwrap(), "built by tool\n");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn reruns_post_link_host_tool_when_dependent_staged_output_changes() {
    let root = temp_dir("craft-exec-artifact-file-from-tool-rebuild");
    let app_dir = root.join("app");
    let tool_dir = root.join("tool");
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::create_dir_all(app_dir.join("assets")).unwrap();
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
kern = "0.7.1"

[[bin]]
name = "app"
root = "src/main.rn"

[build-dependencies]
tool = { path = "../tool", package = "tool" }
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let bundle = b.stage_copy_package_file_to_artifact("assets/message.txt", "bundle/message.txt");
let note = b.stage_artifact_file_from_tool(
    "tool",
    "artifact-note",
    "notes/build.txt",
    .{ b.output_path(bundle), "" }
);
b.depend(note, bundle);
}
"#,
    )
    .unwrap();
    fs::write(app_dir.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
    fs::write(app_dir.join("assets").join("message.txt"), "alpha\n").unwrap();
    fs::write(
        tool_dir.join("Craft.toml"),
        r#"
[package]
name = "tool"
version = "0.1.0"
kern = "0.7.1"

[[bin]]
name = "artifact-note"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        tool_dir.join("src/main.rn"),
        r#"
use base.mem.alloc.GPA;
use std.fs;
use std.proc;
use sys.mem.Page;

fn main(argc: i32, argv: **u8) i32 {
let page = Page.{}..&;
let gpa = GPA.{ backing: page }..&;
let args = proc.args(argc, argv);
let .{ Some: path } = args.get(1) else return 1;
let .{ Ok: text } = fs.read_to_string(gpa, path) else return 1;
let mut text = text;
let text = text..&;
defer text.deinit(gpa);
let mut stdout = fs.stdout();
let _ = stdout..&.write_all(text.as_str());
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
        crate::script::ScriptCommand::Build,
        &FeatureSelection::default(),
    )
    .unwrap();
    let build_plan =
        crate::build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let link_action = action_plan
        .link_actions
        .iter()
        .find(|action| {
            action.package_id.name == "app" && action.target_kind == crate::plan::TargetKind::Bin
        })
        .unwrap();
    let note_path = action_plan
        .artifact_output_nodes_for_link_action(link_action)
        .iter()
        .find(|node| node.output.ends_with("notes/build.txt"))
        .map(|node| PathBuf::from(&node.output))
        .unwrap();

    let first = build(&build_plan, &action_plan).unwrap();
    assert!(first.action_cache_stats.staged_misses > 0);
    assert_eq!(fs::read_to_string(&note_path).unwrap(), "alpha\n");

    fs::write(app_dir.join("assets").join("message.txt"), "beta\n").unwrap();

    let second = build(&build_plan, &action_plan).unwrap();
    assert!(second.action_cache_stats.staged_misses > 0);
    assert_eq!(fs::read_to_string(&note_path).unwrap(), "beta\n");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_reports_staged_tool_stderr_on_failure() {
    let root = temp_dir("craft-exec-tool-stderr");
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
kern = "0.7.1"

[[bin]]
name = "app"
root = "src/main.rn"

[build-dependencies]
tool = { path = "../tool", package = "tool" }
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let _ = b.stage_artifact_file_from_tool("tool", "artifact-note", "notes/build.txt", .{});
}
"#,
    )
    .unwrap();
    fs::write(app_dir.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
    fs::write(
        tool_dir.join("Craft.toml"),
        r#"
[package]
name = "tool"
version = "0.1.0"
kern = "0.7.1"

[[bin]]
name = "artifact-note"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        tool_dir.join("src/main.rn"),
        r#"
use std.io;
use std.io.Writer;

fn main() i32 {
let mut out = io.stdout();
let out_writer = *mut Writer.{ out..& };
let _ = out_writer.write("partial stdout\n");

let mut err = io.stderr();
let err_writer = *mut Writer.{ err..& };
let _ = err_writer.write("tool failed loudly\n");
return 7;
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
    let build_plan =
        crate::build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let err = build(&build_plan, &action_plan).unwrap_err();
    let message = err.to_string();

    assert!(
        message.contains("artifact-note") && message.contains("exited with status"),
        "unexpected error: {message}"
    );
    assert!(
        message.contains("stderr:\ntool failed loudly"),
        "unexpected error: {message}"
    );
    assert!(
        message.contains("stdout:\npartial stdout"),
        "unexpected error: {message}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_relative_source_root_uses_member_package_root() {
    let root = temp_dir("craft-exec-member-relative-source-root");
    let app_dir = root.join("app");
    fs::create_dir_all(app_dir.join("src")).unwrap();
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
kern = "0.7.1"

[runtime]
entry = "rt"
bundle = "std"

[[bin]]
name = "app"
root = "src/placeholder.rn"
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let _ = b;
b.set_source_root("src/real_main.rn");
}
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("src/placeholder.rn"),
        "fn main() i32 { return 1; }\n",
    )
    .unwrap();
    fs::write(
        app_dir.join("src/real_main.rn"),
        r#"
use std.io;

fn main() i32 {
    io.println("member-source-root", .{});
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
        .find(|package| package.package_id.name == "app")
        .unwrap()
        .units
        .iter()
        .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
        .unwrap();

    let crate::build_plan::SourceRootBinding::PackagePath(source_root) = &unit.source_root else {
        panic!("expected relative source root to remain package-relative");
    };
    assert_eq!(source_root, "src/real_main.rn");

    let summary = run(&build_plan, &action_plan, unit).unwrap();
    assert!(summary.executable.is_file());
    let output = run_binary_with_retry(&summary.executable, 0);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "member-source-root\n"
    );

    let _ = fs::remove_dir_all(root);
}
