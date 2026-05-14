use kernc_cli::test_support::{build_and_run, compile_source_with_args};

fn compile_source(source: &str) -> std::process::Output {
    compile_source_with_args("kernc_trait_test", source, &[])
}

#[test]
fn compiles_multi_supertrait_lookup_through_generic_bound() {
    let output = compile_source(
        r#"
trait A { fn a() i32; };
trait B { fn b() i32; };
trait C: A + B { fn c() i32; };

impl &i32 : A { pub fn a() i32 { return self.*; } }
impl &i32 : B { pub fn b() i32 { return self.* + 10; } }
impl &i32 : C { pub fn c() i32 { return self.* + 100; } }

fn use_it[T](x: &T) i32
    where &T: C,
{
    return x.a() + x.b() + x.c();
}

fn main() i32 {
    let v = 1i32;
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
trait Base { fn foo() i32; };
trait Derived: Base { fn bar() i32; };

impl &i32 : Base { pub fn foo() i32 { return self.*; } }
impl &i32 : Derived { pub fn bar() i32 { return self.* + 1; } }

fn main() i32 {
    let v = 3i32;
    let d = (v.& as &Derived);
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
fn compiles_trait_object_from_concrete_pointer_via_as_and_bnc() {
    let output = compile_source(
        r#"
trait Base { fn foo() i32; };

impl &i32 : Base {
    pub fn foo() i32 { return self.*; }
}

fn takes_base(x: &Base) i32 {
    return x.foo();
}

fn main() i32 {
    let v = 3i32;
    let explicit = (v.& as &Base);
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
fn compiles_trait_object_upcast_via_as_and_bnc() {
    let output = compile_source(
        r#"
trait Base { fn foo() i32; };
trait Derived: Base { fn bar() i32; };

impl &i32 : Base { pub fn foo() i32 { return self.*; } }
impl &i32 : Derived { pub fn bar() i32 { return self.* + 1; } }

fn takes_base(x: &Base) i32 {
    return x.foo();
}

fn main() i32 {
    let v = 3i32;
    let d = (v.& as &Derived);
    let b = (d as &Base);
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
trait A { fn a() i32; };
trait B { fn b() i32; };
trait C: A + B { fn c() i32; };

impl &i32 : A { pub fn a() i32 { return self.*; } }
impl &i32 : B { pub fn b() i32 { return self.* + 10; } }
impl &i32 : C { pub fn c() i32 { return self.* + 100; } }

fn takes_a(x: &A) i32 {
    return x.a();
}

fn takes_b(x: &B) i32 {
    return x.b();
}

fn main() i32 {
    let v = 3i32;
    let c = (v.& as &C);
    let a = (c as &A);
    let b = (c as &B);
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
trait Base[T] { fn get() T; };
trait Derived[T]: Base[T] { fn add(_: T) T; };

impl &i32 : Base[i32] {
    pub fn get() i32 { return self.*; }
}

impl &i32 : Derived[i32] {
    pub fn add(v: i32) i32 { return self.* + v; }
}

fn takes_base(x: &Base[i32]) i32 {
    return x.get();
}

fn main() i32 {
    let v = 3i32;
    let d = (v.& as &Derived[i32]);
    let b = (d as &Base[i32]);
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
trait A { fn foo() i32; };
trait B { fn foo() i32; };
trait C: A + B {};

impl &i32 : A { pub fn foo() i32 { return self.*; } }
impl &i32 : B { pub fn foo() i32 { return self.* + 10; } }
impl &i32 : C {}

fn main() i32 {
    let v = 3i32;
    let c = (v.& as &C);
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
fn rejects_ambiguous_inherited_trait_methods_from_same_parent_trait_with_different_args() {
    let output = compile_source(
        r#"
trait Base[T] { fn get() T; };
trait Left: Base[i32] {};
trait Right: Base[bool] {};
trait Both: Left + Right {};

impl &i32 : Base[i32] { fn get() i32 { return self.*; } }
impl &i32 : Base[bool] { fn get() bool { return true; } }
impl &i32 : Left {}
impl &i32 : Right {}
impl &i32 : Both {}

fn main() i32 {
    let v = 3i32;
    let both = (v.& as &Both);
    return both.get();
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
        stderr.contains("ambiguous inherited trait method `get`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("Base[i32]") && stderr.contains("Base[bool]"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_duplicate_associated_type_definitions_in_trait() {
    let output = compile_source(
        r#"
trait Factory {
    type Out;
    type Out;
};

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
        stderr.contains("the associated type `Out` is defined multiple times"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("defined only once in the same trait or impl"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_duplicate_associated_type_definitions_in_impl() {
    let output = compile_source(
        r#"
trait Factory {
    type Out;
};

struct X {};

impl X: Factory {
    type Out = i32;
    type Out = i64;
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
        stderr.contains("the associated type `Out` is defined multiple times"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("defined only once in the same trait or impl"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_ambiguous_inherited_trait_methods_from_same_parent_trait_in_generic_bound_lookup() {
    let output = compile_source(
        r#"
trait Base[T] { fn get() T; };
trait Left: Base[i32] {};
trait Right: Base[bool] {};
trait Both: Left + Right {};

impl &i32 : Base[i32] { fn get() i32 { return self.*; } }
impl &i32 : Base[bool] { fn get() bool { return true; } }
impl &i32 : Left {}
impl &i32 : Right {}
impl &i32 : Both {}

fn use_it[T](x: &T) i32
    where &T: Both,
{
    return x.get();
}

fn main() i32 {
    let v = 3i32;
    return use_it(v.&);
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
        stderr.contains("ambiguous inherited trait method `get`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("Base[i32]") && stderr.contains("Base[bool]"),
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
    let a = 3i32;
    let b = 7i32;
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
trait Bump[Rhs] {
    type Out;
    fn bump(_: Rhs) Out;
};

struct Vec2 {
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
trait Trivial {
    fn f() i32;
};

trait NeedsBound {
    type Out: Trivial;
    fn make() Out;
};

struct Bad {};

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
trait Bump[Rhs] {
    type Out;
    fn bump(_: Rhs) Out;
};

struct Vec2 {
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
trait Trivial {
    fn f() i32;
};

trait NeedsBound {
    type Out: Trivial;
    fn make() Out;
};

struct Bad {};

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
trait Trivial {
    fn f() i32;
};

trait NeedsBound {
    type Out: Trivial;
    fn make() Out;
};

struct Good {};

impl Good: Trivial {
    fn f() i32 {
        return 7;
    }
}

struct Holder[T] {
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

struct Key {
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
trait Marker {
    fn tag() i32;
};

impl i32 : Marker {
    pub fn tag() i32 {
        return 1;
    }
}

impl &i32 : Marker {
    pub fn tag() i32 {
        return 2;
    }
}

fn value_tag[T](x: T) i32
    where T: Marker,
{
    return x.tag();
}

fn pointer_tag[T](x: &T) i32
    where &T: Marker,
{
    return x.tag();
}

fn main() i32 {
    let value = 7i32;
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
enum Mode {
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
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = gpa().on(page)..&;
    let text = String.{}..&;
    defer text.deinit(gpa);
    if (text.try_push_str(gpa, "kern").is_err()) {
        return 1;
    }

    if (!(text == "kern")) {
        return 2;
    }
    if (!("kern" == text)) {
        return 3;
    }
    if (text != "kern") {
        return 4;
    }
    if ("lang" == text) {
        return 5;
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
fn runs_slice_array_eq_operator_impls() {
    let output = build_and_run(
        "kernc_slice_array_eq_operator_impls",
        r#"
fn main() i32 {
    let array = [4]i32.{1, 2, 3, 4};
    let slice = array.&[0...4];

    if (!(slice == [4]i32.{1, 2, 3, 4})) {
        return 1;
    }
    if (!([4]i32.{1, 2, 3, 4} == slice)) {
        return 2;
    }
    if (slice != [4]i32.{1, 2, 3, 4}) {
        return 3;
    }
    if (slice == [3]i32.{1, 2, 3}) {
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
fn runs_match_value_patterns_through_pattern_trait() {
    let output = build_and_run(
        "kernc_match_value_patterns_pattern_trait",
        r#"
struct Key {
    raw: i32,
    bias: i32,
};

impl Key : Eq[Key] {
    pub fn eq(other: Key) bool {
        return (self.raw + self.bias) == (other.raw + other.bias);
    }
}

struct KeySum {
    value: i32,
};

impl KeySum : Pattern[Key] {
    type Bind = void;

    pub fn apply(value: Key) ?Bind {
        if ((value.raw + value.bias) == self.value) {
            return .{ Some: {} };
        }
        return .None;
    }
}

fn classify(key: Key) i32 {
    return match (key) {
        KeySum.{ value: 3 } => 3,
        KeySum.{ value: 9 } => 9,
        _ => 0,
    };
}

fn make_key(raw: i32, bias: i32) Key {
    return Key.{ raw: raw, bias: bias };
}

fn main() i32 {
    if (classify(Key.{ raw: 2, bias: 1 }) != 3) {
        return 1;
    }
    if (classify(Key.{ raw: 8, bias: 1 }) != 9) {
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
fn rejects_match_value_pattern_even_when_eq_impl_exists() {
    let output = compile_source(
        r#"
struct Key {
    raw: i32,
    bias: i32,
};

impl Key : Eq[Key] {
    pub fn eq(other: Key) bool {
        return (self.raw + self.bias) == (other.raw + other.bias);
    }
}

fn classify(key: Key) i32 {
    return match (key) {
        make_key(1, 2) => 1,
        _ => 0,
    };
}

fn make_key(raw: i32, bias: i32) Key {
    return Key.{ raw: raw, bias: bias };
}

fn main() i32 {
    return classify(make_key(1, 2));
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
        stderr.contains("match value is not a valid pattern"),
        "{stderr}"
    );
    assert!(stderr.contains("Pattern[Key]"), "{stderr}");
}

#[test]
fn rejects_match_value_pattern_without_pattern_impl() {
    let output = compile_source(
        r#"
struct Token {
    id: i32,
};

fn classify(token: Token) i32 {
    return match (token) {
        make_token(1) => 1,
        _ => 0,
    };
}

fn make_token(id: i32) Token {
    return Token.{ id: id };
}

fn main() i32 {
    return classify(make_token(1));
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
        stderr.contains("match value is not a valid pattern"),
        "{stderr}"
    );
    assert!(stderr.contains("Pattern[Token]"), "{stderr}");
}

#[test]
fn runs_slice_array_eq_method_impls() {
    let output = build_and_run(
        "kernc_slice_array_eq_method_impls",
        r#"
fn main() i32 {
    let array = [4]i32.{1, 2, 3, 4};
    let slice = array.&[0...4];

    if (!slice.eq([4]i32.{1, 2, 3, 4})) {
        return 1;
    }
    if (slice.eq([3]i32.{1, 2, 3})) {
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
fn runs_eq_methods_inside_test_assertion_chains() {
    let output = build_and_run(
        "kernc_eq_methods_inside_test_assertion_chains",
        r#"
use base.coll.{list, string};
use base.mem.alloc.gpa;
use base.test.{report};
use std.io;
use std.mem.page;

fn main() i32 {
    let t = report(io.stderr())..&;
    let page = page()..&;
    let gpa = gpa().on(page)..&;

    let list = list[i32]()..&;
    defer list.deinit(gpa);
    list.try_push(gpa, 1).is_ok().should().sum(@loc(), t);
    list.try_push(gpa, 2).is_ok().should().sum(@loc(), t);
    list.try_push(gpa, 3).is_ok().should().sum(@loc(), t);
    list.as_slice().eq([3]i32.{ 1, 2, 3 }).should().sum(@loc(), t);

    let text = string()..&;
    defer text.deinit(gpa);
    text.try_push_str(gpa, "Hello").is_ok().should().sum(@loc(), t);
    text.try_push_str(gpa, ", ").is_ok().should().sum(@loc(), t);
    text.try_push_str(gpa, "Kern").is_ok().should().sum(@loc(), t);
    text.as_str().eq("Hello, Kern").should().sum(@loc(), t);

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
fn runs_argument_inferred_generic_trait_method_impl() {
    let output = build_and_run(
        "kernc_argument_inferred_generic_trait_method_impl",
        r#"
trait Fits[Rhs] {
    fn fits(_: Rhs) bool;
};

impl[T, N: usize] &[T] : Fits[[N]T] {
    pub fn fits(other: [N]T) bool {
        return self.@len() == N;
    }
}

fn main() i32 {
    let array = [4]i32.{1, 2, 3, 4};
    let slice = array.&[0...4];

    if (!slice.fits([4]i32.{1, 2, 3, 4})) {
        return 1;
    }
    if (slice.fits([3]i32.{1, 2, 3})) {
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

#[test]
fn runs_argument_inferred_trait_method_with_associated_return() {
    let output = build_and_run(
        "kernc_argument_inferred_trait_method_assoc_return",
        r#"
trait Score {
    fn score() i32;
};

struct Wrap[N: usize] {
    value: i32,
};

impl[N: usize] Wrap[N] : Score {
    pub fn score() i32 {
        return self.value;
    }
}

trait Make[Rhs] {
    type Out: Score;
    fn make(_: Rhs) Out;
};

impl[T, N: usize] &[T] : Make[[N]T] {
    type Out = Wrap[N];

    pub fn make(other: [N]T) Out {
        return Wrap[N].{ value: (self.@len() + N) as i32 };
    }
}

fn main() i32 {
    let array = [4]i32.{1, 2, 3, 4};
    let slice = array.&[0...4];
    return slice.make([4]i32.{1, 2, 3, 4}).score() - 8;
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

#[test]
fn runs_argument_inferred_method_with_supertrait_bound() {
    let output = build_and_run(
        "kernc_argument_inferred_method_supertrait_bound",
        r#"
trait Parent[Rhs] {
    fn parent(_: Rhs) i32;
};

trait Child[Rhs]: Parent[Rhs] {};

impl[T, N: usize] &[T] : Parent[[N]T] {
    pub fn parent(other: [N]T) i32 {
        return (self.@len() + N) as i32;
    }
}

impl[T, N: usize] &[T] : Child[[N]T] {}

fn use_child[S, T, N: usize](value: S, arg: [N]T) i32
    where S: Child[[N]T],
{
    return value.parent(arg);
}

fn main() i32 {
    let array = [4]i32.{1, 2, 3, 4};
    let slice = array.&[0...4];
    return use_child[&[i32], i32, 4](slice, [4]i32.{1, 2, 3, 4}) - 8;
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

#[test]
fn runs_argument_inferred_method_after_where_bound_substitution() {
    let output = build_and_run(
        "kernc_argument_inferred_method_where_bound",
        r#"
trait Allowed {
    fn marker() i32;
};

struct Gate[Rhs] {
    value: Rhs,
};

impl Gate[[4]i32] : Allowed {
    pub fn marker() i32 {
        return 4;
    }
}

trait Checked[Rhs] {
    fn checked(_: Rhs) i32;
};

impl[T, N: usize] &[T] : Checked[[N]T]
    where Gate[[N]T]: Allowed,
{
    pub fn checked(other: [N]T) i32 {
        return Gate[[N]T].{ value: other }.marker();
    }
}

fn main() i32 {
    let array = [4]i32.{1, 2, 3, 4};
    let slice = array.&[0...4];
    return slice.checked([4]i32.{1, 2, 3, 4}) - 4;
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

#[test]
fn rejects_argument_inferred_method_when_where_bound_is_unsatisfied() {
    let output = compile_source(
        r#"
trait Allowed {
    fn marker() i32;
};

struct Gate[Rhs] {
    value: Rhs,
};

impl Gate[[4]i32] : Allowed {
    pub fn marker() i32 {
        return 4;
    }
}

trait Checked[Rhs] {
    fn checked(_: Rhs) i32;
};

impl[T, N: usize] &[T] : Checked[[N]T]
    where Gate[[N]T]: Allowed,
{
    pub fn checked(other: [N]T) i32 {
        return Gate[[N]T].{ value: other }.marker();
    }
}

fn main() i32 {
    let array = [4]i32.{1, 2, 3, 4};
    let slice = array.&[0...4];
    return slice.checked([3]i32.{1, 2, 3});
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
        stderr.contains("no field or method named `checked`")
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
fn rejects_argument_inferred_method_when_receiver_const_arg_disagrees() {
    let output = compile_source(
        r#"
struct Box[N: usize] {};

trait Use[Rhs] {
    fn use_it(_: Rhs) i32;
};

impl[N: usize] Box[N] : Use[[N]i32] {
    pub fn use_it(other: [N]i32) i32 {
        return N as i32;
    }
}

fn main() i32 {
    let value = Box[3].{};
    return value.use_it([4]i32.{1, 2, 3, 4});
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
        stderr.contains("mismatched types") || stderr.contains("no field or method named `use_it`"),
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
fn runs_argument_inferred_method_with_associated_where_equality() {
    let output = build_and_run(
        "kernc_argument_inferred_method_assoc_where_equality",
        r#"
trait HasOut {
    type Out;
};

struct Gate[N: usize] {};

impl Gate[4] : HasOut {
    type Out = i32;
}

impl Gate[3] : HasOut {
    type Out = bool;
}

trait Checked[Rhs] {
    fn checked(_: Rhs) i32;
};

impl[T, N: usize] &[T] : Checked[[N]T]
    where Gate[N]: HasOut[Out = i32],
{
    pub fn checked(other: [N]T) i32 {
        let _ = other;
        return N as i32;
    }
}

fn main() i32 {
    let array = [4]i32.{1, 2, 3, 4};
    let slice = array.&[0...4];
    return slice.checked([4]i32.{1, 2, 3, 4}) - 4;
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

#[test]
fn rejects_argument_inferred_method_when_associated_where_equality_is_unsatisfied() {
    let output = compile_source(
        r#"
trait HasOut {
    type Out;
};

struct Gate[N: usize] {};

impl Gate[4] : HasOut {
    type Out = i32;
}

impl Gate[3] : HasOut {
    type Out = bool;
}

trait Checked[Rhs] {
    fn checked(_: Rhs) i32;
};

impl[T, N: usize] &[T] : Checked[[N]T]
    where Gate[N]: HasOut[Out = i32],
{
    pub fn checked(other: [N]T) i32 {
        let _ = other;
        return N as i32;
    }
}

fn main() i32 {
    let array = [4]i32.{1, 2, 3, 4};
    let slice = array.&[0...4];
    return slice.checked([3]i32.{1, 2, 3});
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
        stderr.contains("no field or method named `checked`")
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
fn rejects_ambiguous_argument_inferred_method_candidates() {
    let output = compile_source(
        r#"
trait Use[Rhs] {
    fn pick(_: Rhs) i32;
};

trait AlsoUse[Rhs] {
    fn pick(_: Rhs) i32;
};

impl[T, N: usize] &[T] : Use[[N]T] {
    pub fn pick(other: [N]T) i32 {
        let _ = other;
        return 1;
    }
}

impl[T, N: usize] &[T] : AlsoUse[[N]T] {
    pub fn pick(other: [N]T) i32 {
        let _ = other;
        return 2;
    }
}

fn main() i32 {
    let array = [4]i32.{1, 2, 3, 4};
    let slice = array.&[0...4];
    return slice.pick([4]i32.{1, 2, 3, 4});
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
        stderr.contains("ambiguous impl method `pick`"),
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
fn argument_inferred_method_lookup_does_not_steal_callable_fields() {
    let output = build_and_run(
        "kernc_argument_inferred_method_callable_field",
        r#"
struct Other {};

impl Other {
    pub fn run(value: i32) i32 {
        return value;
    }
}

struct Runner {
    run: &fn([2]i32) i32,
};

fn sum(values: [2]i32) i32 {
    return values.[0] + values.[1];
}

fn main() i32 {
    let runner = Runner.{ run: sum };
    return (runner.run)(.{20, 22}) - 42;
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
    let count = count_bits(240u32);
    if (count != 4u32) {
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
impl &u8 : Integer {}

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
fn compiles_builtin_slice_bounds_marker_for_range_types() {
    let output = compile_source(
        r#"
fn accept[T](bounds: T) void
    where T: SliceBounds,
{
    let _ = bounds;
}

fn main() i32 {
    accept(0usize...4usize);
    accept(0usize...);
    accept(...4usize);
    accept(...);
    return 0;
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
fn rejects_signed_range_for_slice_bounds_marker() {
    let output = compile_source(
        r#"
fn accept[T](bounds: T) void
    where T: SliceBounds,
{
    let _ = bounds;
}

fn main() i32 {
    accept(-1...5);
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
        stderr.contains("SliceBounds"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_explicit_impl_of_builtin_slice_bounds_marker_trait() {
    let output = compile_source(
        r#"
impl i32 : SliceBounds {}

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
        stderr.contains(
            "builtin slice-bounds marker trait `SliceBounds` cannot be implemented explicitly"
        ),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_pointer_popcount_even_if_integer_marker_impl_is_attempted() {
    let output = compile_source(
        r#"
impl &u8 : Integer {}

fn main() i32 {
    let p = 0 as &u8;
    let _ = @popCount[&u8](p);
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
    if (signed_id(7i32) != 7i32) {
        return 1;
    }
    if (unsigned_id(9u32) != 9u32) {
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
    let _ = signed_id(1u32);
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
fn mark(counter: &mut i32, value: bool) bool {
    counter.* += 1;
    return value;
}

fn main() i32 {
    let mut calls = 0i32;

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
struct Vec2 {
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
trait Bump[Rhs] {
    type Out;
    fn bump(_: Rhs) Out;
};

struct Vec2 {
    x: i32,
    y: i32,
};

impl Vec2 : Bump[i32] {
    type Out = Vec2;

    fn bump(rhs: i32) Vec2 {
        return Vec2.{ x: self.x + rhs, y: self.y + rhs };
    }
}

fn plus_one[T](value: T) T.Bump[i32].Out
    where T: Bump[i32],
{
    return value.bump(1i32);
}

fn main() i32 {
    let out = plus_one(Vec2.{ x: 2, y: 5 });
    if (out.x != 3) {
        return 1;
    }
    if (out.y != 6) {
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
