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
fn accepts_const_generic_supertrait_object_upcast() {
    let output = build_and_run_source(
        r#"
type Base[N: usize] = trait {
    value: fn() i32,
};

type Derived[N: usize]: Base[N] = trait {};

type X = struct {};

impl *X: Derived[4] {}

impl *X: Base[4] {
    fn value() i32 {
        return 7;
    }
}

fn main() i32 {
    let x = X.{};
    let derived = *Derived[4].{ x.& };
    let base = *Base[4].{ derived };
    return base.value() - 7;
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
fn dispatches_inherited_const_supertrait_method_from_generic_trait_object_impl() {
    let output = build_and_run_source(
        r#"
type Base[N: usize] = trait {
    value: fn() i32,
};

type Derived[N: usize]: Base[N] = trait {};

type X = struct {};

impl[N: usize] *X: Derived[N] {}

impl[N: usize] *X: Base[N] {
    fn value() i32 {
        return N as i32;
    }
}

fn main() i32 {
    let x = X.{};
    let derived = *Derived[4].{ x.& };
    return derived.value() - 4;
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
fn preserves_specialized_assoc_bindings_while_lowering_supertrait_trait_objects() {
    let output = build_and_run_source(
        r#"
type Width[N: usize] = trait {
    width: fn() i32,
};

type Label = trait {
    label: fn() i32,
};

type Mapped[N: usize]: Width[N] + Label = trait {
    fold: fn() i32,
};

type Check[N: usize] = trait {
    type Ty: Mapped[N];
    make: fn() Ty,
};

type RichCheck[N: usize]: Check[N] + Width[N] = trait {
    prove: fn() i32,
};

type Data = struct {
    seed: i32,
};

type GenericProof[N: usize] = struct {
    seed: i32,
};

type QuadProof = struct {
    seed: i32,
    bonus: i32,
};

impl[N: usize] GenericProof[N]: Width[N] {
    fn width() i32 {
        return N as i32;
    }
}

impl[N: usize] GenericProof[N]: Label {
    fn label() i32 {
        return self.seed;
    }
}

impl[N: usize] GenericProof[N]: Mapped[N] {
    fn fold() i32 {
        return self.label() + self.width();
    }
}

impl QuadProof: Width[4] {
    fn width() i32 {
        return 4;
    }
}

impl QuadProof: Label {
    fn label() i32 {
        return self.seed + self.bonus;
    }
}

impl QuadProof: Mapped[4] {
    fn fold() i32 {
        return self.label() + self.width();
    }
}

impl[N: usize] *Data: Width[N] {
    fn width() i32 {
        return N as i32;
    }
}

impl[N: usize] *Data: Check[N] {
    type Ty = GenericProof[N];

    fn make() Ty {
        return GenericProof[N].{ seed: self.seed };
    }
}

impl *Data: Check[4] {
    type Ty = QuadProof;

    fn make() Ty {
        return QuadProof.{ seed: self.seed, bonus: 20 };
    }
}

impl[N: usize] *Data: RichCheck[N]
    where *Data: Check[N],
{
    fn prove() i32 {
        return self.make().fold() + self.width();
    }
}

fn via_object(value: *Data) i32 {
    let rich = *RichCheck[4].{ value };
    return rich.prove() + rich.width();
}

fn main() i32 {
    let data = Data.{ seed: 7 };
    return via_object(data.&) - 39;
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
