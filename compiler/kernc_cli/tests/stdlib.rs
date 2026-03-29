use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn unique_temp_path(prefix: &str, extension: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let file_name = format!("{}_{}_{}.{}", prefix, std::process::id(), nanos, extension);
    std::env::temp_dir().join(file_name)
}

fn run_kernc(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_kernc"))
        .current_dir(repo_root())
        .args(args)
        .output()
        .unwrap()
}

fn build_temp_program(prefix: &str, source: &str, base_args: &[&str]) -> (PathBuf, PathBuf) {
    let source_path = unique_temp_path(prefix, "kr");
    let exe_ext = if cfg!(windows) { "exe" } else { "out" };
    let executable_path = unique_temp_path(prefix, exe_ext);

    fs::write(&source_path, source).unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();

    let mut args = Vec::with_capacity(base_args.len() + 3);
    args.extend_from_slice(base_args);
    args.push(source_arg.as_str());
    args.push("-o");
    args.push(exe_arg.as_str());

    let output = run_kernc(&args);
    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    (source_path, executable_path)
}

fn assert_not_textual_llvm_ir(path: &Path) {
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

#[test]
fn compiles_std_hello_world_in_compile_only_mode() {
    let source = repo_root().join("examples/hello_world.kr");
    let object = unique_temp_path("kernc_std_hello_world", "o");

    let source_arg = source.to_string_lossy().into_owned();
    let object_arg = object.to_string_lossy().into_owned();
    let args = vec![
        "-c",
        "--use-std",
        source_arg.as_str(),
        "-o",
        object_arg.as_str(),
    ];
    let output = run_kernc(&args);

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        object.exists(),
        "expected object file at {}",
        object.display()
    );
    assert_not_textual_llvm_ir(&object);

    let _ = fs::remove_file(&object);
}

#[test]
fn links_compile_only_object_via_link_only_mode() {
    let source_path = unique_temp_path("kernc_std_link_only", "kr");
    let object_path = unique_temp_path("kernc_std_link_only", "o");
    let exe_ext = if cfg!(windows) { "exe" } else { "out" };
    let executable_path = unique_temp_path("kernc_std_link_only", exe_ext);

    fs::write(
        &source_path,
        r#"
use std.io;

extern fn main() i32 {
    io.println("link only", .{});
    return 0;
}
"#,
    )
    .unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let object_arg = object_path.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();

    let compile_output = run_kernc(&[
        "-c",
        "--use-std",
        "--link-profile",
        "hosted",
        source_arg.as_str(),
        "-o",
        object_arg.as_str(),
    ]);
    assert!(
        compile_output.status.success(),
        "kernc compile-only failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compile_output.stdout),
        String::from_utf8_lossy(&compile_output.stderr)
    );
    assert_not_textual_llvm_ir(&object_path);

    let link_output = run_kernc(&[
        "--link-only",
        "--link-profile",
        "hosted",
        "--link-input",
        object_arg.as_str(),
        "-o",
        exe_arg.as_str(),
    ]);
    assert!(
        link_output.status.success(),
        "kernc link-only failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&link_output.stdout),
        String::from_utf8_lossy(&link_output.stderr)
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "link-only binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&object_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn links_hosted_program_with_std_without_kern_entry_shims() {
    let source_path = unique_temp_path("kernc_std_hosted", "kr");
    let exe_ext = if cfg!(windows) { "exe" } else { "out" };
    let executable_path = unique_temp_path("kernc_std_hosted", exe_ext);

    fs::write(
        &source_path,
        r#"
use std.io;

extern fn main() i32 {
    io.println("hosted std", .{});
    return 0;
}
"#,
    )
    .unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();
    let args = vec![
        "--use-std",
        "--link-profile",
        "hosted",
        source_arg.as_str(),
        "-o",
        exe_arg.as_str(),
    ];
    let output = run_kernc(&args);

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        executable_path.exists(),
        "expected executable at {}",
        executable_path.display()
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn links_windows_kern_program_with_std_by_default() {
    if !cfg!(windows) {
        return;
    }

    let source = repo_root().join("examples/hello_world.kr");
    let executable_path = unique_temp_path("kernc_std_windows_kern", "exe");

    let source_arg = source.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();
    let output = run_kernc(&["--use-std", source_arg.as_str(), "-o", exe_arg.as_str()]);

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        executable_path.exists(),
        "expected executable at {}",
        executable_path.display()
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "default kern binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&run_output.stdout).contains("hello, world!"),
        "unexpected stdout:\n{}",
        String::from_utf8_lossy(&run_output.stdout)
    );

    let _ = fs::remove_file(&executable_path);
}

#[test]
fn runs_windows_kern_program_with_quoted_command_line_arguments() {
    if !cfg!(windows) {
        return;
    }

    let (source_path, executable_path) = build_temp_program(
        "kernc_std_windows_args",
        r#"
extern fn main(args: [][]u8) i32 {
    if (#args != 4) {
        return 1;
    }
    if (!args.[1].eq("plain")) {
        return 2;
    }
    if (!args.[2].eq("two words")) {
        return 3;
    }
    if (!args.[3].eq("quote\"value")) {
        return 4;
    }
    return 0;
}
"#,
        &["--use-std"],
    );

    let run_output = Command::new(&executable_path)
        .arg("plain")
        .arg("two words")
        .arg("quote\"value")
        .output()
        .unwrap();
    assert!(
        run_output.status.success(),
        "kern std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn runs_windows_kern_program_with_unicode_command_line_arguments() {
    if !cfg!(windows) {
        return;
    }

    let (source_path, executable_path) = build_temp_program(
        "kernc_std_windows_unicode_args",
        r#"
extern fn main(args: [][]u8) i32 {
    if (#args != 4) {
        return 1;
    }
    if (!args.[1].eq("测试")) {
        return 2;
    }
    if (!args.[2].eq("空 白")) {
        return 3;
    }
    if (!args.[3].eq("emoji-🙂")) {
        return 4;
    }
    return 0;
}
"#,
        &["--use-std"],
    );

    let run_output = Command::new(&executable_path)
        .arg("\u{6D4B}\u{8BD5}")
        .arg("\u{7A7A} \u{767D}")
        .arg("emoji-\u{1F642}")
        .output()
        .unwrap();
    assert!(
        run_output.status.success(),
        "kern std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn links_windows_freestanding_program_with_explicit_entry() {
    if !cfg!(windows) {
        return;
    }

    let (source_path, executable_path) = build_temp_program(
        "kernc_windows_freestanding",
        r#"
extern {
    fn ExitProcess(code: u32) void;
}

#[export_name("mainCRTStartup")]
extern fn start() void {
    ExitProcess(0);
}
"#,
        &[
            "--link-profile",
            "freestanding",
            "--entry",
            "mainCRTStartup",
            "-l",
            "kernel32",
        ],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "freestanding binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn runs_hosted_program_using_std_env_get() {
    let source_path = unique_temp_path("kernc_std_env", "kr");
    let exe_ext = if cfg!(windows) { "exe" } else { "out" };
    let executable_path = unique_temp_path("kernc_std_env", exe_ext);

    fs::write(
        &source_path,
        r#"
use std.env;
use std.mem.alloc.{PageAllocator, GPAllocator};

extern fn main() i32 {
    let page = PageAllocator.{}..&;
    let gpa = GPAllocator.{ backing: page }..&;

    if (!env.has(gpa, "KERN_STD_ENV_TEST")) {
        return 10;
    }
    if (env.has(gpa, "KERN_STD_ENV_MISSING")) {
        return 11;
    }

    let mut found = match (env.get(gpa, "KERN_STD_ENV_TEST")) {
        .Some: value => value,
        .None => return 1,
    };
    defer found..&.deinit(gpa);

    if (!found.&.eq("alpha-beta")) {
        return 2;
    }

    if (env.get(gpa, "KERN_STD_ENV_MISSING").is_some()) {
        return 3;
    }

    let mut fallback = match (env.get_or_clone(gpa, "KERN_STD_ENV_MISSING", "fallback")) {
        .Some: value => value,
        .None => return 4,
    };
    defer fallback..&.deinit(gpa);
    if (!fallback.&.eq("fallback")) {
        return 5;
    }

    let mut empty = match (env.get_or_empty(gpa, "KERN_STD_ENV_MISSING")) {
        .Some: value => value,
        .None => return 6,
    };
    defer empty..&.deinit(gpa);
    if (!empty.&.is_empty()) {
        return 7;
    }

    let mut saw_target = false;
    let visited = env.visit(.[saw_target = saw_target..&](entry: env.Var) bool {
        if (entry.name.eq("KERN_STD_ENV_TEST")) {
            if (!entry.value.eq("alpha-beta")) {
                return false;
            }
            saw_target.* = true;
        }
        return true;
    });
    if (visited == 0) {
        return 8;
    }
    if (!saw_target) {
        return 9;
    }

    return 0;
}
"#,
    )
    .unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();
    let args = vec![
        "--use-std",
        "--link-profile",
        "hosted",
        source_arg.as_str(),
        "-o",
        exe_arg.as_str(),
    ];
    let output = run_kernc(&args);

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let run_output = Command::new(&executable_path)
        .env("KERN_STD_ENV_TEST", "alpha-beta")
        .output()
        .unwrap();
    assert!(
        run_output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}
