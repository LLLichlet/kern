//! CLI integration tests for optional/result propagation.

use std::process::Output;

use kernc_cli::test_support::{build_and_run, compile_source_with_args};

fn compile_source(source: &str) -> Output {
    compile_source_with_args("kernc_propagate_test", source, &[])
}

fn build_and_run_source(source: &str) -> Output {
    build_and_run("kernc_propagate_run", source, &["--runtime-libc", "yes"])
}

#[test]
fn runs_builtin_optional_propagation() {
    let output = build_and_run_source(
        r#"
fn bump(value: ?i32) ?i32 {
    let inner = value.?;
    return ?i32.{ Some: inner + 1 };
}

fn main() i32 {
    return match (bump(?i32.{ Some: 41 })) {
        .{ Some: value } => value,
        .None => 1,
    };
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(42),
        "program exited unexpectedly:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn runs_builtin_optional_propagation_failure_path() {
    let output = build_and_run_source(
        r#"
fn bump(value: ?i32) ?i32 {
    let inner = value.?;
    return ?i32.{ Some: inner + 1 };
}

fn main() i32 {
    return match (bump(?i32.None)) {
        .None => 0,
        .{ Some: _ } => 1,
    };
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "program exited unexpectedly:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_braced_payloadless_builtin_optional_constructor() {
    let output = compile_source(
        r#"
fn main() i32 {
    let value = ?i32.{ None };
    return match (value) {
        .None => 0,
        .{ Some: _ } => 1,
    };
}
"#,
    );

    assert!(
        !output.status.success(),
        "expected compilation failure, but kernc succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("variant `None` does not take a payload"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn runs_builtin_result_propagation_and_preserves_error_payload() {
    let output = build_and_run_source(
        r#"
fn bump(value: i32!i32) i32!i32 {
    let inner = value.?;
    return i32!i32.{ Ok: inner + 1 };
}

fn main() i32 {
    return match (bump(i32!i32.{ Err: 7 })) {
        .{ Err: err } => err,
        .{ Ok: _ } => 1,
    };
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(7),
        "program exited unexpectedly:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_legacy_result_propagation_operator() {
    let output = compile_source(
        r#"
fn bump(value: i32!i32) i32!i32 {
    let inner = value.!;
    return i32!i32.{ Ok: inner + 1 };
}
"#,
    );

    assert!(
        !output.status.success(),
        "expected compilation failure, but kernc succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_nested_builtin_optional_and_result_matching() {
    let output = compile_source(
        r#"
fn main() i32 {
    let value = ?i32!i32.{ Some: i32!i32.{ Ok: 9 } };
    return match (value) {
        .{ Some: .{ Ok: inner } } => inner,
        .{ Some: .{ Err: _ } } => 1,
        .None => 2,
    };
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
