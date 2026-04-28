use super::*;

#[test]
fn string_literals_are_fixed_byte_arrays() {
    let output = build_and_run_source(
        r#"
const TITLE = "abc\0";
const EMPTY = "";

fn take_slice(text: []u8) usize {
    return #text;
}

fn take_array(value: [5]u8) u8 {
    return value.[4];
}

fn main() i32 {
    if (#TITLE != 4) {
        return 1;
    }
    if (TITLE.[0] != b'a' or TITLE.[3] != 0) {
        return 2;
    }
    if (#EMPTY != 0) {
        return 3;
    }
    if (take_slice("hello") != 5) {
        return 4;
    }
    if (take_array("abcd\0") != 0) {
        return 5;
    }
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
fn rejects_duplicate_generic_parameters() {
    let output = compile_source(
        r#"
fn identity[T, T](value: T) T {
    return value;
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
        stderr.contains("the generic parameter `T` is defined multiple times"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("defined only once in the same generic parameter list"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn parses_casts_after_prefix_unary_operators() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let array = [4]u8.{ 1, 2, 3, 4 };
    return #array as i32 - 1;
}
"#,
    );

    assert_eq!(output.status.code(), Some(3));
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
use std.{. as stdlib, io.{.}};
use base.{io.{Printable, Writer as W}};

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
fn runs_direct_array_printing_through_slice_trait_coercion() {
    let output = build_and_run_source_with_std(
        r#"
use std.io;

fn main() i32 {
    let array = [3]i32.{ 1, 2, 3 };
    io.println("{}", .{ array, });
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
    assert_eq!(String::from_utf8_lossy(&output.stdout), "[1, 2, 3]\n");
}

#[test]
fn rejects_passing_explicit_slice_literal_to_fixed_array_parameter() {
    let output = compile_source(
        r#"
fn take(items: [3]i32) i32 {
    return items.[1];
}

fn main() i32 {
    return take([]mut i32.{ 1, 2, 3 });
}
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted an explicit slice literal as `[3]i32`:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("expected `[3]i32`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("found `[]mut i32`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn immutable_subslice_method_miss_suggests_mutable_slice_syntax() {
    let output = compile_source_with_std(
        r#"
use base.coll;

fn main() i32 {
    let mut values = [4]i32.{ 1, 2, 3, 4 };
    let view = values..[0 .. 4];
    view.[0 .. 2].reverse();
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
        stderr.contains("no field or method named `reverse` found on type `[]i32`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("use `..[start .. end]` when you need a mutable subslice"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn runs_explicit_slice_literals_with_local_backing_storage() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let items = []mut i32.{ 1, 2, 3 };
    items.[1] = 9;
    return items.[1] - 9;
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
fn runs_computed_mut_slice_element_address_of_with_signed_index() {
    let output = build_and_run_source(
        r#"
fn bump(ptr: *mut i32) void {
    ptr.* += 1;
}

fn main() i32 {
    let mut array = [3]i32.{ 10, 20, 30 };
    let view = array..[0 .. 3];
    let i = i32.{0};
    bump(view.[i + 1]..&);
    return view.[1] - 21;
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
fn runs_in_place_quicksort_on_mut_slice() {
    let output = build_and_run_source(
        r#"
fn swap(a: *mut i32, b: *mut i32) void {
    let t = a.*;
    a.* = b.*;
    b.* = t;
}

fn partition(arr: []mut i32, low: i32, high: i32) i32 {
    let pivot = arr.[high];
    let mut i = low - 1;

    let mut j = low;
    while (j <= high - 1) {
        if (arr.[j] < pivot) {
            i += 1;
            swap(arr.[i]..&, arr.[j]..&);
        }
        j += 1;
    }

    let pivot_idx = i + 1;
    swap(arr.[pivot_idx]..&, arr.[high]..&);
    return pivot_idx;
}

fn quick_sort(arr: []mut i32, low: i32, high: i32) void {
    if (low < high) {
        let pivot = partition(arr, low, high);
        quick_sort(arr, low, pivot - 1);
        quick_sort(arr, pivot + 1, high);
    }
}

fn main() i32 {
    let mut array = [8]i32.{ 1, 23, 3, 7, 8, 29, 28, 57 };
    let view = array..[0 .. 8];
    quick_sort(view, 0, 7);

    let expected = [8]i32.{ 1, 3, 7, 8, 23, 28, 29, 57 };
    let mut i = 0;
    while (i < 8) {
        if (array.[i] != expected.[i]) {
            return (i as i32) + 1;
        }
        i += 1;
    }
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
}

#[test]
fn infers_usize_for_while_counter_from_array_length_comparison() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let data = [4]u8.{ 3, 5, 7, 11 };
    let mut sum = i32.{0};

    let mut i = 0;
    while (i < #data) {
        sum += data.[i] as i32;
        i += 1;
    }

    return sum - 26;
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
fn infers_usize_for_slice_bounds_from_expected_context() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let data = [4]u8.{ 9, 8, 7, 6 };
    let start = 0;
    let tail = data.[start .. #data];
    return (#tail as i32) - 4;
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
fn keeps_explicit_numeric_casts_working_with_delayed_literal_inference() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let value = 0 as usize;
    return value as i32;
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
    let ptr = usize.{0} as *mut i32;
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
    let ptr = usize.{1} as *mut i32;
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
    let ptr = ?^mut i32.{ Some: usize.{1} as ^mut i32 };
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
    let some = (?*mut i32).{ Some: usize.{1} as *mut i32 };

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
    let lhs = usize.{1} as *mut i32;
    let rhs = usize.{2} as *mut i32;
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
    let ptr = usize.{100} as *mut i32;
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
fn infers_bare_integer_literals_for_object_pointer_offsets() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let ptr = usize.{100} as *mut i32;
    let step = 7;
    let next = ptr + step;
    let prev = 3 + next - 2;
    return (prev as usize) as i32;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(132),
        "object-pointer offset inference regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn keeps_zero_sized_object_pointer_offsets_stable_as_a_builtin_primitive() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let ptr = usize.{77} as *mut void;
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
    let ptr = usize.{9} as ^mut i32;
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
fn infers_bare_integer_literals_for_volatile_pointer_offsets() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let ptr = usize.{9} as ^mut i32;
    let step = 5;
    let next = ptr + step;
    let prev = next - 2;
    return (prev as usize) as i32;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(12),
        "address-pointer offset inference regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn keeps_pointer_offset_literals_polymorphic_until_later_exact_context() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let ptr = usize.{100} as *mut i32;
    let step = 7;
    let next = ptr + step;
    let amount = usize.{step};
    return ((next as usize) + amount) as i32;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(135),
        "pointer-offset polymorphic literal regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn pointer_offset_literal_conflicts_report_human_facing_types_instead_of_typevars() {
    let output = compile_source(
        r#"
fn main() i32 {
    let ptr = usize.{0} as *mut i32;
    let step = 1;
    let _next = ptr + step;
    let narrowed = u8.{step};
    return 0;
}
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted conflicting pointer-offset literal narrowing:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("?T"),
        "unexpected unresolved typevar leaked into diagnostic:\n{}",
        stderr
    );
    assert!(
        stderr.contains("pointer offset integer"),
        "expected pointer-offset diagnostic wording:\n{}",
        stderr
    );
}

#[test]
fn bitwise_literal_constraints_reject_later_float_reinterpretation() {
    let output = compile_source(
        r#"
fn main() i32 {
    let mask = 1;
    let _bits = mask << 2;
    let wrong = f64.{mask};
    return 0;
}
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted float reinterpretation after integer-only bitwise use:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("?T"),
        "unexpected unresolved typevar leaked into diagnostic:\n{}",
        stderr
    );
    assert!(
        stderr.contains("inferred integer literal"),
        "expected integer-literal diagnostic wording:\n{}",
        stderr
    );
}

#[test]
fn bitwise_literal_constraints_still_allow_later_integer_specialization() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let mask = 1;
    let widened = mask << 3;
    let narrowed = u8.{mask};
    return (widened as i32) + (narrowed as i32) - 9;
}
"#,
    );

    assert!(
        output.status.success(),
        "bitwise literal integer specialization regression failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn unary_bitwise_literal_constraints_reject_later_float_reinterpretation() {
    let output = compile_source(
        r#"
fn main() i32 {
    let mask = 1;
    let _bits = ~mask;
    let wrong = f64.{mask};
    return 0;
}
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted float reinterpretation after unary integer-only bitwise use:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("?T"),
        "unexpected unresolved typevar leaked into diagnostic:\n{}",
        stderr
    );
    assert!(
        stderr.contains("inferred integer literal"),
        "expected integer-literal diagnostic wording:\n{}",
        stderr
    );
}

#[test]
fn unary_bitwise_literal_constraints_still_allow_later_integer_specialization() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let mask = 1;
    let flipped = ~mask;
    let narrowed = u8.{mask};
    return (flipped as i32) + (narrowed as i32) + 1;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "unary bitwise literal integer specialization regression failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn infers_bare_integer_literals_for_integer_to_pointer_casts() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let raw = 1;
    let ptr = raw as *mut i32;
    let widened = usize.{raw};
    return ((ptr as usize) + widened) as i32 - 2;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "integer-to-pointer cast literal inference regression failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn integer_to_pointer_cast_literals_conflict_cleanly_with_non_pointer_sized_integers() {
    let output = compile_source(
        r#"
fn main() i32 {
    let raw = 1;
    let _ptr = raw as *mut i32;
    let narrowed = u8.{raw};
    return 0;
}
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted narrowing after integer-to-pointer cast inference:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("?T"),
        "unexpected unresolved typevar leaked into diagnostic:\n{}",
        stderr
    );
    assert!(
        stderr.contains("pointer offset integer"),
        "expected pointer-sized integer diagnostic wording:\n{}",
        stderr
    );
}

#[test]
fn permits_direct_volatile_to_object_pointer_casts() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let raw = usize.{1} as ^mut i32;
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
fn member_lookup_keeps_same_named_fields_and_methods_distinct() {
    let output = build_and_run_source(
        r#"
type Counter = struct {
    len: i32,
};

impl Counter {
    pub fn len() i32 {
        return self.len + 10;
    }
}

fn main() i32 {
    let counter = Counter.{ len: 5 };
    return counter.len + counter.len();
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(20),
        "same-name field/method regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn method_call_syntax_prefers_methods_but_parenthesized_field_calls_still_work() {
    let output = build_and_run_source(
        r#"
fn forty() i32 {
    return 40;
}

type Slot = struct {
    len: *Fn() i32,
};

impl Slot {
    pub fn len() i32 {
        return 7;
    }
}

fn main() i32 {
    let slot = Slot.{ len: forty };
    return slot.len() + (slot.len)();
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(47),
        "method/function-field call disambiguation regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn parenthesized_non_callable_field_call_suggests_method_call_syntax() {
    let output = compile_source(
        r#"
type Counter = struct {
    len: i32,
};

impl Counter {
    pub fn len() i32 {
        return 7;
    }
}

fn main() i32 {
    let counter = Counter.{ len: 5 };
    return (counter.len)();
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
        stderr.contains("expression is not callable"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("remove the parentheses to call method `len()`"),
        "unexpected stderr:\n{}",
        stderr
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
fn defaults_inferred_integer_generic_arguments_before_bound_checking() {
    let output = build_and_run_source(
        r#"
type Step[T] = trait {
    step: fn() T,
};

impl i32 : Step[i32] {
    pub fn step() i32 {
        return self + 1;
    }
}

fn advance[T](value: T) T
    where T: Step[T],
{
    return value.step();
}

fn main() i32 {
    return advance(41) - 42;
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
