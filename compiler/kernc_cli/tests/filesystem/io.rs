use super::*;

#[test]
fn runs_hosted_program_with_fs_create_followed_by_another_result_match() {
    let temp_file = unique_temp_path("kernc_std_fs_create_chain", "txt");
    let temp_path = kern_string_literal(&temp_file);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use sys.os;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn ok_bool() bool!os.Error {{
    return .{{ Ok: true }};
}}

fn main() i32 {{
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
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {{
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
fn runs_hosted_program_using_std_fs_roundtrip() {
    let temp_file = unique_temp_path("kernc_std_fs_roundtrip", "txt");
    let temp_path = kern_string_literal(&temp_file);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {{
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
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {{
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
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {{
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
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {{
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
