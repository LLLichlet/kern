use super::*;

#[test]
fn rejects_nested_enum_payload_gap_in_match_exhaustiveness() {
    let output = compile_source(
        r#"
type Inner = enum {
    X,
    Y,
};

type Outer = enum {
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
type Inner = enum {
    X,
    Y,
};

type Payload = struct {
    inner: Inner,
};

type Outer = enum {
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
type Inner = enum {
    X,
    Y,
};

type Outer = enum {
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
type Bit = enum {
    Zero,
    One,
};

type Leaf = enum {
    Empty,
    Full: Bit,
};

type Node = struct {
    left: Leaf,
    right: Leaf,
};

type Tree = enum {
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
type Mode = enum {
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
type Mode = enum {
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
type Mode = enum {
    Off,
    On,
};

type Alias = Mode;

type Box[T] = enum {
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
type Mode = enum {
    Off,
    On,
};

type Box[T] = enum {
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
        (u8.{0}) .. (u8.{2}) => 10,
        (u8.{2}) ..= (u8.{3}) => 20,
        _ => 30,
    };
}

fn main() i32 {
    return classify(Box[Mode].Empty)
        + classify(Box[Mode].{ Full: Mode.Off })
        + classify(Box[Mode].{ Full: Mode.On })
        + bucket(u8.{0})
        + bucket(u8.{1})
        + bucket(u8.{2})
        + bucket(u8.{3})
        + bucket(u8.{4})
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
type Inner = enum {
    X,
    Y,
};

type Outer = enum {
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
type Inner = enum {
    X,
    Y,
};

type Outer = enum {
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
type Option[T] = enum {
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
    if (classify(u8.{0}) + classify(u8.{200}) == 3) {
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
    return match (u8.{7}) {
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
    return match (u8.{7}) {
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
const MID = u128.{170141183460469231731687303715884105727};
const NEXT = u128.{170141183460469231731687303715884105728};
const MAX = u128.{340282366920938463463374607431768211455};

fn classify(value: u128) i32 {
    return match (value) {
        0..=MID => 1,
        NEXT..=MAX => 2,
    };
}

fn main() i32 {
    if (classify(u128.{0}) + classify(MAX) == 3) {
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
const MAX = u128.{340282366920938463463374607431768211455};

fn main() i32 {
    return match (u128.{7}) {
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
const MAX = u128.{340282366920938463463374607431768211455};

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
const MAX = u128.{340282366920938463463374607431768211455};

fn main() i32 {
    return match (u128.{7}) {
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
type Inner = enum {
    X,
    Y,
};

type Payload = struct {
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
type Pair = struct {
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
type Pair = struct {
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
type Inner = enum {
    X,
    Y,
};

type Payload = struct {
    inner: Inner,
};

type Outer = enum {
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
type Color = enum {
    R,
    B,
};

type Leaf = enum {
    E,
};

type Tree = enum {
    E,
    T: Node,
};

type Node = struct {
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
type Slot = enum {
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
type Pair = struct {
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
type Maybe = enum {
    None,
    Some: i32,
};

type Holder = struct {
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
