use super::*;

#[test]
fn base_num_parses_common_integer_text() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_base_num_parse",
        r#"
use base.num;
use base.test;
use std.io;

fn main() i32 {
    let t = test.report(io.stderr())..&;

    "255".parse[u8]().should_ok().eq(u8.{255}).sum(@loc(), t);
    "ff".parse_radix[u8](16).should_ok().eq(u8.{255}).sum(@loc(), t);
    "65535".parse[u16]().should_ok().eq(u16.{65535}).sum(@loc(), t);
    "ffff".parse_radix[u16](16).should_ok().eq(u16.{65535}).sum(@loc(), t);
    "4294967295".parse[u32]().should_ok().eq(u32.{4294967295}).sum(@loc(), t);
    "ffffffff".parse_radix[u32](16).should_ok().eq(u32.{4294967295}).sum(@loc(), t);
    "18446744073709551615".parse[u64]().should_ok().eq(~u64.{0}).sum(@loc(), t);
    "ff".parse_radix[u64](16).should_ok().eq(u64.{255}).sum(@loc(), t);
    "340282366920938463463374607431768211455".parse[u128]().should_ok().eq(~u128.{0}).sum(@loc(), t);
    "10000000000000000".parse_radix[u128](16).should_ok().eq(u128.{18446744073709551616}).sum(@loc(), t);
    "ZZ".parse_radix[u64](36).should_ok().eq(u64.{1295}).sum(@loc(), t);
    "42".parse[usize]().should_ok().eq(usize.{42}).sum(@loc(), t);
    "-128".parse[i8]().should_ok().eq(i8.{-128}).sum(@loc(), t);
    "7f".parse_radix[i8](16).should_ok().eq(i8.{127}).sum(@loc(), t);
    "-32768".parse[i16]().should_ok().eq(i16.{-32768}).sum(@loc(), t);
    "7fff".parse_radix[i16](16).should_ok().eq(i16.{32767}).sum(@loc(), t);
    "-80000000".parse_radix[i32](16).should_ok().eq(i32.{-2147483648}).sum(@loc(), t);
    "2147483647".parse[i32]().should_ok().eq(i32.{2147483647}).sum(@loc(), t);
    "-9223372036854775808".parse[i64]().should_ok().eq(i64.{-9223372036854775808}).sum(@loc(), t);
    "+9223372036854775807".parse[i64]().should_ok().eq(i64.{9223372036854775807}).sum(@loc(), t);
    "-170141183460469231731687303715884105728".parse[i128]().should_ok().eq(i128.{-170141183460469231731687303715884105728}).sum(@loc(), t);
    "170141183460469231731687303715884105727".parse[i128]().should_ok().eq(i128.{170141183460469231731687303715884105727}).sum(@loc(), t);
    "-42".parse[isize]().should_ok().eq(isize.{-42}).sum(@loc(), t);

    "".parse[u64]().should_err().eq(num.ParseIntError.Empty).sum(@loc(), t);
    "-1".parse[u64]().should_err().eq(num.ParseIntError.InvalidDigit).sum(@loc(), t);
    "2".parse_radix[u64](2).should_err().eq(num.ParseIntError.InvalidDigit).sum(@loc(), t);
    "10".parse_radix[u64](1).should_err().eq(num.ParseIntError.InvalidRadix).sum(@loc(), t);
    "256".parse[u8]().should_err().eq(num.ParseIntError.Overflow).sum(@loc(), t);
    "65536".parse[u16]().should_err().eq(num.ParseIntError.Overflow).sum(@loc(), t);
    "4294967296".parse[u32]().should_err().eq(num.ParseIntError.Overflow).sum(@loc(), t);
    "18446744073709551616".parse[u64]().should_err().eq(num.ParseIntError.Overflow).sum(@loc(), t);
    "340282366920938463463374607431768211456".parse[u128]().should_err().eq(num.ParseIntError.Overflow).sum(@loc(), t);
    "-129".parse[i8]().should_err().eq(num.ParseIntError.Overflow).sum(@loc(), t);
    "32768".parse[i16]().should_err().eq(num.ParseIntError.Overflow).sum(@loc(), t);
    "9223372036854775808".parse[i64]().should_err().eq(num.ParseIntError.Overflow).sum(@loc(), t);
    "-2147483649".parse[i32]().should_err().eq(num.ParseIntError.Overflow).sum(@loc(), t);
    "170141183460469231731687303715884105728".parse[i128]().should_err().eq(num.ParseIntError.Overflow).sum(@loc(), t);
    "12_3".parse[i32]().should_err().eq(num.ParseIntError.InvalidDigit).sum(@loc(), t);

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
