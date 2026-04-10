use super::{load_current_analysis_context, sync_analysis_context};
use crate::build_plan;
use crate::elaborate::{plan, FeatureSelection};
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

fn with_env_var<T>(name: &str, value: &str, f: impl FnOnce() -> T) -> T {
    let previous = std::env::var_os(name);
    unsafe {
        std::env::set_var(name, value);
    }
    let result = f();
    unsafe {
        if let Some(previous) = previous {
            std::env::set_var(name, previous);
        } else {
            std::env::remove_var(name);
        }
    }
    result
}

#[test]
fn syncs_and_loads_current_analysis_context() {
    let root = temp_dir("craft-analysis-context");
    fs::create_dir_all(root.join("src")).unwrap();
    let env_name = format!(
        "KERN_ANALYSIS_CONTEXT_{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    fs::write(
        root.join("Craft.toml"),
        format!(
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.6.7\"

[features]
experimental = []

[craft]
env = [\"{env_name}\"]

[[bin]]
name = \"app\"
root = \"src/main.rn\"
"
        ),
    )
    .unwrap();
    fs::write(
        root.join("craft.rn"),
        format!(
            "\
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {{
if (p.feature_enabled(\"experimental\")) {{
    p.cfg_bool(\"enable_telemetry\", true);
    p.define_string(\"GREETING_MSG\", \"Hello from craft\");
}}

if (p.env(\"{env_name}\") != .None) {{
    p.cfg_bool(\"is_dev_env\", true);
}}
}}
"
        ),
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

    let elaboration = with_env_var(&env_name, "1", || {
        plan(
            &manifest_path,
            &manifest,
            &workspace_members,
            false,
            crate::script::ScriptCommand::Build,
            &feature_selection,
        )
        .unwrap()
    });
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
    assert_eq!(values.get("is_dev_env").map(String::as_str), Some("true"));
    assert_eq!(
        values.get("GREETING_MSG").map(String::as_str),
        Some("Hello from craft")
    );
    let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(gitignore.contains(".craft/"));
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
kern = \"0.6.7\"

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
kern = \"0.6.7\"

[[bin]]
name = \"app\"
root = \"src/main.rn\"
",
    )
    .unwrap();

    assert!(load_current_analysis_context(&manifest_path, &root)
        .unwrap()
        .is_none());
}
