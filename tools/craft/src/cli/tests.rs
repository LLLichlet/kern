use super::{
    ColorChoice, Command, RunSelection, UiOptions, parse_args, run_command,
    summarize_check_sources, summarize_source_security, validate_check_source_policy,
};
use crate::elaborate::FeatureSelection;
use crate::graph::SourceId;
use crate::manifest::{Manifest, ReleaseSourcePolicy};
use crate::operation_lock::WorkspaceOperationLock;
use crate::resolver::{ExternalPackageId, ResolvedExternalPackage, ResolvedGraph};
use std::fs;
use std::path::PathBuf;
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
kern = "0.7.0"

[[bin]]
name = "demo"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(root.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();
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
kern = "0.7.0"

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
kern = "0.7.0"

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
kern = "0.7.0"

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
        Command::Help
    ));
    assert!(matches!(
        parse_args(["build".to_string(), "--version".to_string()]).unwrap(),
        Command::Version
    ));
}

#[test]
fn parses_lock_with_inline_feature_option() {
    let cmd = parse_args(["lock".to_string(), "--features=ssl".to_string()]).unwrap();

    match cmd {
        Command::Lock {
            path,
            feature_selection,
            ui,
        } => {
            assert!(path.is_none());
            assert!(feature_selection.enable_default);
            assert_eq!(feature_selection.explicit.len(), 1);
            assert!(feature_selection.explicit.contains("ssl"));
            assert_eq!(ui, UiOptions::default());
        }
        other => panic!("expected lock command, got {other:?}"),
    }
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
            assert!(ui.verbose);
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
        } => {
            assert_eq!(path.as_deref(), Some(std::path::Path::new("demo")));
            assert!(feature_selection.enable_default);
            assert!(feature_selection.explicit.is_empty());
            assert_eq!(ui, UiOptions::default());
            assert_eq!(selection, RunSelection::DefaultBin);
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
fn parses_test_with_inline_feature_option() {
    let cmd = parse_args(["test".to_string(), "--features=simd".to_string()]).unwrap();

    match cmd {
        Command::Test {
            path,
            feature_selection,
            ui,
        } => {
            assert!(path.is_none());
            assert!(feature_selection.enable_default);
            assert_eq!(feature_selection.explicit.len(), 1);
            assert!(feature_selection.explicit.contains("simd"));
            assert_eq!(ui, UiOptions::default());
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
                    verbose: true,
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
                    verbose: true,
                    timings: false,
                    color: ColorChoice::Always,
                }
            );
        }
        other => panic!("expected build command, got {other:?}"),
    }
}

#[test]
fn parses_timings_flag() {
    let cmd = parse_args(["build".to_string(), "--timings".to_string()]).unwrap();

    match cmd {
        Command::Build { ui, .. } => {
            assert_eq!(
                ui,
                UiOptions {
                    verbose: false,
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
                    verbose: false,
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
kern = "0.7.0"

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
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let summary = summarize_source_security(&manifest);
    assert_eq!(summary.policy_mode, ReleaseSourcePolicy::Warn);
    assert_eq!(summary.floating_git_sources, 3);
    assert_eq!(summary.insecure_transport_sources, 1);
    assert_eq!(
        summary.warnings,
        vec![
            "blocked(floating-git)".to_string(),
            "insecure(floating-git)".to_string(),
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
fn run_command_can_execute_selected_example() {
    let root = temp_dir("craft-cli-run-example");
    write_bin_and_example_package(&root);

    run_command(Command::Run {
        path: Some(root.clone()),
        feature_selection: FeatureSelection::default(),
        ui: UiOptions::default(),
        selection: RunSelection::Example("sample".to_string()),
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
kern = "0.7.0"

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
kern = "0.7.0"

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
kern = "0.7.0"

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
fn publish_requires_current_release_lock_and_metadata() {
    let root = temp_dir("craft-cli-publish");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Craft.toml"),
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.0"
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

    let err = run_command(Command::Publish {
        path: Some(root.clone()),
        feature_selection: FeatureSelection {
            profile: crate::script::ProfileSelection::Release,
            ..Default::default()
        },
        ui: UiOptions::default(),
    })
    .unwrap_err();
    assert!(err.to_string().contains("craft lock --profile release"));
    assert!(!root.join("Craft.lock").exists());

    run_command(Command::Lock {
        path: Some(root.clone()),
        feature_selection: FeatureSelection {
            profile: crate::script::ProfileSelection::Release,
            ..Default::default()
        },
        ui: UiOptions::default(),
    })
    .unwrap();

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
kern = "0.7.0"

[[bin]]
name = "member"
root = "src/main.rn"
"#,
    )
    .unwrap();
    fs::write(member.join("src/main.rn"), "fn main() i32 { return 0; }\n").unwrap();

    run_command(Command::Lock {
        path: Some(root.clone()),
        feature_selection: FeatureSelection {
            profile: crate::script::ProfileSelection::Release,
            ..Default::default()
        },
        ui: UiOptions::default(),
    })
    .unwrap();

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
