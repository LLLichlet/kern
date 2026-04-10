use super::*;

#[test]
fn resolve_analysis_uses_workspace_package_root_and_local_aliases() {
    let root = unique_temp_dir("analysis_workspace");
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
        format!(
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"{CURRENT_KERN_VERSION}\"

[lib]
root = \"src/lib.rn\"

[dependencies]
util = {{ path = \"../util\" }}
"
        ),
    )
    .unwrap();
    fs::write(app_dir.join("src/lib.rn"), "mod sub;\n").unwrap();
    fs::write(app_dir.join("src/sub.rn"), "fn local() i32 { return 1; }\n").unwrap();
    fs::write(
        util_dir.join("Craft.toml"),
        format!(
            "\
[package]
name = \"util\"
version = \"0.1.0\"
kern = \"{CURRENT_KERN_VERSION}\"

[lib]
root = \"src/lib.rn\"
"
        ),
    )
    .unwrap();
    fs::write(
        util_dir.join("src/lib.rn"),
        "fn helper() i32 { return 1; }\n",
    )
    .unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&app_dir.join("src/sub.rn")).unwrap();
    let source = fs::read_to_string(app_dir.join("src/sub.rn")).unwrap();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source,
        },
    });

    let resolved = analysis.resolve_analysis(&uri).unwrap();

    assert_eq!(
        super::normalize_path(&resolved.input_file),
        super::normalize_path(&app_dir.join("src/lib.rn"))
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
            .map(PathBuf::from),
        Some(super::normalize_path(&util_dir.join("src/lib.rn")))
    );
    assert!(resolved.compile_options.module_aliases.contains_key("std"));
}

#[test]
fn resolve_analysis_uses_craft_sdk_for_package_script_roots() {
    let root = unique_temp_dir("analysis_craft_script");
    let app_dir = root.join("app");
    fs::create_dir_all(app_dir.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        "[workspace]\nmembers = [\"app\"]\n",
    )
    .unwrap();
    fs::write(
        app_dir.join("Craft.toml"),
        format!(
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"{CURRENT_KERN_VERSION}\"

[lib]
root = \"src/lib.rn\"
"
        ),
    )
    .unwrap();
    fs::write(app_dir.join("src/lib.rn"), "pub fn helper() void {}\n").unwrap();
    fs::write(
        app_dir.join("craft.rn"),
        "use craft.plan;\npub fn craft(p: *mut plan.Plan) void { let _ = p; }\n",
    )
    .unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&app_dir.join("craft.rn")).unwrap();
    let source = fs::read_to_string(app_dir.join("craft.rn")).unwrap();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source,
        },
    });

    let resolved = analysis.resolve_analysis(&uri).unwrap();

    assert_eq!(
        super::normalize_path(&resolved.input_file),
        super::normalize_path(&app_dir.join("craft.rn"))
    );
    assert!(resolved
        .compile_options
        .module_aliases
        .contains_key("craft"));
}

#[test]
fn bin_only_package_with_std_import_keeps_diagnostics_local_to_the_target() {
    let root = unique_temp_dir("analysis_bin_only_std");
    fs::create_dir_all(root.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        format!(
            "\
[package]
name = \"my_app\"
version = \"0.1.0\"
kern = \"{CURRENT_KERN_VERSION}\"

[[bin]]
name = \"my_app\"
root = \"src/main.rn\"
"
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        "use std.io;\n\nfn main() i32 {\n    io.println(\"Hello Kern!\", .{});\n    return 0;\n}\n",
    )
    .unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&root.join("src/main.rn")).unwrap();
    let source = fs::read_to_string(root.join("src/main.rn")).unwrap();

    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source,
        },
    });

    let resolved = analysis.resolve_analysis(&uri).unwrap();
    assert_eq!(
        super::normalize_path(&resolved.input_file),
        super::normalize_path(&root.join("src/main.rn"))
    );
    assert!(resolved.compile_options.module_aliases.contains_key("std"));

    let target_bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    assert!(outcome.bundles.iter().all(|bundle| bundle.uri == uri));
    assert!(target_bundle.diagnostics.is_empty());
}

#[test]
fn resolve_analysis_applies_craft_cfg_and_define_values() {
    let root = unique_temp_dir("analysis_craft_cfg_define");
    fs::create_dir_all(root.join("src")).unwrap();
    let env_name = format!(
        "KERN_LSP_ANALYSIS_{}",
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
name = \"my_app\"
version = \"0.1.0\"
kern = \"{CURRENT_KERN_VERSION}\"

[features]
experimental = []

[craft]
env = [\"{env_name}\"]

[[bin]]
name = \"my_app\"
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
        p.define_string(\"GREETING_MSG\", \"Hello from the experimental future!\");
    }}

    if (p.env(\"{env_name}\") != .None) {{
        p.cfg_bool(\"is_dev_env\", true);
    }}
}}
"
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        "\
use std.io;

#[if(enable_telemetry)]
fn init_telemetry() void {
    io.println(\"[Telemetry] Enabled\", .{});
}

#[if(is_dev_env)]
fn print_env_mode() void {
    io.println(\"[Mode] Development\", .{});
}

fn main() i32 {
    print_env_mode();
    init_telemetry();
    let _ = GREETING_MSG;
    return 0;
}
",
    )
    .unwrap();

    let mut options = CompileOptions {
        library_bundle: LibraryBundle::Std,
        ..CompileOptions::default()
    };
    options.craft_features.push("experimental".to_string());
    let mut analysis = AnalysisEngine::new(AnalysisSettings {
        compile_options: options,
    });
    let uri = file_path_to_uri(&root.join("src/main.rn")).unwrap();
    let source = fs::read_to_string(root.join("src/main.rn")).unwrap();

    let outcome = with_env_var(&env_name, "1", || {
        analysis.open_document(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                _language_id: "kern".to_string(),
                version: 1,
                text: source,
            },
        })
    });
    let resolved = with_env_var(&env_name, "1", || analysis.resolve_analysis(&uri)).unwrap();

    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("enable_telemetry")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("is_dev_env")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("GREETING_MSG")
            .map(String::as_str),
        Some("Hello from the experimental future!")
    );

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    assert!(
        bundle.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        bundle.diagnostics
    );
}

#[test]
fn resolve_analysis_uses_persisted_craft_analysis_context_by_default() {
    let root = unique_temp_dir("analysis_persisted_craft_ctx");
    fs::create_dir_all(root.join("src")).unwrap();
    let env_name = format!(
        "KERN_LSP_PERSISTED_{}",
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
name = \"my_app\"
version = \"0.1.0\"
kern = \"{CURRENT_KERN_VERSION}\"

[features]
experimental = []

[craft]
env = [\"{env_name}\"]

[[bin]]
name = \"my_app\"
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
        p.define_string(\"GREETING_MSG\", \"Hello from the experimental future!\");
    }}

    if (p.env(\"{env_name}\") != .None) {{
        p.cfg_bool(\"is_dev_env\", true);
    }}
}}
"
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        "\
use std.io;

#[if(enable_telemetry)]
fn init_telemetry() void {
    io.println(\"[Telemetry] Enabled\", .{});
}

#[if(!enable_telemetry)]
fn init_telemetry() void {
    io.println(\"[Telemetry] Disabled\", .{});
}

#[if(is_dev_env)]
fn print_env_mode() void {
    io.println(\"[Mode] Development\", .{});
}

#[if(!is_dev_env)]
fn print_env_mode() void {
}

fn main() i32 {
    print_env_mode();
    init_telemetry();
    let _ = GREETING_MSG;
    return 0;
}
",
    )
    .unwrap();

    with_env_var(&env_name, "1", || {
        analysis_context::sync_project_analysis_context(
            &root.join("Craft.toml"),
            true,
            &[String::from("experimental")],
        )
        .unwrap()
    });

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&root.join("src/main.rn")).unwrap();
    let source = fs::read_to_string(root.join("src/main.rn")).unwrap();

    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source,
        },
    });
    let resolved = analysis.resolve_analysis(&uri).unwrap();

    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("enable_telemetry")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("is_dev_env")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("GREETING_MSG")
            .map(String::as_str),
        Some("Hello from the experimental future!")
    );

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    assert!(
        bundle.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        bundle.diagnostics
    );
}

#[test]
fn resolve_analysis_matches_generated_source_root_from_persisted_context() {
    let root = unique_temp_dir("analysis_persisted_generated_root");

    fs::write(
        root.join("Craft.toml"),
        format!(
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"{CURRENT_KERN_VERSION}\"

[[bin]]
name = \"app\"
root = \"src/placeholder.rn\"
"
        ),
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        "\
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    let generated = b.emit_generated(
        \"src/main.rn\",
        \"#[if(generated)]\\nfn main() i32 { let _ = ENTRY_KIND; return 0; }\\n\"
    );
    b.set_source_root(generated);
    b.cfg_bool(\"generated\", true);
    b.define_string(\"ENTRY_KIND\", \"generated\");
}
",
    )
    .unwrap();

    analysis_context::sync_project_analysis_context(&root.join("Craft.toml"), true, &[]).unwrap();

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
    let project =
        craft::project::AnalysisProject::load_from_manifest(&root.join("Craft.toml")).unwrap();
    let direct = project.resolve_for_file(&generated_path, &CompileOptions::default());
    assert_eq!(
        direct
            .compile_options
            .custom_defines
            .get("generated")
            .map(String::as_str),
        Some("true")
    );
    let uri = file_path_to_uri(&generated_path).unwrap();
    let source = "#[if(generated)]\nfn main() i32 { let _ = ENTRY_KIND; return 0; }\n";

    let mut analysis = AnalysisEngine::default();
    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });
    let resolved = analysis.resolve_analysis(&uri).unwrap();

    assert_eq!(
        super::normalize_path(&resolved.input_file),
        super::normalize_path(&generated_path)
    );
    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("generated")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("ENTRY_KIND")
            .map(String::as_str),
        Some("generated")
    );

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    assert!(
        bundle.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        bundle.diagnostics
    );
}

#[test]
fn resolve_analysis_maps_copied_template_sources_into_generated_root() {
    let root = unique_temp_dir("analysis_persisted_generated_alias");
    fs::create_dir_all(root.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        format!(
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"{CURRENT_KERN_VERSION}\"

[[bin]]
name = \"app\"
root = \"src/placeholder.rn\"
"
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        "\
mod build_info;

#[if(generated)]
fn main() i32 {
    let _ = build_info.MAGIC_NUMBER;
    return 0;
}
",
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
    assert!(generated_main.is_file());
    assert!(generated_main
        .parent()
        .unwrap()
        .join("build_info.rn")
        .is_file());

    let uri = file_path_to_uri(&root.join("src/main.rn")).unwrap();
    let source = fs::read_to_string(root.join("src/main.rn")).unwrap();

    let mut analysis = AnalysisEngine::default();
    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source,
        },
    });
    let resolved = analysis.resolve_analysis(&uri).unwrap();

    assert_eq!(
        super::normalize_path(&resolved.input_file),
        super::normalize_path(&generated_main)
    );
    assert_eq!(
        resolved
            .compile_options
            .custom_defines
            .get("generated")
            .map(String::as_str),
        Some("true")
    );

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    assert!(
        bundle.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        bundle.diagnostics
    );
}

#[test]
fn resolve_analysis_prefers_nearest_init_module_root_without_manifest() {
    let root = unique_temp_dir("analysis_init_root");
    let dbg_dir = root.join("dbg");
    fs::create_dir_all(&dbg_dir).unwrap();

    fs::write(
        dbg_dir.join("init.rn"),
        "mod option;\nmod result;\npub use .option.Option;\npub use .result.Result;\n",
    )
    .unwrap();
    fs::write(
        dbg_dir.join("option.rn"),
        "pub type Option[T] = enum { Some: T, None };\n",
    )
    .unwrap();
    fs::write(
        dbg_dir.join("result.rn"),
        "use ..Option;\npub type Result[T] = enum { Ok: T, Err: Option[T] };\n",
    )
    .unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&dbg_dir.join("result.rn")).unwrap();
    let source = fs::read_to_string(dbg_dir.join("result.rn")).unwrap();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source,
        },
    });

    let resolved = analysis.resolve_analysis(&uri).unwrap();

    assert_eq!(
        super::normalize_path(&resolved.input_file),
        super::normalize_path(&dbg_dir.join("init.rn"))
    );
}

#[test]
fn standalone_submodule_analysis_does_not_treat_parent_import_as_root_error() {
    let root = unique_temp_dir("analysis_parent_import");
    let dbg_dir = root.join("dbg");
    fs::create_dir_all(&dbg_dir).unwrap();

    fs::write(
        dbg_dir.join("init.rn"),
        "mod option;\nmod result;\npub use .option.Option;\npub use .result.Result;\n",
    )
    .unwrap();
    fs::write(
        dbg_dir.join("option.rn"),
        "pub type Option[T] = enum { Some: T, None };\n",
    )
    .unwrap();
    let result_source = "use ..Option;\npub type Result[T] = enum { Ok: T, Err: Option[T] };\n";
    fs::write(dbg_dir.join("result.rn"), result_source).unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&dbg_dir.join("result.rn")).unwrap();
    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: result_source.to_string(),
        },
    });

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    assert!(bundle.diagnostics.iter().all(|diagnostic| {
        !diagnostic
            .message
            .contains("Cannot use `..` (Parent) from the root module")
    }));
}
