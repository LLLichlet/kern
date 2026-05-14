use kernc_cli::test_support::{
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
fn compiles_atomic_intrinsics_and_fence_with_base_sync_constants() {
    let output = compile_source_with_args(
        r#"
use sync.{MemOrder, atomic, fence, RELAXED, ACQUIRE, RELEASE, ACQ_REL, SEQ_CST};

const LOAD_ORDER: MemOrder = ACQUIRE;
const LOAD_CODE: u8 = LOAD_ORDER;

fn main() i32 {
    let mut value = 0usize;
    let _ = @atomicLoad[usize](value.&, ACQUIRE);
    @atomicStore[usize](value..&, 1, RELEASE);
    let _ = @atomicLoad[usize](value.&, LOAD_CODE);

    let cas = @atomicCas[usize](value..&, 1, 2, ACQ_REL, ACQUIRE);
    let _ = cas.success;
    let _ = cas.value;

    let _ = @atomicRmwAdd[usize](value..&, 1, RELAXED);
    let _ = @atomicRmwNand[usize](value..&, 255, ACQ_REL);
    let _ = @atomicRmwXor[usize](value..&, 4, ACQ_REL);
    let _ = @atomicRmwUMax[usize](value..&, 8, SEQ_CST);

    let mut ptr = value..&;
    let _ = @atomicXchg[&mut usize](ptr..&, value..&, SEQ_CST);

    let mut cell = atomic[usize](0);
    cell..&.store[RELEASE](1);
    let _ = cell.&.load[LOAD_ORDER]();
    let _ = cell..&.compare_exchange[ACQ_REL, ACQUIRE](1, 2);

    @fence(RELEASE);
    fence[SEQ_CST]();
    return 0;
}
"#,
        &["--module-path", "sync=library/base/sync"],
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_extern_enum_const_generic_wrappers() {
    let output = compile_source_with_args(
        r#"
use sync.{MemOrder, atomic, ACQUIRE, RELEASE};

fn load_acquire(cell: &sync.Atomic[usize]) usize {
    return cell.load[ACQUIRE]();
}

fn main() i32 {
    let mut cell = atomic[usize](7);
    cell..&.store[RELEASE](9);
    let value = load_acquire(cell.&);
    if (value != 9) return 1;
    return 0;
}
"#,
        &["--module-path", "sync=library/base/sync"],
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

fn main() i32 {
    let mut value = 0usize;
    let cas = @atomicCasWeak[usize](value..&, 1, 2, ACQ_REL, ACQUIRE);
    let _ = cas.success;
    let _ = cas.value;
    return 0;
}
"#,
        &["--module-path", "sync=library/base/sync"],
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
const RELEASE: u8 = 2;

fn main() i32 {
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
const RELEASE: u8 = 2;

fn main() i32 {
    let mut value = 0usize;
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
fn rejects_invalid_base_sync_ordering_after_const_generic_substitution() {
    let output = compile_source_with_args(
        r#"
use sync.{atomic, RELEASE};

fn main() i32 {
    let mut value = atomic[usize](0);
    let _ = value.&.load[RELEASE]();
    return 0;
}
"#,
        &["--module-path", "sync=library/base/sync"],
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
const ACQUIRE: u8 = 1;

fn main() i32 {
    let mut value = 0u128;
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
