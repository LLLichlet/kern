use super::*;

#[test]
fn emits_ir_for_simd_bit_intrinsics() {
    let output = emit_llvm_ir(
        r#"
fn main() i32 {
    let pop = @popCount(u32x4.{ 1, 3, 7, 15 });
    let lead = @clz(u32x4.{ 1, 2, 4, 8 });
    let trail = @ctz(u32x4.{ 8, 4, 2, 1 });
    let swap = @bswap(u32x4.{ 0x11223344, 0x01020304, 0xA0B0C0D0, 0x000000FF });
    return (pop.[0] + lead.[1] + trail.[2] + swap.[3]) as i32;
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
        ir.contains("llvm.ctpop"),
        "expected SIMD ctpop intrinsic in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("llvm.ctlz"),
        "expected SIMD ctlz intrinsic in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("llvm.cttz"),
        "expected SIMD cttz intrinsic in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("llvm.bswap"),
        "expected SIMD bswap intrinsic in LLVM IR:\n{}",
        ir
    );
}

#[test]
fn emits_ir_for_extended_simd_primitive_families() {
    let output = emit_llvm_ir(
        r#"
fn main() i32 {
    let ptr_sized = usizex2.{ 3, 5 } + usizex2.{ 2, 4 };
    let signed_wide = isizex2.{ 0 - 3, 7 };
    let big_unsigned = @popCount(u128x2.{ 0xFF, 0xF0F0 });
    let cmp = signed_wide < isizex2.{ 0, 0 };
    return (ptr_sized.[0] as i32) + (big_unsigned.[1] as i32) + (cmp.[0] as i32);
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
        ir.contains("<2 x i128>"),
        "expected wide SIMD integer vectors in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("llvm.ctpop"),
        "expected SIMD ctpop intrinsic in LLVM IR:\n{}",
        ir
    );
}

#[test]
fn emits_ir_for_simd_abs_min_and_max() {
    let output = emit_llvm_ir(
        r#"
fn main(argc: i32, argv: &&u8) i32 {
    let a = i32x4.{ argc, argc - 1, 0 - argc, 7 };
    let b = i32x4.{ 3, 7, 0 - 5, argc };
    let base = argc as f32;
    let c = f32x4.{ 0.0 - base, 2.0, -0.0, 0.0 - 4.0 };
    let d = f32x4.{ 2.0, 0.0 - base, 0.0, base };
    let ints = @simdAbs(a);
    let mins = @simdMin(a, b);
    let floats = @simdAbs(c);
    let maxs = @simdMax(c, d);
    return ints.[0] + mins.[1] + (floats.[2] as i32) + (maxs.[3] as i32);
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
        ir.contains("bitcast"),
        "expected SIMD floating abs to bitcast through an integer vector:\n{}",
        ir
    );
    assert!(
        ir.contains(" and "),
        "expected SIMD floating abs to clear sign bits with `and`:\n{}",
        ir
    );
    assert!(
        ir.contains("icmp"),
        "expected SIMD integer abs/min to compare vector lanes:\n{}",
        ir
    );
    assert!(
        ir.contains("fcmp"),
        "expected SIMD floating max to compare vector lanes:\n{}",
        ir
    );
    assert!(
        ir.contains("select"),
        "expected SIMD min/max and integer abs to use vector selects:\n{}",
        ir
    );
}

#[test]
fn emits_ir_for_simd_clamp() {
    let output = emit_llvm_ir(
        r#"
fn main(argc: i32, argv: &&u8) i32 {
    let ints = @simdClamp(
        i32x4.{ argc, argc - 1, 9, 5 },
        i32x4.{ 0 - 3, 3, 0, 4 },
        i32x4.{ 7, 6, 8, argc }
    );
    let base = argc as f32;
    let floats = @simdClamp(
        f32x4.{ 0.0 - base, 2.5, 9.0, -0.0 },
        f32x4.{ -2.0, 3.0, 1.0, 0.0 },
        f32x4.{ 4.0, 6.0, 8.0, base }
    );
    return ints.[0] + (floats.[2] as i32);
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
        ir.matches("select").count() >= 2,
        "expected SIMD clamp to lower through nested vector selects:\n{}",
        ir
    );
    assert!(
        ir.contains("icmp"),
        "expected integer SIMD clamp comparisons in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("fcmp"),
        "expected floating-point SIMD clamp comparisons in LLVM IR:\n{}",
        ir
    );
}

#[test]
fn emits_ir_for_simd_float_math_intrinsics() {
    let output = emit_llvm_ir(
        r#"
fn main(argc: i32, argv: &&u8) i32 {
    let base = argc as f32;
    let roots = @simdSqrt(f32x4.{ base + 1.0, base + 4.0, base + 9.0, base + 16.0 });
    let floors = @simdFloor(f32x4.{ base + 1.9, 0.0 - base, 2.0, -0.0 });
    let ceils = @simdCeil(f32x4.{ base + 1.1, 0.0 - base, 2.0, -0.0 });
    let truncs = @simdTrunc(f32x4.{ base + 1.9, 0.0 - base, 2.0, -0.0 });
    let rounds = @simdRound(f32x4.{ base + 1.4, base + 1.6, 0.0 - 1.4, 0.0 - 1.6 });
    return (roots.[0] as i32) + (floors.[1] as i32) + (ceils.[2] as i32) + (truncs.[3] as i32) + (rounds.[0] as i32);
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
        ir.contains("llvm.sqrt"),
        "expected SIMD sqrt intrinsic in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("llvm.floor"),
        "expected SIMD floor intrinsic in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("llvm.ceil"),
        "expected SIMD ceil intrinsic in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("llvm.trunc"),
        "expected SIMD trunc intrinsic in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.matches("llvm.trunc").count() >= 2,
        "expected SIMD round lowering to reuse truncation semantics in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("select"),
        "expected SIMD round lowering to use vector selects in LLVM IR:\n{}",
        ir
    );
}

#[test]
fn emits_ir_for_simd_rearrangement_helpers() {
    let output = emit_llvm_ir(
        r#"
fn main(argc: i32, argv: &&u8) i32 {
    let v = i32x4.{ argc, argc + 1, argc + 2, argc + 3 };
    let swz = @simdSwizzle(v, [4]u32.{ 3, 0, 2, 1 });
    let rev = @simdReverse(v);
    let rot = @simdRotateLeft(v, 1);
    let mix = @simdInterleaveHi(v, i32x4.{ 10, 20, 30, 40 });
    let cat = @simdConcatLo(v, i32x4.{ 10, 20, 30, 40 });
    let de = @simdDeinterleaveLo(i32x4.{ argc, 1, argc + 1, 2 }, i32x4.{ argc + 2, 3, argc + 3, 4 });
    let low = @simdLowHalf[i32x2](v);
    let stitched = @simdWithHighHalf[i32x4](v, i32x2.{ 10, 20 });
    return swz.[0] + rev.[0] + rot.[1] + mix.[2] + cat.[3] + de.[1] + low.[0] + stitched.[3];
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
        ir.matches("shufflevector").count() >= 6,
        "expected rearrangement helpers to lower through LLVM shufflevector:\n{}",
        ir
    );
    assert!(
        ir.contains("insertelement"),
        "expected half insertion helpers to lower through lane insertion:\n{}",
        ir
    );
}

#[test]
fn emits_ir_for_simd_splat_cast_and_bitcast() {
    let output = emit_llvm_ir(
        r#"
fn main() i32 {
    let ones = @simdSplat[i32x4](7);
    let as_float = @simdCast[f32x4](ones);
    let bits = @simdBitcast[u32x4](as_float);
    return (bits.[0] as i32) + ones.[1];
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
        "expected SIMD splat/cast lane insertion in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("sitofp"),
        "expected lane-wise numeric SIMD cast in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("bitcast <4 x float>"),
        "expected SIMD bitcast in LLVM IR:\n{}",
        ir
    );
}
