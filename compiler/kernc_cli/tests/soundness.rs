use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use kernc_cli::test_support::{
    build_and_run, compile_source_with_args, executable_extension, repo_root, run_kernc,
    unique_temp_path,
};
use kernc_utils::config::resolve_base_path;

#[derive(Default)]
struct SoundnessCase {
    compile_args: Vec<String>,
    module_paths: Vec<(String, String)>,
    module_interface_paths: Vec<(String, String)>,
    stderr_substrings: Vec<String>,
    exit_code: Option<i32>,
    timeout_ms: Option<u64>,
    source: String,
}

enum TimedCompileResult {
    Output(Output),
    TimedOut,
}

fn maybe_add_default_runtime_contract(args: &mut Vec<String>) {
    if args.iter().any(|arg| arg == "--runtime-entry") {
        return;
    }

    if args.iter().any(|arg| {
        arg == "-c"
            || arg == "--link-only"
            || arg == "--entry-symbol"
            || arg == "--emit-llvm"
            || arg.starts_with("--emit-llvm=")
    }) {
        return;
    }

    let links_libc = args.windows(2).any(|window| {
        window[0] == "--runtime-libc" && matches!(window[1].as_str(), "yes" | "true" | "on")
    });
    let entry = if links_libc { "crt" } else { "rt" };
    let has_bundle = args.iter().any(|arg| arg == "--library-bundle");
    let has_base_alias = has_module_alias(args, "base");
    args.push("--runtime-entry".to_string());
    args.push(entry.to_string());

    if !has_bundle && !has_base_alias {
        args.push("--module-path".to_string());
        args.push(format!("base={}", resolve_base_path().display()));
    }
}

fn has_module_alias(args: &[String], name: &str) -> bool {
    args.windows(2).any(|window| {
        window[0] == "--module-path"
            && window[1]
                .split_once('=')
                .is_some_and(|(alias, _)| alias == name)
    })
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
fn known_bug_compile_cases() {
    run_known_bug_compile_cases(&cases_in("known-bug-compile"));
}

#[test]
fn known_bug_reject_cases() {
    run_known_bug_reject_cases(&cases_in("known-bug-reject"));
}

#[test]
fn known_bug_timeout_cases() {
    run_known_bug_timeout_cases(&cases_in("known-bug-timeout"));
}

#[test]
fn build_pass_cases() {
    run_build_pass_cases(&cases_in("build-pass"));
}

#[test]
fn run_pass_cases() {
    run_run_pass_cases(&cases_in("run-pass"));
}

#[test]
fn tree_run_pass_cases() {
    run_tree_run_pass_cases(&case_dirs_in("tree-run-pass"));
}

#[test]
fn known_bug_run_cases() {
    run_run_pass_cases(&cases_in("known-bug-run"));
}

fn run_reject_cases(paths: &[PathBuf]) {
    for path in paths {
        let case = parse_case(path);
        let compile_args = case
            .compile_args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let output = match case.timeout_ms {
            Some(timeout_ms) => match compile_source_with_args_timeout(
                "kernc_soundness_reject",
                &case.source,
                &compile_args,
                Duration::from_millis(timeout_ms),
            ) {
                TimedCompileResult::Output(output) => output,
                TimedCompileResult::TimedOut => {
                    panic!(
                        "{} timed out after {} ms while compiling",
                        path.display(),
                        timeout_ms
                    );
                }
            },
            None => compile_source_with_args("kernc_soundness_reject", &case.source, &compile_args),
        };

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
        let output = compile_source_case_output(path, &case, "kernc_soundness_build_pass");

        assert!(
            output.status.success(),
            "{} failed to compile:\nstdout:\n{}\nstderr:\n{}",
            path.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn run_known_bug_compile_cases(paths: &[PathBuf]) {
    for path in paths {
        let case = parse_case(path);
        let compile_args = case
            .compile_args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let output = compile_source_with_args(
            "kernc_soundness_known_bug_compile",
            &case.source,
            &compile_args,
        );

        assert!(
            output.status.success(),
            "{} no longer reproduces its known compile-time bug:\nstdout:\n{}\nstderr:\n{}",
            path.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn run_known_bug_reject_cases(paths: &[PathBuf]) {
    for path in paths {
        let case = parse_case(path);
        let compile_args = case
            .compile_args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let output = compile_source_with_args(
            "kernc_soundness_known_bug_reject",
            &case.source,
            &compile_args,
        );

        assert!(
            !output.status.success(),
            "{} no longer reproduces its known reject-time bug:\nstdout:\n{}\nstderr:\n{}",
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

fn run_known_bug_timeout_cases(paths: &[PathBuf]) {
    for path in paths {
        let case = parse_case(path);
        let compile_args = case
            .compile_args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let timeout = Duration::from_millis(case.timeout_ms.unwrap_or(2_000));
        let result = compile_source_with_args_timeout(
            "kernc_soundness_known_bug_timeout",
            &case.source,
            &compile_args,
            timeout,
        );

        match result {
            TimedCompileResult::TimedOut => {}
            TimedCompileResult::Output(output) => {
                panic!(
                    "{} no longer reproduces its known timeout bug:\nstdout:\n{}\nstderr:\n{}",
                    path.display(),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }
}

fn run_reject_tree_cases(paths: &[PathBuf]) {
    for path in paths {
        let case = parse_case(&path.join("main.rn"));
        let output = compile_case_tree_output(path, &case);
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

fn run_run_pass_cases(paths: &[PathBuf]) {
    for path in paths {
        let case = parse_case(path);
        let output = build_and_run_case_output(path, &case, "kernc_soundness_run_pass");
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

fn run_tree_run_pass_cases(paths: &[PathBuf]) {
    for path in paths {
        let case = parse_case(&path.join("main.rn"));
        let output = build_and_run_case_tree_output(path, &case);
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

fn compile_source_case_output(path: &Path, case: &SoundnessCase, prefix: &str) -> Output {
    let compile_args = case
        .compile_args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    match case.timeout_ms {
        Some(timeout_ms) => match compile_source_with_args_timeout(
            prefix,
            &case.source,
            &compile_args,
            Duration::from_millis(timeout_ms),
        ) {
            TimedCompileResult::Output(output) => output,
            TimedCompileResult::TimedOut => {
                panic!(
                    "{} timed out after {} ms while compiling",
                    path.display(),
                    timeout_ms
                );
            }
        },
        None => compile_source_with_args(prefix, &case.source, &compile_args),
    }
}

fn compile_case_tree_output(case_root: &Path, case: &SoundnessCase) -> Output {
    let temp_dir = unique_temp_path("kernc_soundness_tree", "dir");
    copy_case_tree(case_root, &temp_dir);

    let main = temp_dir.join("main.rn");
    let output_path = unique_temp_path("kernc_soundness_tree", executable_extension());

    let mut args: Vec<String> = case.compile_args.clone();
    for (alias, rel_path) in &case.module_interface_paths {
        let source_root = temp_dir.join(rel_path);
        let metadata_root = temp_dir.join(format!(".soundness-kmeta-{}", alias));
        compile_interface_package(&source_root, &metadata_root, case.timeout_ms, case_root);
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

    let output = match case.timeout_ms {
        Some(timeout_ms) => {
            match run_kernc_with_timeout(&args, Duration::from_millis(timeout_ms)) {
                TimedCompileResult::Output(output) => output,
                TimedCompileResult::TimedOut => {
                    let _ = fs::remove_file(&output_path);
                    let _ = fs::remove_dir_all(&temp_dir);
                    panic!(
                        "{} timed out after {} ms while compiling",
                        case_root.display(),
                        timeout_ms
                    );
                }
            }
        }
        None => run_kernc(args.iter().map(OsStr::new)),
    };

    let _ = fs::remove_file(output_path);
    let _ = fs::remove_dir_all(temp_dir);
    output
}

fn build_and_run_case_tree_output(case_root: &Path, case: &SoundnessCase) -> Output {
    let temp_dir = unique_temp_path("kernc_soundness_tree_run", "dir");
    copy_case_tree(case_root, &temp_dir);

    let main = temp_dir.join("main.rn");
    let output_path = unique_temp_path("kernc_soundness_tree_run", executable_extension());

    let mut args: Vec<String> = case.compile_args.clone();
    maybe_add_default_runtime_contract(&mut args);
    for (alias, rel_path) in &case.module_interface_paths {
        let source_root = temp_dir.join(rel_path);
        let metadata_root = temp_dir.join(format!(".soundness-kmeta-{}", alias));
        compile_interface_package(&source_root, &metadata_root, case.timeout_ms, case_root);
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

    let compile_output = match case.timeout_ms {
        Some(timeout_ms) => {
            match run_kernc_with_timeout(&args, Duration::from_millis(timeout_ms)) {
                TimedCompileResult::Output(output) => output,
                TimedCompileResult::TimedOut => {
                    let _ = fs::remove_file(&output_path);
                    let _ = fs::remove_dir_all(&temp_dir);
                    panic!(
                        "{} timed out after {} ms while compiling",
                        case_root.display(),
                        timeout_ms
                    );
                }
            }
        }
        None => run_kernc(args.iter().map(OsStr::new)),
    };

    assert!(
        compile_output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compile_output.stdout),
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let run_output = Command::new(&output_path).output().unwrap();

    let _ = fs::remove_file(output_path);
    let _ = fs::remove_dir_all(temp_dir);
    run_output
}

fn build_and_run_case_output(path: &Path, case: &SoundnessCase, prefix: &str) -> Output {
    let compile_args = case
        .compile_args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    match case.timeout_ms {
        Some(timeout_ms) => build_and_run_with_timeout(
            path,
            prefix,
            &case.source,
            &compile_args,
            Duration::from_millis(timeout_ms),
        ),
        None => build_and_run(prefix, &case.source, &compile_args),
    }
}

fn compile_source_with_args_timeout(
    prefix: &str,
    source: &str,
    extra_args: &[&str],
    timeout: Duration,
) -> TimedCompileResult {
    let source_path = unique_temp_path(prefix, "rn");
    let object_path = unique_temp_path(prefix, "o");
    fs::write(&source_path, source).unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let object_arg = object_path.to_string_lossy().into_owned();

    let mut args: Vec<String> = vec!["-c".to_string()];
    args.extend(extra_args.iter().map(|arg| (&arg).to_string()));
    args.push(source_arg);
    args.push("-o".to_string());
    args.push(object_arg);

    let result = run_kernc_with_timeout(&args, timeout);

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&object_path);
    result
}

fn build_and_run_with_timeout(
    path: &Path,
    prefix: &str,
    source: &str,
    compile_args: &[&str],
    timeout: Duration,
) -> Output {
    let source_path = unique_temp_path(prefix, "rn");
    let executable_path = unique_temp_path(prefix, executable_extension());
    fs::write(&source_path, source).unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();

    let mut args: Vec<String> = compile_args.iter().map(|arg| (&arg).to_string()).collect();
    maybe_add_default_runtime_contract(&mut args);
    args.push(source_arg);
    args.push("-o".to_string());
    args.push(exe_arg);

    let compile_output = match run_kernc_with_timeout(&args, timeout) {
        TimedCompileResult::Output(output) => output,
        TimedCompileResult::TimedOut => {
            let _ = fs::remove_file(&source_path);
            let _ = fs::remove_file(&executable_path);
            panic!(
                "{} timed out after {} ms while compiling",
                path.display(),
                timeout.as_millis()
            );
        }
    };
    assert!(
        compile_output.status.success(),
        "{} failed to compile:\nstdout:\n{}\nstderr:\n{}",
        path.display(),
        String::from_utf8_lossy(&compile_output.stdout),
        String::from_utf8_lossy(&compile_output.stderr)
    );

    let run_output = match run_binary_with_timeout(&executable_path, timeout) {
        TimedCompileResult::Output(output) => output,
        TimedCompileResult::TimedOut => {
            let _ = fs::remove_file(&source_path);
            let _ = fs::remove_file(&executable_path);
            panic!(
                "{} timed out after {} ms while running",
                path.display(),
                timeout.as_millis()
            );
        }
    };

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
    run_output
}

fn run_kernc_with_timeout(args: &[String], timeout: Duration) -> TimedCompileResult {
    let mut child = Command::new(kernc_binary())
        .current_dir(repo_root())
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();

    loop {
        if child.try_wait().unwrap().is_some() {
            return TimedCompileResult::Output(child.wait_with_output().unwrap());
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return TimedCompileResult::TimedOut;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn run_binary_with_timeout(executable: &Path, timeout: Duration) -> TimedCompileResult {
    let mut child = Command::new(executable)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();

    loop {
        if child.try_wait().unwrap().is_some() {
            return TimedCompileResult::Output(child.wait_with_output().unwrap());
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return TimedCompileResult::TimedOut;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn kernc_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_kernc").map(PathBuf::from) {
        return path;
    }

    let mut path = std::env::current_exe().expect("missing current test executable path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(if cfg!(windows) { "kernc.exe" } else { "kernc" });
    path
}

fn compile_interface_package(
    source_root: &Path,
    metadata_root: &Path,
    timeout_ms: Option<u64>,
    case_root: &Path,
) {
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
    let args = [
        OsStr::new("-c"),
        OsStr::new("--module-root-name"),
        OsStr::new(module_root_name),
        OsStr::new("--metadata-output"),
        metadata_root.as_os_str(),
        entry.as_os_str(),
        OsStr::new("-o"),
        object_path.as_os_str(),
    ]
    .into_iter()
    .map(|arg| arg.to_string_lossy().to_string())
    .collect::<Vec<_>>();
    let output = match timeout_ms {
        Some(timeout_ms) => {
            match run_kernc_with_timeout(&args, Duration::from_millis(timeout_ms)) {
                TimedCompileResult::Output(output) => output,
                TimedCompileResult::TimedOut => {
                    panic!(
                        "{} timed out after {} ms while compiling interface package `{}`",
                        case_root.display(),
                        timeout_ms,
                        source_root.display()
                    );
                }
            }
        }
        None => run_kernc(args.iter().map(OsStr::new)),
    };
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

    let entries =
        fs::read_dir(src).unwrap_or_else(|err| panic!("failed to read {}: {}", src.display(), err));
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
        } else if let Some(value) = directive.strip_prefix("timeout-ms:") {
            case.timeout_ms = Some(value.trim().parse().unwrap_or_else(|err| {
                panic!(
                    "invalid `timeout-ms` directive in {}: {} ({})",
                    path.display(),
                    value.trim(),
                    err
                )
            }));
        }
    }

    case
}
