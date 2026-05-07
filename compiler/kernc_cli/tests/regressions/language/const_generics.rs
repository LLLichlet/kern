use super::*;
#[test]
fn rejects_infinite_polymorphic_recursion_with_instantiation_chain() {
    let output = compile_source(
        r#"
struct Wrap[T] {
    inner: T,
};

fn poly[T](x: T) i32 {
    return poly[Wrap[T]](Wrap[T].{ inner: x });
}

fn main() i32 {
    return poly[i32](0);
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
        stderr.contains("infinitely many specializations"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("instantiation chain: poly[i32] -> poly[Wrap[i32]]"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("stack overflow"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_mutual_polymorphic_recursion_with_instantiation_chain() {
    let output = compile_source(
        r#"
struct Wrap[T] {
    inner: T,
};

fn f[T](x: T) i32 {
    return g[Wrap[T]](Wrap[T].{ inner: x });
}

fn g[T](x: T) i32 {
    return f[Wrap[T]](Wrap[T].{ inner: x });
}

fn main() i32 {
    return f[i32](0);
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
        stderr.contains("infinitely many specializations"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("instantiation chain: f[i32] -> g[Wrap[i32]] -> f[Wrap[Wrap[i32]]]"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("stack overflow"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_const_generic_polymorphic_recursion_with_specialization_diagnostic() {
    let output = compile_source(
        r#"
fn grow[N: usize]() i32 {
    return grow[N + 1]();
}

fn main() i32 {
    return grow[0]();
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
        stderr.contains("recursive specialization depth limit")
            || stderr.contains("specialization work queue limit")
            || stderr.contains("specialization limit"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("grow[0] -> grow[1]"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("const generic arguments do not stabilize across recursive calls"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("stack overflow"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_mutual_const_generic_polymorphic_recursion_with_specialization_diagnostic() {
    let output = compile_source(
        r#"
fn f[N: usize]() i32 {
    return g[N + 1]();
}

fn g[N: usize]() i32 {
    return f[N + 1]();
}

fn main() i32 {
    return f[0]();
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
        stderr.contains("recursive specialization depth limit")
            || stderr.contains("specialization work queue limit")
            || stderr.contains("specialization limit"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("f[0] -> g[1] -> f[2]"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("const generic arguments do not stabilize across recursive calls"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("stack overflow"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn runs_const_generic_params_in_ordinary_expressions() {
    let output = build_and_run_source(
        r#"
enum Mode {
    Off,
    On,
};

fn leaf[N: usize]() i32 {
    return N as i32;
}

fn forward[N: usize]() i32 {
    return leaf[N]() + leaf[N + 1]();
}

fn choose[B: bool]() i32 {
    if (B) {
        return 11;
    }

    return 22;
}

fn select_mode[M: Mode]() i32 {
    return match (M) {
        .Off => 30,
        .On => 40,
    };
}

fn main() i32 {
    return forward[7]() + choose[true]() + choose[false]() + select_mode[Mode.On]() - 88;
}
"#,
    );

    assert!(
        output.status.success(),
        "program failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn accepts_large_non_recursive_specialization_batches_without_queue_limit_false_positive() {
    let mut source = String::from(
        r#"
fn id[N: usize](x: i32) i32 {
    return x;
}

fn main() i32 {
    let mut sum = 0;
"#,
    );

    for n in 0..1100 {
        source.push_str(&format!("    sum += id[{}](1);\n", n));
    }

    source.push_str(
        r#"
    return if (sum == 1100) 0 else 1;
}
"#,
    );

    let output = build_and_run_source(&source);

    assert!(
        output.status.success(),
        "expected compilation success, but kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("specialization work queue limit"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn runs_const_fn_with_const_generic_arguments_during_consteval() {
    let output = build_and_run_source(
        r#"
const fn bump[N: usize]() usize {
    return N + 1;
}

const fn width[N: usize](value: [N]u8) usize {
    return (value.[0] as usize) + bump[N]() + @sizeOf[[N]u8]();
}

const TOTAL = width[3]([3]u8.{ 1, 2, 3 });

fn main() i32 {
    return TOTAL as i32;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(8),
        "const generic consteval regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn normalizes_const_trait_projection_during_expression_typeck() {
    let output = build_and_run_source(
        r#"
trait HasOut[N: usize] {
    type Out;
};

struct X {};

impl X: HasOut[1] {
    type Out = i32;
}

fn take(value: X.HasOut[1].Out) i32 {
    return value;
}

fn main() i32 {
    return take(7);
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(7),
        "const trait projection regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn normalizes_const_trait_projection_from_env_bounds() {
    let output = build_and_run_source(
        r#"
trait HasOut[N: usize] {
    type Out;
};

struct X {};

impl X: HasOut[1] {
    type Out = i32;
}

fn lift[T](value: T.HasOut[1].Out) T.HasOut[1].Out
    where T: HasOut[1, Out = i32],
{
    return value;
}

fn main() i32 {
    return lift[X](7);
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(7),
        "const trait projection env-bound regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn normalizes_const_specific_assoc_projection_over_generic_impl() {
    let output = compile_source(
        r#"
trait HasOut[N: usize] {
    type Out;
};

struct X {};

impl[N: usize] X: HasOut[N] {
    type Out = i32;
}

impl X: HasOut[4] {
    type Out = i64;
}

fn take(value: X.HasOut[4].Out) i32 {
    let _ = value;
    return 0;
}

fn main() i32 {
    return take(i64.{7});
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
fn dispatches_const_specific_trait_object_method_with_assoc_binding() {
    let output = build_and_run_source(
        r#"
trait Factory[N: usize] {
    type Out;
    fn make() Out;
};

struct X {};

impl[N: usize] &X: Factory[N] {
    type Out = i32;

    fn make() Out {
        return 1;
    }
}

impl &X: Factory[4] {
    type Out = i64;

    fn make() Out {
        return 9;
    }
}

fn main() i32 {
    let x = X.{};
    let factory = &Factory[4, Out = i64].{ x.& };
    let value = factory.make();
    if (value != i64.{9}) {
        return 1;
    }
    return 0;
}
"#,
    );

    assert!(
        output.status.success(),
        "hosted regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_ambiguous_const_target_specialization_overlap() {
    let output = compile_source(
        r#"
trait HasOut {
    type Out;
};

struct Pair[A: usize, B: usize] {};

impl[N: usize] Pair[0, N]: HasOut {
    type Out = i32;
}

impl[N: usize] Pair[N, 0]: HasOut {
    type Out = i64;
}

fn main() i32 {
    let _ = Pair[0, 0].{};
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
        stderr.contains("overlapping trait impls are not allowed"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_non_contractive_const_trait_projection_cycle_without_ice() {
    let output = compile_source(
        r#"
trait Loop[N: usize] {
    type Out;
};

struct X {};

impl[N: usize] X: Loop[N] {
    type Out = X.Loop[N + 1].Out;
}

fn project(value: X) X.Loop[0].Out {
    let _ = value;
    return X.{};
}

fn main() i32 {
    let _ = project(X.{});
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
        stderr.contains("recursive associated type projection cycle detected"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("Kern Compiler Internal Error"),
        "unexpected ICE stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_reverse_solving_const_env_bound_for_projection() {
    let output = compile_source(
        r#"
trait HasOut[N: usize] {
    type Out;
};

struct X {};

impl X: HasOut[1] {
    type Out = i32;
}

fn bad[T, N: usize]() T.HasOut[N].Out
    where T: HasOut[1, Out = i32],
{
    return 0;
}

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
        stderr.contains("expected `T.HasOut[N].Out`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("found `i32`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("Kern Compiler Internal Error"),
        "unexpected ICE stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_nested_typed_struct_pattern_with_mismatched_const_generic_argument() {
    let output = compile_source(
        r#"
struct Inner[N: usize] {
    data: [N]u8,
};

struct Outer[N: usize] {
    inner: Inner[N],
};

fn main() i32 {
    let value = Outer[3].{ inner: Inner[3].{ data: [3]u8.{ 1, 2, 3 } } };
    let Outer[3].{ inner: Inner[4].{ data } } = value;
    return data.[0] as i32;
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
        stderr.contains("expected `Inner[3]`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("found `Inner[4]`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_nested_typed_enum_pattern_with_mismatched_const_generic_argument() {
    let output = compile_source(
        r#"
enum Inner[N: usize] {
    A: [N]u8,
    B,
};

enum Outer[N: usize] {
    Wrap: Inner[N],
    Done,
};

fn main() i32 {
    let value = Outer[3].{ Wrap: Inner[3].{ A: [3]u8.{ 1, 2, 3 } } };
    return match (value) {
        .{ Wrap: Inner[4].{ A: _ } } => 0,
        .{ Wrap: .B } => 1,
        .Done => 2,
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
        stderr.contains("mismatched types"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("expected `Inner[3]`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("found `Inner[4]`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn runs_nested_typed_const_generic_patterns() {
    let output = build_and_run_source(
        r#"
enum Inner[N: usize] {
    A: [N]u8,
    B,
};

struct Outer[N: usize] {
    inner: Inner[N],
};

fn classify(value: Outer[3]) i32 {
    return match (value) {
        Outer[3].{ inner: Inner[3].{ A: _ } } => 4,
        Outer[3].{ inner: .B } => 5,
    };
}

fn main() i32 {
    let value = Outer[3].{ inner: Inner[3].{ A: [3]u8.{ 4, 5, 6 } } };
    let Outer[3].{ inner: Inner[3].{ A: payload } } =
        Outer[3].{ inner: Inner[3].{ A: [3]u8.{ 4, 5, 6 } } } else return 1;
    return classify(value) + (payload.[2] as i32) - 10;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "nested const-generic pattern regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
