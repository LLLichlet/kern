use super::*;

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
fn hosted_std_io_prints_base_string_and_list_values() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_printable_collections",
        r#"
use std.io;
use base.coll.{List, String};
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;

    let mut text = String.{};
    defer text..&.deinit(gpa);
    if (!text..&.push_str(gpa, "kern")) {
        return 1;
    }

    let mut items = List[usize].{};
    defer items..&.deinit(gpa);
    if (!items..&.push(gpa, usize.{1})) {
        return 2;
    }
    if (!items..&.push(gpa, usize.{2})) {
        return 3;
    }

    io.println("{} {}", .{ text, items, });
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected std io printable collections program to succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(stdout.contains("kern"), "unexpected stdout:\n{}", stdout);
    assert!(
        stdout.contains("<List len=2, cap=8, items: [1, 2]>"),
        "unexpected stdout:\n{}",
        stdout
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
