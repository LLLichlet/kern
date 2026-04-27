use super::{
    ColorChoice, Command, HelpTopic, InstallSelection, RunSelection, UiOptions, Verbosity,
    parse_args, run_command, summarize_check_sources, summarize_source_security,
    validate_check_source_policy,
};
use crate::elaborate::FeatureSelection;
use crate::graph::SourceId;
use crate::manifest::{Manifest, ReleaseSourcePolicy};
use crate::operation_lock::WorkspaceOperationLock;
use crate::resolver::{ExternalPackageId, ResolvedExternalPackage, ResolvedGraph};
use crate::test_support::{
    FAILPOINT_AFTER_ANALYSIS_CONTEXT_SYNC, FAILPOINT_AFTER_COMPILE_STATE_WRITE,
    FAILPOINT_AFTER_LINK_STATE_WRITE, FAILPOINT_AFTER_STAGED_OUTPUT_WRITE,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_minimal_bin_package(root: &std::path::Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
}

fn write_minimal_lib_package(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[lib]
root = "src/lib.rn"
"#,
    )
    .unwrap();
    fs::write(root.join("src/lib.rn"), "pub fn demo() void {}\n").unwrap();
}

fn write_publishable_bin_package(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"
description = "Demo package"
license = "MIT"
authors = ["Demo <demo@example.com>"]
readme = "README.md"
repository = "https://example.com/demo"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(root.join("README.md"), "# demo\n").unwrap();
    fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
}

fn assert_lockfile_is_current(root: &Path) {
    let lockfile = fs::read_to_string(root.join("Craft.lock")).unwrap();
    assert!(lockfile.contains("manifest = \"Craft.toml\""));
    assert!(lockfile.contains("name = \"demo\""));
    assert!(!lockfile.contains("partial lockfile"));
}

fn assert_command_resyncs_missing_and_damaged_lockfile(
    prefix: &str,
    setup: impl Fn(&Path),
    mut command: impl FnMut(&Path) -> Command,
) {
    let root = temp_dir(prefix);
    setup(&root);

    run_command(command(&root)).unwrap();
    assert_lockfile_is_current(&root);

    fs::remove_file(root.join("Craft.lock")).unwrap();
    run_command(command(&root)).unwrap();
    assert_lockfile_is_current(&root);

    fs::write(root.join("Craft.lock"), "partial lockfile\n").unwrap();
    run_command(command(&root)).unwrap();
    assert_lockfile_is_current(&root);

    let _ = fs::remove_dir_all(root);
}

fn arg_check_source(first: &str, second: &str) -> String {
    format!(
        r#"
use std.proc;

fn main(argc: i32, argv: **u8) i32 {{
    let args = proc.args(argc, argv);
    if (args.len() != 3) {{
        return 1;
    }}
    let first = match (args.get(1)) {{
        .{{ Some: arg }} => arg,
        .None => return 2,
    }};
    if (first != "{first}") {{
        return 3;
    }}
    let second = match (args.get(2)) {{
        .{{ Some: arg }} => arg,
        .None => return 4,
    }};
    if (second != "{second}") {{
        return 5;
    }}
    return 0;
}}
"#
    )
}

fn write_arg_check_bin_package(root: &std::path::Path, first: &str, second: &str) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(root.join("src/main.rn"), arg_check_source(first, second)).unwrap();
}

fn write_arg_check_test_package(root: &std::path::Path, first: &str, second: &str) {
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[test]
roots = ["tests/smoke.rn"]
"#,
    )
    .unwrap();
    fs::write(root.join("tests/smoke.rn"), arg_check_source(first, second)).unwrap();
}

fn write_bin_and_test_package(root: &std::path::Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[[bin]]
name = "demo"
root = "src/main.rn"

[test]
roots = ["tests/smoke.rn"]
"#,
    )
    .unwrap();
    fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
    fs::write(root.join("tests/smoke.rn"), "fn main() i32 { return 0; }\n").unwrap();
}

fn write_bin_and_example_package(root: &std::path::Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("examples")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[[bin]]
name = "demo"
root = "src/main.rn"

[example]
roots = ["examples/sample.rn"]
"#,
    )
    .unwrap();
    fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
    fs::write(
        root.join("examples/sample.rn"),
        "fn main() i32 { return 0; }\n",
    )
    .unwrap();
}

fn write_multi_bin_package(root: &std::path::Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[[bin]]
name = "demo"
root = "src/main.rn"

[[bin]]
name = "helper"
root = "src/helper.rn"
"#,
    )
    .unwrap();
    fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
    fs::write(root.join("src/helper.rn"), "fn main() i32 { return 0; }\n").unwrap();
}

fn write_workspace_with_member_test_package(root: &std::path::Path) -> PathBuf {
    let member = root.join("member");
    fs::create_dir_all(member.join("tests")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        "[workspace]\nmembers = [\"member\"]\n",
    )
    .unwrap();
    fs::write(
        member.join("Craft.toml"),
        r#"
[package]
name = "member"
version = "0.1.0"
kern = "0.7.2"

[test]
roots = ["tests/smoke.rn"]
"#,
    )
    .unwrap();
    fs::write(
        member.join("tests/smoke.rn"),
        "fn main() i32 { return 0; }\n",
    )
    .unwrap();
    member.join("Craft.toml")
}

fn write_invalid_workspace_lock(root: &std::path::Path) {
    let lock_path = root.join(".craft").join("lock").join("workspace.lock");
    fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
    fs::write(&lock_path, "operation=build\n").unwrap();
    thread::sleep(Duration::from_millis(350));
}

fn run_command_with_timeout(command: Command, timeout: Duration) {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = run_command(command);
        tx.send(result).unwrap();
    });
    rx.recv_timeout(timeout)
        .unwrap_or_else(|_| panic!("command did not finish within {:?}", timeout))
        .unwrap();
}

fn wait_for_failpoint_ready(child: &mut Child, path: &std::path::Path, timeout: Duration) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if path.exists() {
            return;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                panic!(
                    "failpoint subprocess exited with status {status} before path `{}` appeared",
                    path.display()
                );
            }
            Ok(None) => {}
            Err(err) => {
                panic!("failed to poll failpoint subprocess status: {err}");
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "path `{}` did not appear within {:?}",
        path.display(),
        timeout
    );
}

fn failpoint_ready_timeout() -> Duration {
    if cfg!(windows) || std::env::var_os("CI").is_some() {
        Duration::from_secs(60)
    } else {
        Duration::from_secs(10)
    }
}

fn kill_recovery_command_timeout() -> Duration {
    if cfg!(windows) || std::env::var_os("CI").is_some() {
        Duration::from_secs(60)
    } else {
        Duration::from_secs(10)
    }
}

#[derive(Clone, Copy, Debug)]
enum KillRecoveryMode {
    Build,
    Check,
    Run,
    Test,
}

fn spawn_command_subprocess_with_failpoint(
    root: &std::path::Path,
    mode: KillRecoveryMode,
    failpoint: &str,
    ready_path: &std::path::Path,
) -> Child {
    let current_exe = std::env::current_exe().unwrap();
    ProcessCommand::new(current_exe)
        .arg("--exact")
        .arg("cli::tests::subprocess_runs_command_until_killed")
        .arg("--nocapture")
        .env("CRAFT_TEST_SUBPROCESS_MODE", format!("{mode:?}"))
        .env(
            "CRAFT_TEST_SUBPROCESS_PROJECT_PATH",
            root.join("Craft.toml"),
        )
        .env("CRAFT_TEST_FAILPOINT", failpoint)
        .env("CRAFT_TEST_FAILPOINT_READY_FILE", ready_path)
        .spawn()
        .unwrap()
}

fn command_for_mode(root: &std::path::Path, mode: KillRecoveryMode) -> Command {
    match mode {
        KillRecoveryMode::Build => Command::Build {
            path: Some(root.to_path_buf()),
            feature_selection: FeatureSelection::default(),
            ui: UiOptions::default(),
            include_examples: false,
        },
        KillRecoveryMode::Check => Command::Check {
            path: Some(root.to_path_buf()),
            feature_selection: FeatureSelection::default(),
            ui: UiOptions::default(),
        },
        KillRecoveryMode::Run => Command::Run {
            path: Some(root.to_path_buf()),
            feature_selection: FeatureSelection::default(),
            ui: UiOptions::default(),
            selection: RunSelection::DefaultBin,
            runtime_args: Vec::new(),
        },
        KillRecoveryMode::Test => Command::Test {
            path: Some(root.to_path_buf()),
            feature_selection: FeatureSelection::default(),
            ui: UiOptions::default(),
            runtime_args: Vec::new(),
        },
    }
}

fn write_generated_build_script_package(root: &std::path::Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[[bin]]
name = "demo"
root = "src/placeholder.rn"
"#,
    )
    .unwrap();
    fs::write(
        root.join("src/placeholder.rn"),
        "fn main() i32 { return 1; }\n",
    )
    .unwrap();
    fs::write(
        root.join("build.rn"),
        r#"
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
let main = b.emit_generated(
    "src/main.rn",
    "mod helper;\nfn main() i32 { return helper.answer(); }\n"
);
let _ = b.emit_generated(
    "src/helper.rn",
    "pub/ fn answer() i32 { return 0; }\n"
);
b.set_source_root(main);
}
"#,
    )
    .unwrap();
}

fn run_kill_recovery_case(root: &std::path::Path, mode: KillRecoveryMode, failpoint: &str) {
    let ready_path = root.join(".craft-failpoint-ready");
    let mut child = spawn_command_subprocess_with_failpoint(root, mode, failpoint, &ready_path);
    wait_for_failpoint_ready(&mut child, &ready_path, failpoint_ready_timeout());
    child.kill().unwrap();
    let _ = child.wait().unwrap();

    assert!(root.join(".craft/lock/workspace.lock").exists());

    let timeout = kill_recovery_command_timeout();
    run_command_with_timeout(command_for_mode(root, mode), timeout);
    run_command_with_timeout(command_for_mode(root, mode), timeout);

    assert!(!root.join(".craft/lock/workspace.lock").exists());
}

fn demo_executable_name() -> String {
    format!("demo{}", std::env::consts::EXE_SUFFIX)
}

#[test]
fn parses_check_with_path_and_feature_options() {
    let cmd = parse_args([
        "check".to_string(),
        "--project-path".to_string(),
        "demo".to_string(),
        "--no-default-features".to_string(),
        "--features".to_string(),
        "tls,simd".to_string(),
    ])
    .unwrap();

    match cmd {
        Command::Check {
            path,
            feature_selection,
            ui,
        } => {
            assert_eq!(path.as_deref(), Some(std::path::Path::new("demo")));
            assert!(!feature_selection.enable_default);
            assert!(feature_selection.explicit.contains("tls"));
            assert!(feature_selection.explicit.contains("simd"));
            assert_eq!(
                feature_selection.profile,
                crate::script::ProfileSelection::Dev
            );
            assert_eq!(ui, UiOptions::default());
        }
        other => panic!("expected check command, got {other:?}"),
    }
}

#[test]
fn parses_init_with_project_path() {
    let cmd = parse_args([
        "init".to_string(),
        "--project-path".to_string(),
        "demo".to_string(),
    ])
    .unwrap();

    match cmd {
        Command::Init { path, ui } => {
            assert_eq!(path.as_deref(), Some(std::path::Path::new("demo")));
            assert_eq!(ui, UiOptions::default());
        }
        other => panic!("expected init command, got {other:?}"),
    }
}

#[test]
fn parses_short_project_path_alias() {
    let cmd = parse_args(["check".to_string(), "-p".to_string(), "demo".to_string()]).unwrap();

    match cmd {
        Command::Check {
            path,
            feature_selection,
            ui,
        } => {
            assert_eq!(path.as_deref(), Some(std::path::Path::new("demo")));
            assert!(feature_selection.enable_default);
            assert!(feature_selection.explicit.is_empty());
            assert_eq!(ui, UiOptions::default());
        }
        other => panic!("expected check command, got {other:?}"),
    }
}

#[test]
fn parses_clean_with_project_path() {
    let cmd = parse_args([
        "clean".to_string(),
        "--project-path".to_string(),
        "demo".to_string(),
    ])
    .unwrap();

    match cmd {
        Command::Clean { path, ui } => {
            assert_eq!(path.as_deref(), Some(std::path::Path::new("demo")));
            assert_eq!(ui, UiOptions::default());
        }
        other => panic!("expected clean command, got {other:?}"),
    }
}

#[test]
fn parses_global_version_flags() {
    assert!(matches!(
        parse_args(["--version".to_string()]).unwrap(),
        Command::Version
    ));
    assert!(matches!(
        parse_args(["-V".to_string()]).unwrap(),
        Command::Version
    ));
    assert!(matches!(
        parse_args(["-v".to_string()]).unwrap(),
        Command::Version
    ));
}

#[test]
fn parses_help_and_version_after_command_options() {
    assert!(matches!(
        parse_args(["build".to_string(), "-v".to_string(), "--help".to_string(),]).unwrap(),
        Command::Help {
            topic: HelpTopic::Command(ref topic),
            ..
        } if topic == "build"
    ));
    assert!(matches!(
        parse_args(["build".to_string(), "--version".to_string()]).unwrap(),
        Command::Version
    ));
}

#[test]
fn parses_explicit_help_topic() {
    assert!(matches!(
        parse_args(["help".to_string(), "run".to_string()]).unwrap(),
        Command::Help {
            topic: HelpTopic::Command(ref topic),
            ..
        } if topic == "run"
    ));
}

#[test]
fn parses_build_without_path() {
    let cmd = parse_args(["build".to_string()]).unwrap();

    match cmd {
        Command::Build {
            path,
            feature_selection,
            ui,
            include_examples,
        } => {
            assert!(path.is_none());
            assert!(feature_selection.enable_default);
            assert!(feature_selection.explicit.is_empty());
            assert_eq!(
                feature_selection.profile,
                crate::script::ProfileSelection::Dev
            );
            assert_eq!(ui, UiOptions::default());
            assert!(!include_examples);
        }
        other => panic!("expected build command, got {other:?}"),
    }
}

#[test]
fn parses_build_with_release_profile() {
    let cmd = parse_args([
        "build".to_string(),
        "--profile".to_string(),
        "release".to_string(),
    ])
    .unwrap();

    match cmd {
        Command::Build {
            path,
            feature_selection,
            ui,
            include_examples,
        } => {
            assert!(path.is_none());
            assert!(feature_selection.enable_default);
            assert!(feature_selection.explicit.is_empty());
            assert_eq!(
                feature_selection.profile,
                crate::script::ProfileSelection::Release
            );
            assert_eq!(ui, UiOptions::default());
            assert!(!include_examples);
        }
        other => panic!("expected build command, got {other:?}"),
    }
}

#[test]
fn parses_install_with_root_and_named_bin() {
    let cmd = parse_args([
        "install".to_string(),
        "--project-path".to_string(),
        "demo".to_string(),
        "--profile".to_string(),
        "release".to_string(),
        "--bin".to_string(),
        "helper".to_string(),
        "--root".to_string(),
        "/tmp/kern-root".to_string(),
    ])
    .unwrap();

    match cmd {
        Command::Install {
            path,
            feature_selection,
            ui,
            selection,
            root,
        } => {
            assert_eq!(path.as_deref(), Some(std::path::Path::new("demo")));
            assert_eq!(
                feature_selection.profile,
                crate::script::ProfileSelection::Release
            );
            assert_eq!(ui, UiOptions::default());
            assert_eq!(selection, InstallSelection::Bin("helper".to_string()));
            assert_eq!(
                root.as_deref(),
                Some(std::path::Path::new("/tmp/kern-root"))
            );
        }
        other => panic!("expected install command, got {other:?}"),
    }
}

#[test]
fn parses_uninstall_with_root_and_named_bin() {
    let cmd = parse_args([
        "uninstall".to_string(),
        "--project-path=demo".to_string(),
        "--bin=helper".to_string(),
        "--root=/tmp/kern-root".to_string(),
    ])
    .unwrap();

    match cmd {
        Command::Uninstall {
            path,
            ui,
            selection,
            root,
        } => {
            assert_eq!(path.as_deref(), Some(std::path::Path::new("demo")));
            assert_eq!(ui, UiOptions::default());
            assert_eq!(selection, InstallSelection::Bin("helper".to_string()));
            assert_eq!(
                root.as_deref(),
                Some(std::path::Path::new("/tmp/kern-root"))
            );
        }
        other => panic!("expected uninstall command, got {other:?}"),
    }
}

#[test]
fn parses_short_install_root_alias() {
    let cmd = parse_args([
        "install".to_string(),
        "-r".to_string(),
        "/tmp/kern-root".to_string(),
    ])
    .unwrap();

    match cmd {
        Command::Install { root, .. } => {
            assert_eq!(
                root.as_deref(),
                Some(std::path::Path::new("/tmp/kern-root"))
            );
        }
        other => panic!("expected install command, got {other:?}"),
    }
}

#[test]
fn parses_short_bin_alias_for_install() {
    let cmd = parse_args([
        "install".to_string(),
        "-b".to_string(),
        "helper".to_string(),
    ])
    .unwrap();

    match cmd {
        Command::Install { selection, .. } => {
            assert_eq!(selection, InstallSelection::Bin("helper".to_string()));
        }
        other => panic!("expected install command, got {other:?}"),
    }
}

#[test]
fn parses_short_bin_alias_for_uninstall() {
    let cmd = parse_args([
        "uninstall".to_string(),
        "-b".to_string(),
        "helper".to_string(),
    ])
    .unwrap();

    match cmd {
        Command::Uninstall { selection, .. } => {
            assert_eq!(selection, InstallSelection::Bin("helper".to_string()));
        }
        other => panic!("expected uninstall command, got {other:?}"),
    }
}

#[test]
fn parses_fetch_with_path() {
    let cmd = parse_args(["fetch".to_string(), "--project-path=demo".to_string()]).unwrap();

    match cmd {
        Command::Fetch {
            path,
            feature_selection,
            ui,
        } => {
            assert_eq!(path.as_deref(), Some(std::path::Path::new("demo")));
            assert!(feature_selection.enable_default);
            assert!(feature_selection.explicit.is_empty());
            assert_eq!(ui, UiOptions::default());
        }
        other => panic!("expected fetch command, got {other:?}"),
    }
}

#[test]
fn parses_publish_and_forces_release_profile() {
    let cmd = parse_args([
        "publish".to_string(),
        "--project-path".to_string(),
        "demo".to_string(),
    ])
    .unwrap();

    match cmd {
        Command::Publish {
            path,
            feature_selection,
            ui,
        } => {
            assert_eq!(path.as_deref(), Some(std::path::Path::new("demo")));
            assert!(feature_selection.enable_default);
            assert!(feature_selection.explicit.is_empty());
            assert_eq!(
                feature_selection.profile,
                crate::script::ProfileSelection::Release
            );
            assert_eq!(ui, UiOptions::default());
        }
        other => panic!("expected publish command, got {other:?}"),
    }
}

#[test]
fn parses_doc_with_verbose_output() {
    let cmd = parse_args(["doc".to_string(), "--verbose".to_string()]).unwrap();

    match cmd {
        Command::Doc {
            path,
            feature_selection,
            ui,
        } => {
            assert!(path.is_none());
            assert!(feature_selection.enable_default);
            assert!(feature_selection.explicit.is_empty());
            assert_eq!(ui.verbosity, Verbosity::Verbose);
            assert_eq!(ui.color, ColorChoice::Auto);
        }
        other => panic!("expected doc command, got {other:?}"),
    }
}

#[test]
fn parses_run_with_path() {
    let cmd = parse_args([
        "run".to_string(),
        "--project-path".to_string(),
        "demo".to_string(),
    ])
    .unwrap();

    match cmd {
        Command::Run {
            path,
            feature_selection,
            ui,
            selection,
            runtime_args,
        } => {
            assert_eq!(path.as_deref(), Some(std::path::Path::new("demo")));
            assert!(feature_selection.enable_default);
            assert!(feature_selection.explicit.is_empty());
            assert_eq!(ui, UiOptions::default());
            assert_eq!(selection, RunSelection::DefaultBin);
            assert!(runtime_args.is_empty());
        }
        other => panic!("expected run command, got {other:?}"),
    }
}

#[test]
fn parses_run_passthrough_args_after_separator() {
    let cmd = parse_args([
        "run".to_string(),
        "-p".to_string(),
        "demo".to_string(),
        "--".to_string(),
        "--help".to_string(),
        "--color=never".to_string(),
        "plain".to_string(),
    ])
    .unwrap();

    match cmd {
        Command::Run {
            path, runtime_args, ..
        } => {
            assert_eq!(path.as_deref(), Some(std::path::Path::new("demo")));
            assert_eq!(
                runtime_args,
                vec![
                    "--help".to_string(),
                    "--color=never".to_string(),
                    "plain".to_string()
                ]
            );
        }
        other => panic!("expected run command, got {other:?}"),
    }
}

#[test]
fn parses_run_example_selector() {
    let cmd = parse_args([
        "run".to_string(),
        "--example".to_string(),
        "sample".to_string(),
    ])
    .unwrap();

    match cmd {
        Command::Run { selection, .. } => {
            assert_eq!(selection, RunSelection::Example("sample".to_string()));
        }
        other => panic!("expected run command, got {other:?}"),
    }
}

#[test]
fn parses_short_bin_alias_for_run() {
    let cmd = parse_args(["run".to_string(), "-b".to_string(), "helper".to_string()]).unwrap();

    match cmd {
        Command::Run { selection, .. } => {
            assert_eq!(selection, RunSelection::Bin("helper".to_string()));
        }
        other => panic!("expected run command, got {other:?}"),
    }
}

#[test]
fn parses_test_with_inline_feature_option() {
    let cmd = parse_args(["test".to_string(), "--features=simd".to_string()]).unwrap();

    match cmd {
        Command::Test {
            path,
            feature_selection,
            ui,
            runtime_args,
        } => {
            assert!(path.is_none());
            assert!(feature_selection.enable_default);
            assert_eq!(feature_selection.explicit.len(), 1);
            assert!(feature_selection.explicit.contains("simd"));
            assert_eq!(ui, UiOptions::default());
            assert!(runtime_args.is_empty());
        }
        other => panic!("expected test command, got {other:?}"),
    }
}

#[test]
fn parses_test_passthrough_args_after_separator() {
    let cmd = parse_args([
        "test".to_string(),
        "--features=simd".to_string(),
        "--".to_string(),
        "--filter".to_string(),
        "smoke".to_string(),
    ])
    .unwrap();

    match cmd {
        Command::Test {
            feature_selection,
            runtime_args,
            ..
        } => {
            assert!(feature_selection.explicit.contains("simd"));
            assert_eq!(
                runtime_args,
                vec!["--filter".to_string(), "smoke".to_string()]
            );
        }
        other => panic!("expected test command, got {other:?}"),
    }
}

#[test]
fn parses_verbose_flag() {
    let cmd = parse_args(["build".to_string(), "--verbose".to_string()]).unwrap();

    match cmd {
        Command::Build { ui, .. } => {
            assert_eq!(
                ui,
                UiOptions {
                    verbosity: Verbosity::Verbose,
                    timings: false,
                    color: ColorChoice::Auto,
                }
            );
        }
        other => panic!("expected build command, got {other:?}"),
    }
}

#[test]
fn parses_short_verbose_and_color_always() {
    let cmd = parse_args([
        "build".to_string(),
        "-v".to_string(),
        "--color=always".to_string(),
    ])
    .unwrap();

    match cmd {
        Command::Build { ui, .. } => {
            assert_eq!(
                ui,
                UiOptions {
                    verbosity: Verbosity::Verbose,
                    timings: false,
                    color: ColorChoice::Always,
                }
            );
        }
        other => panic!("expected build command, got {other:?}"),
    }
}

#[test]
fn parses_repeated_short_verbose_levels() {
    let cmd = parse_args(["build".to_string(), "-vv".to_string()]).unwrap();

    match cmd {
        Command::Build { ui, .. } => {
            assert_eq!(ui.verbosity, Verbosity::Debug);
        }
        other => panic!("expected build command, got {other:?}"),
    }

    let cmd = parse_args(["build".to_string(), "-v".to_string(), "-v".to_string()]).unwrap();

    match cmd {
        Command::Build { ui, .. } => {
            assert_eq!(ui.verbosity, Verbosity::Debug);
        }
        other => panic!("expected build command, got {other:?}"),
    }

    let cmd = parse_args(["build".to_string(), "-vvv".to_string()]).unwrap();

    match cmd {
        Command::Build { ui, .. } => {
            assert_eq!(ui.verbosity, Verbosity::Trace);
        }
        other => panic!("expected build command, got {other:?}"),
    }
}

#[test]
fn parses_long_verbose_levels() {
    let cmd = parse_args(["build".to_string(), "--verbose=2".to_string()]).unwrap();

    match cmd {
        Command::Build { ui, .. } => {
            assert_eq!(ui.verbosity, Verbosity::Debug);
        }
        other => panic!("expected build command, got {other:?}"),
    }

    let cmd = parse_args(["build".to_string(), "--verbose=trace".to_string()]).unwrap();

    match cmd {
        Command::Build { ui, .. } => {
            assert_eq!(ui.verbosity, Verbosity::Trace);
        }
        other => panic!("expected build command, got {other:?}"),
    }
}

#[test]
fn rejects_unknown_verbose_level() {
    let err = parse_args(["build".to_string(), "--verbose=chatty".to_string()]).unwrap_err();
    assert!(format!("{err}").contains("unsupported `--verbose` value `chatty`"));
}

#[test]
fn parses_timings_flag() {
    let cmd = parse_args(["build".to_string(), "--timings".to_string()]).unwrap();

    match cmd {
        Command::Build { ui, .. } => {
            assert_eq!(
                ui,
                UiOptions {
                    verbosity: Verbosity::Normal,
                    timings: true,
                    color: ColorChoice::Auto,
                }
            );
        }
        other => panic!("expected build command, got {other:?}"),
    }
}

#[test]
fn parses_no_color_alias() {
    let cmd = parse_args(["check".to_string(), "--no-color".to_string()]).unwrap();

    match cmd {
        Command::Check { ui, .. } => {
            assert_eq!(
                ui,
                UiOptions {
                    verbosity: Verbosity::Normal,
                    timings: false,
                    color: ColorChoice::Never,
                }
            );
        }
        other => panic!("expected check command, got {other:?}"),
    }
}

#[test]
fn parses_build_examples_flag() {
    let cmd = parse_args(["build".to_string(), "--examples".to_string()]).unwrap();

    match cmd {
        Command::Build {
            include_examples, ..
        } => {
            assert!(include_examples);
        }
        other => panic!("expected build command, got {other:?}"),
    }
}

#[test]
fn rejects_examples_flag_for_non_build_commands() {
    let err = parse_args(["test".to_string(), "--examples".to_string()]).unwrap_err();
    assert!(err.to_string().contains("unsupported option `--examples`"));
}

#[test]
fn rejects_passthrough_separator_for_non_runtime_commands() {
    let err =
        parse_args(["build".to_string(), "--".to_string(), "--flag".to_string()]).unwrap_err();
    assert!(err.to_string().contains("only accepted by `craft run`"));
}

#[test]
fn rejects_multiple_run_target_selectors() {
    let err = parse_args([
        "run".to_string(),
        "--bin".to_string(),
        "demo".to_string(),
        "--example".to_string(),
        "sample".to_string(),
    ])
    .unwrap_err();
    assert!(err.to_string().contains("accepts at most one"));
}

#[test]
fn rejects_unknown_color_choice() {
    let err = parse_args(["build".to_string(), "--color=rgb".to_string()]).unwrap_err();
    assert!(
        err.to_string().contains("expected auto, always, or never"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_empty_feature_names() {
    let err = parse_args([
        "check".to_string(),
        "--features".to_string(),
        "simd,".to_string(),
    ])
    .unwrap_err();
    assert!(
        err.to_string().contains("must not be empty"),
        "unexpected error: {err}"
    );
}

#[test]
fn summarizes_check_sources_by_backend() {
    let resolved = ResolvedGraph {
        workspace_root: PathBuf::from("."),
        packages: Vec::new(),
        external_packages: vec![
            ResolvedExternalPackage {
                id: ExternalPackageId {
                    package_name: "log".to_string(),
                    source: SourceId::GitDependency {
                        git: "https://example.com/log.git".to_string(),
                        rev: None,
                        branch: Some("main".to_string()),
                        tag: None,
                    },
                    version: Some("1".to_string()),
                },
            },
            ResolvedExternalPackage {
                id: ExternalPackageId {
                    package_name: "net".to_string(),
                    source: SourceId::GitDependency {
                        git: "https://example.com/net.git".to_string(),
                        rev: None,
                        branch: None,
                        tag: Some("v2".to_string()),
                    },
                    version: Some("2".to_string()),
                },
            },
            ResolvedExternalPackage {
                id: ExternalPackageId {
                    package_name: "util".to_string(),
                    source: SourceId::PathDependency {
                        path: "../util".to_string(),
                    },
                    version: None,
                },
            },
        ],
    };

    let summary = summarize_check_sources(&resolved);
    assert_eq!(summary.git_sources, 2);
    assert_eq!(summary.git_packages, 2);
    assert_eq!(summary.path_packages, 1);
}

#[test]
fn summarize_source_security_respects_allowlists_and_warn_mode() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[craft]
release-source-policy = "warn"
allow-floating-git = ["default"]
allow-insecure-source = ["insecure"]

[workspace]
members = []

[workspace.dependencies]
default = { git = "https://example.com/default.git", branch = "main" }
insecure = { git = "http://example.com/insecure.git", branch = "main" }
blocked = { git = "https://example.com/blocked.git", branch = "main" }

[resources]
limine = { git = "https://example.com/limine.git", branch = "main" }
mirror = { git = "http://example.com/mirror.git", branch = "main" }
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let summary = summarize_source_security(&manifest);
    assert_eq!(summary.policy_mode, ReleaseSourcePolicy::Warn);
    assert_eq!(summary.floating_git_sources, 5);
    assert_eq!(summary.insecure_transport_sources, 2);
    assert_eq!(
        summary.warnings,
        vec![
            "blocked(floating-git)".to_string(),
            "insecure(floating-git)".to_string(),
            "resource:limine(floating-git)".to_string(),
            "resource:mirror(insecure-transport)".to_string(),
            "resource:mirror(floating-git)".to_string(),
        ]
    );
    assert_eq!(
        summary.suppressed,
        vec![
            "default(floating-git)".to_string(),
            "insecure(insecure-transport)".to_string(),
        ]
    );
    assert!(summary.release_blockers().is_empty());
}

#[test]
fn validate_check_source_policy_rejects_release_blockers() {
    let summary = super::policy::SourceSecuritySummary {
        policy_mode: ReleaseSourcePolicy::Enforce,
        floating_git_sources: 1,
        insecure_transport_sources: 1,
        warnings: vec![
            "default(floating-git)".to_string(),
            "default(insecure-transport)".to_string(),
        ],
        suppressed: Vec::new(),
        release_blockers: vec![
            "default(floating-git)".to_string(),
            "default(insecure-transport)".to_string(),
        ],
    };
    let selection = FeatureSelection {
        profile: crate::script::ProfileSelection::Release,
        ..FeatureSelection::default()
    };

    let err =
        validate_check_source_policy(std::path::Path::new("Craft.toml"), &selection, &summary)
            .unwrap_err();
    assert!(err.to_string().contains("release source policy rejected"));
}

#[test]
fn check_command_waits_for_workspace_lock() {
    let root = temp_dir("craft-cli-workspace-lock");
    write_minimal_bin_package(&root);
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let root_for_holder = root.clone();

    let holder = thread::spawn(move || {
        let _lock = WorkspaceOperationLock::acquire(&root_for_holder, "build").unwrap();
        ready_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });

    ready_rx.recv().unwrap();
    let root_for_check = root.clone();
    let start = Instant::now();
    let waiter = thread::spawn(move || {
        run_command(Command::Check {
            path: Some(root_for_check),
            feature_selection: FeatureSelection::default(),
            ui: UiOptions::default(),
        })
        .unwrap();
        start.elapsed()
    });

    thread::sleep(Duration::from_millis(200));
    release_tx.send(()).unwrap();

    holder.join().unwrap();
    let waited = waiter.join().unwrap();
    assert!(waited >= Duration::from_millis(150));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn clean_command_removes_derived_craft_state() {
    let root = temp_dir("craft-cli-clean");
    write_minimal_bin_package(&root);
    fs::create_dir_all(root.join(".craft/build/dev")).unwrap();
    fs::create_dir_all(root.join(".craft/resources/pkg/resource")).unwrap();
    fs::write(root.join(".craft/analysis.toml"), "derived = true\n").unwrap();
    fs::write(root.join(".craft/build/dev/artifact"), "artifact\n").unwrap();

    run_command(Command::Clean {
        path: Some(root.clone()),
        ui: UiOptions::default(),
    })
    .unwrap();

    assert!(!root.join(".craft/build").exists());
    assert!(!root.join(".craft/resources").exists());
    assert!(!root.join(".craft/analysis.toml").exists());
    assert!(root.join(".craft/lock").is_dir());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_command_waits_for_workspace_lock() {
    let root = temp_dir("craft-cli-build-workspace-lock");
    write_minimal_bin_package(&root);
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let root_for_holder = root.clone();

    let holder = thread::spawn(move || {
        let _lock = WorkspaceOperationLock::acquire(&root_for_holder, "test").unwrap();
        ready_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });

    ready_rx.recv().unwrap();
    let root_for_build = root.clone();
    let start = Instant::now();
    let waiter = thread::spawn(move || {
        run_command(Command::Build {
            path: Some(root_for_build),
            feature_selection: FeatureSelection::default(),
            ui: UiOptions::default(),
            include_examples: false,
        })
        .unwrap();
        start.elapsed()
    });

    thread::sleep(Duration::from_millis(200));
    release_tx.send(()).unwrap();

    holder.join().unwrap();
    let waited = waiter.join().unwrap();
    assert!(waited >= Duration::from_millis(150));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn check_command_recovers_from_invalid_workspace_lock() {
    let root = temp_dir("craft-cli-check-invalid-workspace-lock");
    write_minimal_bin_package(&root);
    write_invalid_workspace_lock(&root);

    run_command_with_timeout(
        Command::Check {
            path: Some(root.clone()),
            feature_selection: FeatureSelection::default(),
            ui: UiOptions::default(),
        },
        Duration::from_secs(30),
    );

    assert!(!root.join(".craft/lock/workspace.lock").exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_command_recovers_from_invalid_workspace_lock() {
    let root = temp_dir("craft-cli-build-invalid-workspace-lock");
    write_minimal_bin_package(&root);
    write_invalid_workspace_lock(&root);

    run_command_with_timeout(
        Command::Build {
            path: Some(root.clone()),
            feature_selection: FeatureSelection::default(),
            ui: UiOptions::default(),
            include_examples: false,
        },
        Duration::from_secs(30),
    );

    assert!(!root.join(".craft/lock/workspace.lock").exists());
    assert!(
        root.join(".craft/build/dev/target/out/demo-0.1.0/bin")
            .join(demo_executable_name())
            .is_file()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_command_recovers_after_killed_process_leaves_partial_generated_state() {
    let root = temp_dir("craft-cli-kill-recovery");
    write_generated_build_script_package(&root);
    run_kill_recovery_case(
        &root,
        KillRecoveryMode::Build,
        FAILPOINT_AFTER_STAGED_OUTPUT_WRITE,
    );
    assert!(
        root.join(".craft/build/dev/target/gen/demo-0.1.0/bin/demo/src")
            .join("main.rn")
            .exists()
    );
    assert!(
        root.join(".craft/build/dev/target/gen/demo-0.1.0/bin/demo/src")
            .join("main.rn")
            .is_file()
    );
    assert!(
        root.join(".craft/build/dev/target/gen/demo-0.1.0/bin/demo/src")
            .join("helper.rn")
            .is_file()
    );
    assert!(
        root.join(".craft/build/dev/target/out/demo-0.1.0/bin")
            .join(demo_executable_name())
            .is_file()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_command_recovers_after_killed_process_leaves_partial_compile_state() {
    let root = temp_dir("craft-cli-kill-recovery-compile-state");
    write_minimal_bin_package(&root);

    run_kill_recovery_case(
        &root,
        KillRecoveryMode::Build,
        FAILPOINT_AFTER_COMPILE_STATE_WRITE,
    );

    assert!(
        root.join(".craft/build/dev/target/obj/demo-0.1.0/bin")
            .join("demo.o")
            .is_file()
    );
    assert!(
        root.join(".craft/build/dev/target/obj/demo-0.1.0/bin")
            .join(".demo.o.craft-state")
            .is_file()
    );
    assert!(
        root.join(".craft/build/dev/target/out/demo-0.1.0/bin")
            .join(demo_executable_name())
            .is_file()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_command_recovers_after_killed_process_leaves_partial_link_state() {
    let root = temp_dir("craft-cli-kill-recovery-link-state");
    write_minimal_bin_package(&root);

    run_kill_recovery_case(
        &root,
        KillRecoveryMode::Build,
        FAILPOINT_AFTER_LINK_STATE_WRITE,
    );

    assert!(
        root.join(".craft/build/dev/target/out/demo-0.1.0/bin")
            .join(demo_executable_name())
            .is_file()
    );
    assert!(
        root.join(".craft/build/dev/target/out/demo-0.1.0/bin")
            .join(format!(".{}.craft-state", demo_executable_name()))
            .is_file()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn check_command_recovers_after_killed_process_leaves_partial_generated_state() {
    let root = temp_dir("craft-cli-check-kill-recovery-generated");
    write_generated_build_script_package(&root);

    run_kill_recovery_case(
        &root,
        KillRecoveryMode::Check,
        FAILPOINT_AFTER_STAGED_OUTPUT_WRITE,
    );

    assert!(
        root.join(".craft/build/dev/target/gen/demo-0.1.0/bin/demo/src")
            .join("main.rn")
            .is_file()
    );
    assert!(
        root.join(".craft/build/dev/target/gen/demo-0.1.0/bin/demo/src")
            .join("helper.rn")
            .is_file()
    );
    assert!(root.join(".craft/analysis.toml").is_file());
    assert!(
        !root
            .join(".craft/build/dev/target/obj/demo-0.1.0/bin")
            .join("demo.o")
            .exists()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn check_command_recovers_after_killed_process_leaves_partial_analysis_context() {
    let root = temp_dir("craft-cli-check-kill-recovery-analysis");
    write_generated_build_script_package(&root);

    run_kill_recovery_case(
        &root,
        KillRecoveryMode::Check,
        FAILPOINT_AFTER_ANALYSIS_CONTEXT_SYNC,
    );

    assert!(root.join(".craft/analysis.toml").is_file());
    assert!(
        root.join(".craft/build/dev/target/gen/demo-0.1.0/bin/demo/src")
            .join("main.rn")
            .is_file()
    );
    assert!(
        !root
            .join(".craft/build/dev/target/obj/demo-0.1.0/bin")
            .join("demo.o")
            .exists()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn run_command_recovers_after_killed_process_leaves_partial_link_state() {
    let root = temp_dir("craft-cli-run-kill-recovery-link-state");
    write_minimal_bin_package(&root);

    run_kill_recovery_case(
        &root,
        KillRecoveryMode::Run,
        FAILPOINT_AFTER_LINK_STATE_WRITE,
    );

    assert!(
        root.join(".craft/build/dev/target/out/demo-0.1.0/bin")
            .join(demo_executable_name())
            .is_file()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_command_recovers_after_killed_process_leaves_partial_link_state() {
    let root = temp_dir("craft-cli-test-kill-recovery-link-state");
    write_bin_and_test_package(&root);

    run_kill_recovery_case(
        &root,
        KillRecoveryMode::Test,
        FAILPOINT_AFTER_LINK_STATE_WRITE,
    );

    let test_out_dir = root.join(".craft/build/dev/target/out/demo-0.1.0/test");
    assert!(test_out_dir.is_dir());
    assert!(
        fs::read_dir(&test_out_dir)
            .unwrap()
            .any(|entry| entry.unwrap().path().is_file())
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn subprocess_runs_command_until_killed() {
    let Ok(mode) = std::env::var("CRAFT_TEST_SUBPROCESS_MODE") else {
        return;
    };

    let root = PathBuf::from(std::env::var("CRAFT_TEST_SUBPROCESS_PROJECT_PATH").unwrap());
    match mode.as_str() {
        "Build" => run_command(command_for_mode(
            root.parent().unwrap(),
            KillRecoveryMode::Build,
        ))
        .unwrap(),
        "Check" => run_command(command_for_mode(
            root.parent().unwrap(),
            KillRecoveryMode::Check,
        ))
        .unwrap(),
        "Run" => run_command(command_for_mode(
            root.parent().unwrap(),
            KillRecoveryMode::Run,
        ))
        .unwrap(),
        "Test" => run_command(command_for_mode(
            root.parent().unwrap(),
            KillRecoveryMode::Test,
        ))
        .unwrap(),
        other => panic!("unexpected subprocess mode `{other}`"),
    }
}

#[test]
fn test_command_waits_for_workspace_root_lock_for_member_paths() {
    let root = temp_dir("craft-cli-test-workspace-lock");
    let member = write_workspace_with_member_test_package(&root);
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let root_for_holder = root.clone();

    let holder = thread::spawn(move || {
        let _lock = WorkspaceOperationLock::acquire(&root_for_holder, "build").unwrap();
        ready_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });

    ready_rx.recv().unwrap();
    let start = Instant::now();
    let waiter = thread::spawn(move || {
        run_command(Command::Test {
            path: Some(member),
            feature_selection: FeatureSelection::default(),
            ui: UiOptions::default(),
            runtime_args: Vec::new(),
        })
        .unwrap();
        start.elapsed()
    });

    thread::sleep(Duration::from_millis(200));
    release_tx.send(()).unwrap();

    holder.join().unwrap();
    let waited = waiter.join().unwrap();
    assert!(waited >= Duration::from_millis(150));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_command_skips_test_targets() {
    let root = temp_dir("craft-cli-build-skips-tests");
    write_bin_and_test_package(&root);

    run_command(Command::Build {
        path: Some(root.clone()),
        feature_selection: FeatureSelection::default(),
        ui: UiOptions::default(),
        include_examples: false,
    })
    .unwrap();

    assert!(
        root.join(".craft/build/dev/target/out/demo-0.1.0/bin")
            .join(format!("demo{}", std::env::consts::EXE_SUFFIX))
            .is_file()
    );
    assert!(
        !root
            .join(".craft/build/dev/target/out/demo-0.1.0/test/smoke")
            .exists()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_command_can_include_examples() {
    let root = temp_dir("craft-cli-build-includes-examples");
    write_bin_and_example_package(&root);

    run_command(Command::Build {
        path: Some(root.clone()),
        feature_selection: FeatureSelection::default(),
        ui: UiOptions::default(),
        include_examples: true,
    })
    .unwrap();

    assert!(
        root.join(".craft/build/dev/target/out/demo-0.1.0/bin")
            .join(format!("demo{}", std::env::consts::EXE_SUFFIX))
            .is_file()
    );
    assert!(
        root.join(".craft/build/dev/target/out/demo-0.1.0/example")
            .join(format!("sample{}", std::env::consts::EXE_SUFFIX))
            .is_file()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn install_command_copies_selected_bin_into_root_bin_dir() {
    let root = temp_dir("craft-cli-install");
    let install_root = temp_dir("craft-cli-install-root");
    write_multi_bin_package(&root);

    run_command(Command::Install {
        path: Some(root.clone()),
        feature_selection: FeatureSelection::default(),
        ui: UiOptions::default(),
        selection: InstallSelection::Bin("helper".to_string()),
        root: Some(install_root.clone()),
    })
    .unwrap();

    assert!(
        install_root
            .join("bin")
            .join(format!("helper{}", std::env::consts::EXE_SUFFIX))
            .is_file()
    );
    assert!(
        !install_root
            .join("bin")
            .join(format!("demo{}", std::env::consts::EXE_SUFFIX))
            .exists()
    );

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(install_root);
}

#[test]
fn uninstall_command_removes_selected_installed_bin() {
    let root = temp_dir("craft-cli-uninstall");
    let install_root = temp_dir("craft-cli-uninstall-root");
    write_multi_bin_package(&root);

    run_command(Command::Install {
        path: Some(root.clone()),
        feature_selection: FeatureSelection::default(),
        ui: UiOptions::default(),
        selection: InstallSelection::Bin("helper".to_string()),
        root: Some(install_root.clone()),
    })
    .unwrap();

    run_command(Command::Uninstall {
        path: Some(root.clone()),
        ui: UiOptions::default(),
        selection: InstallSelection::Bin("helper".to_string()),
        root: Some(install_root.clone()),
    })
    .unwrap();

    assert!(
        !install_root
            .join("bin")
            .join(format!("helper{}", std::env::consts::EXE_SUFFIX))
            .exists()
    );

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(install_root);
}

#[test]
fn run_command_can_execute_selected_example() {
    let root = temp_dir("craft-cli-run-example");
    write_bin_and_example_package(&root);

    run_command(Command::Run {
        path: Some(root.clone()),
        feature_selection: FeatureSelection::default(),
        ui: UiOptions::default(),
        selection: RunSelection::Example("sample".to_string()),
        runtime_args: Vec::new(),
    })
    .unwrap();

    assert!(
        root.join(".craft/build/dev/target/out/demo-0.1.0/example")
            .join(format!("sample{}", std::env::consts::EXE_SUFFIX))
            .is_file()
    );
    assert!(
        !root
            .join(".craft/build/dev/target/out/demo-0.1.0/bin")
            .join(format!("demo{}", std::env::consts::EXE_SUFFIX))
            .exists()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn run_command_passes_runtime_args_to_bin() {
    let root = temp_dir("craft-cli-run-args");
    write_arg_check_bin_package(&root, "--version", "two words");

    run_command(Command::Run {
        path: Some(root.clone()),
        feature_selection: FeatureSelection::default(),
        ui: UiOptions::default(),
        selection: RunSelection::DefaultBin,
        runtime_args: vec!["--version".to_string(), "two words".to_string()],
    })
    .unwrap();

    assert!(root.join("Craft.lock").is_file());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn run_command_rewrites_damaged_lockfile() {
    let root = temp_dir("craft-cli-run-lock-repair");
    write_minimal_bin_package(&root);
    fs::write(root.join("Craft.lock"), "partial lockfile\n").unwrap();

    run_command(Command::Run {
        path: Some(root.clone()),
        feature_selection: FeatureSelection::default(),
        ui: UiOptions::default(),
        selection: RunSelection::DefaultBin,
        runtime_args: Vec::new(),
    })
    .unwrap();

    let lockfile = fs::read_to_string(root.join("Craft.lock")).unwrap();
    assert!(lockfile.contains("manifest = \"Craft.toml\""));
    assert!(lockfile.contains("name = \"demo\""));
    assert!(!lockfile.contains("partial lockfile"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_command_passes_runtime_args_to_test_targets() {
    let root = temp_dir("craft-cli-test-args");
    write_arg_check_test_package(&root, "--filter", "smoke");

    run_command(Command::Test {
        path: Some(root.clone()),
        feature_selection: FeatureSelection::default(),
        ui: UiOptions::default(),
        runtime_args: vec!["--filter".to_string(), "smoke".to_string()],
    })
    .unwrap();

    let _ = fs::remove_dir_all(root);
}

#[test]
fn init_command_scaffolds_minimal_bin_package() {
    let root = temp_dir("craft-cli-init-minimal");

    run_command(Command::Init {
        path: Some(root.clone()),
        ui: UiOptions::default(),
    })
    .unwrap();

    let manifest = fs::read_to_string(root.join("Craft.toml")).unwrap();
    assert!(manifest.contains("[[bin]]"));
    assert!(manifest.contains("root = \"src/main.rn\""));
    assert_eq!(
        fs::read_to_string(root.join(".gitignore")).unwrap(),
        ".craft/\n"
    );
    let lockfile = fs::read_to_string(root.join("Craft.lock")).unwrap();
    assert!(lockfile.contains("manifest = \"Craft.toml\""));
    assert!(lockfile.contains("name = \"craft_cli_init_minimal"));
    assert!(root.join("src/main.rn").is_file());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn init_command_collects_existing_test_and_example_roots() {
    let root = temp_dir("craft-cli-init-existing-layout");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests/nested")).unwrap();
    fs::create_dir_all(root.join("examples")).unwrap();
    fs::write(root.join("src/lib.rn"), "pub fn demo() void {}\n").unwrap();
    fs::write(
        root.join("tests/nested/smoke.rn"),
        "fn main() i32 { return 0; }\n",
    )
    .unwrap();
    fs::write(
        root.join("examples/sample.rn"),
        "fn main() i32 { return 0; }\n",
    )
    .unwrap();

    run_command(Command::Init {
        path: Some(root.clone()),
        ui: UiOptions::default(),
    })
    .unwrap();

    let manifest = fs::read_to_string(root.join("Craft.toml")).unwrap();
    assert!(manifest.contains("[lib]"));
    assert!(manifest.contains("[test]"));
    assert!(manifest.contains("\"tests/nested/smoke.rn\""));
    assert!(manifest.contains("[example]"));
    assert!(manifest.contains("\"examples/sample.rn\""));
    let lockfile = fs::read_to_string(root.join("Craft.lock")).unwrap();
    assert!(lockfile.contains("kind = \"lib\""));
    assert!(lockfile.contains("kind = \"test\""));
    assert!(lockfile.contains("kind = \"example\""));
    assert!(!root.join("src/main.rn").exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_command_uses_workspace_root_outputs_for_member_paths() {
    let root = temp_dir("craft-cli-build-member-workspace-root");
    let member = root.join("member");
    fs::create_dir_all(member.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        "[workspace]\nmembers = [\"member\"]\n",
    )
    .unwrap();
    fs::write(
        member.join("Craft.toml"),
        r#"
[package]
name = "member"
version = "0.1.0"
kern = "0.7.2"

[[bin]]
name = "member"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(member.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();

    run_command(Command::Build {
        path: Some(member.clone()),
        feature_selection: FeatureSelection::default(),
        ui: UiOptions::default(),
        include_examples: false,
    })
    .unwrap();

    assert!(
        root.join(".craft/build/dev/target/out/member-0.1.0/bin")
            .join(format!("member{}", std::env::consts::EXE_SUFFIX))
            .is_file()
    );
    assert!(root.join("Craft.lock").is_file());
    assert!(!member.join("Craft.lock").exists());
    assert!(!member.join(".craft").exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_command_member_path_does_not_build_workspace_root_package() {
    let root = temp_dir("craft-cli-build-member-excludes-root-package");
    let member = root.join("member");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(member.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "rootpkg"
version = "0.1.0"
kern = "0.7.2"

[[bin]]
name = "rootpkg"
root = "src/main.rn"

[workspace]
members = ["member"]
"#,
    )
    .unwrap();
    fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
    fs::write(
        member.join("Craft.toml"),
        r#"
[package]
name = "member"
version = "0.1.0"
kern = "0.7.2"

[[bin]]
name = "member"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(member.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();

    run_command(Command::Build {
        path: Some(member.clone()),
        feature_selection: FeatureSelection::default(),
        ui: UiOptions::default(),
        include_examples: false,
    })
    .unwrap();

    assert!(
        root.join(".craft/build/dev/target/out/member-0.1.0/bin")
            .join(format!("member{}", std::env::consts::EXE_SUFFIX))
            .is_file()
    );
    assert!(
        !root
            .join(".craft/build/dev/target/out/rootpkg-0.1.0/bin")
            .join(format!("rootpkg{}", std::env::consts::EXE_SUFFIX))
            .exists()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn package_graph_commands_resync_missing_and_damaged_lockfiles() {
    assert_command_resyncs_missing_and_damaged_lockfile(
        "craft-cli-check-lock-resync",
        write_minimal_bin_package,
        |root| Command::Check {
            path: Some(root.to_path_buf()),
            feature_selection: FeatureSelection::default(),
            ui: UiOptions::default(),
        },
    );
    assert_command_resyncs_missing_and_damaged_lockfile(
        "craft-cli-fetch-lock-resync",
        write_minimal_bin_package,
        |root| Command::Fetch {
            path: Some(root.to_path_buf()),
            feature_selection: FeatureSelection::default(),
            ui: UiOptions::default(),
        },
    );
    assert_command_resyncs_missing_and_damaged_lockfile(
        "craft-cli-publish-lock-resync",
        write_publishable_bin_package,
        |root| Command::Publish {
            path: Some(root.to_path_buf()),
            feature_selection: FeatureSelection {
                profile: crate::script::ProfileSelection::Release,
                ..Default::default()
            },
            ui: UiOptions::default(),
        },
    );
    assert_command_resyncs_missing_and_damaged_lockfile(
        "craft-cli-doc-lock-resync",
        write_minimal_lib_package,
        |root| Command::Doc {
            path: Some(root.to_path_buf()),
            feature_selection: FeatureSelection::default(),
            ui: UiOptions::default(),
        },
    );
    assert_command_resyncs_missing_and_damaged_lockfile(
        "craft-cli-build-lock-resync",
        write_minimal_bin_package,
        |root| Command::Build {
            path: Some(root.to_path_buf()),
            feature_selection: FeatureSelection::default(),
            ui: UiOptions::default(),
            include_examples: false,
        },
    );

    let install_root = temp_dir("craft-cli-install-lock-resync-root");
    assert_command_resyncs_missing_and_damaged_lockfile(
        "craft-cli-install-lock-resync",
        write_minimal_bin_package,
        |root| Command::Install {
            path: Some(root.to_path_buf()),
            feature_selection: FeatureSelection::default(),
            ui: UiOptions::default(),
            selection: InstallSelection::AllBins,
            root: Some(install_root.clone()),
        },
    );
    let _ = fs::remove_dir_all(install_root);

    let uninstall_root = temp_dir("craft-cli-uninstall-lock-resync-root");
    assert_command_resyncs_missing_and_damaged_lockfile(
        "craft-cli-uninstall-lock-resync",
        write_minimal_bin_package,
        |root| Command::Uninstall {
            path: Some(root.to_path_buf()),
            ui: UiOptions::default(),
            selection: InstallSelection::AllBins,
            root: Some(uninstall_root.clone()),
        },
    );
    let _ = fs::remove_dir_all(uninstall_root);

    assert_command_resyncs_missing_and_damaged_lockfile(
        "craft-cli-run-lock-resync",
        write_minimal_bin_package,
        |root| Command::Run {
            path: Some(root.to_path_buf()),
            feature_selection: FeatureSelection::default(),
            ui: UiOptions::default(),
            selection: RunSelection::DefaultBin,
            runtime_args: Vec::new(),
        },
    );
    assert_command_resyncs_missing_and_damaged_lockfile(
        "craft-cli-test-lock-resync",
        write_bin_and_test_package,
        |root| Command::Test {
            path: Some(root.to_path_buf()),
            feature_selection: FeatureSelection::default(),
            ui: UiOptions::default(),
            runtime_args: Vec::new(),
        },
    );
}

#[test]
fn build_auto_syncs_lockfile_and_rebuilds_without_clean() {
    let root = temp_dir("craft-cli-build-lock-autosync");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();

    run_command(Command::Build {
        path: Some(root.clone()),
        feature_selection: FeatureSelection::default(),
        ui: UiOptions::default(),
        include_examples: false,
    })
    .unwrap();

    assert!(root.join("Craft.lock").is_file());
    fs::remove_file(root.join("Craft.lock")).unwrap();

    run_command(Command::Build {
        path: Some(root.clone()),
        feature_selection: FeatureSelection::default(),
        ui: UiOptions::default(),
        include_examples: false,
    })
    .unwrap();

    assert!(root.join("Craft.lock").is_file());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_updates_lockfile_after_manifest_changes() {
    let root = temp_dir("craft-cli-build-lock-update");
    write_minimal_bin_package(&root);

    run_command(Command::Build {
        path: Some(root.clone()),
        feature_selection: FeatureSelection::default(),
        ui: UiOptions::default(),
        include_examples: false,
    })
    .unwrap();

    let initial = fs::read_to_string(root.join("Craft.lock")).unwrap();
    assert!(initial.contains("version = \"0.1.0\""));

    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.2.0"
kern = "0.7.2"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
    )
    .unwrap();

    run_command(Command::Build {
        path: Some(root.clone()),
        feature_selection: FeatureSelection::default(),
        ui: UiOptions::default(),
        include_examples: false,
    })
    .unwrap();

    let updated = fs::read_to_string(root.join("Craft.lock")).unwrap();
    assert!(updated.contains("version = \"0.2.0\""));
    assert!(!updated.contains("version = \"0.1.0\""));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn member_build_recreates_deleted_workspace_lockfile() {
    let root = temp_dir("craft-cli-member-lock-recreate");
    let member = root.join("member");
    fs::create_dir_all(member.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        "[workspace]\nmembers = [\"member\"]\n",
    )
    .unwrap();
    fs::write(
        member.join("Craft.toml"),
        r#"
[package]
name = "member"
version = "0.1.0"
kern = "0.7.2"

[[bin]]
name = "member"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(member.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();

    run_command(Command::Build {
        path: Some(member.clone()),
        feature_selection: FeatureSelection::default(),
        ui: UiOptions::default(),
        include_examples: false,
    })
    .unwrap();

    fs::remove_file(root.join("Craft.lock")).unwrap();

    run_command(Command::Build {
        path: Some(member.clone()),
        feature_selection: FeatureSelection::default(),
        ui: UiOptions::default(),
        include_examples: false,
    })
    .unwrap();

    assert!(root.join("Craft.lock").is_file());
    assert!(!member.join("Craft.lock").exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn publish_auto_syncs_release_lock_and_checks_metadata() {
    let root = temp_dir("craft-cli-publish");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"
description = "Demo package"
license = "MIT"
authors = ["Demo <demo@example.com>"]
readme = "README.md"
repository = "https://example.com/demo"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(root.join("README.md"), "# demo\n").unwrap();
    fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();

    run_command(Command::Publish {
        path: Some(root.clone()),
        feature_selection: FeatureSelection {
            profile: crate::script::ProfileSelection::Release,
            ..Default::default()
        },
        ui: UiOptions::default(),
    })
    .unwrap();

    assert!(root.join("Craft.lock").exists());

    run_command(Command::Publish {
        path: Some(root.clone()),
        feature_selection: FeatureSelection {
            profile: crate::script::ProfileSelection::Release,
            ..Default::default()
        },
        ui: UiOptions::default(),
    })
    .unwrap();

    let _ = fs::remove_dir_all(root);
}

#[test]
fn publish_accepts_workspace_package_metadata_for_members() {
    let root = temp_dir("craft-cli-publish-workspace");
    let member = root.join("member");
    fs::create_dir_all(member.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[workspace]
members = ["member"]

[workspace.package]
description = "Shared package metadata"
license = "MIT"
authors = ["Demo <demo@example.com>"]
readme = "README.md"
repository = "https://example.com/workspace"
"#,
    )
    .unwrap();
    fs::write(root.join("README.md"), "# workspace\n").unwrap();
    fs::write(
        member.join("Craft.toml"),
        r#"
[package]
name = "member"
version = "0.1.0"
kern = "0.7.2"

[[bin]]
name = "member"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(member.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();

    run_command(Command::Publish {
        path: Some(root.clone()),
        feature_selection: FeatureSelection {
            profile: crate::script::ProfileSelection::Release,
            ..Default::default()
        },
        ui: UiOptions::default(),
    })
    .unwrap();

    assert!(root.join("Craft.lock").exists());

    let _ = fs::remove_dir_all(root);
}
