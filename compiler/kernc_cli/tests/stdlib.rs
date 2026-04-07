mod support;

use std::fs;
use std::process::Command;

use support::{
    assert_not_textual_llvm_ir, assert_success, build_and_run, build_temp_program,
    compile_source_with_args, repo_root, run_kernc, unique_temp_path,
};

#[test]
fn compile_only_std_program_emits_no_std_warnings() {
    let output = compile_source_with_args(
        "kernc_std_compile_no_warnings",
        r#"
fn main() i32 {
    return 0;
}
"#,
        &["--library-bundle", "std"],
    );
    assert_success(&output, "kernc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("warning"),
        "unexpected std warning noise during compile-only build:\n{}",
        stderr
    );
}

#[test]
fn runs_hosted_program_using_gpa_alignment_and_arena() {
    let output = build_and_run(
        "kernc_std_alloc",
        r#"
use base.mem.Layout;
use base.mem.alloc.{GPA, Arena};
use sys.mem.Page;

fn main() i32 {
    let page = Page.{}..&;

    let gpa = GPA.{ backing: page }..&;
    defer gpa.deinit();

    let aligned = Layout.{ size: 33, align: 256 };
    let ptr_a = match (gpa.alloc(aligned)) {
        .{ Some: ptr } => ptr,
        .None => return 1,
    };
    if (((ptr_a as usize) % 256) != 0) {
        return 2;
    }

    let compact = Layout.{ size: 17, align: 64 };
    let ptr_b = match (gpa.alloc(compact)) {
        .{ Some: ptr } => ptr,
        .None => return 3,
    };
    if (((ptr_b as usize) % 64) != 0) {
        return 4;
    }

    gpa.free(ptr_a, aligned);
    gpa.free(ptr_b, compact);

    let ptr_c = match (gpa.alloc(aligned)) {
        .{ Some: ptr } => ptr,
        .None => return 5,
    };
    if (((ptr_c as usize) % 256) != 0) {
        return 6;
    }
    gpa.free(ptr_c, aligned);

    let arena = Arena.{ backing: page }..&;
    defer arena.deinit();

    let arena_a = match (arena.alloc(Layout.{ size: 24, align: 16 })) {
        .{ Some: ptr } => ptr,
        .None => return 7,
    };
    if (((arena_a as usize) % 16) != 0) {
        return 8;
    }

    let arena_b = match (arena.alloc(Layout.{ size: 40, align: 32 })) {
        .{ Some: ptr } => ptr,
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
        .{ Some: ptr } => ptr,
        .None => return 12,
    };
    if ((arena_reused as usize) != (arena_a as usize)) {
        return 13;
    }

    let grow = Arena.{ backing: page }..&;
    defer grow.deinit();

    let bump_a = match (grow.alloc(Layout.{ size: 12, align: 8 })) {
        .{ Some: ptr } => ptr,
        .None => return 14,
    };
    let bump_b = match (grow.alloc(Layout.{ size: 12, align: 8 })) {
        .{ Some: ptr } => ptr,
        .None => return 15,
    };
    if ((bump_b as usize) <= (bump_a as usize)) {
        return 16;
    }

    let large = match (grow.alloc(Layout.{ size: 9000, align: 64 })) {
        .{ Some: ptr } => ptr,
        .None => return 17,
    };
    if (((large as usize) % 64) != 0) {
        return 18;
    }

    grow.reset();

    let large_reused = match (grow.alloc(Layout.{ size: 9000, align: 64 })) {
        .{ Some: ptr } => ptr,
        .None => return 19,
    };
    if ((large_reused as usize) != (large as usize)) {
        return 20;
    }

    let bump_reused = match (grow.alloc(Layout.{ size: 12, align: 8 })) {
        .{ Some: ptr } => ptr,
        .None => return 21,
    };
    if ((bump_reused as usize) <= (large_reused as usize)) {
        return 22;
    }

    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
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
use base.mem.Layout;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    defer gpa.deinit();

    let good = Layout.{ size: 16, align: 16 };
    let ptr = match (gpa.alloc(good)) {
        .{ Some: ptr } => ptr,
        .None => return 1,
    };

    gpa.free(ptr, Layout.{ size: 8, align: 16 });
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
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

fn main() i32 {
    dbg.log("boot {}", .{ 1, });
    dbg.debug("trace {}", .{ "ok", });
    dbg.assert(true, "should not fail {}", .{ 7, });
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected dbg helpers to succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&run_output.stderr);
    assert!(
        stderr.contains("log: boot 1"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("debug: trace ok"),
        "unexpected stderr:\n{}",
        stderr
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn dbg_assert_failure_aborts_with_message() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_dbg_assert_fail",
        r#"
use std.dbg;

fn main() i32 {
    dbg.assert(false, "boom {}", .{ 42, });
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
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
        stderr.contains("assertion failed: boom 42"),
        "unexpected stderr:\n{}",
        stderr
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn runs_test_assertion_helpers() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_test_helpers",
        r#"
use std.test;

fn main() i32 {
    test.assert(true, "should not fail", .{});
    test.eq(usize.{4}, usize.{4});
    test.not_eq(usize.{4}, usize.{5});
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected std.test helpers to succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn test_eq_failure_aborts_with_message() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_test_eq_fail",
        r#"
use std.test;

fn main() i32 {
    test.eq(usize.{4}, usize.{5});
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        !run_output.status.success(),
        "expected std.test.eq failure to abort:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&run_output.stderr);
    assert!(
        stderr.contains("test failed: expected values to be equal"),
        "unexpected stderr:\n{}",
        stderr
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn test_eq_supports_payloadless_user_enums() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_test_enum_eq",
        r#"
use std.test;

type Mode = enum {
    Fast,
    Slow,
};

fn main() i32 {
    test.eq(Mode.Fast, Mode.Fast);
    test.not_eq(Mode.Fast, Mode.Slow);
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected std.test to support payloadless enums:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn runs_test_option_and_result_helpers() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_test_option_result_helpers",
        r#"
use std.test;
use base.{Option, Result};

fn parse(flag: bool) Result[usize, i32] {
    if (flag) {
        return .{ Ok: 7 };
    }
    return .{ Err: -1 };
}

fn main() i32 {
    let some = test.expect_some(Option[usize].{ Some: 9 });
    test.eq(some, usize.{9});
    test.expect_none(Option[usize].{ None });

    let ok = test.expect_ok(parse(true));
    test.eq(ok, usize.{7});

    let err = test.expect_err(parse(false));
    test.eq(err, i32.{-1});
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected std.test option/result helpers to succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn test_expect_some_failure_aborts_with_message() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_test_expect_some_fail",
        r#"
use std.test;
use base.Option;

fn main() i32 {
    let _ = test.expect_some(Option[usize].{ None });
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        !run_output.status.success(),
        "expected std.test.expect_some failure to abort:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&run_output.stderr);
    assert!(
        stderr.contains("test failed: expected option to contain a value"),
        "unexpected stderr:\n{}",
        stderr
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn test_expect_err_failure_aborts_with_message() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_test_expect_err_fail",
        r#"
use std.test;
use base.Result;

fn main() i32 {
    let _ = test.expect_err(Result[usize, i32].{ Ok: 3 });
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        !run_output.status.success(),
        "expected std.test.expect_err failure to abort:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&run_output.stderr);
    assert!(
        stderr.contains("test failed: expected result to be err"),
        "unexpected stderr:\n{}",
        stderr
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn wrapped_fmt_helpers_accept_inline_integer_literals() {
    let output = build_and_run(
        "kernc_std_fmt_wrapper_literals",
        r#"
use std.io;

fn wrap(fmt: []u8, args: []*io.Printable) void {
    io.println(fmt, args);
}

fn main() i32 {
    wrap("{}", .{ 42, });
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    assert!(
        output.status.success(),
        "expected wrapped fmt helper to succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("42"), "unexpected stdout:\n{}", stdout);
}

#[test]
fn hints_about_trailing_comma_for_single_print_argument() {
    let output = compile_source_with_args(
        "kernc_std_print_scalar_hint",
        r#"
use std.io;

fn main() i32 {
    io.println("value={}", .{ 42 });
    return 0;
}
"#,
        &["--library-bundle", "std"],
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
        "--library-bundle",
        "std",
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
    let object = unique_temp_path("kernc_std_hello_world_\u{4F60}\u{597D}", "o");

    let source_arg = source.to_string_lossy().into_owned();
    let object_arg = object.to_string_lossy().into_owned();
    let args = vec![
        "-c",
        "--library-bundle",
        "std",
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

fn main() i32 {
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
        "--library-bundle",
        "std",
        "--runtime-entry",
        "crt",
        "--runtime-provider",
        "toolchain",
        "--runtime-libc",
        "yes",
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
        "--library-bundle",
        "std",
        "--runtime-entry",
        "crt",
        "--runtime-provider",
        "toolchain",
        "--runtime-libc",
        "yes",
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
fn links_hosted_program_with_std_using_toolchain_provider() {
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
        "--runtime-provider",
        "toolchain",
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
    if (!first.eq("alpha")) {
        return 2;
    }
    if (!second.eq("beta gamma")) {
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
        "--runtime-provider",
        "toolchain",
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
        "--runtime-provider",
        "toolchain",
        "--runtime-libc",
        "yes",
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

fn main() i32 {    let value = run_cb(.[]() i32 {
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
        "--library-bundle",
        "std",
        "--runtime-entry",
        "crt",
        "--runtime-provider",
        "toolchain",
        "--runtime-libc",
        "yes",
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
        symbols.lines().any(|line| {
            line.split_whitespace()
                .last()
                .is_some_and(|symbol| symbol.trim_start_matches('_') == "main")
        }),
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
fn links_windows_rt_program_with_std_using_toolchain_provider() {
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
        "--runtime-provider",
        "toolchain",
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
fn runs_hosted_program_with_indexed_command_line_arguments() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_hosted_args",
        r#"
use std.proc;

fn main(argc: i32, argv: **u8) i32 {
    let args = proc.args(argc, argv);
    if (args.len() != 3) {
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
    if (!second.eq("alpha")) {
        return 3;
    }
    let third = match (args.get(2)) {
        .{ Some: arg } => arg,
        .None => return 4,
    };
    if (!third.eq("beta gamma")) {
        return 4;
    }
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path)
        .arg("alpha")
        .arg("beta gamma")
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

    let start = time.now();
    time.sleep_millis(10);
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
    if (!plain.eq("plain")) {
        return 2;
    }
    let spaced = match (args.get(2)) {
        .{ Some: arg } => arg,
        .None => return 3,
    };
    if (!spaced.eq("two words")) {
        return 3;
    }
    let quoted = match (args.get(3)) {
        .{ Some: arg } => arg,
        .None => return 4,
    };
    if (!quoted.eq("quote\"value")) {
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
    if (!first.eq("\u{6D4B}\u{8BD5}")) {
        return 2;
    }
    let second = match (args.get(2)) {
        .{ Some: arg } => arg,
        .None => return 3,
    };
    if (!second.eq("\u{7A7A} \u{767D}")) {
        return 3;
    }
    let third = match (args.get(3)) {
        .{ Some: arg } => arg,
        .None => return 4,
    };
    if (!third.eq("emoji-\u{1F642}")) {
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
        &["--entry", "mainCRTStartup", "-l", "kernel32"],
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

    if (!found.&.eq("alpha-beta")) {
        return 2;
    }

    if (env.get(gpa, "KERN_STD_ENV_MISSING").is_some()) {
        return 3;
    }

    let mut fallback = match (env.get_or_clone(gpa, "KERN_STD_ENV_MISSING", "fallback")) {
        .{ Some: value } => value,
        .None => return 4,
    };
    defer fallback..&.deinit(gpa);
    if (!fallback.&.eq("fallback")) {
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
        "--library-bundle",
        "std",
        "--runtime-entry",
        "crt",
        "--runtime-provider",
        "toolchain",
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
