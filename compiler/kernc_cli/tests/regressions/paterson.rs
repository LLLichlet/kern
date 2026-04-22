use super::*;

#[test]
fn accepts_equal_size_impl_prerequisite_when_it_is_acyclic() {
    let output = build_and_run_source(
        r#"
type Need = trait {};
type Marker = trait {};

impl *i32 : Need {}

impl[T] T : Marker
    where T: Need,
{}

fn requires_marker[T](value: T) i32
    where T: Marker,
{
    let _ = value;
    return 7;
}

fn main() i32 {
    let value = i32.{42};
    if (requires_marker(value.&) == 7) {
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
fn rejects_impl_prerequisite_that_grows_structural_size() {
    let output = compile_source(
        r#"
type Marker = trait {};

type Wrap[T] = struct {
    inner: T,
};

impl[T] Wrap[T] : Marker
    where Wrap[Wrap[T]]: Marker,
{}

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
        stderr.contains("impl prerequisite is not structurally bounded by the impl head"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("structural constructor count grows"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("termination check"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_impl_prerequisite_that_duplicates_parameters() {
    let output = compile_source(
        r#"
type Marker = trait {};
type Need[A, B] = trait {};

impl[T] T : Marker
    where T: Need[T, T],
{}

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
        stderr.contains("impl prerequisite is not structurally bounded by the impl head"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("`T` occurs"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("termination check"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_impl_prerequisite_that_grows_const_generic_structure() {
    let output = compile_source(
        r#"
type Marker = trait {};

type Buf[N: usize] = struct {};

impl[N: usize] Buf[N] : Marker
    where Buf[N + 1]: Marker,
{}

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
        stderr.contains("impl prerequisite is not structurally bounded by the impl head"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("Buf[N]: Marker"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("Buf[N + 1]: Marker"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("structural constructor count grows"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn non_decreasing_impl_does_not_trigger_unrelated_cycle_diagnostic() {
    let output = compile_source(
        r#"
type Marker = trait {};
type Need = trait {};

type Wrap[T] = struct {
    inner: T,
};

impl[T] T : Marker
    where T: Need,
{}

impl[T] T : Need
    where Wrap[T]: Need,
          T: Marker,
{}

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
        stderr.contains("impl prerequisite is not structurally bounded by the impl head"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("Wrap[T]: Need"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("impl requirement participates in a cyclic proof"),
        "unexpected stderr:\n{}",
        stderr
    );
}
