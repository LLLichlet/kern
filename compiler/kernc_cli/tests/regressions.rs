mod support;

use std::fs;
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;

use support::{
    assert_success, build_and_run, compile_source_tree_with_args, compile_source_with_args,
    emit_llvm_ir_with_args, run_kernc, unique_temp_path,
};

fn compile_source(source: &str) -> std::process::Output {
    compile_source_with_args("kernc_regression_test", source, &[])
}

fn compile_source_with_std(source: &str) -> std::process::Output {
    compile_source_with_args("kernc_regression_std_test", source, &["--library-bundle", "std"])
}

fn compile_source_tree(entry: &str, files: &[(&str, &str)]) -> std::process::Output {
    compile_source_tree_with_args("kernc_regression_tree", entry, files, &["-c"])
}

fn build_and_run_source(source: &str) -> std::process::Output {
    build_and_run(
        "kernc_regression_run",
        source,
        &["--runtime-libc", "yes"],
    )
}

fn build_and_run_source_with_std(source: &str) -> std::process::Output {
    build_and_run(
        "kernc_regression_std_run",
        source,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    )
}

#[test]
fn runs_i128_division_and_remainder_without_external_runtime_helpers() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let wide = (u128.{1} << u128.{100}) + u128.{12345};
    let divisor = u128.{97};
    let quotient = wide / divisor;
    let remainder = wide % divisor;
    if (quotient * divisor + remainder != wide) {
        return 1;
    }
    if (remainder >= divisor) {
        return 2;
    }

    let signed_wide = (i128.{0} - (i128.{1} << i128.{100})) + i128.{12345};
    let signed_divisor = i128.{97};
    let signed_quotient = signed_wide / signed_divisor;
    let signed_remainder = signed_wide % signed_divisor;
    if (signed_quotient * signed_divisor + signed_remainder != signed_wide) {
        return 3;
    }
    if (signed_remainder >= i128.{0}) {
        return 4;
    }
    return 0;
}
"#,
    );

    assert!(
        output.status.success(),
        "hosted regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn successful_compile_prints_unused_private_function_warning_and_prunes_ir() {
    let source = r#"
fn helper() i32 {
    return 1;
}

fn main() i32 {
    return 0;
}
"#;

    let output = emit_llvm_ir_with_args("kernc_unused_private_warning", source, &[]);
    assert_success(&output, "kernc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("private function `helper` is never used"),
        "unexpected stderr:\n{}",
        stderr
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("helper"),
        "unused helper leaked into LLVM IR:\n{}",
        stdout
    );
}

#[test]
fn successful_compile_prints_unused_private_constant_warning_and_prunes_ir() {
    let source = r#"
const helper = 1;

fn main() i32 {
    return 0;
}
"#;

    let output = emit_llvm_ir_with_args("kernc_unused_private_const_warning", source, &[]);
    assert_success(&output, "kernc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("private constant `helper` is never used"),
        "unexpected stderr:\n{}",
        stderr
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("helper"),
        "unused helper leaked into LLVM IR:\n{}",
        stdout
    );
}

#[test]
fn successful_compile_prints_unused_private_static_warning_and_prunes_ir() {
    let source = r#"
static helper = 1;

fn main() i32 {
    return 0;
}
"#;

    let output = emit_llvm_ir_with_args("kernc_unused_private_static_warning", source, &[]);
    assert_success(&output, "kernc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("private static `helper` is never used"),
        "unexpected stderr:\n{}",
        stderr
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("helper"),
        "unused helper leaked into LLVM IR:\n{}",
        stdout
    );
}

#[test]
fn resolves_imported_generic_bounds_for_struct_field_literals() {
    let source = r#"
use base.coll.Map;

type Wrap = struct {
    item: Map[u64, i32],
};

fn main() i32 {
    let _ = Wrap.{ item: Map[u64, i32].{} };
    return 0;
}
"#;

    let output = compile_source_with_std(source);
    assert_success(&output, "kernc");
}

#[test]
fn emits_inline_attributes_in_llvm_ir() {
    let source = r#"
#[inline(always)]
fn hot_add(lhs: i32, rhs: i32) i32 {
    return lhs + rhs;
}

#[inline(never)]
fn cold_add(lhs: i32, rhs: i32) i32 {
    return lhs + rhs;
}

fn main() i32 {
    return hot_add(1, 2) + cold_add(3, 4);
}
"#;

    let output = emit_llvm_ir_with_args("kernc_inline_attrs_ir", source, &[]);
    assert_success(&output, "kernc");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("alwaysinline"),
        "expected alwaysinline in LLVM IR, got:\n{}",
        stdout
    );
    assert!(
        stdout.contains("noinline"),
        "expected noinline in LLVM IR, got:\n{}",
        stdout
    );
}

#[test]
fn indexes_const_arrays_through_their_global_storage() {
    let source = r#"
const TABLE = [4]u8.{ 1, 2, 3, 4 };

fn main() i32 {
    return TABLE.[2] as i32;
}
"#;

    let output = emit_llvm_ir_with_args("kernc_const_array_index_ir", source, &[]);
    assert_success(&output, "kernc");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("constant [4 x i8] c\"\\01\\02\\03\\04\""),
        "expected a constant global array in LLVM IR, got:\n{}",
        stdout
    );
    assert!(
        stdout.contains("ptr @_K4root5TABLE"),
        "expected index access to address the global const directly, got:\n{}",
        stdout
    );
    assert!(
        !stdout.contains("tmp_materialized_lvalue"),
        "const array indexing unexpectedly materialized a stack temporary:\n{}",
        stdout
    );
}

#[test]
fn folds_const_fn_array_initializers_into_global_data() {
    let source = r#"
const fn build() [4]mut u8 {
    let mut table = [4]mut u8.{ 0; 4 };
    table.[2] = 7;
    return table;
}

const TABLE = build();

fn main() i32 {
    return TABLE.[2] as i32;
}
"#;

    let output = emit_llvm_ir_with_args("kernc_const_fn_array_init_ir", source, &[]);
    assert_success(&output, "kernc");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("@_K4root5TABLE = global [4 x i8] c\"\\00\\00\\07\\00\""),
        "expected folded global array initializer in LLVM IR, got:\n{}",
        stdout
    );
    assert!(
        !stdout.contains("@_K4root5TABLE = global [4 x i8] zeroinitializer"),
        "const fn array initializer unexpectedly fell back to zero initialization:\n{}",
        stdout
    );
}

#[test]
fn emits_llvm_memmove_for_memmove_intrinsic() {
    let source = r#"
fn main() i32 {
    let buf = [4]mut u8.{ 1, 2, 3, 4 };
    @memmove(buf.[1]..& as *mut u8, buf.[0].& as *u8, 3);
    return 0;
}
"#;

    let output = emit_llvm_ir_with_args("kernc_memmove_ir", source, &[]);
    assert_success(&output, "kernc");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("llvm.memmove"),
        "expected llvm.memmove in LLVM IR, got:\n{}",
        stdout
    );
}

#[test]
fn runs_memmove_intrinsic_with_overlapping_ranges() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let buf = [4]mut u8.{ 1, 2, 3, 4 };
    @memmove(buf.[1]..& as *mut u8, buf.[0].& as *u8, 3);

    if (buf.[0] != 1) return 1;
    if (buf.[1] != 1) return 2;
    if (buf.[2] != 2) return 3;
    if (buf.[3] != 3) return 4;
    return 0;
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
fn compiles_same_private_const_name_in_multiple_modules() {
    let output = compile_source_tree_with_args(
        "kernc_private_const_module_scope",
        "main.rn",
        &[
            (
                "main.rn",
                r#"
mod left;
mod right;

fn main() i32 {
    return left.value() + right.value();
}
"#,
            ),
            (
                "left.rn",
                r#"
const SHARED = 10;

pub fn value() i32 {
    return SHARED as i32;
}
"#,
            ),
            (
                "right.rn",
                r#"
const SHARED = 32;

pub fn value() i32 {
    return SHARED as i32;
}
"#,
            ),
        ],
        &["-c"],
    );

    assert_success(&output, "kernc");
}

#[test]
fn successful_compile_prints_unused_binding_warnings() {
    let source = r#"
fn helper(_: i32, unused_param: i32, used_param: i32) i32 {
    let unused_local = used_param;
    return used_param;
}

fn main() i32 {
    return helper(1, 2, 3);
}
"#;

    let output = emit_llvm_ir_with_args("kernc_unused_bindings_warning", source, &[]);
    assert_success(&output, "kernc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("parameter `unused_param` is never used"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("local variable `unused_local` is never used"),
        "unexpected stderr:\n{}",
        stderr
    );
    assert!(
        !stderr.contains("parameter `_` is never used"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn successful_compile_prints_dead_store_warning() {
    let source = r#"
fn helper(seed: i32) i32 {
    let mut value = seed;
    if (seed == 0) {
        return value;
    }
    value = seed + 1;
    value = seed + 2;
    return value;
}

fn main() i32 {
    return helper(1);
}
"#;

    let output = emit_llvm_ir_with_args("kernc_dead_store_warning", source, &[]);
    assert_success(&output, "kernc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("value assigned to `value` is never read"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn successful_compile_prints_dead_initializer_warning() {
    let source = r#"
fn helper(seed: i32) i32 {
    let mut value = seed;
    value = seed + 1;
    return value;
}

fn main() i32 {
    return helper(1);
}
"#;

    let output = emit_llvm_ir_with_args("kernc_dead_initializer_warning", source, &[]);
    assert_success(&output, "kernc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("initial value assigned to `value` is never read"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn pure_enum_payload_bound_from_match_compiles_and_runs() {
    let output = build_and_run_source(
        r#"
type Kind = enum {
    Root,
    Section,
};

type MaybeKind = enum {
    None,
    Some: Kind,
};

fn unwrap_kind(value: MaybeKind) Kind {
    return match (value) {
        .{ Some: kind } => kind,
        .None => Kind.Root,
    };
}

fn main() i32 {
    let kind = unwrap_kind(MaybeKind.{ Some: Kind.Section });
    match (kind) {
        .Root => return 1,
        .Section => return 0,
    }
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "program exited unexpectedly:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn method_returning_option_of_pure_enum_compiles_and_runs() {
    let output = build_and_run_source(
        r#"
type Kind = enum {
    Root,
    Section,
};

type Option[T] = enum {
    None,
    Some: T,
};

type Holder = struct {};

impl Holder {
    fn section_kind(flag: bool) Option[Kind] {
        if (flag) {
            return .{ Some: Kind.Section };
        }
        return .{ None };
    }
}

fn main() i32 {
    let holder = Holder.{};
    let kind = match (holder.section_kind(true)) {
        .{ Some: kind } => kind,
        .None => Kind.Root,
    };
    match (kind) {
        .Root => return 1,
        .Section => return 0,
    }
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "program exited unexpectedly:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn public_reexport_keeps_private_function_reachable_in_ir() {
    let source = r#"
fn helper() i32 {
    return 1;
}

pub use .helper as exported;

fn main() i32 {
    return 0;
}
"#;

    let output = emit_llvm_ir_with_args("kernc_reexport_root_ir", source, &[]);
    assert_success(&output, "kernc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("private function `helper` is never used"),
        "unexpected stderr:\n{}",
        stderr
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("helper"),
        "reexport-root helper unexpectedly pruned from LLVM IR:\n{}",
        stdout
    );
}

#[test]
fn imports_kmeta_package_with_alias_and_links_against_real_package_name() {
    let root = unique_temp_path("kernc_kmeta_pkg", "dir");
    let lib_dir = root.join("lib");
    let metadata_dir = root.join("kmeta");
    let lib_entry = lib_dir.join("init.rn");
    let lib_object = root.join("util.o");
    let main_source = root.join("main.rn");
    let exe_ext = if cfg!(windows) { "exe" } else { "out" };
    let executable = root.join(format!("app.{}", exe_ext));

    fs::create_dir_all(&lib_dir).unwrap();
    fs::create_dir_all(&metadata_dir).unwrap();

    fs::write(
        &lib_entry,
        r#"
pub fn answer() i32 {
    return 42;
}
"#,
    )
    .unwrap();

    let lib_entry_arg = lib_entry.to_string_lossy().into_owned();
    let lib_object_arg = lib_object.to_string_lossy().into_owned();
    let metadata_arg = metadata_dir.to_string_lossy().into_owned();
    let lib_output = run_kernc([
        "-c",
        "--root-module",
        "util",
        "--emit-kmeta",
        metadata_arg.as_str(),
        lib_entry_arg.as_str(),
        "-o",
        lib_object_arg.as_str(),
    ]);
    assert_success(&lib_output, "kernc library compile");

    let manifest = fs::read_to_string(metadata_dir.join("Kmeta.toml")).unwrap();
    assert!(
        manifest.contains("package_name = \"util\""),
        "unexpected kmeta manifest:\n{}",
        manifest
    );
    assert!(
        manifest.contains("root_module_name = \"util\""),
        "unexpected kmeta manifest:\n{}",
        manifest
    );

    fs::write(
        &main_source,
        r#"
fn main() i32 {
    return if (dep.answer() == 42) { 0 } else { 1 };
}
"#,
    )
    .unwrap();

    let main_source_arg = main_source.to_string_lossy().into_owned();
    let dep_mapping = format!("dep={}", metadata_dir.to_string_lossy());
    let exe_arg = executable.to_string_lossy().into_owned();
    let app_output = run_kernc([
        "--runtime-entry",
        "crt",
        "--runtime-provider",
        "toolchain",
        "--runtime-libc",
        "yes",
        "-I",
        dep_mapping.as_str(),
        "--link-input",
        lib_object_arg.as_str(),
        main_source_arg.as_str(),
        "-o",
        exe_arg.as_str(),
    ]);
    assert_success(&app_output, "kernc app compile");

    let run_output = Command::new(&executable).output().unwrap();
    assert!(
        run_output.status.success(),
        "compiled program failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn kmeta_snapshot_keeps_cfg_gated_submodule_sources() {
    let root = unique_temp_path("kernc_kmeta_cfg_submodule", "dir");
    let lib_dir = root.join("lib");
    let inner_dir = lib_dir.join("inner");
    let metadata_dir = root.join("kmeta");
    let lib_entry = lib_dir.join("init.rn");
    let inner_entry = inner_dir.join("init.rn");
    let gated_entry = inner_dir.join("entry.rn");
    let lib_object = root.join("pkg.o");

    fs::create_dir_all(&inner_dir).unwrap();
    fs::create_dir_all(&metadata_dir).unwrap();

    fs::write(
        &lib_entry,
        r#"
pub mod inner;
"#,
    )
    .unwrap();
    fs::write(
        &inner_entry,
        r#"
#[if(runtime_entry != "none")]
pub mod entry;

pub fn answer() i32 {
    return 42;
}
"#,
    )
    .unwrap();
    fs::write(
        &gated_entry,
        r#"
pub fn hidden() i32 {
    return 7;
}
"#,
    )
    .unwrap();

    let lib_entry_arg = lib_entry.to_string_lossy().into_owned();
    let lib_object_arg = lib_object.to_string_lossy().into_owned();
    let metadata_arg = metadata_dir.to_string_lossy().into_owned();
    let output = run_kernc([
        "-c",
        "--root-module",
        "pkg",
        "--emit-kmeta",
        metadata_arg.as_str(),
        lib_entry_arg.as_str(),
        "-o",
        lib_object_arg.as_str(),
    ]);
    assert_success(&output, "kernc kmeta snapshot");

    assert!(
        metadata_dir.join("src/inner/entry.rn").is_file(),
        "cfg-gated submodule source missing from kmeta snapshot"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn concurrent_kmeta_snapshot_writers_serialize_on_output_lock() {
    let root = unique_temp_path("kernc_kmeta_parallel", "dir");
    let lib_dir = root.join("lib");
    let metadata_dir = root.join("kmeta");
    let lib_entry = lib_dir.join("init.rn");
    let object_a = root.join("pkg-a.o");
    let object_b = root.join("pkg-b.o");

    fs::create_dir_all(&lib_dir).unwrap();
    fs::create_dir_all(&metadata_dir).unwrap();
    fs::write(metadata_dir.join("stale.txt"), "stale").unwrap();

    let mut root_source = String::new();
    for i in 0..96 {
        let module_name = format!("m{i:03}");
        root_source.push_str(&format!("pub mod {module_name};\n"));
        fs::write(
            lib_dir.join(format!("{module_name}.rn")),
            format!(
                r#"
pub fn answer() i32 {{
    return {i};
}}
"#
            ),
        )
        .unwrap();
    }
    fs::write(&lib_entry, root_source).unwrap();

    let entry_arg = lib_entry.to_string_lossy().into_owned();
    let metadata_arg = metadata_dir.to_string_lossy().into_owned();
    let object_a_arg = object_a.to_string_lossy().into_owned();
    let object_b_arg = object_b.to_string_lossy().into_owned();
    let repo_root = support::repo_root();
    let barrier = Arc::new(Barrier::new(3));

    let spawn = |object_arg: String| {
        let barrier = Arc::clone(&barrier);
        let entry_arg = entry_arg.clone();
        let metadata_arg = metadata_arg.clone();
        let repo_root = repo_root.clone();
        thread::spawn(move || {
            barrier.wait();
            Command::new(env!("CARGO_BIN_EXE_kernc"))
                .current_dir(repo_root)
                .args([
                    "-c",
                    "--root-module",
                    "pkg",
                    "--emit-kmeta",
                    metadata_arg.as_str(),
                    entry_arg.as_str(),
                    "-o",
                    object_arg.as_str(),
                ])
                .output()
                .unwrap()
        })
    };

    let compile_a = spawn(object_a_arg);
    let compile_b = spawn(object_b_arg);
    barrier.wait();

    let output_a = compile_a.join().unwrap();
    let output_b = compile_b.join().unwrap();
    assert_success(&output_a, "kernc concurrent kmeta writer A");
    assert_success(&output_b, "kernc concurrent kmeta writer B");
    assert!(metadata_dir.join("Kmeta.toml").is_file());
    assert!(metadata_dir.join("src/init.rn").is_file());

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn rejects_raw_source_tree_passed_via_imported_interface_alias() {
    let root = unique_temp_path("kernc_kmeta_reject_raw", "dir");
    let dep_dir = root.join("dep");
    let dep_entry = dep_dir.join("init.rn");
    let main_source = root.join("main.rn");
    let object_path = root.join("app.o");

    fs::create_dir_all(&dep_dir).unwrap();
    fs::write(
        &dep_entry,
        r#"
pub fn answer() i32 {
    return 7;
}
"#,
    )
    .unwrap();
    fs::write(
        &main_source,
        r#"
fn main() i32 {
    return dep.answer();
}
"#,
    )
    .unwrap();

    let dep_mapping = format!("dep={}", dep_dir.to_string_lossy());
    let main_source_arg = main_source.to_string_lossy().into_owned();
    let object_arg = object_path.to_string_lossy().into_owned();
    let output = run_kernc([
        "-c",
        "-I",
        dep_mapping.as_str(),
        main_source_arg.as_str(),
        "-o",
        object_arg.as_str(),
    ]);

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted a raw source tree for -I:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing `Kmeta.toml`") || stderr.contains("expects a kmeta package root"),
        "unexpected stderr:\n{}",
        stderr
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn compiles_generic_supertrait_method_lookup() {
    let output = compile_source(
        r#"
type Base = trait {
    foo: fn() i32,
};

type Derived[U]: Base = trait {
    add: fn(U) i32,
};

impl *i32 : Base {
    pub fn foo() i32 {
        return self.*;
    }
}

impl *i32 : Derived[i32] {
    pub fn add(v: i32) i32 {
        return self.* + v;
    }
}

fn use_it[T](value: *T) i32
    where *T: Derived[i32],
{
    return value.foo() + value.add(2);
}

fn main() i32 {
    let value = i32.{5};
    return use_it(value.&);
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
fn preserves_outer_binding_after_shadowing_match_payload() {
    let output = build_and_run_source_with_std(
        r#"
use base.Result;
use base.coll.String;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn make_text(alloc: *mut base.mem.alloc.Allocator, text: []u8) Result[String, i32] {
    let mut out = String.{};
    if (!out..&.push_str(alloc, text)) {
        return .{ Err: 1 };
    }
    return .{ Ok: out };
}

fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    defer gpa.deinit();

    let mut text = match (make_text(gpa, "kern-lang")) {
        .{ Ok: text } => text,
        .{ Err: _ } => return 1,
    };
    defer text..&.deinit(gpa);

    let mut text2 = match (make_text(gpa, "kern")) {
        .{ Ok: text } => text,
        .{ Err: _ } => return 2,
    };
    text2..&.deinit(gpa);

    if (!text.&.eq("kern-lang")) {
        return 3;
    }

    return 0;
}
"#,
    );

    assert!(
        output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_const_enum_and_const_array_usage() {
    let output = compile_source(
        r#"
type Mode: u8 = enum {
    Off,
    On,
};

type Option[T] = enum {
    None,
    Some: T,
};

const TABLE = [_]u8.{ 3, 5, 8 };
const DEFAULT_MODE = Mode.On;
const VALUE = Option[i32].{ Some: 7 };

fn main() i32 {
    let mode = match (DEFAULT_MODE) {
        .Off => i32.{0},
        .On => i32.{10},
    };

    let picked = match (VALUE) {
        .None => i32.{0},
        .{ Some: v } => v,
    };

    return mode + picked + (TABLE.[1] as i32);
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
fn compiles_type_qualified_payloadless_enum_variants_in_const_and_runtime_contexts() {
    let output = build_and_run_source(
        r#"
type DocumentKind = enum {
    KeyValue,
    Table,
};

type Option[T] = enum {
    None,
    Some: T,
};

const DEFAULT_KIND = DocumentKind.KeyValue;
const EMPTY = Option[i32].None;

const fn score(kind: DocumentKind, value: Option[i32]) i32 {
    let kind_score = match (kind) {
        .KeyValue => i32.{11},
        .Table => i32.{17},
    };

    let value_score = match (value) {
        .None => i32.{5},
        .{ Some: inner } => inner,
    };

    return kind_score + value_score;
}

const TOTAL = score(DocumentKind.KeyValue, Option[i32].None);

fn passthrough(value: Option[i32]) Option[i32] {
    return value;
}

fn main() i32 {
    let contextual = passthrough(.None);
    let some = Option[i32].{ Some: 19 };

    let base = score(DEFAULT_KIND, EMPTY);
    let contextual_score = match (contextual) {
        .None => i32.{3},
        .{ Some: _ } => i32.{100},
    };
    let some_score = match (some) {
        .None => i32.{100},
        .{ Some: inner } => inner,
    };

    return TOTAL + base + contextual_score + some_score;
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(54),
        "hosted regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_type_qualified_payload_variant_without_braces() {
    let output = compile_source(
        r#"
type Option[T] = enum {
    None,
    Some: T,
};

fn main() i32 {
    let value = Option[i32].Some;
    return match (value) {
        .None => 0,
        .{ Some: inner } => inner,
    };
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
        stderr.contains("variant `Some` requires a payload"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn compiles_if_expression_returning_type_qualified_payloadless_variants() {
    let output = build_and_run_source(
        r#"
type DocumentKind = enum {
    KeyValue,
    Table,
};

fn main() i32 {
    let kind = if (true) {
        DocumentKind.Table
    } else {
        DocumentKind.KeyValue
    };

    return match (kind) {
        .KeyValue => 1,
        .Table => 0,
    };
}
"#,
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "hosted regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_imported_type_alias_payloadless_variants_in_if_expressions() {
    let output = compile_source_tree(
        "main.rn",
        &[
            (
                "main.rn",
                r#"
mod kinds;
use .kinds.DocumentKind;

fn main() i32 {
    let kind = if (true) {
        DocumentKind.Table
    } else {
        DocumentKind.KeyValue
    };

    return match (kind) {
        .KeyValue => 1,
        .Table => 0,
    };
}
"#,
            ),
            (
                "kinds.rn",
                r#"
pub type DocumentKind = enum {
    KeyValue,
    Table,
};
"#,
            ),
        ],
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_const_fn_in_global_array_len_and_method_calls() {
    let output = compile_source(
        r#"
type Switch = enum {
    Off = 0,
    On = 1,
    Value: i32,
};

type Pair = struct {
    left: i32,
    right: i32,
};

const fn inc(v: i32) i32 {
    let next = v + 1;
    return next;
}

const fn id[T](value: T) T {
    return value;
}

const fn choose(flag: bool) Switch {
    if (flag) {
        return Switch.{ Value: 7 };
    }
    return Switch.{ On };
}

const fn unwrap_switch(v: Switch) i32 {
    match (v) {
        .Off => 0,
        .On => 1,
        .{ Value: payload } => payload,
    }
}

impl Pair {
    pub const fn sum() i32 {
        let total = self.left + self.right;
        return total;
    }
}

const TABLE = [inc(3)]u8.{ 1, 2, 3, 4 };
const TOTAL = unwrap_switch(choose(true)) + Pair.{ left: 5, right: id[i32](3) }.sum() + (TABLE.[3] as i32);

fn main() i32 {
    return TOTAL;
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
fn compiles_concrete_slice_impl_methods() {
    let output = compile_source(
        r#"
fn slice_len(value: []u8) usize {
    return #value;
}

impl []u8 {
    pub fn len_via_impl() usize {
        return slice_len(self);
    }
}

fn main() i32 {
    let text = "hi";
    return text.len_via_impl() as i32;
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
fn compiles_generic_std_helper_calling_layout_of_recursive_type() {
    let output = compile_source_with_std(
        r#"
use base.mem.layout_of;

type Node[K, V] = struct {
    next: *mut Node[K, V],
    key: K,
    value: V,
};

fn free_node[K, V](alloc: *mut base.mem.alloc.Allocator, node: *mut Node[K, V]) void {
    alloc.free(node as *mut u8, layout_of[Node[K, V]]());
}

fn wrap_free[K, V](alloc: *mut base.mem.alloc.Allocator, node: *mut Node[K, V]) void {
    free_node(alloc, node);
}

fn main() i32 {
    let _ = wrap_free[i32, i32];
    return 0;
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
fn prunes_mutually_exclusive_extern_blocks_before_name_collection() {
    let output = compile_source(
        r#"
#[if(arch == "x86_64")]
extern {
    fn system_probe() i32;
}

#[if(arch == "aarch64")]
extern {
    fn system_probe() i32;
}

fn main() i32 {
    return 0;
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
fn runs_captured_closure_boundary_conversions() {
    let output = build_and_run_source(
        r#"
fn use_closure(cb: *Fn() i32) i32 {
    return cb();
}

fn use_mut_closure(cb: *mut Fn() void) void {
    cb();
}

fn main() i32 {
    let mut calls = i32.{0};
    let value = use_closure(.[ptr = calls..&]() i32 {
        ptr.* += 1;
        return 77;
    });
    if (value != 77) {
        return 1;
    }
    if (calls != 1) {
        return 2;
    }

    let mut counter = i32.{0};
    let mut closure = .[ptr = counter..&]() void {
        ptr.* += 1;
    };
    use_mut_closure(closure);
    if (counter != 1) {
        return 3;
    }

    return 0;
}
"#,
    );

    assert!(
        output.status.success(),
        "hosted regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_assignment_through_struct_array_fields_only() {
    let output = compile_source(
        r#"
type Buffer = struct {
    items: [4]i32,
};

fn main() i32 {
    let mut buf = Buffer.{ items: [4]i32.{ 0; 4 } };
    buf.items.[0] = 5;

    let ptr = buf..&;
    ptr.items.[1] = 7;

    return buf.items.[0] + ptr.items.[1];
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
fn runs_array_and_slice_mutability_semantics() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let arr = [5]mut u8.{ b'a', b'b', b'c', b'd', b'e' };
    arr.[1] = b'x';
    if (arr.[1] != b'x') {
        return 1;
    }

    let view = arr..[1 .. 4];
    view.[0] = b'd';
    view.[1] = b'y';
    view.[2] = b'x';
    if (arr.[1] != b'd') {
        return 2;
    }
    if (arr.[2] != b'y') {
        return 3;
    }
    if (arr.[3] != b'x') {
        return 4;
    }

    let mut whole = [3]u8.{ b'1', b'2', b'3' };
    whole = [3]u8.{ b'4', b'5', b'6' };
    if (whole.[0] != b'4' or whole.[1] != b'5' or whole.[2] != b'6') {
        return 5;
    }

    return 0;
}
"#,
    );

    assert!(
        output.status.success(),
        "hosted regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn runs_zig_style_multiline_strings() {
    let output = build_and_run(
        "kernc_multiline_string_run",
        r#"
use std.io;

fn main() i32 {

    let msg =
        \\line one
        \\line "two"
        \\line three
    ;

    let mut out = io.stdout();
    let _ = out..&.write(msg);
    let _ = out..&.write("\n");
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    assert!(
        output.status.success(),
        "hosted regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "line one\nline \"two\"\nline three\n"
    );
}

#[test]
fn compiles_and_runs_trailing_commas_in_common_lists() {
    let output = build_and_run_source(
        r#"
type Pair[T,] = struct {
    left: T,
    right: T,
};

type Choice = enum {
    A,
    B,
};

type Ops = trait {
    run: fn(i32, i32,) i32,
};

fn add(a: i32, b: i32,) i32 {
    return a + b;
}

fn sum_pair(pair: Pair[i32,],) i32 {
    let values = [2]i32.{ pair.left, pair.right, };
    match (pair.left) {
        2, => return add(values.[0], values.[1],),
        _ => return 1,
    }
}

fn main() i32 {
    let pair = Pair[i32,].{ left: 2, right: 3, };
    if (sum_pair(pair,) == 5) {
        return 0;
    }
    return 1;
}
"#,
    );

    assert!(
        output.status.success(),
        "trailing comma regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn runs_defer_after_return_value_evaluation() {
    let output = build_and_run_source(
        r#"
type Guard = struct {
    ptr: *mut i32,
};

impl *mut Guard {
    pub fn deinit() void {
        self.ptr.* = 2;
    }
}

fn read_before_defer() i32 {
    let mut state = i32.{1};
    let mut guard = Guard.{ ptr: state..& };
    defer guard..&.deinit();
    return state;
}

fn main() i32 {
    if (read_before_defer() != 1) {
        return 1;
    }
    return 0;
}
"#,
    );

    assert!(
        output.status.success(),
        "hosted regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn runs_match_arm_block_with_statement_before_return() {
    let output = build_and_run_source(
        r#"
type Result[T, E] = enum {
    Ok: T,
    Err: E,
};

fn fail() Result[i32, i32] {
    return .{ Err: 7 };
}

fn main() i32 {
    let _ = match (fail()) {
        .{ Ok: v } => v,
        .{ Err: _err } => {
            let _ = i32.{0};
            return 0;
        },
    };

    return 1;
}
"#,
    );

    assert!(
        output.status.success(),
        "hosted regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_returning_never_expression_without_emitting_extra_ret() {
    let output = compile_source(
        r#"
fn fail() bool {
    return @trap();
}

fn main() i32 {
    let _ = fail();
    return 0;
}
"#,
    );

    assert_success(&output, "kernc");
}

#[test]
fn compiles_generic_helper_returning_match_of_never_arms() {
    let output = compile_source(
        r#"
type Result[T, E] = enum {
    Ok: T,
    Err: E,
};

fn expect_ok[T, E](value: Result[T, E]) T {
    match (value) {
        .{ Ok: payload } => return payload,
        .{ Err: _ } => {
            return match (0) {
                0 => @trap(),
                _ => @trap(),
            };
        },
    }
}

fn main() i32 {
    let _ = expect_ok[i32, bool](.{ Ok: 7 });
    return 0;
}
"#,
    );

    assert_success(&output, "kernc");
}

#[test]
fn compiles_never_in_let_initializer_without_emitting_store() {
    let output = compile_source(
        r#"
fn main() i32 {
    let x = @trap();
    let _ = x;
    return 0;
}
"#,
    );

    assert_success(&output, "kernc");
}

#[test]
fn compiles_never_in_call_argument_without_emitting_followup_call() {
    let output = compile_source(
        r#"
fn consume(value: i32) void {
    let _ = value;
}

fn main() i32 {
    consume(@trap());
    return 0;
}
"#,
    );

    assert_success(&output, "kernc");
}

#[test]
fn runs_for_clauses_with_non_void_init_post_and_body() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let mut phase = i32.{0};

    for (
        { phase += i32.{2}; i32.{99} };
        phase < i32.{3};
        { phase += i32.{10}; i32.{88} }
    ) {
        phase += i32.{1};
        i32.{77}
    }

    return phase - i32.{13};
}
"#,
    );

    assert!(
        output.status.success(),
        "hosted regression binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_const_fn_loops_with_assignment_break_and_continue() {
    let output = compile_source(
        r#"
const fn sum_skip(limit: i32) i32 {
    let mut acc = i32.{0};

    for (let mut i = i32.{0}; i < limit; i += i32.{1}) {
        if (i == i32.{2}) {
            continue;
        }
        if (i == i32.{5}) {
            break;
        }
        acc += i;
    }

    return acc;
}

const TOTAL = sum_skip(i32.{7});

fn main() i32 {
    return TOTAL;
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
fn compiles_const_fn_mutating_local_struct_fields_and_array_elements() {
    let output = compile_source(
        r#"
type Pair = struct {
    left: i32,
    right: i32,
};

const fn build_total() i32 {
    let mut pair = Pair.{ left: 1, right: 2 };
    pair.left += 4;
    pair.right = pair.left + pair.right;

    let mut items = [3]mut i32.{ 5, 6, 7 };
    items.[1] = pair.right;
    items.[2] += items.[0];

    return pair.right + items.[1] + items.[2];
}

const TOTAL = build_total();

fn main() i32 {
    return TOTAL;
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
fn compiles_const_fn_mutating_local_through_pointer() {
    let output = compile_source(
        r#"
const fn bump(ptr: *mut i32) void {
    ptr.* += 1;
}

const fn run() i32 {
    let mut value = i32.{1};
    bump(value..&);
    return value;
}

const RESULT = run();

fn main() i32 {
    return RESULT;
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
fn compiles_mut_pointer_to_array_whole_value_assignment() {
    let output = compile_source(
        r#"
fn replace(buf: *mut [4]u8) void {
    buf.* = [4]u8.{ 1, 2, 3, 4 };
}

fn main() i32 {
    return 0;
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
fn compiles_pointer_to_mut_array_element_assignment() {
    let output = compile_source(
        r#"
fn write(buf: *[4]mut u8, index: usize, value: u8) void {
    buf.*.[index] = value;
}

fn main() i32 {
    return 0;
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
fn rejects_element_assignment_through_mut_pointer_to_non_mut_array() {
    let output = compile_source(
        r#"
fn write(buf: *mut [4]u8, index: usize, value: u8) void {
    buf.*.[index] = value;
}

fn main() i32 {
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
        stderr.contains("cannot assign to an immutable variable or location"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn compiles_const_fn_mutating_struct_field_through_pointer_auto_deref() {
    let output = compile_source(
        r#"
type Counter = struct {
    value: i32,
};

const fn bump(counter: *mut Counter) void {
    counter.value += 3;
}

const fn run() i32 {
    let mut counter = Counter.{ value: 4 };
    bump(counter..&);
    return counter.value;
}

const RESULT = run();

fn main() i32 {
    return RESULT;
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
fn rejects_assignment_through_non_mut_array_elements() {
    let output = compile_source(
        r#"
fn main() i32 {
    let mut arr = [4]i32.{ 0; 4 };
    arr.[0] = 3;
    return arr.[0];
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
        stderr.contains("cannot assign to an immutable variable or location"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_rebinding_immutable_array_binding() {
    let output = compile_source(
        r#"
fn main() i32 {
    let arr = [3]u8.{ b'a', b'b', b'c' };
    arr = [3]u8.{ b'x', b'y', b'z' };
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
        stderr.contains("cannot assign to an immutable variable or location"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_non_const_fn_in_const_context() {
    let output = compile_source(
        r#"
fn runtime_only(v: i32) i32 {
    return v + 1;
}

const BAD = runtime_only(1);

fn main() i32 {
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
        stderr.contains("only `const fn` can be called in constant expressions"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_arrays_larger_than_llvm_indexable_limit() {
    let output = compile_source(
        r#"
fn main() i32 {
    let _ = [4294967296]u8.{ undef };
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
}

#[test]
fn allows_private_named_struct_fields_within_defining_module() {
    let output = compile_source_tree(
        "main.rn",
        &[
            (
                "main.rn",
                r#"
mod data;

fn main() i32 {
    return data.read_secret();
}
"#,
            ),
            (
                "data.rn",
                r#"
pub type Bag = struct {
    secret: i32,
    pub open: i32,
};

pub fn read_secret() i32 {
    let bag = Bag.{ secret: 5, open: 8 };
    return bag.secret + bag.open;
}
"#,
            ),
        ],
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_private_named_struct_fields_across_modules() {
    let output = compile_source_tree(
        "main.rn",
        &[
            (
                "main.rn",
                r#"
mod data;

fn main() i32 {
    let bag = data.make();
    return bag.secret + bag.open;
}
"#,
            ),
            (
                "data.rn",
                r#"
pub type Bag = struct {
    secret: i32,
    pub open: i32,
};

pub fn make() Bag {
    return Bag.{ secret: 5, open: 8 };
}
"#,
            ),
        ],
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("field `secret` of type `Bag` is private"),
        "unexpected stderr:\n{}",
        stderr
    );
}


