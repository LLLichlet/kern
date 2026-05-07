use super::*;

#[test]
fn builds_and_runs_simd_masked_memory_ops() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let mut data = [4]i32.{ 10, 20, 30, 40 };
    let mask = boolx4.{ true, false, true, false };
    let fallback = i32x4.{ 1, 2, 3, 4 };
    let loaded = @simdMaskedLoad[i32x4](data.[0]..&, mask, fallback, 4);
    @simdMaskedStore(data.[0]..&, mask, loaded + i32x4.{ 5, 5, 5, 5 }, 4);

    let idx = [4]usize.{ 3, 99, 0, 99 };
    let gathered = @simdMaskedGather[i32x4](data.[0]..&, idx.[0].&, mask, i32x4.{ 7, 8, 9, 10 });
    @simdMaskedScatter(data.[0]..&, idx.[0].&, mask, gathered + i32x4.{ 1, 1, 1, 1 });

    return data.[0] + data.[1] + data.[2] + data.[3];
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(112),
        "hosted SIMD binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn builds_and_runs_u8x16_scan_style_masks() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let ws = [16]u8.{ b' ', b'\n', b'\r', b'\t', b' ', b' ', b'\t', b'\n', b' ', b' ', b' ', b'\r', b'\t', b' ', b' ', b'X' };
    let digits = [16]u8.{ b'0', b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'1', b'2', b'3', b'4', b'5', b'X' };

    let ws_chunk = @simdLoad[u8x16](ws.[0].&, 1);
    let ws_mask =
        (ws_chunk == @simdSplat[u8x16](b' ')) |
        (ws_chunk == @simdSplat[u8x16](b'\n')) |
        (ws_chunk == @simdSplat[u8x16](b'\r')) |
        (ws_chunk == @simdSplat[u8x16](b'\t'));
    let ws_non_mask = @simdBitmask(!ws_mask);
    if (@simdAll(ws_mask)) {
        return 101;
    }
    if (ws_non_mask != usize.{0x8000}) {
        return 102;
    }
    if (@ctz(ws_non_mask) != usize.{15}) {
        return 103;
    }

    let digit_chunk = @simdLoad[u8x16](digits.[0].&, 1);
    let digit_mask =
        (digit_chunk >= @simdSplat[u8x16](b'0')) &
        (digit_chunk <= @simdSplat[u8x16](b'9'));
    let digit_non_mask = @simdBitmask(!digit_mask);
    if (@simdAll(digit_mask)) {
        return 104;
    }
    if (digit_non_mask != usize.{0x8000}) {
        return 105;
    }
    if (@ctz(digit_non_mask) != usize.{15}) {
        return 106;
    }

    return 55;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(55),
        "hosted SIMD binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn emits_ir_for_simd_bitmask() {
    let output = emit_llvm_ir(
        r#"
fn first_true(mask: boolx16) usize {
    return @ctz(@simdBitmask(mask));
}

fn main() i32 {
    let a = u8x16.{ 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15 };
    let mask = a == @simdSplat[u8x16](7);
    return first_true(mask) as i32;
}
"#,
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let ir = String::from_utf8_lossy(&output.stdout);
    assert!(
        ir.contains("bitcast <16 x i1>"),
        "expected packed SIMD mask bitcast in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("zext i16"),
        "expected packed SIMD mask zero-extension in LLVM IR:\n{}",
        ir
    );
}

#[test]
fn rejects_simd_bitmask_that_does_not_fit_in_usize() {
    let output = compile_source(
        r#"
fn main() i32 {
    let mask = @simdSplat[boolx128](true);
    let _ = @simdBitmask(mask);
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
        stderr.contains("fit in `usize`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn emits_ir_for_simd_shuffle_load_store_and_target_features() {
    let base_arg = format!("base={}", resolve_base_path().display());
    let output = emit_ir_with_args(
        "kernc_simd_test",
        r#"
use base.mem;

#[target_feature("avx2,fma")]
fn remix(ptr: &mut f32) f32 {
    let a = @simdLoad[f32x4](ptr, 4);
    let b = @simdLoad[f32x4](ptr + usize.{4}, 4);
    let mixed = @simdShuffle(a, b, [4]u32.{ 0, 5, 2, 7 });
    @simdStore(ptr, mixed, 4);
    return @simdReduceAdd(mixed);
}

fn main() i32 {
    let mut data = [8]f32.{ 1.0, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0 };
    return remix(data.[0]..&) as i32;
}
"#,
        &["--module-path", base_arg.as_str()],
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let ir = String::from_utf8_lossy(&output.stdout);
    assert!(
        ir.contains("shufflevector"),
        "expected SIMD shuffle in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("load <4 x float>"),
        "expected SIMD load in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("store <4 x float>"),
        "expected SIMD store in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("target-features"),
        "expected target feature attribute in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("+avx2,+fma"),
        "expected normalized target feature list in LLVM IR:\n{}",
        ir
    );
}

#[test]
fn emits_ir_for_simd_masked_memory_ops() {
    let output = emit_llvm_ir(
        r#"
fn main() i32 {
    let mut data = [4]f32.{ 1.0, 2.0, 3.0, 4.0 };
    let mask = boolx4.{ true, false, true, false };
    let idx = [4]usize.{ 3, 9, 0, 9 };
    let partial = @simdMaskedLoad[f32x4](data.[0]..&, mask, f32x4.{ 0.0, 0.0, 0.0, 0.0 }, 4);
    @simdMaskedStore(data.[0]..&, mask, partial, 4);
    let gathered = @simdMaskedGather[f32x4](data.[0]..&, idx.[0].&, mask, f32x4.{ -1.0, -1.0, -1.0, -1.0 });
    @simdMaskedScatter(data.[0]..&, idx.[0].&, mask, gathered);
    return partial.[0] as i32;
}
"#,
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let ir = String::from_utf8_lossy(&output.stdout);
    assert!(
        ir.contains("br i1"),
        "expected masked lane branches in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("load float"),
        "expected scalar masked loads in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("store float"),
        "expected scalar masked stores in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("load i64"),
        "expected masked gather/scatter index loads in LLVM IR:\n{}",
        ir
    );
}

#[test]
fn emits_ir_for_simd_gather_and_scatter() {
    let output = emit_llvm_ir(
        r#"
fn main() i32 {
    let mut data = [8]f32.{ 1.0, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0 };
    let idx = [4]usize.{ 7, 0, 5, 2 };
    let gathered = @simdGather[f32x4](data.[0]..&, idx.[0].&);
    @simdScatter(data.[0]..&, idx.[0].&, gathered);
    return gathered.[0] as i32;
}
"#,
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let ir = String::from_utf8_lossy(&output.stdout);
    assert!(
        ir.contains("insertelement"),
        "expected scalarized gather insertion in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("extractelement"),
        "expected scalarized scatter extraction in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("getelementptr"),
        "expected indexed element addressing in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("load i64"),
        "expected scalar index loads in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("store float"),
        "expected scalar scatter stores in LLVM IR:\n{}",
        ir
    );
}
