use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn unique_temp_path(prefix: &str, extension: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let file_name = format!("{}_{}_{}.{}", prefix, std::process::id(), nanos, extension);
    std::env::temp_dir().join(file_name)
}

fn run_kernc(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_kernc"))
        .current_dir(repo_root())
        .args(args)
        .output()
        .unwrap()
}

fn compile_source(source: &str) -> std::process::Output {
    let source_path = unique_temp_path("kernc_test", "kr");
    let object_path = unique_temp_path("kernc_test", "o");
    fs::write(&source_path, source).unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let object_arg = object_path.to_string_lossy().into_owned();
    let args = vec!["-c", source_arg.as_str(), "-o", object_arg.as_str()];
    let output = run_kernc(&args);

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&object_path);
    output
}

#[test]
fn compiles_anonymous_aggregates_example() {
    let source = repo_root().join("examples/anonymous_aggregates.kr");
    let object = unique_temp_path("anonymous_aggregates", "o");

    let source_arg = source.to_string_lossy().into_owned();
    let object_arg = object.to_string_lossy().into_owned();
    let args = vec!["-c", source_arg.as_str(), "-o", object_arg.as_str()];
    let output = run_kernc(&args);

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        object.exists(),
        "expected object file at {}",
        object.display()
    );

    let _ = fs::remove_file(&object);
}

#[test]
fn compiles_anonymous_enum_match_with_explicit_discriminants() {
    let output = compile_source(
        r#"
type Switch = enum: u16 {
    Off = 4,
    On = 7,
    Error: i32,
};

fn decode_named(v: Switch) i32 {
    match (v) {
        .Off => 40,
        .On => 70,
        .Error: payload => payload,
    }
}

fn decode_anon(v: enum: u16 { Off = 4, On = 7, Error: i32 }) i32 {
    match (v) {
        .Off => 1,
        .On => 2,
        .Error: payload => payload,
    }
}

extern fn main(args: [][]u8) i32 {
    let named = Switch.{ Error: 9 };
    let anon = enum: u16 { Off = 4, On = 7, Error: i32 }.{ Error: 11 };
    return decode_named(named) + decode_anon(anon);
}
"#,
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_extern_enum_declarations() {
    let output = compile_source(
        r#"
extern type Bad = enum {
    A,
    B,
};
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
        stderr.contains("enum types do not support `extern`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_extern_union_bnc_without_extern_on_the_anonymous_side() {
    let output = compile_source(
        r#"
extern type CWord = union {
    bytes: [4]u8,
    int: i32,
};

fn read_plain(word: union { bytes: [4]u8, int: i32 }) i32 {
    word.int
}

extern fn main(args: [][]u8) i32 {
    let word = CWord.{ int: 9 };
    return read_plain(word);
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
        stderr.contains("mismatched types"),
        "unexpected stderr:\n{}",
        stderr
    );
}
