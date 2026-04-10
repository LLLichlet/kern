use super::*;

#[test]
fn runs_i128_division_and_remainder_without_external_runtime_helpers() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let wide = (u128.{1} << u128.{100}) + u128.{12345};
    let divisor = u128.{97};
    let quotient = wide / divisor;
    let remainder = wide % divisor;
    if (quotient * divisor + remainder != wide) {
        return 1;
    }
    if (remainder >= divisor) {
        return 2;
    }

    let signed_wide = (i128.{0} - (i128.{1} << i128.{100})) + i128.{12345};
    let signed_divisor = i128.{97};
    let signed_quotient = signed_wide / signed_divisor;
    let signed_remainder = signed_wide % signed_divisor;
    if (signed_quotient * signed_divisor + signed_remainder != signed_wide) {
        return 3;
    }
    if (signed_remainder >= i128.{0}) {
        return 4;
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
fn successful_compile_prints_unused_private_function_warning_and_prunes_ir() {
    let source = r#"
fn helper() i32 {
    return 1;
}

fn main() i32 {
    return 0;
}
"#;

    let output = emit_llvm_ir_with_args("kernc_unused_private_warning", source, &[]);
    assert_success(&output, "kernc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("private function `helper` is never used"),
        "unexpected stderr:\n{}",
        stderr
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("helper"),
        "unused helper leaked into LLVM IR:\n{}",
        stdout
    );
}

#[test]
fn successful_compile_prints_unused_private_constant_warning_and_prunes_ir() {
    let source = r#"
const helper = 1;

fn main() i32 {
    return 0;
}
"#;

    let output = emit_llvm_ir_with_args("kernc_unused_private_const_warning", source, &[]);
    assert_success(&output, "kernc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("private constant `helper` is never used"),
        "unexpected stderr:\n{}",
        stderr
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("helper"),
        "unused helper leaked into LLVM IR:\n{}",
        stdout
    );
}

#[test]
fn successful_compile_prints_unused_private_static_warning_and_prunes_ir() {
    let source = r#"
static helper = 1;

fn main() i32 {
    return 0;
}
"#;

    let output = emit_llvm_ir_with_args("kernc_unused_private_static_warning", source, &[]);
    assert_success(&output, "kernc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("private static `helper` is never used"),
        "unexpected stderr:\n{}",
        stderr
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("helper"),
        "unused helper leaked into LLVM IR:\n{}",
        stdout
    );
}

#[test]
fn resolves_imported_generic_bounds_for_struct_field_literals() {
    let source = r#"
use base.coll.Map;

type Wrap = struct {
    item: Map[u64, i32],
};

fn main() i32 {
    let _ = Wrap.{ item: Map[u64, i32].{} };
    return 0;
}
"#;

    let output = compile_source_with_std(source);
    assert_success(&output, "kernc");
}

#[test]
fn emits_inline_attributes_in_llvm_ir() {
    let source = r#"
#[inline(always)]
fn hot_add(lhs: i32, rhs: i32) i32 {
    return lhs + rhs;
}

#[inline(never)]
fn cold_add(lhs: i32, rhs: i32) i32 {
    return lhs + rhs;
}

fn main() i32 {
    return hot_add(1, 2) + cold_add(3, 4);
}
"#;

    let output = emit_llvm_ir_with_args("kernc_inline_attrs_ir", source, &[]);
    assert_success(&output, "kernc");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("alwaysinline"),
        "expected alwaysinline in LLVM IR, got:\n{}",
        stdout
    );
    assert!(
        stdout.contains("noinline"),
        "expected noinline in LLVM IR, got:\n{}",
        stdout
    );
}

#[test]
fn indexes_const_arrays_through_their_global_storage() {
    let source = r#"
const TABLE = [4]u8.{ 1, 2, 3, 4 };

fn main() i32 {
    return TABLE.[2] as i32;
}
"#;

    let output = emit_llvm_ir_with_args("kernc_const_array_index_ir", source, &[]);
    assert_success(&output, "kernc");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("constant [4 x i8] c\"\\01\\02\\03\\04\""),
        "expected a constant global array in LLVM IR, got:\n{}",
        stdout
    );
    assert!(
        stdout.contains("ptr @_K4root5TABLE"),
        "expected index access to address the global const directly, got:\n{}",
        stdout
    );
    assert!(
        !stdout.contains("tmp_materialized_lvalue"),
        "const array indexing unexpectedly materialized a stack temporary:\n{}",
        stdout
    );
}

#[test]
fn compiles_result_with_payload_error_enum_without_union_alignment_ice() {
    let source = r#"
use base.Result;

type ParseError = enum {
    BadToken,
};

type HandshakeError = enum {
    Parse: ParseError,
    RouteRejected,
};

fn compute(ok: bool) Result[usize, HandshakeError] {
    if (ok) {
        return .{ Ok: usize.{7} };
    }
    return .{ Err: .{ Parse: ParseError.BadToken } };
}

fn main() i32 {
    match (compute(false)) {
        .{ Ok: _ } => return 1,
        .{ Err: err } => match (err) {
            .{ Parse: cause } => {
                if (cause != ParseError.BadToken) {
                    return 2;
                }
            },
            .RouteRejected => return 3,
        },
    }
    return 0;
}
"#;

    let output = compile_source_with_std(source);
    assert_success(&output, "kernc");
}

#[test]
fn folds_const_fn_array_initializers_into_global_data() {
    let source = r#"
const fn build() [4]mut u8 {
    let mut table = [4]mut u8.{ 0; 4 };
    table.[2] = 7;
    return table;
}

const TABLE = build();

fn main() i32 {
    return TABLE.[2] as i32;
}
"#;

    let output = emit_llvm_ir_with_args("kernc_const_fn_array_init_ir", source, &[]);
    assert_success(&output, "kernc");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("@_K4root5TABLE = global [4 x i8] c\"\\00\\00\\07\\00\""),
        "expected folded global array initializer in LLVM IR, got:\n{}",
        stdout
    );
    assert!(
        !stdout.contains("@_K4root5TABLE = global [4 x i8] zeroinitializer"),
        "const fn array initializer unexpectedly fell back to zero initialization:\n{}",
        stdout
    );
}

#[test]
fn emits_llvm_memmove_for_memmove_intrinsic() {
    let source = r#"
fn main() i32 {
    let buf = [4]mut u8.{ 1, 2, 3, 4 };
    @memmove(buf.[1]..& as *mut u8, buf.[0].& as *u8, 3);
    return 0;
}
"#;

    let output = emit_llvm_ir_with_args("kernc_memmove_ir", source, &[]);
    assert_success(&output, "kernc");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("llvm.memmove"),
        "expected llvm.memmove in LLVM IR, got:\n{}",
        stdout
    );
}

#[test]
fn runs_memmove_intrinsic_with_overlapping_ranges() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let buf = [4]mut u8.{ 1, 2, 3, 4 };
    @memmove(buf.[1]..& as *mut u8, buf.[0].& as *u8, 3);

    if (buf.[0] != 1) return 1;
    if (buf.[1] != 1) return 2;
    if (buf.[2] != 2) return 3;
    if (buf.[3] != 3) return 4;
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
fn compiles_same_private_const_name_in_multiple_modules() {
    let output = compile_source_tree_with_args(
        "kernc_private_const_module_scope",
        "main.rn",
        &[
            (
                "main.rn",
                r#"
mod left;
mod right;

fn main() i32 {
    return left.value() + right.value();
}
"#,
            ),
            (
                "left.rn",
                r#"
const SHARED = 10;

pub fn value() i32 {
    return SHARED as i32;
}
"#,
            ),
            (
                "right.rn",
                r#"
const SHARED = 32;

pub fn value() i32 {
    return SHARED as i32;
}
"#,
            ),
        ],
        &["-c"],
    );

    assert_success(&output, "kernc");
}

#[test]
fn successful_compile_prints_unused_binding_warnings() {
    let source = r#"
fn helper(_: i32, unused_param: i32, used_param: i32) i32 {
    let unused_local = used_param;
    return used_param;
}

fn main() i32 {
    return helper(1, 2, 3);
}
"#;

    let output = emit_llvm_ir_with_args("kernc_unused_bindings_warning", source, &[]);
    assert_success(&output, "kernc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("parameter `unused_param` is never used"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("local variable `unused_local` is never used"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("parameter `_` is never used"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn successful_compile_prints_dead_store_warning() {
    let source = r#"
fn helper(seed: i32) i32 {
    let mut value = seed;
    if (seed == 0) {
        return value;
    }
    value = seed + 1;
    value = seed + 2;
    return value;
}

fn main() i32 {
    return helper(1);
}
"#;

    let output = emit_llvm_ir_with_args("kernc_dead_store_warning", source, &[]);
    assert_success(&output, "kernc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("value assigned to `value` is never read"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn successful_compile_prints_dead_initializer_warning() {
    let source = r#"
fn helper(seed: i32) i32 {
    let mut value = seed;
    value = seed + 1;
    return value;
}

fn main() i32 {
    return helper(1);
}
"#;

    let output = emit_llvm_ir_with_args("kernc_dead_initializer_warning", source, &[]);
    assert_success(&output, "kernc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("initial value assigned to `value` is never read"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn pure_enum_payload_bound_from_match_compiles_and_runs() {
    let output = build_and_run_source(
        r#"
type Kind = enum {
    Root,
    Section,
};

type MaybeKind = enum {
    None,
    Some: Kind,
};

fn unwrap_kind(value: MaybeKind) Kind {
    return match (value) {
        .{ Some: kind } => kind,
        .None => Kind.Root,
    };
}

fn main() i32 {
    let kind = unwrap_kind(MaybeKind.{ Some: Kind.Section });
    match (kind) {
        .Root => return 1,
        .Section => return 0,
    }
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "program exited unexpectedly:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn method_returning_option_of_pure_enum_compiles_and_runs() {
    let output = build_and_run_source(
        r#"
type Kind = enum {
    Root,
    Section,
};

type Option[T] = enum {
    None,
    Some: T,
};

type Holder = struct {};

impl Holder {
    fn section_kind(flag: bool) Option[Kind] {
        if (flag) {
            return .{ Some: Kind.Section };
        }
        return .{ None };
    }
}

fn main() i32 {
    let holder = Holder.{};
    let kind = match (holder.section_kind(true)) {
        .{ Some: kind } => kind,
        .None => Kind.Root,
    };
    match (kind) {
        .Root => return 1,
        .Section => return 0,
    }
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "program exited unexpectedly:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn public_reexport_keeps_private_function_reachable_in_ir() {
    let source = r#"
fn helper() i32 {
    return 1;
}

pub use .helper as exported;

fn main() i32 {
    return 0;
}
"#;

    let output = emit_llvm_ir_with_args("kernc_reexport_root_ir", source, &[]);
    assert_success(&output, "kernc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("private function `helper` is never used"),
        "unexpected stderr:\n{}",
        stderr
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("helper"),
        "reexport-root helper unexpectedly pruned from LLVM IR:\n{}",
        stdout
    );
}
