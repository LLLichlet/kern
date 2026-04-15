use super::*;

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
