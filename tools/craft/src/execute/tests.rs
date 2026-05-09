use super::orchestrate::{build, check};
use super::runtime::{run, test};
use super::{
    external, linker_input_paths_for_primary_output, multi_linker_input_dir,
    parallel_target_compile_jobs, parallel_target_link_jobs, runtime_packages, runtime_profile_key,
    validate_package_metadata_root,
};
use crate::build_plan::{self, StagedActionKind};
use crate::elaborate::{FeatureSelection, plan};
use crate::lockfile;
use crate::manifest::Manifest;
use crate::publish_proof;
use crate::workspace;
use kernc_utils::llvm_bitcode::file_has_llvm_bitcode_magic;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod build_graph;
mod external_deps;
mod generated;
mod local_packages;
mod metadata;
mod runtime_cache;
mod simd;
mod test_targets;

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn has_llvm_bitcode_magic(path: &Path) -> bool {
    file_has_llvm_bitcode_magic(path)
}

fn symbol_dump_tool() -> String {
    for candidate in if cfg!(windows) {
        vec!["llvm-nm.exe", "nm.exe"]
    } else {
        vec!["llvm-nm", "nm"]
    } {
        if let Some(path) = find_tool_in_path(candidate) {
            return path;
        }
    }

    if cfg!(windows) {
        for candidate in [
            r"C:\Program Files\LLVM\bin\llvm-nm.exe",
            r"C:\LLVM-21\bin\llvm-nm.exe",
        ] {
            if Path::new(candidate).is_file() {
                return candidate.to_string();
            }
        }
    }

    panic!("failed to locate `llvm-nm` or `nm` in PATH");
}

fn find_tool_in_path(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

fn find_tool_in_known_locations(candidates: &[&str]) -> Option<String> {
    for candidate in candidates {
        if let Some(path) = find_tool_in_path(candidate) {
            return Some(path);
        }
    }

    if cfg!(windows) {
        for dir in [r"C:\Program Files\LLVM\bin", r"C:\LLVM-21\bin"] {
            for candidate in candidates {
                let path = Path::new(dir).join(candidate);
                if path.is_file() {
                    return Some(path.to_string_lossy().to_string());
                }
            }
        }
    }

    None
}

fn c_compiler_tool() -> String {
    if let Ok(cc) = std::env::var("CC")
        && !cc.is_empty()
    {
        return cc;
    }
    let candidates = if cfg!(windows) {
        ["cc.exe", "clang.exe", "clang-cl.exe"]
    } else {
        ["cc", "clang", "clang-cl"]
    };
    find_tool_in_known_locations(&candidates)
        .unwrap_or_else(|| if cfg!(windows) { "clang.exe" } else { "cc" }.to_string())
}

fn archive_tool() -> String {
    let candidates = if cfg!(windows) {
        ["llvm-ar.exe", "llvm-lib.exe", "ar.exe", "lib.exe"]
    } else {
        ["llvm-ar", "ar", "llvm-lib", "lib"]
    };
    find_tool_in_known_locations(&candidates)
        .unwrap_or_else(|| if cfg!(windows) { "llvm-ar.exe" } else { "ar" }.to_string())
}

fn demo_archive_name() -> &'static str {
    if cfg!(windows) {
        "demo.lib"
    } else {
        "libdemo.a"
    }
}

fn run_command_checked(command: &mut Command, label: &str) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{} failed\nstdout:\n{}\nstderr:\n{}",
        label,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn create_demo_static_library(dir: &Path) {
    create_demo_static_library_with_source(
        dir,
        r#"
int ext_add(int lhs, int rhs) {
    return lhs + rhs;
}
"#,
    );
}

fn create_demo_static_library_with_source(dir: &Path, source: &str) {
    fs::write(dir.join("demo.c"), source).unwrap();

    let mut cc = Command::new(c_compiler_tool());
    cc.arg("-c")
        .arg("demo.c")
        .arg("-o")
        .arg("demo.o")
        .current_dir(dir);
    run_command_checked(&mut cc, "cc compile demo.c");

    let mut ar = Command::new(archive_tool());
    ar.arg("rcs")
        .arg(demo_archive_name())
        .arg("demo.o")
        .current_dir(dir);
    run_command_checked(&mut ar, &format!("archive {}", demo_archive_name()));
}

fn create_named_static_library_with_source(dir: &Path, name: &str, source: &str) {
    let archive_name = if cfg!(windows) {
        format!("{name}.lib")
    } else {
        format!("lib{name}.a")
    };
    let source_name = format!("{name}.c");
    let object_name = format!("{name}.o");

    fs::write(dir.join(&source_name), source).unwrap();

    let mut cc = Command::new(c_compiler_tool());
    cc.arg("-c")
        .arg(&source_name)
        .arg("-o")
        .arg(&object_name)
        .current_dir(dir);
    run_command_checked(&mut cc, &format!("cc compile {source_name}"));

    let mut ar = Command::new(archive_tool());
    ar.arg("rcs")
        .arg(&archive_name)
        .arg(&object_name)
        .current_dir(dir);
    run_command_checked(&mut ar, &format!("archive {archive_name}"));
}

fn run_binary_with_retry(executable: &Path, expected_code: i32) -> Output {
    let mut last_output = None;
    for attempt in 0..3 {
        let output = Command::new(executable).output().unwrap();
        if output.status.code() == Some(expected_code) {
            return output;
        }
        let signaled = cfg!(unix) && output.status.code().is_none();
        last_output = Some(output);
        if !signaled || attempt == 2 {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    let output = last_output.expect("expected at least one process launch");
    panic!(
        "expected `{}` to exit with code {}, got status {:?}\nstdout:\n{}\nstderr:\n{}",
        executable.display(),
        expected_code,
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn linker_input_manifest_controls_primary_output_resolution() {
    let root = temp_dir("craft-linker-input-manifest");
    let primary_output = root.join("libfoo.o");
    let linker_input_dir = multi_linker_input_dir(&primary_output);
    fs::create_dir_all(&linker_input_dir).unwrap();

    let first = linker_input_dir.join("thin1.o");
    let second = linker_input_dir.join("thin0.o");
    let stray = linker_input_dir.join("stray.o");
    fs::write(&first, b"BC\xc0\xdeone").unwrap();
    fs::write(&second, b"BC\xc0\xdetwo").unwrap();
    fs::write(&stray, b"BC\xc0\xdestray").unwrap();
    fs::write(
        &primary_output,
        format!(
            "version=1\nlinker_input={}\nlinker_input={}\n",
            first.display(),
            second.display()
        ),
    )
    .unwrap();

    let resolved = linker_input_paths_for_primary_output(&primary_output).unwrap();
    assert_eq!(resolved, vec![first, second]);

    let _ = fs::remove_dir_all(root);
}

fn build_release_hello_workspace(root: &Path, profile_body: &str) -> super::ExecutionSummary {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        format!(
            r#"
[package]
name = "hello"
version = "0.1.0"
kern = "0.7.5"

{profile_body}

[[bin]]
name = "hello"
root = "src/main.rn"
"#
        ),
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
        &FeatureSelection {
            profile: crate::script::ProfileSelection::Release,
            ..FeatureSelection::default()
        },
    )
    .unwrap();
    let build_plan = build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    build(&build_plan, &action_plan).unwrap()
}

#[test]
fn release_thinlto_build_produces_runnable_binary() {
    let root = temp_dir("craft-release-thinlto-run");
    let _summary = build_release_hello_workspace(
        &root,
        r#"
[profile.release]
opt = 3
codegen-units = 2
lto = "thin"
"#,
    );

    let executable = root
        .join(".craft")
        .join("build")
        .join("release")
        .join("target")
        .join("out")
        .join("hello-0.1.0")
        .join("bin")
        .join("hello");
    let output = run_binary_with_retry(&executable, 0);
    assert!(output.status.success());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_succeeds_for_linux_freestanding_bin_without_program_main() {
    if cfg!(windows) || cfg!(target_os = "macos") {
        return;
    }

    let root = temp_dir("craft-build-freestanding-none");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "kernel"
version = "0.1.0"
kern = "0.7.5"

[runtime]
entry = "none"
libc = false
bundle = "base"

[[bin]]
name = "kernel"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        r#"
#[export_name("_start")]
fn kmain() void {
    while (true) {}
    @unreachable();
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
    let binary = action_plan
        .link_actions
        .iter()
        .find(|action| action.package_id.name == "kernel")
        .unwrap()
        .artifact_path
        .clone();

    let summary = build(&build_plan, &action_plan).unwrap();
    assert_eq!(summary.link_actions, 1);
    assert!(
        binary.exists(),
        "expected freestanding binary at {}",
        binary.display()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_reports_invalid_pointer_static_initializer_failure() {
    let root = temp_dir("craft-build-invalid-pointer-static-init");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.5"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        r#"
struct FramebufferRequest {
    response: *u8,
};

static REQUEST = FramebufferRequest.{ response: ?T };

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
    let err = build(&build_plan, &action_plan).unwrap_err();

    assert!(
        err.to_string().contains(&format!(
            "compile failed for `{}`",
            root.join("src/main.rn").display()
        )),
        "unexpected error: {err}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_can_attach_relative_link_arg_path_for_freestanding_bin() {
    if cfg!(windows) || cfg!(target_os = "macos") {
        return;
    }

    let root = temp_dir("craft-build-freestanding-link-script");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("link")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "kernel"
version = "0.1.0"
kern = "0.7.5"

[runtime]
entry = "none"
libc = false
bundle = "base"

[[bin]]
name = "kernel"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: &mut builder.Builder) void {
    b.link_arg_path("-T", "link/kernel.ld");
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("link").join("kernel.ld"),
        r#"
ENTRY(_start)
SECTIONS {
  . = 0x100000;
  .text : { *(.text .text.*) }
  .rodata : { *(.rodata .rodata.*) }
  .data : { *(.data .data.*) }
  .bss : { *(.bss .bss.*) *(COMMON) }
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        r#"
#[export_name("_start")]
fn kmain() void {
    while (true) {}
    @unreachable();
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
    let binary = action_plan
        .link_actions
        .iter()
        .find(|action| action.package_id.name == "kernel")
        .unwrap()
        .artifact_path
        .clone();

    let summary = build(&build_plan, &action_plan).unwrap();
    assert_eq!(summary.link_actions, 1);
    assert!(
        binary.exists(),
        "expected freestanding binary at {}",
        binary.display()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn freestanding_link_rebuilds_when_link_arg_path_contents_change() {
    if cfg!(windows) || cfg!(target_os = "macos") {
        return;
    }

    let root = temp_dir("craft-build-freestanding-link-script-rebuild");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("link")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "kernel"
version = "0.1.0"
kern = "0.7.5"

[runtime]
entry = "none"
libc = false
bundle = "base"

[[bin]]
name = "kernel"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: &mut builder.Builder) void {
    b.link_arg_path("-T", "link/kernel.ld");
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("link").join("kernel.ld"),
        r#"
ENTRY(_start)
SECTIONS {
  . = 0x100000;
  .text : { *(.text .text.*) }
  .rodata : { *(.rodata .rodata.*) }
  .data : { *(.data .data.*) }
  .bss : { *(.bss .bss.*) *(COMMON) }
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        r#"
#[export_name("_start")]
fn kmain() void {
    while (true) {}
    @unreachable();
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
    let binary = action_plan
        .link_actions
        .iter()
        .find(|action| action.package_id.name == "kernel")
        .unwrap()
        .artifact_path
        .clone();

    let first = build(&build_plan, &action_plan).unwrap();
    assert_eq!(first.link_actions, 1);
    let first_image = fs::read(&binary).unwrap();

    fs::write(
        root.join("link").join("kernel.ld"),
        r#"
ENTRY(_start)
SECTIONS {
  . = 0x200000;
  .text : { *(.text .text.*) }
  .rodata : { *(.rodata .rodata.*) }
  .data : { *(.data .data.*) }
  .bss : { *(.bss .bss.*) *(COMMON) }
}
"#,
    )
    .unwrap();

    let second = build(&build_plan, &action_plan).unwrap();
    assert_eq!(second.link_actions, 1);
    let second_image = fs::read(&binary).unwrap();
    assert_ne!(first_image, second_image);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_can_link_native_static_library_from_root_package_path() {
    let root = temp_dir("craft-build-native-link");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "native"
version = "0.1.0"
kern = "0.7.5"

[runtime]
entry = "rt"
bundle = "std"

[[bin]]
name = "native"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: &mut builder.Builder) void {
    b.link_search(b.package.root);
    b.link_system_lib("demo");
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        r#"
use std.io;

extern {
    fn ext_add(lhs: i32, rhs: i32) i32;
}

fn main() i32 {
    let value = ext_add(20, 22);
    "native={}".fmt(.{value}).println();
    if (value != 42) {
        return 1;
    }
    return 0;
}
"#,
    )
    .unwrap();
    create_demo_static_library(&root);

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
        .find(|action| action.package_id.name == "native")
        .unwrap();
    let root_display = root.to_string_lossy().replace('\\', "/");

    assert!(
        link_action
            .link
            .search_paths
            .iter()
            .any(|path| path == &root_display)
    );
    assert!(link_action.link.system_libs.iter().any(|lib| lib == "demo"));

    let _summary = build(&build_plan, &action_plan).unwrap();
    let output = run_binary_with_retry(&link_action.artifact_path, 0);
    assert_eq!(String::from_utf8_lossy(&output.stdout), "native=42\n");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_can_compile_and_link_c_source() {
    let root = temp_dir("craft-build-cc-source");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("native")).unwrap();
    fs::create_dir_all(root.join("native/include")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "native"
version = "0.1.0"
kern = "0.7.5"

[runtime]
entry = "rt"
bundle = "std"

[[bin]]
name = "native"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r##"
use craft.builder;

pub fn build(b: &mut builder.Builder) void {
    let header = b.stage_generated("demo.h", "#define EXT_OFFSET 20\n");
    let _ = b.cc_config("native/demo.c", .{
        include_dirs: .{"native/include", b.paths.generated_root},
        defines: .{"CRAFT_NATIVE_ENABLED=1"},
        args: .{},
        dependencies: .{header},
    });
}
"##,
    )
    .unwrap();
    fs::write(
        root.join("native/include/native_extra.h"),
        "#define EXT_EXTRA 0\n",
    )
    .unwrap();
    fs::write(
        root.join("native/demo.c"),
        r#"
#include "demo.h"
#include "native_extra.h"

#ifndef CRAFT_NATIVE_ENABLED
#error "expected craft C define"
#endif

int ext_add(int lhs, int rhs) {
    return lhs + rhs + EXT_OFFSET + EXT_EXTRA;
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        r#"
use std.io;

extern {
    fn ext_add(lhs: i32, rhs: i32) i32;
}

fn main() i32 {
    let value = ext_add(10, 12);
    "native={}".fmt(.{value}).println();
    if (value == 42) {
        return 0;
    }
    if (value == 49) {
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
    let link_action = action_plan
        .link_actions
        .iter()
        .find(|action| action.package_id.name == "native")
        .unwrap();

    assert!(
        link_action
            .link
            .args
            .iter()
            .any(|arg| arg.ends_with("native_demo.c.o"))
    );
    let cc_action = action_plan
        .build_nodes
        .iter()
        .find(|action| matches!(action.kind, StagedActionKind::CcCompile { .. }))
        .unwrap();
    assert_eq!(cc_action.depends_on.len(), 1);

    let summary = build(&build_plan, &action_plan).unwrap();
    assert!(summary.action_cache_stats.staged_misses >= 1);
    let output = run_binary_with_retry(&link_action.artifact_path, 0);
    assert_eq!(String::from_utf8_lossy(&output.stdout), "native=42\n");

    fs::write(
        root.join("native/include/native_extra.h"),
        "#define EXT_EXTRA 7\n",
    )
    .unwrap();
    let summary = build(&build_plan, &action_plan).unwrap();
    assert!(summary.action_cache_stats.staged_misses >= 1);
    let output = run_binary_with_retry(&link_action.artifact_path, 0);
    assert_eq!(String::from_utf8_lossy(&output.stdout), "native=49\n");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn relinks_when_project_local_native_library_changes() {
    let root = temp_dir("craft-build-native-link-rebuild");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "native"
version = "0.1.0"
kern = "0.7.5"

[runtime]
entry = "rt"
bundle = "std"

[[bin]]
name = "native"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: &mut builder.Builder) void {
    b.link_search(b.package.root);
    b.link_system_lib("demo");
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        r#"
use std.io;

extern {
    fn ext_add(lhs: i32, rhs: i32) i32;
}

fn main() i32 {
    "native={}".fmt(.{ ext_add(20, 22), }).println();
    return 0;
}
"#,
    )
    .unwrap();
    create_demo_static_library_with_source(
        &root,
        r#"
int ext_add(int lhs, int rhs) {
    return lhs + rhs;
}
"#,
    );

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
        .find(|action| action.package_id.name == "native")
        .unwrap();

    let first = build(&build_plan, &action_plan).unwrap();
    assert_eq!(first.link_actions, 1);
    let first_image = fs::read(&link_action.artifact_path).unwrap();
    let first_output = run_binary_with_retry(&link_action.artifact_path, 0);
    assert_eq!(String::from_utf8_lossy(&first_output.stdout), "native=42\n");

    create_demo_static_library_with_source(
        &root,
        r#"
int ext_add(int lhs, int rhs) {
    return lhs + rhs + 1;
}
"#,
    );

    let second = build(&build_plan, &action_plan).unwrap();
    assert_eq!(second.link_actions, 1);
    let second_image = fs::read(&link_action.artifact_path).unwrap();
    let second_output = run_binary_with_retry(&link_action.artifact_path, 0);
    assert_eq!(
        String::from_utf8_lossy(&second_output.stdout),
        "native=43\n"
    );
    assert_ne!(first_image, second_image);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_script_resolves_relative_link_search_paths_from_member_package_root() {
    let root = temp_dir("craft-build-native-member-link");
    let app_dir = root.join("app");
    let native_dir = app_dir.join("native");
    fs::create_dir_all(app_dir.join("src")).unwrap();
    fs::create_dir_all(&native_dir).unwrap();
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
kern = "0.7.5"

[runtime]
entry = "rt"
bundle = "std"

[[bin]]
name = "app"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: &mut builder.Builder) void {
    b.link_search("native");
    b.link_system_lib("demo");
}
"#,
    )
    .unwrap();
    fs::write(
        app_dir.join("src/main.rn"),
        r#"
use std.io;

extern {
    fn ext_add(lhs: i32, rhs: i32) i32;
}

fn main() i32 {
    let value = ext_add(9, 33);
    "member-native={}".fmt(.{value}).println();
    if (value != 42) {
        return 1;
    }
    return 0;
}
"#,
    )
    .unwrap();
    create_demo_static_library(&native_dir);

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
    let link_action = action_plan
        .link_actions
        .iter()
        .find(|action| action.package_id.name == "app")
        .unwrap();

    assert!(
        link_action
            .link
            .search_paths
            .iter()
            .any(|path| path == "native")
    );
    assert_eq!(link_action.package_root_path, app_dir);

    let _summary = build(&build_plan, &action_plan).unwrap();
    let output = run_binary_with_retry(&link_action.artifact_path, 0);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "member-native=42\n"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn relinks_when_project_local_link_search_directory_appears() {
    if cfg!(windows) {
        return;
    }

    let root = temp_dir("craft-link-search-dir-appears");
    let native_dir = root.join("native");
    let fallback_dir = root.join("fallback");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(&fallback_dir).unwrap();
    create_named_static_library_with_source(
        &fallback_dir,
        "craftappears",
        r#"
int craft_appears_value(void) {
    return 1;
}
"#,
    );
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.5"

[runtime]
entry = "rt"
bundle = "std"

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

pub fn build(b: &mut builder.Builder) void {
    b.link_search("native");
    b.link_search("fallback");
    b.link_system_lib("craftappears");
}
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/main.rn"),
        r#"
extern {
    fn craft_appears_value() i32;
}

fn main() i32 {
    if (craft_appears_value() != 1) {
        return 1;
    }
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
    let link_action = action_plan
        .link_actions
        .iter()
        .find(|action| action.package_id.name == "demo")
        .unwrap();

    let first = build(&build_plan, &action_plan).unwrap();
    assert_eq!(first.link_actions, 1);
    let first_output = run_binary_with_retry(&link_action.artifact_path, 0);
    assert!(first_output.status.success());

    fs::create_dir_all(&native_dir).unwrap();
    create_named_static_library_with_source(
        &native_dir,
        "craftappears",
        r#"
int craft_appears_value(void) {
    return 123;
}
"#,
    );

    let second = build(&build_plan, &action_plan).unwrap();
    assert_eq!(second.compile_actions, 0);
    assert_eq!(second.link_actions, 1);
    let second_output = Command::new(&link_action.artifact_path).output().unwrap();
    assert_eq!(second_output.status.code(), Some(1));

    let _ = fs::remove_dir_all(root);
}

fn init_git_package(repo: &Path, manifest: &str, lib_source: &str) {
    fs::create_dir_all(repo.join("src")).unwrap();
    let manifest = add_repository_to_manifest(manifest, repo);
    fs::write(repo.join("Craft.toml"), manifest).unwrap();
    fs::write(repo.join("src/lib.rn"), lib_source).unwrap();
    write_publish_artifacts(repo);
    run_git(repo, ["init", "--initial-branch=main"]);
    run_git(repo, ["config", "user.name", "Craft Tests"]);
    run_git(
        repo,
        ["config", "user.email", "craft-tests@example.invalid"],
    );
    run_git(repo, ["add", "."]);
    run_git(repo, ["commit", "-m", "initial"]);
}

fn commit_git_package(repo: &Path, message: &str) {
    write_publish_artifacts(repo);
    run_git(repo, ["add", "."]);
    run_git(repo, ["commit", "-m", message]);
}

fn write_publish_artifacts(repo: &Path) {
    let manifest_path = repo.join("Craft.toml");
    let manifest = Manifest::load(&manifest_path).unwrap();
    let elaboration = plan(
        &manifest_path,
        &manifest,
        &[],
        false,
        crate::script::ScriptCommand::Check,
        &FeatureSelection::default(),
    )
    .unwrap();
    lockfile::sync_lockfile(&manifest_path, &elaboration).unwrap();
    publish_proof::write_test_publish_proof(repo, &toml_string_literal(repo)).unwrap();
}

fn add_repository_to_manifest(manifest: &str, repo: &Path) -> String {
    if manifest.contains("repository =") {
        return manifest.to_string();
    }
    manifest.replacen(
        "kern = \"0.7.5\"",
        &format!(
            "kern = \"0.7.5\"\nrepository = \"{}\"",
            toml_string_literal(repo)
        ),
        1,
    )
}

fn toml_string_literal(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\")
}

fn run_git<const N: usize>(cwd: &Path, args: [&str; N]) {
    let output = Command::new("git")
        .args(["-c", "commit.gpgsign=false"])
        .args(["-c", "tag.gpgSign=false"])
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
