use super::AnalysisProject;
use super::paths::normalize_platform_path;
use crate::analysis_context;
use crate::build_plan;
use crate::elaborate::{FeatureSelection, plan};
use crate::manifest::Manifest;
use crate::plan::TargetKind;
use crate::workspace::load_members;
use kernc_utils::config::{CompileOptions, LibraryBundle, RuntimeEntry};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn normalize_test_path(path: &Path) -> PathBuf {
    normalize_platform_path(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
}

fn normalize_test_optional_path(path: Option<&String>) -> Option<PathBuf> {
    path.map(|path| normalize_test_path(Path::new(path)))
}

fn normalize_test_alias_map(
    aliases: &std::collections::BTreeMap<PathBuf, PathBuf>,
) -> std::collections::BTreeMap<PathBuf, PathBuf> {
    aliases
        .iter()
        .map(|(source, generated)| {
            (
                normalize_test_path(source.as_path()),
                normalize_test_path(generated.as_path()),
            )
        })
        .collect()
}

fn temp_dir(prefix: &str) -> PathBuf {
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
fn resolves_workspace_local_library_aliases_for_analysis() {
    let root = temp_dir("craft-project-analysis");
    let app_dir = root.join("app");
    let util_dir = root.join("util");
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::create_dir_all(util_dir.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        "[workspace]\nmembers = [\"app\", \"util\"]\n",
    )
    .unwrap();
    fs::write(
        app_dir.join("Craft.toml"),
        "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.6.7\"

[lib]
root = \"src/lib.rn\"

[dependencies]
util = { path = \"../util\" }
",
    )
    .unwrap();
    fs::write(app_dir.join("src/lib.rn"), "use util;\n").unwrap();
    fs::write(
        util_dir.join("Craft.toml"),
        "\
[package]
name = \"util\"
version = \"0.1.0\"
kern = \"0.6.7\"

[lib]
root = \"src/lib.rn\"
",
    )
    .unwrap();
    fs::write(
        util_dir.join("src/lib.rn"),
        "fn helper() i32 { return 1; }\n",
    )
    .unwrap();

    let project = AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
    let resolved =
        project.resolve_for_file(&app_dir.join("src/lib.rn"), &CompileOptions::default());

    assert_eq!(
        normalize_test_path(&resolved.input_file),
        normalize_test_path(&app_dir.join("src/lib.rn"))
    );
    assert_eq!(
        resolved.compile_options.root_module_name,
        Some("app".to_string())
    );
    assert_eq!(
        resolved
            .compile_options
            .module_aliases
            .get("util")
            .and_then(|path| normalize_test_optional_path(Some(path))),
        Some(normalize_test_path(&util_dir.join("src/lib.rn")))
    );
}

#[test]
fn resolves_external_path_dependency_aliases_for_analysis() {
    let root = temp_dir("craft-project-external-analysis");
    let deps_dir = root.join("deps");
    let app_dir = root.join("app");
    let util_dir = deps_dir.join("util");
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::create_dir_all(util_dir.join("src")).unwrap();

    fs::write(
        app_dir.join("Craft.toml"),
        "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.6.7\"

[lib]
root = \"src/lib.rn\"

[dependencies]
util = { path = \"../deps/util\" }
",
    )
    .unwrap();
    fs::write(app_dir.join("src/lib.rn"), "use util;\n").unwrap();
    fs::write(
        util_dir.join("Craft.toml"),
        "\
[package]
name = \"util\"
version = \"0.1.0\"
kern = \"0.6.7\"

[lib]
root = \"src/lib.rn\"
",
    )
    .unwrap();
    fs::write(
        util_dir.join("src/lib.rn"),
        "pub fn helper() i32 { return 1; }\n",
    )
    .unwrap();

    let project = AnalysisProject::load_from_manifest(&app_dir.join("Craft.toml")).unwrap();
    let resolved =
        project.resolve_for_file(&app_dir.join("src/lib.rn"), &CompileOptions::default());

    assert_eq!(
        resolved
            .compile_options
            .module_aliases
            .get("util")
            .and_then(|path| normalize_test_optional_path(Some(path))),
        Some(normalize_test_path(&util_dir.join("src/lib.rn")))
    );
}

#[test]
fn library_analysis_keeps_lib_runtime_defaults_even_with_runtime_section() {
    let root = temp_dir("craft-project-lib-runtime-analysis");
    fs::create_dir_all(root.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        "\
[package]
name = \"demo\"
version = \"0.1.0\"
kern = \"0.6.7\"

[runtime]
entry = \"rt\"
libc = false
bundle = \"base\"

[lib]
root = \"src/lib.rn\"
",
    )
    .unwrap();
    fs::write(
        root.join("src/lib.rn"),
        "pub fn answer() i32 { return 42; }\n",
    )
    .unwrap();

    let project = AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
    let resolved = project.resolve_for_file(&root.join("src/lib.rn"), &CompileOptions::default());

    assert_eq!(resolved.compile_options.runtime_entry, RuntimeEntry::None);
    assert!(!resolved.compile_options.runtime_libc);
    assert_eq!(resolved.compile_options.library_bundle, LibraryBundle::Base);
}

#[test]
fn test_analysis_applies_runtime_section_to_tests() {
    let root = temp_dir("craft-project-test-runtime-analysis");
    fs::create_dir_all(root.join("tests")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        "\
[package]
name = \"demo\"
version = \"0.1.0\"
kern = \"0.6.7\"

[runtime]
entry = \"rt\"
libc = false
bundle = \"base\"

[test]
roots = [\"tests/smoke.rn\"]
",
    )
    .unwrap();
    fs::write(root.join("tests/smoke.rn"), "fn main() i32 { return 0; }\n").unwrap();

    let project = AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
    let resolved =
        project.resolve_for_file(&root.join("tests/smoke.rn"), &CompileOptions::default());

    assert_eq!(resolved.compile_options.runtime_entry, RuntimeEntry::Rt);
    assert!(!resolved.compile_options.runtime_libc);
    assert_eq!(resolved.compile_options.library_bundle, LibraryBundle::Base);
}

#[test]
fn prefers_exact_named_target_root_over_library_root() {
    let root = temp_dir("craft-project-multi-target-analysis");
    let app_dir = root.join("app");
    fs::create_dir_all(app_dir.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        "[workspace]\nmembers = [\"app\"]\n",
    )
    .unwrap();
    fs::write(
        app_dir.join("Craft.toml"),
        "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.6.7\"

[lib]
root = \"src/lib.rn\"

[[bin]]
name = \"demo\"
root = \"src/demo.rn\"
",
    )
    .unwrap();
    fs::write(
        app_dir.join("src/lib.rn"),
        "fn helper() i32 { return 1; }\n",
    )
    .unwrap();
    fs::write(app_dir.join("src/demo.rn"), "fn main() i32 { return 0; }\n").unwrap();

    let project = AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
    let resolved =
        project.resolve_for_file(&app_dir.join("src/demo.rn"), &CompileOptions::default());

    assert_eq!(
        normalize_test_path(&resolved.input_file),
        normalize_test_path(&app_dir.join("src/demo.rn"))
    );
    assert_eq!(resolved.compile_options.root_module_name, None);
}

#[test]
fn prefers_named_target_module_directory_over_library_root() {
    let root = temp_dir("craft-project-module-dir-analysis");
    let app_dir = root.join("app");
    fs::create_dir_all(app_dir.join("src/demo")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        "[workspace]\nmembers = [\"app\"]\n",
    )
    .unwrap();
    fs::write(
        app_dir.join("Craft.toml"),
        "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.6.7\"

[lib]
root = \"src/lib.rn\"

[[bin]]
name = \"demo\"
root = \"src/demo.rn\"
",
    )
    .unwrap();
    fs::write(
        app_dir.join("src/lib.rn"),
        "fn helper() i32 { return 1; }\n",
    )
    .unwrap();
    fs::write(
        app_dir.join("src/demo.rn"),
        "mod extra;\nfn main() i32 { return extra::run(); }\n",
    )
    .unwrap();
    fs::write(
        app_dir.join("src/demo/extra.rn"),
        "pub fn run() i32 { return 0; }\n",
    )
    .unwrap();

    let project = AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
    let resolved = project.resolve_for_file(
        &app_dir.join(Path::new("src/demo/extra.rn")),
        &CompileOptions::default(),
    );

    assert_eq!(
        normalize_test_path(&resolved.input_file),
        normalize_test_path(&app_dir.join("src/demo.rn"))
    );
    assert_eq!(resolved.compile_options.root_module_name, None);
}

#[test]
fn resolves_package_craft_script_with_sdk_alias_even_when_library_exists() {
    let root = temp_dir("craft-project-script-analysis");
    let app_dir = root.join("app");
    fs::create_dir_all(app_dir.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        "[workspace]\nmembers = [\"app\"]\n",
    )
    .unwrap();
    fs::write(
        app_dir.join("Craft.toml"),
        "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.6.7\"

[lib]
root = \"src/lib.rn\"
",
    )
    .unwrap();
    fs::write(app_dir.join("src/lib.rn"), "pub fn helper() void {}\n").unwrap();
    fs::write(
        app_dir.join("craft.rn"),
        "use craft.plan;\npub fn craft(p: *mut plan.Plan) void { let _ = p; }\n",
    )
    .unwrap();

    let project = AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
    let resolved = project.resolve_for_file(&app_dir.join("craft.rn"), &CompileOptions::default());

    assert_eq!(
        normalize_test_path(&resolved.input_file),
        normalize_test_path(&app_dir.join("craft.rn"))
    );
    assert_eq!(
        resolved
            .compile_options
            .module_aliases
            .get("craft")
            .and_then(|path| normalize_test_optional_path(Some(path))),
        Some(normalize_test_path(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("sdk")
                .join("init.rn")
                .parent()
                .unwrap(),
        ))
    );
    assert_eq!(resolved.compile_options.root_module_name, None);
}

#[test]
fn resolves_workspace_craft_script_with_sdk_alias() {
    let root = temp_dir("craft-workspace-script-analysis");
    fs::create_dir_all(root.join("app/src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        "[workspace]\nmembers = [\"app\"]\n",
    )
    .unwrap();
    fs::write(
        root.join("craft.rn"),
        "use craft.plan;\npub fn craft(p: *mut plan.Plan) void { let _ = p; }\n",
    )
    .unwrap();
    fs::write(
        root.join("app/Craft.toml"),
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
    fs::write(
        root.join("app/src/main.rn"),
        "fn main() i32 { return 0; }\n",
    )
    .unwrap();

    let project = AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
    let resolved = project.resolve_for_file(&root.join("craft.rn"), &CompileOptions::default());

    assert_eq!(
        normalize_test_path(&resolved.input_file),
        normalize_test_path(&root.join("craft.rn"))
    );
    assert_eq!(
        resolved
            .compile_options
            .module_aliases
            .get("craft")
            .and_then(|path| normalize_test_optional_path(Some(path))),
        Some(normalize_test_path(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("sdk")
                .join("init.rn")
                .parent()
                .unwrap(),
        ))
    );
}

#[test]
fn resolve_for_file_applies_craft_cfg_and_define_values() {
    let root = temp_dir("craft-project-custom-defines");
    fs::create_dir_all(root.join("src")).unwrap();
    let env_name = format!(
        "KERN_PROJECT_ANALYSIS_{}",
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

    let project = AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
    let mut options = CompileOptions::default();
    options.craft_features.push("experimental".to_string());

    let resolved = with_env_var(&env_name, "1", || {
        project.resolve_for_file(&root.join("src/main.rn"), &options)
    });

    let defines = &resolved.compile_options.custom_defines;
    let collected = defines
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<HashMap<_, _>>();
    assert_eq!(
        collected.get("enable_telemetry").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        collected.get("is_dev_env").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        collected.get("GREETING_MSG").map(String::as_str),
        Some("Hello from craft")
    );
}

#[test]
fn resolve_for_file_prefers_persisted_analysis_context_without_explicit_features() {
    let root = temp_dir("craft-project-persisted-analysis");
    fs::create_dir_all(root.join("src")).unwrap();
    let env_name = format!(
        "KERN_PROJECT_PERSISTED_{}",
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
    let mut selection = FeatureSelection::default();
    selection.explicit.insert("experimental".to_string());
    let elaboration = with_env_var(&env_name, "1", || {
        plan(
            &manifest_path,
            &manifest,
            &workspace_members,
            false,
            crate::script::ScriptCommand::Build,
            &selection,
        )
        .unwrap()
    });
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    analysis_context::sync_analysis_context(&manifest_path, &elaboration, &build_plan, &selection)
        .unwrap();

    let project = AnalysisProject::load_from_manifest(&manifest_path).unwrap();
    let resolved = project.resolve_for_file(&root.join("src/main.rn"), &CompileOptions::default());
    let defines = resolved
        .compile_options
        .custom_defines
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<HashMap<_, _>>();

    assert_eq!(
        defines.get("enable_telemetry").map(String::as_str),
        Some("true")
    );
    assert_eq!(defines.get("is_dev_env").map(String::as_str), Some("true"));
    assert_eq!(
        defines.get("GREETING_MSG").map(String::as_str),
        Some("Hello from craft")
    );
}

#[test]
fn resolve_for_generated_source_root_uses_analysis_unit_matching() {
    let root = temp_dir("craft-project-generated-analysis");

    fs::write(
        root.join("Craft.toml"),
        "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.6.7\"

[[bin]]
name = \"app\"
root = \"src/placeholder.rn\"
",
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        "\
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    let generated = b.emit_generated(
        \"src/main.rn\",
        \"fn main() i32 { return 0; }\\n\"
    );
    b.set_source_root(generated);
    b.cfg_bool(\"generated\", true);
    b.define_string(\"ENTRY_KIND\", \"generated\");
}
",
    )
    .unwrap();

    let manifest_path = root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path).unwrap();
    let workspace_members = load_members(&manifest_path, &manifest).unwrap();
    let selection = FeatureSelection::default();
    let elaboration = plan(
        &manifest_path,
        &manifest,
        &workspace_members,
        false,
        crate::script::ScriptCommand::Build,
        &selection,
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let generated_root = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .and_then(|unit| match &unit.source_root {
            crate::build_plan::SourceRootBinding::AbsolutePath(path) => Some(PathBuf::from(path)),
            _ => None,
        })
        .expect("expected generated source root");
    analysis_context::sync_analysis_context(&manifest_path, &elaboration, &build_plan, &selection)
        .unwrap();

    let project = AnalysisProject::load_from_manifest(&manifest_path).unwrap();
    let resolved = project.resolve_for_file(&generated_root, &CompileOptions::default());
    let defines = resolved
        .compile_options
        .custom_defines
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<HashMap<_, _>>();

    assert_eq!(
        normalize_test_path(&resolved.input_file),
        normalize_test_path(&generated_root)
    );
    assert_eq!(defines.get("generated").map(String::as_str), Some("true"));
    assert_eq!(
        defines.get("ENTRY_KIND").map(String::as_str),
        Some("generated")
    );
}

#[test]
fn resolve_for_copied_template_source_uses_generated_unit_root() {
    let root = temp_dir("craft-project-generated-alias");
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
root = \"src/placeholder.rn\"
",
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        "mod build_info;\nfn main() i32 { let _ = build_info.MAGIC_NUMBER; return 0; }\n",
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        "\
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    let main = b.stage_copy_package_file(\"src/main.rn\", \"src/main.rn\");
    let _ = b.stage_generated(
        \"src/build_info.rn\",
        \"pub const MAGIC_NUMBER = i32.{42};\\n\"
    );
    b.set_source_root_from(main);
    b.cfg_bool(\"generated\", true);
}
",
    )
    .unwrap();

    analysis_context::sync_project_analysis_context(&root.join("Craft.toml"), true, &[]).unwrap();

    let project = AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
    let resolved = project.resolve_for_file(&root.join("src/main.rn"), &CompileOptions::default());
    let generated_main = root
        .join(".craft")
        .join("build")
        .join("dev")
        .join("target")
        .join("gen")
        .join("app-0.1.0")
        .join("bin")
        .join("app")
        .join("src")
        .join("main.rn");
    let generated_info = generated_main.parent().unwrap().join("build_info.rn");

    assert_eq!(
        normalize_test_path(&resolved.input_file),
        normalize_test_path(&generated_main)
    );
    let normalized_aliases = normalize_test_alias_map(&resolved.source_path_aliases);
    assert_eq!(
        normalized_aliases.get(&normalize_test_path(&root.join("src/main.rn"))),
        Some(&normalize_test_path(&generated_main))
    );
    assert!(generated_info.is_file());
    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("generated")
            .map(String::as_str),
        Some("true")
    );
}

#[test]
fn explicit_feature_selection_overrides_persisted_analysis_context() {
    let root = temp_dir("craft-project-explicit-overrides-persisted");
    fs::create_dir_all(root.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.6.7\"

[features]
experimental = []
stable = []

[[bin]]
name = \"app\"
root = \"src/main.rn\"
",
    )
    .unwrap();
    fs::write(
        root.join("craft.rn"),
        "\
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {
    if (p.feature_enabled(\"experimental\")) {
        p.cfg_bool(\"mode_experimental\", true);
    }
    if (p.feature_enabled(\"stable\")) {
        p.cfg_bool(\"mode_stable\", true);
    }
}
",
    )
    .unwrap();
    fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();

    analysis_context::sync_project_analysis_context(
        &root.join("Craft.toml"),
        true,
        &[String::from("experimental")],
    )
    .unwrap();

    let project = AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
    let mut options = CompileOptions::default();
    options.craft_features.push("stable".to_string());
    let resolved = project.resolve_for_file(&root.join("src/main.rn"), &options);
    let defines = resolved
        .compile_options
        .custom_defines
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<HashMap<_, _>>();

    assert_eq!(defines.get("mode_experimental").map(String::as_str), None);
    assert_eq!(defines.get("mode_stable").map(String::as_str), Some("true"));
}

#[test]
fn resolve_project_manifest_path_handles_nonexistent_generated_descendant() {
    let root = temp_dir("craft-project-discover-generated");
    fs::write(
        root.join("Craft.toml"),
        "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"0.6.7\"
",
    )
    .unwrap();

    let generated_path = root
        .join(".craft")
        .join("build")
        .join("dev")
        .join("target")
        .join("gen")
        .join("app-0.1.0")
        .join("bin")
        .join("app")
        .join("src")
        .join("main.rn");

    let manifest = super::resolve_project_manifest_path(Some(&generated_path)).unwrap();
    assert_eq!(manifest, root.join("Craft.toml"));
}
