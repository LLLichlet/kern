use std::fs;

use kernc_cli::test_support::{
    assert_success, build_and_run, compile_source_with_args, repo_root, run_kernc, unique_temp_path,
};

fn compile_source(source: &str) -> std::process::Output {
    compile_source_with_args("kernc_test", source, &[])
}

#[test]
fn compiles_anonymous_aggregates_example() {
    let source = repo_root().join("examples/anonymous_aggregates.rn");
    let object = unique_temp_path("anonymous_aggregates", "o");

    let source_arg = source.to_string_lossy().into_owned();
    let object_arg = object.to_string_lossy().into_owned();
    let args = vec!["-c", source_arg.as_str(), "-o", object_arg.as_str()];
    let output = run_kernc(&args);

    assert_success(&output, "kernc");
    assert!(
        object.exists(),
        "expected object file at {}",
        object.display()
    );

    let _ = fs::remove_file(&object);
}

#[test]
fn compiles_anonymous_enum_match_with_explicit_discriminants() {
    let output = compile_source(
        r#"
type Switch = enum: u16 {
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
fn rejects_extern_enum_declarations() {
    let output = compile_source(
        r#"
extern type Bad = enum {
    A,
    B,
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
        stderr.contains("enum types do not support `extern`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_named_right_side_extern_struct_syntax() {
    let output = compile_source(
        r#"
type Header = extern struct {
    tag: u32,
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
        stderr.contains("named struct declarations must use `extern type Name = struct { ... }`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_top_level_extern_import_syntax() {
    let output = compile_source(
        r#"
extern fn puts(msg: *u8) i32;
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
extern type CWord = union {
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
type FloatBits = union {
    f: f32,
    i: u32,
    bytes: [4]mut u8,
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
