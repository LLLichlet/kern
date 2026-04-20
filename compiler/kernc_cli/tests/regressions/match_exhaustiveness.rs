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
