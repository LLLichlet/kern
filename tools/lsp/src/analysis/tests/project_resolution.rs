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
        "[workspace]\nname = \"workspace\"\nmembers = [\"app\", \"util\"]\n",
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
root = \"src/lib.kn\"

[dependencies]
util = {{ path = \"../util\" }}
"
        ),
    )
    .unwrap();
    fs::write(app_dir.join("src/lib.kn"), "mod sub;\n").unwrap();
    fs::write(app_dir.join("src/sub.kn"), "fn local() i32 { return 1; }\n").unwrap();
    fs::write(
        util_dir.join("Craft.toml"),
        format!(
            "\
[package]
name = \"util\"
version = \"0.1.0\"
kern = \"{CURRENT_KERN_VERSION}\"

[lib]
root = \"src/lib.kn\"
"
        ),
    )
    .unwrap();
    fs::write(
        util_dir.join("src/lib.kn"),
        "fn helper() i32 { return 1; }\n",
    )
    .unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&app_dir.join("src/sub.kn")).unwrap();
    let source = fs::read_to_string(app_dir.join("src/sub.kn")).unwrap();

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
        super::normalize_path(&app_dir.join("src/lib.kn"))
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
        Some(super::normalize_path(&util_dir.join("src/lib.kn")))
    );
    assert!(resolved.compile_options.module_aliases.contains_key("std"));
}

#[test]
fn workspace_source_refresh_keeps_project_cache_but_reloads_driver_cache() {
    let root = unique_temp_dir("analysis_source_refresh_cache");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        format!(
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"{CURRENT_KERN_VERSION}\"

[lib]
root = \"src/lib.kn\"
"
        ),
    )
    .unwrap();
    fs::write(root.join("src/lib.kn"), "fn value() i32 { return 1; }\n").unwrap();

    let uri = file_path_to_uri(&root.join("src/lib.kn")).unwrap();
    let source = fs::read_to_string(root.join("src/lib.kn")).unwrap();
    let mut analysis = AnalysisEngine::default();
    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source,
        },
    });
    let _ = analysis.hover(
        &uri,
        Position {
            line: 0,
            character: 3,
        },
    );

    assert_eq!(analysis.cached_project_count(), 1);
    assert_eq!(analysis.cached_driver_count(), 1);

    let targets = analysis.refresh_workspace_targets();

    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].0, uri);
    assert_eq!(targets[0].1, DiagnosticsAnalysisMode::Full);
    assert_eq!(analysis.cached_project_count(), 1);
    assert_eq!(analysis.cached_driver_count(), 0);
}

#[test]
fn project_metadata_reload_clears_project_and_driver_caches() {
    let root = unique_temp_dir("analysis_project_reload_cache");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        format!(
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"{CURRENT_KERN_VERSION}\"

[lib]
root = \"src/lib.kn\"
"
        ),
    )
    .unwrap();
    fs::write(root.join("src/lib.kn"), "fn value() i32 { return 1; }\n").unwrap();

    let uri = file_path_to_uri(&root.join("src/lib.kn")).unwrap();
    let source = fs::read_to_string(root.join("src/lib.kn")).unwrap();
    let mut analysis = AnalysisEngine::default();
    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source,
        },
    });
    let _ = analysis.hover(
        &uri,
        Position {
            line: 0,
            character: 3,
        },
    );

    assert_eq!(analysis.cached_project_count(), 1);
    assert_eq!(analysis.cached_driver_count(), 1);

    let targets = analysis.reload_project_metadata_targets();

    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].0, uri);
    assert_eq!(targets[0].1, DiagnosticsAnalysisMode::Full);
    assert_eq!(analysis.cached_project_count(), 0);
    assert_eq!(analysis.cached_driver_count(), 0);
}

#[test]
fn watched_file_paths_classify_project_metadata() {
    let root = unique_temp_dir("analysis_watched_file_classification");
    let source_uri = file_path_to_uri(&root.join("src/lib.kn")).unwrap();
    let manifest_uri = file_path_to_uri(&root.join("Craft.toml")).unwrap();
    let lock_uri = file_path_to_uri(&root.join("Craft.lock")).unwrap();
    let analysis_context_uri = file_path_to_uri(&root.join(".craft/analysis.toml")).unwrap();

    assert!(!AnalysisEngine::watched_files_require_project_reload(&[
        source_uri
    ]));
    assert!(AnalysisEngine::watched_files_require_project_reload(&[
        manifest_uri
    ]));
    assert!(AnalysisEngine::watched_files_require_project_reload(&[
        lock_uri
    ]));
    assert!(AnalysisEngine::watched_files_require_project_reload(&[
        analysis_context_uri
    ]));
}

#[test]
fn resolve_analysis_reports_invalid_craft_manifest() {
    let root = unique_temp_dir("analysis_invalid_manifest");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("Craft.toml"), "not valid craft toml").unwrap();
    let source = "fn helper() i32 { return 1; }\n";
    fs::write(root.join("src/main.kn"), source).unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&root.join("src/main.kn")).unwrap();
    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: source.to_string(),
        },
    });

    let err = analysis.resolve_analysis(&uri).unwrap_err();
    assert!(
        err.contains("failed to resolve Craft project for LSP analysis"),
        "{err}"
    );

    let outcome = analysis.analyze_document_uri(&uri);
    let diagnostic = &outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap()
        .diagnostics[0];
    assert!(
        diagnostic.message.contains("analysis failed"),
        "{diagnostic:?}"
    );
    assert!(diagnostic.message.contains("Craft.toml"), "{diagnostic:?}");
}

#[test]
fn resolve_analysis_uses_craft_sdk_for_package_script_roots() {
    let root = unique_temp_dir("analysis_craft_script");
    let app_dir = root.join("app");
    fs::create_dir_all(app_dir.join("src")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        "[workspace]\nname = \"workspace\"\nmembers = [\"app\"]\n",
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
root = \"src/lib.kn\"
"
        ),
    )
    .unwrap();
    fs::write(app_dir.join("src/lib.kn"), "pub fn helper() void {}\n").unwrap();
    fs::write(
        app_dir.join("build.kn"),
        "use craft.builder;\npub fn build(b: &mut builder.Builder) void { let _ = b; }\n",
    )
    .unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&app_dir.join("build.kn")).unwrap();
    let source = fs::read_to_string(app_dir.join("build.kn")).unwrap();

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
        super::normalize_path(&app_dir.join("build.kn"))
    );
    assert!(
        resolved
            .compile_options
            .module_aliases
            .contains_key("craft")
    );
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
root = \"src/main.kn\"
"
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/main.kn"),
        "use std.io;\n\nfn main() i32 {\n    \"Hello Kern!\".println();\n    return 0;\n}\n",
    )
    .unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&root.join("src/main.kn")).unwrap();
    let source = fs::read_to_string(root.join("src/main.kn")).unwrap();

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
        super::normalize_path(&root.join("src/main.kn"))
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
fn bin_submodule_analysis_uses_bin_root_for_current_package_imports() {
    let root = unique_temp_dir("analysis_bin_submodule_root_import");
    fs::create_dir_all(root.join("src/mem")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        format!(
            "\
[package]
name = \"kernel\"
version = \"0.1.0\"
kern = \"{CURRENT_KERN_VERSION}\"

[[bin]]
name = \"kernel\"
root = \"src/main.kn\"
"
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/main.kn"),
        "pub mod lock;\npub mod mem;\nfn main() i32 { return 0; }\n",
    )
    .unwrap();
    fs::write(
        root.join("src/lock.kn"),
        "pub struct SpinLock {};\npub const SPIN_UNLOCKED = SpinLock.{}\n",
    )
    .unwrap();
    fs::write(root.join("src/mem/mod.kn"), "mod bitmap;\n").unwrap();
    let bitmap_source = "use /lock.{SpinLock, SPIN_UNLOCKED};\n";
    fs::write(root.join("src/mem/bitmap.kn"), bitmap_source).unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&root.join("src/mem/bitmap.kn")).unwrap();
    let outcome = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: bitmap_source.to_string(),
        },
    });

    let resolved = analysis.resolve_analysis(&uri).unwrap();
    assert_eq!(
        super::normalize_path(&resolved.input_file),
        super::normalize_path(&root.join("src/main.kn"))
    );

    let bundle = outcome
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .unwrap();
    assert!(bundle.diagnostics.iter().all(|diagnostic| {
        !diagnostic
            .message
            .contains("Unresolved import: cannot find module `lock`")
    }));
}

#[test]
fn resolve_analysis_applies_build_cfg_and_define_values() {
    let root = unique_temp_dir("analysis_craft_cfg_define");
    fs::create_dir_all(root.join("src")).unwrap();

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

[[bin]]
name = \"my_app\"
root = \"src/main.kn\"
"
        ),
    )
    .unwrap();
    fs::write(
        root.join("build.kn"),
        "\
use craft.builder;

pub fn build(b: &mut builder.Builder) void {{
    if (b.feature_enabled(\"experimental\")) {{
        b.cfg_bool(\"enable_telemetry\", true);
        b.define_string(\"GREETING_MSG\", \"Hello from the experimental future!\");
    }}
}}
",
    )
    .unwrap();
    fs::write(
        root.join("src/main.kn"),
        "\
use std.io;

#[if(enable_telemetry)]
fn init_telemetry() void {
    \"[Telemetry] Enabled\".println();
}

fn main() i32 {
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
    let uri = file_path_to_uri(&root.join("src/main.kn")).unwrap();
    let source = fs::read_to_string(root.join("src/main.kn")).unwrap();

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

[[bin]]
name = \"my_app\"
root = \"src/main.kn\"
"
        ),
    )
    .unwrap();
    fs::write(
        root.join("build.kn"),
        "\
use craft.builder;

pub fn build(b: &mut builder.Builder) void {{
    if (b.feature_enabled(\"experimental\")) {{
        b.cfg_bool(\"enable_telemetry\", true);
        b.define_string(\"GREETING_MSG\", \"Hello from the experimental future!\");
    }}
}}
",
    )
    .unwrap();
    fs::write(
        root.join("src/main.kn"),
        "\
use std.io;

#[if(enable_telemetry)]
fn init_telemetry() void {
    \"[Telemetry] Enabled\".println();
}

#[if(!enable_telemetry)]
fn init_telemetry() void {
    \"[Telemetry] Disabled\".println();
}

fn main() i32 {
    init_telemetry();
    let _ = GREETING_MSG;
    return 0;
}
",
    )
    .unwrap();

    analysis_context::sync_project_analysis_context(
        &root.join("Craft.toml"),
        true,
        &[String::from("experimental")],
    )
    .unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&root.join("src/main.kn")).unwrap();
    let source = fs::read_to_string(root.join("src/main.kn")).unwrap();

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
root = \"src/placeholder.kn\"
"
        ),
    )
    .unwrap();
    fs::write(
        root.join("build.kn"),
        "\
use craft.builder;

pub fn build(b: &mut builder.Builder) void {
    let generated = b.emit_generated(
        \"src/main.kn\",
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
        .join("main.kn");
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
root = \"src/placeholder.kn\"
"
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/main.kn"),
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
        root.join("build.kn"),
        "\
use craft.builder;

pub fn build(b: &mut builder.Builder) void {
    let main = b.stage_copy_package_file(\"src/main.kn\", \"src/main.kn\");
    let _ = b.stage_generated(
        \"src/build_info.kn\",
        \"pub const MAGIC_NUMBER = 42i32;\\n\"
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
        .join("main.kn");
    assert!(generated_main.is_file());
    assert!(
        generated_main
            .parent()
            .unwrap()
            .join("build_info.kn")
            .is_file()
    );

    let uri = file_path_to_uri(&root.join("src/main.kn")).unwrap();
    let source = fs::read_to_string(root.join("src/main.kn")).unwrap();

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
        dbg_dir.join("mod.kn"),
        "mod option;\nmod result;\npub use .option.Option;\npub use .result.Result;\n",
    )
    .unwrap();
    fs::write(
        dbg_dir.join("option.kn"),
        "pub enum Option[T] { Some: T, None }\n",
    )
    .unwrap();
    fs::write(
        dbg_dir.join("result.kn"),
        "use ..Option;\npub enum Result[T] { Ok: T, Err: Option[T] }\n",
    )
    .unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&dbg_dir.join("result.kn")).unwrap();
    let source = fs::read_to_string(dbg_dir.join("result.kn")).unwrap();

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
        super::normalize_path(&dbg_dir.join("mod.kn"))
    );
}

#[test]
fn standalone_submodule_analysis_does_not_treat_parent_import_as_root_error() {
    let root = unique_temp_dir("analysis_parent_import");
    let dbg_dir = root.join("dbg");
    fs::create_dir_all(&dbg_dir).unwrap();

    fs::write(
        dbg_dir.join("mod.kn"),
        "mod option;\nmod result;\npub use .option.Option;\npub use .result.Result;\n",
    )
    .unwrap();
    fs::write(
        dbg_dir.join("option.kn"),
        "pub enum Option[T] { Some: T, None }\n",
    )
    .unwrap();
    let result_source = "use ..Option;\npub enum Result[T] { Ok: T, Err: Option[T] }\n";
    fs::write(dbg_dir.join("result.kn"), result_source).unwrap();

    let mut analysis = AnalysisEngine::default();
    let uri = file_path_to_uri(&dbg_dir.join("result.kn")).unwrap();
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

#[test]
fn code_lenses_return_craft_build_and_test_targets() {
    let root = unique_temp_dir("analysis_code_lens_targets");
    let src_dir = root.join("src");
    let tests_dir = root.join("tests");
    fs::create_dir_all(&src_dir).unwrap();
    fs::create_dir_all(&tests_dir).unwrap();
    fs::write(
        root.join("Craft.toml"),
        format!(
            "\
[package]
name = \"app\"
version = \"0.1.0\"
kern = \"{CURRENT_KERN_VERSION}\"

[lib]
root = \"src/lib.kn\"

[test]
roots = [\"tests/smoke.kn\"]
"
        ),
    )
    .unwrap();
    let lib_source = "pub fn value() i32 { return 1; }\n";
    fs::write(src_dir.join("lib.kn"), lib_source).unwrap();
    let test_source = "use app.value;\nfn test_smoke() void { let _ = value(); }\n";
    fs::write(tests_dir.join("smoke.kn"), test_source).unwrap();

    let mut analysis = AnalysisEngine::default();
    let lib_uri = file_path_to_uri(&src_dir.join("lib.kn")).unwrap();
    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: lib_uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: lib_source.to_string(),
        },
    });
    let test_uri = file_path_to_uri(&tests_dir.join("smoke.kn")).unwrap();
    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: test_uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: test_source.to_string(),
        },
    });

    let lib_lenses = analysis.code_lenses(&lib_uri).unwrap();
    assert_eq!(lib_lenses.len(), 1, "{lib_lenses:#?}");
    assert_eq!(lib_lenses[0].title, "Build lib");
    assert_eq!(lib_lenses[0].command, "kern.craft.buildPackage");
    assert_eq!(
        lib_lenses[0].arguments[0]["manifestPath"].as_str().unwrap(),
        root.join("Craft.toml").to_string_lossy().as_ref()
    );
    assert_eq!(lib_lenses[0].arguments[0]["targetKind"], "lib");

    let test_lenses = analysis.code_lenses(&test_uri).unwrap();
    assert_eq!(test_lenses.len(), 1, "{test_lenses:#?}");
    assert_eq!(test_lenses[0].title, "Run Test smoke");
    assert_eq!(test_lenses[0].command, "kern.craft.testTarget");
    assert_eq!(test_lenses[0].arguments[0]["targetName"], "smoke");
}
