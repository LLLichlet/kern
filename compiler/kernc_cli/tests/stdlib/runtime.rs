use super::*;

fn nm_defines_global_symbol(symbols: &str, expected: &str) -> bool {
    symbols.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let Some(name) = fields.next_back() else {
            return false;
        };
        let Some(kind) = fields.next_back() else {
            return false;
        };

        kind != "U" && name.trim_start_matches('_') == expected
    })
}

#[test]
fn direct_source_build_defaults_to_std_rt_and_source_stem_output() {
    let temp_dir = unique_temp_path("kernc_direct_defaults", "dir");
    fs::create_dir_all(&temp_dir).unwrap();

    let source = repo_root().join("examples/hello_world.rn");
    let expected_output = temp_dir.join(format!("hello_world{}", std::env::consts::EXE_SUFFIX));
    let source_arg = source.to_string_lossy().into_owned();

    let output = Command::new(env!("CARGO_BIN_EXE_kernc"))
        .current_dir(&temp_dir)
        .arg(source_arg.as_str())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        expected_output.exists(),
        "expected executable at {}",
        expected_output.display()
    );

    let run_output = Command::new(&expected_output).output().unwrap();
    assert!(
        run_output.status.success(),
        "default binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&run_output.stdout).contains("hello, world!"),
        "unexpected stdout:\n{}",
        String::from_utf8_lossy(&run_output.stdout)
    );

    let _ = fs::remove_file(&expected_output);
    let _ = fs::remove_dir_all(&temp_dir);
}

fn compile_cross_target_std_object(prefix: &str, target: &str) -> std::process::Output {
    // Keep cfg-gated std/runtime codepaths compiled even on non-native CI hosts.
    compile_source_with_args(
        prefix,
        r#"
use std.env;
use std.proc;

fn main(argc: i32, argv: **u8) i32 {
    let args = proc.args(argc, argv);
    let _ = args.len();
    let _ = args.contains("--help");
    let _ = args.position("--help");
    let _ = args.value_after("--target");
    let _ = args.find_prefixed("--profile=");
    let mut saw_entry = false;

    let visited = env.visit(.[saw_entry = saw_entry..&](entry: env.Var) bool {
        let _ = entry.name;
        let _ = entry.value;
        saw_entry.* = true;
        return false;
    });
    let _ = visited;
    let _ = saw_entry;
    return 0;
}
"#,
        &[
            "--library-bundle",
            "std",
            "--runtime-entry",
            "rt",
            "--runtime-libc",
            "no",
            "--target",
            target,
        ],
    )
}

#[test]
fn links_hosted_program_with_std_and_crt_startup() {
    let source_path = unique_temp_path("kernc_std_hosted", "rn");
    let exe_ext = if cfg!(windows) { "exe" } else { "out" };
    let executable_path = unique_temp_path("kernc_std_hosted", exe_ext);

    fs::write(
        &source_path,
        r#"
use std.io;

fn main() i32 {
    io.println("hosted std", .{});
    return 0;
}
"#,
    )
    .unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();
    let args = vec![
        "--library-bundle",
        "std",
        "--runtime-entry",
        "crt",
        "--runtime-libc",
        "yes",
        "--print-link-command",
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
fn runs_hosted_program_using_export_name_slice_abi_without_main_special_casing() {
    let root = unique_temp_path("kernc_std_hosted_extern_slice", "dir");
    fs::create_dir_all(&root).unwrap();

    let main_source = root.join("main.rn");
    let bridge_source = root.join("bridge_mod.rn");
    let exe_ext = if cfg!(windows) { "exe" } else { "out" };
    let executable_path = unique_temp_path("kernc_std_hosted_extern_slice", exe_ext);

    fs::write(
        &main_source,
        r#"
mod bridge_mod;

extern {
    fn bridge(args: [][]u8) i32;
}

fn main() i32 {
    let argv = [2][]u8.{ "alpha", "beta gamma", };
    return bridge(argv);
}
"#,
    )
    .unwrap();

    fs::write(
        &bridge_source,
        r#"
#[export_name("bridge")]
extern fn bridge_impl(args: [][]u8) i32 {
    if (#args != 2) {
        return 1;
    }

    let first = args.[0];
    let second = args.[1];
    if (first != "alpha") {
        return 2;
    }
    if (second != "beta gamma") {
        return 3;
    }
    return 0;
}
"#,
    )
    .unwrap();

    let source_arg = main_source.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();
    let output = run_kernc([
        "--library-bundle",
        "std",
        "--runtime-entry",
        "crt",
        "--runtime-libc",
        "yes",
        source_arg.as_str(),
        "-o",
        exe_arg.as_str(),
    ]);

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&main_source);
    let _ = fs::remove_file(&bridge_source);
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn custom_defines_are_available_as_compile_time_constants() {
    let source_path = unique_temp_path("kernc_custom_define_const", "rn");
    let exe_ext = if cfg!(windows) { "exe" } else { "out" };
    let executable_path = unique_temp_path("kernc_custom_define_const", exe_ext);

    fs::write(
        &source_path,
        r#"
fn main() i32 {
    let _ = GREETING_MSG;
    return 0;
}
"#,
    )
    .unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();
    let output = run_kernc([
        "--library-bundle",
        "std",
        "--runtime-entry",
        "crt",
        "--runtime-libc",
        "yes",
        "--define",
        "GREETING_MSG=Hello from injected define",
        source_arg.as_str(),
        "-o",
        exe_arg.as_str(),
    ]);

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "custom define binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn runs_hosted_program_using_std_proc_shell_capture() {
    let run_output = build_and_run(
        "kernc_std_proc_shell_capture",
        r#"
use base.mem.alloc.GPA;
use std.proc;
use sys.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;

    let mut capture = match (proc.shell_capture(gpa, "echo shell_capture")) {
        .{ Ok: value } => value,
        .{ Err: _ } => return 10,
    };
    defer capture..&.deinit(gpa);

    if (!capture.success()) {
        return 20;
    }
    if (capture.len() == 0 or capture.is_empty()) {
        return 30;
    }
    if (!capture.as_str().starts_with("shell_capture")) {
        return 40;
    }

    let status = match (proc.shell_status(gpa, "echo status_only")) {
        .{ Ok: value } => value,
        .{ Err: _ } => return 50,
    };
    if (status != 0) {
        return 60;
    }

    let success = match (proc.shell_success(gpa, "echo success_only")) {
        .{ Ok: value } => value,
        .{ Err: _ } => return 70,
    };
    if (!success) {
        return 80;
    }

    return 0;
}
"#,
        &[
            "--library-bundle",
            "std",
            "--runtime-entry",
            "crt",
            "--runtime-libc",
            "yes",
        ],
    );

    assert!(
        run_output.status.success(),
        "std.proc shell capture binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );
}

#[test]
fn links_windows_rt_program_with_std_bundle() {
    if !cfg!(windows) {
        return;
    }

    let source = repo_root().join("examples/hello_world.rn");
    let executable_path = unique_temp_path("kernc_std_windows_rt", "exe");

    let source_arg = source.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();
    let output = run_kernc([
        "--library-bundle",
        "std",
        "--runtime-entry",
        "rt",
        source_arg.as_str(),
        "-o",
        exe_arg.as_str(),
    ]);

    assert_success(&output, "kernc");
    assert!(
        executable_path.exists(),
        "expected executable at {}",
        executable_path.display()
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "default rt binary failed:\nstdout:\n{}\nstderr:\n{}",
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
fn links_unix_freestanding_program_without_program_main() {
    if cfg!(windows) || cfg!(target_os = "macos") {
        return;
    }

    let (source_path, executable_path) = build_temp_program(
        "kernc_unix_freestanding_none",
        r#"
#[export_name("_start")]
fn kmain() void {
    while (true) {}
    @unreachable();
}
"#,
        &[
            "--library-bundle",
            "base",
            "--runtime-entry",
            "none",
            "--runtime-libc",
            "no",
            "--entry-symbol",
            "_start",
        ],
    );

    assert!(
        executable_path.exists(),
        "expected freestanding executable at {}",
        executable_path.display()
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn runs_hosted_program_with_indexed_command_line_arguments() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_hosted_args",
        r#"
use std.proc;

fn main(argc: i32, argv: **u8) i32 {
    let args = proc.args(argc, argv);
    if (args.len() != 6) {
        return 1;
    }
    let first = match (args.get(0)) {
        .{ Some: arg } => arg,
        .None => return 2,
    };
    if (#first == 0) {
        return 2;
    }
    let second = match (args.get(1)) {
        .{ Some: arg } => arg,
        .None => return 3,
    };
    if (second != "alpha") {
        return 3;
    }
    let third = match (args.get(2)) {
        .{ Some: arg } => arg,
        .None => return 4,
    };
    if (third != "beta gamma") {
        return 4;
    }
    let alpha_pos = match (args.position("alpha")) {
        .{ Some: index } => index,
        .None => return 5,
    };
    if (alpha_pos != 1) {
        return 6;
    }
    if (!args.contains("--name") or args.contains("--missing")) {
        return 7;
    }
    let name = match (args.value_after("--name")) {
        .{ Some: value } => value,
        .None => return 8,
    };
    if (name != "kern") {
        return 9;
    }
    let cfg = match (args.find_prefixed("--cfg=")) {
        .{ Some: value } => value,
        .None => return 10,
    };
    if (cfg != "fast") {
        return 11;
    }
    if (args.value_after("--cfg=fast").is_some()) {
        return 12;
    }
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path)
        .arg("alpha")
        .arg("beta gamma")
        .arg("--name")
        .arg("kern")
        .arg("--cfg=fast")
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
fn runs_std_time_duration_and_sleep_helpers() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_time",
        r#"
use std.time;
use base.cmp.{LESS, EQUAL, GREATER};

fn main() i32 {
    let fixed = time.from_millis(1500);
    if (fixed.as_secs() != 1) {
        return 1;
    }
    if (fixed.as_millis() != 1500) {
        return 2;
    }
    if (fixed.subsec_nanos() != 500_000_000) {
        return 3;
    }
    if (fixed.div_u64(3).as_millis() != 500) {
        return 4;
    }
    if (fixed.saturating_mul(2).as_secs() != 3) {
        return 5;
    }
    if (fixed.saturating_add(time.from_millis(600)).as_millis() != 2100) {
        return 6;
    }
    if (fixed.saturating_sub(time.from_secs(2)).as_nanos() != 0) {
        return 7;
    }
    if (time.from_secs(2).units_per_sec(400) != 200) {
        return 8;
    }
    if (!(time.from_secs(1) == time.from_millis(1000))) {
        return 13;
    }
    if (time.from_secs(1) == time.from_millis(999)) {
        return 14;
    }
    if (time.from_millis(1).cmp(time.from_secs(1)) != LESS) {
        return 15;
    }
    if (time.from_secs(1).cmp(time.from_millis(1000)) != EQUAL) {
        return 16;
    }
    if (time.from_secs(2).cmp(time.from_secs(1)) != GREATER) {
        return 17;
    }
    let _ = time.from_micros(250).hash();

    let start = time.now();
    time.sleep_nanos(1);
    time.sleep_micros(1);
    time.sleep_millis(10);
    time.sleep_secs(0);
    let elapsed = start.elapsed();
    if (elapsed.as_millis() < 5) {
        return 9;
    }
    if (elapsed.as_nanos() == 0) {
        return 10;
    }
    if (elapsed.div_u64(2).as_nanos() == 0) {
        return 11;
    }
    if (elapsed.units_per_sec(2) == 0) {
        return 12;
    }
    let end = time.now();
    if (end.cmp(start) == LESS) {
        return 18;
    }
    let _ = end.hash();

    return 0;
}
"#,
        &["--library-bundle", "std"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "std time binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn runs_std_term_convenience_helpers() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_term_helpers",
        r#"
use std.term;

fn main() i32 {
    let _ = term.stdin_is_terminal();
    let _ = term.stdout_is_terminal();
    let _ = term.stderr_is_terminal();

    let out_size = term.stdout_size();
    if (out_size.is_ok()) {
        let size = match (out_size) {
            .{ Ok: size } => size,
            .{ Err: _ } => return 1,
        };
        if (size.rows == 0 or size.cols == 0) {
            return 2;
        }
    }

    let _ = term.stdin_size();
    let _ = term.stderr_size();
    let _ = term.stdout_rows();
    let _ = term.stdout_columns();
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "std term binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn accepts_hosted_std_program_with_no_arg_main() {
    let output = compile_source_with_args(
        "kernc_std_hosted_main_without_args",
        r#"
fn main() i32 {
    return 0;
}
"#,
        &[
            "--library-bundle",
            "std",
            "--runtime-entry",
            "crt",
            "--runtime-libc",
            "yes",
        ],
    );

    assert_success(&output, "kernc hosted std no-arg main");
}

#[test]
fn cross_compiles_windows_std_runtime_env_and_proc_paths() {
    let output = compile_cross_target_std_object(
        "kernc_cross_windows_std_runtime_env_proc",
        "x86_64-windows-msvc",
    );

    assert_success(&output, "kernc cross-compile windows std runtime/env/proc");
}

#[test]
fn cross_compiles_x86_64_darwin_std_runtime_env_and_proc_paths() {
    let output = compile_cross_target_std_object(
        "kernc_cross_x86_64_darwin_std_runtime_env_proc",
        "x86_64-apple-darwin",
    );

    assert_success(
        &output,
        "kernc cross-compile x86_64 darwin std runtime/env/proc",
    );
}

#[test]
fn cross_compiles_aarch64_darwin_std_runtime_env_and_proc_paths() {
    let output = compile_cross_target_std_object(
        "kernc_cross_aarch64_darwin_std_runtime_env_proc",
        "aarch64-apple-darwin",
    );

    assert_success(
        &output,
        "kernc cross-compile aarch64 darwin std runtime/env/proc",
    );
}

#[test]
fn hosted_minimal_program_does_not_export_rt_memory_symbols() {
    if cfg!(windows) {
        return;
    }

    let source_path = unique_temp_path("kernc_std_hosted_no_rt_mem", "rn");
    let object_path = unique_temp_path("kernc_std_hosted_no_rt_mem", "o");
    let exe_ext = if cfg!(windows) { "exe" } else { "out" };
    let executable_path = unique_temp_path("kernc_std_hosted_no_rt_mem", exe_ext);

    fs::write(
        &source_path,
        r#"
fn main() i32 {
    return 0;
}
"#,
    )
    .unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let object_arg = object_path.to_string_lossy().into_owned();
    let compile_output = run_kernc([
        "-c",
        "--library-bundle",
        "std",
        "--runtime-entry",
        "crt",
        "--runtime-libc",
        "yes",
        source_arg.as_str(),
        "-o",
        object_arg.as_str(),
    ]);
    assert_success(&compile_output, "kernc compile-only hosted minimal");

    let object_nm = Command::new("nm")
        .arg("-g")
        .arg(&object_path)
        .output()
        .unwrap();
    assert!(
        object_nm.status.success(),
        "nm failed for object:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&object_nm.stdout),
        String::from_utf8_lossy(&object_nm.stderr)
    );
    let object_symbols = String::from_utf8_lossy(&object_nm.stdout);
    for symbol in ["memcpy", "memmove", "memset"] {
        assert!(
            !nm_defines_global_symbol(&object_symbols, symbol),
            "unexpected rt memory symbol `{}` in object:\n{}",
            symbol,
            object_symbols
        );
    }

    let exe_arg = executable_path.to_string_lossy().into_owned();
    let link_output = run_kernc([
        "--library-bundle",
        "std",
        "--runtime-entry",
        "crt",
        "--runtime-libc",
        "yes",
        source_arg.as_str(),
        "-o",
        exe_arg.as_str(),
    ]);
    assert_success(&link_output, "kernc linked hosted minimal");

    let exe_nm = Command::new("nm")
        .arg("-g")
        .arg(&executable_path)
        .output()
        .unwrap();
    assert!(
        exe_nm.status.success(),
        "nm failed for executable:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&exe_nm.stdout),
        String::from_utf8_lossy(&exe_nm.stderr)
    );
    let exe_symbols = String::from_utf8_lossy(&exe_nm.stdout);
    for symbol in ["memcpy", "memmove", "memset"] {
        assert!(
            !nm_defines_global_symbol(&exe_symbols, symbol),
            "unexpected rt memory symbol `{}` in executable:\n{}",
            symbol,
            exe_symbols
        );
    }

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&object_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn rejects_extern_main_when_program_entry_is_enabled() {
    let output = compile_source_with_args(
        "kernc_std_hosted_extern_main",
        r#"
extern fn main() i32 {
    return 0;
}
"#,
        &[
            "--library-bundle",
            "std",
            "--runtime-entry",
            "crt",
            "--runtime-libc",
            "yes",
        ],
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted extern program main:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("program `main` must not be declared `extern`"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_invalid_program_main_parameter_shape() {
    let output = compile_source_with_args(
        "kernc_std_invalid_main_param",
        r#"
fn main(value: i32) i32 {
    return value;
}
"#,
        &[
            "--library-bundle",
            "std",
            "--runtime-entry",
            "crt",
            "--runtime-libc",
            "yes",
        ],
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted invalid program main signature:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("program `main` accepts either zero parameters or exactly `(i32, **u8)`"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn runs_windows_rt_program_with_quoted_command_line_arguments() {
    if !cfg!(windows) {
        return;
    }

    let (source_path, executable_path) = build_temp_program(
        "kernc_std_windows_args",
        r#"
use std.proc;

fn main(argc: i32, argv: **u8) i32 {
    let args = proc.args(argc, argv);
    if (args.len() != 4) {
        return 1;
    }
    let plain = match (args.get(1)) {
        .{ Some: arg } => arg,
        .None => return 2,
    };
    if (plain != "plain") {
        return 2;
    }
    let spaced = match (args.get(2)) {
        .{ Some: arg } => arg,
        .None => return 3,
    };
    if (spaced != "two words") {
        return 3;
    }
    let quoted = match (args.get(3)) {
        .{ Some: arg } => arg,
        .None => return 4,
    };
    if (quoted != "quote\"value") {
        return 4;
    }
    return 0;
}
"#,
        &["--library-bundle", "std"],
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
fn runs_windows_rt_program_with_unicode_command_line_arguments() {
    if !cfg!(windows) {
        return;
    }

    let (source_path, executable_path) = build_temp_program(
        "kernc_std_windows_unicode_args",
        r#"
use std.proc;

fn main(argc: i32, argv: **u8) i32 {
    let args = proc.args(argc, argv);
    if (args.len() != 4) {
        return 1;
    }
    let first = match (args.get(1)) {
        .{ Some: arg } => arg,
        .None => return 2,
    };
    if (first != "\u{6D4B}\u{8BD5}") {
        return 2;
    }
    let second = match (args.get(2)) {
        .{ Some: arg } => arg,
        .None => return 3,
    };
    if (second != "\u{7A7A} \u{767D}") {
        return 3;
    }
    let third = match (args.get(3)) {
        .{ Some: arg } => arg,
        .None => return 4,
    };
    if (third != "emoji-\u{1F642}") {
        return 4;
    }
    return 0;
}
"#,
        &["--library-bundle", "std"],
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
            "--runtime-entry",
            "none",
            "--runtime-libc",
            "no",
            "--library-bundle",
            "none",
            "--entry-symbol",
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
    let source_path = unique_temp_path("kernc_std_env", "rn");
    let exe_ext = if cfg!(windows) { "exe" } else { "out" };
    let executable_path = unique_temp_path("kernc_std_env", exe_ext);

    fs::write(
        &source_path,
        r#"
use std.env;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;

    if (!env.has(gpa, "KERN_STD_ENV_TEST")) {
        return 10;
    }
    if (env.has(gpa, "KERN_STD_ENV_MISSING")) {
        return 11;
    }

    let mut found = match (env.get(gpa, "KERN_STD_ENV_TEST")) {
        .{ Some: value } => value,
        .None => return 1,
    };
    defer found..&.deinit(gpa);

    if (found.& != "alpha-beta") {
        return 2;
    }
    if (!env.value_equals("KERN_STD_ENV_TEST", "alpha-beta")) {
        return 20;
    }
    if (env.value_equals("KERN_STD_ENV_TEST", "wrong")) {
        return 21;
    }
    if (env.value_equals("KERN_STD_ENV_MISSING", "alpha-beta")) {
        return 22;
    }
    let mut visited_value = false;
    let found_value = env.visit_value("KERN_STD_ENV_TEST", .[visited_value = visited_value..&](value: []u8) bool {
        if (value != "alpha-beta") {
            return false;
        }
        visited_value.* = true;
        return false;
    });
    if (!found_value or !visited_value) {
        return 23;
    }
    if (env.visit_value("KERN_STD_ENV_MISSING", .[](value: []u8) bool {
        let _ = value;
        return false;
    })) {
        return 24;
    }

    if (env.get(gpa, "KERN_STD_ENV_MISSING").is_some()) {
        return 3;
    }

    let mut fallback = match (env.get_or_clone(gpa, "KERN_STD_ENV_MISSING", "fallback")) {
        .{ Some: value } => value,
        .None => return 4,
    };
    defer fallback..&.deinit(gpa);
    if (fallback.& != "fallback") {
        return 5;
    }

    let mut empty = match (env.get_or_empty(gpa, "KERN_STD_ENV_MISSING")) {
        .{ Some: value } => value,
        .None => return 6,
    };
    defer empty..&.deinit(gpa);
    if (!empty.&.is_empty()) {
        return 7;
    }

    let mut saw_target = false;
    let visited = env.visit(.[saw_target = saw_target..&](entry: env.Var) bool {
        if (entry.name_eq("KERN_STD_ENV_TEST")) {
            if (!entry.value_eq("alpha-beta") or !entry.eq("KERN_STD_ENV_TEST", "alpha-beta")) {
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
        "--library-bundle",
        "std",
        "--runtime-entry",
        "crt",
        "--runtime-libc",
        "yes",
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

#[test]
fn rejects_unknown_runtime_provider_flag() {
    let output = compile_source_with_args(
        "kernc_unknown_runtime_provider",
        r#"
fn main() i32 {
    return 0;
}
"#,
        &[
            "--runtime-entry",
            "rt",
            "--runtime-provider",
            "toolchain",
            "--library-bundle",
            "std",
        ],
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted unknown runtime-provider flag:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Unrecognized option `--runtime-provider`"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
