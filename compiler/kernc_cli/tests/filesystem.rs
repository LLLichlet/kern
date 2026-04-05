mod support;

use std::fs;

use support::{build_and_run, kern_string_literal, unique_temp_path};

fn build_and_run_hosted(source: &str) -> std::process::Output {
    build_and_run(
        "kernc_std_fs",
        source,
        &["--use-std", "--link-profile", "hosted"],
    )
}

#[test]
fn runs_hosted_program_with_fs_create_followed_by_another_result_match() {
    let temp_file = unique_temp_path("kernc_std_fs_create_chain", "txt");
    let temp_path = kern_string_literal(&temp_file);

    let output = build_and_run_hosted(&format!(
        r#"
use std.Result;
use std.fs;
use std.os;
use std.mem.alloc.{{Page, GPA}};

fn ok_bool() Result[bool, os.Error] {{
    return .{{ Ok: true }};
}}

extern fn main(_: [][]u8) i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    let mut writer = match (fs.create(gpa, "{path}")) {{
        .{{ Ok: file }} => file,
        .{{ Err: _ }} => return 1,
    }};
    let ok = match (ok_bool()) {{
        .{{ Ok: value }} => value,
        .{{ Err: _ }} => return 2,
    }};
    if (!ok) {{
        return 3;
    }}
    match (writer..&.close()) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 4,
    }}
    return 0;
}}
"#,
        path = temp_path
    ));

    assert!(
        output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_file(&temp_file);
}

#[test]
fn runs_hosted_program_using_std_fs_convenience_functions() {
    let temp_file = unique_temp_path("kernc_std_fs_convenience", "txt");
    let temp_path = kern_string_literal(&temp_file);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use std.mem.alloc.{{Page, GPA}};

extern fn main(_: [][]u8) i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    let written = match (fs.write_all(gpa, "{path}", "abc123")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 1,
    }};
    if (written != 6) {{
        return 2;
    }}

    let mut text = match (fs.read_to_string(gpa, "{path}")) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 3,
    }};
    defer text..&.deinit(gpa);

    if (!text.&.eq("abc123")) {{
        return 4;
    }}

    match (fs.remove_file(gpa, "{path}")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 5,
    }}

    let missing = fs.open_read(gpa, "{path}");
    if (!missing.is_err()) {{
        return 6;
    }}

    return 0;
}}
"#,
        path = temp_path
    ));

    assert!(
        output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn runs_hosted_program_using_std_fs_metadata_and_directories() {
    let temp_dir = unique_temp_path("kernc_std_fs_dir_meta", "dir");
    let temp_file = temp_dir.join("data.txt");
    let dir_path = kern_string_literal(&temp_dir);
    let file_path = kern_string_literal(&temp_file);

    let _ = fs::remove_file(&temp_file);
    let _ = fs::remove_dir_all(&temp_dir);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use std.mem.alloc.{{Page, GPA}};

extern fn main(_: [][]u8) i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    let dir_exists_before = match (fs.exists(gpa, "{dir_path}")) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 1,
    }};
    if (dir_exists_before) {{
        return 2;
    }}

    match (fs.create_dir(gpa, "{dir_path}")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 3,
    }}

    let dir_exists = match (fs.exists(gpa, "{dir_path}")) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 4,
    }};
    if (!dir_exists) {{
        return 5;
    }}

    let dir_meta = match (fs.metadata(gpa, "{dir_path}")) {{
        .{{ Ok: meta }} => meta,
        .{{ Err: _ }} => return 6,
    }};
    if (!dir_meta.is_dir() or dir_meta.is_file()) {{
        return 7;
    }}

    let dir_is_dir = match (fs.is_dir(gpa, "{dir_path}")) {{
        .{{ Ok: yes }} => yes,
        .{{ Err: _ }} => return 8,
    }};
    if (!dir_is_dir) {{
        return 9;
    }}

    let file_exists_before = match (fs.exists(gpa, "{file_path}")) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 10,
    }};
    if (file_exists_before) {{
        return 11;
    }}

    let written = match (fs.write_all(gpa, "{file_path}", "hello")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 12,
    }};
    if (written != 5) {{
        return 13;
    }}

    let file_meta = match (fs.metadata(gpa, "{file_path}")) {{
        .{{ Ok: meta }} => meta,
        .{{ Err: _ }} => return 14,
    }};
    if (!file_meta.is_file() or file_meta.is_dir()) {{
        return 15;
    }}
    if (file_meta.size != 5) {{
        return 16;
    }}

    let file_is_file = match (fs.is_file(gpa, "{file_path}")) {{
        .{{ Ok: yes }} => yes,
        .{{ Err: _ }} => return 17,
    }};
    if (!file_is_file) {{
        return 18;
    }}

    let file_is_dir = match (fs.is_dir(gpa, "{file_path}")) {{
        .{{ Ok: yes }} => yes,
        .{{ Err: _ }} => return 19,
    }};
    if (file_is_dir) {{
        return 20;
    }}

    match (fs.remove_file(gpa, "{file_path}")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 21,
    }}

    let file_exists_after = match (fs.exists(gpa, "{file_path}")) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 22,
    }};
    if (file_exists_after) {{
        return 23;
    }}

    match (fs.remove_dir(gpa, "{dir_path}")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 24,
    }}

    let dir_exists_after = match (fs.exists(gpa, "{dir_path}")) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 25,
    }};
    if (dir_exists_after) {{
        return 26;
    }}

    return 0;
}}
"#,
        dir_path = dir_path,
        file_path = file_path
    ));

    assert!(
        output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_file(&temp_file);
    let _ = fs::remove_dir(&temp_dir);
}

#[test]
fn runs_hosted_program_using_std_fs_roundtrip() {
    let temp_file = unique_temp_path("kernc_std_fs_roundtrip", "txt");
    let temp_path = kern_string_literal(&temp_file);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use std.mem.alloc.{{Page, GPA}};

extern fn main(_: [][]u8) i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    let mut writer = match (fs.create(gpa, "{path}")) {{
        .{{ Ok: file }} => file,
        .{{ Err: _ }} => return 1,
    }};
    let written = match (writer..&.write_all("kern-fs")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 2,
    }};
    if (written != 7) {{
        return 3;
    }}
    match (writer..&.close()) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 4,
    }}

    let mut reader = match (fs.open_read(gpa, "{path}")) {{
        .{{ Ok: file }} => file,
        .{{ Err: _ }} => return 5,
    }};
    let mut text = match (reader..&.read_to_string(gpa)) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 6,
    }};
    defer text..&.deinit(gpa);

    if (!text.&.eq("kern-fs")) {{
        return 7;
    }}
    match (reader..&.close()) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 8,
    }}

    let missing = fs.open_read(gpa, "{path}.missing");
    if (!missing.is_err()) {{
        return 9;
    }}

    return 0;
}}
"#,
        path = temp_path
    ));

    assert!(
        output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_file(&temp_file);
}

#[test]
fn runs_hosted_program_using_std_fs_open_variants() {
    let temp_file = unique_temp_path("kernc_std_fs_open_variants", "txt");
    let temp_path = kern_string_literal(&temp_file);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use std.mem.alloc.{{Page, GPA}};

extern fn main(_: [][]u8) i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    let mut created = match (fs.create_new(gpa, "{path}")) {{
        .{{ Ok: file }} => file,
        .{{ Err: _ }} => return 1,
    }};
    match (created..&.write_all("ab")) {{
        .{{ Ok: count }} => {{
            if (count != 2) {{
                return 2;
            }}
        }},
        .{{ Err: _ }} => return 3,
    }}
    created..&.deinit();

    let created_again = fs.create_new(gpa, "{path}");
    if (!created_again.is_err()) {{
        return 4;
    }}

    let mut appended = match (fs.open_append(gpa, "{path}")) {{
        .{{ Ok: file }} => file,
        .{{ Err: _ }} => return 5,
    }};
    match (appended..&.write_all("cd")) {{
        .{{ Ok: count }} => {{
            if (count != 2) {{
                return 6;
            }}
        }},
        .{{ Err: _ }} => return 7,
    }}
    appended..&.deinit();

    let mut writer = match (fs.open_write(gpa, "{path}")) {{
        .{{ Ok: file }} => file,
        .{{ Err: _ }} => return 8,
    }};
    match (writer..&.write("Z")) {{
        .{{ Ok: count }} => {{
            if (count != 1) {{
                return 9;
            }}
        }},
        .{{ Err: _ }} => return 10,
    }}
    writer..&.deinit();

    let mut text = match (fs.read_to_string(gpa, "{path}")) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 11,
    }};
    defer text..&.deinit(gpa);

    if (!text.&.eq("Zbcd")) {{
        return 12;
    }}

    return 0;
}}
"#,
        path = temp_path
    ));

    assert!(
        output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_file(&temp_file);
}

#[test]
fn runs_hosted_program_using_std_fs_create_dir_all() {
    let temp_root = unique_temp_path("kernc_std_fs_dir_all", "dir");
    let nested_dir = temp_root.join("a").join("b").join("c");
    let nested_file = nested_dir.join("note.txt");
    let root_path = kern_string_literal(&temp_root);
    let dir_path = kern_string_literal(&nested_dir);
    let file_path = kern_string_literal(&nested_file);

    let _ = fs::remove_file(&nested_file);
    let _ = fs::remove_dir_all(&temp_root);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use std.mem.alloc.{{Page, GPA}};

extern fn main(_: [][]u8) i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    match (fs.create_dir_all(gpa, "{dir_path}")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 1,
    }}

    let root_is_dir = match (fs.is_dir(gpa, "{root_path}")) {{
        .{{ Ok: yes }} => yes,
        .{{ Err: _ }} => return 2,
    }};
    if (!root_is_dir) {{
        return 3;
    }}

    let nested_is_dir = match (fs.is_dir(gpa, "{dir_path}")) {{
        .{{ Ok: yes }} => yes,
        .{{ Err: _ }} => return 4,
    }};
    if (!nested_is_dir) {{
        return 5;
    }}

    match (fs.create_dir_all(gpa, "{dir_path}")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 6,
    }}

    let written = match (fs.write_all(gpa, "{file_path}", "nested")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 7,
    }};
    if (written != 6) {{
        return 8;
    }}

    let mut text = match (fs.read_to_string(gpa, "{file_path}")) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 9,
    }};
    defer text..&.deinit(gpa);

    if (!text.&.eq("nested")) {{
        return 10;
    }}

    return 0;
}}
"#,
        root_path = root_path,
        dir_path = dir_path,
        file_path = file_path
    ));

    assert!(
        output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_file(&nested_file);
    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn runs_hosted_program_using_std_fs_rename() {
    let temp_root = unique_temp_path("kernc_std_fs_rename", "dir");
    let old_dir = temp_root.join("old_dir");
    let new_dir = temp_root.join("new_dir");
    let old_file = old_dir.join("before.txt");
    let renamed_file = old_dir.join("after.txt");
    let new_file = new_dir.join("after.txt");
    let old_dir_path = kern_string_literal(&old_dir);
    let new_dir_path = kern_string_literal(&new_dir);
    let old_file_path = kern_string_literal(&old_file);
    let renamed_file_path = kern_string_literal(&renamed_file);
    let new_file_path = kern_string_literal(&new_file);

    let _ = fs::remove_file(&new_file);
    let _ = fs::remove_file(&renamed_file);
    let _ = fs::remove_file(&old_file);
    let _ = fs::remove_dir_all(&temp_root);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use std.mem.alloc.{{Page, GPA}};

extern fn main(_: [][]u8) i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    match (fs.create_dir_all(gpa, "{old_dir_path}")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 1,
    }}

    match (fs.write_all(gpa, "{old_file_path}", "rename-me")) {{
        .{{ Ok: count }} => {{
            if (count != 9) {{
                return 2;
            }}
        }},
        .{{ Err: _ }} => return 3,
    }}

    match (fs.rename(gpa, "{old_file_path}", "{renamed_file_path}")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 4,
    }}

    let old_file_exists = match (fs.exists(gpa, "{old_file_path}")) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 5,
    }};
    if (old_file_exists) {{
        return 6;
    }}

    let mut text = match (fs.read_to_string(gpa, "{renamed_file_path}")) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 7,
    }};
    defer text..&.deinit(gpa);
    if (!text.&.eq("rename-me")) {{
        return 8;
    }}

    match (fs.rename(gpa, "{old_dir_path}", "{new_dir_path}")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 9,
    }}

    let old_dir_exists = match (fs.exists(gpa, "{old_dir_path}")) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 10,
    }};
    if (old_dir_exists) {{
        return 11;
    }}

    let new_dir_is_dir = match (fs.is_dir(gpa, "{new_dir_path}")) {{
        .{{ Ok: yes }} => yes,
        .{{ Err: _ }} => return 12,
    }};
    if (!new_dir_is_dir) {{
        return 13;
    }}

    let new_file_exists = match (fs.exists(gpa, "{new_file_path}")) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 14,
    }};
    if (!new_file_exists) {{
        return 15;
    }}

    return 0;
}}
"#,
        old_dir_path = old_dir_path,
        new_dir_path = new_dir_path,
        old_file_path = old_file_path,
        renamed_file_path = renamed_file_path,
        new_file_path = new_file_path
    ));

    assert!(
        output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn runs_hosted_program_using_std_fs_read_dir() {
    let temp_root = unique_temp_path("kernc_std_fs_read_dir", "dir");
    let alpha_dir = temp_root.join("alpha");
    let file_a = temp_root.join("a.txt");
    let file_b = temp_root.join("b.txt");
    let root_path = kern_string_literal(&temp_root);
    let alpha_path = kern_string_literal(&alpha_dir);
    let file_a_path = kern_string_literal(&file_a);
    let file_b_path = kern_string_literal(&file_b);

    let _ = fs::remove_file(&file_a);
    let _ = fs::remove_file(&file_b);
    let _ = fs::remove_dir_all(&temp_root);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use std.mem.alloc.{{Page, GPA}};

extern fn main(_: [][]u8) i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    match (fs.create_dir_all(gpa, "{alpha_path}")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 1,
    }}
    match (fs.write_all(gpa, "{file_a_path}", "A")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 2,
    }}
    match (fs.write_all(gpa, "{file_b_path}", "B")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 3,
    }}

    let mut total = usize.{{0}};
    let mut saw_alpha_dir = bool.{{false}};
    let mut saw_a_file = bool.{{false}};
    let mut saw_b_file = bool.{{false}};

    let visited = match (fs.read_dir(gpa, "{root_path}", .[
        total = total..&,
        saw_alpha_dir = saw_alpha_dir..&,
        saw_a_file = saw_a_file..&,
        saw_b_file = saw_b_file..&
    ](entry: fs.DirEntry) bool {{
        total.* += 1;
        if (entry.name.eq("alpha")) {{
            if (!entry.is_dir()) {{
                return false;
            }}
            saw_alpha_dir.* = true;
        }}
        if (entry.name.eq("a.txt")) {{
            if (!entry.is_file()) {{
                return false;
            }}
            saw_a_file.* = true;
        }}
        if (entry.name.eq("b.txt")) {{
            if (!entry.is_file()) {{
                return false;
            }}
            saw_b_file.* = true;
        }}
        return true;
    }})) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 4,
    }};

    if (visited != 3 or total != 3) {{
        return 5;
    }}
    if (!saw_alpha_dir or !saw_a_file or !saw_b_file) {{
        return 6;
    }}

    let mut early = usize.{{0}};
    let stopped = match (fs.read_dir(gpa, "{root_path}", .[
        early = early..&
    ](_: fs.DirEntry) bool {{
        early.* += 1;
        return false;
    }})) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 7,
    }};

    if (stopped != 1 or early != 1) {{
        return 8;
    }}

    return 0;
}}
"#,
        root_path = root_path,
        alpha_path = alpha_path,
        file_a_path = file_a_path,
        file_b_path = file_b_path
    ));

    assert!(
        output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn runs_hosted_program_using_std_fs_remove_dir_all() {
    let temp_root = unique_temp_path("kernc_std_fs_remove_dir_all", "dir");
    let nested_dir = temp_root.join("one").join("two");
    let nested_file = nested_dir.join("deep.txt");
    let sibling_file = temp_root.join("root.txt");
    let root_path = kern_string_literal(&temp_root);
    let nested_dir_path = kern_string_literal(&nested_dir);
    let nested_file_path = kern_string_literal(&nested_file);
    let sibling_file_path = kern_string_literal(&sibling_file);

    let _ = fs::remove_file(&nested_file);
    let _ = fs::remove_file(&sibling_file);
    let _ = fs::remove_dir_all(&temp_root);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use std.mem.alloc.{{Page, GPA}};

extern fn main(_: [][]u8) i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    match (fs.create_dir_all(gpa, "{nested_dir_path}")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 1,
    }}
    match (fs.write_all(gpa, "{nested_file_path}", "deep")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 2,
    }}
    match (fs.write_all(gpa, "{sibling_file_path}", "root")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 3,
    }}

    match (fs.remove_dir_all(gpa, "{root_path}")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 4,
    }}

    let root_exists = match (fs.exists(gpa, "{root_path}")) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 5,
    }};
    if (root_exists) {{
        return 6;
    }}

    let nested_exists = match (fs.exists(gpa, "{nested_file_path}")) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 7,
    }};
    if (nested_exists) {{
        return 8;
    }}

    return 0;
}}
"#,
        root_path = root_path,
        nested_dir_path = nested_dir_path,
        nested_file_path = nested_file_path,
        sibling_file_path = sibling_file_path
    ));

    assert!(
        output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn runs_hosted_program_using_std_fs_path_views() {
    let output = build_and_run_hosted(
        r#"
use std.fs;

extern fn main(_: [][]u8) i32 {
    let path = "/tmp/kern/archive.tar";

    if (!fs.file_name(path).is_some_and(.[](name: []u8) bool {
        return name.eq("archive.tar");
    })) {
        return 1;
    }
    if (!fs.parent(path).is_some_and(.[](dir: []u8) bool {
        return dir.eq("/tmp/kern");
    })) {
        return 2;
    }
    if (!fs.extension(path).is_some_and(.[](ext: []u8) bool {
        return ext.eq("tar");
    })) {
        return 3;
    }
    if (!fs.file_stem(path).is_some_and(.[](stem: []u8) bool {
        return stem.eq("archive");
    })) {
        return 4;
    }

    if (!fs.parent("/tmp/kern/").is_some_and(.[](dir: []u8) bool {
        return dir.eq("/tmp");
    })) {
        return 5;
    }
    if (!fs.parent("/tmp").is_some_and(.[](dir: []u8) bool {
        return dir.eq("/");
    })) {
        return 6;
    }
    if (fs.parent("/").is_some()) {
        return 7;
    }
    if (fs.file_name("/").is_some()) {
        return 8;
    }
    if (fs.parent("plain.txt").is_some()) {
        return 9;
    }

    if (!fs.file_stem(".gitignore").is_some_and(.[](stem: []u8) bool {
        return stem.eq(".gitignore");
    })) {
        return 10;
    }
    if (fs.extension(".gitignore").is_some()) {
        return 11;
    }
    if (!fs.file_stem("config.").is_some_and(.[](stem: []u8) bool {
        return stem.eq("config");
    })) {
        return 12;
    }
    if (fs.extension("config.").is_some()) {
        return 13;
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
fn runs_hosted_program_using_std_fs_path_join_and_normalize() {
    let output = build_and_run_hosted(
        r#"
use std.fs;
use std.mem.alloc.{Page, GPA};

extern fn main(_: [][]u8) i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;

    let mut joined = match (fs.join(gpa, "/tmp/kern", "src/main.rn")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 1,
    };
    defer joined..&.deinit(gpa);
    if (!joined.&.eq("/tmp/kern/src/main.rn")) {
        return 2;
    }

    let mut bare = match (fs.join(gpa, "", "note.txt")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 3,
    };
    defer bare..&.deinit(gpa);
    if (!bare.&.eq("note.txt")) {
        return 4;
    }

    let mut rooted = match (fs.join(gpa, "/tmp/kern", "/etc/passwd")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 5,
    };
    defer rooted..&.deinit(gpa);
    if (!rooted.&.eq("/etc/passwd")) {
        return 6;
    }

    let mut normalized = match (fs.normalize(gpa, "/tmp/./kern//src/../out/file.txt")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 7,
    };
    defer normalized..&.deinit(gpa);
    if (!normalized.&.eq("/tmp/kern/out/file.txt")) {
        return 8;
    }

    let mut relative = match (fs.normalize(gpa, "alpha/./beta/../gamma")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 9,
    };
    defer relative..&.deinit(gpa);
    if (!relative.&.eq("alpha/gamma")) {
        return 10;
    }

    let mut escaped = match (fs.normalize(gpa, "../../alpha/../beta")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 11,
    };
    defer escaped..&.deinit(gpa);
    if (!escaped.&.eq("../../beta")) {
        return 12;
    }

    let mut root = match (fs.normalize(gpa, "/alpha/../..")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 13,
    };
    defer root..&.deinit(gpa);
    if (!root.&.eq("/")) {
        return 14;
    }

    let mut empty = match (fs.normalize(gpa, "")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 15,
    };
    defer empty..&.deinit(gpa);
    if (!empty.&.eq(".")) {
        return 16;
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
fn runs_hosted_program_using_std_fs_path_replacements() {
    let output = build_and_run_hosted(
        r#"
use std.fs;
use std.mem.alloc.{Page, GPA};

extern fn main(_: [][]u8) i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;

    let mut renamed = match (fs.with_file_name(gpa, "/tmp/kern/main.rn", "lib.rn")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 1,
    };
    defer renamed..&.deinit(gpa);
    if (!renamed.&.eq("/tmp/kern/lib.rn")) {
        return 2;
    }

    let mut reext = match (fs.with_extension(gpa, "/tmp/kern/main.rn", "ll")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 3,
    };
    defer reext..&.deinit(gpa);
    if (!reext.&.eq("/tmp/kern/main.ll")) {
        return 4;
    }

    let mut stripped = match (fs.with_extension(gpa, "archive.tar", "")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 5,
    };
    defer stripped..&.deinit(gpa);
    if (!stripped.&.eq("archive")) {
        return 6;
    }

    let mut hidden = match (fs.with_extension(gpa, ".gitignore", "txt")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 7,
    };
    defer hidden..&.deinit(gpa);
    if (!hidden.&.eq(".gitignore.txt")) {
        return 8;
    }

    let mut rooted = match (fs.with_file_name(gpa, "/", "boot")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 9,
    };
    defer rooted..&.deinit(gpa);
    if (!rooted.&.eq("/boot")) {
        return 10;
    }

    let bad = fs.with_extension(gpa, "/", "txt");
    if (!bad.is_err()) {
        return 11;
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
fn runs_hosted_program_using_std_fs_windows_path_semantics() {
    if !cfg!(windows) {
        return;
    }

    let output = build_and_run_hosted(
        r#"
use std.fs;
use std.mem.alloc.{Page, GPA};

extern fn main(_: [][]u8) i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;

    if (!fs.parent("C:\\kern\\src\\main.rn").is_some_and(.[](dir: []u8) bool {
        return dir.eq("C:\\kern\\src");
    })) {
        return 1;
    }
    if (fs.parent("C:\\").is_some()) {
        return 2;
    }
    if (!fs.file_name("C:\\kern\\main.rn").is_some_and(.[](name: []u8) bool {
        return name.eq("main.rn");
    })) {
        return 3;
    }

    let mut joined = match (fs.join(gpa, "C:\\kern", "src\\main.rn")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 4,
    };
    defer joined..&.deinit(gpa);
    if (!joined.&.eq("C:\\kern\\src\\main.rn")) {
        return 5;
    }

    let mut forward = match (fs.join(gpa, "C:/kern", "src/main.rn")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 6,
    };
    defer forward..&.deinit(gpa);
    if (!forward.&.eq("C:/kern/src/main.rn")) {
        return 7;
    }

    let mut rooted = match (fs.join(gpa, "C:\\kern", "D:\\other\\out.rn")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 8,
    };
    defer rooted..&.deinit(gpa);
    if (!rooted.&.eq("D:\\other\\out.rn")) {
        return 9;
    }

    let mut normalized = match (fs.normalize(gpa, "C:\\kern\\.\\src\\\\..\\out\\file.txt")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 10,
    };
    defer normalized..&.deinit(gpa);
    if (!normalized.&.eq("C:\\kern\\out\\file.txt")) {
        return 11;
    }

    let mut forward_normalized = match (fs.normalize(gpa, "C:/kern/./src//../out/file.txt")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 12,
    };
    defer forward_normalized..&.deinit(gpa);
    if (!forward_normalized.&.eq("C:/kern/out/file.txt")) {
        return 13;
    }

    let mut unc_joined = match (fs.join(gpa, "\\\\server\\share", "dir\\main.rn")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 14,
    };
    defer unc_joined..&.deinit(gpa);
    if (!unc_joined.&.eq("\\\\server\\share\\dir\\main.rn")) {
        return 15;
    }

    let mut unc = match (fs.normalize(gpa, "\\\\server\\share\\src\\\\..\\out\\file.txt")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 16,
    };
    defer unc..&.deinit(gpa);
    if (!unc.&.eq("\\\\server\\share\\out\\file.txt")) {
        return 17;
    }

    if (!fs.parent("\\\\server\\share\\out\\file.txt").is_some_and(.[](dir: []u8) bool {
        return dir.eq("\\\\server\\share\\out");
    })) {
        return 18;
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
fn runs_hosted_program_using_std_fs_unicode_paths_on_windows() {
    if !cfg!(windows) {
        return;
    }

    let temp_root = std::env::temp_dir().join("kern-\u{6D4B}\u{8BD5}-\u{6587}\u{4EF6}\u{5939}");
    let temp_file = temp_root.join("\u{4F60}\u{597D}-emoji-\u{1F642}.txt");
    let root_path = kern_string_literal(&temp_root);
    let file_path = kern_string_literal(&temp_file);

    let _ = fs::remove_file(&temp_file);
    let _ = fs::remove_dir_all(&temp_root);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use std.mem.alloc.{{Page, GPA}};

extern fn main(_: [][]u8) i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    match (fs.create_dir_all(gpa, "{root_path}")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 1,
    }}

    match (fs.write_all(gpa, "{file_path}", "unicode-ok")) {{
        .{{ Ok: count }} => {{
            if (count != 10) {{
                return 2;
            }}
        }},
        .{{ Err: _ }} => return 3,
    }}

    let mut text = match (fs.read_to_string(gpa, "{file_path}")) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 4,
    }};
    defer text..&.deinit(gpa);
    if (!text.&.eq("unicode-ok")) {{
        return 5;
    }}

    let mut hits = usize.{{0}};
    let visited = match (fs.read_dir(gpa, "{root_path}", .[
        hits = hits..&
    ](entry: fs.DirEntry) bool {{
        if (entry.name.eq("你好-emoji-🙂.txt")) {{
            hits.* += 1;
        }}
        return true;
    }})) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 6,
    }};

    if (visited != 1 or hits != 1) {{
        return 7;
    }}

    return 0;
}}
"#,
        root_path = root_path,
        file_path = file_path
    ));

    assert!(
        output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_file(&temp_file);
    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn runs_hosted_program_using_std_fs_copy_and_append() {
    let temp_root = unique_temp_path("kernc_std_fs_copy_append", "dir");
    let from_file = temp_root.join("from.txt");
    let to_file = temp_root.join("to.txt");
    let from_path = kern_string_literal(&from_file);
    let to_path = kern_string_literal(&to_file);

    let _ = fs::remove_file(&from_file);
    let _ = fs::remove_file(&to_file);
    let _ = fs::remove_dir_all(&temp_root);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use std.mem.alloc.{{Page, GPA}};

extern fn main(_: [][]u8) i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    match (fs.create_dir_all(gpa, "{root_path}")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 1,
    }}

    let written = match (fs.write_all(gpa, "{from_path}", "kern")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 2,
    }};
    if (written != 4) {{
        return 3;
    }}

    let copied = match (fs.copy(gpa, "{from_path}", "{to_path}")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 4,
    }};
    if (copied != 4) {{
        return 5;
    }}

    let appended = match (fs.append_all(gpa, "{to_path}", "-lang")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 6,
    }};
    if (appended != 5) {{
        return 7;
    }}

    let mut text = match (fs.read_to_string(gpa, "{to_path}")) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 8,
    }};
    defer text..&.deinit(gpa);
    if (!text.&.eq("kern-lang")) {{
        return 9;
    }}

    let mut src = match (fs.open_read(gpa, "{from_path}")) {{
        .{{ Ok: file }} => file,
        .{{ Err: _ }} => return 10,
    }};
    defer src..&.deinit();

    let mut dst = match (fs.create(gpa, "{to_path}")) {{
        .{{ Ok: file }} => file,
        .{{ Err: _ }} => return 11,
    }};
    defer dst..&.deinit();

    let roundtrip = match (src..&.copy_to(dst..&)) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 12,
    }};
    if (roundtrip != 4) {{
        return 13;
    }}

    let mut text2 = match (fs.read_to_string(gpa, "{to_path}")) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 14,
    }};
    defer text2..&.deinit(gpa);
    if (!text2.&.eq("kern")) {{
        return 15;
    }}

    return 0;
}}
"#,
        root_path = kern_string_literal(&temp_root),
        from_path = from_path,
        to_path = to_path
    ));

    assert!(
        output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_file(&from_file);
    let _ = fs::remove_file(&to_file);
    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn runs_hosted_program_using_std_fs_walk_dir() {
    let temp_root = unique_temp_path("kernc_std_fs_walk_dir", "dir");
    let alpha_dir = temp_root.join("alpha");
    let beta_dir = alpha_dir.join("beta");
    let root_file = temp_root.join("root.txt");
    let beta_file = beta_dir.join("deep.txt");
    let root_path = kern_string_literal(&temp_root);
    let alpha_path = kern_string_literal(&alpha_dir);
    let beta_path = kern_string_literal(&beta_dir);
    let root_file_path = kern_string_literal(&root_file);
    let beta_file_path = kern_string_literal(&beta_file);

    let _ = fs::remove_file(&beta_file);
    let _ = fs::remove_file(&root_file);
    let _ = fs::remove_dir_all(&temp_root);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use std.mem.alloc.{{Page, GPA}};

extern fn main(_: [][]u8) i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    match (fs.create_dir_all(gpa, "{beta_path}")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 1,
    }}
    match (fs.write_all(gpa, "{root_file_path}", "root")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 2,
    }}
    match (fs.write_all(gpa, "{beta_file_path}", "deep")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 3,
    }}

    let mut saw_alpha = bool.{{false}};
    let mut saw_beta = bool.{{false}};
    let mut saw_root_file = bool.{{false}};
    let mut saw_beta_file = bool.{{false}};

    let walked = match (fs.walk_dir(gpa, "{root_path}", .[
        saw_alpha = saw_alpha..&,
        saw_beta = saw_beta..&,
        saw_root_file = saw_root_file..&,
        saw_beta_file = saw_beta_file..&
    ](path: []u8, entry: fs.DirEntry, depth: usize) bool {{
        if (path.eq("{alpha_path}")) {{
            if (!entry.is_dir() or depth != 1) {{
                return false;
            }}
            saw_alpha.* = true;
        }}
        if (path.eq("{root_file_path}")) {{
            if (!entry.is_file() or depth != 1) {{
                return false;
            }}
            saw_root_file.* = true;
        }}
        if (path.eq("{beta_path}")) {{
            if (!entry.is_dir() or depth != 2) {{
                return false;
            }}
            saw_beta.* = true;
        }}
        if (path.eq("{beta_file_path}")) {{
            if (!entry.is_file() or depth != 3) {{
                return false;
            }}
            saw_beta_file.* = true;
        }}
        return true;
    }})) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 4,
    }};

    if (walked != 4) {{
        return 5;
    }}
    if (!saw_alpha or !saw_beta or !saw_root_file or !saw_beta_file) {{
        return 6;
    }}

    let mut file_hits = usize.{{0}};
    let walked_files = match (fs.walk_files(gpa, "{root_path}", .[
        file_hits = file_hits..&
    ](_: []u8, entry: fs.DirEntry, _: usize) bool {{
        if (!entry.is_file()) {{
            return false;
        }}
        file_hits.* += 1;
        return true;
    }})) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 7,
    }};

    if (walked_files != 4 or file_hits != 2) {{
        return 8;
    }}

    let mut dir_hits = usize.{{0}};
    let walked_dirs = match (fs.walk_dirs(gpa, "{root_path}", .[
        dir_hits = dir_hits..&
    ](_: []u8, entry: fs.DirEntry, _: usize) bool {{
        if (!entry.is_dir()) {{
            return false;
        }}
        dir_hits.* += 1;
        return true;
    }})) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 9,
    }};

    if (walked_dirs != 4 or dir_hits != 2) {{
        return 10;
    }}

    let mut early = usize.{{0}};
    let stopped = match (fs.walk_dir(gpa, "{root_path}", .[
        early = early..&
    ](_: []u8, _: fs.DirEntry, _: usize) bool {{
        early.* += 1;
        return false;
    }})) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 11,
    }};

    if (stopped != 1 or early != 1) {{
        return 12;
    }}

    return 0;
}}
"#,
        root_path = root_path,
        alpha_path = alpha_path,
        beta_path = beta_path,
        root_file_path = root_file_path,
        beta_file_path = beta_file_path
    ));

    assert!(
        output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_file(&beta_file);
    let _ = fs::remove_file(&root_file);
    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn runs_hosted_program_using_std_fs_if_exists_helpers() {
    let temp_root = unique_temp_path("kernc_std_fs_if_exists", "dir");
    let file_path = temp_root.join("data.txt");
    let root_path = kern_string_literal(&temp_root);
    let file_path_str = kern_string_literal(&file_path);

    let _ = fs::remove_file(&file_path);
    let _ = fs::remove_dir_all(&temp_root);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use std.mem.alloc.{{Page, GPA}};

extern fn main(_: [][]u8) i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    let missing_dir = match (fs.remove_dir_if_exists(gpa, "{root_path}")) {{
        .{{ Ok: removed }} => removed,
        .{{ Err: _ }} => return 1,
    }};
    if (missing_dir) {{
        return 2;
    }}

    let created = match (fs.create_dir_if_missing(gpa, "{root_path}")) {{
        .{{ Ok: created }} => created,
        .{{ Err: _ }} => return 3,
    }};
    if (!created) {{
        return 4;
    }}

    let created_again = match (fs.create_dir_if_missing(gpa, "{root_path}")) {{
        .{{ Ok: created }} => created,
        .{{ Err: _ }} => return 5,
    }};
    if (created_again) {{
        return 6;
    }}

    match (fs.write_all(gpa, "{file_path}", "payload")) {{
        .{{ Ok: count }} => {{
            if (count != 7) {{
                return 7;
            }}
        }},
        .{{ Err: _ }} => return 8,
    }}

    let removed_file = match (fs.remove_file_if_exists(gpa, "{file_path}")) {{
        .{{ Ok: removed }} => removed,
        .{{ Err: _ }} => return 9,
    }};
    if (!removed_file) {{
        return 10;
    }}

    let removed_file_again = match (fs.remove_file_if_exists(gpa, "{file_path}")) {{
        .{{ Ok: removed }} => removed,
        .{{ Err: _ }} => return 11,
    }};
    if (removed_file_again) {{
        return 12;
    }}

    let removed_dir = match (fs.remove_dir_if_exists(gpa, "{root_path}")) {{
        .{{ Ok: removed }} => removed,
        .{{ Err: _ }} => return 13,
    }};
    if (!removed_dir) {{
        return 14;
    }}

    let removed_dir_again = match (fs.remove_dir_if_exists(gpa, "{root_path}")) {{
        .{{ Ok: removed }} => removed,
        .{{ Err: _ }} => return 15,
    }};
    if (removed_dir_again) {{
        return 16;
    }}

    return 0;
}}
"#,
        root_path = root_path,
        file_path = file_path_str
    ));

    assert!(
        output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_file(&file_path);
    let _ = fs::remove_dir_all(&temp_root);
}
