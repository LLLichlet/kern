use super::*;

#[test]
fn base_num_parses_common_integer_text() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_base_num_parse",
        r#"
use base.num;
use base.test;
use base.io.Writer;
use std.io;

fn main() i32 {
    let mut err = io.stderr();
    let mut ctx = test.context(*mut Writer.{ err..& });

    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_u8("255"), "expected parse ok", .{}), u8.{255}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_u8_radix("ff", 16), "expected parse ok", .{}), u8.{255}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_u16("65535"), "expected parse ok", .{}), u16.{65535}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_u16_radix("ffff", 16), "expected parse ok", .{}), u16.{65535}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_u32("4294967295"), "expected parse ok", .{}), u32.{4294967295}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_u32_radix("ffffffff", 16), "expected parse ok", .{}), u32.{4294967295}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_u64("18446744073709551615"), "expected parse ok", .{}), ~u64.{0}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_u64_radix("ff", 16), "expected parse ok", .{}), u64.{255}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_u128("340282366920938463463374607431768211455"), "expected parse ok", .{}), ~u128.{0}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_u128_radix("10000000000000000", 16), "expected parse ok", .{}), u128.{18446744073709551616}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_u64_radix("ZZ", 36), "expected parse ok", .{}), u64.{1295}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_usize("42"), "expected parse ok", .{}), usize.{42}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_i8("-128"), "expected parse ok", .{}), i8.{-128}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_i8_radix("7f", 16), "expected parse ok", .{}), i8.{127}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_i16("-32768"), "expected parse ok", .{}), i16.{-32768}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_i16_radix("7fff", 16), "expected parse ok", .{}), i16.{32767}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_i32_radix("-80000000", 16), "expected parse ok", .{}), i32.{-2147483648}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_i32("2147483647"), "expected parse ok", .{}), i32.{2147483647}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_i64("-9223372036854775808"), "expected parse ok", .{}), i64.{-9223372036854775808}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_i64("+9223372036854775807"), "expected parse ok", .{}), i64.{9223372036854775807}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_i128("-170141183460469231731687303715884105728"), "expected parse ok", .{}), i128.{-170141183460469231731687303715884105728}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_i128("170141183460469231731687303715884105727"), "expected parse ok", .{}), i128.{170141183460469231731687303715884105727}, "parsed value mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_ok(@loc(), num.parse_isize("-42"), "expected parse ok", .{}), isize.{-42}, "parsed value mismatch", .{});

    ctx..&.eq(@loc(), ctx..&.expect_err(@loc(), num.parse_u64(""), "expected parse err", .{}), num.ParseIntError.Empty, "parse error mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_err(@loc(), num.parse_u64("-1"), "expected parse err", .{}), num.ParseIntError.InvalidDigit, "parse error mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_err(@loc(), num.parse_u64_radix("2", 2), "expected parse err", .{}), num.ParseIntError.InvalidDigit, "parse error mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_err(@loc(), num.parse_u64_radix("10", 1), "expected parse err", .{}), num.ParseIntError.InvalidRadix, "parse error mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_err(@loc(), num.parse_u8("256"), "expected parse err", .{}), num.ParseIntError.Overflow, "parse error mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_err(@loc(), num.parse_u16("65536"), "expected parse err", .{}), num.ParseIntError.Overflow, "parse error mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_err(@loc(), num.parse_u32("4294967296"), "expected parse err", .{}), num.ParseIntError.Overflow, "parse error mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_err(@loc(), num.parse_u64("18446744073709551616"), "expected parse err", .{}), num.ParseIntError.Overflow, "parse error mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_err(@loc(), num.parse_u128("340282366920938463463374607431768211456"), "expected parse err", .{}), num.ParseIntError.Overflow, "parse error mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_err(@loc(), num.parse_i8("-129"), "expected parse err", .{}), num.ParseIntError.Overflow, "parse error mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_err(@loc(), num.parse_i16("32768"), "expected parse err", .{}), num.ParseIntError.Overflow, "parse error mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_err(@loc(), num.parse_i64("9223372036854775808"), "expected parse err", .{}), num.ParseIntError.Overflow, "parse error mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_err(@loc(), num.parse_i32("-2147483649"), "expected parse err", .{}), num.ParseIntError.Overflow, "parse error mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_err(@loc(), num.parse_i128("170141183460469231731687303715884105728"), "expected parse err", .{}), num.ParseIntError.Overflow, "parse error mismatch", .{});
    ctx..&.eq(@loc(), ctx..&.expect_err(@loc(), num.parse_i32("12_3"), "expected parse err", .{}), num.ParseIntError.InvalidDigit, "parse error mismatch", .{});

    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "expected base.num parse helpers to succeed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}
