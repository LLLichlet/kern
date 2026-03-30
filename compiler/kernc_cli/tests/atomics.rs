mod support;

use support::{
    compile_source_with_args as compile_with_args, emit_llvm_ir_with_args as emit_ir_with_args,
};

fn compile_source_with_args(source: &str, extra_args: &[&str]) -> std::process::Output {
    compile_with_args("kernc_atomic_test", source, extra_args)
}

fn emit_llvm_ir_with_args(source: &str, extra_args: &[&str]) -> std::process::Output {
    emit_ir_with_args("kernc_atomic_test", source, extra_args)
}

fn compile_source(source: &str) -> std::process::Output {
    compile_source_with_args(source, &[])
}

#[test]
fn compiles_atomic_intrinsics_and_fence_with_std_sync_constants() {
    let output = compile_source_with_args(
        r#"
use sync.{MemOrder, RELAXED, ACQUIRE, RELEASE, ACQ_REL, SEQ_CST};

const LOAD_ORDER = MemOrder.{1};

extern fn main(args: [][]u8) i32 {
    let mut value = usize.{0};
    let _ = @atomicLoad[usize](value.&, ACQUIRE);
    @atomicStore[usize](value..&, 1, RELEASE);
    let _ = @atomicLoad[usize](value.&, LOAD_ORDER);

    let cas = @atomicCas[usize](value..&, 1, 2, ACQ_REL, ACQUIRE);
    let _ = cas.success;
    let _ = cas.value;

    let _ = @atomicRmwAdd[usize](value..&, 1, RELAXED);
    let _ = @atomicRmwNand[usize](value..&, 255, ACQ_REL);
    let _ = @atomicRmwXor[usize](value..&, 4, ACQ_REL);
    let _ = @atomicRmwUMax[usize](value..&, 8, SEQ_CST);

    let mut ptr = value..&;
    let _ = @atomicXchg[*mut usize](ptr..&, value..&, SEQ_CST);

    @fence(RELEASE);
    return 0;
}
"#,
        &["-M", "sync=library/std/sync"],
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn emits_weak_cmpxchg_for_atomic_cas_weak() {
    let output = emit_llvm_ir_with_args(
        r#"
use sync.{ACQUIRE, ACQ_REL};

extern fn main(args: [][]u8) i32 {
    let mut value = usize.{0};
    let cas = @atomicCasWeak[usize](value..&, 1, 2, ACQ_REL, ACQUIRE);
    let _ = cas.success;
    let _ = cas.value;
    return 0;
}
"#,
        &["-M", "sync=library/std/sync"],
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("cmpxchg weak"),
        "expected weak cmpxchg in LLVM IR, got:\n{}",
        stdout
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("cmpxchg weak"),
        "expected LLVM IR on stdout, got stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_non_constant_atomic_ordering() {
    let output = compile_source(
        r#"
const RELEASE = 2;

extern fn main(args: [][]u8) i32 {
    let order = RELEASE;
    @fence(order);
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
        stderr.contains("not a compile-time constant"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_invalid_atomic_ordering_for_load() {
    let output = compile_source(
        r#"
const RELEASE = 2;

extern fn main(args: [][]u8) i32 {
    let mut value = usize.{0};
    let _ = @atomicLoad[usize](value.&, RELEASE);
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
        stderr.contains("not valid for `load order`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_atomic_widths_that_need_runtime_helpers() {
    let output = compile_source_with_args(
        r#"
const ACQUIRE = 1;

extern fn main(args: [][]u8) i32 {
    let mut value = u128.{0};
    let _ = @atomicLoad[u128](value.&, ACQUIRE);
    return 0;
}
"#,
        &["--target", "i686-unknown-linux-gnu"],
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("supports lock-free atomics only up to 64 bits"),
        "unexpected stderr:\n{}",
        stderr
    );
}
