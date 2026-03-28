use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn unique_temp_path(prefix: &str, extension: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let file_name = format!("{}_{}_{}.{}", prefix, std::process::id(), nanos, extension);
    std::env::temp_dir().join(file_name)
}

fn run_kernc(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_kernc"))
        .current_dir(repo_root())
        .args(args)
        .output()
        .unwrap()
}

fn compile_source(source: &str) -> std::process::Output {
    let source_path = unique_temp_path("kernc_trait_test", "kr");
    let object_path = unique_temp_path("kernc_trait_test", "o");
    fs::write(&source_path, source).unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let object_arg = object_path.to_string_lossy().into_owned();
    let args = vec!["-c", source_arg.as_str(), "-o", object_arg.as_str()];
    let output = run_kernc(&args);

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&object_path);
    output
}

fn compile_source_with_std(source: &str) -> std::process::Output {
    let source_path = unique_temp_path("kernc_trait_test_std", "kr");
    let object_path = unique_temp_path("kernc_trait_test_std", "o");
    fs::write(&source_path, source).unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let object_arg = object_path.to_string_lossy().into_owned();
    let args = vec![
        "-c",
        "--use-std",
        source_arg.as_str(),
        "-o",
        object_arg.as_str(),
    ];
    let output = run_kernc(&args);

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&object_path);
    output
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
    let output = compile_source_with_std(
        r#"
use std.cmp.Ord;

fn classify[T](lhs: *T, rhs: T) i32
    where *T: Ord[T],
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
    return classify(a.&, b) + classify(c.&, d);
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
fn compiles_std_cmp_ord_bound_for_custom_impls() {
    let output = compile_source_with_std(
        r#"
use std.cmp.{Eq, Ordering, Comparable, Ord, LESS, EQUAL, GREATER};

type Key = struct {
    raw: i32,
    bias: i32,
};

impl *Key : Eq[Key] {
    pub fn eq(other: Key) bool {
        return self.raw == other.raw and self.bias == other.bias;
    }
}

impl *Key : Comparable[Key] {
    pub fn cmp(other: Key) Ordering {
        let lhs = self.raw + self.bias;
        let rhs = other.raw + other.bias;
        if (lhs < rhs) return LESS;
        if (lhs > rhs) return GREATER;
        return EQUAL;
    }
}

impl *Key : Ord[Key] {}

fn classify[T](lhs: *T, rhs: T) i32
    where *T: Ord[T],
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
    return classify(lhs.&, rhs);
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
