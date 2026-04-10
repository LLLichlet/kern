use super::{
    build, external, linker_input_paths_for_primary_output, multi_object_output_dir,
    parallel_target_compile_jobs, parallel_target_link_jobs, run, runtime_packages,
    runtime_profile_key, test, validate_package_metadata_root,
};
use crate::build_plan;
use crate::elaborate::{plan, FeatureSelection};
use crate::manifest::Manifest;
use crate::workspace;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

mod build_graph;
mod external_deps;
mod generated;
mod local_packages;
mod metadata;
mod runtime_cache;
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

fn build_release_hello_workspace(root: &Path, profile_body: &str) -> super::ExecutionSummary {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        format!(
            r#"
[package]
name = "hello"
version = "0.1.0"
kern = "0.6.7"

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
