use super::*;

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
const DEFAULT_MODE = Mode.On;
const VALUE = Option[i32].{ Some: 7 };

fn main() i32 {
    let mode = match (DEFAULT_MODE) {
        .Off => i32.{0},
        .On => i32.{10},
    };

    let picked = match (VALUE) {
        .None => i32.{0},
        .{ Some: v } => v,
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
fn compiles_type_qualified_payloadless_enum_variants_in_const_and_runtime_contexts() {
    let output = build_and_run_source(
        r#"
type DocumentKind = enum {
    KeyValue,
    Table,
};

type Option[T] = enum {
    None,
    Some: T,
};

const DEFAULT_KIND = DocumentKind.KeyValue;
const EMPTY = Option[i32].None;

const fn score(kind: DocumentKind, value: Option[i32]) i32 {
    let kind_score = match (kind) {
        .KeyValue => i32.{11},
        .Table => i32.{17},
    };

    let value_score = match (value) {
        .None => i32.{5},
        .{ Some: inner } => inner,
    };

    return kind_score + value_score;
}

const TOTAL = score(DocumentKind.KeyValue, Option[i32].None);

fn passthrough(value: Option[i32]) Option[i32] {
    return value;
}

fn main() i32 {
    let contextual = passthrough(.None);
    let some = Option[i32].{ Some: 19 };

    let base = score(DEFAULT_KIND, EMPTY);
    let contextual_score = match (contextual) {
        .None => i32.{3},
        .{ Some: _ } => i32.{100},
    };
    let some_score = match (some) {
        .None => i32.{100},
        .{ Some: inner } => inner,
    };

    return TOTAL + base + contextual_score + some_score;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(54),
        "hosted regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn contextual_integer_literals_follow_small_integer_comparison_operands() {
    let output = build_and_run_source(
        r#"
fn validate(count: u8) i32 {
    if (count == 0 or count > 64) {
        return 1;
    }
    return 0;
}

fn main() i32 {
    let valid = validate(u8.{8});
    let zero = validate(u8.{0});
    let large = validate(u8.{65});
    return valid + (zero * 10) + (large * 100);
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(110),
        "contextual small-int comparison regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn match_value_patterns_accept_type_qualified_scalar_literals() {
    let output = build_and_run_source(
        r#"
fn classify(byte: u8) i32 {
    return match (byte) {
        u8.{4} => 40,
        u8.{21} => 21,
        _ => 0,
    };
}

fn main() i32 {
    return classify(u8.{4}) + classify(u8.{21}) + classify(u8.{9});
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(61),
        "typed scalar match-pattern regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn allows_same_block_shadowing_to_create_a_mutable_working_copy() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let value = i32.{5};
    let mut value = value;
    value = 9;
    return value;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(9),
        "hosted regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn preserves_outer_binding_in_shadowing_initializer() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let value = i32.{5};
    let value = value + 7;
    return value;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(12),
        "hosted regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn lowers_void_aggregate_initializers_without_ice() {
    let output = build_and_run_source(
        r#"
type Result[T, E] = enum {
    Ok: T,
    Err: E,
};

fn explicit_void() Result[void, i32] {
    return .{ Ok: void.{} };
}

fn contextual_void() Result[void, i32] {
    return .{ Ok: .{} };
}

fn main() i32 {
    let first = match (explicit_void()) {
        .{ Ok: _ } => i32.{0},
        .{ Err: code } => code,
    };
    let second = match (contextual_void()) {
        .{ Ok: _ } => i32.{0},
        .{ Err: code } => code,
    };
    return first + second;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "hosted regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_type_qualified_payload_variant_without_braces() {
    let output = compile_source(
        r#"
type Option[T] = enum {
    None,
    Some: T,
};

fn main() i32 {
    let value = Option[i32].Some;
    return match (value) {
        .None => 0,
        .{ Some: inner } => inner,
    };
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
        stderr.contains("variant `Some` requires a payload"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn compiles_if_expression_returning_type_qualified_payloadless_variants() {
    let output = build_and_run_source(
        r#"
type DocumentKind = enum {
    KeyValue,
    Table,
};

fn main() i32 {
    let kind = if (true) {
        DocumentKind.Table
    } else {
        DocumentKind.KeyValue
    };

    return match (kind) {
        .KeyValue => 1,
        .Table => 0,
    };
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "hosted regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_imported_type_alias_payloadless_variants_in_if_expressions() {
    let output = compile_source_tree(
        "main.rn",
        &[
            (
                "main.rn",
                r#"
mod kinds;
use .kinds.DocumentKind;

fn main() i32 {
    let kind = if (true) {
        DocumentKind.Table
    } else {
        DocumentKind.KeyValue
    };

    return match (kind) {
        .KeyValue => 1,
        .Table => 0,
    };
}
"#,
            ),
            (
                "kinds.rn",
                r#"
pub type DocumentKind = enum {
    KeyValue,
    Table,
};
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
        .{ Value: payload } => payload,
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

fn main() i32 {
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

fn main() i32 {
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
use base.mem.layout_of;

type Node[K, V] = struct {
    next: *mut Node[K, V],
    key: K,
    value: V,
};

fn free_node[K, V](alloc: *mut base.mem.alloc.Allocator, node: *mut Node[K, V]) void {
    alloc.free(node as *mut u8, layout_of[Node[K, V]]());
}

fn wrap_free[K, V](alloc: *mut base.mem.alloc.Allocator, node: *mut Node[K, V]) void {
    free_node(alloc, node);
}

fn main() i32 {
    let _ = wrap_free[i32, i32];
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

fn main() i32 {
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

fn main() i32 {
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

fn main() i32 {
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
fn main() i32 {
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
fn runs_zig_style_multiline_strings() {
    let output = build_and_run(
        "kernc_multiline_string_run",
        r#"
use std.io;

fn main() i32 {

    let msg =
        \\line one
        \\line "two"
        \\line three
    ;

    let mut out = io.stdout();
    let _ = out..&.write(msg);
    let _ = out..&.write("\n");
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    assert!(
        output.status.success(),
        "hosted regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "line one\nline \"two\"\nline three\n"
    );
}

#[test]
fn compiles_and_runs_trailing_commas_in_common_lists() {
    let output = build_and_run_source(
        r#"
type Pair[T,] = struct {
    left: T,
    right: T,
};

type Choice = enum {
    A,
    B,
};

type Ops = trait {
    run: fn(i32, i32,) i32,
};

fn add(a: i32, b: i32,) i32 {
    return a + b;
}

fn sum_pair(pair: Pair[i32,],) i32 {
    let values = [2]i32.{ pair.left, pair.right, };
    match (pair.left) {
        2, => return add(values.[0], values.[1],),
        _ => return 1,
    }
}

fn main() i32 {
    let pair = Pair[i32,].{ left: 2, right: 3, };
    if (sum_pair(pair,) == 5) {
        return 0;
    }
    return 1;
}
"#,
    );

    assert!(
        output.status.success(),
        "trailing comma regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn hints_about_trailing_comma_for_type_qualified_single_element_array_literal() {
    let output = compile_source(
        r#"
fn main() i32 {
    let out = [1]mut u8.{ 7 };
    let _ = out;
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
        stderr.contains("write `Type.{ value, }` with a trailing comma"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("scalar initialization"),
        "unexpected stderr:\n{}",
        stderr
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

fn main() i32 {
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

fn main() i32 {
    let _ = match (fail()) {
        .{ Ok: v } => v,
        .{ Err: _err } => {
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
fn compiles_returning_never_expression_without_emitting_extra_ret() {
    let output = compile_source(
        r#"
fn fail() bool {
    return @trap();
}

fn main() i32 {
    let _ = fail();
    return 0;
}
"#,
    );

    assert_success(&output, "kernc");
}

#[test]
fn compiles_generic_helper_returning_match_of_never_arms() {
    let output = compile_source(
        r#"
type Result[T, E] = enum {
    Ok: T,
    Err: E,
};

fn expect_ok[T, E](value: Result[T, E]) T {
    match (value) {
        .{ Ok: payload } => return payload,
        .{ Err: _ } => {
            return match (0) {
                0 => @trap(),
                _ => @trap(),
            };
        },
    }
}

fn main() i32 {
    let _ = expect_ok[i32, bool](.{ Ok: 7 });
    return 0;
}
"#,
    );

    assert_success(&output, "kernc");
}

#[test]
fn compiles_never_in_let_initializer_without_emitting_store() {
    let output = compile_source(
        r#"
fn main() i32 {
    let x = @trap();
    let _ = x;
    return 0;
}
"#,
    );

    assert_success(&output, "kernc");
}

#[test]
fn compiles_never_in_call_argument_without_emitting_followup_call() {
    let output = compile_source(
        r#"
fn consume(value: i32) void {
    let _ = value;
}

fn main() i32 {
    consume(@trap());
    return 0;
}
"#,
    );

    assert_success(&output, "kernc");
}

#[test]
fn runs_for_clauses_with_non_void_init_post_and_body() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
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
