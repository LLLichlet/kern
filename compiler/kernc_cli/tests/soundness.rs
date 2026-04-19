use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use kernc_cli::test_support::{
    build_and_run, compile_source_with_args, executable_extension, run_kernc, unique_temp_path,
};

#[derive(Default)]
struct SoundnessCase {
    compile_args: Vec<String>,
    module_paths: Vec<(String, String)>,
    module_interface_paths: Vec<(String, String)>,
    stderr_substrings: Vec<String>,
    exit_code: Option<i32>,
    source: String,
}

#[test]
fn reject_cases() {
    run_reject_cases(&cases_in("reject"));
}

#[test]
fn reject_tree_cases() {
    run_reject_tree_cases(&case_dirs_in("tree-reject"));
}

#[test]
fn reject_interface_cases() {
    run_reject_tree_cases(&case_dirs_in("interface-reject"));
}

#[test]
fn build_pass_cases() {
    run_build_pass_cases(&cases_in("build-pass"));
}

#[test]
fn run_pass_cases() {
    run_run_pass_cases(&cases_in("run-pass"));
}

fn run_reject_cases(paths: &[PathBuf]) {
    for path in paths {
        let case = parse_case(path);
        let compile_args = case
            .compile_args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let output =
            compile_source_with_args("kernc_soundness_reject", &case.source, &compile_args);

        assert!(
            !output.status.success(),
            "{} unexpectedly compiled:\nstdout:\n{}\nstderr:\n{}",
            path.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        for needle in &case.stderr_substrings {
            assert!(
                stderr.contains(needle),
                "{} missing stderr fragment `{}`:\n{}",
                path.display(),
                needle,
                stderr
            );
        }
    }
}

fn run_build_pass_cases(paths: &[PathBuf]) {
    for path in paths {
        let case = parse_case(path);
        let compile_args = case
            .compile_args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let output =
            compile_source_with_args("kernc_soundness_build_pass", &case.source, &compile_args);

        assert!(
            output.status.success(),
            "{} failed to compile:\nstdout:\n{}\nstderr:\n{}",
            path.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn run_reject_tree_cases(paths: &[PathBuf]) {
    for path in paths {
        let output = compile_case_tree(path);
        assert!(
            !output.status.success(),
            "{} unexpectedly compiled:\nstdout:\n{}\nstderr:\n{}",
            path.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let case = parse_case(&path.join("main.rn"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        for needle in &case.stderr_substrings {
            assert!(
                stderr.contains(needle),
                "{} missing stderr fragment `{}`:\n{}",
                path.display(),
                needle,
                stderr
            );
        }
    }
}

fn run_run_pass_cases(paths: &[PathBuf]) {
    for path in paths {
        let case = parse_case(path);
        let compile_args = case
            .compile_args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let output = build_and_run("kernc_soundness_run_pass", &case.source, &compile_args);
        let expected_exit = case.exit_code.unwrap_or(0);

        assert_eq!(
            output.status.code(),
            Some(expected_exit),
            "{} returned the wrong exit status:\nstdout:\n{}\nstderr:\n{}",
            path.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn cases_in(kind: &str) -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("soundness")
        .join(kind);
    let mut out = Vec::new();
    collect_case_paths(&root, &mut out);
    out.sort();
    out
}

fn case_dirs_in(kind: &str) -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("soundness")
        .join(kind);
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("main.rn").is_file() {
            out.push(path);
        }
    }
    out.sort();
    out
}

fn collect_case_paths(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_case_paths(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rn") {
            out.push(path);
        }
    }
}

fn compile_case_tree(case_root: &Path) -> Output {
    let temp_dir = unique_temp_path("kernc_soundness_tree", "dir");
    copy_case_tree(case_root, &temp_dir);

    let main = temp_dir.join("main.rn");
    let case = parse_case(&main);
    let output_path = unique_temp_path("kernc_soundness_tree", executable_extension());

    let mut args: Vec<String> = case.compile_args.clone();
    for (alias, rel_path) in &case.module_interface_paths {
        let source_root = temp_dir.join(rel_path);
        let metadata_root = temp_dir.join(format!(".soundness-kmeta-{}", alias));
        compile_interface_package(&source_root, &metadata_root);
        args.push("--module-interface-path".to_string());
        args.push(format!("{}={}", alias, metadata_root.display()));
    }
    for (alias, rel_path) in &case.module_paths {
        args.push("--module-path".to_string());
        args.push(format!("{}={}", alias, temp_dir.join(rel_path).display()));
    }
    args.push(main.display().to_string());
    args.push("-o".to_string());
    args.push(output_path.display().to_string());

    let output = run_kernc(args.iter().map(OsStr::new));

    let _ = fs::remove_file(output_path);
    let _ = fs::remove_dir_all(temp_dir);
    output
}

fn compile_interface_package(source_root: &Path, metadata_root: &Path) {
    let entry = source_root.join("init.rn");
    assert!(
        entry.is_file(),
        "interface package root {} is missing init.rn",
        source_root.display()
    );
    fs::create_dir_all(metadata_root).unwrap_or_else(|err| {
        panic!(
            "failed to create interface metadata root {}: {}",
            metadata_root.display(),
            err
        )
    });

    let object_path = metadata_root.join("iface.o");
    let module_root_name = source_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("iface");
    let output = run_kernc(
        [
            OsStr::new("-c"),
            OsStr::new("--module-root-name"),
            OsStr::new(module_root_name),
            OsStr::new("--metadata-output"),
            metadata_root.as_os_str(),
            entry.as_os_str(),
            OsStr::new("-o"),
            object_path.as_os_str(),
        ]
        .into_iter(),
    );
    assert!(
        output.status.success(),
        "failed to compile interface package {}:\nstdout:\n{}\nstderr:\n{}",
        source_root.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn copy_case_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst)
        .unwrap_or_else(|err| panic!("failed to create {}: {}", dst.display(), err));

    let entries = fs::read_dir(src)
        .unwrap_or_else(|err| panic!("failed to read {}: {}", src.display(), err));
    for entry in entries.flatten() {
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_case_tree(&src_path, &dst_path);
        } else {
            fs::copy(&src_path, &dst_path).unwrap_or_else(|err| {
                panic!(
                    "failed to copy {} to {}: {}",
                    src_path.display(),
                    dst_path.display(),
                    err
                )
            });
        }
    }
}

fn parse_case(path: &Path) -> SoundnessCase {
    let source = fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("failed to read {}: {}", path.display(), err));
    let mut case = SoundnessCase {
        source,
        ..SoundnessCase::default()
    };

    for line in case.source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("//") else {
            break;
        };
        let directive = rest.trim();

        if let Some(value) = directive.strip_prefix("compile-args:") {
            case.compile_args
                .extend(value.split_whitespace().map(str::to_string));
        } else if let Some(value) = directive.strip_prefix("module-path:") {
            let value = value.trim();
            let Some((alias, rel_path)) = value.split_once('=') else {
                panic!(
                    "invalid `module-path` directive in {}: {}",
                    path.display(),
                    value
                );
            };
            case.module_paths
                .push((alias.trim().to_string(), rel_path.trim().to_string()));
        } else if let Some(value) = directive.strip_prefix("module-interface-path:") {
            let value = value.trim();
            let Some((alias, rel_path)) = value.split_once('=') else {
                panic!(
                    "invalid `module-interface-path` directive in {}: {}",
                    path.display(),
                    value
                );
            };
            case.module_interface_paths
                .push((alias.trim().to_string(), rel_path.trim().to_string()));
        } else if let Some(value) = directive.strip_prefix("stderr:") {
            case.stderr_substrings.push(value.trim().to_string());
        } else if let Some(value) = directive.strip_prefix("exit:") {
            case.exit_code = Some(value.trim().parse().unwrap_or_else(|err| {
                panic!(
                    "invalid `exit` directive in {}: {} ({})",
                    path.display(),
                    value.trim(),
                    err
                )
            }));
        }
    }

    case
}
