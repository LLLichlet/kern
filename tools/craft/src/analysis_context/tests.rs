use super::{load_current_analysis_context, sync_analysis_context};
use crate::build_plan;
use crate::elaborate::{FeatureSelection, plan};
use crate::manifest::Manifest;
use crate::workspace::load_members;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn syncs_and_loads_current_analysis_context() {
    let root = temp_dir("craft-analysis-context");
    fs::create_dir_all(root.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.7.5\"

[features]
experimental = []

[[bin]]
name = \"app\"
root = \"src/main.rn\"
",
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        "\
use craft.builder;

pub fn build(b: &mut builder.Builder) void {
    if (b.feature_enabled(\"experimental\")) {
        b.cfg_bool(\"enable_telemetry\", true);
        b.define_string(\"GREETING_MSG\", \"Hello from build\");
    }
}
",
    )
    .unwrap();
    fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();

    let manifest_path = root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path).unwrap();
    let workspace_members = load_members(&manifest_path, &manifest).unwrap();
    let mut feature_selection = FeatureSelection::default();
    feature_selection
        .explicit
        .insert("experimental".to_string());

    let elaboration = plan(
        &manifest_path,
        &manifest,
        &workspace_members,
        false,
        crate::script::ScriptCommand::Build,
        &feature_selection,
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    sync_analysis_context(
        &manifest_path,
        &elaboration,
        &build_plan,
        &feature_selection,
    )
    .unwrap();

    let context = load_current_analysis_context(&manifest_path, &root)
        .unwrap()
        .unwrap();
    let values = context
        .compile_time_values_for(&manifest_path, &root.join("src/main.rn"), &root)
        .unwrap();

    assert_eq!(
        values.get("enable_telemetry").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        values.get("GREETING_MSG").map(String::as_str),
        Some("Hello from build")
    );
    assert!(!root.join(".gitignore").exists());
}

#[test]
fn stale_manifest_digest_invalidates_analysis_context() {
    let root = temp_dir("craft-analysis-context-stale");
    fs::create_dir_all(root.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.7.5\"

[[bin]]
name = \"app\"
root = \"src/main.rn\"
",
    )
    .unwrap();
    fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();

    let manifest_path = root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path).unwrap();
    let workspace_members = load_members(&manifest_path, &manifest).unwrap();
    let feature_selection = FeatureSelection::default();
    let elaboration = plan(
        &manifest_path,
        &manifest,
        &workspace_members,
        false,
        crate::script::ScriptCommand::Build,
        &feature_selection,
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    sync_analysis_context(
        &manifest_path,
        &elaboration,
        &build_plan,
        &feature_selection,
    )
    .unwrap();

    fs::write(
        root.join("Craft.toml"),
        "\
[package]
name = \"app\"
version = \"0.1.1\"
kern = \"0.7.5\"

[[bin]]
name = \"app\"
root = \"src/main.rn\"
",
    )
    .unwrap();

    assert!(
        load_current_analysis_context(&manifest_path, &root)
            .unwrap()
            .is_none()
    );
}

#[test]
fn invalid_rendered_analysis_context_is_ignored() {
    let root = temp_dir("craft-analysis-context-invalid");
    fs::create_dir_all(root.join(".craft")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.7.5\"

[[bin]]
name = \"app\"
root = \"src/main.rn\"
",
    )
    .unwrap();
    fs::write(
        root.join(".craft").join("analysis.toml"),
        "version = nope\n",
    )
    .unwrap();

    assert!(
        load_current_analysis_context(&root.join("Craft.toml"), &root)
            .unwrap()
            .is_none()
    );
}
