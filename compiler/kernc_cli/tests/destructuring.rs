mod support;

use std::process::Output;

use support::{build_and_run, compile_source_with_args};

fn compile_source(source: &str) -> Output {
    compile_source_with_args("kernc_destructuring_test", source, &[])
}

fn build_and_run_source(source: &str) -> Output {
    build_and_run(
        "kernc_destructuring_run",
        source,
        &["--runtime-libc", "yes"],
    )
}

#[test]
fn compiles_nested_let_destructuring_with_structs_and_enums() {
    let output = build_and_run_source(
        r#"
type Option[T] = enum {
    None,
    Some: T,
};

type Point = struct {
    x: i32,
    y: i32,
};

type Node = struct {
    pos: Point,
    maybe: Option[i32],
};

fn main() i32 {
    let node = Node.{
        pos: .{ x: 5, y: 7 },
        maybe: .{ Some: 11 },
    };

    let .{ pos: .{ x, y }, maybe: .{ Some: value } } = node else return 1;
    return x + y + value;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(23),
        "program exited unexpectedly:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_match_destructuring_with_field_puns() {
    let output = build_and_run_source(
        r#"
type Pair = struct {
    left: i32,
    right: i32,
};

fn sum(pair: Pair) i32 {
    return match (pair) {
        .{ left, right } => left + right,
    };
}

fn main() i32 {
    return sum(Pair.{ left: 4, right: 9 });
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(13),
        "program exited unexpectedly:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_mut_binding_inside_struct_pattern() {
    let output = build_and_run_source(
        r#"
type Boxed = struct {
    value: i32,
};

fn main() i32 {
    let .{ value: mut inner } = Boxed.{ value: 4 };
    inner = 10;
    return inner;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(10),
        "program exited unexpectedly:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_unknown_struct_pattern_field() {
    let output = compile_source(
        r#"
type Pair = struct {
    left: i32,
    right: i32,
};

fn main() i32 {
    let .{ left, missing } = Pair.{ left: 1, right: 2 };
    return left;
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
        String::from_utf8_lossy(&output.stderr).contains("field `missing` does not exist"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_braced_payloadless_enum_pattern() {
    let output = compile_source(
        r#"
type Option[T] = enum {
    None,
    Some: T,
};

fn main() i32 {
    return match (Option[i32].{ None }) {
        .{ None } => 0,
        .{ Some: value } => value,
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
        String::from_utf8_lossy(&output.stderr).contains("payload-less form"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

