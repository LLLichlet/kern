use super::*;

#[test]
fn applies_build_script_link_directives_per_unit() {
    let root = temp_dir("craft-build-plan-script");
    let os_variant = os_variant_name(crate::script::host_target().os);
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.0"

[features]
default = ["simd"]
simd = []

[[bin]]
name = "demo"
root = "src/main.rn"

[test]
roots = ["tests/smoke.rn"]
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        format!(
            r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {{
if (b.feature_enabled("simd")) {{
    b.link_arg("-flto");
}}

if (b.target.os == .{os_variant}) {{
    b.link_arg("-Dtarget-os-match");
}}

if (b.unit.kind == .bin) {{
    b.link_framework("Security");
}}

if (b.unit.kind == .test) {{
    b.link_search("native/test");
}}
}}
"#
        ),
    )
    .unwrap();

    let manifest_path = root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path).unwrap();
    let build_elaboration = plan(
        &manifest_path,
        &manifest,
        &[],
        false,
        crate::script::ScriptCommand::Build,
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let test_elaboration = plan(
        &manifest_path,
        &manifest,
        &[],
        false,
        crate::script::ScriptCommand::Test,
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&build_elaboration, crate::script::ScriptCommand::Build).unwrap();
    let build_actions = build_plan.derive_actions(&crate::script::host_target());
    let build_package = build_plan
        .packages
        .iter()
        .find(|package| package.package_id.name == "demo")
        .unwrap();
    let test_plan = derive(&test_elaboration, crate::script::ScriptCommand::Test).unwrap();
    let test_actions = test_plan.derive_actions(&crate::script::host_target());
    let test_package = test_plan
        .packages
        .iter()
        .find(|package| package.package_id.name == "demo")
        .unwrap();
    assert_eq!(
        build_package
            .build_script
            .as_ref()
            .map(|script| script.relative_path.as_str()),
        Some("build.rn")
    );

    let bin = build_package
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();
    assert!(bin.link.args.iter().any(|arg| arg == "-flto"));
    assert!(bin.link.args.iter().any(|arg| arg == "-Dtarget-os-match"));
    assert!(bin.link.frameworks.iter().any(|name| name == "Security"));

    let test = test_package
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Test)
        .unwrap();
    assert!(test.link.args.iter().any(|arg| arg == "-flto"));
    assert!(
        test.link
            .search_paths
            .iter()
            .any(|path| path == "native/test")
    );
    let bin_action = build_actions
        .link_actions
        .iter()
        .find(|action| action.package_id.name == "demo" && action.target_kind == TargetKind::Bin)
        .unwrap();
    assert!(
        bin_action
            .link
            .frameworks
            .iter()
            .any(|name| name == "Security")
    );
    assert!(bin_action.link.args.iter().any(|arg| arg == "-flto"));
    let test_action = test_actions
        .link_actions
        .iter()
        .find(|action| action.package_id.name == "demo" && action.target_kind == TargetKind::Test)
        .unwrap();
    assert!(
        test_action
            .link
            .search_paths
            .iter()
            .any(|path| path == "native/test")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_can_resolve_relative_link_arg_paths_from_package_root() {
    let root = temp_dir("craft-build-plan-link-arg");
    fs::create_dir_all(root.join("link")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.0"

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
    b.link_arg_path("-T", "link/kernel.ld");
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("link").join("kernel.ld"),
        "ENTRY(_start)\nSECTIONS { .text : { *(.text .text.*) } }\n",
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
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();
    let expected = root
        .join("link")
        .join("kernel.ld")
        .to_string_lossy()
        .replace('\\', "/");

    assert_eq!(unit.link.args.get(0).map(String::as_str), Some("-T"));
    assert_eq!(
        unit.link.args.get(1).map(String::as_str),
        Some(expected.as_str())
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_can_generate_sources_and_mutate_unit_cfg_define() {
    let root = temp_dir("craft-build-plan-generated");
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.0"

[[bin]]
name = "demo"
root = "src/placeholder.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let root = b.emit_generated(
    "src/main.rn",
    "fn main() i32 { return 0; }\n"
);
b.set_source_root(root);
b.cfg_bool("generated", true);
b.define_string("entry", "generated");
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
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();
    let unit_nodes = build_plan.compile_input_nodes_for_unit(unit);

    let SourceRootBinding::AbsolutePath(source_root) = &unit.source_root else {
        panic!("expected generated source root to be an absolute path binding");
    };
    assert!(
        Path::new(source_root).is_absolute(),
        "expected generated source root to be absolute: {}",
        source_root
    );
    assert!(!Path::new(source_root).exists());
    assert_eq!(
        unit.cfg.get("generated"),
        Some(&crate::plan::PlanValue::Bool(true))
    );
    assert_eq!(
        unit.define.get("entry"),
        Some(&crate::plan::PlanValue::String("generated".to_string()))
    );
    assert_eq!(unit.generated_files.len(), 1);
    assert_eq!(unit.generated_files[0].origin, GeneratedFileOrigin::Emitted);
    assert_eq!(unit_nodes.len(), 1);
    assert!(matches!(
        &unit_nodes[0].kind,
        StagedActionKind::WriteFile { .. }
    ));
    assert_eq!(unit_nodes[0].phase, StagedActionPhase::PreCompile);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_can_resolve_and_stage_declared_resources() {
    let root = temp_dir("craft-build-plan-resource");
    fs::create_dir_all(root.join("vendor/limine/cfg")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "kernel"
version = "0.1.0"
kern = "0.7.0"

[[bin]]
name = "kernel"
root = "src/main.rn"

[resources]
limine = { path = "vendor/limine" }
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    let resource_root = b.resource_root("limine");
    let cfg = b.resource_path("limine", "cfg/limine.conf");
    b.define_string("LIMINE_ROOT", resource_root);
    b.link_arg_path("-T", cfg);
    let _ = b.copy_resource_file_to_artifact("limine", "cfg/limine.conf", "boot/limine.conf");
}
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
    fs::write(root.join("vendor/limine/cfg/limine.conf"), "TIMEOUT=0\n").unwrap();

    let manifest_path = root.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path).unwrap();
    let elaboration = plan(
        &manifest_path,
        &manifest,
        &[],
        false,
        crate::script::ScriptCommand::Build,
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();

    let resource_root = match unit.define.get("LIMINE_ROOT") {
        Some(crate::plan::PlanValue::String(value)) => value.as_str(),
        other => panic!("expected LIMINE_ROOT define, got {other:?}"),
    };
    assert!(resource_root.contains("/.craft/resources/"));
    assert!(resource_root.ends_with("/limine"));
    assert_eq!(unit.link.args.first().map(String::as_str), Some("-T"));
    assert!(unit.link.args[1].ends_with("/cfg/limine.conf"));

    let unit_nodes = build_plan.artifact_output_nodes_for_unit(unit);
    assert_eq!(unit_nodes.len(), 1);
    assert!(unit_nodes[0].output.ends_with("boot/limine.conf"));
    assert!(matches!(
        &unit_nodes[0].kind,
        StagedActionKind::CopyFile { source } if source.ends_with("/cfg/limine.conf")
    ));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_can_copy_package_files_into_generated_root() {
    let root = temp_dir("craft-build-plan-copy");
    fs::create_dir_all(root.join("templates")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.0"

[[bin]]
name = "demo"
root = "src/placeholder.rn"
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
let root = b.copy_package_file("templates/main.rn", "src/main.rn");
b.set_source_root(root);
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
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();
    let unit_nodes = build_plan.compile_input_nodes_for_unit(unit);

    assert_eq!(unit.generated_files.len(), 1);
    assert_eq!(
        unit.generated_files[0].origin,
        GeneratedFileOrigin::Copied {
            source: "templates/main.rn".to_string()
        }
    );
    assert_eq!(unit_nodes.len(), 1);
    assert!(matches!(
        &unit_nodes[0].kind,
        StagedActionKind::CopyFile { source } if source == "templates/main.rn"
    ));
    assert_eq!(unit_nodes[0].phase, StagedActionPhase::PreCompile);
    let SourceRootBinding::AbsolutePath(source_root) = &unit.source_root else {
        panic!("expected copied generated source root to be an absolute path binding");
    };
    assert!(!Path::new(source_root).exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_can_model_explicit_staged_dependencies() {
    let root = temp_dir("craft-build-plan-staged-deps");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.0"

[[bin]]
name = "demo"
root = "src/placeholder.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let helper = b.stage_generated("tmp/value.txt", "41\n");
let source = b.stage_generated("src/main.rn", "fn main() i32 { return 0; }\n");
b.depend(source, helper);
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
        crate::script::ScriptCommand::Build,
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();
    let unit_nodes = build_plan.compile_input_nodes_for_unit(unit);

    assert_eq!(unit_nodes.len(), 2);
    let helper = unit_nodes
        .iter()
        .find(|action| action.output.ends_with("tmp/value.txt"))
        .unwrap();
    let source = unit_nodes
        .iter()
        .find(|action| action.output.ends_with("src/main.rn"))
        .unwrap();
    assert_eq!(source.depends_on, vec![helper.id]);
    assert!(matches!(
        &unit.source_root,
        SourceRootBinding::BuildOutput { id, path }
            if *id == source.id && path.replace('\\', "/").ends_with("src/main.rn")
    ));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_can_stage_post_link_artifact_outputs() {
    let root = temp_dir("craft-build-plan-post-link");
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.0"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
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
        crate::script::ScriptCommand::Build,
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();
    let link_action = action_plan
        .link_actions
        .iter()
        .find(|action| action.package_id.name == "demo" && action.target_kind == TargetKind::Bin)
        .unwrap();
    let unit_nodes = build_plan.artifact_output_nodes_for_unit(unit);
    let link_nodes = action_plan.artifact_output_nodes_for_link_action(link_action);

    assert_eq!(unit_nodes.len(), 2);
    assert!(
        unit_nodes
            .iter()
            .all(|action| action.phase == StagedActionPhase::PostLink)
    );
    assert_eq!(link_nodes.len(), 2);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_can_stage_post_link_directory_copies() {
    let root = temp_dir("craft-build-plan-post-link-dir");
    fs::create_dir_all(root.join("assets").join("images")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.0"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
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
        crate::script::ScriptCommand::Build,
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();
    let link_action = action_plan
        .link_actions
        .iter()
        .find(|action| action.package_id.name == "demo" && action.target_kind == TargetKind::Bin)
        .unwrap();
    let unit_nodes = build_plan.artifact_output_nodes_for_unit(unit);
    let link_nodes = action_plan.artifact_output_nodes_for_link_action(link_action);

    assert_eq!(unit_nodes.len(), 1);
    assert!(matches!(
        &unit_nodes[0].kind,
        StagedActionKind::CopyDirectory { source } if source == "assets"
    ));
    assert_eq!(unit_nodes[0].phase, StagedActionPhase::PostLink);
    assert_eq!(link_nodes.len(), 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_receives_host_target_and_domain_context() {
    let root = temp_dir("craft-build-plan-domain-context");
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.0"

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
b.define_string("host_arch", b.host.arch);
b.define_string("target_arch", b.target.arch);

match (b.unit.domain) {
    .host => b.link_arg("-host-unit"),
    .target => b.link_arg("-target-unit"),
}
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
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();

    assert_eq!(unit.domain, crate::graph::BuildDomain::Target);
    assert_eq!(
        unit.define.get("host_arch"),
        Some(&crate::plan::PlanValue::String(
            crate::script::host_target().arch.to_string()
        ))
    );
    assert_eq!(
        unit.define.get("target_arch"),
        Some(&crate::plan::PlanValue::String(
            crate::script::host_target().arch.to_string()
        ))
    );
    assert!(unit.link.args.iter().any(|arg| arg == "-target-unit"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_roots_use_absolute_paths_for_workspace_root_package() {
    let root = temp_dir("craft-build-plan-root-paths");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.0"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
b.define_string("package_root", b.package.root);
b.define_string("workspace_root", b.workspace.root);
b.link_search(b.package.root);
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
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let unit = build_plan.packages[0]
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();
    let root_display = root.to_string_lossy().replace('\\', "/");

    assert_eq!(
        unit.define.get("package_root"),
        Some(&crate::plan::PlanValue::String(root_display.clone()))
    );
    assert_eq!(
        unit.define.get("workspace_root"),
        Some(&crate::plan::PlanValue::String(root_display.clone()))
    );
    assert!(
        unit.link
            .search_paths
            .iter()
            .any(|path| path == &root_display)
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_roots_use_absolute_paths_for_workspace_members() {
    let root = temp_dir("craft-build-plan-member-paths");
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
kern = "0.7.0"

[[bin]]
name = "app"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(app_dir.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
    fs::write(
        app_dir.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
b.define_string("package_root", b.package.root);
b.define_string("workspace_root", b.workspace.root);
b.link_search(b.package.root);
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
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let package = build_plan
        .packages
        .iter()
        .find(|package| package.package_id.name == "app")
        .unwrap();
    let unit = package
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();
    let root_display = root.to_string_lossy().replace('\\', "/");
    let app_display = app_dir.to_string_lossy().replace('\\', "/");

    assert_eq!(
        unit.define.get("package_root"),
        Some(&crate::plan::PlanValue::String(app_display.clone()))
    );
    assert_eq!(
        unit.define.get("workspace_root"),
        Some(&crate::plan::PlanValue::String(root_display.clone()))
    );
    assert!(
        unit.link
            .search_paths
            .iter()
            .any(|path| path == &app_display)
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_exposes_expected_paths_for_lib_and_bin_units() {
    let root = temp_dir("craft-build-plan-path-values");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.0"

[lib]
root = "src/lib.rn"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/lib.rn"),
        "pub fn value() i32 { return 1; }\n",
    )
    .unwrap();
    fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
b.define_string("build_root", b.paths.build_root);
b.define_string("generated_root", b.paths.generated_root);
b.define_string("artifact_root", b.paths.artifact_root);
b.define_string("object_path", b.paths.object);
b.define_string("artifact_path", b.paths.artifact);
match (b.paths.metadata) {
    .{ Some: path } => b.define_string("metadata_path", path),
    .None => b.define_string("metadata_path", "<none>"),
}
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
        &crate::elaborate::FeatureSelection::default(),
    )
    .unwrap();
    let build_plan = derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let package = build_plan
        .packages
        .iter()
        .find(|package| package.package_id.name == "demo")
        .unwrap();
    let lib_unit = package
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Lib)
        .unwrap();
    let bin_unit = package
        .units
        .iter()
        .find(|unit| unit.target_kind == TargetKind::Bin)
        .unwrap();

    let expected_build_root = workspace_build_root(&root, "dev", crate::graph::BuildDomain::Target)
        .to_string_lossy()
        .replace('\\', "/");

    let expected_lib_generated = generated_root_path(
        &root,
        crate::graph::BuildDomain::Target,
        &package.package_id,
        "dev",
        TargetKind::Lib,
        "demo",
    )
    .to_string_lossy()
    .replace('\\', "/");
    let expected_bin_generated = generated_root_path(
        &root,
        crate::graph::BuildDomain::Target,
        &package.package_id,
        "dev",
        TargetKind::Bin,
        "demo",
    )
    .to_string_lossy()
    .replace('\\', "/");

    let expected_lib_stage = artifact_root_path(
        &root,
        crate::graph::BuildDomain::Target,
        &package.package_id,
        "dev",
        TargetKind::Lib,
        "demo",
    )
    .to_string_lossy()
    .replace('\\', "/");
    let expected_bin_stage = artifact_root_path(
        &root,
        crate::graph::BuildDomain::Target,
        &package.package_id,
        "dev",
        TargetKind::Bin,
        "demo",
    )
    .to_string_lossy()
    .replace('\\', "/");

    let expected_lib_object = object_path(
        &root,
        crate::graph::BuildDomain::Target,
        &package.package_id,
        "dev",
        TargetKind::Lib,
        "demo",
    )
    .to_string_lossy()
    .replace('\\', "/");
    let expected_bin_object = object_path(
        &root,
        crate::graph::BuildDomain::Target,
        &package.package_id,
        "dev",
        TargetKind::Bin,
        "demo",
    )
    .to_string_lossy()
    .replace('\\', "/");

    let expected_lib_artifact = artifact_path(
        &root,
        &crate::script::host_target(),
        crate::graph::BuildDomain::Target,
        &package.package_id,
        "dev",
        TargetKind::Lib,
        "demo",
    )
    .to_string_lossy()
    .replace('\\', "/");
    let expected_bin_artifact = artifact_path(
        &root,
        &crate::script::host_target(),
        crate::graph::BuildDomain::Target,
        &package.package_id,
        "dev",
        TargetKind::Bin,
        "demo",
    )
    .to_string_lossy()
    .replace('\\', "/");

    let expected_lib_metadata = metadata_path(
        &root,
        crate::graph::BuildDomain::Target,
        &package.package_id,
        "dev",
    )
    .to_string_lossy()
    .replace('\\', "/");

    assert_eq!(
        lib_unit.define.get("build_root"),
        Some(&crate::plan::PlanValue::String(expected_build_root.clone()))
    );
    assert_eq!(
        bin_unit.define.get("build_root"),
        Some(&crate::plan::PlanValue::String(expected_build_root))
    );
    assert_eq!(
        lib_unit.define.get("generated_root"),
        Some(&crate::plan::PlanValue::String(expected_lib_generated))
    );
    assert_eq!(
        bin_unit.define.get("generated_root"),
        Some(&crate::plan::PlanValue::String(expected_bin_generated))
    );
    assert_eq!(
        lib_unit.define.get("artifact_root"),
        Some(&crate::plan::PlanValue::String(expected_lib_stage))
    );
    assert_eq!(
        bin_unit.define.get("artifact_root"),
        Some(&crate::plan::PlanValue::String(expected_bin_stage))
    );
    assert_eq!(
        lib_unit.define.get("object_path"),
        Some(&crate::plan::PlanValue::String(expected_lib_object))
    );
    assert_eq!(
        bin_unit.define.get("object_path"),
        Some(&crate::plan::PlanValue::String(expected_bin_object))
    );
    assert_eq!(
        lib_unit.define.get("artifact_path"),
        Some(&crate::plan::PlanValue::String(expected_lib_artifact))
    );
    assert_eq!(
        bin_unit.define.get("artifact_path"),
        Some(&crate::plan::PlanValue::String(expected_bin_artifact))
    );
    assert_eq!(
        lib_unit.define.get("metadata_path"),
        Some(&crate::plan::PlanValue::String(expected_lib_metadata))
    );
    assert_eq!(
        bin_unit.define.get("metadata_path"),
        Some(&crate::plan::PlanValue::String("<none>".to_string()))
    );

    let _ = fs::remove_dir_all(root);
}
