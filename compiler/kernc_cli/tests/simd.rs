use kernc_cli::test_support::{
    build_and_run, compile_source_with_args as compile_with_args,
    emit_llvm_ir_with_args as emit_ir_with_args,
};

fn compile_source(source: &str) -> std::process::Output {
    compile_with_args("kernc_simd_test", source, &[])
}

fn emit_llvm_ir(source: &str) -> std::process::Output {
    emit_ir_with_args("kernc_simd_test", source, &[])
}

fn build_and_run_source(source: &str) -> std::process::Output {
    build_and_run("kernc_simd_test", source, &[])
}

#[test]
fn builds_and_runs_simd_lane_ops_and_intrinsics() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let a = f32x4.{ 1.0, 2.0, 3.0, 4.0 };
    let b = f32x4.{ 5.0, 1.0, 3.0, 0.0 };

    let mask = a < b;
    let any = @simdAny(mask);
    let all = @simdAll(a == a);

    let mut out = @simdSelect(mask, b, a);
    out.[1] = out.[1] + 9.0;

    if (any and all) {
        let total = (out.[0] as i32)
            + (out.[1] as i32)
            + (out.[2] as i32)
            + (out.[3] as i32);
        return total - 20;
    }

    return 99;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(3),
        "hosted SIMD binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn emits_vector_ir_for_simd_ops() {
    let output = emit_llvm_ir(
        r#"
fn main() i32 {
    let a = f32x4.{ 1.0, 2.0, 3.0, 4.0 };
    let b = f32x4.{ 5.0, 1.0, 3.0, 0.0 };
    let mask = a < b;
    let mut out = @simdSelect(mask, b, a);
    out.[2] = 9.0;
    return (out.[0] as i32) + (out.[2] as i32);
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
        ir.contains("fcmp"),
        "expected vector compare in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("select"),
        "expected vector select in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("extractelement"),
        "expected lane extraction in LLVM IR:\n{}",
        ir
    );
    assert!(
        ir.contains("insertelement"),
        "expected lane insertion in LLVM IR:\n{}",
        ir
    );
}

#[test]
fn builds_and_runs_simd_splat_cast_and_bitcast() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let ones = @simdSplat[i32x4](7);
    let as_float = @simdCast[f32x4](ones);
    let roundtrip = @simdCast[i32x4](as_float);
    let bits = @simdBitcast[u32x4](f32x4.{ 1.0, 2.0, 4.0, 8.0 });

    if (!@simdAll(roundtrip == ones)) {
        return 91;
    }

    if (!@simdAll(bits == u32x4.{ 1065353216, 1073741824, 1082130432, 1090519040 })) {
        return 92;
    }

    return 42;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(42),
        "hosted SIMD binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn builds_and_runs_simd_bit_intrinsics() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let pop = @popCount(u32x4.{ 1, 3, 7, 15 });
    let lead = @clz(u32x4.{ 1, 2, 4, 8 });
    let trail = @ctz(u32x4.{ 8, 4, 2, 1 });
    let swap = @bswap(u32x4.{ 0x11223344, 0x01020304, 0xA0B0C0D0, 0x000000FF });

    if (!@simdAll(pop == u32x4.{ 1, 2, 3, 4 })) {
        return 101;
    }

    if (!@simdAll(lead == u32x4.{ 31, 30, 29, 28 })) {
        return 102;
    }

    if (!@simdAll(trail == u32x4.{ 3, 2, 1, 0 })) {
        return 103;
    }

    if (!@simdAll(swap == u32x4.{ 0x44332211, 0x04030201, 0xD0C0B0A0, 0xFF000000 })) {
        return 104;
    }

    return 27;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(27),
        "hosted SIMD binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn builds_and_runs_extended_simd_primitive_families() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let ptr_sized = usizex2.{ 3, 5 } + usizex2.{ 2, 4 };
    let signed_wide = isizex2.{ 0 - 3, 7 };
    let big_signed = i128x2.{ 10, 20 } + i128x2.{ 1, 2 };
    let big_unsigned = @popCount(u128x2.{ 0xFF, 0xF0F0 });

    if (!@simdAll(ptr_sized == usizex2.{ 5, 9 })) {
        return 111;
    }

    if (!@simdAll((signed_wide < isizex2.{ 0, 0 }) == boolx2.{ true, false })) {
        return 112;
    }

    if (!@simdAll(big_signed == i128x2.{ 11, 22 })) {
        return 113;
    }

    if (!@simdAll(big_unsigned == u128x2.{ 8, 8 })) {
        return 114;
    }

    return 31;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(31),
        "hosted SIMD binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn builds_and_runs_simd_abs_min_and_max() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let ints = @simdAbs(i32x4.{ 0 - 3, 2, 0, 0 - 7 });
    let floats = @simdAbs(f32x4.{ -1.5, 2.0, -0.0, -4.0 });
    let float_bits = @simdBitcast[u32x4](floats);
    let mins = @simdMin(i32x4.{ 9, 2, 0 - 4, 8 }, i32x4.{ 3, 7, 0 - 5, 8 });
    let maxs = @simdMax(f32x4.{ 1.0, -3.0, -0.0, 2.5 }, f32x4.{ 2.0, -5.0, 0.0, 9.0 });
    let max_bits = @simdBitcast[u32x4](maxs);

    if (!@simdAll(ints == i32x4.{ 3, 2, 0, 7 })) {
        return 121;
    }

    if (!@simdAll(float_bits == u32x4.{ 0x3FC00000, 0x40000000, 0x00000000, 0x40800000 })) {
        return 122;
    }

    if (!@simdAll(mins == i32x4.{ 3, 2, 0 - 5, 8 })) {
        return 123;
    }

    if (!@simdAll(max_bits == u32x4.{ 0x40000000, 0xC0400000, 0x00000000, 0x41100000 })) {
        return 124;
    }

    return 35;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(35),
        "hosted SIMD binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn builds_and_runs_simd_clamp() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let ints = @simdClamp(
        i32x4.{ 0 - 8, 2, 9, 5 },
        i32x4.{ 0 - 3, 3, 0, 4 },
        i32x4.{ 7, 6, 8, 4 }
    );
    let floats = @simdClamp(
        f32x4.{ -5.0, 2.5, 9.0, -0.0 },
        f32x4.{ -2.0, 3.0, 1.0, 0.0 },
        f32x4.{ 4.0, 6.0, 8.0, 7.0 }
    );
    let float_bits = @simdBitcast[u32x4](floats);

    if (!@simdAll(ints == i32x4.{ 0 - 3, 3, 8, 4 })) {
        return 131;
    }

    if (!@simdAll(float_bits == u32x4.{ 0xC0000000, 0x40400000, 0x41000000, 0x00000000 })) {
        return 132;
    }

    return 37;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(37),
        "hosted SIMD binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn builds_and_runs_simd_float_math_intrinsics() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let roots = @simdSqrt(f32x4.{ 1.0, 4.0, 9.0, 16.0 });
    let floors = @simdFloor(f32x4.{ 1.9, -1.2, 2.0, -0.0 });
    let ceils = @simdCeil(f32x4.{ 1.1, -1.8, 2.0, -0.0 });
    let truncs = @simdTrunc(f32x4.{ 1.9, -1.8, 2.0, -0.0 });
    let rounds = @simdRound(f32x4.{ 1.4, 1.6, -1.4, -1.6 });

    if (!@simdAll(roots == f32x4.{ 1.0, 2.0, 3.0, 4.0 })) {
        return 141;
    }

    if (!@simdAll(floors == f32x4.{ 1.0, -2.0, 2.0, 0.0 })) {
        return 142;
    }

    if (!@simdAll(ceils == f32x4.{ 2.0, -1.0, 2.0, 0.0 })) {
        return 143;
    }

    if (!@simdAll(truncs == f32x4.{ 1.0, -1.0, 2.0, 0.0 })) {
        return 144;
    }

    if (!@simdAll(rounds == f32x4.{ 1.0, 2.0, -1.0, -2.0 })) {
        return 145;
    }

    return 39;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(39),
        "hosted SIMD binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn builds_and_runs_simd_rearrangement_helpers() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let v = i32x4.{ 1, 2, 3, 4 };
    let swz = @simdSwizzle(v, [4]u32.{ 3, 0, 2, 1 });
    let rev = @simdReverse(v);
    let rotl = @simdRotateLeft(v, 1);
    let rotr = @simdRotateRight(v, 2);
    let ilo = @simdInterleaveLo(i32x4.{ 10, 20, 30, 40 }, i32x4.{ 1, 2, 3, 4 });
    let ihi = @simdInterleaveHi(i32x4.{ 10, 20, 30, 40 }, i32x4.{ 1, 2, 3, 4 });
    let zlo = @simdZipLo(i32x4.{ 10, 20, 30, 40 }, i32x4.{ 1, 2, 3, 4 });
    let zhi = @simdZipHi(i32x4.{ 10, 20, 30, 40 }, i32x4.{ 1, 2, 3, 4 });
    let clo = @simdConcatLo(i32x4.{ 10, 20, 30, 40 }, i32x4.{ 1, 2, 3, 4 });
    let chi = @simdConcatHi(i32x4.{ 10, 20, 30, 40 }, i32x4.{ 1, 2, 3, 4 });
    let dlo = @simdDeinterleaveLo(i32x4.{ 10, 1, 20, 2 }, i32x4.{ 30, 3, 40, 4 });
    let dhi = @simdDeinterleaveHi(i32x4.{ 10, 1, 20, 2 }, i32x4.{ 30, 3, 40, 4 });
    let ulo = @simdUnzipLo(i32x4.{ 10, 1, 20, 2 }, i32x4.{ 30, 3, 40, 4 });
    let uhi = @simdUnzipHi(i32x4.{ 10, 1, 20, 2 }, i32x4.{ 30, 3, 40, 4 });
    let low = @simdLowHalf[i32x2](v);
    let high = @simdHighHalf[i32x2](v);
    let with_low = @simdWithLowHalf[i32x4](i32x4.{ 10, 20, 30, 40 }, i32x2.{ 1, 2 });
    let with_high = @simdWithHighHalf[i32x4](i32x4.{ 10, 20, 30, 40 }, i32x2.{ 7, 8 });

    if (!@simdAll(swz == i32x4.{ 4, 1, 3, 2 })) {
        return 150;
    }

    if (!@simdAll(rev == i32x4.{ 4, 3, 2, 1 })) {
        return 151;
    }

    if (!@simdAll(rotl == i32x4.{ 2, 3, 4, 1 })) {
        return 152;
    }

    if (!@simdAll(rotr == i32x4.{ 3, 4, 1, 2 })) {
        return 153;
    }

    if (!@simdAll(ilo == i32x4.{ 10, 1, 20, 2 })) {
        return 154;
    }

    if (!@simdAll(ihi == i32x4.{ 30, 3, 40, 4 })) {
        return 155;
    }

    if (!@simdAll(zlo == ilo)) {
        return 160;
    }

    if (!@simdAll(zhi == ihi)) {
        return 161;
    }

    if (!@simdAll(clo == i32x4.{ 10, 20, 1, 2 })) {
        return 156;
    }

    if (!@simdAll(chi == i32x4.{ 30, 40, 3, 4 })) {
        return 157;
    }

    if (!@simdAll(dlo == i32x4.{ 10, 20, 30, 40 })) {
        return 158;
    }

    if (!@simdAll(dhi == i32x4.{ 1, 2, 3, 4 })) {
        return 159;
    }

    if (!@simdAll(ulo == dlo)) {
        return 162;
    }

    if (!@simdAll(uhi == dhi)) {
        return 163;
    }

    if (!@simdAll(low == i32x2.{ 1, 2 })) {
        return 164;
    }

    if (!@simdAll(high == i32x2.{ 3, 4 })) {
        return 165;
    }

    if (!@simdAll(with_low == i32x4.{ 1, 2, 30, 40 })) {
        return 166;
    }

    if (!@simdAll(with_high == i32x4.{ 10, 20, 7, 8 })) {
        return 167;
    }

    return 43;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(43),
        "hosted SIMD binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn builds_and_runs_simd_shuffle_reduce_and_load_store() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let data = [8]mut f32.{ 1.0, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0 };
    let first = @simdLoad[f32x4](data.[0]..&, 4);
    let second = @simdLoad[f32x4](data.[4]..&, 4);
    let mixed = @simdShuffle(first, second, [4]u32.{ 0, 5, 2, 7 });

    let sum = @simdReduceAdd(mixed);
    let max = @simdReduceMax(mixed);
    @simdStore(data.[0]..&, mixed, 4);

    return (sum as i32) - (max as i32) + (data.[1] as i32);
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(44),
        "hosted SIMD binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn builds_and_runs_simd_gather_and_scatter() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let data = [8]mut i32.{ 10, 20, 30, 40, 50, 60, 70, 80 };
    let idx = [4]usize.{ 7, 0, 5, 0 };
    let lanes = @simdGather[i32x4](data.[0]..&, idx.[0].&);
    let bumped = lanes + i32x4.{ 1, 2, 3, 4 };
    @simdScatter(data.[0]..&, idx.[0].&, bumped);
    return data.[7] + data.[5] + data.[0];
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(158),
        "hosted SIMD binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

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
fn main(argc: i32, argv: **u8) i32 {
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
fn main(argc: i32, argv: **u8) i32 {
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
fn main(argc: i32, argv: **u8) i32 {
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
fn main(argc: i32, argv: **u8) i32 {
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

#[test]
fn builds_and_runs_simd_masked_memory_ops() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let data = [4]mut i32.{ 10, 20, 30, 40 };
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
    let output = emit_llvm_ir(
        r#"
#[target_feature("avx2,fma")]
fn remix(ptr: *mut f32) f32 {
    let a = @simdLoad[f32x4](ptr, 4);
    let b = @simdLoad[f32x4](ptr + usize.{4}, 4);
    let mixed = @simdShuffle(a, b, [4]u32.{ 0, 5, 2, 7 });
    @simdStore(ptr, mixed, 4);
    return @simdReduceAdd(mixed);
}

fn main() i32 {
    let data = [8]mut f32.{ 1.0, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0 };
    return remix(data.[0]..&) as i32;
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
    let data = [4]mut f32.{ 1.0, 2.0, 3.0, 4.0 };
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
    let data = [8]mut f32.{ 1.0, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0 };
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

#[test]
fn rejects_simd_cast_with_mismatched_lane_counts() {
    let output = compile_source(
        r#"
fn main() i32 {
    let v = i32x4.{ 1, 2, 3, 4 };
    let _ = @simdCast[f32x2](v);
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
        stderr.contains("requires matching SIMD lane counts"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_simd_bitcast_with_mismatched_sizes() {
    let output = compile_source(
        r#"
fn main() i32 {
    let v = f32x4.{ 1.0, 2.0, 3.0, 4.0 };
    let _ = @simdBitcast[u16x4](v);
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
        stderr.contains("same size"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_simd_abs_for_unsigned_vectors() {
    let output = compile_source(
        r#"
fn main() i32 {
    let _ = @simdAbs(u32x4.{ 1, 2, 3, 4 });
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
        stderr.contains("signed integer or floating-point SIMD lanes"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_simd_min_for_mask_vectors() {
    let output = compile_source(
        r#"
fn main() i32 {
    let _ = @simdMin(boolx4.{ true, false, true, false }, boolx4.{ false, true, false, true });
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
        stderr.contains("integer or floating-point SIMD lanes"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_simd_clamp_for_mask_vectors() {
    let output = compile_source(
        r#"
fn main() i32 {
    let _ = @simdClamp(
        boolx4.{ true, false, true, false },
        boolx4.{ false, false, false, false },
        boolx4.{ true, true, true, true }
    );
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
        stderr.contains("integer or floating-point SIMD lanes"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_simd_sqrt_for_integer_vectors() {
    let output = compile_source(
        r#"
fn main() i32 {
    let _ = @simdSqrt(i32x4.{ 1, 4, 9, 16 });
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
        stderr.contains("floating-point SIMD lanes"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_simd_rotate_with_non_constant_amount() {
    let output = compile_source(
        r#"
fn main(argc: i32, argv: **u8) i32 {
    let _ = @simdRotateLeft(i32x4.{ 1, 2, 3, 4 }, argc as usize);
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
        stderr.contains("amount must be a compile-time constant"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_simd_swizzle_with_out_of_range_index() {
    let output = compile_source(
        r#"
fn main() i32 {
    let _ = @simdSwizzle(i32x4.{ 1, 2, 3, 4 }, [4]u32.{ 0, 4, 2, 1 });
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
        stderr.contains("out of range"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_simd_interleave_with_odd_lane_count() {
    let output = compile_source(
        r#"
fn main() i32 {
    let _ = @simdInterleaveLo(i32x3.{ 1, 2, 3 }, i32x3.{ 4, 5, 6 });
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
        stderr.contains("requires an even SIMD lane count"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_simd_with_half_for_wrong_lane_shape() {
    let output = compile_source(
        r#"
fn main() i32 {
    let _ = @simdWithLowHalf[i32x4](i32x4.{ 10, 20, 30, 40 }, i32x3.{ 1, 2, 3 });
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
        stderr.contains("exactly twice as many lanes"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_non_power_of_two_simd_alignment() {
    let output = compile_source(
        r#"
fn main() i32 {
    let data = [4]mut f32.{ 1.0, 2.0, 3.0, 4.0 };
    let _ = @simdLoad[f32x4](data.[0]..&, 3);
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
        stderr.contains("alignment must be a non-zero power of two"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_masked_load_with_mismatched_mask_lanes() {
    let output = compile_source(
        r#"
fn main() i32 {
    let data = [4]mut f32.{ 1.0, 2.0, 3.0, 4.0 };
    let _ = @simdMaskedLoad[f32x4](
        data.[0]..&,
        boolx2.{ true, false },
        f32x4.{ 0.0, 0.0, 0.0, 0.0 },
        4
    );
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
        stderr.contains("mask lane count must match the value lane count"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_non_usize_gather_indices_pointer() {
    let output = compile_source(
        r#"
fn main() i32 {
    let data = [4]mut f32.{ 1.0, 2.0, 3.0, 4.0 };
    let idx = [4]u32.{ 0, 1, 2, 3 };
    let _ = @simdGather[f32x4](data.[0]..&, idx.[0].&);
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
        stderr.contains("indices pointer must point to `usize`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_non_constant_simd_lane_index() {
    let output = compile_source(
        r#"
fn main() i32 {
    let i = usize.{1};
    let v = f32x4.{ 1.0, 2.0, 3.0, 4.0 };
    let _ = v.[i];
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
        stderr.contains("SIMD lane index must be a compile-time constant"),
        "unexpected stderr:\n{}",
        stderr
    );
}
