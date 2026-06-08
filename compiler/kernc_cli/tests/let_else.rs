//! CLI integration tests for `let ... else` pattern behavior.

use std::process::Output;

use kernc_cli::test_support::{build_and_run, compile_source_with_args};

fn compile_source(source: &str) -> Output {
    compile_source_with_args("kernc_let_else_test", source, &[])
}

fn build_and_run_source(source: &str) -> Output {
    build_and_run("kernc_let_else_run", source, &["--runtime-libc", "yes"])
}

#[test]
fn compiles_typed_variant_let_else_and_binds_payload() {
    let output = build_and_run_source(
        r#"
enum Option[T] {
    None,
    Some: T,
};

fn extract(value: Option[i32]) i32 {
    let Option[i32].{ Some: inner } = value else return 0;
    return inner;
}

fn main() i32 {
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
enum Option[T] {
    None,
    Some: T,
};

const fn unwrap_or(value: Option[i32], fallback: i32) i32 {
    let .{ Some: inner } = value else return fallback;
    return inner;
}

const PICKED = unwrap_or(Option[i32].{ Some: 9 }, 5);

fn main() i32 {
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
fn rejects_unbraced_let_variant_payload_pattern() {
    let output = compile_source(
        r#"
enum Option[T] {
    None,
    Some: T,
};

fn main() i32 {
    let .Some: value = Option[i32].{ Some: 3 } else return 0;
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
            .contains("enum payload patterns must use braced destructuring syntax"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_unbraced_match_variant_payload_pattern() {
    let output = compile_source(
        r#"
enum Option[T] {
    None,
    Some: T,
};

fn main() i32 {
    return match (Option[i32].{ Some: 3 }) {
        .Some: value => value,
        .None => 0,
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
        String::from_utf8_lossy(&output.stderr)
            .contains("enum payload patterns must use braced destructuring syntax"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_refutable_let_without_else() {
    let output = compile_source(
        r#"
enum Option[T] {
    None,
    Some: T,
};

fn main() i32 {
    let .{ Some: value } = Option[i32].{ Some: 3 };
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
fn main() i32 {
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
            .contains("irrefutable `let` patterns cannot use `else`"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_non_diverging_let_else_branch() {
    let output = compile_source(
        r#"
enum Option[T] {
    None,
    Some: T,
};

fn main() i32 {
    let .{ Some: value } = Option[i32].None else 0;
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
enum Option[T] {
    None,
    Some: T,
};

fn pick(value: Option[i32], fallback: bool) i32 {
    let .{ Some: inner } = value else if (fallback) { return 7; } else { return 3; };
    return inner;
}

fn main() i32 {
    return pick(Option[i32].None, true);
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
fn compiles_let_else_with_failure_arm_block() {
    let output = build_and_run_source(
        r#"
enum Result[T, E] {
    Ok: T,
    Err: E,
};

fn unwrap_or_error(value: Result[i32, i32]) i32 {
    let .{ Ok: inner } = value else {
        .{ Err: err } => return err,
    };
    return inner;
}

fn main() i32 {
    return unwrap_or_error(Result[i32, i32].{ Err: 27 });
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(27),
        "program exited unexpectedly:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_const_fn_using_failure_arm_block() {
    let output = compile_source(
        r#"
enum Option[T] {
    None,
    Some: T,
};

const fn pick(value: Option[i32]) i32 {
    let .{ Some: inner } = value else {
        .None => return 11,
    };
    return inner;
}

const PICKED = pick(Option[i32].None);

fn main() i32 {
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
fn rejects_non_exhaustive_let_else_arm_block() {
    let output = compile_source(
        r#"
enum Result[T, E] {
    Ok: T,
    Err: E,
    Pending,
};

fn main() i32 {
    let .{ Ok: value } = Result[i32, i32].Pending else {
        .{ Err: err } => return err,
    };
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
            .contains("`let ... else` arms do not cover all remaining failure cases"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn preserves_plain_else_expression_starting_with_identifier() {
    let output = build_and_run_source(
        r#"
enum Option[T] {
    None,
    Some: T,
};

fn fail() never {
    @trap();
}

fn pick(value: Option[i32]) i32 {
    let .{ Some: inner } = value else fail();
    return inner;
}

fn main() i32 {
    return pick(Option[i32].{ Some: 19 });
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(19),
        "program exited unexpectedly:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn preserves_outer_binding_after_nested_let_else_shadowing() {
    let output = build_and_run_source(
        r#"
enum Option[T] {
    None,
    Some: T,
};

fn main() i32 {
    let value = 5i32;

    {
        let .{ Some: value } = Option[i32].{ Some: 9 } else return 1;
        if (value != 9i32) {
            return 2;
        }
    }

    return value;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(5),
        "program exited unexpectedly:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn let_else_shadowing_does_not_capture_its_own_uninitialized_binding() {
    let output = build_and_run_source(
        r#"
enum Result[T, E] {
    Ok: T,
    Err: E,
};

fn validate(count: u8) Result[u8, i32] {
    if (count == 0u8 or count > 64u8) {
        return .{ Err: 99 };
    }
    return .{ Ok: count };
}

fn keep_valid(count: u8) i32 {
    let .{ Ok: count } = validate(count) else {
        .{ Err: err } => return err,
    };
    return count as i32;
}

fn main() i32 {
    return keep_valid(8u8);
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(8),
        "program exited unexpectedly:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_nested_let_else_inside_failure_branch_block() {
    let output = build_and_run_source(
        r#"
enum Option[T] {
    None,
    Some: T,
};

fn pick(value: Option[Option[i32]]) i32 {
    let .{ Some: inner } = value else {
        let .{ Some: fallback } = Option[i32].{ Some: 41 } else return 1;
        return fallback;
    };

    let .{ Some: number } = inner else {
        let .{ Some: fallback } = Option[i32].{ Some: 17 } else return 2;
        return fallback;
    };

    return number;
}

fn main() i32 {
    return pick(Option[Option[i32]].{ Some: Option[i32].None });
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(17),
        "program exited unexpectedly:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_nested_let_else_arm_blocks() {
    let output = build_and_run_source(
        r#"
enum Result[T, E] {
    Ok: T,
    Err: E,
};

fn pick(value: Result[Result[i32, i32], i32]) i32 {
    let .{ Ok: inner } = value else {
        .{ Err: outer_err } => return outer_err,
    };
    let .{ Ok: number } = inner else {
        .{ Err: inner_err } => {
            let .{ Ok: fallback } = Result[i32, i32].{ Ok: inner_err + 1 } else {
                .{ Err: fallback_err } => return fallback_err,
            };
            return fallback;
        },
    };
    return number;
}

fn main() i32 {
    return pick(Result[Result[i32, i32], i32].{ Ok: Result[i32, i32].{ Err: 8 } });
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(9),
        "program exited unexpectedly:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
