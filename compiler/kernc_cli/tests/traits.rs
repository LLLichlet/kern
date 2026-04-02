mod support;

use support::{build_and_run, compile_source_with_args};

fn compile_source(source: &str) -> std::process::Output {
    compile_source_with_args("kernc_trait_test", source, &[])
}

#[test]
fn compiles_multi_supertrait_lookup_through_generic_bound() {
    let output = compile_source(
        r#"
type A = trait { a: fn() i32, };
type B = trait { b: fn() i32, };
type C: A + B = trait { c: fn() i32, };

impl *i32 : A { pub fn a() i32 { return self.*; } }
impl *i32 : B { pub fn b() i32 { return self.* + 10; } }
impl *i32 : C { pub fn c() i32 { return self.* + 100; } }

fn use_it[T](x: *T) i32
    where *T: C,
{
    return x.a() + x.b() + x.c();
}

extern fn main(args: [][]u8) i32 {
    let v = i32.{1};
    return use_it(v.&);
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
fn compiles_supertrait_methods_on_trait_object() {
    let output = compile_source(
        r#"
type Base = trait { foo: fn() i32, };
type Derived: Base = trait { bar: fn() i32, };

impl *i32 : Base { pub fn foo() i32 { return self.*; } }
impl *i32 : Derived { pub fn bar() i32 { return self.* + 1; } }

extern fn main(args: [][]u8) i32 {
    let v = i32.{3};
    let d = *Derived.{ v.& };
    return d.foo() + d.bar();
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
fn compiles_trait_object_from_concrete_pointer_via_constructor_and_bnc() {
    let output = compile_source(
        r#"
type Base = trait { foo: fn() i32, };

impl *i32 : Base {
    pub fn foo() i32 { return self.*; }
}

fn takes_base(x: *Base) i32 {
    return x.foo();
}

extern fn main(args: [][]u8) i32 {
    let v = i32.{3};
    let explicit = *Base.{ v.& };
    return explicit.foo() + takes_base(v.&);
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
fn compiles_trait_object_upcast_via_explicit_constructor_and_bnc() {
    let output = compile_source(
        r#"
type Base = trait { foo: fn() i32, };
type Derived: Base = trait { bar: fn() i32, };

impl *i32 : Base { pub fn foo() i32 { return self.*; } }
impl *i32 : Derived { pub fn bar() i32 { return self.* + 1; } }

fn takes_base(x: *Base) i32 {
    return x.foo();
}

extern fn main(args: [][]u8) i32 {
    let v = i32.{3};
    let d = *Derived.{ v.& };
    let b = *Base.{ d };
    return b.foo() + takes_base(d) + d.bar();
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
fn compiles_multi_parent_trait_object_upcasts() {
    let output = compile_source(
        r#"
type A = trait { a: fn() i32, };
type B = trait { b: fn() i32, };
type C: A + B = trait { c: fn() i32, };

impl *i32 : A { pub fn a() i32 { return self.*; } }
impl *i32 : B { pub fn b() i32 { return self.* + 10; } }
impl *i32 : C { pub fn c() i32 { return self.* + 100; } }

fn takes_a(x: *A) i32 {
    return x.a();
}

fn takes_b(x: *B) i32 {
    return x.b();
}

extern fn main(args: [][]u8) i32 {
    let v = i32.{3};
    let c = *C.{ v.& };
    let a = *A.{ c };
    let b = *B.{ c };
    return a.a() + b.b() + c.c() + takes_a(c) + takes_b(c);
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
fn compiles_generic_parent_trait_object_upcast() {
    let output = compile_source(
        r#"
type Base[T] = trait { get: fn() T, };
type Derived[T]: Base[T] = trait { add: fn(T) T, };

impl *i32 : Base[i32] {
    pub fn get() i32 { return self.*; }
}

impl *i32 : Derived[i32] {
    pub fn add(v: i32) i32 { return self.* + v; }
}

fn takes_base(x: *Base[i32]) i32 {
    return x.get();
}

extern fn main(args: [][]u8) i32 {
    let v = i32.{3};
    let d = *Derived[i32].{ v.& };
    let b = *Base[i32].{ d };
    return b.get() + takes_base(d) + d.add(5);
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
fn rejects_ambiguous_inherited_trait_methods() {
    let output = compile_source(
        r#"
type A = trait { foo: fn() i32, };
type B = trait { foo: fn() i32, };
type C: A + B = trait {};

impl *i32 : A { pub fn foo() i32 { return self.*; } }
impl *i32 : B { pub fn foo() i32 { return self.* + 10; } }
impl *i32 : C {}

extern fn main(args: [][]u8) i32 {
    let v = i32.{3};
    let c = *C.{ v.& };
    return c.foo();
}
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ambiguous inherited trait method `foo`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn compiles_std_cmp_ord_bound_for_builtin_scalars() {
    let output = build_and_run(
        "kernc_trait_ord_value_bound",
        r#"
use std.cmp.Ord;

fn classify[T](lhs: T, rhs: T) i32
    where T: Ord[T],
{
    match (lhs.cmp(rhs)) {
        -1 => -1,
        0 => 0,
        1 => 1,
        _ => 99,
    }
}

extern fn main(args: [][]u8) i32 {
    let a = i32.{3};
    let b = i32.{7};
    let c = bool.{true};
    let d = bool.{false};
    return classify(a, b) + classify(c, d);
}
"#,
        &["--use-std"],
    );

    assert!(
        output.status.success(),
        "program failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
}

#[test]
fn compiles_std_cmp_ord_bound_for_custom_impls() {
    let output = build_and_run(
        "kernc_trait_custom_ord_value_bound",
        r#"
use std.cmp.{Eq, Ordering, Comparable, Ord, LESS, EQUAL, GREATER};

type Key = struct {
    raw: i32,
    bias: i32,
};

impl Key : Eq[Key] {
    pub fn eq(other: Key) bool {
        return self.raw == other.raw and self.bias == other.bias;
    }
}

impl Key : Comparable[Key] {
    pub fn cmp(other: Key) Ordering {
        let lhs = self.raw + self.bias;
        let rhs = other.raw + other.bias;
        if (lhs < rhs) return LESS;
        if (lhs > rhs) return GREATER;
        return EQUAL;
    }
}

impl Key : Ord[Key] {}

fn classify[T](lhs: T, rhs: T) i32
    where T: Ord[T],
{
    match (lhs.cmp(rhs)) {
        -1 => -1,
        0 => 0,
        1 => 1,
        _ => 99,
    }
}

extern fn main(args: [][]u8) i32 {
    let lhs = Key.{ raw: 3, bias: 4 };
    let rhs = Key.{ raw: 6, bias: 0 };
    if (classify(lhs, rhs) != 1) {
        return 1;
    }
    return 0;
}
"#,
        &["--use-std"],
    );

    assert!(
        output.status.success(),
        "program failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
}

#[test]
fn value_and_pointer_impls_remain_distinct_trait_targets() {
    let output = compile_source(
        r#"
type Marker = trait {
    tag: fn() i32,
};

impl i32 : Marker {
    pub fn tag() i32 {
        return 1;
    }
}

impl *i32 : Marker {
    pub fn tag() i32 {
        return 2;
    }
}

fn value_tag[T](x: T) i32
    where T: Marker,
{
    return x.tag();
}

fn pointer_tag[T](x: *T) i32
    where *T: Marker,
{
    return x.tag();
}

extern fn main(args: [][]u8) i32 {
    let value = i32.{7};
    let _ = value_tag(value);
    let _ = pointer_tag(value.&);
    return 0;
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
