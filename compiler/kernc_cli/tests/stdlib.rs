mod support;

use std::fs;
use std::process::Command;

use support::{
    assert_not_textual_llvm_ir, assert_success, build_and_run, build_temp_program,
    compile_source_with_args, repo_root, run_kernc, unique_temp_path,
};

#[test]
fn runs_hosted_program_using_gpa_alignment_and_arena_allocator() {
    let output = build_and_run(
        "kernc_std_alloc",
        r#"
use std.mem.Layout;
use std.mem.alloc.{PageAllocator, GPAllocator, ArenaAllocator, BumpAllocator};

extern fn main() i32 {
    let page = PageAllocator.{}..&;

    let gpa = GPAllocator.{ backing: page }..&;
    defer gpa.deinit();

    let aligned = Layout.{ size: 33, align: 256 };
    let ptr_a = match (gpa.alloc(aligned)) {
        .Some: ptr => ptr,
        .None => return 1,
    };
    if (((ptr_a as usize) % 256) != 0) {
        return 2;
    }

    let compact = Layout.{ size: 17, align: 64 };
    let ptr_b = match (gpa.alloc(compact)) {
        .Some: ptr => ptr,
        .None => return 3,
    };
    if (((ptr_b as usize) % 64) != 0) {
        return 4;
    }

    gpa.free(ptr_a, aligned);
    gpa.free(ptr_b, compact);

    let ptr_c = match (gpa.alloc(aligned)) {
        .Some: ptr => ptr,
        .None => return 5,
    };
    if (((ptr_c as usize) % 256) != 0) {
        return 6;
    }
    gpa.free(ptr_c, aligned);

    let arena = ArenaAllocator.{ backing: page }..&;
    defer arena.deinit();

    let arena_a = match (arena.alloc(Layout.{ size: 24, align: 16 })) {
        .Some: ptr => ptr,
        .None => return 7,
    };
    if (((arena_a as usize) % 16) != 0) {
        return 8;
    }

    let arena_b = match (arena.alloc(Layout.{ size: 40, align: 32 })) {
        .Some: ptr => ptr,
        .None => return 9,
    };
    if (((arena_b as usize) % 32) != 0) {
        return 10;
    }
    if ((arena_b as usize) <= (arena_a as usize)) {
        return 11;
    }

    arena.reset();

    let arena_reused = match (arena.alloc(Layout.{ size: 24, align: 16 })) {
        .Some: ptr => ptr,
        .None => return 12,
    };
    if ((arena_reused as usize) != (arena_a as usize)) {
        return 13;
    }

    let bump = BumpAllocator.{ backing: page }..&;
    defer bump.deinit();

    let bump_a = match (bump.alloc(Layout.{ size: 12, align: 8 })) {
        .Some: ptr => ptr,
        .None => return 14,
    };
    let bump_b = match (bump.alloc(Layout.{ size: 12, align: 8 })) {
        .Some: ptr => ptr,
        .None => return 15,
    };
    if ((bump_b as usize) <= (bump_a as usize)) {
        return 16;
    }

    bump.reset();

    let bump_reused = match (bump.alloc(Layout.{ size: 12, align: 8 })) {
        .Some: ptr => ptr,
        .None => return 17,
    };
    if ((bump_reused as usize) != (bump_a as usize)) {
        return 18;
    }

    return 0;
}
"#,
        &["--use-std", "--link-profile", "hosted"],
    );

    assert!(
        output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_gpa_invalid_free_usage() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_alloc_invalid_free",
        r#"
use std.mem.Layout;
use std.mem.alloc.{PageAllocator, GPAllocator};

extern fn main() i32 {
    let page = PageAllocator.{}..&;
    let gpa = GPAllocator.{ backing: page }..&;
    defer gpa.deinit();

    let good = Layout.{ size: 16, align: 16 };
    let ptr = match (gpa.alloc(good)) {
        .Some: ptr => ptr,
        .None => return 1,
    };

    gpa.free(ptr, Layout.{ size: 8, align: 16 });
    return 0;
}
"#,
        &["--use-std", "--link-profile", "hosted"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        !run_output.status.success(),
        "expected invalid GPA free to fail:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn runs_dbg_logging_helpers() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_dbg_helpers",
        r#"
use std.dbg;

extern fn main() i32 {
    dbg.log("boot");
    dbg.debug("trace");
    dbg.assert(true, "should not fail");
    return 0;
}
"#,
        &["--use-std", "--link-profile", "hosted"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected dbg helpers to succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&run_output.stderr);
    assert!(stderr.contains("log: boot"), "unexpected stderr:\n{}", stderr);
    assert!(stderr.contains("debug: trace"), "unexpected stderr:\n{}", stderr);

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn dbg_assert_failure_aborts_with_message() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_dbg_assert_fail",
        r#"
use std.dbg;

extern fn main() i32 {
    dbg.assert(false, "boom");
    return 0;
}
"#,
        &["--use-std", "--link-profile", "hosted"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        !run_output.status.success(),
        "expected dbg.assert(false, ...) to abort:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&run_output.stderr);
    assert!(
        stderr.contains("assertion failed: boom"),
        "unexpected stderr:\n{}",
        stderr
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn hints_about_trailing_comma_for_single_print_argument() {
    let output = compile_source_with_args(
        "kernc_std_print_scalar_hint",
        r#"
use std.io;

extern fn main() i32 {
    io.println("value={}", .{ 42 });
    return 0;
}
"#,
        &["--use-std"],
    );

    assert!(
        !output.status.success(),
        "expected compilation failure, but kernc succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("write `.{ value, }` with a trailing comma"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("scalar initialization"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn compiles_std_hello_world_in_compile_only_mode() {
    let source = repo_root().join("examples/hello_world.rn");
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

#[cfg(windows)]
#[test]
fn compiles_std_hello_world_to_unicode_object_path() {
    let source = repo_root().join("examples/hello_world.rn");
    let object = unique_temp_path("kernc_std_hello_world_对象", "o");

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

    assert_success(&output, "kernc");
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
    let source_path = unique_temp_path("kernc_std_link_only", "rn");
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

    let compile_output = run_kernc([
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

    let link_output = run_kernc([
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
    let source_path = unique_temp_path("kernc_std_hosted", "rn");
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
fn custom_defines_are_available_as_compile_time_constants() {
    let source_path = unique_temp_path("kernc_custom_define_const", "rn");
    let exe_ext = if cfg!(windows) { "exe" } else { "out" };
    let executable_path = unique_temp_path("kernc_custom_define_const", exe_ext);

    fs::write(
        &source_path,
        r#"
extern fn main() i32 {
    let _ = GREETING_MSG;
    return 0;
}
"#,
    )
    .unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();
    let output = run_kernc([
        "--use-std",
        "--link-profile",
        "hosted",
        "-D",
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
fn compile_only_object_does_not_export_synthesized_symbols() {
    if cfg!(windows) {
        return;
    }

    let source_path = unique_temp_path("kernc_internal_symbols", "rn");
    let object_path = unique_temp_path("kernc_internal_symbols", "o");

    fs::write(
        &source_path,
        r#"
use std.io;

fn run_cb(cb: *Fn() i32) i32 {
    return cb();
}

extern fn main(args: [][]u8) i32 {
    let _ = args;
    let value = run_cb(.[]() i32 {
        return 42;
    });
    io.println("{}", .{"world",});
    io.println("{}", .{value,});
    return 0;
}
"#,
    )
    .unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let object_arg = object_path.to_string_lossy().into_owned();
    let output = run_kernc([
        "-c",
        "--use-std",
        "--link-profile",
        "hosted",
        source_arg.as_str(),
        "-o",
        object_arg.as_str(),
    ]);

    assert_success(&output, "kernc");

    let nm_output = Command::new("nm")
        .arg("-g")
        .arg(&object_path)
        .output()
        .unwrap();
    assert!(
        nm_output.status.success(),
        "nm failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&nm_output.stdout),
        String::from_utf8_lossy(&nm_output.stderr)
    );
    let symbols = String::from_utf8_lossy(&nm_output.stdout);
    assert!(
        symbols.lines().any(|line| line.ends_with(" main")),
        "expected exported `main`, got:\n{}",
        symbols
    );
    for hidden in [".str.", "__closure_fn_", "__vtable_"] {
        assert!(
            !symbols.contains(hidden),
            "unexpected exported synthesized symbol `{}`:\n{}",
            hidden,
            symbols
        );
    }

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&object_path);
}

#[test]
fn links_windows_kern_program_with_std_by_default() {
    if !cfg!(windows) {
        return;
    }

    let source = repo_root().join("examples/hello_world.rn");
    let executable_path = unique_temp_path("kernc_std_windows_kern", "exe");

    let source_arg = source.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();
    let output = run_kernc(["--use-std", source_arg.as_str(), "-o", exe_arg.as_str()]);

    assert_success(&output, "kernc");
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
    let source_path = unique_temp_path("kernc_std_env", "rn");
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
