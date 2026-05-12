use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kernc_utils::config::resolve_base_path;

static UNIQUE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static TEST_PROCESS_LIMITER: OnceLock<TestProcessLimiter> = OnceLock::new();

const DEFAULT_KERNC_TIMEOUT: Duration = Duration::from_secs(180);
const DEFAULT_RUN_TIMEOUT: Duration = Duration::from_secs(30);

struct TestProcessLimiter {
    active: Mutex<usize>,
    available: Condvar,
    limit: usize,
}

struct TestProcessSlot;

impl Drop for TestProcessSlot {
    fn drop(&mut self) {
        let limiter = test_process_limiter();
        let mut active = limiter.active.lock().unwrap();
        *active = active.saturating_sub(1);
        limiter.available.notify_one();
    }
}

fn test_process_limiter() -> &'static TestProcessLimiter {
    TEST_PROCESS_LIMITER.get_or_init(|| TestProcessLimiter {
        active: Mutex::new(0),
        available: Condvar::new(),
        limit: configured_process_limit(),
    })
}

fn configured_process_limit() -> usize {
    if let Some(limit) = read_positive_usize_env("KERNC_TEST_PROCESS_LIMIT") {
        return limit;
    }

    let available = thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);
    if available <= 4 {
        1
    } else {
        (available / 4).clamp(1, 4)
    }
}

fn read_positive_usize_env(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn read_duration_env(name: &str, default: Duration) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(Duration::from_millis)
        .unwrap_or(default)
}

fn acquire_test_process_slot() -> TestProcessSlot {
    let limiter = test_process_limiter();
    let mut active = limiter.active.lock().unwrap();
    while *active >= limiter.limit {
        active = limiter.available.wait(active).unwrap();
    }
    *active += 1;
    TestProcessSlot
}

fn run_command_with_timeout(mut command: Command, timeout: Duration, context: &str) -> Output {
    let _slot = acquire_test_process_slot();
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().unwrap_or_else(|err| {
        panic!("failed to spawn {context}: {err}");
    });
    let mut stdout = child.stdout.take().expect("missing piped stdout");
    let mut stderr = child.stderr.take().expect("missing piped stderr");
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });
    let start = std::time::Instant::now();

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(err) => panic!("failed to poll {context}: {err}"),
        }

        if start.elapsed() >= timeout {
            let _ = child.kill();
            let status = child.wait().unwrap_or_else(|err| {
                panic!("failed to wait for timed-out {context}: {err}");
            });
            let output = collect_output(context, status, stdout_reader, stderr_reader);
            panic!(
                "{context} timed out after {} ms\nstdout:\n{}\nstderr:\n{}",
                timeout.as_millis(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        thread::sleep(Duration::from_millis(10));
    };

    collect_output(context, status, stdout_reader, stderr_reader)
}

fn collect_output(
    context: &str,
    status: std::process::ExitStatus,
    stdout_reader: thread::JoinHandle<std::io::Result<Vec<u8>>>,
    stderr_reader: thread::JoinHandle<std::io::Result<Vec<u8>>>,
) -> Output {
    let stdout = stdout_reader
        .join()
        .unwrap_or_else(|_| panic!("failed to join {context} stdout reader"))
        .unwrap_or_else(|err| panic!("failed to read {context} stdout: {err}"));
    let stderr = stderr_reader
        .join()
        .unwrap_or_else(|_| panic!("failed to join {context} stderr reader"))
        .unwrap_or_else(|err| panic!("failed to read {context} stderr: {err}"));
    Output {
        status,
        stdout,
        stderr,
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

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

pub fn unique_temp_path(prefix: &str, extension: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let counter = UNIQUE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = format!(
        "{}_{}_{}_{}.{}",
        prefix,
        std::process::id(),
        nanos,
        counter,
        extension
    );
    std::env::temp_dir().join(file_name)
}

pub fn executable_extension() -> &'static str {
    if cfg!(windows) { "exe" } else { "out" }
}

pub fn kern_string_literal(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\")
}

pub fn run_kernc<I, S>(args: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(kernc_binary());
    command.current_dir(repo_root()).args(args);
    run_command_with_timeout(
        command,
        read_duration_env("KERNC_TEST_TIMEOUT_MS", DEFAULT_KERNC_TIMEOUT),
        "kernc",
    )
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

pub fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{} failed:\nstdout:\n{}\nstderr:\n{}",
        context,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub fn assert_not_textual_llvm_ir(path: &Path) {
    let bytes = fs::read(path).unwrap();
    let head_len = bytes.len().min(64);
    let head = &bytes[..head_len];
    let head_text = String::from_utf8_lossy(head);

    assert!(
        !head_text.contains("; ModuleID") && !head_text.contains("source_filename"),
        "expected a native object file, got textual LLVM IR at {}:\n{}",
        path.display(),
        head_text
    );
}

pub fn compile_source_with_args(prefix: &str, source: &str, extra_args: &[&str]) -> Output {
    let source_path = unique_temp_path(prefix, "rn");
    let object_path = unique_temp_path(prefix, "o");
    fs::write(&source_path, source).unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let object_arg = object_path.to_string_lossy().into_owned();

    let mut args: Vec<String> = vec!["-c".to_string()];
    args.extend(extra_args.iter().map(|arg| (*arg).to_string()));
    args.push(source_arg);
    args.push("-o".to_string());
    args.push(object_arg);

    let output = run_kernc(&args);

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&object_path);
    output
}

pub fn emit_llvm_ir_with_args(prefix: &str, source: &str, extra_args: &[&str]) -> Output {
    let source_path = unique_temp_path(prefix, "rn");
    fs::write(&source_path, source).unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();

    let mut args: Vec<String> = vec!["--emit-llvm".to_string()];
    args.extend(extra_args.iter().map(|arg| (*arg).to_string()));
    args.push(source_arg);

    let output = run_kernc(&args);

    let _ = fs::remove_file(&source_path);
    output
}

pub fn emit_llvm_ir_stage_with_args(
    prefix: &str,
    stage: &str,
    source: &str,
    extra_args: &[&str],
) -> Output {
    let source_path = unique_temp_path(prefix, "rn");
    fs::write(&source_path, source).unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();

    let mut args: Vec<String> = vec![format!("--emit-llvm={stage}")];
    args.extend(extra_args.iter().map(|arg| (*arg).to_string()));
    args.push(source_arg);

    let output = run_kernc(&args);

    let _ = fs::remove_file(&source_path);
    output
}

pub fn compile_source_tree_with_args(
    prefix: &str,
    entry: &str,
    files: &[(&str, &str)],
    extra_args: &[&str],
) -> Output {
    let temp_dir = unique_temp_path(prefix, "dir");
    let object_path = unique_temp_path(prefix, "o");
    fs::create_dir_all(&temp_dir).unwrap();

    for (relative_path, source) in files {
        let path = temp_dir.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, source).unwrap();
    }

    let entry_path = temp_dir.join(entry);
    let entry_arg = entry_path.to_string_lossy().into_owned();
    let object_arg = object_path.to_string_lossy().into_owned();

    let mut args: Vec<String> = extra_args.iter().map(|arg| (*arg).to_string()).collect();
    maybe_add_default_runtime_contract(&mut args);
    args.push(entry_arg);
    args.push("-o".to_string());
    args.push(object_arg);

    let output = run_kernc(&args);

    let _ = fs::remove_file(&object_path);
    let _ = fs::remove_dir_all(&temp_dir);
    output
}

pub fn build_temp_program(prefix: &str, source: &str, base_args: &[&str]) -> (PathBuf, PathBuf) {
    let source_path = unique_temp_path(prefix, "rn");
    let executable_path = unique_temp_path(prefix, executable_extension());

    fs::write(&source_path, source).unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();

    let mut args: Vec<String> = base_args.iter().map(|arg| (*arg).to_string()).collect();
    maybe_add_default_runtime_contract(&mut args);
    args.push(source_arg);
    args.push("-o".to_string());
    args.push(exe_arg);

    let output = run_kernc(&args);
    assert_success(&output, "kernc");

    (source_path, executable_path)
}

pub fn build_and_run(prefix: &str, source: &str, compile_args: &[&str]) -> Output {
    let source_path = unique_temp_path(prefix, "rn");
    let executable_path = unique_temp_path(prefix, executable_extension());

    fs::write(&source_path, source).unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();

    let mut args: Vec<String> = compile_args.iter().map(|arg| (*arg).to_string()).collect();
    maybe_add_default_runtime_contract(&mut args);
    args.push(source_arg);
    args.push("-o".to_string());
    args.push(exe_arg);

    let compile_output = run_kernc(&args);
    assert_success(&compile_output, "kernc");

    let run_output = run_command_with_timeout(
        Command::new(&executable_path),
        read_duration_env("KERNC_TEST_RUN_TIMEOUT_MS", DEFAULT_RUN_TIMEOUT),
        "compiled test binary",
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
    run_output
}
