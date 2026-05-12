use super::*;

#[test]
fn compile_only_std_program_emits_no_std_warnings() {
    let output = compile_source_with_args(
        "kernc_std_compile_no_warnings",
        r#"
fn main() i32 {
    return 0;
}
"#,
        &["--library-bundle", "std"],
    );
    assert_success(&output, "kernc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("warning"),
        "unexpected std warning noise during compile-only build:\n{}",
        stderr
    );
}

#[test]
fn base_bundle_exposes_freestanding_io_helpers() {
    let output = compile_source_with_args(
        "kernc_base_io_helpers",
        r#"
use base.io.Write;

fn main() i32 {
    let mut storage = [32]u8.{undef};
    let mut fixed = (storage..&[0 .. 32]).writer();
    let writer = &mut Write.{ fixed..& };

    "base {} {}".fmt(.{ "io", usize.{7}, }).write_to(writer);
    if (fixed..&.as_slice() != "base io 7") {
        return 1;
    }
    if (!writer.write_all("!")) {
        return 2;
    }
    if (fixed..&.as_slice() != "base io 7!") {
        return 3;
    }
    return 0;
}
"#,
        &["--library-bundle", "base"],
    );
    assert_success(&output, "kernc");
}

#[test]
fn base_bundle_exposes_freestanding_test_helpers() {
    let output = compile_source_with_args(
        "kernc_base_test_helpers",
        r#"
use base.test;
use base.io.discard;

fn main() i32 {
    let t = test.report(discard())..&;

    true.should().sum(@loc(), t);
    (usize.{3} == usize.{3}).should().sum(@loc(), t);
    (usize.{3} != usize.{4}).should().sum(@loc(), t);
    (?usize.{ Some: 7 }).should_some().eq(usize.{7}).sum(@loc(), t);
    (?usize.None).should_none().sum(@loc(), t);
    usize!i32.{ Ok: 9 }.should_ok().eq(usize.{9}).sum(@loc(), t);
    return 0;
}
"#,
        &["--library-bundle", "base"],
    );
    assert_success(&output, "kernc");
}

#[test]
fn std_bundle_does_not_expose_std_coll_module() {
    let output = compile_source_with_args(
        "kernc_std_coll_module",
        r#"
use std.coll.List;

fn main() i32 {
    return 0;
}
"#,
        &["--library-bundle", "std"],
    );
    assert!(
        !output.status.success(),
        "expected std.coll import to fail:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("std.coll") || stderr.contains("coll"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn std_bundle_does_not_expose_std_test_module() {
    let output = compile_source_with_args(
        "kernc_std_test_module",
        r#"
use std.test;

fn main() i32 {
    return 0;
}
"#,
        &["--library-bundle", "std"],
    );
    assert!(
        !output.status.success(),
        "expected std.test import to fail:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("std.test") || stderr.contains("test"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn std_bundle_does_not_expose_prov_or_sys() {
    let prov_output = compile_source_with_args(
        "kernc_std_hidden_prov",
        r#"
use prov.os.OpenOptions;

fn main() i32 {
    let options = OpenOptions.{ read: true };
    return if (options.read) { 0 } else { 1 };
}
"#,
        &["--library-bundle", "std"],
    );
    assert!(
        !prov_output.status.success(),
        "expected public prov import to fail:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&prov_output.stdout),
        String::from_utf8_lossy(&prov_output.stderr)
    );
    let prov_stderr = String::from_utf8_lossy(&prov_output.stderr);
    assert!(
        prov_stderr.contains("prov") || prov_stderr.contains("module"),
        "unexpected stderr:\n{}",
        prov_stderr
    );

    let sys_output = compile_source_with_args(
        "kernc_std_hidden_sys",
        r#"
use sys.mem.Page;

fn main() i32 {
    let _ = Page.{};
    return 0;
}
"#,
        &["--library-bundle", "std"],
    );
    assert!(
        !sys_output.status.success(),
        "expected public sys import to fail:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&sys_output.stdout),
        String::from_utf8_lossy(&sys_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&sys_output.stderr);
    assert!(
        stderr.contains("sys") || stderr.contains("module"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn std_bundle_alone_does_not_auto_inject_rt_module() {
    let output = compile_source_with_args(
        "kernc_std_hidden_rt_module",
        r#"
use rt.mem.memmove;

fn main() i32 {
    let _ = memmove;
    return 0;
}
"#,
        &["--library-bundle", "std"],
    );
    assert!(
        !output.status.success(),
        "expected implicit rt import to fail:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("rt") || stderr.contains("module"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn runtime_entry_does_not_expose_rt_mem_module() {
    let output = compile_source_with_args(
        "kernc_rt_hidden_mem_module",
        r#"
use rt.mem.memmove;

fn main() i32 {
    let _ = memmove;
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-entry", "rt"],
    );
    assert!(
        !output.status.success(),
        "expected rt.mem to stay hidden even when runtime entry is enabled:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("mem") || stderr.contains("rt") || stderr.contains("module"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn runtime_entry_does_not_auto_inject_base_module() {
    let temp_dir = unique_temp_path("kernc_rt_without_bundle", "dir");
    let rt_dir = temp_dir.join("rt");
    let source_path = temp_dir.join("main.rn");
    let object_path = temp_dir.join("main.o");

    fs::create_dir_all(&rt_dir).unwrap();
    fs::write(rt_dir.join("init.rn"), "").unwrap();
    fs::write(
        &source_path,
        r#"
use base.mem.alloc.Page;

fn main() i32 {
    let _ = Page.{};
    return 0;
}
"#,
    )
    .unwrap();

    let rt_arg = format!("rt={}", rt_dir.display());
    let source_arg = source_path.to_string_lossy().into_owned();
    let object_arg = object_path.to_string_lossy().into_owned();
    let output = run_kernc([
        "-c",
        "--runtime-entry",
        "rt",
        "--module-path",
        rt_arg.as_str(),
        source_arg.as_str(),
        "-o",
        object_arg.as_str(),
    ]);

    assert!(
        !output.status.success(),
        "expected base to remain unresolved without an explicit bundle or module path:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Unresolved external import root `base`"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn official_rt_links_without_base_or_std_bundle() {
    let source_path = unique_temp_path("kernc_rt_standalone_bundle_none", "rn");
    let exe_ext = if cfg!(windows) { "exe" } else { "out" };
    let executable_path = unique_temp_path("kernc_rt_standalone_bundle_none", exe_ext);

    fs::write(
        &source_path,
        r#"
fn main() i32 {
    return 0;
}
"#,
    )
    .unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();
    let output = run_kernc([
        "--runtime-entry",
        "rt",
        "--library-bundle",
        "none",
        source_arg.as_str(),
        "-o",
        exe_arg.as_str(),
    ]);

    assert_success(&output, "kernc");

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "official rt bundle-none binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn std_bundle_does_not_expose_page_allocator_through_base_alloc() {
    let output = compile_source_with_args(
        "kernc_base_alloc_page_unavailable",
        r#"
use base.mem.alloc.Page;

fn main() i32 {
    let _ = Page.{};
    return 0;
}
"#,
        &["--library-bundle", "std"],
    );
    assert!(
        !output.status.success(),
        "expected base.mem.alloc.Page import to fail:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Page") || stderr.contains("alloc"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn compiles_std_hello_world_in_compile_only_mode() {
    let source = unique_temp_path("kernc_std_hello_world", "rn");
    let object = unique_temp_path("kernc_std_hello_world", "o");
    fs::write(&source, HOSTED_HELLO_WORLD_SOURCE).unwrap();

    let source_arg = source.to_string_lossy().into_owned();
    let object_arg = object.to_string_lossy().into_owned();
    let args = vec![
        "-c",
        "--library-bundle",
        "std",
        source_arg.as_str(),
        "-o",
        object_arg.as_str(),
    ];
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
    assert_not_textual_llvm_ir(&object);

    let _ = fs::remove_file(&source);
    let _ = fs::remove_file(&object);
}

#[cfg(windows)]
#[test]
fn compiles_std_hello_world_to_unicode_object_path() {
    let source = unique_temp_path("kernc_std_hello_world", "rn");
    let object = unique_temp_path("kernc_std_hello_world_\u{4F60}\u{597D}", "o");
    fs::write(&source, HOSTED_HELLO_WORLD_SOURCE).unwrap();

    let source_arg = source.to_string_lossy().into_owned();
    let object_arg = object.to_string_lossy().into_owned();
    let args = vec![
        "-c",
        "--library-bundle",
        "std",
        source_arg.as_str(),
        "-o",
        object_arg.as_str(),
    ];
    let output = run_kernc(&args);

    assert_success(&output, "kernc");
    assert!(
        object.exists(),
        "expected object file at {}",
        object.display()
    );
    assert_not_textual_llvm_ir(&object);

    let _ = fs::remove_file(&source);
    let _ = fs::remove_file(&object);
}

#[test]
fn links_compile_only_object_via_link_only_mode() {
    let source_path = unique_temp_path("kernc_std_link_only", "rn");
    let object_path = unique_temp_path("kernc_std_link_only", "o");
    let exe_ext = if cfg!(windows) { "exe" } else { "out" };
    let executable_path = unique_temp_path("kernc_std_link_only", exe_ext);

    fs::write(
        &source_path,
        r#"
use std.io;

fn main() i32 {
    "link only".println();
    return 0;
}
"#,
    )
    .unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let object_arg = object_path.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();

    let compile_output = run_kernc([
        "-c",
        "--library-bundle",
        "std",
        "--runtime-entry",
        "crt",
        "--runtime-libc",
        "yes",
        source_arg.as_str(),
        "-o",
        object_arg.as_str(),
    ]);
    assert!(
        compile_output.status.success(),
        "kernc compile-only failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compile_output.stdout),
        String::from_utf8_lossy(&compile_output.stderr)
    );
    assert_not_textual_llvm_ir(&object_path);

    let link_output = run_kernc([
        "--link-only",
        "--library-bundle",
        "std",
        "--runtime-entry",
        "crt",
        "--runtime-libc",
        "yes",
        "--link-input",
        object_arg.as_str(),
        "-o",
        exe_arg.as_str(),
    ]);
    assert!(
        link_output.status.success(),
        "kernc link-only failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&link_output.stdout),
        String::from_utf8_lossy(&link_output.stderr)
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        run_output.status.success(),
        "link-only binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&object_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn compile_only_object_does_not_export_synthesized_symbols() {
    if cfg!(windows) {
        return;
    }

    let source_path = unique_temp_path("kernc_internal_symbols", "rn");
    let object_path = unique_temp_path("kernc_internal_symbols", "o");

    fs::write(
        &source_path,
        r#"
use std.io;

fn run_cb(cb: &Fn() i32) i32 {
    return cb();
}

fn main() i32 {    let value = run_cb([]() i32 {
        return 42;
    });
    "{}".fmt(.{"world"}).println();
    "{}".fmt(.{value}).println();
    return 0;
}
"#,
    )
    .unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let object_arg = object_path.to_string_lossy().into_owned();
    let output = run_kernc([
        "-c",
        "--library-bundle",
        "std",
        "--runtime-entry",
        "crt",
        "--runtime-libc",
        "yes",
        source_arg.as_str(),
        "-o",
        object_arg.as_str(),
    ]);

    assert_success(&output, "kernc");

    let nm_output = Command::new("nm")
        .arg("-g")
        .arg(&object_path)
        .output()
        .unwrap();
    assert!(
        nm_output.status.success(),
        "nm failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&nm_output.stdout),
        String::from_utf8_lossy(&nm_output.stderr)
    );
    let symbols = String::from_utf8_lossy(&nm_output.stdout);
    assert!(
        symbols.lines().any(|line| {
            line.split_whitespace()
                .last()
                .is_some_and(|symbol| symbol.trim_start_matches('_') == "main")
        }),
        "expected exported `main`, got:\n{}",
        symbols
    );
    for hidden in [".str.", "__closure_fn_", "__vtable_"] {
        assert!(
            !symbols.contains(hidden),
            "unexpected exported synthesized symbol `{}`:\n{}",
            hidden,
            symbols
        );
    }

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&object_path);
}
