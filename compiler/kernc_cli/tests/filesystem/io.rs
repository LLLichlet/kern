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

    let exists = match (fs.exists(gpa, "{path}")) {{
        .{{ Ok: value }} => value,
        .{{ Err: _ }} => return 20,
    }};
    if (!exists) {{
        return 21;
    }}
    let is_file = match (fs.is_file(gpa, "{path}")) {{
        .{{ Ok: value }} => value,
        .{{ Err: _ }} => return 22,
    }};
    if (!is_file) {{
        return 23;
    }}
    let is_dir = match (fs.is_dir(gpa, "{path}")) {{
        .{{ Ok: value }} => value,
        .{{ Err: _ }} => return 24,
    }};
    if (is_dir) {{
        return 25;
    }}
    let size = match (fs.file_size(gpa, "{path}")) {{
        .{{ Ok: value }} => value,
        .{{ Err: _ }} => return 26,
    }};
    if (size != 6) {{
        return 27;
    }}
    let empty = match (fs.is_empty_file(gpa, "{path}")) {{
        .{{ Ok: value }} => value,
        .{{ Err: _ }} => return 28,
    }};
    if (empty) {{
        return 29;
    }}

    let mut text = match (fs.read_to_string(gpa, "{path}")) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 3,
    }};
    defer text..&.deinit(gpa);

    if (text.& != "abc123") {{
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

    let missing_exists = match (fs.exists(gpa, "{path}")) {{
        .{{ Ok: value }} => value,
        .{{ Err: _ }} => return 30,
    }};
    if (missing_exists) {{
        return 31;
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

    if (text.& != "kern-fs") {{
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
fn runs_hosted_program_using_std_fs_exact_and_byte_reads() {
    let temp_file = unique_temp_path("kernc_std_fs_exact_reads", "bin");
    let temp_path = kern_string_literal(&temp_file);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    let written = match (fs.write_all(gpa, "{path}", "abcdef")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 1,
    }};
    if (written != 6) {{
        return 2;
    }}

    let mut bytes = match (fs.read_all(gpa, "{path}")) {{
        .{{ Ok: bytes }} => bytes,
        .{{ Err: _ }} => return 3,
    }};
    defer bytes..&.deinit(gpa);
    if (bytes.as_slice() != "abcdef") {{
        return 4;
    }}

    let mut file = match (fs.open_read(gpa, "{path}")) {{
        .{{ Ok: file }} => file,
        .{{ Err: _ }} => return 5,
    }};
    defer file..&.deinit();

    let mut first = [3]u8.{{undef}};
    match (file..&.read_exact(first..[0 .. 3])) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 6,
    }}
    if (first.[0 .. 3] != "abc") {{
        return 7;
    }}

    let mut rest = match (file..&.read_to_end(gpa)) {{
        .{{ Ok: bytes }} => bytes,
        .{{ Err: _ }} => return 8,
    }};
    defer rest..&.deinit(gpa);
    if (rest.as_slice() != "def") {{
        return 9;
    }}

    let mut empty = [0]u8.{{}};
    match (file..&.read_exact(empty..[0 .. 0])) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 10,
    }}

    let mut too_much = [1]u8.{{undef}};
    match (file..&.read_exact(too_much..[0 .. 1])) {{
        .{{ Ok: _ }} => return 11,
        .{{ Err: err }} => match (err) {{
            .UnexpectedEof => {{}},
            _ => return 12,
        }},
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
fn runs_hosted_program_using_std_fs_seek_truncate_and_flush() {
    let temp_file = unique_temp_path("kernc_std_fs_seek_truncate", "bin");
    let temp_path = kern_string_literal(&temp_file);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    let options = fs.OpenOptions.{{
        read: true,
        write: true,
        create: true,
        truncate: true,
    }};
    let mut file = match (fs.open(gpa, "{path}", options.&)) {{
        .{{ Ok: file }} => file,
        .{{ Err: _ }} => return 1,
    }};
    defer file..&.deinit();

    let written = match (file..&.write_all("abcdef")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 2,
    }};
    if (written != 6) {{
        return 3;
    }}
    let pos_after_write = match (file..&.tell()) {{
        .{{ Ok: pos }} => pos,
        .{{ Err: _ }} => return 4,
    }};
    if (pos_after_write != 6) {{
        return 4;
    }}

    let pos_after_seek = match (file..&.seek(.{{ Start: 2 }})) {{
        .{{ Ok: pos }} => pos,
        .{{ Err: _ }} => return 5,
    }};
    if (pos_after_seek != 2) {{
        return 5;
    }}
    let rewritten = match (file..&.write_all("XY")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 6,
    }};
    if (rewritten != 2) {{
        return 6;
    }}
    let pos_after_rewrite = match (file..&.tell()) {{
        .{{ Ok: pos }} => pos,
        .{{ Err: _ }} => return 7,
    }};
    if (pos_after_rewrite != 4) {{
        return 7;
    }}
    match (file..&.flush()) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 8,
    }}

    let start_pos = match (file..&.seek(.{{ Start: 0 }})) {{
        .{{ Ok: pos }} => pos,
        .{{ Err: _ }} => return 9,
    }};
    if (start_pos != 0) {{
        return 9;
    }}
    let mut all = [6]u8.{{undef}};
    match (file..&.read_exact(all..[0 .. 6])) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 10,
    }}
    if (all.[0 .. 6] != "abXYef") {{
        return 11;
    }}

    let tail_pos = match (file..&.seek(.{{ End: -2 }})) {{
        .{{ Ok: pos }} => pos,
        .{{ Err: _ }} => return 12,
    }};
    if (tail_pos != 4) {{
        return 12;
    }}
    let mut tail = [2]u8.{{undef}};
    match (file..&.read_exact(tail..[0 .. 2])) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 13,
    }}
    if (tail.[0 .. 2] != "ef") {{
        return 14;
    }}

    match (file..&.truncate(4)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 15,
    }}
    let reread_pos = match (file..&.seek(.{{ Start: 0 }})) {{
        .{{ Ok: pos }} => pos,
        .{{ Err: _ }} => return 16,
    }};
    if (reread_pos != 0) {{
        return 16;
    }}
    let mut remaining = match (file..&.read_to_end(gpa)) {{
        .{{ Ok: bytes }} => bytes,
        .{{ Err: _ }} => return 17,
    }};
    defer remaining..&.deinit(gpa);
    if (remaining.as_slice() != "abXY") {{
        return 18;
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

    if (text.& != "Zbcd") {{
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
    if (text.& != "rename-me") {{
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
fn runs_hosted_program_using_std_fs_atomic_tmp_write() {
    let temp_root = unique_temp_path("kernc_std_fs_atomic_write", "dir");
    let target_file = temp_root.join("target.txt");
    let tmp_file = temp_root.join("target.tmp");
    let bad_target = temp_root.join("missing").join("out.txt");
    let bad_tmp = temp_root.join("bad.tmp");
    let root_path = kern_string_literal(&temp_root);
    let target_path = kern_string_literal(&target_file);
    let tmp_path = kern_string_literal(&tmp_file);
    let bad_target_path = kern_string_literal(&bad_target);
    let bad_tmp_path = kern_string_literal(&bad_tmp);

    let _ = fs::remove_file(&target_file);
    let _ = fs::remove_file(&tmp_file);
    let _ = fs::remove_file(&bad_tmp);
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

    match (fs.write_all(gpa, "{target_path}", "old")) {{
        .{{ Ok: count }} => {{
            if (count != 3) {{
                return 2;
            }}
        }},
        .{{ Err: _ }} => return 3,
    }}

    let written = match (fs.write_all_atomic_tmp(gpa, "{target_path}", "{tmp_path}", "new-data")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 4,
    }};
    if (written != 8) {{
        return 5;
    }}

    let tmp_exists = match (fs.exists(gpa, "{tmp_path}")) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 6,
    }};
    if (tmp_exists) {{
        return 7;
    }}

    let mut text = match (fs.read_to_string(gpa, "{target_path}")) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 8,
    }};
    defer text..&.deinit(gpa);
    if (text.& != "new-data") {{
        return 9;
    }}

    let failed = fs.write_all_atomic_tmp(gpa, "{bad_target_path}", "{bad_tmp_path}", "bad");
    if (!failed.is_err()) {{
        return 10;
    }}

    let bad_tmp_exists = match (fs.exists(gpa, "{bad_tmp_path}")) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 11,
    }};
    if (bad_tmp_exists) {{
        return 12;
    }}

    let mut after_failure = match (fs.read_to_string(gpa, "{target_path}")) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 13,
    }};
    defer after_failure..&.deinit(gpa);
    if (after_failure.& != "new-data") {{
        return 14;
    }}

    return 0;
}}
"#,
        root_path = root_path,
        target_path = target_path,
        tmp_path = tmp_path,
        bad_target_path = bad_target_path,
        bad_tmp_path = bad_tmp_path
    ));

    assert!(
        output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_file(&target_file);
    let _ = fs::remove_file(&tmp_file);
    let _ = fs::remove_file(&bad_tmp);
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
    if (text.& != "kern-lang") {{
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
    if (text2.& != "kern") {{
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
