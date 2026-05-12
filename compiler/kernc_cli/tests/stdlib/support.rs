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
fn base_abi_cstr_owned_tracks_pointer_and_length() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_base_abi_cstr_owned",
        r#"
use base.abi;
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = gpa().on(page)..&;

    let .{ Ok: owned } = abi.cstr.owned(gpa, "kern") else {
        return 9;
    };
    let mut text = owned;
    defer text..&.deinit(gpa);

    if (text.&.len() != 4) {
        return 1;
    }
    if (text.&.ptr().cstr_len() != 4) {
        return 2;
    }
    if ((text.&.ptr() + 4).* != 0) {
        return 3;
    }
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected base abi cstr owned helper to succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
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
fn runs_test_assertion_helpers() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_test_helpers",
        r#"
use base.test;
use std.io;

fn main() i32 {
    let t = test.report(io.stderr())..&;

    true.should().sum(@loc(), t);
    (2 + 2 == 4).should().sum(@loc(), t);
    "42".parse[i32]().should_ok().eq(42).sum(@loc(), t);
    "ff".parse_radix[u8](16).should_ok().eq(u8.{255}).sum(@loc(), t);
    i32!i32.{ Ok: 42 }.should_ok().eq(42).sum(@loc(), t);
    i32!i32.{ Err: -7 }.should_err().eq(-7).sum(@loc(), t);
    (?usize.{ Some: 7 }).should_some().eq(7).sum(@loc(), t);
    (?usize.None).should_none().sum(@loc(), t);
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected base.test helpers to succeed:\nstdout:\n{}\nstderr:\n{}",
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
use base.test;
use std.io;

fn main() i32 {
    let t = test.report(io.stderr())..&;

    (usize.{4} == usize.{5}).should().sum(@loc(), t);
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
use base.test;
use std.io;

fn main() i32 {
    let t = test.report(io.stderr())..&;

    (?usize.{ Some: 4 }).should_some().eq(usize.{5}).sum(@loc(), t);
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
    assert!(
        stderr.contains("test failed: expected values to be equal"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("expected: 5"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("actual: 4"),
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
use base.test;
use std.io;

enum Mode {
    Fast,
    Slow,
};

fn main() i32 {
    let t = test.report(io.stderr())..&;

    (Mode.Fast == Mode.Fast).should().sum(@loc(), t);
    (Mode.Fast != Mode.Slow).should().sum(@loc(), t);
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected base.test to support payloadless enums:\nstdout:\n{}\nstderr:\n{}",
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
use base.test;
use std.io;

fn parse(flag: bool) usize!i32 {
    if (flag) {
        return .{ Ok: 7 };
    }
    return .{ Err: -1 };
}

fn main() i32 {
    let t = test.report(io.stderr())..&;

    let some = ?usize.{ Some: 9 }.should_some().sum(@loc(), t);
    some.should().eq(usize.{9}).sum(@loc(), t);
    (?usize.None).should_none().sum(@loc(), t);
    (?usize.{ Some: 11 }).should_some().eq(usize.{11}).sum(@loc(), t);
    (?usize.None).should_none().sum(@loc(), t);

    let ok = parse(true).should_ok().sum(@loc(), t);
    ok.should().eq(usize.{7}).sum(@loc(), t);
    parse(true).should_ok().eq(usize.{7}).sum(@loc(), t);

    let err = parse(false).should_err().sum(@loc(), t);
    err.should().eq(i32.{-1}).sum(@loc(), t);
    parse(false).should_err().eq(i32.{-1}).sum(@loc(), t);
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected base.test option/result helpers to succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn test_message_assertion_failure_uses_custom_format() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_test_msg_fail",
        r#"
use base.test;
use std.io;

fn main() i32 {
    let t = test.report(io.stderr())..&;

    let _ = usize!i32.{ Err: 7 }.should_ok().sum(@loc(), t);
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
use base.test;
use std.io;

fn main() i32 {
    let t = test.report(io.stderr())..&;

    false.should().sum(@loc(), t);
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        !run_output.status.success(),
        "expected base.test context located eq failure to abort with a diagnostic:\nstdout:\n{}\nstderr:\n{}",
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
use base.test;
use std.io;

fn main() i32 {
    let t = test.report(io.stderr())..&;

    let _ = (?usize.None).should_some().sum(@loc(), t);
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        !run_output.status.success(),
        "expected base.test context expect_some failure to abort with a diagnostic:\nstdout:\n{}\nstderr:\n{}",
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
use base.coll.{List, list, String, string};
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = gpa().on(page)..&;

    let mut text = string();
    defer text..&.deinit(gpa);
    if (text..&.try_push_str(gpa, "kern").is_err()) {
        return 1;
    }

    let mut items = list[usize]();
    defer items..&.deinit(gpa);
    if (items..&.try_push(gpa, usize.{1}).is_err()) {
        return 2;
    }
    if (items..&.try_push(gpa, usize.{2}).is_err()) {
        return 3;
    }

    "{} {}".fmt(.{ text, items, }).println();
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
fn hosted_std_io_prints_generic_slices() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_printable_slices",
        r#"
use std.io;

fn main() i32 {
    let values = [4]usize.{ 9, 1, 7, 3 };
    let ordered = values.&[0 .. 4];

    let mut scratch = [3]usize.{ 5, 4, 6 };
    let window = scratch.&[0 .. 3];

    "{} {}".fmt(.{ ordered, window, }).println();
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected std io printable slices program to succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(
        stdout.contains("[9, 1, 7, 3] [5, 4, 6]"),
        "unexpected stdout:\n{}",
        stdout
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn hosted_std_io_prints_custom_value_printable() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_printable_value_impl",
        r#"
use std.io;
use base.io.{FormatSpec, Formatable, Write};

struct Pair {
    left: usize,
    right: usize,
};

impl Pair : Formatable {
    pub fn write_fmt(spec: FormatSpec, writer: &mut Write) void {
        _ = spec;
        let _ = writer.write("(");
        self.left.&.write_to(writer);
        let _ = writer.write(", ");
        self.right.&.write_to(writer);
        let _ = writer.write(")");
    }
}

fn main() i32 {
    let pair = Pair.{ left: 2, right: 5 };
    "{}".fmt(.{ pair, }).println();
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected std io custom value printable program to succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert!(stdout.contains("(2, 5)"), "unexpected stdout:\n{}", stdout);

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
fn hosted_std_io_formats_to_memory_writers() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_io_memory_writers",
        r#"
use base.io.Write;
use base.coll.{String, string};
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {
    let mut fixed_storage = [64]u8.{undef};
    let mut fixed = (fixed_storage..&[0 .. 64]).writer();
    let fixed_writer = &mut Write.{ fixed..& };
    "{}{} {{}} {}".fmt(.{ 12, "ab", false, }).write_to(fixed_writer);
    if (fixed..&.as_slice() != "12ab {} false") {
        return 1;
    }
    if (fixed..&.did_overflow()) {
        return 2;
    }

    let mut small_storage = [5]u8.{undef};
    let mut small = (small_storage..&[0 .. 5]).writer();
    let small_writer = &mut Write.{ small..& };
    if (small_writer.write_all("abcdef")) {
        return 3;
    }
    if (small..&.as_slice() != "abcde") {
        return 4;
    }
    if (!small..&.did_overflow()) {
        return 5;
    }

    let page = Page.{}..&;
    let gpa = gpa().on(page)..&;
    defer gpa.deinit();

    let out = string()..&;
    defer out.deinit(gpa);
    let mut string_sink = out.writer(gpa);
    let sink_writer = &mut Write.{ string_sink..& };
    "[{}{}] {{x}}".fmt(.{ "id-", usize.{7}, }).write_to(sink_writer);
    if (string_sink..&.did_fail()) {
        return 6;
    }
    if (out.as_str() != "[id-7] {x}") {
        return 7;
    }

    let mut fmt_storage = [256]u8.{undef};
    let mut fmt = (fmt_storage..&[0 .. 256]).writer();
    let fmt_writer = &mut Write.{ fmt..& };
    "p={02} r={>4} l={<4} c={^5} z={0>3} f={_>4} cut={.3} mix={>6.2} left={<6.2} zero={0>5.2} hex={x} HEX={X} bin={b} oct={o} alt={#x} wide={#08x} neg={x} bad={q} old={:02}"
        .fmt(.{
            7,
            "go",
            "go",
            "go",
            7,
            7,
            "abcdef",
            "abcdef",
            "abcdef",
            12345,
            u32.{48879},
            u32.{48879},
            u8.{10},
            u8.{10},
            u16.{48879},
            u16.{255},
            i8.{-1},
        })
        .write_to(fmt_writer);
    if (fmt..&.as_slice() != "p=07 r=  go l=go   c= go   z=007 f=___7 cut=abc mix=    ab left=ab     zero=00012 hex=beef HEX=BEEF bin=1010 oct=12 alt=0xbeef wide=00000xff neg=ff bad={q} old={:02}") {
        return 8;
    }

    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected std io memory writer program to succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn hosted_std_io_reads_from_memory_readers() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_io_memory_readers",
        r#"
use base.io.Read;
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {
    let mut reader = "abcdef".reader();
    let reader_obj = &mut Read.{ reader..& };

    let mut head = [2]u8.{undef};
    if (!reader_obj.read_exact(head..&[0 .. 2])) {
        return 1;
    }
    if (head.&[0 .. 2] != "ab") {
        return 2;
    }
    if (reader..&.remaining() != 4 or reader..&.remaining_slice() != "cdef") {
        return 3;
    }
    if (reader_obj.skip(2) != 2) {
        return 4;
    }

    let mut tail = [3]u8.{undef};
    if (reader_obj.read_exact(tail..&[0 .. 3])) {
        return 5;
    }
    if (tail.[0] != b'e' or tail.[1] != b'f') {
        return 6;
    }
    if (!reader..&.is_empty()) {
        return 7;
    }

    let page = Page.{}..&;
    let gpa = gpa().on(page)..&;
    defer gpa.deinit();

    let mut reader2 = "kern-io".reader();
    let reader2_obj = &mut Read.{ reader2..& };
    let mut bytes = match (reader2_obj.read_to_end(gpa)) {
        .{ Ok: list } => list,
        .{ Err: _ } => return 8,
    };
    defer bytes..&.deinit(gpa);
    if (bytes..&.as_slice() != "kern-io") {
        return 9;
    }
    if (reader2_obj.skip(1) != 0) {
        return 10;
    }

    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected std io memory reader program to succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn hosted_std_io_copies_between_generic_adapters() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_io_copy_adapters",
        r#"
use base.io.{
    Read,
    Write,
    discard,
};
use base.coll.{String, string};
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {
    let mut source = "abcdef".reader();
    let source_reader = &mut Read.{ source..& };

    let mut storage = [8]u8.{undef};
    let mut fixed = (storage..&[0 .. 8]).writer();
    let fixed_writer = &mut Write.{ fixed..& };
    let mut counted = fixed_writer.counting();
    let counted_writer = &mut Write.{ counted..& };

    let copied = source_reader.copy_n_to(counted_writer, 4);
    if (copied != 4) {
        return 1;
    }
    if (counted..&.bytes_written() != 4) {
        return 2;
    }
    if (fixed..&.as_slice() != "abcd") {
        return 3;
    }
    if (source..&.remaining_slice() != "ef") {
        return 4;
    }

    let page = Page.{}..&;
    let gpa = gpa().on(page)..&;
    defer gpa.deinit();

    let mut text = string();
    defer text..&.deinit(gpa);
    let mut sink = text..&.writer(gpa);
    let sink_writer = &mut Write.{ sink..& };

    let mut source2 = "0123456789".reader();
    let source2_reader = &mut Read.{ source2..& };
    let mut limited = source2_reader.limit(6);
    let limited_reader = &mut Read.{ limited..& };
    let limited_copied = limited_reader.copy_to(sink_writer);
    if (limited_copied != 6) {
        return 5;
    }
    if (text..&.as_str() != "012345") {
        return 6;
    }
    if (source2..&.remaining_slice() != "6789") {
        return 7;
    }

    let mut source3 = "discard".reader();
    let source3_reader = &mut Read.{ source3..& };
    let mut null = discard();
    let null_sink = &mut Write.{ null..& };
    if (source3_reader.copy_to(null_sink) != 7) {
        return 8;
    }
    if (!source3..&.is_empty()) {
        return 9;
    }

    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected std io copy adapters program to succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn test_expect_err_failure_aborts_with_message() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_test_expect_err_fail",
        r#"
use base.test;
use std.io;

fn main() i32 {
    let t = test.report(io.stderr())..&;

    let _ = usize!i32.{ Ok: 3 }.should_err().sum(@loc(), t);
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        !run_output.status.success(),
        "expected base.test context expect_err failure to abort with a diagnostic:\nstdout:\n{}\nstderr:\n{}",
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
