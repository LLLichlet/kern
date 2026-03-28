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

fn build_and_run_hosted(source: &str) -> std::process::Output {
    let source_path = unique_temp_path("kernc_std_fs", "kr");
    let exe_ext = if cfg!(windows) { "exe" } else { "out" };
    let executable_path = unique_temp_path("kernc_std_fs", exe_ext);

    fs::write(&source_path, source).unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let exe_arg = executable_path.to_string_lossy().into_owned();
    let args = vec![
        "--use-std",
        "--link-profile",
        "hosted",
        source_arg.as_str(),
        "-o",
        exe_arg.as_str(),
    ];
    let output = run_kernc(&args);

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let run_output = Command::new(&executable_path).output().unwrap();

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
    run_output
}

fn kern_string_literal(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\")
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
use std.mem.alloc.{{PageAllocator, GPAllocator}};

fn ok_bool() Result[bool, os.Error] {{
    return .{{ Ok: true }};
}}

extern fn main() i32 {{
    let page = PageAllocator.{{}}..&;
    let gpa = GPAllocator.{{ backing: page }}..&;

    let mut writer = match (fs.create(gpa, "{path}")) {{
        .Ok: file => file,
        .Err: _ => return 1,
    }};
    let ok = match (ok_bool()) {{
        .Ok: value => value,
        .Err: _ => return 2,
    }};
    if (!ok) {{
        return 3;
    }}
    match (writer..&.close()) {{
        .Ok: _ => {{}},
        .Err: _ => return 4,
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
use std.mem.alloc.{{PageAllocator, GPAllocator}};

extern fn main() i32 {{
    let page = PageAllocator.{{}}..&;
    let gpa = GPAllocator.{{ backing: page }}..&;

    let written = match (fs.write_all(gpa, "{path}", "abc123")) {{
        .Ok: count => count,
        .Err: _ => return 1,
    }};
    if (written != 6) {{
        return 2;
    }}

    let mut text = match (fs.read_to_string(gpa, "{path}")) {{
        .Ok: text => text,
        .Err: _ => return 3,
    }};
    defer text..&.deinit(gpa);

    if (!text.&.eq("abc123")) {{
        return 4;
    }}

    match (fs.remove_file(gpa, "{path}")) {{
        .Ok: _ => {{}},
        .Err: _ => return 5,
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
use std.mem.alloc.{{PageAllocator, GPAllocator}};

extern fn main() i32 {{
    let page = PageAllocator.{{}}..&;
    let gpa = GPAllocator.{{ backing: page }}..&;

    let dir_exists_before = match (fs.exists(gpa, "{dir_path}")) {{
        .Ok: exists => exists,
        .Err: _ => return 1,
    }};
    if (dir_exists_before) {{
        return 2;
    }}

    match (fs.create_dir(gpa, "{dir_path}")) {{
        .Ok: _ => {{}},
        .Err: _ => return 3,
    }}

    let dir_exists = match (fs.exists(gpa, "{dir_path}")) {{
        .Ok: exists => exists,
        .Err: _ => return 4,
    }};
    if (!dir_exists) {{
        return 5;
    }}

    let dir_meta = match (fs.metadata(gpa, "{dir_path}")) {{
        .Ok: meta => meta,
        .Err: _ => return 6,
    }};
    if (!dir_meta.is_dir() or dir_meta.is_file()) {{
        return 7;
    }}

    let dir_is_dir = match (fs.is_dir(gpa, "{dir_path}")) {{
        .Ok: yes => yes,
        .Err: _ => return 8,
    }};
    if (!dir_is_dir) {{
        return 9;
    }}

    let file_exists_before = match (fs.exists(gpa, "{file_path}")) {{
        .Ok: exists => exists,
        .Err: _ => return 10,
    }};
    if (file_exists_before) {{
        return 11;
    }}

    let written = match (fs.write_all(gpa, "{file_path}", "hello")) {{
        .Ok: count => count,
        .Err: _ => return 12,
    }};
    if (written != 5) {{
        return 13;
    }}

    let file_meta = match (fs.metadata(gpa, "{file_path}")) {{
        .Ok: meta => meta,
        .Err: _ => return 14,
    }};
    if (!file_meta.is_file() or file_meta.is_dir()) {{
        return 15;
    }}
    if (file_meta.size != 5) {{
        return 16;
    }}

    let file_is_file = match (fs.is_file(gpa, "{file_path}")) {{
        .Ok: yes => yes,
        .Err: _ => return 17,
    }};
    if (!file_is_file) {{
        return 18;
    }}

    let file_is_dir = match (fs.is_dir(gpa, "{file_path}")) {{
        .Ok: yes => yes,
        .Err: _ => return 19,
    }};
    if (file_is_dir) {{
        return 20;
    }}

    match (fs.remove_file(gpa, "{file_path}")) {{
        .Ok: _ => {{}},
        .Err: _ => return 21,
    }}

    let file_exists_after = match (fs.exists(gpa, "{file_path}")) {{
        .Ok: exists => exists,
        .Err: _ => return 22,
    }};
    if (file_exists_after) {{
        return 23;
    }}

    match (fs.remove_dir(gpa, "{dir_path}")) {{
        .Ok: _ => {{}},
        .Err: _ => return 24,
    }}

    let dir_exists_after = match (fs.exists(gpa, "{dir_path}")) {{
        .Ok: exists => exists,
        .Err: _ => return 25,
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
use std.mem.alloc.{{PageAllocator, GPAllocator}};

extern fn main() i32 {{
    let page = PageAllocator.{{}}..&;
    let gpa = GPAllocator.{{ backing: page }}..&;

    let mut writer = match (fs.create(gpa, "{path}")) {{
        .Ok: file => file,
        .Err: _ => return 1,
    }};
    let written = match (writer..&.write_all("kern-fs")) {{
        .Ok: count => count,
        .Err: _ => return 2,
    }};
    if (written != 7) {{
        return 3;
    }}
    match (writer..&.close()) {{
        .Ok: _ => {{}},
        .Err: _ => return 4,
    }}

    let mut reader = match (fs.open_read(gpa, "{path}")) {{
        .Ok: file => file,
        .Err: _ => return 5,
    }};
    let mut text = match (reader..&.read_to_string(gpa)) {{
        .Ok: text => text,
        .Err: _ => return 6,
    }};
    defer text..&.deinit(gpa);

    if (!text.&.eq("kern-fs")) {{
        return 7;
    }}
    match (reader..&.close()) {{
        .Ok: _ => {{}},
        .Err: _ => return 8,
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
use std.mem.alloc.{{PageAllocator, GPAllocator}};

extern fn main() i32 {{
    let page = PageAllocator.{{}}..&;
    let gpa = GPAllocator.{{ backing: page }}..&;

    let mut created = match (fs.create_new(gpa, "{path}")) {{
        .Ok: file => file,
        .Err: _ => return 1,
    }};
    match (created..&.write_all("ab")) {{
        .Ok: count => {{
            if (count != 2) {{
                return 2;
            }}
        }},
        .Err: _ => return 3,
    }}
    created..&.deinit();

    let created_again = fs.create_new(gpa, "{path}");
    if (!created_again.is_err()) {{
        return 4;
    }}

    let mut appended = match (fs.open_append(gpa, "{path}")) {{
        .Ok: file => file,
        .Err: _ => return 5,
    }};
    match (appended..&.write_all("cd")) {{
        .Ok: count => {{
            if (count != 2) {{
                return 6;
            }}
        }},
        .Err: _ => return 7,
    }}
    appended..&.deinit();

    let mut writer = match (fs.open_write(gpa, "{path}")) {{
        .Ok: file => file,
        .Err: _ => return 8,
    }};
    match (writer..&.write("Z")) {{
        .Ok: count => {{
            if (count != 1) {{
                return 9;
            }}
        }},
        .Err: _ => return 10,
    }}
    writer..&.deinit();

    let mut text = match (fs.read_to_string(gpa, "{path}")) {{
        .Ok: text => text,
        .Err: _ => return 11,
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
use std.mem.alloc.{{PageAllocator, GPAllocator}};

extern fn main() i32 {{
    let page = PageAllocator.{{}}..&;
    let gpa = GPAllocator.{{ backing: page }}..&;

    match (fs.create_dir_all(gpa, "{dir_path}")) {{
        .Ok: _ => {{}},
        .Err: _ => return 1,
    }}

    let root_is_dir = match (fs.is_dir(gpa, "{root_path}")) {{
        .Ok: yes => yes,
        .Err: _ => return 2,
    }};
    if (!root_is_dir) {{
        return 3;
    }}

    let nested_is_dir = match (fs.is_dir(gpa, "{dir_path}")) {{
        .Ok: yes => yes,
        .Err: _ => return 4,
    }};
    if (!nested_is_dir) {{
        return 5;
    }}

    match (fs.create_dir_all(gpa, "{dir_path}")) {{
        .Ok: _ => {{}},
        .Err: _ => return 6,
    }}

    let written = match (fs.write_all(gpa, "{file_path}", "nested")) {{
        .Ok: count => count,
        .Err: _ => return 7,
    }};
    if (written != 6) {{
        return 8;
    }}

    let mut text = match (fs.read_to_string(gpa, "{file_path}")) {{
        .Ok: text => text,
        .Err: _ => return 9,
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
use std.mem.alloc.{{PageAllocator, GPAllocator}};

extern fn main() i32 {{
    let page = PageAllocator.{{}}..&;
    let gpa = GPAllocator.{{ backing: page }}..&;

    match (fs.create_dir_all(gpa, "{old_dir_path}")) {{
        .Ok: _ => {{}},
        .Err: _ => return 1,
    }}

    match (fs.write_all(gpa, "{old_file_path}", "rename-me")) {{
        .Ok: count => {{
            if (count != 9) {{
                return 2;
            }}
        }},
        .Err: _ => return 3,
    }}

    match (fs.rename(gpa, "{old_file_path}", "{renamed_file_path}")) {{
        .Ok: _ => {{}},
        .Err: _ => return 4,
    }}

    let old_file_exists = match (fs.exists(gpa, "{old_file_path}")) {{
        .Ok: exists => exists,
        .Err: _ => return 5,
    }};
    if (old_file_exists) {{
        return 6;
    }}

    let mut text = match (fs.read_to_string(gpa, "{renamed_file_path}")) {{
        .Ok: text => text,
        .Err: _ => return 7,
    }};
    defer text..&.deinit(gpa);
    if (!text.&.eq("rename-me")) {{
        return 8;
    }}

    match (fs.rename(gpa, "{old_dir_path}", "{new_dir_path}")) {{
        .Ok: _ => {{}},
        .Err: _ => return 9,
    }}

    let old_dir_exists = match (fs.exists(gpa, "{old_dir_path}")) {{
        .Ok: exists => exists,
        .Err: _ => return 10,
    }};
    if (old_dir_exists) {{
        return 11;
    }}

    let new_dir_is_dir = match (fs.is_dir(gpa, "{new_dir_path}")) {{
        .Ok: yes => yes,
        .Err: _ => return 12,
    }};
    if (!new_dir_is_dir) {{
        return 13;
    }}

    let new_file_exists = match (fs.exists(gpa, "{new_file_path}")) {{
        .Ok: exists => exists,
        .Err: _ => return 14,
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
use std.mem.alloc.{{PageAllocator, GPAllocator}};

extern fn main() i32 {{
    let page = PageAllocator.{{}}..&;
    let gpa = GPAllocator.{{ backing: page }}..&;

    match (fs.create_dir_all(gpa, "{alpha_path}")) {{
        .Ok: _ => {{}},
        .Err: _ => return 1,
    }}
    match (fs.write_all(gpa, "{file_a_path}", "A")) {{
        .Ok: _ => {{}},
        .Err: _ => return 2,
    }}
    match (fs.write_all(gpa, "{file_b_path}", "B")) {{
        .Ok: _ => {{}},
        .Err: _ => return 3,
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
        .Ok: count => count,
        .Err: _ => return 4,
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
        .Ok: count => count,
        .Err: _ => return 7,
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
use std.mem.alloc.{{PageAllocator, GPAllocator}};

extern fn main() i32 {{
    let page = PageAllocator.{{}}..&;
    let gpa = GPAllocator.{{ backing: page }}..&;

    match (fs.create_dir_all(gpa, "{nested_dir_path}")) {{
        .Ok: _ => {{}},
        .Err: _ => return 1,
    }}
    match (fs.write_all(gpa, "{nested_file_path}", "deep")) {{
        .Ok: _ => {{}},
        .Err: _ => return 2,
    }}
    match (fs.write_all(gpa, "{sibling_file_path}", "root")) {{
        .Ok: _ => {{}},
        .Err: _ => return 3,
    }}

    match (fs.remove_dir_all(gpa, "{root_path}")) {{
        .Ok: _ => {{}},
        .Err: _ => return 4,
    }}

    let root_exists = match (fs.exists(gpa, "{root_path}")) {{
        .Ok: exists => exists,
        .Err: _ => return 5,
    }};
    if (root_exists) {{
        return 6;
    }}

    let nested_exists = match (fs.exists(gpa, "{nested_file_path}")) {{
        .Ok: exists => exists,
        .Err: _ => return 7,
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

extern fn main() i32 {
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
use std.mem.alloc.{PageAllocator, GPAllocator};

extern fn main() i32 {
    let page = PageAllocator.{}..&;
    let gpa = GPAllocator.{ backing: page }..&;

    let mut joined = match (fs.join(gpa, "/tmp/kern", "src/main.kr")) {
        .Ok: path => path,
        .Err: _ => return 1,
    };
    defer joined..&.deinit(gpa);
    if (!joined.&.eq("/tmp/kern/src/main.kr")) {
        return 2;
    }

    let mut bare = match (fs.join(gpa, "", "note.txt")) {
        .Ok: path => path,
        .Err: _ => return 3,
    };
    defer bare..&.deinit(gpa);
    if (!bare.&.eq("note.txt")) {
        return 4;
    }

    let mut rooted = match (fs.join(gpa, "/tmp/kern", "/etc/passwd")) {
        .Ok: path => path,
        .Err: _ => return 5,
    };
    defer rooted..&.deinit(gpa);
    if (!rooted.&.eq("/etc/passwd")) {
        return 6;
    }

    let mut normalized = match (fs.normalize(gpa, "/tmp/./kern//src/../out/file.txt")) {
        .Ok: path => path,
        .Err: _ => return 7,
    };
    defer normalized..&.deinit(gpa);
    if (!normalized.&.eq("/tmp/kern/out/file.txt")) {
        return 8;
    }

    let mut relative = match (fs.normalize(gpa, "alpha/./beta/../gamma")) {
        .Ok: path => path,
        .Err: _ => return 9,
    };
    defer relative..&.deinit(gpa);
    if (!relative.&.eq("alpha/gamma")) {
        return 10;
    }

    let mut escaped = match (fs.normalize(gpa, "../../alpha/../beta")) {
        .Ok: path => path,
        .Err: _ => return 11,
    };
    defer escaped..&.deinit(gpa);
    if (!escaped.&.eq("../../beta")) {
        return 12;
    }

    let mut root = match (fs.normalize(gpa, "/alpha/../..")) {
        .Ok: path => path,
        .Err: _ => return 13,
    };
    defer root..&.deinit(gpa);
    if (!root.&.eq("/")) {
        return 14;
    }

    let mut empty = match (fs.normalize(gpa, "")) {
        .Ok: path => path,
        .Err: _ => return 15,
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
