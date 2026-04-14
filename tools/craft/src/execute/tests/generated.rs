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
kern = "0.7.0"

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
kern = "0.7.0"

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
kern = "0.7.0"

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
kern = "0.7.0"

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
kern = "0.7.0"

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
kern = "0.7.0"

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
kern = "0.7.0"

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
kern = "0.7.0"

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
