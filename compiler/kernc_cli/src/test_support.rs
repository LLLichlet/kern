use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static UNIQUE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    Command::new(kernc_binary())
        .current_dir(repo_root())
        .args(args)
        .output()
        .unwrap()
}

fn maybe_add_default_runtime_contract(args: &mut Vec<String>) {
    if args.iter().any(|arg| arg == "--runtime-entry") {
        return;
    }

    if args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "-c" | "--emit-llvm" | "--link-only" | "--entry-symbol"
        )
    }) {
        return;
    }

    let links_libc = args.windows(2).any(|window| {
        window[0] == "--runtime-libc" && matches!(window[1].as_str(), "yes" | "true" | "on")
    });
    let entry = if links_libc { "crt" } else { "rt" };
    args.push("--runtime-entry".to_string());
    args.push(entry.to_string());

    if !args.iter().any(|arg| arg == "--runtime-provider") {
        args.push("--runtime-provider".to_string());
        args.push("toolchain".to_string());
    }
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

    let run_output = Command::new(&executable_path).output().unwrap();

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
    run_output
}
