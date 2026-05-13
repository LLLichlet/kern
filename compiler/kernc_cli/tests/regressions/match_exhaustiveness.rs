use super::*;

#[test]
fn rejects_nested_enum_payload_gap_in_match_exhaustiveness() {
    let output = compile_source(
        r#"
enum Inner {
    X,
    Y,
};

enum Outer {
    A: Inner,
    B,
};

fn main() i32 {
    let value = Outer.{ A: Inner.Y };
    return match (value) {
        .{ A: .X } => 1,
        .B => 2,
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

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("match expression is not exhaustive"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains(".{ A: .Y }"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_nested_struct_payload_gap_in_match_exhaustiveness() {
    let output = compile_source(
        r#"
enum Inner {
    X,
    Y,
};

struct Payload {
    inner: Inner,
};

enum Outer {
    A: Payload,
    B,
};

fn main() i32 {
    let value = Outer.{ A: Payload.{ inner: Inner.Y } };
    return match (value) {
        .{ A: .{ inner: .X } } => 1,
        .B => 2,
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

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("match expression is not exhaustive"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains(".{ A: .{ inner: .Y } }"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn accepts_exhaustive_nested_enum_match() {
    let output = build_and_run_source(
        r#"
enum Inner {
    X,
    Y,
};

enum Outer {
    A: Inner,
    B,
};

fn classify(value: Outer) i32 {
    return match (value) {
        .{ A: .X } => 11,
        .{ A: .Y } => 22,
        .B => 33,
    };
}

fn main() i32 {
    if (classify(Outer.{ A: Inner.X }) + classify(Outer.{ A: Inner.Y }) + classify(Outer.B) == 66) {
        return 0;
    }

    return 1;
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn accepts_exhaustive_qualified_value_patterns_with_nested_enum_payloads() {
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

fn classify(tree: Tree) i32 {
    return match (tree) {
        Tree.Nil => 0,
        Tree.{ Branch: Node.{ left: Leaf.Empty, right: Leaf.Empty } } => 1,
        Tree.{ Branch: Node.{ left: Leaf.Empty, right: Leaf.{ Full: Bit.Zero } } } => 2,
        Tree.{ Branch: Node.{ left: Leaf.Empty, right: Leaf.{ Full: Bit.One } } } => 3,
        Tree.{ Branch: Node.{ left: Leaf.{ Full: Bit.Zero }, right: Leaf.Empty } } => 4,
        Tree.{ Branch: Node.{ left: Leaf.{ Full: Bit.Zero }, right: Leaf.{ Full: Bit.Zero } } } => 5,
        Tree.{ Branch: Node.{ left: Leaf.{ Full: Bit.Zero }, right: Leaf.{ Full: Bit.One } } } => 6,
        Tree.{ Branch: Node.{ left: Leaf.{ Full: Bit.One }, right: Leaf.Empty } } => 7,
        Tree.{ Branch: Node.{ left: Leaf.{ Full: Bit.One }, right: Leaf.{ Full: Bit.Zero } } } => 8,
        Tree.{ Branch: Node.{ left: Leaf.{ Full: Bit.One }, right: Leaf.{ Full: Bit.One } } } => 9,
    };
}

fn main() i32 {
    return classify(Tree.{ Branch: Node.{ left: Leaf.{ Full: Bit.One }, right: Leaf.{ Full: Bit.One } } }) - 9;
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn qualified_value_patterns_contribute_to_non_exhaustive_witnesses() {
    let output = compile_source(
        r#"
enum Mode {
    Cold,
    Warm,
    Hot,
};

fn classify(mode: Mode) i32 {
    return match (mode) {
        Mode.Cold => 1,
        Mode.Warm => 2,
    };
}

fn main() i32 {
    return classify(Mode.Hot);
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
        stderr.contains("match expression is not exhaustive"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(stderr.contains(".Hot"), "unexpected stderr:\n{}", stderr);
}

#[test]
fn warns_when_qualified_value_pattern_is_shadowed() {
    let output = compile_source(
        r#"
enum Mode {
    Off,
    On,
};

fn classify(mode: Mode) i32 {
    return match (mode) {
        Mode.Off => 1,
        Mode.Off => 2,
        Mode.On => 3,
    };
}

fn main() i32 {
    return classify(Mode.On) - 3;
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning"), "unexpected stderr:\n{}", stderr);
    assert!(
        stderr.contains("unreachable match pattern"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn accepts_exhaustive_qualified_value_patterns_through_aliases_and_generic_namespaces() {
    let output = build_and_run_source(
        r#"
enum Mode {
    Off,
    On,
};

type Alias = Mode;

enum Box[T] {
    Empty,
    Full: T,
};

fn classify_alias(mode: Alias) i32 {
    return match (mode) {
        Alias.Off => 1,
        Alias.On => 2,
    };
}

fn classify_box(value: Box[Mode]) i32 {
    return match (value) {
        Box[Mode].Empty => 3,
        Box[Mode].{ Full: Mode.Off } => 4,
        Box[Mode].{ Full: Mode.On } => 5,
    };
}

fn main() i32 {
    return classify_alias(Alias.On)
        + classify_box(Box[Mode].Empty)
        + classify_box(Box[Mode].{ Full: Mode.Off })
        + classify_box(Box[Mode].{ Full: Mode.On })
        - 14;
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn grouped_match_value_patterns_remain_structural() {
    let output = build_and_run_source(
        r#"
enum Mode {
    Off,
    On,
};

enum Box[T] {
    Empty,
    Full: T,
};

fn classify(value: Box[Mode]) i32 {
    return match (value) {
        ((Box[Mode].Empty)) => 1,
        ((Box[Mode].{ Full: (Mode.Off) })) => 2,
        ((Box[Mode])).{ Full: ((Mode.On)) } => 3,
    };
}

fn bucket(value: u8) i32 {
    return match (value) {
        (0u8) .. (2u8) => 10,
        (2u8) ..= (3u8) => 20,
        _ => 30,
    };
}

fn main() i32 {
    return classify(Box[Mode].Empty)
        + classify(Box[Mode].{ Full: Mode.Off })
        + classify(Box[Mode].{ Full: Mode.On })
        + bucket(0u8)
        + bucket(1u8)
        + bucket(2u8)
        + bucket(3u8)
        + bucket(4u8)
        - 96;
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_nested_enum_gap_in_let_else_arm_block() {
    let output = compile_source(
        r#"
enum Inner {
    X,
    Y,
};

enum Outer {
    A: Inner,
    B,
};

fn main() i32 {
    let .{ A: .X } = Outer.{ A: Inner.Y } else {
        .B => return 2,
    };
    return 0;
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
        stderr.contains("`let ... else` arms do not cover all remaining failure cases"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains(".{ A: .Y }"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn warns_when_nested_match_pattern_is_shadowed_by_broader_variant_pattern() {
    let output = compile_source(
        r#"
enum Inner {
    X,
    Y,
};

enum Outer {
    A: Inner,
    B,
};

fn main() i32 {
    let value = Outer.{ A: Inner.Y };
    return match (value) {
        .{ A: _ } => 1,
        .{ A: .X } => 2,
        .B => 3,
    };
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning"), "unexpected stderr:\n{}", stderr);
    assert!(
        stderr.contains("unreachable match pattern"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("previous patterns already cover every value matched by this pattern"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn warns_when_match_pattern_appears_after_catch_all() {
    let output = compile_source(
        r#"
enum Option[T] {
    None,
    Some: T,
};

fn main() i32 {
    return match (Option[i32].None) {
        _ => 0,
        .None => 1,
    };
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning"), "unexpected stderr:\n{}", stderr);
    assert!(
        stderr.contains("unreachable match pattern"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn accepts_exhaustive_bool_match_without_catch_all() {
    let output = build_and_run_source(
        r#"
fn classify(value: bool) i32 {
    return match (value) {
        true => 10,
        false => 20,
    };
}

fn main() i32 {
    if (classify(true) + classify(false) == 30) {
        return 0;
    }

    return 1;
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_non_exhaustive_bool_match_without_catch_all() {
    let output = compile_source(
        r#"
fn main() i32 {
    return match (true) {
        true => 1,
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

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("match expression is not exhaustive"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(stderr.contains("false"), "unexpected stderr:\n{}", stderr);
}

#[test]
fn accepts_exhaustive_integer_range_match_without_catch_all() {
    let output = build_and_run_source(
        r#"
fn classify(value: u8) i32 {
    return match (value) {
        0..=127 => 1,
        128..=255 => 2,
    };
}

fn main() i32 {
    if (classify(0u8) + classify(200u8) == 3) {
        return 0;
    }

    return 1;
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_non_exhaustive_integer_range_match_without_catch_all() {
    let output = compile_source(
        r#"
fn main() i32 {
    return match (7u8) {
        1..=255 => 1,
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

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("match expression is not exhaustive"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(stderr.contains("0"), "unexpected stderr:\n{}", stderr);
}

#[test]
fn warns_when_integer_subrange_is_fully_shadowed() {
    let output = compile_source(
        r#"
fn main() i32 {
    return match (7u8) {
        0..=10 => 1,
        3..=5 => 2,
        _ => 3,
    };
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unreachable match pattern"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn accepts_exhaustive_u128_range_match_without_catch_all() {
    let output = build_and_run_source(
        r#"
const MID = 170141183460469231731687303715884105727u128;
const NEXT = 170141183460469231731687303715884105728u128;
const MAX = 340282366920938463463374607431768211455u128;

fn classify(value: u128) i32 {
    return match (value) {
        0..=MID => 1,
        NEXT..=MAX => 2,
    };
}

fn main() i32 {
    if (classify(0u128) + classify(MAX) == 3) {
        return 0;
    }

    return 1;
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_non_exhaustive_u128_range_match_missing_zero() {
    let output = compile_source(
        r#"
const MAX = 340282366920938463463374607431768211455u128;

fn main() i32 {
    return match (7u128) {
        1..=MAX => 1,
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

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("match expression is not exhaustive"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(stderr.contains("`0`"), "unexpected stderr:\n{}", stderr);
}

#[test]
fn rejects_non_exhaustive_u128_range_match_missing_max() {
    let output = compile_source(
        r#"
const MAX = 340282366920938463463374607431768211455u128;

fn main() i32 {
    return match (MAX) {
        0..MAX => 1,
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

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("match expression is not exhaustive"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("340282366920938463463374607431768211455"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn warns_when_u128_subrange_is_fully_shadowed() {
    let output = compile_source(
        r#"
const MAX = 340282366920938463463374607431768211455u128;

fn main() i32 {
    return match (7u128) {
        0..=10 => 1,
        3..=5 => 2,
        11..=MAX => 3,
    };
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unreachable match pattern"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn accepts_exhaustive_struct_match_through_nested_enum_patterns() {
    let output = build_and_run_source(
        r#"
enum Inner {
    X,
    Y,
};

struct Payload {
    inner: Inner,
};

fn classify(value: Payload) i32 {
    return match (value) {
        .{ inner: .X } => 5,
        .{ inner: .Y } => 7,
    };
}

fn main() i32 {
    if (classify(Payload.{ inner: Inner.X }) + classify(Payload.{ inner: Inner.Y }) == 12) {
        return 0;
    }

    return 1;
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn accepts_exhaustive_struct_value_patterns_over_bool_fields() {
    let output = build_and_run_source(
        r#"
struct Pair {
    left: bool,
    right: bool,
};

fn classify(pair: Pair) i32 {
    return match (pair) {
        Pair.{ left: false, right: false } => 0,
        Pair.{ left: false, right: true } => 1,
        Pair.{ left: true, right: false } => 2,
        Pair.{ left: true, right: true } => 3,
    };
}

fn main() i32 {
    return classify(Pair.{ left: true, right: true }) - 3;
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_non_exhaustive_struct_value_pattern_bool_gap() {
    let output = compile_source(
        r#"
struct Pair {
    left: bool,
    right: bool,
};

fn classify(pair: Pair) i32 {
    return match (pair) {
        Pair.{ left: false, right: false } => 0,
        Pair.{ left: false, right: true } => 1,
        Pair.{ left: true, right: false } => 2,
    };
}

fn main() i32 {
    return classify(Pair.{ left: true, right: true });
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
        stderr.contains("match expression is not exhaustive"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("left: true") && stderr.contains("right: true"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn accepts_exhaustive_nested_typed_struct_value_patterns() {
    let output = build_and_run_source(
        r#"
enum Inner {
    X,
    Y,
};

struct Payload {
    inner: Inner,
};

enum Outer {
    A: Payload,
    B,
};

fn classify(value: Outer) i32 {
    return match (value) {
        Outer.{ A: Payload.{ inner: .X } } => 11,
        Outer.{ A: Payload.{ inner: .Y } } => 22,
        .B => 33,
    };
}

fn main() i32 {
    if (classify(Outer.{ A: Payload.{ inner: Inner.X } })
        + classify(Outer.{ A: Payload.{ inner: Inner.Y } })
        + classify(Outer.B) == 66)
    {
        return 0;
    }

    return 1;
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn lowers_enum_payload_value_pattern_structurally() {
    let output = build_and_run_source(
        r#"
enum Color {
    R,
    B,
};

enum Leaf {
    E,
};

enum Tree {
    E,
    T: Node,
};

struct Node {
    color: Color,
    left: Leaf,
    right: Leaf,
    weight: i32,
};

fn classify(tree: Tree) i32 {
    return match (tree) {
        .E => 0,
        Tree.{ T: Node.{ color: Color.R, left: Leaf.E, right: Leaf.E, weight: 9 } } => 1,
        _ => 2,
    };
}

fn main() i32 {
    let hit = classify(Tree.{ T: Node.{ color: Color.R, left: Leaf.E, right: Leaf.E, weight: 9 } });
    let miss_color = classify(Tree.{ T: Node.{ color: Color.B, left: Leaf.E, right: Leaf.E, weight: 9 } });
    let miss_int = classify(Tree.{ T: Node.{ color: Color.R, left: Leaf.E, right: Leaf.E, weight: 10 } });
    return classify(Tree.E) + hit + miss_color + miss_int - 5;
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn lowers_scalar_enum_payload_value_pattern_structurally() {
    let output = build_and_run_source(
        r#"
enum Slot {
    Empty,
    Count: i32,
};

fn classify(slot: Slot) i32 {
    return match (slot) {
        .Empty => 0,
        Slot.{ Count: 7 } => 1,
        _ => 2,
    };
}

fn main() i32 {
    return classify(Slot.Empty) + classify(Slot.{ Count: 7 }) + classify(Slot.{ Count: 8 }) - 3;
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn lowers_struct_value_pattern_scalar_fields_structurally() {
    let output = build_and_run_source(
        r#"
struct Pair {
    left: i32,
    right: i32,
};

fn classify(pair: Pair) i32 {
    return match (pair) {
        Pair.{ left: 1, right: 2 } => 5,
        _ => 9,
    };
}

fn main() i32 {
    return classify(Pair.{ left: 1, right: 2 })
        + classify(Pair.{ left: 1, right: 3 })
        - 14;
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn lowers_nested_enum_payload_value_pattern_structurally() {
    let output = build_and_run_source(
        r#"
enum Maybe {
    None,
    Some: i32,
};

struct Holder {
    item: Maybe,
    flag: bool,
};

fn classify(holder: Holder) i32 {
    return match (holder) {
        Holder.{ item: Maybe.{ Some: 7 }, flag: true } => 3,
        Holder.{ item: .None, flag: false } => 4,
        _ => 9,
    };
}

fn main() i32 {
    return classify(Holder.{ item: Maybe.{ Some: 7 }, flag: true })
        + classify(Holder.{ item: Maybe.{ Some: 8 }, flag: true })
        + classify(Holder.{ item: Maybe.None, flag: false })
        - 16;
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn match_arm_alternative_patterns_share_one_binding_environment() {
    let output = build_and_run_source(
        r#"
enum Token {
    Int: i32,
    Float: i32,
    Text,
};

fn value(token: Token) i32 {
    return match (token) {
        .{ Int: n }, .{ Float: n } => n,
        .Text => 0,
    };
}

fn main() i32 {
    return value(Token.{ Int: 4 }) + value(Token.{ Float: 5 }) - 9;
}
"#,
    );

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_match_arm_alternative_patterns_with_different_bindings() {
    let output = compile_source(
        r#"
enum Token {
    Int: i32,
    Float: i32,
    Text,
};

fn value(token: Token) i32 {
    return match (token) {
        .{ Int: n }, .{ Float: other } => n,
        .Text => 0,
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

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("match arm patterns must bind the same names"),
        "unexpected stderr:\n{}",
        stderr
    );
}
