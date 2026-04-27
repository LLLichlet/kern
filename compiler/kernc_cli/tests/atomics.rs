use kernc_cli::test_support::{
    build_and_run, compile_source_with_args as compile_with_args,
    emit_llvm_ir_with_args as emit_ir_with_args,
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
fn runs_std_sync_atomic_wrappers() {
    let output = build_and_run(
        "kernc_atomic_wrapper_test",
        r#"
use sync.{
    atomic,
    fence,
    RELAXED,
    ACQUIRE,
    RELEASE,
    ACQ_REL,
    SEQ_CST,
};

fn main() i32 {
    let mut byte = atomic[u8](0);
    if (byte..&.load[ACQUIRE]() != 0) {
        return 1;
    }
    byte..&.store[RELEASE](1);
    if (byte..&.load[ACQUIRE]() != 1) {
        return 2;
    }
    let old_byte = byte..&.exchange[ACQ_REL](0);
    if (old_byte != 1 or byte..&.load[ACQUIRE]() != 0) {
        return 3;
    }
    let byte_cas = byte..&.compare_exchange[ACQ_REL, ACQUIRE](0, 1);
    if (!byte_cas.success or byte_cas.value != 0) {
        return 4;
    }

    let mut counter = atomic[usize](1);
    if (counter..&.fetch_add[SEQ_CST](2) != 1) {
        return 5;
    }
    if (counter..&.load[ACQUIRE]() != 3) {
        return 6;
    }
    if (counter..&.fetch_sub[ACQ_REL](1) != 3) {
        return 7;
    }
    if (counter..&.exchange[SEQ_CST](10) != 2) {
        return 8;
    }
    let cas = counter..&.compare_exchange[ACQ_REL, ACQUIRE](10, 12);
    if (!cas.success or cas.value != 10) {
        return 9;
    }
    let weak = counter..&.compare_exchange_weak[ACQ_REL, ACQUIRE](99, 1);
    if (weak.success or weak.value != 12) {
        return 10;
    }
    if (counter..&.fetch_or[ACQ_REL](3) != 12) {
        return 11;
    }
    if (counter..&.fetch_and[ACQ_REL](7) != 15) {
        return 12;
    }
    if (counter..&.fetch_xor[ACQ_REL](2) != 7) {
        return 13;
    }
    if (counter..&.load[ACQUIRE]() != 5) {
        return 14;
    }

    let mut left = usize.{1};
    let mut right = usize.{2};
    let mut ptr = atomic[*mut usize](left..&);
    if (ptr..&.load[ACQUIRE]() != left..&) {
        return 15;
    }
    ptr..&.store[RELEASE](right..&);
    if (ptr..&.load[ACQUIRE]() != right..&) {
        return 16;
    }
    let old_ptr = ptr..&.exchange[ACQ_REL](left..&);
    if (old_ptr != right..& or ptr..&.load[ACQUIRE]() != left..&) {
        return 17;
    }
    let ptr_cas = ptr..&.compare_exchange[ACQ_REL, ACQUIRE](left..&, right..&);
    if (!ptr_cas.success or ptr_cas.value != left..&) {
        return 18;
    }

    fence[SEQ_CST]();
    let _ = RELAXED;
    return 0;
}
"#,
        &[
            "--module-path",
            "sync=library/std/sync",
            "--runtime-libc",
            "yes",
        ],
    );

    assert!(
        output.status.success(),
        "atomic wrapper binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_atomic_intrinsics_and_fence_with_std_sync_constants() {
    let output = compile_source_with_args(
        r#"
use sync.{MemOrder, RELAXED, ACQUIRE, RELEASE, ACQ_REL, SEQ_CST};

const LOAD_ORDER = MemOrder.{1};

fn main() i32 {
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
        &["--module-path", "sync=library/std/sync"],
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
    let mut value = usize.{0};
    let cas = @atomicCasWeak[usize](value..&, 1, 2, ACQ_REL, ACQUIRE);
    let _ = cas.success;
    let _ = cas.value;
    return 0;
}
"#,
        &["--module-path", "sync=library/std/sync"],
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
const RELEASE = 2;

fn main() i32 {
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

fn main() i32 {
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
