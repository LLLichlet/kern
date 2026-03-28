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
