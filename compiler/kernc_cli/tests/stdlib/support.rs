use super::*;

#[test]
fn runs_msg_logging_helpers() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_msg_helpers",
        r#"
use std.msg;

fn main() i32 {
    "boot {}".fmt(.{1}).log();
    "trace {}".fmt(.{"ok"}).debug();
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected msg helpers to succeed:\nstdout:\n{}\nstderr:\n{}",
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
fn msg_panic_aborts_with_message() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_msg_panic_fail",
        r#"
use std.msg;

fn main() i32 {
    "boom {}".fmt(.{42}).panic();
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        !run_output.status.success(),
        "expected formatted panic to abort:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&run_output.stderr);
    assert!(
        stderr.contains("panic: boom 42"),
        "unexpected stderr:\n{}",
        stderr
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn test_eq_failure_aborts_with_message() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_test_eq_fail",
        r#"
use base.test.{report};
use std.io;

fn main() i32 {
    let t = report(io.stderr())..&;

    (4usize == 5usize).should().sum(@loc(), t);
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        !run_output.status.success(),
        "expected base.test context eq failure to abort with a diagnostic:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&run_output.stderr);
    assert!(
        stderr.contains("test failed: expected condition to be true"),
        "unexpected stderr:\n{}",
        stderr
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn test_eq_failure_reports_expected_and_actual_values_when_formattable() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_test_eq_fail_formattable",
        r#"
use base.test.{report};
use std.io;

fn main() i32 {
    let t = report(io.stderr())..&;

    (?usize.{ Some: 4 }).should_some().eq(5usize).sum(@loc(), t);
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        !run_output.status.success(),
        "expected base.test formatted equality failure to abort with a diagnostic:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&run_output.stderr);
    assert!(stderr.contains("test failed: expected values to be equal"));
    assert!(stderr.contains("expected: 5"));
    assert!(stderr.contains("actual: 4"));

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn test_message_assertion_failure_uses_custom_format() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_test_msg_fail",
        r#"
use base.test.{report};
use std.io;

fn main() i32 {
    let t = report(io.stderr())..&;

    let _ = usize!i32.{ Err: 7 }.should_ok().sum(@loc(), t);
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        !run_output.status.success(),
        "expected base.test context failure to abort with a diagnostic:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&run_output.stderr);
    assert!(
        stderr.contains("test failed: expected result to be Ok"),
        "unexpected stderr:\n{}",
        stderr
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn test_message_assertion_failure_can_report_source_location() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_test_loc_fail",
        r#"
use base.test.{report};
use std.io;

fn main() i32 {
    let t = report(io.stderr())..&;

    false.should().sum(@loc(), t);
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        !run_output.status.success(),
        "expected base.test located failure to abort with a diagnostic:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&run_output.stderr);
    assert!(
        stderr.contains(".rn:") && stderr.contains("test failed: expected condition to be true"),
        "unexpected stderr:\n{}",
        stderr
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn test_expect_some_failure_aborts_with_message() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_test_expect_some_fail",
        r#"
use base.test.{report};
use std.io;

fn main() i32 {
    let t = report(io.stderr())..&;

    let _ = (?usize.None).should_some().sum(@loc(), t);
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        !run_output.status.success(),
        "expected base.test expect_some failure to abort with a diagnostic:\nstdout:\n{}\nstderr:\n{}",
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
fn hosted_std_io_prints_to_stdout_and_stderr() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_io_print_streams",
        r#"
use std.io;

fn main() i32 {
    "out {}".fmt(.{ 1, }).print();
    " line {}".fmt(.{ 2, }).println();
    "err {}".fmt(.{ 3, }).eprint();
    " line {}".fmt(.{ 4, }).eprintln();
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected std io stream helpers to succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&run_output.stdout),
        "out 1 line 2\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&run_output.stderr),
        "err 3 line 4\n"
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn hosted_std_io_prints_byte_slices_directly() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_io_print_byte_slices",
        r#"
use std.io;
use std.io.Printable;

fn write_line[T](value: T) void
    where T: Printable,
{
    value.println();
}

fn write_err_line[T](value: T) void
    where T: Printable,
{
    value.eprintln();
}

fn main() i32 {
    let bytes = [5]u8.{ b'h', b'e', b'l', b'l', b'o' };
    bytes.print();
    " ".print();
    bytes.&[1 .. 4].println();
    bytes.eprint();
    " ".eprint();
    bytes.&[0 .. 2].eprintln();
    write_line(bytes.&[2 .. 5]);
    write_err_line(bytes.&[3 .. 5]);
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected std io byte slice helpers to succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    assert_eq!(
        String::from_utf8_lossy(&run_output.stdout),
        "hello ell\nllo\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&run_output.stderr),
        "hello he\nlo\n"
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn test_expect_err_failure_aborts_with_message() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_test_expect_err_fail",
        r#"
use base.test.{report};
use std.io;

fn main() i32 {
    let t = report(io.stderr())..&;

    let _ = usize!i32.{ Ok: 3 }.should_err().sum(@loc(), t);
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        !run_output.status.success(),
        "expected base.test expect_err failure to abort with a diagnostic:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&run_output.stderr);
    assert!(
        stderr.contains("test failed: expected result to be Err"),
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
use base.io.Formatable;
use std.io;

fn wrap(fmt: &[u8], args: &[&Formatable]) void {
    fmt.fmt(args).println();
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
fn wrapped_fmt_helpers_accept_call_results_inside_inline_argument_arrays() {
    let output = build_and_run(
        "kernc_std_fmt_wrapper_call_results",
        r#"
use base.io.Formatable;
use std.io;

fn wrap(fmt: &[u8], args: &[&Formatable]) void {
    fmt.fmt(args).println();
}

fn forty_two() i32 {
    return 42;
}

fn main() i32 {
    wrap("{}", .{ forty_two(), });
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    assert!(
        output.status.success(),
        "expected wrapped fmt helper call result program to succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("42"), "unexpected stdout:\n{}", stdout);
}

#[test]
fn wrapped_fmt_helpers_accept_cast_expressions_inside_inline_argument_arrays() {
    let output = build_and_run(
        "kernc_std_fmt_wrapper_cast_exprs",
        r#"
use base.io.Formatable;
use std.io;

fn wrap(fmt: &[u8], args: &[&Formatable]) void {
    fmt.fmt(args).println();
}

fn main() i32 {
    let value = 42 as u64;
    wrap("{}", .{ value as usize, });
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    assert!(
        output.status.success(),
        "expected wrapped fmt helper cast expression program to succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("42"), "unexpected stdout:\n{}", stdout);
}

#[test]
fn print_accepts_single_argument_list_without_trailing_comma() {
    let output = compile_source_with_args(
        "kernc_std_print_single_arg",
        r#"
use std.io;

fn main() i32 {
    "value={}".fmt(.{ 42 }).println();
    return 0;
}
"#,
        &["--library-bundle", "std"],
    );

    assert!(
        output.status.success(),
        "single print argument failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
