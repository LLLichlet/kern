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
fn compiles_base_sync_from_base_bundle() {
    let output = compile_source_with_args(
        r#"
use base.sync.{atomic, SEQ_CST};

fn main() i32 {
    let mut counter = atomic[usize](0);
    counter..&.store[SEQ_CST](1);
    return counter..&.load[SEQ_CST]() as i32 - 1;
}
"#,
        &["--library-bundle", "base"],
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn runs_base_sync_atomic_wrappers() {
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

    let mut flag = atomic[bool](false);
    if (flag..&.load[ACQUIRE]()) {
        return 19;
    }
    flag..&.store[RELEASE](true);
    if (!flag..&.load[ACQUIRE]()) {
        return 20;
    }
    if (!flag..&.exchange[ACQ_REL](false)) {
        return 21;
    }
    if (flag..&.load[ACQUIRE]()) {
        return 22;
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
            "sync=library/base/sync",
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
fn runs_base_sync_spin_lock_helpers() {
    let output = build_and_run(
        "kernc_spin_lock_test",
        r#"
use sync.spin_lock;

type Pair = struct {
    left: i32,
    right: i32,
};

fn main() i32 {
    let mut counter = spin_lock[i32](5);
    if (counter..&.is_locked()) {
        return 1;
    }

    let first = counter..&.with_lock[i32](.[](value: *mut i32) i32 {
        value.* += 1;
        return value.*;
    });
    if (first != 6) {
        return 2;
    }
    if (counter..&.is_locked()) {
        return 3;
    }

    let next = match (counter..&.try_with_lock[i32](.[](value: *mut i32) i32 {
        value.* += 4;
        return value.*;
    })) {
        .{ Some: value } => value,
        .None => return 4,
    };
    if (next != 10) {
        return 5;
    }

    let reentrant_blocked = counter..&.with_lock[bool](.[lock = counter..&](value: *mut i32) bool {
        if (!lock.is_locked()) {
            return false;
        }
        let nested = lock.try_with_lock[i32](.[](inner: *mut i32) i32 {
            inner.* = 99;
            return inner.*;
        });
        match (nested) {
            .None => {},
            .{ Some: _ } => return false,
        }
        value.* += 1;
        return true;
    });
    if (!reentrant_blocked) {
        return 6;
    }

    let final_counter = counter..&.with_lock[i32](.[](value: *mut i32) i32 {
        return value.*;
    });
    if (final_counter != 11) {
        return 7;
    }

    let mut pair = spin_lock[Pair](Pair.{ left: 2, right: 3 });
    let total = pair..&.with_lock[i32](.[](value: *mut Pair) i32 {
        value.left *= 5;
        value.right += 7;
        return value.left + value.right;
    });
    if (total != 20) {
        return 8;
    }

    return 0;
}
"#,
        &[
            "--module-path",
            "sync=library/base/sync",
            "--runtime-libc",
            "yes",
        ],
    );

    assert!(
        output.status.success(),
        "spin lock binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn runs_base_sync_once_helpers() {
    let output = build_and_run(
        "kernc_once_test",
        r#"
use sync.once;

fn main() i32 {
    let mut gate = once();
    if (gate..&.is_completed()) {
        return 1;
    }

    let mut hits = i32.{0};
    gate..&.call_once(.[hits = hits..&]() void {
        hits.* += 1;
    });
    if (!gate..&.is_completed()) {
        return 2;
    }
    if (hits != 1) {
        return 3;
    }

    gate..&.call_once(.[hits = hits..&]() void {
        hits.* += 10;
    });
    if (hits != 1) {
        return 4;
    }

    let ran_after_done = gate..&.try_call_once(.[hits = hits..&]() void {
        hits.* += 100;
    });
    if (ran_after_done or hits != 1) {
        return 5;
    }

    let mut second = once();
    let won = second..&.try_call_once(.[hits = hits..&]() void {
        hits.* += 7;
    });
    if (!won or hits != 8) {
        return 6;
    }
    if (!second..&.is_completed()) {
        return 7;
    }
    let lost = second..&.try_call_once(.[hits = hits..&]() void {
        hits.* += 70;
    });
    if (lost or hits != 8) {
        return 8;
    }

    return 0;
}
"#,
        &[
            "--module-path",
            "sync=library/base/sync",
            "--runtime-libc",
            "yes",
        ],
    );

    assert!(
        output.status.success(),
        "once binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_atomic_intrinsics_and_fence_with_base_sync_constants() {
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
    let mut value = usize.{0};
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
