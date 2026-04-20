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
fn runs_root_module_use_imports_for_std_and_base() {
    let output = build_and_run_source_with_std(
        r#"
use std;
use base;

fn main() i32 {
    let same = base.coll.bytes_eq("ok", "ok");
    if (!same) {
        return 1;
    }
    std.io.print("{}", .{"ok",});
    return 0;
}
"#,
    );

    assert!(
        output.status.success(),
        "program failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "ok");
}

#[test]
fn runs_nested_use_trees_with_grouped_self_imports() {
    let output = build_and_run_source_with_std(
        r#"
use std.{. as stdlib, io.{., Printable, Writer as W}};

type Pair = struct {
    value: usize,
};

impl Pair : Printable {
    pub fn fmt(writer: *mut W) void {
        let _ = writer.write("[");
        self.value.&.fmt(writer);
        let _ = writer.write("]");
    }
}

fn main() i32 {
    stdlib.io.print("{}", .{ Pair.{ value: 1 }, });
    io.println("{}", .{ Pair.{ value: 2 }, });
    return 0;
}
"#,
    );

    assert!(
        output.status.success(),
        "program failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "[1][2]\n");
}

#[test]
fn runs_explicit_loc_intrinsic_and_const_loc_values() {
    let output = build_and_run_source(
        r#"
const GLOBAL = @loc();

fn take(loc: struct { file: []u8, line: usize, col: usize }) usize {
    return loc.line;
}

fn main() i32 {
    let local = @loc();
    if (#GLOBAL.file == 0) {
        return 11;
    }
    if (GLOBAL.line != 2) {
        return 12;
    }
    if (GLOBAL.col == 0) {
        return 13;
    }
    if (#local.file == 0) {
        return 14;
    }
    if (local.line != 9) {
        return 15;
    }
    if (local.col == 0) {
        return 16;
    }
    if (take(@loc()) == 0) {
        return 17;
    }
    return 0;
}
"#,
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "explicit @loc regression binary failed:\nstdout:\n{}\nstderr:\n{}",
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
fn rejects_direct_raw_pointer_literals_and_null_raw_pointer_casts() {
    let rejected = compile_source(
        r#"
fn main() i32 {
    let ptr = *mut i32.{0};
    return if ((ptr as usize) == 0) 0 else 1;
}
"#,
    );

    assert!(
        !rejected.status.success(),
        "kernc unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("raw pointers cannot be initialized with `.{...}`"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&rejected.stderr)
    );

    let null_cast = build_and_run_source(
        r#"
fn main() i32 {
    let ptr = 0 as *mut i32;
    return if ((ptr as usize) == 0) 0 else 1;
}
"#,
    );

    assert_eq!(
        null_cast.status.code(),
        Some(0),
        "raw-pointer null-cast regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&null_cast.stdout),
        String::from_utf8_lossy(&null_cast.stderr)
    );

    let direct_non_zero = build_and_run_source(
        r#"
fn main() i32 {
    let ptr = 1 as *mut i32;
    return if ((ptr as usize) == 1) 0 else 1;
}
"#,
    );

    assert_eq!(
        direct_non_zero.status.code(),
        Some(0),
        "direct integer-to-pointer regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&direct_non_zero.stdout),
        String::from_utf8_lossy(&direct_non_zero.stderr)
    );

    let optional = compile_source(
        r#"
fn main() i32 {
    let zero = 0 as ?*mut i32;
    let one = 1 as ?*mut i32;

    let zero_score = match (zero) {
        .None => i32.{0},
        .{ Some: _ } => i32.{10},
    };
    let one_score = match (one) {
        .None => i32.{20},
        .{ Some: ptr } => if ((ptr as usize) == 1) i32.{3} else i32.{30},
    };

    return zero_score + one_score;
}
"#,
    );

    assert!(
        !optional.status.success(),
        "kernc unexpectedly accepted direct integer-to-builtin-option pointer casts:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&optional.stdout),
        String::from_utf8_lossy(&optional.stderr)
    );
}

#[test]
fn explicit_generic_pointer_casts_instantiate_target_types() {
    let output = build_and_run_source(
        r#"
type Boxed[T] = struct {
    ptr: *mut T,
};

fn make_ptr[T](addr: usize) *mut T {
    return addr as *mut T;
}

fn clear[T](value: *mut Boxed[T]) void {
    value.ptr = make_ptr[T](1);
}

fn main() i32 {
    let mut boxed = Boxed[i32].{ ptr: make_ptr[i32](2) };
    clear[i32](boxed..&);
    return if ((boxed.ptr as usize) == 1) 0 else 1;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "generic pointer cast regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn accepts_optional_volatile_pointer_types() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let ptr = ?^mut i32.{ Some: 1 as ^mut i32 };
    return match (ptr) {
        .None => 1,
        .{ Some: raw } => if ((raw as usize) == 1) 0 else 2,
    };
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "optional volatile-pointer regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn keeps_optional_pointer_values_as_plain_builtin_enums() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let none = (?*mut i32).None;
    let some = (?*mut i32).{ Some: 1 as *mut i32 };

    let none_score = match (none) {
        .None => i32.{0},
        .{ Some: _ } => i32.{10},
    };
    let some_score = match (some) {
        .None => i32.{20},
        .{ Some: ptr } => if ((ptr as usize) == 1) i32.{3} else i32.{30},
    };

    return none_score + some_score;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(3),
        "optional pointer enum regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_unsupported_object_pointer_addition_forms_in_builtin_pointer_arithmetic() {
    let output = compile_source(
        r#"
fn main() i32 {
    let lhs = 1 as *mut i32;
    let rhs = 2 as *mut i32;
    let _ = lhs + rhs;
    return 0;
}
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted unsupported `*T + *T` arithmetic:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("invalid pointer arithmetic"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn keeps_object_pointer_offset_arithmetic_available_as_a_builtin_primitive() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let ptr = 100 as *mut i32;
    let next = ptr + usize.{7};
    let prev = next - usize.{3};
    return (prev as usize) as i32;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(116),
        "object-pointer arithmetic regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn keeps_zero_sized_object_pointer_offsets_stable_as_a_builtin_primitive() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let ptr = 77 as *mut void;
    let next = ptr + usize.{9};
    let prev = next - usize.{4};
    return if ((prev as usize) == 77) 0 else 1;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "zero-sized object-pointer arithmetic regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn keeps_builtin_address_pointer_arithmetic_for_volatile_pointers() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let ptr = 9 as ^mut i32;
    let next = ptr + usize.{5};
    let prev = next - usize.{2};
    return (prev as usize) as i32;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(12),
        "address-pointer arithmetic regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn permits_direct_volatile_to_object_pointer_casts() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let raw = 1 as ^mut i32;
    let ptr = raw as *mut i32;
    return if ((ptr as usize) == 1) 0 else 1;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "volatile-to-object pointer cast regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn emits_volatile_loads_and_stores_for_address_pointer_dereferences() {
    let output = emit_llvm_ir_with_args(
        "kernc_volatile_pointer_ir",
        r#"
#[export_name("kern_read_reg")]
extern fn read_reg(reg: ^u32) u32 {
    return reg.*;
}

#[export_name("kern_write_reg")]
extern fn write_reg(reg: ^mut u32, value: u32) void {
    reg.* = value;
}
"#,
        &[],
    );

    assert!(
        output.status.success(),
        "kernc failed to emit LLVM IR for volatile pointer regression:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let ir = String::from_utf8_lossy(&output.stdout);
    assert!(
        ir.contains("load volatile i32"),
        "expected volatile load for `^T` dereference, got LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("store volatile i32"),
        "expected volatile store for `^mut T` dereference, got LLVM IR:\n{}",
        ir
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
    return Switch.On;
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
fn rejects_infinite_polymorphic_recursion_with_instantiation_chain() {
    let output = compile_source(
        r#"
type Wrap[T] = struct {
    inner: T,
};

fn poly[T](x: T) i32 {
    return poly[Wrap[T]](Wrap[T].{ inner: x });
}

fn main() i32 {
    return poly[i32](0);
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
        stderr.contains("infinitely many specializations"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("instantiation chain: poly[i32] -> poly[Wrap[i32]]"),
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
fn rejects_mutual_polymorphic_recursion_with_instantiation_chain() {
    let output = compile_source(
        r#"
type Wrap[T] = struct {
    inner: T,
};

fn f[T](x: T) i32 {
    return g[Wrap[T]](Wrap[T].{ inner: x });
}

fn g[T](x: T) i32 {
    return f[Wrap[T]](Wrap[T].{ inner: x });
}

fn main() i32 {
    return f[i32](0);
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
        stderr.contains("infinitely many specializations"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("instantiation chain: f[i32] -> g[Wrap[i32]] -> f[Wrap[Wrap[i32]]]"),
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
fn rejects_const_generic_polymorphic_recursion_with_specialization_diagnostic() {
    let output = compile_source(
        r#"
fn grow[N: usize]() i32 {
    return grow[N + 1]();
}

fn main() i32 {
    return grow[0]();
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
        stderr.contains("recursive specialization depth limit")
            || stderr.contains("specialization work queue limit")
            || stderr.contains("specialization limit"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("grow[0] -> grow[1]"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("const generic arguments do not stabilize across recursive calls"),
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
fn rejects_mutual_const_generic_polymorphic_recursion_with_specialization_diagnostic() {
    let output = compile_source(
        r#"
fn f[N: usize]() i32 {
    return g[N + 1]();
}

fn g[N: usize]() i32 {
    return f[N + 1]();
}

fn main() i32 {
    return f[0]();
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
        stderr.contains("recursive specialization depth limit")
            || stderr.contains("specialization work queue limit")
            || stderr.contains("specialization limit"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("f[0] -> g[1] -> f[2]"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("const generic arguments do not stabilize across recursive calls"),
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
fn runs_const_generic_params_in_ordinary_expressions() {
    let output = build_and_run_source(
        r#"
type Mode = enum {
    Off,
    On,
};

fn leaf[N: usize]() i32 {
    return N as i32;
}

fn forward[N: usize]() i32 {
    return leaf[N]() + leaf[N + 1]();
}

fn choose[B: bool]() i32 {
    if (B) {
        return 11;
    }

    return 22;
}

fn select_mode[M: Mode]() i32 {
    return match (M) {
        .Off => 30,
        .On => 40,
    };
}

fn main() i32 {
    return forward[7]() + choose[true]() + choose[false]() + select_mode[Mode.On]() - 88;
}
"#,
    );

    assert!(
        output.status.success(),
        "program failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn runs_const_fn_with_const_generic_arguments_during_consteval() {
    let output = build_and_run_source(
        r#"
const fn bump[N: usize]() usize {
    return N + 1;
}

const fn width[N: usize](value: [N]u8) usize {
    return (value.[0] as usize) + bump[N]() + @sizeOf[[N]u8]();
}

const TOTAL = width[3]([3]u8.{ 1, 2, 3 });

fn main() i32 {
    return TOTAL as i32;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(8),
        "const generic consteval regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn normalizes_const_trait_projection_during_expression_typeck() {
    let output = build_and_run_source(
        r#"
type HasOut[N: usize] = trait {
    type Out;
};

type X = struct {};

impl X: HasOut[1] {
    type Out = i32;
}

fn take(value: X.HasOut[1].Out) i32 {
    return value;
}

fn main() i32 {
    return take(7);
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(7),
        "const trait projection regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn normalizes_const_trait_projection_from_env_bounds() {
    let output = build_and_run_source(
        r#"
type HasOut[N: usize] = trait {
    type Out;
};

type X = struct {};

impl X: HasOut[1] {
    type Out = i32;
}

fn lift[T](value: T.HasOut[1].Out) T.HasOut[1].Out
    where T: HasOut[1, Out = i32],
{
    return value;
}

fn main() i32 {
    return lift[X](7);
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(7),
        "const trait projection env-bound regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_nested_typed_struct_pattern_with_mismatched_const_generic_argument() {
    let output = compile_source(
        r#"
type Inner[N: usize] = struct {
    data: [N]u8,
};

type Outer[N: usize] = struct {
    inner: Inner[N],
};

fn main() i32 {
    let value = Outer[3].{ inner: Inner[3].{ data: [3]u8.{ 1, 2, 3 } } };
    let Outer[3].{ inner: Inner[4].{ data } } = value;
    return data.[0] as i32;
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
        stderr.contains("mismatched types"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("expected `Inner[3]`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("found `Inner[4]`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_nested_typed_enum_pattern_with_mismatched_const_generic_argument() {
    let output = compile_source(
        r#"
type Inner[N: usize] = enum {
    A: [N]u8,
    B,
};

type Outer[N: usize] = enum {
    Wrap: Inner[N],
    Done,
};

fn main() i32 {
    let value = Outer[3].{ Wrap: Inner[3].{ A: [3]u8.{ 1, 2, 3 } } };
    return match (value) {
        .{ Wrap: Inner[4].{ A: _ } } => 0,
        .{ Wrap: .B } => 1,
        .Done => 2,
    };
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
        stderr.contains("mismatched types"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("expected `Inner[3]`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("found `Inner[4]`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn runs_nested_typed_const_generic_patterns() {
    let output = build_and_run_source(
        r#"
type Inner[N: usize] = enum {
    A: [N]u8,
    B,
};

type Outer[N: usize] = struct {
    inner: Inner[N],
};

fn classify(value: Outer[3]) i32 {
    return match (value) {
        Outer[3].{ inner: Inner[3].{ A: _ } } => 4,
        Outer[3].{ inner: .B } => 5,
    };
}

fn main() i32 {
    let value = Outer[3].{ inner: Inner[3].{ A: [3]u8.{ 4, 5, 6 } } };
    let Outer[3].{ inner: Inner[3].{ A: payload } } =
        Outer[3].{ inner: Inner[3].{ A: [3]u8.{ 4, 5, 6 } } } else return 1;
    return classify(value) + (payload.[2] as i32) - 10;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "nested const-generic pattern regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

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

#[test]
fn rejects_direct_recursive_struct_layout_cycle() {
    let output = compile_source(
        r#"
type Bad = struct {
    inner: Bad,
};

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
        stderr.contains("recursively contains itself by value"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("recursive layout chain: Bad -> Bad"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_direct_recursive_enum_payload_layout_cycle() {
    let output = compile_source(
        r#"
type Bad = enum {
    Loop: Bad,
};

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
        stderr.contains("recursively contains itself by value"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("recursive layout chain: Bad -> Bad"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_indirect_recursive_struct_layout_cycle_with_chain() {
    let output = compile_source(
        r#"
type A = struct {
    b: B,
};

type B = struct {
    a: A,
};

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
        stderr.contains("recursively contains itself by value"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("recursive layout chain: A -> B -> A"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_constant_overshift_without_panicking() {
    let output = compile_source(
        r#"
const BAD = 1 << 999;

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
        stderr.contains("shift amount in constant expression is too large"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("panicked at"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_constant_division_overflow_without_panicking() {
    let output = compile_source(
        r#"
fn main() i32 {
    let _ = [((-170141183460469231731687303715884105728) / (-1))]u8.{ undef };
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
        stderr.contains("division overflow in constant expression"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert_eq!(
        stderr
            .matches("division overflow in constant expression")
            .count(),
        1,
        "unexpected duplicated stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("panicked at"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_array_length_constants_that_exceed_usize_range() {
    let output = compile_source(
        r#"
fn main() i32 {
    let _ = [18446744073709551616]u8.{ undef };
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
        stderr.contains("integer literal 18446744073709551616 is out of bounds for type `usize`")
            || stderr.contains("constant expression is too large for this usize-like context"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn accepts_large_u128_constant_literals() {
    let output = build_and_run_source(
        r#"
const MID = u128.{170141183460469231731687303715884105728};
const MAX = u128.{340282366920938463463374607431768211455};

fn main() i32 {
    if (!(MAX > MID)) {
        return 1;
    }
    if (!(MID > u128.{1})) {
        return 2;
    }
    return 0;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "large u128 literal regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn folds_large_u128_constant_comparisons_correctly() {
    let output = build_and_run_source(
        r#"
const MID = u128.{170141183460469231731687303715884105728};
const OK = MID > u128.{1};

fn main() i32 {
    return if (OK) 0 else 1;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "large u128 const comparison regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_returning_capturing_closure_as_fn_pointer() {
    let output = compile_source(
        r#"
fn make() *Fn(i32) i32 {
    let base = i32.{7};
    return .[base](x: i32) i32 {
        return x + base;
    };
}

fn main() i32 {
    let f = make();
    return f(5);
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
        stderr.contains("cannot return a capturing closure as `*Fn(i32) i32`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("closure environment would escape the current stack frame"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("LLVM IR Verification Failed"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_trailing_capturing_closure_tail_as_fn_pointer() {
    let output = compile_source(
        r#"
fn make() *Fn(i32) i32 {
    let base = i32.{7};
    .[base](x: i32) i32 {
        return x + base;
    }
}

fn main() i32 {
    let f = make();
    return f(5);
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
        stderr.contains("cannot return a capturing closure as `*Fn(i32) i32`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("LLVM IR Verification Failed"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn returns_noncapturing_closure_as_fn_pointer() {
    let output = build_and_run_source(
        r#"
fn make() *Fn(i32) i32 {
    return .[](x: i32) i32 {
        return x + 7;
    };
}

fn main() i32 {
    let f = make();
    return f(5) - 12;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "noncapturing closure return regression binary failed:\nstdout:\n{}\nstderr:\n{}",
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
fn dispatches_trait_objects_through_const_specific_target_impls() {
    let output = build_and_run_source(
        r#"
type Score = trait {
    value: fn() i32,
};

type Buf[N: usize] = struct {};

impl[N: usize] *Buf[N]: Score {
    fn value() i32 {
        return 1;
    }
}

impl *Buf[4]: Score {
    fn value() i32 {
        return 2;
    }
}

fn main() i32 {
    let buf = Buf[4].{};
    let score = *Score.{ buf.& };
    return score.value() - 2;
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
fn dispatches_trait_objects_through_const_specific_trait_args() {
    let output = build_and_run_source(
        r#"
type Score[N: usize] = trait {
    value: fn() i32,
};

type X = struct {};

impl[N: usize] *X: Score[N] {
    fn value() i32 {
        return 1;
    }
}

impl *X: Score[4] {
    fn value() i32 {
        return 2;
    }
}

fn main() i32 {
    let x = X.{};
    let score = *Score[4].{ x.& };
    return score.value() - 2;
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
fn dispatches_bound_methods_through_const_specific_trait_args() {
    let output = build_and_run_source(
        r#"
type Family[N: usize] = trait {
    value: fn() i32,
};

type X = struct {};

impl *X: Family[1] {
    fn value() i32 {
        return 11;
    }
}

impl *X: Family[2] {
    fn value() i32 {
        return 22;
    }
}

fn call[N: usize](x: *X) i32
    where *X: Family[N],
{
    return x.value();
}

fn main() i32 {
    let x = X.{};
    return call[2](x.&) - 22;
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
fn casts_to_const_generic_trait_object_from_generic_impl() {
    let output = build_and_run_source(
        r#"
type Score[N: usize] = trait {
    value: fn() i32,
};

type X = struct {};

impl[N: usize] *X: Score[N] {
    fn value() i32 {
        return N as i32;
    }
}

fn main() i32 {
    let x = X.{};
    let score = *Score[4].{ x.& };
    return score.value() - 4;
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
fn compiles_assignment_through_struct_mut_array_fields_only() {
    let output = compile_source(
        r#"
type Buffer = struct {
    items: [4]mut i32,
};

fn main() i32 {
    let mut buf = Buffer.{ items: [4]mut i32.{ 0; 4 } };
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
fn runs_defer_after_block_value_evaluation() {
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

fn read_block_before_defer() i32 {
    return {
        let mut state = i32.{1};
        let mut guard = Guard.{ ptr: state..& };
        defer guard..&.deinit();
        state
    };
}

fn main() i32 {
    if (read_block_before_defer() != 1) {
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
fn runs_block_defers_in_lifo_order_after_materializing_value() {
    let output = build_and_run_source(
        r#"
type Push = struct {
    ptr: *mut i32,
    digit: i32,
};

impl *mut Push {
    pub fn deinit() void {
        self.ptr.* = self.ptr.* * 10 + self.digit;
    }
}

fn main() i32 {
    let mut state = i32.{0};
    let value = {
        let mut first = Push.{ ptr: state..&, digit: 1 };
        let mut second = Push.{ ptr: state..&, digit: 2 };
        defer first..&.deinit();
        defer second..&.deinit();
        7
    };

    if (value != 7) {
        return 1;
    }
    if (state != 21) {
        return 2;
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

#[test]
fn accepts_multiline_string_inline_asm_templates() {
    let output = compile_source(
        r#"
fn main() i32 {
    @asm(.{
        asm:
            \\nop
            \\nop
        ,
        volatile: true,
    });
    return 0;
}
"#,
    );

    assert_success(&output, "kernc multiline @asm");
}

#[test]
fn accepts_multiline_string_inline_asm_templates_for_aarch64_darwin_target() {
    let output = compile_source_with_args(
        "kernc_multiline_inline_asm_aarch64_darwin",
        r#"
fn main() i32 {
    @asm(.{
        asm:
            \\nop
            \\nop
        ,
        volatile: true,
    });
    return 0;
}
"#,
        &["--target", "aarch64-apple-darwin"],
    );

    assert_success(&output, "kernc multiline @asm for aarch64-apple-darwin");
}

#[test]
fn rejects_legacy_inline_asm_string_arrays_with_migration_hint() {
    let output = compile_source(
        r#"
fn main() i32 {
    @asm(.{
        asm: .{
            "nop",
            "nop",
        },
        volatile: true,
    });
    return 0;
}
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted legacy @asm array syntax:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`asm` template must be a string literal"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("use one string literal instead"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn reports_targeted_error_for_unterminated_inline_asm_string() {
    let output = compile_source(
        r#"
fn main() i32 {
    @asm(.{
        asm: "nop
        volatile: true,
    });
    return 0;
}
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted malformed @asm string:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unterminated string literal before end of line"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("Expected expression"),
        "unexpected cascading parser stderr:\n{}",
        stderr
    );
}

#[test]
fn reports_missing_comma_between_inline_asm_config_fields() {
    let output = compile_source(
        r#"
fn main() i32 {
    @asm(.{
        asm: "nop"
        volatile: true,
    });
    return 0;
}
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted malformed @asm fields:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("expected `,` between fields in data initializer"),
        "unexpected stderr:\n{}",
        stderr
    );
}
