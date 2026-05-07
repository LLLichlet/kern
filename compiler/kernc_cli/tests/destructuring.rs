use std::process::Output;

use kernc_cli::test_support::{build_and_run, compile_source_with_args};

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
enum Option[T] {
    None,
    Some: T,
};

struct Point {
    x: i32,
    y: i32,
};

struct Node {
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
struct Pair {
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
fn compiles_fully_typed_nested_enum_struct_patterns_with_field_puns() {
    let output = build_and_run_source(
        r#"
enum Bit {
    Zero,
    One,
};

enum Leaf {
    Empty,
    Full: Bit,
};

struct Node {
    left: Leaf,
    right: Leaf,
};

enum Tree {
    Nil,
    Branch: Node,
};

fn classify_pun(tree: Tree) i32 {
    return match (tree) {
        Tree.Nil => 0,
        Tree.{ Branch: Node.{ left: Leaf.Empty, right } } => match (right) {
            Leaf.Empty => 1,
            Leaf.{ Full: Bit.Zero } => 2,
            Leaf.{ Full: Bit.One } => 3,
        },
        Tree.{ Branch: Node.{ left: Leaf.{ Full: Bit.Zero }, right: right } } => match (right) {
            Leaf.Empty => 4,
            Leaf.{ Full: Bit.Zero } => 5,
            Leaf.{ Full: Bit.One } => 6,
        },
        Tree.{ Branch: Node.{ left: Leaf.{ Full: Bit.One }, right } } => match (right) {
            Leaf.Empty => 7,
            Leaf.{ Full: Bit.Zero } => 8,
            Leaf.{ Full: Bit.One } => 9,
        },
    };
}

fn main() i32 {
    let a = classify_pun(Tree.{ Branch: Node.{ left: Leaf.Empty, right: Leaf.{ Full: Bit.One } } });
    let b = classify_pun(Tree.{ Branch: Node.{ left: Leaf.{ Full: Bit.Zero }, right: Leaf.Empty } });
    let c = classify_pun(Tree.{ Branch: Node.{ left: Leaf.{ Full: Bit.One }, right: Leaf.{ Full: Bit.Zero } } });
    return a + b + c;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(15),
        "program exited unexpectedly:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_typed_struct_initialization_with_field_puns() {
    let output = build_and_run_source(
        r#"
struct Point {
    x: i32,
    y: i32,
};

fn main() i32 {
    let x = 4;
    let y = 9;
    let point = Point.{ x, y };
    return point.x + point.y;
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
struct Boxed {
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
struct Pair {
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
enum Option[T] {
    None,
    Some: T,
};

fn main() i32 {
    return match (Option[i32].None) {
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
