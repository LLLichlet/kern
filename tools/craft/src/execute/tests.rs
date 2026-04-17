use super::{
    build, external, linker_input_paths_for_primary_output, multi_linker_input_dir,
    parallel_target_compile_jobs, parallel_target_link_jobs, run, runtime_packages,
    runtime_profile_key, test, validate_package_metadata_root,
};
use crate::build_plan;
use crate::elaborate::{FeatureSelection, plan};
use crate::manifest::Manifest;
use crate::workspace;
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
    fs::read(path)
        .map(|bytes| bytes.starts_with(b"BC\xc0\xde"))
        .unwrap_or(false)
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
kern = "0.7.0"

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

fn init_git_package(repo: &Path, manifest: &str, lib_source: &str) {
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(repo.join("Craft.toml"), manifest).unwrap();
    fs::write(repo.join("src/lib.rn"), lib_source).unwrap();
    run_git(repo, ["init", "--initial-branch=main"]);
    run_git(repo, ["config", "user.name", "Craft Tests"]);
    run_git(
        repo,
        ["config", "user.email", "craft-tests@example.invalid"],
    );
    run_git(repo, ["add", "."]);
    run_git(repo, ["commit", "-m", "initial"]);
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
