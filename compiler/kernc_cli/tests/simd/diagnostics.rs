use super::*;

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
        stderr.contains("indices must be in the range"),
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
        stderr.contains("exactly half of the base lane count"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_non_power_of_two_simd_alignment() {
    let output = compile_source(
        r#"
fn main() i32 {
    let mut data = [4]f32.{ 1.0, 2.0, 3.0, 4.0 };
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
fn rejects_simd_alignment_that_exceeds_backend_limit() {
    let output = compile_source(
        r#"
fn main() i32 {
    let mut data = [4]f32.{ 1.0, 2.0, 3.0, 4.0 };
    let _ = @simdLoad[f32x4](data.[0]..&, 1099511627776);
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
        stderr.contains("maximum backend-supported alignment"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("Kern Compiler Internal Error"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_masked_load_with_mismatched_mask_lanes() {
    let output = compile_source(
        r#"
fn main() i32 {
    let mut data = [4]f32.{ 1.0, 2.0, 3.0, 4.0 };
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
        stderr.contains("mask lane count must match the SIMD value lane count"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_non_usize_gather_indices_pointer() {
    let output = compile_source(
        r#"
fn main() i32 {
    let mut data = [4]f32.{ 1.0, 2.0, 3.0, 4.0 };
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
