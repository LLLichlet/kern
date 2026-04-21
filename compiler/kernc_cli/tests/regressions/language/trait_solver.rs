use super::*;
#[test]
fn rejects_uninstantiated_generic_function_items_in_value_position() {
    let output = compile_source(
        r#"
fn id[T](x: T) T {
    return x;
}

fn main() i32 {
    let _ = id;
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
        stderr.contains(
            "generic function `id` cannot be used as a value without explicit instantiation"
        ),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("use `id[...]` with concrete generic arguments"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("Kern ICE"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_non_trait_where_clause_bounds() {
    let output = compile_source(
        r#"
fn f[A](a: A) A where A: A {
    return a;
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
        stderr.contains("where-clause bounds must name a trait"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("found `A`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_invalid_assoc_projection_where_bound_without_overflowing() {
    let output = compile_source(
        r#"
type N = trait { type O : N; };

fn f[A](a: A) A where A.N.O : A { return a.a.a.a; }
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
        stderr.contains("where-clause bounds must name a trait"),
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
fn accepts_unique_more_specific_overlapping_trait_impl() {
    let output = compile_source(
        r#"
type Score = trait {
    value: fn() i32,
};

impl[T] []T : Score {
    fn value() i32 {
        return 1;
    }
}

impl []u8 : Score {
    fn value() i32 {
        return 2;
    }
}

fn score(bytes: []u8) i32 {
    return bytes.value();
}

fn main() i32 {
    return score("ok") - 1;
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
fn accepts_unique_more_specific_overlapping_trait_impl_with_const_args() {
    let output = build_and_run_source(
        r#"
type Score = trait {
    value: fn() i32,
};

type Buf[N: usize] = struct {};

impl[N: usize] Buf[N]: Score {
    fn value() i32 {
        return 1;
    }
}

impl Buf[4]: Score {
    fn value() i32 {
        return 2;
    }
}

fn score(buf: Buf[4]) i32 {
    return buf.value();
}

fn main() i32 {
    return score(Buf[4].{}) - 2;
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
fn rejects_more_specific_overlap_with_conflicting_associated_type_proofs() {
    let output = compile_source(
        r#"
type TypeIs[T] = trait {
    type Is;
};

impl[S, T] S: TypeIs[T] {
    type Is = T;
}

type FakeProof[L, R] = struct {};

impl[L, R] FakeProof[L, R]: TypeIs[R] {
    type Is = L;
}

fn rewrite[R, Rw](value: Rw.TypeIs[R].Is) R
    where Rw: TypeIs[R, Is = R],
{
    return value;
}

fn cast[L, R](value: L) R
    where FakeProof[L, R]: TypeIs[R, Is = L],
{
    return rewrite[R, FakeProof[L, R]](value);
}

fn seed() i32 {
    return 11;
}

fn forge[R]() R {
    return cast[fn() i32, R](seed);
}

fn main() i32 {
    return forge[i32]();
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
        stderr.contains("type does not satisfy trait bounds")
            || stderr.contains("cannot resolve associated type projection"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("FakeProof[L, R]: TypeIs[R, Is = R]")
            || stderr.contains("TypeIs[R, Is = R]"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_overlapping_trait_impls_with_conflicting_associated_type_proofs() {
    let output = compile_source(
        r#"
type TypeIs[T] = trait {
    type Is;
};

type Proof[L, R] = struct {};

impl[L, R] Proof[L, R]: TypeIs[L] {
    type Is = L;
}

impl[L, R] Proof[L, R]: TypeIs[R] {
    type Is = L;
}

impl[L, R] Proof[L, R]: TypeIs[R] {
    type Is = R;
}

fn rewrite[RW, R](value: RW.TypeIs[R].Is) R
    where RW: TypeIs[R, Is = R],
{
    return value;
}

fn cast[L, R](value: L) R
    where Proof[L, R]: TypeIs[L, Is = L],
          Proof[L, R]: TypeIs[R, Is = R],
{
    return rewrite[Proof[L, R], R](value);
}

fn main() i32 {
    let _ = cast[bool, i32](true);
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
    assert!(
        stderr.contains("associated type projection ambiguous") || stderr.contains("global proofs"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_projection_from_global_impl_with_unsatisfied_where_clause() {
    let output = compile_source(
        r#"
type Need = trait {};

type HasOut = trait {
    type Out;
};

type X = struct {};

impl[T] T: HasOut
    where T: Need,
{
    type Out = i32;
}

fn take(value: X.HasOut.Out) i32 {
    return value;
}

fn main() i32 {
    return take(7);
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
        stderr.contains("type does not satisfy trait bounds")
            || stderr.contains("cannot resolve associated type projection")
            || stderr.contains("mismatched types"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_direct_trait_proof_against_shadowed_generic_assoc_impl() {
    let output = compile_source(
        r#"
type HasOut[N: usize] = trait {
    type Out;
};

type X = struct {};

impl[N: usize] X: HasOut[N] {
    type Out = i32;
}

impl X: HasOut[4] {
    type Out = i64;
}

fn need[T](value: T) void
    where T: HasOut[4, Out = i32],
{
    let _ = value;
}

fn main() i32 {
    need(X.{});
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
        stderr.contains("type does not satisfy trait bounds"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("X: HasOut[4, Out = i32]"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_trait_object_assoc_binding_against_shadowed_generic_impl() {
    let output = compile_source(
        r#"
type Factory[N: usize] = trait {
    type Out;
    make: fn() Out,
};

type X = struct {};

impl[N: usize] *X: Factory[N] {
    type Out = i32;

    fn make() Out {
        return 1;
    }
}

impl *X: Factory[4] {
    type Out = i64;

    fn make() Out {
        return 9;
    }
}

fn main() i32 {
    let x = X.{};
    let _ = *Factory[4, Out = i32].{ x.& };
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
        stderr.contains("mismatched types")
            || stderr.contains("type does not satisfy trait bounds")
            || stderr.contains("does not implement the target trait"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("Factory[4, Out = i32]") || stderr.contains("Out = i32"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_trait_impls_when_their_where_clauses_are_unsatisfied() {
    let output = compile_source(
        r#"
type Marker = trait {};
type Need = trait {};

impl[T] T : Marker where T: Need {}

fn requires_marker[T](value: T) void where T: Marker {
    let _ = value;
}

fn main() i32 {
    requires_marker(i32.{123});
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
        stderr.contains("type does not satisfy trait bounds"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("required bound: `i32: Marker`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_occurs_check_violation_when_matching_env_trait_bounds() {
    let output = compile_source(
        r#"
type Wrap[T] = struct {
    inner: T,
};

type Marker[T] = trait {
    value: fn() i32,
};

fn needs_self[T](value: T) i32
    where T: Marker[T],
{
    return value.value();
}

fn bad[T](value: T) i32
    where T: Marker[Wrap[T]],
{
    return needs_self[T](value);
}

type X = struct {};

impl X: Marker[Wrap[X]] {
    fn value() i32 {
        return 42;
    }
}

fn main() i32 {
    return bad(X.{});
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
        stderr.contains("type does not satisfy trait bounds"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("required bound: `T: Marker[T]`")
            || stderr.contains("required bound: `X: Marker[X]`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_indirect_occurs_check_cycle_through_multiple_trait_args() {
    let output = compile_source(
        r#"
type Wrap[T] = struct {
    inner: T,
};

type Marker[A, B] = trait {
    value: fn() i32,
};

fn needs_self[T, U](value: T) i32
    where T: Marker[T, U],
{
    return value.value();
}

fn bad[T, U](value: T) i32
    where T: Marker[U, Wrap[T]],
{
    return needs_self[T, U](value);
}

type X = struct {};

impl X: Marker[Wrap[X], Wrap[X]] {
    fn value() i32 {
        return 42;
    }
}

fn main() i32 {
    return bad[X, Wrap[X]](X.{});
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
        stderr.contains("type does not satisfy trait bounds"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("required bound: `T: Marker[T, U]`")
            || stderr.contains("required bound: `X: Marker[X, Wrap[X]]`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_mismatched_const_trait_bound_arguments() {
    let output = compile_source(
        r#"
type Cap[N: usize] = trait {
    value: fn() i32,
};

type X = struct {};

impl X: Cap[0] {
    fn value() i32 {
        return 0;
    }
}

fn need[T](x: T) i32
    where T: Cap[1],
{
    return x.value();
}

fn main() i32 {
    return need(X.{});
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
        stderr.contains("type does not satisfy trait bounds"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("required bound: `T: Cap[1]`")
            || stderr.contains("required bound: `X: Cap[1]`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_reverse_solving_const_env_bound_for_direct_trait_proof() {
    let output = compile_source(
        r#"
type Cap[N: usize] = trait {
    value: fn() i32,
};

type X = struct {};

impl X: Cap[1] {
    fn value() i32 {
        return 1;
    }
}

fn needs[T, N: usize](x: T) i32
    where T: Cap[N],
{
    return x.value();
}

fn bad[T, N: usize](x: T) i32
    where T: Cap[1],
{
    return needs[T, N](x);
}

fn main() i32 {
    return bad[X, 2](X.{});
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
        stderr.contains("type does not satisfy trait bounds"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("required bound: `T: Cap[N]`")
            || stderr.contains("required bound: `X: Cap[2]`"),
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
fn rejects_reverse_solving_const_global_impl_for_direct_trait_proof() {
    let output = compile_source(
        r#"
type Cap[N: usize] = trait {
    value: fn() i32,
};

type X = struct {};

impl X: Cap[1] {
    fn value() i32 {
        return 1;
    }
}

fn needs[N: usize](x: X) i32
    where X: Cap[N],
{
    return x.value();
}

fn bad[N: usize]() i32 {
    return needs[N](X.{});
}

fn main() i32 {
    return bad[2]();
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
        stderr.contains("type does not satisfy trait bounds"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("required bound: `X: Cap[N]`")
            || stderr.contains("required bound: `X: Cap[2]`"),
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
fn rejects_reverse_solving_const_global_impl_for_projection() {
    let output = compile_source(
        r#"
type HasOut[N: usize] = trait {
    type Out;
};

type X = struct {};

impl X: HasOut[1] {
    type Out = i32;
}

fn need[N: usize](value: X.HasOut[N].Out) i32 {
    return value;
}

fn main() i32 {
    return need[2](7);
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
        stderr.contains("mismatched types")
            || stderr.contains("type does not satisfy trait bounds"),
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
fn rejects_reverse_solving_const_receiver_for_inherent_impl_method() {
    let output = compile_source(
        r#"
type Box[N: usize] = struct {};

impl Box[1] {
    fn tag() i32 {
        return 1;
    }
}

fn bad[N: usize](value: Box[N]) i32 {
    return value.tag();
}

fn main() i32 {
    return bad[2](Box[2].{});
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
        stderr.contains("no field or method named `tag` found on type `Box[N]`")
            || stderr.contains("no field or method named `tag` found on type `Box[2]`"),
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
fn rejects_self_recursive_trait_impl_where_clauses_without_overflowing() {
    let output = compile_source(
        r#"
type Marker = trait {};

impl[T] T : Marker where T: Marker {}

fn requires_marker[T](value: T) void where T: Marker {
    let _ = value;
}

fn main() i32 {
    requires_marker(i32.{123});
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
        stderr.contains("impl cannot require itself in its own where-clause"),
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
fn rejects_self_referential_impl_where_clauses_with_associated_types() {
    let output = compile_source(
        r#"
type Forge = trait {
    type Out;
    make: fn() Out,
};

type Carrier[T] = struct {};

impl[T] Carrier[T] : Forge
    where Carrier[T]: Forge,
{
    type Out = T;

    fn make() Out {
        return self.make();
    }
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
        stderr.contains("impl cannot require itself in its own where-clause"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("Carrier[T]: Forge"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn suppresses_followup_missing_method_error_for_self_referential_impls() {
    let output = compile_source(
        r#"
type Forge = trait {
    type Out;
    make: fn() Out,
};

type Carrier[T] = struct {};

impl[T] Carrier[T] : Forge
    where Carrier[T]: Forge,
{
    type Out = T;

    fn make() Out {
        return self.make();
    }
}

fn conjure[T]() T {
    return Carrier[T].{}.make();
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
        stderr.contains("impl cannot require itself in its own where-clause"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("no field or method named `make` found"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_self_referential_generic_trait_impl_where_clauses() {
    let output = compile_source(
        r#"
type Forge[T] = trait {
    make: fn() T,
};

type Carrier[T] = struct {};

impl[T] Carrier[T] : Forge[T]
    where Carrier[T]: Forge[T],
{
    fn make() T {
        return self.make();
    }
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
        stderr.contains("impl cannot require itself in its own where-clause"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("Carrier[T]: Forge[T]"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_cyclic_trait_impl_proof_chains() {
    let output = compile_source(
        r#"
type Pre[T] = trait {};

type Forge[T] = trait {
    make: fn() T,
};

type Carrier[T] = struct {};

impl[T] Carrier[T] : Pre[T]
    where Carrier[T]: Forge[T],
{}

impl[T] Carrier[T] : Forge[T]
    where Carrier[T]: Pre[T],
{
    fn make() T {
        return self.make();
    }
}

fn conjure[T]() T {
    return Carrier[T].{}.make();
}

fn main() i32 {
    let _ = conjure[i32]();
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
        stderr.contains("impl requirement participates in a cyclic proof"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains(
            "proof cycle: Carrier[T]: Forge[T] -> Carrier[T]: Pre[T] -> Carrier[T]: Forge[T]"
        ),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("no field or method named `make` found"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn accepts_trait_impls_when_their_where_clauses_are_satisfied() {
    let output = build_and_run_source(
        r#"
type Marker = trait {};
type Need = trait {};

impl i32 : Need {}
impl[T] T : Marker where T: Need {}

fn requires_marker[T](value: T) i32 where T: Marker {
    let _ = value;
    return 0;
}

fn main() i32 {
    return requires_marker(i32.{123});
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "trait impl where-clause regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
