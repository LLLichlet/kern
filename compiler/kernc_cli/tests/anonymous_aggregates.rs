//! CLI integration tests for anonymous aggregate syntax and lowering.

use kernc_cli::test_support::{assert_success, build_and_run, compile_source_with_args};

fn compile_source(source: &str) -> std::process::Output {
    compile_source_with_args("kernc_test", source, &[])
}

#[test]
fn compiles_anonymous_aggregates_example() {
    let output = compile_source(
        r#"
extern struct CLayout {
    tag: u8,
    value: u64,
    flag: u16,
};

struct NativeLayout {
    tag: u8,
    value: u64,
    flag: u16,
};

struct Pair {
    x: i32,
    y: i32,
};

fn sum_pair(pair: struct { y: i32, x: i32 }) i32 {
    return pair.x + pair.y;
}

fn read_word(word: union { int: i32, bytes: [4]u8 }) i32 {
    return word.int;
}

fn classify(state: enum: u32 { Off = 0, On = 1, Error: i32 }) i32 {
    return match (state) {
        .Off => 0,
        .On => 1,
        .{ Error: code } => code,
    };
}

fn main() i32 {
    if (@sizeOf[CLayout]() != 24) {
        return 1;
    }
    if (@sizeOf[NativeLayout]() != 16) {
        return 2;
    }
    if (@sizeOf[struct { tag: u8, value: u64, flag: u16 }]() != 16) {
        return 3;
    }

    let pair = Pair.{ x: 2, y: 3 };
    let word = union { bytes: [4]u8, int: i32 }.{ int: 7 };
    return sum_pair(pair) + read_word(word) + classify(.{ Error: 11 });
}
"#,
    );

    assert_success(&output, "kernc");
}

#[test]
fn compiles_anonymous_enum_match_with_explicit_discriminants() {
    let output = compile_source(
        r#"
enum Switch: u16 {
    Off = 4,
    On = 7,
    Error: i32,
};

fn decode_named(v: Switch) i32 {
    match (v) {
        .Off => 40,
        .On => 70,
        .{ Error: payload } => payload,
    }
}

fn decode_anon(v: enum: u16 { Off = 4, On = 7, Error: i32 }) i32 {
    match (v) {
        .Off => 1,
        .On => 2,
        .{ Error: payload } => payload,
    }
}

fn main() i32 {
    let named = Switch.{ Error: 9 };
    let anon = enum: u16 { Off = 4, On = 7, Error: i32 }.{ Error: 11 };
    return decode_named(named) + decode_anon(anon);
}
"#,
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn contextual_struct_field_puns_work_inside_enum_payload_literals() {
    let output = build_and_run(
        "contextual_struct_field_puns",
        r#"
struct Pair {
    x: i32,
    y: i32,
};

struct Box[T] {
    value: T,
};

fn make_pair(x: i32, y: i32) ?Pair {
    return .{ Some: .{ x, y } };
}

fn make_box[T](value: T) ?Box[T] {
    return .{ Some: .{ value } };
}

fn main() i32 {
    let .{ Some: pair } = make_pair(2, 3) else return 1;
    let .{ Some: boxed } = make_box[i32](4) else return 2;
    return pair.x + pair.y + boxed.value - 9;
}
"#,
        &["--runtime-libc", "yes"],
    );

    assert_success(&output, "kernc");
}

#[test]
fn compiles_extern_enum_declarations_as_backing_integer_abi_values() {
    let output = compile_source(
        r#"
extern enum Mode: u32 {
    Read = 1,
    Write,
};

fn take_raw(value: u32) u32 {
    return value;
}

fn main() i32 {
    let raw: u32 = Mode.Write;
    return (take_raw(Mode.Read) + raw) as i32 - 3;
}
"#,
    );

    assert_success(&output, "kernc");
}

#[test]
fn rejects_extern_enum_arithmetic_without_backing_integer_boundary() {
    let output = compile_source(
        r#"
extern enum Mode: u32 {
    Read = 1,
    Write,
};

fn main() i32 {
    let raw = Mode.Read + 1u32;
    return raw as i32;
}
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted extern enum arithmetic:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("operator `+` is not available"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_extern_enum_without_backing_type() {
    let output = compile_source(
        r#"
extern enum Bad {
    A,
};
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("extern enum declarations must specify an integer backing type"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_extern_enum_payload_variants() {
    let output = compile_source(
        r#"
extern enum Bad: u32 {
    A: i32,
};
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("extern enum variants cannot carry payloads"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn accepts_type_alias_to_anonymous_extern_struct() {
    let output = compile_source(
        r#"
type Header = extern struct {
    tag: u32,
};

fn main() i32 {
    return @sizeOf[Header]() as i32;
}
"#,
    );

    assert_success(&output, "kernc");
}

#[test]
fn rejects_top_level_extern_import_syntax() {
    let output = compile_source(
        r#"
extern fn puts(msg: &u8) i32;
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("external imports must be declared inside `extern { ... }` blocks"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_extern_union_bnc_without_extern_on_the_anonymous_side() {
    let output = compile_source(
        r#"
extern union CWord {
    bytes: [4]u8,
    int: i32,
};

fn read_plain(word: union { bytes: [4]u8, int: i32 }) i32 {
    word.int
}

fn main() i32 {
    let word = CWord.{ int: 9 };
    return read_plain(word);
}
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("mismatched types"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_pub_fields_on_anonymous_structs() {
    let output = compile_source(
        r#"
fn read_pair(pair: struct { pub left: i32, right: i32 }) i32 {
    return pair.left;
}

fn main() i32 {
    let pair = struct { pub left: i32, right: i32 }.{ left: 3, right: 4 };
    return read_pair(pair);
}
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("anonymous struct fields cannot be declared pub"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn runs_union_field_reinterpretation_and_nested_lvalue_updates() {
    let output = build_and_run(
        "kernc_union_field_lvalue",
        r#"
union FloatBits {
    f: f32,
    i: u32,
    bytes: [4]u8,
};

fn main() i32 {
    let mut data = FloatBits.{ f: 3.14159 };
    let raw_bits = data.i;
    data.i = data.i ^ 0x80000000;
    let negative_pi = data.f;
    data.bytes.[0] = 0;

    if (raw_bits == 0) {
        return 1;
    }
    if (!(negative_pi < 0.0)) {
        return 2;
    }

    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    assert!(
        output.status.success(),
        "hosted union test failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn accepts_const_generic_named_struct_pointer_decay_to_anonymous_struct() {
    let output = compile_source(
        r#"
struct Buf[N: usize] {
    data: [N]u8,
};

fn first(ptr: &struct { data: [4]u8 }) i32 {
    return ptr.data.[0] as i32;
}

fn main() i32 {
    let buf = Buf[4].{ data: [4]u8.{ 7, 2, 3, 4 } };
    return first(buf.&) - 7;
}
"#,
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_anonymous_aggregate_function_signatures_without_literal_access() {
    let output = compile_source(
        r#"
#[export_name("accept_pair")]
fn accept_pair(pair: struct { x: i32, y: i32 }) i32 {
    let _ = pair;
    return 0;
}

#[export_name("accept_word")]
fn accept_word(word: union { int: i32, bytes: [4]u8 }) i32 {
    let _ = word;
    return 0;
}

fn main() i32 {
    return 0;
}
"#,
    );

    assert_success(&output, "kernc anonymous aggregate signature regression");
}

#[test]
fn rejects_const_generic_named_struct_pointer_decay_to_mismatched_anonymous_struct() {
    let output = compile_source(
        r#"
struct Buf[N: usize] {
    data: [N]u8,
};

fn first(ptr: &struct { data: [4]u8 }) i32 {
    return ptr.data.[0] as i32;
}

fn main() i32 {
    let buf = Buf[3].{ data: [3]u8.{ 7, 2, 3 } };
    return first(buf.&);
}
"#,
    );

    assert!(
        !output.status.success(),
        "expected compilation failure, but kernc succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("expected `&struct { data: [4]u8 }`"),
        "unexpected stderr:\n{}",
        stderr
    );
}
