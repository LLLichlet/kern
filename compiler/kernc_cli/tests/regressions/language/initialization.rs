use super::*;
#[test]
fn rejects_direct_recursive_struct_layout_cycle() {
    let output = compile_source(
        r#"
type Bad = struct {
    inner: Bad,
};

fn main() i32 {
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
        stderr.contains("recursively contains itself by value"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("recursive layout chain: Bad -> Bad"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_direct_recursive_enum_payload_layout_cycle() {
    let output = compile_source(
        r#"
type Bad = enum {
    Loop: Bad,
};

fn main() i32 {
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
        stderr.contains("recursively contains itself by value"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("recursive layout chain: Bad -> Bad"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_indirect_recursive_struct_layout_cycle_with_chain() {
    let output = compile_source(
        r#"
type A = struct {
    b: B,
};

type B = struct {
    a: A,
};

fn main() i32 {
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
        stderr.contains("recursively contains itself by value"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("recursive layout chain: A -> B -> A"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_constant_overshift_without_panicking() {
    let output = compile_source(
        r#"
const BAD = 1 << 999;

fn main() i32 {
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
        stderr.contains("shift amount in constant expression is too large"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("panicked at"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_constant_division_overflow_without_panicking() {
    let output = compile_source(
        r#"
fn main() i32 {
    let _ = [((-170141183460469231731687303715884105728) / (-1))]u8.{ undef };
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
        stderr.contains("division overflow in constant expression"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert_eq!(
        stderr
            .matches("division overflow in constant expression")
            .count(),
        1,
        "unexpected duplicated stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("panicked at"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_array_length_constants_that_exceed_usize_range() {
    let output = compile_source(
        r#"
fn main() i32 {
    let _ = [18446744073709551616]u8.{ undef };
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
        stderr.contains("integer literal 18446744073709551616 is out of bounds for type `usize`")
            || stderr.contains("constant expression is too large for this usize-like context"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_unresolved_optional_type_in_pointer_static_initializer_without_panicking() {
    let output = compile_source(
        r#"
type FramebufferRequest = struct {
    response: *u8,
};

static REQUEST = FramebufferRequest.{ response: ?T };

fn main() i32 {
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
        stderr.contains("Cannot find type `T` in this scope"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("optional types cannot be evaluated as value expressions"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert_eq!(
        stderr
            .matches("optional types cannot be evaluated as value expressions")
            .count(),
        1,
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("optional types are ordinary enum families, not null-pointer syntax"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("panicked at")
            && !stderr.contains("Kern Compiler Internal Error")
            && !stderr.contains("expected a valid constant expression"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_resolved_optional_type_in_pointer_static_initializer_without_panicking() {
    let output = compile_source(
        r#"
type FramebufferRequest = struct {
    response: *u8,
};

static REQUEST = FramebufferRequest.{ response: ?u8 };

fn main() i32 {
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
        stderr.contains("optional types cannot be evaluated as value expressions"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert_eq!(
        stderr
            .matches("optional types cannot be evaluated as value expressions")
            .count(),
        1,
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("if you meant the empty optional constructor, write `?T.None`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("panicked at")
            && !stderr.contains("Kern Compiler Internal Error")
            && !stderr.contains("expected a valid constant expression")
            && !stderr.contains("mismatched types"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn accepts_optional_none_constructor_in_static_initializer() {
    let output = build_and_run_source(
        r#"
type FramebufferRequest = struct {
    response: ?*u8,
};

static REQUEST = FramebufferRequest.{ response: (?*u8).None };

fn main() i32 {
    return match (REQUEST.response) {
        .None => 0,
        .{ Some: _ } => 1,
    };
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "optional none static initializer regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn accepts_optional_alias_none_constructor_in_static_initializer() {
    let output = build_and_run_source(
        r#"
type MaybePtr = ?*u8;

type FramebufferRequest = struct {
    response: MaybePtr,
};

static REQUEST = FramebufferRequest.{ response: MaybePtr.None };

fn main() i32 {
    return match (REQUEST.response) {
        .None => 0,
        .{ Some: _ } => 1,
    };
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "optional alias none static initializer regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn accepts_integer_to_pointer_casts_in_static_initializer() {
    let output = build_and_run_source(
        r#"
type FramebufferResponse = struct {
    count: u64,
};

type FramebufferRequest = struct {
    response: *FramebufferResponse,
    mmio: *u8,
};

static REQUEST = FramebufferRequest.{
    response: 0 as *FramebufferResponse,
    mmio: 0x1000 as *u8,
};

fn main() i32 {
    if (REQUEST.response != (0 as *FramebufferResponse)) {
        return 1;
    }
    if (REQUEST.mmio != (0x1000 as *u8)) {
        return 2;
    }
    return 0;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "integer-to-pointer cast static initializer regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_integer_to_trait_object_pointer_static_initializer_without_panicking() {
    let output = compile_source(
        r#"
type Writer = trait {
    write: fn([]u8) usize,
};

pub static mut WRITER = 0 as *mut Writer;

fn main() i32 {
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
        stderr.contains("cannot cast an integer to a fat pointer using `as`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("trait objects, slices, and closure objects carry metadata"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("panicked at") && !stderr.contains("Kern Compiler Internal Error"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_result_type_in_pointer_static_initializer_without_panicking() {
    let output = compile_source(
        r#"
type FramebufferRequest = struct {
    response: *u8,
};

static REQUEST = FramebufferRequest.{ response: i32!u8 };

fn main() i32 {
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
        stderr.contains("result types cannot be evaluated as value expressions"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains(
            "results are types; construct values with `T!E.{ Ok: ... }` or `T!E.{ Err: ... }`"
        ),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("panicked at")
            && !stderr.contains("Kern Compiler Internal Error")
            && !stderr.contains("expected a valid constant expression"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_optional_alias_in_pointer_static_initializer_without_panicking() {
    let output = compile_source(
        r#"
type MaybeByte = ?u8;

type FramebufferRequest = struct {
    response: *u8,
};

static REQUEST = FramebufferRequest.{ response: MaybeByte };

fn main() i32 {
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
        stderr.contains("optional types cannot be evaluated as value expressions"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("if you meant the empty optional constructor, write `?T.None`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("panicked at")
            && !stderr.contains("Kern Compiler Internal Error")
            && !stderr.contains("expected a valid constant expression"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_result_alias_in_pointer_static_initializer_without_panicking() {
    let output = compile_source(
        r#"
type ResultByte = i32!u8;

type FramebufferRequest = struct {
    response: *u8,
};

static REQUEST = FramebufferRequest.{ response: ResultByte };

fn main() i32 {
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
        stderr.contains("result types cannot be evaluated as value expressions"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains(
            "results are types; construct values with `T!E.{ Ok: ... }` or `T!E.{ Err: ... }`"
        ),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("panicked at")
            && !stderr.contains("Kern Compiler Internal Error")
            && !stderr.contains("expected a valid constant expression"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_integer_pointer_static_initializer_without_panicking() {
    let output = compile_source(
        r#"
type FramebufferRequest = struct {
    response: *u8,
};

static REQUEST = FramebufferRequest.{ response: 0 };

fn main() i32 {
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
        stderr.contains("mismatched types"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("expected `*u8`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("found `i32`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("panicked at") && !stderr.contains("Kern Compiler Internal Error"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_missing_struct_field_in_static_initializer_without_panicking() {
    let output = compile_source(
        r#"
type Pair = struct {
    a: u64,
    b: u64,
};

static BAD = Pair.{ a: 1 };

fn main() i32 {
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
        stderr.contains("field `b` is missing and has no default value"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("panicked at") && !stderr.contains("Kern Compiler Internal Error"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_unknown_struct_field_in_static_initializer_without_panicking() {
    let output = compile_source(
        r#"
type Pair = struct {
    a: u64,
};

static BAD = Pair.{ a: 1, b: 2 };

fn main() i32 {
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
        stderr.contains("field `b` does not exist in `Pair`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("panicked at") && !stderr.contains("Kern Compiler Internal Error"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_static_array_initializer_length_mismatch_without_panicking() {
    let output = compile_source(
        r#"
static BAD = [2]u8.{ 1, 2, 3 };

fn main() i32 {
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
        stderr.contains("array literal length (3) does not match expected length (2)"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("panicked at") && !stderr.contains("Kern Compiler Internal Error"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_static_enum_initializer_with_multiple_variants_without_panicking() {
    let output = compile_source(
        r#"
type Option[T] = enum {
    None,
    Some: T,
};

static BAD = Option[i32].{ None: 0, Some: 1 };

fn main() i32 {
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
        stderr.contains("Enum literal must specify exactly one variant"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("panicked at")
            && !stderr.contains("Kern Compiler Internal Error")
            && !stderr.contains("cannot resolve global constant"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_static_enum_initializer_payload_for_payloadless_variant_without_panicking() {
    let output = compile_source(
        r#"
type Option[T] = enum {
    None,
    Some: T,
};

static BAD = Option[i32].{ None: 1 };

fn main() i32 {
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
        stderr.contains("variant `None` does not take a payload"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("panicked at") && !stderr.contains("Kern Compiler Internal Error"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_static_enum_initializer_missing_payload_without_panicking() {
    let output = compile_source(
        r#"
type Option[T] = enum {
    None,
    Some: T,
};

static BAD = Option[i32].Some;

fn main() i32 {
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
        stderr.contains("variant `Some` requires a payload"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("panicked at") && !stderr.contains("Kern Compiler Internal Error"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn accepts_large_u128_constant_literals() {
    let output = build_and_run_source(
        r#"
const MID = u128.{170141183460469231731687303715884105728};
const MAX = u128.{340282366920938463463374607431768211455};

fn main() i32 {
    if (!(MAX > MID)) {
        return 1;
    }
    if (!(MID > u128.{1})) {
        return 2;
    }
    return 0;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "large u128 literal regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn folds_large_u128_constant_comparisons_correctly() {
    let output = build_and_run_source(
        r#"
const MID = u128.{170141183460469231731687303715884105728};
const OK = MID > u128.{1};

fn main() i32 {
    return if (OK) 0 else 1;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "large u128 const comparison regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
