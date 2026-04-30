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

    ctx..&.eq(ctx..&.expect_ok(num.parse_u8("255")), u8.{255});
    ctx..&.eq(ctx..&.expect_ok(num.parse_u8_radix("ff", 16)), u8.{255});
    ctx..&.eq(ctx..&.expect_ok(num.parse_u16("65535")), u16.{65535});
    ctx..&.eq(ctx..&.expect_ok(num.parse_u16_radix("ffff", 16)), u16.{65535});
    ctx..&.eq(ctx..&.expect_ok(num.parse_u32("4294967295")), u32.{4294967295});
    ctx..&.eq(ctx..&.expect_ok(num.parse_u32_radix("ffffffff", 16)), u32.{4294967295});
    ctx..&.eq(ctx..&.expect_ok(num.parse_u64("18446744073709551615")), ~u64.{0});
    ctx..&.eq(ctx..&.expect_ok(num.parse_u64_radix("ff", 16)), u64.{255});
    ctx..&.eq(ctx..&.expect_ok(num.parse_u128("340282366920938463463374607431768211455")), ~u128.{0});
    ctx..&.eq(ctx..&.expect_ok(num.parse_u128_radix("10000000000000000", 16)), u128.{18446744073709551616});
    ctx..&.eq(ctx..&.expect_ok(num.parse_u64_radix("ZZ", 36)), u64.{1295});
    ctx..&.eq(ctx..&.expect_ok(num.parse_usize("42")), usize.{42});
    ctx..&.eq(ctx..&.expect_ok(num.parse_i8("-128")), i8.{-128});
    ctx..&.eq(ctx..&.expect_ok(num.parse_i8_radix("7f", 16)), i8.{127});
    ctx..&.eq(ctx..&.expect_ok(num.parse_i16("-32768")), i16.{-32768});
    ctx..&.eq(ctx..&.expect_ok(num.parse_i16_radix("7fff", 16)), i16.{32767});
    ctx..&.eq(ctx..&.expect_ok(num.parse_i32_radix("-80000000", 16)), i32.{-2147483648});
    ctx..&.eq(ctx..&.expect_ok(num.parse_i32("2147483647")), i32.{2147483647});
    ctx..&.eq(ctx..&.expect_ok(num.parse_i64("-9223372036854775808")), i64.{-9223372036854775808});
    ctx..&.eq(ctx..&.expect_ok(num.parse_i64("+9223372036854775807")), i64.{9223372036854775807});
    ctx..&.eq(ctx..&.expect_ok(num.parse_i128("-170141183460469231731687303715884105728")), i128.{-170141183460469231731687303715884105728});
    ctx..&.eq(ctx..&.expect_ok(num.parse_i128("170141183460469231731687303715884105727")), i128.{170141183460469231731687303715884105727});
    ctx..&.eq(ctx..&.expect_ok(num.parse_isize("-42")), isize.{-42});

    ctx..&.eq(ctx..&.expect_err(num.parse_u64("")), num.ParseIntError.Empty);
    ctx..&.eq(ctx..&.expect_err(num.parse_u64("-1")), num.ParseIntError.InvalidDigit);
    ctx..&.eq(ctx..&.expect_err(num.parse_u64_radix("2", 2)), num.ParseIntError.InvalidDigit);
    ctx..&.eq(ctx..&.expect_err(num.parse_u64_radix("10", 1)), num.ParseIntError.InvalidRadix);
    ctx..&.eq(ctx..&.expect_err(num.parse_u8("256")), num.ParseIntError.Overflow);
    ctx..&.eq(ctx..&.expect_err(num.parse_u16("65536")), num.ParseIntError.Overflow);
    ctx..&.eq(ctx..&.expect_err(num.parse_u32("4294967296")), num.ParseIntError.Overflow);
    ctx..&.eq(ctx..&.expect_err(num.parse_u64("18446744073709551616")), num.ParseIntError.Overflow);
    ctx..&.eq(ctx..&.expect_err(num.parse_u128("340282366920938463463374607431768211456")), num.ParseIntError.Overflow);
    ctx..&.eq(ctx..&.expect_err(num.parse_i8("-129")), num.ParseIntError.Overflow);
    ctx..&.eq(ctx..&.expect_err(num.parse_i16("32768")), num.ParseIntError.Overflow);
    ctx..&.eq(ctx..&.expect_err(num.parse_i64("9223372036854775808")), num.ParseIntError.Overflow);
    ctx..&.eq(ctx..&.expect_err(num.parse_i32("-2147483649")), num.ParseIntError.Overflow);
    ctx..&.eq(ctx..&.expect_err(num.parse_i128("170141183460469231731687303715884105728")), num.ParseIntError.Overflow);
    ctx..&.eq(ctx..&.expect_err(num.parse_i32("12_3")), num.ParseIntError.InvalidDigit);

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
