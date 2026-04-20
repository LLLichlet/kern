use kernc_cli::test_support::{build_and_run, compile_source_with_args};

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

fn main() i32 {
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

fn main() i32 {
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

fn main() i32 {
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

fn main() i32 {
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

fn main() i32 {
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

fn main() i32 {
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

fn main() i32 {
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
use base.cmp.Ord;

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

fn main() i32 {
    let a = i32.{3};
    let b = i32.{7};
    let c = bool.{true};
    let d = bool.{false};
    return classify(a, b) + classify(c, d);
}
"#,
        &["--library-bundle", "std"],
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
fn compiles_trait_impls_with_concrete_associated_types() {
    let output = build_and_run(
        "kernc_trait_assoc_concrete",
        r#"
type Bump[Rhs] = trait {
    type Out;
};

type Vec2 = struct {
    x: i32,
    y: i32,
};

impl Vec2: Add[i32] {
    type Out = Vec2;

    fn add(rhs: i32) Out {
        return Vec2.{ x: self.x + rhs, y: self.y + rhs };
    }
}

fn main() i32 {
    let v = Vec2.{ x: 3, y: 4 };
    let out = v.add(5);
    if (out.x + out.y != 17) {
        return 1;
    }
    return 0;
}
"#,
        &[],
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
}

#[test]
fn rejects_impl_associated_types_that_repeat_trait_bounds() {
    let output = compile_source(
        r#"
type Trivial = trait {
    f: fn() i32,
};

type NeedsBound = trait {
    type Out: Trivial;
    make: fn() Out,
};

type Bad = struct {};

impl Bad: NeedsBound {
    type Out: Trivial = Bad;

    fn make() Out {
        return Bad.{};
    }
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
        stderr.contains("associated type `Out` in an impl cannot declare trait bounds"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("write `type Out = ConcreteType;` in the impl"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("declare the contract on the trait instead"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_trait_impls_missing_required_associated_types() {
    let output = compile_source(
        r#"
type Bump[Rhs] = trait {
    type Out;
    bump: fn(Rhs) Out,
};

type Vec2 = struct {
    x: i32,
    y: i32,
};

impl Vec2: Add[i32] {
    fn add(rhs: i32) i32 {
        return self.x + self.y + rhs;
    }
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
        stderr.contains("missing associated type definition `Out` in impl"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_impl_associated_type_targets_that_miss_trait_bounds() {
    let output = compile_source(
        r#"
type Trivial = trait {
    f: fn() i32,
};

type NeedsBound = trait {
    type Out: Trivial;
    make: fn() Out,
};

type Bad = struct {};

impl Bad: NeedsBound {
    type Out = i32;

    fn make() Out {
        return 1;
    }
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
        stderr.contains("associated type `Out` does not satisfy the bounds declared by the trait"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("required bound: `i32: Trivial`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn compiles_impl_associated_type_targets_proved_by_impl_where_bounds() {
    let output = build_and_run(
        "kernc_trait_assoc_bound_from_impl_where",
        r#"
type Trivial = trait {
    f: fn() i32,
};

type NeedsBound = trait {
    type Out: Trivial;
    make: fn() Out,
};

type Good = struct {};

impl Good: Trivial {
    fn f() i32 {
        return 7;
    }
}

type Holder[T] = struct {
    value: T,
};

impl[T] Holder[T]: NeedsBound
    where T: Trivial,
{
    type Out = T;

    fn make() Out {
        return self.value;
    }
}

fn main() i32 {
    let holder = Holder[Good].{ value: Good.{} };
    return holder.make().f() - 7;
}
"#,
        &[],
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
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
use base.cmp.{Ordering, Comparable, Ord, LESS, EQUAL, GREATER};

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

fn main() i32 {
    let lhs = Key.{ raw: 3, bias: 4 };
    let rhs = Key.{ raw: 6, bias: 0 };
    if (classify(lhs, rhs) != 1) {
        return 1;
    }
    return 0;
}
"#,
        &["--library-bundle", "std"],
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

fn main() i32 {
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

#[test]
fn compiles_generic_builtin_eq_operator_bound() {
    let output = build_and_run(
        "kernc_builtin_eq_operator_bound",
        r#"
type Mode = enum {
    Fast,
    Slow,
};

fn same[T](lhs: T, rhs: T) bool
    where T: Eq[T],
{
    return lhs == rhs;
}

fn main() i32 {
    if (!same(Mode.Fast, Mode.Fast)) {
        return 1;
    }
    if (same(Mode.Fast, Mode.Slow)) {
        return 2;
    }
    return 0;
}
"#,
        &["--library-bundle", "std"],
    );

    assert!(
        output.status.success(),
        "program failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn runs_string_slice_eq_operator_impls() {
    let output = build_and_run(
        "kernc_string_slice_eq_operator_impls",
        r#"
use base.coll.String;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    let text = String.{}..&;
    defer text.deinit(gpa);
    let _ = text.push_str(gpa, "kern");

    if (!(text == "kern")) {
        return 1;
    }
    if (!("kern" == text)) {
        return 2;
    }
    if (text != "kern") {
        return 3;
    }
    if ("lang" == text) {
        return 4;
    }
    return 0;
}
"#,
        &["--library-bundle", "std"],
    );

    assert!(
        output.status.success(),
        "program failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_generic_integer_marker_bound_for_bit_intrinsic() {
    let output = build_and_run(
        "kernc_integer_marker_bound",
        r#"
fn count_bits[T](value: T) T
    where T: Integer,
{
    return @popCount(value);
}

fn main() i32 {
    let count = count_bits(u32.{240});
    if (count != u32.{4}) {
        return 1;
    }
    return 0;
}
"#,
        &["--library-bundle", "std"],
    );

    assert!(
        output.status.success(),
        "program failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_generic_bit_intrinsic_without_integer_bound() {
    let output = compile_source(
        r#"
fn count_bits[T](value: T) T {
    return @popCount(value);
}

fn main() i32 {
    return 0;
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
    assert!(stderr.contains("Integer"), "unexpected stderr:\n{}", stderr);
}

#[test]
fn rejects_explicit_impl_of_builtin_integer_marker_trait() {
    let output = compile_source(
        r#"
impl *u8 : Integer {}

fn main() i32 {
    return 0;
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
        stderr.contains("builtin numeric marker trait `Integer` cannot be implemented explicitly"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_pointer_popcount_even_if_integer_marker_impl_is_attempted() {
    let output = compile_source(
        r#"
impl *u8 : Integer {}

fn main() i32 {
    let p = 0 as *u8;
    let _ = @popCount[*u8](p);
    return 0;
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
        stderr.contains("builtin numeric marker trait `Integer` cannot be implemented explicitly"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("LLVM IR Verification Failed"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("Invalid LLVM IR generated"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_using_float_marker_as_operator_capability() {
    let output = compile_source(
        r#"
fn add_pair[T](lhs: T, rhs: T) T
    where T: Float,
{
    return lhs + rhs;
}

fn main() i32 {
    return 0;
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
        stderr.contains("Add[T, Out = T]"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn compiles_signed_and_unsigned_integer_marker_bounds() {
    let output = build_and_run(
        "kernc_signed_unsigned_markers",
        r#"
fn signed_id[T](value: T) T
    where T: SignedInteger,
{
    return value;
}

fn unsigned_id[T](value: T) T
    where T: UnsignedInteger,
{
    return value;
}

fn main() i32 {
    if (signed_id(i32.{7}) != i32.{7}) {
        return 1;
    }
    if (unsigned_id(u32.{9}) != u32.{9}) {
        return 2;
    }
    return 0;
}
"#,
        &["--library-bundle", "std"],
    );

    assert!(
        output.status.success(),
        "program failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_unsigned_type_for_signed_integer_marker() {
    let output = compile_source(
        r#"
fn signed_id[T](value: T) T
    where T: SignedInteger,
{
    return value;
}

fn main() i32 {
    let _ = signed_id(u32.{1});
    return 0;
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
        stderr.contains("SignedInteger"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn signed_integer_marker_does_not_replace_neg_trait_bound() {
    let output = compile_source(
        r#"
fn negate[T](value: T) T
    where T: SignedInteger,
{
    return -value;
}

fn main() i32 {
    return 0;
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
        stderr.contains("Neg[Out = T]"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn logical_operators_remain_short_circuit_control_flow() {
    let output = build_and_run(
        "kernc_logical_short_circuit",
        r#"
fn mark(counter: *mut i32, value: bool) bool {
    counter.* += 1;
    return value;
}

fn main() i32 {
    let mut calls = i32.{0};

    if (false and mark(calls..&, true)) {
        return 1;
    }
    if (calls != 0) {
        return 2;
    }

    if (!(true or mark(calls..&, false))) {
        return 3;
    }
    if (calls != 0) {
        return 4;
    }

    if (!(true and mark(calls..&, true))) {
        return 5;
    }
    if (calls != 1) {
        return 6;
    }

    if (!(false or mark(calls..&, true))) {
        return 7;
    }
    if (calls != 2) {
        return 8;
    }

    return 0;
}
"#,
        &["--library-bundle", "std"],
    );

    assert!(
        output.status.success(),
        "program failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_generic_builtin_eq_operator_without_bound() {
    let output = compile_source(
        r#"
fn same[T](lhs: T, rhs: T) bool {
    return lhs == rhs;
}

fn main() i32 {
    return 0;
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
    assert!(stderr.contains("Eq[T]"), "unexpected stderr:\n{}", stderr);
}

#[test]
fn runs_custom_builtin_add_operator_impl() {
    let output = build_and_run(
        "kernc_builtin_add_operator_impl",
        r#"
type Vec2 = struct {
    x: i32,
    y: i32,
};

impl Vec2 : Add[Vec2] {
    type Out = Vec2;

    pub fn add(other: Vec2) Vec2 {
        return Vec2.{ x: self.x + other.x, y: self.y + other.y };
    }
}

fn plus[T](lhs: T, rhs: T) T
    where T: Add[T, Out = T],
{
    return lhs + rhs;
}

fn main() i32 {
    let sum = plus(Vec2.{ x: 1, y: 2 }, Vec2.{ x: 3, y: 4 });
    if (sum.x != 4) {
        return 1;
    }
    if (sum.y != 6) {
        return 2;
    }
    return 0;
}
"#,
        &["--library-bundle", "std"],
    );

    assert!(
        output.status.success(),
        "program failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_projection_return_types_from_generic_trait_bounds() {
    let output = build_and_run(
        "kernc_trait_projection_return_type",
        r#"
type Bump[Rhs] = trait {
    type Out;
    bump: fn(Rhs) Out,
};

type Vec2 = struct {
    x: i32,
    y: i32,
};

impl Vec2 : Bump[i32] {
    type Out = Vec2;
}

fn keep_projection[T](value: T.Bump[i32].Out) T.Bump[i32].Out
    where T: Bump[i32],
{
    return value;
}

fn main() i32 {
    let out = keep_projection[Vec2](Vec2.{ x: 2, y: 5 });
    if (out.x != 2) {
        return 1;
    }
    if (out.y != 5) {
        return 2;
    }
    return 0;
}
"#,
        &[],
    );

    assert!(
        output.status.success(),
        "program failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
