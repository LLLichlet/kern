mod support;

use std::process::Output;

use support::{build_and_run, compile_source_with_args};

fn compile_source(source: &str) -> Output {
    compile_source_with_args("kernc_let_else_test", source, &[])
}

fn build_and_run_source(source: &str) -> Output {
    build_and_run("kernc_let_else_run", source, &["--link-profile", "hosted"])
}

#[test]
fn compiles_typed_variant_let_else_and_binds_payload() {
    let output = build_and_run_source(
        r#"
type Option[T] = enum {
    None,
    Some: T,
};

fn extract(value: Option[i32]) i32 {
    let Option[i32].Some: inner = value else return 0;
    return inner;
}

extern fn main(args: [][]u8) i32 {
    return extract(Option[i32].{ Some: 42 });
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
fn compiles_const_fn_using_let_else() {
    let output = compile_source(
        r#"
type Option[T] = enum {
    None,
    Some: T,
};

const fn unwrap_or(value: Option[i32], fallback: i32) i32 {
    let .Some: inner = value else return fallback;
    return inner;
}

const PICKED = unwrap_or(Option[i32].{ Some: 9 }, 5);

extern fn main(args: [][]u8) i32 {
    return PICKED;
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
fn rejects_refutable_let_without_else() {
    let output = compile_source(
        r#"
type Option[T] = enum {
    None,
    Some: T,
};

extern fn main(args: [][]u8) i32 {
    let .Some: value = Option[i32].{ Some: 3 };
    return value;
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
        String::from_utf8_lossy(&output.stderr)
            .contains("refutable `let` patterns require an `else` branch"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_irrefutable_let_else() {
    let output = compile_source(
        r#"
extern fn main(args: [][]u8) i32 {
    let value = 3 else return 0;
    return value;
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
        String::from_utf8_lossy(&output.stderr)
            .contains("irrefutable `let` bindings cannot use `else`"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_non_diverging_let_else_branch() {
    let output = compile_source(
        r#"
type Option[T] = enum {
    None,
    Some: T,
};

extern fn main(args: [][]u8) i32 {
    let .Some: value = Option[i32].{ None } else 0;
    return value;
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
        String::from_utf8_lossy(&output.stderr)
            .contains("`let ... else` failure branches must diverge"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_let_else_with_if_expression_failure_branch() {
    let output = build_and_run_source(
        r#"
type Option[T] = enum {
    None,
    Some: T,
};

fn pick(value: Option[i32], fallback: bool) i32 {
    let .Some: inner = value else if (fallback) { return 7; } else { return 3; };
    return inner;
}

extern fn main(args: [][]u8) i32 {
    return pick(Option[i32].{ None }, true);
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
