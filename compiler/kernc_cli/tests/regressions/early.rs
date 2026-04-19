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
fn compiles_const_generic_types_and_function_instantiations() {
    let source = r#"
type Array[T, N: usize] = struct {
    data: [N]T,
};

fn id_array[N: usize](arr: [N]i32) [N]i32 {
    return arr;
}

fn main() i32 {
    let wrapped = Array[i32, 4].{ data: [4]i32.{ 1, 2, 3, 4 } };
    let _ = id_array[4](wrapped.data);
    return 0;
}
"#;

    let output = compile_source(source);
    assert_success(&output, "kernc");
}

#[test]
fn infers_direct_const_generic_function_arguments() {
    let source = r#"
fn id_array[N: usize](arr: [N]i32) [N]i32 {
    return arr;
}

fn main() i32 {
    let _ = id_array([4]i32.{ 1, 2, 3, 4 });
    return 0;
}
"#;

    let output = compile_source(source);
    assert_success(&output, "kernc");
}

#[test]
fn supports_computed_const_generic_array_lengths() {
    let source = r#"
type Buf[T, N: usize] = [N + 1]T;

fn main() i32 {
    let _ = Buf[i32, 3].{ 1, 2, 3, 4 };
    return 0;
}
"#;

    let output = compile_source(source);
    assert_success(&output, "kernc");
}

#[test]
fn supports_bool_const_generic_types_and_direct_inference() {
    let source = r#"
type Flag[B: bool] = struct {
    value: bool,
};

fn id_flag[B: bool](flag: Flag[B]) Flag[B] {
    return flag;
}

fn main() i32 {
    let _ = Flag[true and false].{ value: false };
    let _ = id_flag(Flag[true].{ value: true });
    return 0;
}
"#;

    let output = compile_source(source);
    assert_success(&output, "kernc");
}

#[test]
fn supports_payloadless_enum_const_generic_types_and_direct_inference() {
    let source = r#"
type Mode = enum {
    Fast,
    Safe,
};

type Setting[M: Mode] = struct {};

fn id_setting[M: Mode](value: Setting[M]) Setting[M] {
    return value;
}

fn main() i32 {
    let _ = Setting[Mode.Fast].{};
    let _ = id_setting(Setting[Mode.Safe].{});
    return 0;
}
"#;

    let output = compile_source(source);
    assert_success(&output, "kernc");
}

#[test]
fn rejects_payload_carrying_enum_const_generic_parameter_types() {
    let source = r#"
type Rich = enum {
    A: i32,
    B,
};

type Bad[M: Rich] = struct {};
"#;

    let output = compile_source(source);
    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted payload-carrying enum const generic parameters:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("payload-less enum type"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_raw_integer_values_for_enum_const_generics() {
    let source = r#"
type Mode = enum {
    Fast,
    Safe,
};

type Setting[M: Mode] = struct {};

fn main() i32 {
    let _ = Setting[0].{};
    return 0;
}
"#;

    let output = compile_source(source);
    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted a raw integer for an enum const generic:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("must evaluate to a value of enum type `Mode`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("Mode.Fast"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn enum_const_generic_diagnostics_render_variant_names() {
    let source = r#"
type Mode = enum {
    Fast,
    Safe,
};

type Setting[M: Mode] = struct {};

fn takes_fast(value: Setting[Mode.Fast]) void {
    let _ = value;
}

fn main() i32 {
    takes_fast(Setting[Mode.Safe].{});
    return 0;
}
"#;

    let output = compile_source(source);
    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted mismatched enum const generics:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Setting[Mode.Fast]"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("Setting[Mode.Safe]"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_symbolic_bool_const_generic_expressions() {
    let source = r#"
type Flag[B: bool] = struct {
    value: bool,
};

type Negated[B: bool] = Flag[!B];
"#;

    let output = compile_source(source);
    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted symbolic bool const generic expressions:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "const generic argument can only use symbolic computed expressions for integer const parameters"
        ),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("non-integer const parameters such as `bool` may still be passed directly"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_reverse_solving_const_generic_function_arguments() {
    let source = r#"
fn bump[N: usize](arr: [N + 1]i32) [N + 1]i32 {
    return arr;
}

fn main() i32 {
    let _ = bump([4]i32.{ 1, 2, 3, 4 });
    return 0;
}
"#;

    let output = compile_source(source);
    assert!(
        !output.status.success(),
        "kernc unexpectedly reverse-solved const generic arguments:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot infer generic argument(s) `N` for function `bump`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("const generics are inferred only from direct structural matches such as `[N]T`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("does not reverse-solve const expressions like `[N + 1]T`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn emits_inline_attributes_in_llvm_ir() {
    let source = r#"
#[inline]
fn hot_add(lhs: i32, rhs: i32) i32 {
    return lhs + rhs;
}

#[noinline]
fn cold_add(lhs: i32, rhs: i32) i32 {
    if (lhs > rhs) {
        return lhs - rhs;
    }
    if (rhs > lhs) {
        return rhs - lhs;
    }
    return 0;
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
        "expected alwaysinline in LLVM IR for #[inline], got:\n{}",
        stdout
    );
    assert!(
        stdout.contains("noinline"),
        "expected noinline in LLVM IR, got:\n{}",
        stdout
    );
}

#[test]
fn rejects_removed_inline_always_attribute() {
    let source = r#"
#[inline_always]
fn hot_add(lhs: i32, rhs: i32) i32 {
    return lhs + rhs;
}

fn main() i32 {
    return hot_add(1, 2);
}
"#;

    let output = compile_source(source);
    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted removed #[inline_always]:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`#[inline_always]` is not supported"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("use `#[inline]` for forced inlining"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_removed_inline_call_attribute_form() {
    let source = r#"
#[inline(always)]
fn hot_add(lhs: i32, rhs: i32) i32 {
    return lhs + rhs;
}

fn main() i32 {
    return hot_add(1, 2);
}
"#;

    let output = compile_source(source);
    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted removed #[inline(...)]:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`#[inline(...)]` is not supported"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("use marker attributes: `#[inline]` or `#[noinline]`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn propagate_threads_result_error_context_into_generic_calls() {
    let output = build_and_run_source(
        r#"
type Error = enum {
    Oops,
};

fn maybe() ?i32 {
    return .None;
}

fn map_error(_: i32) i32!Error {
    return .{ Err: .Oops };
}

fn check_ok_or() i32!Error {
    let _ = maybe().ok_or(.Oops).!;
    return .{ Ok: 1 };
}

fn check_or_else() i32!Error {
    let base = i32!i32.{ Err: 7 };
    let _ = base.or_else(map_error).!;
    return .{ Ok: 1 };
}

fn main() i32 {
    match (check_ok_or()) {
        .{ Err: .Oops } => {},
        _ => return 1,
    }

    match (check_or_else()) {
        .{ Err: .Oops } => {},
        _ => return 2,
    }

    return 0;
}
"#,
    );

    assert!(
        output.status.success(),
        "propagate inference regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn function_items_work_as_closure_callbacks() {
    let output = build_and_run_source(
        r#"
fn wrap(err: i32) i64 {
    return err as i64 + 1;
}

fn main() i32 {
    let value = i32!i32.{ Err: 41 }.map_err(wrap);
    match (value) {
        .{ Err: err } => {
            if (err != 42) {
                return err as i32;
            }
            return 0;
        },
        .{ Ok: _ } => return 100,
    }
}
"#,
    );

    assert!(
        output.status.success(),
        "function-item callback regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn defaults_emit_llvm_to_raw_stage() {
    let source = r#"
fn main() i32 {
    let mut value = i32.{1};
    value = value + i32.{2};
    return value;
}
"#;

    let output = emit_llvm_ir_with_args("kernc_emit_llvm_raw", source, &[]);
    assert_success(&output, "kernc");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("target triple ="),
        "raw LLVM IR should not be target-configured yet, got:\n{}",
        stdout
    );
    assert!(
        stdout.contains("alloca"),
        "raw LLVM IR should still expose stack allocas before LLVM passes, got:\n{}",
        stdout
    );
}

#[test]
fn emits_verified_llvm_ir_stage_with_target_metadata() {
    let source = r#"
fn main() i32 {
    return 0;
}
"#;

    let output = emit_llvm_ir_stage_with_args("kernc_emit_llvm_verified", "verified", source, &[]);
    assert_success(&output, "kernc");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("target triple ="),
        "verified LLVM IR should include the configured target triple, got:\n{}",
        stdout
    );
    assert!(
        stdout.contains("target datalayout ="),
        "verified LLVM IR should include the configured data layout, got:\n{}",
        stdout
    );
}

#[test]
fn emits_optimized_llvm_ir_stage_after_running_pass_pipeline() {
    let source = r#"
extern fn main() i32 {
    let mut value = i32.{1};
    value = value + i32.{2};
    return value;
}
"#;

    let output =
        emit_llvm_ir_stage_with_args("kernc_emit_llvm_optimized", "optimized", source, &["-O2"]);
    assert_success(&output, "kernc");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("ret i32 3"),
        "optimized LLVM IR should reflect constant folding through the pass pipeline, got:\n{}",
        stdout
    );
    assert!(
        !stdout.contains("alloca"),
        "optimized LLVM IR should not keep the raw stack slot for this simple function, got:\n{}",
        stdout
    );
}

#[test]
fn emits_optimized_llvm_ir_for_multi_cgu_full_lto() {
    let source = r#"
extern fn main() i32 {
    return foo() + bar();
}

extern fn foo() i32 {
    return 1;
}

extern fn bar() i32 {
    return 2;
}
"#;

    let output = emit_llvm_ir_stage_with_args(
        "kernc_emit_llvm_multi_cgu_full_lto",
        "optimized",
        source,
        &["-O2", "--codegen-units", "2", "--lto", "full"],
    );
    assert_success(&output, "kernc");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("target triple ="),
        "optimized multi-CGU full-LTO LLVM IR should be target-configured, got:\n{}",
        stdout
    );
}

#[test]
fn rejects_multi_cgu_emit_llvm_without_full_lto() {
    let source = r#"
extern fn main() i32 {
    return 0;
}
"#;

    let output = emit_llvm_ir_with_args(
        "kernc_emit_llvm_multi_cgu_requires_lto",
        source,
        &["--codegen-units", "2"],
    );
    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted multi-CGU emit-llvm without full LTO:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("`--emit-llvm` with multiple codegen units requires `--lto full`"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn accepts_thin_lto_summary_pipeline() {
    let output = build_and_run(
        "kernc_accepts_thin_lto",
        r#"
#[inline]
fn shared(seed: i32) i32 {
    if (seed > 0) {
        return seed;
    }
    if (seed < 0) {
        return -seed;
    }
    return 0;
}

extern fn left(seed: i32) i32 {
    return shared(seed);
}

extern fn right(seed: i32) i32 {
    return shared(seed);
}

fn main() i32 {
    let sum = left(7) + right(-4);
    return if (sum == 11) 0 else 1;
}
"#,
        &[
            "--codegen-units",
            "2",
            "--lto",
            "thin",
            "--runtime-libc",
            "yes",
        ],
    );
    assert!(
        output.status.success(),
        "thin-LTO summary pipeline binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
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

type ParseError = enum {
    BadToken,
};

type HandshakeError = enum {
    Parse: ParseError,
    RouteRejected,
};

fn compute(ok: bool) usize!HandshakeError {
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
        stdout.contains("@_K4root5TABLE = internal global [4 x i8] c\"\\00\\00\\07\\00\""),
        "expected folded global array initializer in LLVM IR, got:\n{}",
        stdout
    );
    assert!(
        !stdout.contains("@_K4root5TABLE = internal global [4 x i8] zeroinitializer"),
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
fn parent_module_can_access_pub_super_items() {
    let output = compile_source_tree_with_args(
        "kernc_pub_super_parent_access",
        "main.rn",
        &[
            (
                "main.rn",
                r#"
mod inner;

use .inner.parent_only as parent_only_import;

fn main() i32 {
    return inner.parent_only() + parent_only_import();
}
"#,
            ),
            (
                "inner.rn",
                r#"
pub.. fn parent_only() i32 {
    return 0;
}
"#,
            ),
        ],
        &["-c"],
    );

    assert_success(&output, "kernc");
}

#[test]
fn parent_module_can_access_pub_super_reexports() {
    let output = compile_source_tree_with_args(
        "kernc_pub_super_reexport",
        "main.rn",
        &[
            (
                "main.rn",
                r#"
mod middle;

fn main() i32 {
    return middle.shared();
}
"#,
            ),
            (
                "middle.rn",
                r#"
mod leaf;

pub.. use .leaf.shared as shared;
"#,
            ),
            (
                "leaf.rn",
                r#"
pub fn shared() i32 {
    return 0;
}
"#,
            ),
        ],
        &["-c"],
    );

    assert_success(&output, "kernc");
}

#[test]
fn public_reexport_promotes_pub_super_items_to_outer_modules() {
    let output = compile_source_tree_with_args(
        "kernc_pub_super_public_reexport",
        "main.rn",
        &[
            (
                "main.rn",
                r#"
mod middle;

fn main() i32 {
    return middle.shared();
}
"#,
            ),
            (
                "middle.rn",
                r#"
mod leaf;

pub use .leaf.shared as shared;
"#,
            ),
            (
                "leaf.rn",
                r#"
pub.. fn shared() i32 {
    return 0;
}
"#,
            ),
        ],
        &["-c"],
    );

    assert_success(&output, "kernc");
}

#[test]
fn sibling_module_can_access_pub_super_items() {
    let output = compile_source_tree_with_args(
        "kernc_pub_super_sibling_access",
        "main.rn",
        &[
            (
                "main.rn",
                r#"
pub mod left;
mod right;

fn main() i32 {
    return right.value();
}
"#,
            ),
            (
                "left.rn",
                r#"
pub.. fn helper() i32 {
    return 0;
}
"#,
            ),
            (
                "right.rn",
                r#"
use ..left.helper as helper;

pub fn value() i32 {
    return helper();
}
"#,
            ),
        ],
        &["-c"],
    );

    assert_success(&output, "kernc");
}

#[test]
fn descendant_module_can_access_pub_super_items() {
    let output = compile_source_tree_with_args(
        "kernc_pub_super_descendant_access",
        "main.rn",
        &[
            (
                "main.rn",
                r#"
pub mod left;

fn main() i32 {
    return left.deep.value();
}
"#,
            ),
            (
                "left.rn",
                r#"
pub mod deep;

pub.. fn helper() i32 {
    return 0;
}
"#,
            ),
            (
                "deep.rn",
                r#"
use ..helper;

pub fn value() i32 {
    return helper();
}
"#,
            ),
        ],
        &["-c"],
    );

    assert_success(&output, "kernc");
}

#[test]
fn grandparent_module_cannot_access_pub_super_items() {
    let output = compile_source_tree_with_args(
        "kernc_pub_super_grandparent_rejected",
        "main.rn",
        &[
            (
                "main.rn",
                r#"
pub mod outer;

fn main() i32 {
    return outer.mid.helper();
}
"#,
            ),
            (
                "outer.rn",
                r#"
pub mod mid;
"#,
            ),
            (
                "mid.rn",
                r#"
pub.. fn helper() i32 {
    return 0;
}
"#,
            ),
        ],
        &["-c"],
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted grandparent access to pub.. item:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("module has no visible member `helper`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn module_outside_parent_subtree_cannot_access_pub_super_items() {
    let output = compile_source_tree_with_args(
        "kernc_pub_super_outside_subtree_rejected",
        "main.rn",
        &[
            (
                "main.rn",
                r#"
pub mod outer;
mod peer;

fn main() i32 {
    return peer.value();
}
"#,
            ),
            (
                "outer.rn",
                r#"
pub mod mid;
"#,
            ),
            (
                "mid.rn",
                r#"
pub.. fn helper() i32 {
    return 0;
}
"#,
            ),
            (
                "peer.rn",
                r#"
use ..outer.mid.helper as helper;

pub fn value() i32 {
    return helper();
}
"#,
            ),
        ],
        &["-c"],
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted access outside the parent subtree:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Symbol `helper` is not visible from this module"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn sibling_module_can_access_pub_package_items() {
    let output = compile_source_tree_with_args(
        "kernc_pub_package_sibling_access",
        "main.rn",
        &[
            (
                "main.rn",
                r#"
pub mod left;
mod right;

fn main() i32 {
    return right.value();
}
"#,
            ),
            (
                "left.rn",
                r#"
pub/ fn helper() i32 {
    return 0;
}
"#,
            ),
            (
                "right.rn",
                r#"
use ..left.helper as helper;

pub fn value() i32 {
    return helper();
}
"#,
            ),
        ],
        &["-c"],
    );

    assert_success(&output, "kernc");
}

#[test]
fn package_root_paths_work_in_use_type_and_expr_positions() {
    let output = compile_source_tree_with_args(
        "kernc_package_root_paths",
        "main.rn",
        &[
            (
                "main.rn",
                r#"
pub mod util;

use /util.answer;
type Alias = /util.Kind;

fn main() i32 {
    let kind = /util.kind();
    match (kind) {
        .Root => return answer(),
    }
}
"#,
            ),
            (
                "util.rn",
                r#"
pub type Kind = enum {
    Root,
};

pub fn kind() /util.Kind {
    return /util.Kind.Root;
}

pub fn answer() i32 {
    return 0;
}
"#,
            ),
        ],
        &["-c"],
    );

    assert_success(&output, "kernc");
}

#[test]
fn local_use_can_shadow_visible_names_inside_nested_blocks() {
    let output = build_and_run_source(
        r#"
fn helper() i32 {
    return 1;
}

fn other() i32 {
    return 2;
}

fn main() i32 {
    let before = helper();
    {
        use .other as helper;
        let inside = helper();
        if (inside != 2) {
            return 10;
        }
    }
    return before;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(1),
        "program exited unexpectedly:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn local_use_enables_following_type_paths_inside_blocks() {
    let output = compile_source(
        r#"
type Answer = struct {
    value: i32,
};

fn make() i32 {
    return 7;
}

fn main() i32 {
    {
        use .{Answer, make};
        let size = @sizeOf[Answer]();
        if (size == 0) {
            return 10;
        }
        return make();
    }
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
fn local_use_does_not_leak_outside_its_block() {
    let output = compile_source(
        r#"
fn helper() i32 {
    return 0;
}

fn main() i32 {
    {
        use .helper as local_helper;
        let _ = local_helper;
    }
    return local_helper();
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
        stderr.contains("use of undeclared identifier `local_helper`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn bare_use_path_no_longer_falls_back_to_local_package_root() {
    let output = compile_source_tree_with_args(
        "kernc_external_use_no_local_fallback",
        "main.rn",
        &[
            (
                "main.rn",
                r#"
mod util;

use util.answer;

fn main() i32 {
    return answer();
}
"#,
            ),
            (
                "util.rn",
                r#"
pub fn answer() i32 {
    return 0;
}
"#,
            ),
        ],
        &["-c"],
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly resolved a bare external-style import against the local package root:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unresolved external import root `util`"),
        "unexpected stderr:\n{}",
        stderr
    );
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

type Holder = struct {};

impl Holder {
    fn section_kind(flag: bool) ?Kind {
        if (flag) {
            return .{ Some: Kind.Section };
        }
        return .None;
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
