use super::*;

#[test]
fn imports_kmeta_package_with_alias_and_links_against_real_package_name() {
    let root = unique_temp_path("kernc_kmeta_pkg", "dir");
    let lib_dir = root.join("lib");
    let metadata_dir = root.join("kmeta");
    let lib_entry = lib_dir.join("mod.kn");
    let lib_object = root.join("util.o");
    let main_source = root.join("main.kn");
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
        "--module-root-name",
        "util",
        "--metadata-output",
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
    let base_mapping = format!("base={}", repo_root().join("library/base").display());
    let app_output = run_kernc([
        "--runtime-entry",
        "crt",
        "--runtime-libc",
        "yes",
        "--module-path",
        base_mapping.as_str(),
        "--module-interface-path",
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
fn imported_package_cannot_access_pub_package_items() {
    let root = unique_temp_path("kernc_pub_package_external", "dir");
    let lib_dir = root.join("lib");
    let metadata_dir = root.join("kmeta");
    let lib_entry = lib_dir.join("mod.kn");
    let lib_object = root.join("util.o");
    let main_source = root.join("main.kn");

    fs::create_dir_all(&lib_dir).unwrap();
    fs::create_dir_all(&metadata_dir).unwrap();

    fs::write(
        &lib_entry,
        r#"
pub/ fn answer() i32 {
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
        "--module-root-name",
        "util",
        "--metadata-output",
        metadata_arg.as_str(),
        lib_entry_arg.as_str(),
        "-o",
        lib_object_arg.as_str(),
    ]);
    assert_success(&lib_output, "kernc library compile");

    fs::write(
        &main_source,
        r#"
fn main() i32 {
    return dep.answer();
}
"#,
    )
    .unwrap();

    let main_source_arg = main_source.to_string_lossy().into_owned();
    let dep_mapping = format!("dep={}", metadata_dir.to_string_lossy());
    let base_mapping = format!("base={}", repo_root().join("library/base").display());
    let app_output = run_kernc([
        "--runtime-entry",
        "crt",
        "--runtime-libc",
        "yes",
        "--module-path",
        base_mapping.as_str(),
        "--module-interface-path",
        dep_mapping.as_str(),
        "--link-input",
        lib_object_arg.as_str(),
        main_source_arg.as_str(),
        "-o",
        root.join("app.out").to_string_lossy().as_ref(),
    ]);

    assert!(
        !app_output.status.success(),
        "kernc unexpectedly allowed external access to pub/ item:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&app_output.stdout),
        String::from_utf8_lossy(&app_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&app_output.stderr);
    assert!(
        stderr.contains("module has no visible member `answer`")
            || stderr.contains("Symbol `answer` is not visible from this module"),
        "unexpected stderr:\n{}",
        stderr
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn imported_package_cannot_access_pub_package_struct_fields() {
    let root = unique_temp_path("kernc_pub_package_field_external", "dir");
    let lib_dir = root.join("lib");
    let metadata_dir = root.join("kmeta");
    let lib_entry = lib_dir.join("mod.kn");
    let lib_object = root.join("util.o");
    let main_source = root.join("main.kn");

    fs::create_dir_all(&lib_dir).unwrap();
    fs::create_dir_all(&metadata_dir).unwrap();

    fs::write(
        &lib_entry,
        r#"
pub struct Bag {
    pub/ shared: i32,
};

pub fn make() Bag {
    return Bag.{ shared: 42 };
}
"#,
    )
    .unwrap();

    let lib_entry_arg = lib_entry.to_string_lossy().into_owned();
    let lib_object_arg = lib_object.to_string_lossy().into_owned();
    let metadata_arg = metadata_dir.to_string_lossy().into_owned();
    let lib_output = run_kernc([
        "-c",
        "--module-root-name",
        "util",
        "--metadata-output",
        metadata_arg.as_str(),
        lib_entry_arg.as_str(),
        "-o",
        lib_object_arg.as_str(),
    ]);
    assert_success(&lib_output, "kernc library compile");

    fs::write(
        &main_source,
        r#"
fn main() i32 {
    let bag = dep.make();
    return bag.shared;
}
"#,
    )
    .unwrap();

    let main_source_arg = main_source.to_string_lossy().into_owned();
    let dep_mapping = format!("dep={}", metadata_dir.to_string_lossy());
    let base_mapping = format!("base={}", repo_root().join("library/base").display());
    let app_output = run_kernc([
        "--runtime-entry",
        "crt",
        "--runtime-libc",
        "yes",
        "--module-path",
        base_mapping.as_str(),
        "--module-interface-path",
        dep_mapping.as_str(),
        "--link-input",
        lib_object_arg.as_str(),
        main_source_arg.as_str(),
        "-o",
        root.join("app.out").to_string_lossy().as_ref(),
    ]);

    assert!(
        !app_output.status.success(),
        "kernc unexpectedly allowed external access to pub/ field:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&app_output.stdout),
        String::from_utf8_lossy(&app_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&app_output.stderr);
    assert!(
        stderr.contains("field `shared` of type `Bag` is private"),
        "unexpected stderr:\n{}",
        stderr
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn imported_package_cannot_initialize_pub_package_struct_fields() {
    let root = unique_temp_path("kernc_pub_package_field_init_external", "dir");
    let lib_dir = root.join("lib");
    let metadata_dir = root.join("kmeta");
    let lib_entry = lib_dir.join("mod.kn");
    let lib_object = root.join("util.o");
    let main_source = root.join("main.kn");

    fs::create_dir_all(&lib_dir).unwrap();
    fs::create_dir_all(&metadata_dir).unwrap();

    fs::write(
        &lib_entry,
        r#"
pub struct Bag {
    pub/ shared: i32,
};
"#,
    )
    .unwrap();

    let lib_entry_arg = lib_entry.to_string_lossy().into_owned();
    let lib_object_arg = lib_object.to_string_lossy().into_owned();
    let metadata_arg = metadata_dir.to_string_lossy().into_owned();
    let lib_output = run_kernc([
        "-c",
        "--module-root-name",
        "util",
        "--metadata-output",
        metadata_arg.as_str(),
        lib_entry_arg.as_str(),
        "-o",
        lib_object_arg.as_str(),
    ]);
    assert_success(&lib_output, "kernc library compile");

    fs::write(
        &main_source,
        r#"
fn main() i32 {
    let bag = dep.Bag.{ shared: 42 };
    return 0;
}
"#,
    )
    .unwrap();

    let main_source_arg = main_source.to_string_lossy().into_owned();
    let dep_mapping = format!("dep={}", metadata_dir.to_string_lossy());
    let base_mapping = format!("base={}", repo_root().join("library/base").display());
    let app_output = run_kernc([
        "--runtime-entry",
        "crt",
        "--runtime-libc",
        "yes",
        "--module-path",
        base_mapping.as_str(),
        "--module-interface-path",
        dep_mapping.as_str(),
        "--link-input",
        lib_object_arg.as_str(),
        main_source_arg.as_str(),
        "-o",
        root.join("app.out").to_string_lossy().as_ref(),
    ]);

    assert!(
        !app_output.status.success(),
        "kernc unexpectedly allowed external initialization of pub/ field:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&app_output.stdout),
        String::from_utf8_lossy(&app_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&app_output.stderr);
    assert!(
        stderr.contains("field `shared` of type `Bag` is private"),
        "unexpected stderr:\n{}",
        stderr
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn rejects_orphan_impl_of_imported_trait_for_foreign_primitive() {
    let root = unique_temp_path("kernc_orphan_foreign_primitive", "dir");
    let lib_dir = root.join("lib");
    let metadata_dir = root.join("kmeta");
    let lib_entry = lib_dir.join("mod.kn");
    let lib_object = root.join("iface.o");
    let main_source = root.join("main.kn");

    fs::create_dir_all(&lib_dir).unwrap();
    fs::create_dir_all(&metadata_dir).unwrap();

    fs::write(
        &lib_entry,
        r#"
pub trait Foreign {
    fn ping() i32;
};
"#,
    )
    .unwrap();

    let lib_output = run_kernc([
        "-c",
        "--module-root-name",
        "iface",
        "--metadata-output",
        metadata_dir.to_string_lossy().as_ref(),
        lib_entry.to_string_lossy().as_ref(),
        "-o",
        lib_object.to_string_lossy().as_ref(),
    ]);
    assert_success(&lib_output, "kernc library compile");

    fs::write(
        &main_source,
        r#"
impl i32 : dep.Foreign {
    fn ping() i32 {
        return self;
    }
}

fn main() i32 {
    return 0;
}
"#,
    )
    .unwrap();

    let dep_mapping = format!("dep={}", metadata_dir.to_string_lossy());
    let base_mapping = format!("base={}", repo_root().join("library/base").display());
    let app_output = run_kernc([
        "--runtime-entry",
        "crt",
        "--runtime-libc",
        "yes",
        "--module-path",
        base_mapping.as_str(),
        "--module-interface-path",
        dep_mapping.as_str(),
        main_source.to_string_lossy().as_ref(),
        "-o",
        root.join("app.out").to_string_lossy().as_ref(),
    ]);

    assert!(
        !app_output.status.success(),
        "kernc unexpectedly accepted orphan impl:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&app_output.stdout),
        String::from_utf8_lossy(&app_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&app_output.stderr);
    assert!(
        stderr.contains("orphan trait impls are not allowed"),
        "unexpected stderr:\n{}",
        stderr
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn allows_impl_of_imported_trait_for_pointer_to_local_type() {
    let root = unique_temp_path("kernc_orphan_local_pointer", "dir");
    let lib_dir = root.join("lib");
    let metadata_dir = root.join("kmeta");
    let lib_entry = lib_dir.join("mod.kn");
    let lib_object = root.join("iface.o");
    let main_source = root.join("main.kn");
    let executable = root.join(if cfg!(windows) { "app.exe" } else { "app.out" });

    fs::create_dir_all(&lib_dir).unwrap();
    fs::create_dir_all(&metadata_dir).unwrap();

    fs::write(
        &lib_entry,
        r#"
pub trait Foreign {
    fn ping() i32;
};
"#,
    )
    .unwrap();

    let lib_output = run_kernc([
        "-c",
        "--module-root-name",
        "iface",
        "--metadata-output",
        metadata_dir.to_string_lossy().as_ref(),
        lib_entry.to_string_lossy().as_ref(),
        "-o",
        lib_object.to_string_lossy().as_ref(),
    ]);
    assert_success(&lib_output, "kernc library compile");

    fs::write(
        &main_source,
        r#"
struct Local {
    value: i32,
};

impl &Local : dep.Foreign {
    fn ping() i32 {
        return self.value;
    }
}

fn main() i32 {
    let local = Local.{ value: 7 };
    let obj = (local.& as &dep.Foreign);
    return if (obj.ping() == 7) { 0 } else { 1 };
}
"#,
    )
    .unwrap();

    let dep_mapping = format!("dep={}", metadata_dir.to_string_lossy());
    let base_mapping = format!("base={}", repo_root().join("library/base").display());
    let app_output = run_kernc([
        "--runtime-entry",
        "crt",
        "--runtime-libc",
        "yes",
        "--module-path",
        base_mapping.as_str(),
        "--module-interface-path",
        dep_mapping.as_str(),
        main_source.to_string_lossy().as_ref(),
        "-o",
        executable.to_string_lossy().as_ref(),
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
fn rejects_orphan_impl_of_source_tree_trait_for_foreign_primitive() {
    let root = unique_temp_path("kernc_orphan_foreign_source_tree", "dir");
    let dep_dir = root.join("dep");
    let dep_entry = dep_dir.join("mod.kn");
    let main_source = root.join("main.kn");

    fs::create_dir_all(&dep_dir).unwrap();

    fs::write(
        &dep_entry,
        r#"
pub trait Foreign {
    fn ping() i32;
};
"#,
    )
    .unwrap();

    fs::write(
        &main_source,
        r#"
impl i32 : dep.Foreign {
    fn ping() i32 {
        return self;
    }
}

fn main() i32 {
    return 0;
}
"#,
    )
    .unwrap();

    let dep_mapping = format!("dep={}", dep_dir.to_string_lossy());
    let base_mapping = format!("base={}", repo_root().join("library/base").display());
    let app_output = run_kernc([
        "--runtime-entry",
        "crt",
        "--runtime-libc",
        "yes",
        "--module-path",
        base_mapping.as_str(),
        "--module-path",
        dep_mapping.as_str(),
        main_source.to_string_lossy().as_ref(),
        "-o",
        root.join("app.out").to_string_lossy().as_ref(),
    ]);

    assert!(
        !app_output.status.success(),
        "kernc unexpectedly accepted source-tree orphan impl:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&app_output.stdout),
        String::from_utf8_lossy(&app_output.stderr)
    );

    let stderr = String::from_utf8_lossy(&app_output.stderr);
    assert!(
        stderr.contains("orphan trait impls are not allowed"),
        "unexpected stderr:\n{}",
        stderr
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn kmeta_snapshot_keeps_cfg_gated_submodule_sources() {
    let root = unique_temp_path("kernc_kmeta_cfg_submodule", "dir");
    let lib_dir = root.join("lib");
    let inner_dir = lib_dir.join("inner");
    let metadata_dir = root.join("kmeta");
    let lib_entry = lib_dir.join("mod.kn");
    let inner_entry = inner_dir.join("mod.kn");
    let gated_entry = inner_dir.join("entry.kn");
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
        "--module-root-name",
        "pkg",
        "--metadata-output",
        metadata_arg.as_str(),
        lib_entry_arg.as_str(),
        "-o",
        lib_object_arg.as_str(),
    ]);
    assert_success(&output, "kernc kmeta snapshot");

    assert!(
        metadata_dir.join("src/inner/entry.kn").is_file(),
        "cfg-gated submodule source missing from kmeta snapshot"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn concurrent_kmeta_snapshot_writers_serialize_on_output_lock() {
    let root = unique_temp_path("kernc_kmeta_parallel", "dir");
    let lib_dir = root.join("lib");
    let metadata_dir = root.join("kmeta");
    let lib_entry = lib_dir.join("mod.kn");
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
            lib_dir.join(format!("{module_name}.kn")),
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
    let repo_root = repo_root();
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
                    "--module-root-name",
                    "pkg",
                    "--metadata-output",
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
    assert!(metadata_dir.join("src/mod.kn").is_file());

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn rejects_raw_source_tree_passed_via_imported_interface_alias() {
    let root = unique_temp_path("kernc_kmeta_reject_raw", "dir");
    let dep_dir = root.join("dep");
    let dep_entry = dep_dir.join("mod.kn");
    let main_source = root.join("main.kn");
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
        "--module-interface-path",
        dep_mapping.as_str(),
        main_source_arg.as_str(),
        "-o",
        object_arg.as_str(),
    ]);

    assert!(
        !output.status.success(),
        "kernc unexpectedly accepted a raw source tree for --module-interface-path:\nstdout:\n{}\nstderr:\n{}",
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
trait Base {
    fn foo() i32;
};

trait Derived[U]: Base {
    fn add(_: U) i32;
};

impl &i32 : Base {
    pub fn foo() i32 {
        return self.*;
    }
}

impl &i32 : Derived[i32] {
    pub fn add(v: i32) i32 {
        return self.* + v;
    }
}

fn use_it[T](value: &T) i32
    where &T: Derived[i32],
{
    return value.foo() + value.add(2);
}

fn main() i32 {
    let value = 5i32;
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
use base.coll.String;
use base.mem.alloc.gpa;
use std.mem.Page;

fn make_text(alloc: &mut base.mem.alloc.Allocator, text: &[u8]) String!i32 {
    let mut out = String.{};
    if (out..&.try_push_str(alloc, text).is_err()) {
        return .{ Err: 1 };
    }
    return .{ Ok: out };
}

fn main() i32 {
    let page = Page.{}..&;
    let gpa = gpa().on(page)..&;
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

    if (text.& != "kern-lang") {
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
