mod support;

use support::{build_and_run, compile_source_tree_with_args, compile_source_with_args};

fn compile_source(source: &str) -> std::process::Output {
    compile_source_with_args("kernc_regression_test", source, &[])
}

fn compile_source_with_std(source: &str) -> std::process::Output {
    compile_source_with_args("kernc_regression_std_test", source, &["--use-std"])
}

fn compile_source_tree(entry: &str, files: &[(&str, &str)]) -> std::process::Output {
    compile_source_tree_with_args("kernc_regression_tree", entry, files, &["-c"])
}

fn build_and_run_source(source: &str) -> std::process::Output {
    build_and_run(
        "kernc_regression_run",
        source,
        &["--link-profile", "hosted"],
    )
}

#[test]
fn compiles_generic_supertrait_method_lookup() {
    let output = compile_source(
        r#"
type Base = trait {
    foo: fn() i32,
};

type Derived[U]: Base = trait {
    add: fn(U) i32,
};

impl *i32 : Base {
    pub fn foo() i32 {
        return self.*;
    }
}

impl *i32 : Derived[i32] {
    pub fn add(v: i32) i32 {
        return self.* + v;
    }
}

fn use_it[T](value: *T) i32
    where *T: Derived[i32],
{
    return value.foo() + value.add(2);
}

extern fn main(args: [][]u8) i32 {
    let value = i32.{5};
    return use_it(value.&);
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
fn compiles_const_enum_and_const_array_usage() {
    let output = compile_source(
        r#"
type Mode: u8 = enum {
    Off,
    On,
};

type Option[T] = enum {
    None,
    Some: T,
};

const TABLE = [_]u8.{ 3, 5, 8 };
const DEFAULT_MODE = Mode.{ On };
const VALUE = Option[i32].{ Some: 7 };

extern fn main(args: [][]u8) i32 {
    let mode = match (DEFAULT_MODE) {
        .Off => i32.{0},
        .On => i32.{10},
    };

    let picked = match (VALUE) {
        .None => i32.{0},
        .Some: v => v,
    };

    return mode + picked + (TABLE.[1] as i32);
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
fn compiles_const_fn_in_global_array_len_and_method_calls() {
    let output = compile_source(
        r#"
type Switch = enum {
    Off = 0,
    On = 1,
    Value: i32,
};

type Pair = struct {
    left: i32,
    right: i32,
};

const fn inc(v: i32) i32 {
    let next = v + 1;
    return next;
}

const fn id[T](value: T) T {
    return value;
}

const fn choose(flag: bool) Switch {
    if (flag) {
        return Switch.{ Value: 7 };
    }
    return Switch.{ On };
}

const fn unwrap_switch(v: Switch) i32 {
    match (v) {
        .Off => 0,
        .On => 1,
        .Value: payload => payload,
    }
}

impl Pair {
    pub const fn sum() i32 {
        let total = self.left + self.right;
        return total;
    }
}

const TABLE = [inc(3)]u8.{ 1, 2, 3, 4 };
const TOTAL = unwrap_switch(choose(true)) + Pair.{ left: 5, right: id[i32](3) }.sum() + (TABLE.[3] as i32);

extern fn main(args: [][]u8) i32 {
    return TOTAL;
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
fn compiles_concrete_slice_impl_methods() {
    let output = compile_source(
        r#"
fn slice_len(value: []u8) usize {
    return #value;
}

impl []u8 {
    pub fn len_via_impl() usize {
        return slice_len(self);
    }
}

extern fn main(args: [][]u8) i32 {
    let text = "hi";
    return text.len_via_impl() as i32;
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
fn compiles_generic_std_helper_calling_layout_of_recursive_type() {
    let output = compile_source_with_std(
        r#"
use std.mem.layout_of;

type Node[K, V] = struct {
    next: *mut Node[K, V],
    key: K,
    value: V,
};

fn free_node[K, V](alloc: *mut std.mem.alloc.Allocator, node: *mut Node[K, V]) void {
    alloc.free(node as *mut u8, layout_of[Node[K, V]]());
}

fn wrap_free[K, V](alloc: *mut std.mem.alloc.Allocator, node: *mut Node[K, V]) void {
    free_node(alloc, node);
}

extern fn main(args: [][]u8) i32 {
    let _ = wrap_free[i32, i32];
    let _ = args;
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
fn prunes_mutually_exclusive_extern_blocks_before_name_collection() {
    let output = compile_source(
        r#"
#[if(arch == "x86_64")]
extern {
    fn system_probe() i32;
}

#[if(arch == "aarch64")]
extern {
    fn system_probe() i32;
}

extern fn main(args: [][]u8) i32 {
    let _ = args;
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
fn runs_captured_closure_boundary_conversions() {
    let output = build_and_run_source(
        r#"
fn use_closure(cb: *Fn() i32) i32 {
    return cb();
}

fn use_mut_closure(cb: *mut Fn() void) void {
    cb();
}

extern fn main() i32 {
    let mut calls = i32.{0};
    let value = use_closure(.[ptr = calls..&]() i32 {
        ptr.* += 1;
        return 77;
    });
    if (value != 77) {
        return 1;
    }
    if (calls != 1) {
        return 2;
    }

    let mut counter = i32.{0};
    let mut closure = .[ptr = counter..&]() void {
        ptr.* += 1;
    };
    use_mut_closure(closure);
    if (counter != 1) {
        return 3;
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
fn compiles_assignment_through_struct_array_fields_only() {
    let output = compile_source(
        r#"
type Buffer = struct {
    items: [4]i32,
};

extern fn main(args: [][]u8) i32 {
    let mut buf = Buffer.{ items: [4]i32.{ 0; 4 } };
    buf.items.[0] = 5;

    let ptr = buf..&;
    ptr.items.[1] = 7;

    return buf.items.[0] + ptr.items.[1];
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
fn runs_array_and_slice_mutability_semantics() {
    let output = build_and_run_source(
        r#"
extern fn main() i32 {
    let arr = [5]mut u8.{ b'a', b'b', b'c', b'd', b'e' };
    arr.[1] = b'x';
    if (arr.[1] != b'x') {
        return 1;
    }

    let view = arr..[1 .. 4];
    view.[0] = b'd';
    view.[1] = b'y';
    view.[2] = b'x';
    if (arr.[1] != b'd') {
        return 2;
    }
    if (arr.[2] != b'y') {
        return 3;
    }
    if (arr.[3] != b'x') {
        return 4;
    }

    let mut whole = [3]u8.{ b'1', b'2', b'3' };
    whole = [3]u8.{ b'4', b'5', b'6' };
    if (whole.[0] != b'4' or whole.[1] != b'5' or whole.[2] != b'6') {
        return 5;
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
fn runs_defer_after_return_value_evaluation() {
    let output = build_and_run_source(
        r#"
type Guard = struct {
    ptr: *mut i32,
};

impl *mut Guard {
    pub fn deinit() void {
        self.ptr.* = 2;
    }
}

fn read_before_defer() i32 {
    let mut state = i32.{1};
    let mut guard = Guard.{ ptr: state..& };
    defer guard..&.deinit();
    return state;
}

extern fn main() i32 {
    if (read_before_defer() != 1) {
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
fn runs_match_arm_block_with_statement_before_return() {
    let output = build_and_run_source(
        r#"
type Result[T, E] = enum {
    Ok: T,
    Err: E,
};

fn fail() Result[i32, i32] {
    return .{ Err: 7 };
}

extern fn main() i32 {
    let _ = match (fail()) {
        .Ok: v => v,
        .Err: _err => {
            let _ = i32.{0};
            return 0;
        },
    };

    return 1;
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
fn runs_for_clauses_with_non_void_init_post_and_body() {
    let output = build_and_run_source(
        r#"
extern fn main() i32 {
    let mut phase = i32.{0};

    for (
        { phase += i32.{2}; i32.{99} };
        phase < i32.{3};
        { phase += i32.{10}; i32.{88} }
    ) {
        phase += i32.{1};
        i32.{77}
    }

    return phase - i32.{13};
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
fn compiles_const_fn_loops_with_assignment_break_and_continue() {
    let output = compile_source(
        r#"
const fn sum_skip(limit: i32) i32 {
    let mut acc = i32.{0};

    for (let mut i = i32.{0}; i < limit; i += i32.{1}) {
        if (i == i32.{2}) {
            continue;
        }
        if (i == i32.{5}) {
            break;
        }
        acc += i;
    }

    return acc;
}

const TOTAL = sum_skip(i32.{7});

extern fn main(args: [][]u8) i32 {
    let _ = args;
    return TOTAL;
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
fn compiles_const_fn_mutating_local_struct_fields_and_array_elements() {
    let output = compile_source(
        r#"
type Pair = struct {
    left: i32,
    right: i32,
};

const fn build_total() i32 {
    let mut pair = Pair.{ left: 1, right: 2 };
    pair.left += 4;
    pair.right = pair.left + pair.right;

    let mut items = [3]mut i32.{ 5, 6, 7 };
    items.[1] = pair.right;
    items.[2] += items.[0];

    return pair.right + items.[1] + items.[2];
}

const TOTAL = build_total();

extern fn main(args: [][]u8) i32 {
    let _ = args;
    return TOTAL;
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
fn rejects_assignment_through_non_mut_array_elements() {
    let output = compile_source(
        r#"
extern fn main(args: [][]u8) i32 {
    let mut arr = [4]i32.{ 0; 4 };
    arr.[0] = 3;
    return arr.[0];
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
        stderr.contains("cannot assign to an immutable variable or location"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_rebinding_immutable_array_binding() {
    let output = compile_source(
        r#"
extern fn main(args: [][]u8) i32 {
    let arr = [3]u8.{ b'a', b'b', b'c' };
    arr = [3]u8.{ b'x', b'y', b'z' };
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
        stderr.contains("cannot assign to an immutable variable or location"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_non_const_fn_in_const_context() {
    let output = compile_source(
        r#"
fn runtime_only(v: i32) i32 {
    return v + 1;
}

const BAD = runtime_only(1);

extern fn main(args: [][]u8) i32 {
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
        stderr.contains("only `const fn` can be called in constant expressions"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_arrays_larger_than_llvm_indexable_limit() {
    let output = compile_source(
        r#"
extern fn main(args: [][]u8) i32 {
    let _ = [4294967296]u8.{ undef };
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
}

#[test]
fn allows_private_named_struct_fields_within_defining_module() {
    let output = compile_source_tree(
        "main.kr",
        &[
            (
                "main.kr",
                r#"
mod data;

extern fn main(args: [][]u8) i32 {
    return data.read_secret();
}
"#,
            ),
            (
                "data.kr",
                r#"
pub type Bag = struct {
    secret: i32,
    pub open: i32,
};

pub fn read_secret() i32 {
    let bag = Bag.{ secret: 5, open: 8 };
    return bag.secret + bag.open;
}
"#,
            ),
        ],
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_private_named_struct_fields_across_modules() {
    let output = compile_source_tree(
        "main.kr",
        &[
            (
                "main.kr",
                r#"
mod data;

extern fn main(args: [][]u8) i32 {
    let bag = data.make();
    return bag.secret + bag.open;
}
"#,
            ),
            (
                "data.kr",
                r#"
pub type Bag = struct {
    secret: i32,
    pub open: i32,
};

pub fn make() Bag {
    return Bag.{ secret: 5, open: 8 };
}
"#,
            ),
        ],
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("field `secret` of type `Bag` is private"),
        "unexpected stderr:\n{}",
        stderr
    );
}
