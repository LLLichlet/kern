use std::fs;
use std::path::{Path, PathBuf};

use kernc_cli::test_support::{build_and_run, compile_source_with_args};

#[derive(Default)]
struct SoundnessCase {
    compile_args: Vec<String>,
    stderr_substrings: Vec<String>,
    exit_code: Option<i32>,
    source: String,
}

#[test]
fn reject_cases() {
    run_reject_cases(&cases_in("reject"));
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
