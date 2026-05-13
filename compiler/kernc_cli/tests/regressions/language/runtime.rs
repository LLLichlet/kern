use super::*;
#[test]
fn rejects_returning_capturing_closure_as_fn_pointer() {
    let output = compile_source(
        r#"
fn make() &Fn(i32) i32 {
    let base = 7i32;
    return [base](x: i32) i32 {
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
        stderr.contains("cannot return a capturing closure as `&Fn(i32) i32`"),
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
fn make() &Fn(i32) i32 {
    let base = 7i32;
    [base](x: i32) i32 {
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
        stderr.contains("cannot return a capturing closure as `&Fn(i32) i32`"),
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
fn make() &Fn(i32) i32 {
    return [](x: i32) i32 {
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
fn use_closure(cb: &Fn() i32) i32 {
    return cb();
}

fn use_mut_closure(cb: &mut Fn() void) void {
    cb();
}

fn main() i32 {
    let mut calls = 0i32;
    let value = use_closure([ptr = calls..&]() i32 {
        ptr.* += 1;
        return 77;
    });
    if (value != 77) {
        return 1;
    }
    if (calls != 1) {
        return 2;
    }

    let mut counter = 0i32;
    let mut closure = [ptr = counter..&]() void {
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
trait Score {
    fn value() i32;
};

struct Buf[N: usize] {};

impl[N: usize] &Buf[N]: Score {
    fn value() i32 {
        return 1;
    }
}

impl &Buf[4]: Score {
    fn value() i32 {
        return 2;
    }
}

fn main() i32 {
    let buf = Buf[4].{};
    let score = (buf.& as &Score);
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
trait Score[N: usize] {
    fn value() i32;
};

struct X {};

impl[N: usize] &X: Score[N] {
    fn value() i32 {
        return 1;
    }
}

impl &X: Score[4] {
    fn value() i32 {
        return 2;
    }
}

fn main() i32 {
    let x = X.{};
    let score = (x.& as &Score[4]);
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
trait Family[N: usize] {
    fn value() i32;
};

struct X {};

impl &X: Family[1] {
    fn value() i32 {
        return 11;
    }
}

impl &X: Family[2] {
    fn value() i32 {
        return 22;
    }
}

fn call[N: usize](x: &X) i32
    where &X: Family[N],
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
trait Score[N: usize] {
    fn value() i32;
};

struct X {};

impl[N: usize] &X: Score[N] {
    fn value() i32 {
        return N as i32;
    }
}

fn main() i32 {
    let x = X.{};
    let score = (x.& as &Score[4]);
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
struct Buffer {
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
    let mut arr = [5]u8.{ b'a', b'b', b'c', b'd', b'e' };
    arr.[1] = b'x';
    if (arr.[1] != b'x') {
        return 1;
    }

    let view = arr..&[1 .. 4];
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
struct Pair[T,] {
    left: T,
    right: T,
};

enum Choice {
    A,
    B,
};

trait Ops {
    fn run(_: i32, _: i32) i32;
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
fn compiles_single_element_array_literals_without_trailing_comma() {
    let output = build_and_run_source(
        r#"
static STATIC_ONE = [1]u8.{ 7 };

fn take(values: [1]u8) i32 {
    if (values.[0] != 9u8) {
        return 1;
    }
    return 0;
}

fn main() i32 {
    let typed = [1]u8.{ 8 };
    if (STATIC_ONE.[0] != 7u8) {
        return 2;
    }
    if (typed.[0] != 8u8) {
        return 3;
    }
    return take(.{ 9 });
}
"#,
    );

    assert!(
        output.status.success(),
        "single-element array literal binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_local_static_struct_initializers() {
    let output = build_and_run_source(
        r#"
struct Write {
    serial: bool,
};

static mut SELECTED = 0usize as &mut Write;

fn init() void {
    static mut main_writer = Write.{ serial: false };
    static mut debug_writer = Write.{ serial: true };
    if (debug_writer.serial) {
        SELECTED = debug_writer..&;
    } else {
        SELECTED = main_writer..&;
    }
}

fn main() i32 {
    init();
    if (SELECTED.*.serial) {
        return 0;
    }
    return 1;
}
"#,
    );

    assert!(
        output.status.success(),
        "local static struct initializer binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn runs_defer_after_return_value_evaluation() {
    let output = build_and_run_source(
        r#"
struct Guard {
    ptr: &mut i32,
};

impl &mut Guard {
    pub fn deinit() void {
        self.ptr.* = 2;
    }
}

fn read_before_defer() i32 {
    let mut state = 1i32;
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
struct Guard {
    ptr: &mut i32,
};

impl &mut Guard {
    pub fn deinit() void {
        self.ptr.* = 2;
    }
}

fn read_block_before_defer() i32 {
    return {
        let mut state = 1i32;
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
struct Push {
    ptr: &mut i32,
    digit: i32,
};

impl &mut Push {
    pub fn deinit() void {
        self.ptr.* = self.ptr.* * 10 + self.digit;
    }
}

fn main() i32 {
    let mut state = 0i32;
    let value = {
        let mut first = Push.{ ptr: state..&, digit: 1 };
        let mut second = Push.{ ptr: state..&, digit: 2 };
        defer first..&.deinit();
        defer second..&.deinit();
        7i32
    };

    if (value != 7i32) {
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
enum Result[T, E] {
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
            let _ = 0i32;
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
enum Result[T, E] {
    Ok: T,
    Err: E,
};

fn expect_ok[T, E](value: Result[T, E]) T {
    match (value) {
        .{ Ok: payload } => return payload,
        .{ Err: _ } => {
            return match (0i32) {
                0i32 => @trap(),
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
fn compiles_breakpoint_in_let_initializer_without_ice() {
    let output = compile_source(
        r#"
fn main() i32 {
    let x = @breakpoint();
    let _ = x;
    return 0;
}
"#,
    );

    assert_success(&output, "kernc");
}

#[test]
fn compiles_returning_breakpoint_from_void_function_without_ice() {
    let output = compile_source(
        r#"
fn stop() void {
    return @breakpoint();
}

fn main() i32 {
    stop();
    return 0;
}
"#,
    );

    assert_success(&output, "kernc");
}

#[test]
fn compiles_void_fence_in_let_initializer_without_ice() {
    let output = compile_source_with_args(
        "kernc_breakpoint_fence_runtime_regression",
        r#"
use sync.SEQ_CST;

fn main() i32 {
    let x = @fence(SEQ_CST);
    let _ = x;
    return 0;
}
"#,
        &["--module-path", "sync=library/base/sync"],
    );

    assert_success(&output, "kernc");
}

#[test]
fn compiles_void_memmove_in_let_initializer_without_ice() {
    let output = compile_source(
        r#"
fn main() i32 {
    let mut buf = [4]u8.{ 0, 1, 2, 3 };
    let x = @memmove(buf.[1]..& as &mut u8, buf.[0].& as &u8, 3);
    let _ = x;
    return 0;
}
"#,
    );

    assert_success(&output, "kernc");
}

#[test]
fn compiles_inline_asm_in_let_initializer_without_ice() {
    let output = compile_source(
        r#"
fn main() i32 {
    let x = @asm(.{
        asm: "nop",
        volatile: true,
    });
    let _ = x;
    return 0;
}
"#,
    );

    assert_success(&output, "kernc");
}

#[test]
fn compiles_void_effect_intrinsics_in_void_argument_position_without_ice() {
    let output = compile_source(
        r#"
fn consume(value: void) void {
    let _ = value;
}

fn main() i32 {
    let mut buf = [4]u8.{ 0, 1, 2, 3 };
    consume(@breakpoint());
    consume(@memcpy(buf.[1]..& as &mut u8, buf.[0].& as &u8, 3));
    consume(@asm(.{
        asm: "nop",
        volatile: true,
    }));
    return 0;
}
"#,
    );

    assert_success(&output, "kernc");
}

#[test]
fn compiles_returning_atomic_store_from_void_function_without_ice() {
    let output = compile_source_with_args(
        "kernc_breakpoint_atomic_runtime_regression",
        r#"
use sync.SEQ_CST;

fn store(ptr: &mut usize) void {
    return @atomicStore[usize](ptr, 1, SEQ_CST);
}

fn main() i32 {
    let mut value = 0usize;
    store(value..&);
    return 0;
}
"#,
        &["--module-path", "sync=library/base/sync"],
    );

    assert_success(&output, "kernc");
}

#[test]
fn runs_while_loop_after_explicit_init_and_post_statements() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let mut phase = 0i32;

    phase += 2i32;
    while (phase < 3i32) {
        phase += 1i32;
        phase += 10i32;
    }

    return phase - 13i32;
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
fn runs_while_with_break_and_continue() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let mut i = 0i32;
    let mut sum = 0i32;

    while (i < 8i32) {
        i += 1i32;
        if (i == 3i32) {
            continue;
        }
        if (i == 7i32) {
            break;
        }
        sum += i;
    }

    return sum - 18i32;
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
fn runs_iterator_for_over_slice() {
    let output = build_and_run_source(
        r#"
use base.coll.Iterator;

fn main() i32 {
    let values = [4]i32.{ 2, 4, 6, 8 };
    let mut sum = 0i32;

    for (item: values.&[0 .. 4].iter()) {
        sum += item;
    }

    return sum - 20i32;
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
fn rejects_malformed_iterator_loop_header() {
    let output = compile_source(
        r#"
fn main() i32 {
    let values = [3]i32.{ 1, 2, 3 };
    for (item values.&[0 .. 3].iter()) {
    }
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
fn lowers_const_inline_asm_volatile_flag_for_output_asm() {
    let output = emit_llvm_ir_with_args(
        "kernc_inline_asm_const_volatile_ir",
        r#"
const VOL = true;

fn main() i32 {
    let mut out = 0usize;
    @asm(.{
        asm: "mov {}, 7",
        outputs: .{ rax: out..& },
        volatile: VOL,
    });
    return out as i32;
}
"#,
        &[],
    );

    assert_success(&output, "kernc inline asm const volatile");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("asm sideeffect"),
        "expected sideeffect inline asm in LLVM IR:\n{}",
        stdout
    );
}

#[test]
fn rejects_non_constant_inline_asm_volatile_flag() {
    let output = compile_source(
        r#"
fn main() i32 {
    let vol = true;
    let mut out = 0usize;
    @asm(.{
        asm: "mov {}, 7",
        outputs: .{ rax: out..& },
        volatile: vol,
    });
    return out as i32;
}
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted non-constant @asm volatile:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not a compile-time constant"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_array_inline_asm_templates() {
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
        "kernc unexpectedly accepted array @asm template syntax:\nstdout:\n{}\nstderr:\n{}",
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

#[test]
fn test_mode_collects_cases_and_invokes_each_case_by_private_protocol() {
    let metadata_path = unique_temp_path("kernc_test_mode_cases", "cases");
    let metadata_arg = metadata_path.to_string_lossy().into_owned();
    let args = vec![
        "--test-mode".to_string(),
        "--test-metadata-output".to_string(),
        metadata_arg,
        "--runtime-entry".to_string(),
        "rt".to_string(),
        "--module-path".to_string(),
        format!("base={}", repo_root().join("library/base").display()),
    ];
    let (source_path, executable_path) = build_temp_program_with_outputs(
        "kernc_test_mode_cases",
        r#"
#[if(test)]
fn enabled_only_in_test_mode() i32 {
    return 0;
}

#[test]
fn alpha() i32 {
    return enabled_only_in_test_mode();
}

mod nested {
    #[test]
    fn beta(argc: i32, argv: &&u8) i32 {
        if (argc != 2) {
            return 2;
        }
        if (argv.*.* != b'f') {
            return 3;
        }
        if ((argv + 1usize).*.* != b's') {
            return 4;
        }
        return 0;
    }
}
"#,
        &args,
    );

    let manifest = std::fs::read_to_string(&metadata_path).unwrap();
    assert_eq!(manifest, "version=1\ncase=0\talpha\ncase=1\tnested::beta\n");

    let alpha = run_program_with_args(&executable_path, &["--kern-test-case", "0"]);
    assert_eq!(
        alpha.status.code(),
        Some(0),
        "alpha test case failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&alpha.stdout),
        String::from_utf8_lossy(&alpha.stderr)
    );

    let beta = run_program_with_args(
        &executable_path,
        &["--kern-test-case", "1", "first", "second"],
    );
    assert_eq!(
        beta.status.code(),
        Some(0),
        "beta test case failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&beta.stdout),
        String::from_utf8_lossy(&beta.stderr)
    );

    let _ = std::fs::remove_file(source_path);
    let _ = std::fs::remove_file(executable_path);
    let _ = std::fs::remove_file(metadata_path);
}

#[test]
fn test_mode_rejects_non_i32_test_return_type() {
    let metadata_path = unique_temp_path("kernc_bad_test_signature", "cases");
    let metadata_arg = metadata_path.to_string_lossy().into_owned();
    let output = compile_source_with_args(
        "kernc_bad_test_signature",
        r#"
#[test]
fn bad() void {}
"#,
        &[
            "--test-mode",
            "--test-metadata-output",
            &metadata_arg,
            "--runtime-entry",
            "rt",
            "--module-path",
            "base=library/base",
        ],
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted invalid test signature:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`#[test]` function must return `i32`"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains(
            "legal test forms are `fn name() i32` and `fn name(argc: i32, argv: &&u8) i32`"
        ),
        "unexpected stderr:\n{}",
        stderr
    );

    let _ = std::fs::remove_file(metadata_path);
}

#[test]
fn test_condition_is_false_outside_test_mode() {
    let output = compile_source(
        r#"
#[if(test)]
fn test_only() i32 {
    return 0;
}

fn main() i32 {
    return test_only();
}
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly enabled #[if(test)] outside test mode:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("use of undeclared identifier `test_only`"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
