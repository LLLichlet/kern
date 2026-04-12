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
fn std_bundle_does_not_expose_legacy_std_coll_module() {
    let output = compile_source_with_args(
        "kernc_std_legacy_coll_module",
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
fn runtime_entry_does_not_auto_inject_base_or_sys_modules() {
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
        "expected base/sys to remain unresolved without an explicit bundle or module path:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("cannot find module `base`"),
        "unexpected stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn std_bundle_does_not_expose_page_allocator_through_base_alloc() {
    let output = compile_source_with_args(
        "kernc_base_alloc_page_removed",
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
    let source = repo_root().join("examples/hello_world.rn");
    let object = unique_temp_path("kernc_std_hello_world", "o");

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

    let _ = fs::remove_file(&object);
}

#[cfg(windows)]
#[test]
fn compiles_std_hello_world_to_unicode_object_path() {
    let source = repo_root().join("examples/hello_world.rn");
    let object = unique_temp_path("kernc_std_hello_world_\u{4F60}\u{597D}", "o");

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
    io.println("link only", .{});
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

fn run_cb(cb: *Fn() i32) i32 {
    return cb();
}

fn main() i32 {    let value = run_cb(.[]() i32 {
        return 42;
    });
    io.println("{}", .{"world",});
    io.println("{}", .{value,});
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
