use super::*;

#[test]
fn accepts_acyclic_supertrait_hierarchy_with_changed_generic_arguments() {
    let output = build_and_run_source(
        r#"
type Wrap[T] = struct {
    inner: T,
};

type Base[T] = trait {
    get: fn() T,
};

type Derived[T]: Base[Wrap[T]] = trait {
    bonus: fn(T) T,
};

impl *i32 : Base[Wrap[i32]] {
    fn get() Wrap[i32] {
        return Wrap[i32].{ inner: self.* };
    }
}

impl *i32 : Derived[i32] {
    fn bonus(v: i32) i32 {
        return self.* + v;
    }
}

fn takes_base(x: *Base[Wrap[i32]]) i32 {
    return x.get().inner;
}

fn main() i32 {
    let value = i32.{5};
    let derived = *Derived[i32].{ value.& };
    let base = *Base[Wrap[i32]].{ derived };
    if (base.get().inner + takes_base(derived) + derived.bonus(4) == 19) {
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
fn rejects_self_recursive_supertrait_hierarchy() {
    let output = compile_source(
        r#"
type Wrap[T] = struct {
    inner: T,
};

type A[T]: A[Wrap[T]] = trait {
    value: fn() i32,
};

type X = struct {};

impl *X: A[i32] {
    fn value() i32 {
        return 1;
    }
}

fn main() i32 {
    let x = X.{};
    let a = *A[i32].{ x.& };
    return a.value();
}
"#,
    );

    assert!(
        !output.status.success(),
        "recursive supertrait program compiled successfully:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("trait supertrait hierarchy contains a cycle"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("cycle: A -> A"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("must form a DAG"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_mutually_recursive_supertrait_hierarchy() {
    let output = compile_source(
        r#"
type Wrap[T] = struct {
    inner: T,
};

type A[T]: B[Wrap[T]] = trait {
    a: fn() i32,
};

type B[T]: A[Wrap[T]] = trait {
    b: fn() i32,
};

type X = struct {};

impl *X: A[i32] {
    fn a() i32 {
        return 1;
    }
}

impl *X: B[Wrap[i32]] {
    fn b() i32 {
        return 2;
    }
}

fn main() i32 {
    let x = X.{};
    let a = *A[i32].{ x.& };
    return a.a();
}
"#,
    );

    assert!(
        !output.status.success(),
        "mutually recursive supertrait program compiled successfully:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("trait supertrait hierarchy contains a cycle"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("cycle: A -> B -> A"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("non-decreasing") || stderr.contains("structural constructor count grows"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("must form a DAG"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_equal_size_supertrait_cycles() {
    let output = compile_source(
        r#"
type A[T]: B[T] = trait {};
type B[T]: A[T] = trait {};

fn main() i32 {
    return 0;
}
"#,
    );

    assert!(
        !output.status.success(),
        "equal-size recursive supertrait program compiled successfully:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("trait supertrait hierarchy contains a cycle"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("cycle: A -> B -> A"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("equal-size cycle is rejected"),
        "unexpected stderr:\n{}",
        stderr
    );
}
