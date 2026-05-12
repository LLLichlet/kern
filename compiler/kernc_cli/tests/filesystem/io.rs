use super::*;

#[test]
fn runs_hosted_program_with_fs_create_followed_by_another_result_match() {
    let temp_file = unique_temp_path("kernc_std_fs_create_chain", "txt");
    let temp_path = kern_string_literal(&temp_file);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use base.mem.alloc.gpa;
use std.mem.Page;

fn ok_bool() bool!fs.Error {{
    return .{{ Ok: true }};
}}

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = gpa().on(page)..&;

    let mut writer = match ("{path}".path().create(gpa)) {{
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
        "hosted std binary failed with status {:?}:\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
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
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = gpa().on(page)..&;

    let written = match ("{path}".path().write_all(gpa, "abc123")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 1,
    }};
    if (written != 6) {{
        return 2;
    }}

    let exists = match ("{path}".path().exists(gpa)) {{
        .{{ Ok: value }} => value,
        .{{ Err: _ }} => return 20,
    }};
    if (!exists) {{
        return 21;
    }}
    let is_file = match ("{path}".path().is_file(gpa)) {{
        .{{ Ok: value }} => value,
        .{{ Err: _ }} => return 22,
    }};
    if (!is_file) {{
        return 23;
    }}
    let is_dir = match ("{path}".path().is_dir(gpa)) {{
        .{{ Ok: value }} => value,
        .{{ Err: _ }} => return 24,
    }};
    if (is_dir) {{
        return 25;
    }}
    let size = match ("{path}".path().file_size(gpa)) {{
        .{{ Ok: value }} => value,
        .{{ Err: _ }} => return 26,
    }};
    if (size != 6) {{
        return 27;
    }}
    let empty = match ("{path}".path().is_empty_file(gpa)) {{
        .{{ Ok: value }} => value,
        .{{ Err: _ }} => return 28,
    }};
    if (empty) {{
        return 29;
    }}

    let mut text = match ("{path}".path().read_to_string(gpa)) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 3,
    }};
    defer text..&.deinit(gpa);

    if (text.& != "abc123") {{
        return 4;
    }}

    match ("{path}".path().remove_file(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 5,
    }}

    let missing = "{path}".path().open_read(gpa);
    if (!missing.is_err()) {{
        return 6;
    }}

    let missing_exists = match ("{path}".path().exists(gpa)) {{
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
        "hosted std binary failed with status {:?}:\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn runs_hosted_program_using_std_fs_path_view_methods() {
    let temp_file = unique_temp_path("kernc_std_fs_path_view", "txt");
    let temp_path = kern_string_literal(&temp_file);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = gpa().on(page)..&;
    let path = "{path}".path();

    let written = match (path.write_all(gpa, "via path")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 1,
    }};
    if (written != 8) {{
        return 2;
    }}

    let exists = match (path.exists(gpa)) {{
        .{{ Ok: value }} => value,
        .{{ Err: _ }} => return 3,
    }};
    if (!exists) {{
        return 4;
    }}

    let mut text = match (path.read_to_string(gpa)) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 5,
    }};
    defer text..&.deinit(gpa);
    if (text.& != "via path") {{
        return 6;
    }}

    match (path.remove_file(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 7,
    }}

    return 0;
}}
"#,
        path = temp_path
    ));

    assert!(
        output.status.success(),
        "hosted std binary failed with status {:?}:\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_file(&temp_file);
}

#[test]
fn runs_hosted_program_using_owned_string_path_view_methods() {
    let temp_dir = unique_temp_path("kernc_std_fs_owned_string_path", "dir");
    let root_path = kern_string_literal(&temp_dir);

    let _ = fs::remove_file(temp_dir.join("joined.txt"));
    let _ = fs::remove_dir_all(&temp_dir);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = gpa().on(page)..&;
    let root = "{root_path}".path();

    match (root.create_dir_all(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 1,
    }}

    let mut joined = match (root.join(gpa, "joined.txt")) {{
        .{{ Ok: path }} => path,
        .{{ Err: _ }} => return 2,
    }};
    defer joined..&.deinit(gpa);

    let written = match (joined.&.path().write_all(gpa, "owned path")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 3,
    }};
    if (written != 10) {{
        return 4;
    }}

    let mut text = match (joined.&.path().read_to_string(gpa)) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 5,
    }};
    defer text..&.deinit(gpa);
    if (text.& != "owned path") {{
        return 6;
    }}

    match (root.remove_dir_all(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 7,
    }}

    return 0;
}}
"#,
        root_path = root_path
    ));

    assert!(
        output.status.success(),
        "hosted std binary failed with status {:?}:\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_file(temp_dir.join("joined.txt"));
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn runs_hosted_program_using_std_fs_roundtrip() {
    let temp_file = unique_temp_path("kernc_std_fs_roundtrip", "txt");
    let temp_path = kern_string_literal(&temp_file);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = gpa().on(page)..&;

    let mut writer = match ("{path}".path().create(gpa)) {{
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

    let mut reader = match ("{path}".path().open_read(gpa)) {{
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

    let missing = "{path}.missing".path().open_read(gpa);
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
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = gpa().on(page)..&;

    let written = match ("{path}".path().write_all(gpa, "abcdef")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 1,
    }};
    if (written != 6) {{
        return 2;
    }}

    let mut bytes = match ("{path}".path().read_all(gpa)) {{
        .{{ Ok: bytes }} => bytes,
        .{{ Err: _ }} => return 3,
    }};
    defer bytes..&.deinit(gpa);
    if (bytes.as_slice() != "abcdef") {{
        return 4;
    }}

    let mut file = match ("{path}".path().open_read(gpa)) {{
        .{{ Ok: file }} => file,
        .{{ Err: _ }} => return 5,
    }};
    defer file..&.deinit();

    let mut first = [3]u8.{{undef}};
    match (file..&.read_exact(first..&[0 .. 3])) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 6,
    }}
    if (first.&[0 .. 3] != "abc") {{
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
    match (file..&.read_exact(empty..&[0 .. 0])) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 10,
    }}

    let mut too_much = [1]u8.{{undef}};
    match (file..&.read_exact(too_much..&[0 .. 1])) {{
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
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = gpa().on(page)..&;

    let options = fs.OpenOptions.{{
        read: true,
        write: true,
        create: true,
        truncate: true,
    }};
    let mut file = match ("{path}".path().open(gpa, options.&)) {{
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
    match (file..&.read_exact(all..&[0 .. 6])) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 10,
    }}
    if (all.&[0 .. 6] != "abXYef") {{
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
    match (file..&.read_exact(tail..&[0 .. 2])) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 13,
    }}
    if (tail.&[0 .. 2] != "ef") {{
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
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = gpa().on(page)..&;

    let mut created = match ("{path}".path().create_new(gpa)) {{
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

    let created_again = "{path}".path().create_new(gpa);
    if (!created_again.is_err()) {{
        return 4;
    }}

    let mut appended = match ("{path}".path().open_append(gpa)) {{
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

    let mut writer = match ("{path}".path().open_write(gpa)) {{
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

    let mut text = match ("{path}".path().read_to_string(gpa)) {{
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
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = gpa().on(page)..&;

    match ("{old_dir_path}".path().create_dir_all(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 1,
    }}

    match ("{old_file_path}".path().write_all(gpa, "rename-me")) {{
        .{{ Ok: count }} => {{
            if (count != 9) {{
                return 2;
            }}
        }},
        .{{ Err: _ }} => return 3,
    }}

    match ("{old_file_path}".path().rename(gpa, "{renamed_file_path}")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 4,
    }}

    let old_file_exists = match ("{old_file_path}".path().exists(gpa)) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 5,
    }};
    if (old_file_exists) {{
        return 6;
    }}

    let mut text = match ("{renamed_file_path}".path().read_to_string(gpa)) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 7,
    }};
    defer text..&.deinit(gpa);
    if (text.& != "rename-me") {{
        return 8;
    }}

    match ("{old_dir_path}".path().rename(gpa, "{new_dir_path}")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 9,
    }}

    let old_dir_exists = match ("{old_dir_path}".path().exists(gpa)) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 10,
    }};
    if (old_dir_exists) {{
        return 11;
    }}

    let new_dir_is_dir = match ("{new_dir_path}".path().is_dir(gpa)) {{
        .{{ Ok: yes }} => yes,
        .{{ Err: _ }} => return 12,
    }};
    if (!new_dir_is_dir) {{
        return 13;
    }}

    let new_file_exists = match ("{new_file_path}".path().exists(gpa)) {{
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
    let auto_target_file = temp_root.join("auto.txt");
    let tmp_file = temp_root.join("target.tmp");
    let bad_target = temp_root.join("missing").join("out.txt");
    let bad_tmp = temp_root.join("bad.tmp");
    let root_path = kern_string_literal(&temp_root);
    let target_path = kern_string_literal(&target_file);
    let auto_target_path = kern_string_literal(&auto_target_file);
    let tmp_path = kern_string_literal(&tmp_file);
    let bad_target_path = kern_string_literal(&bad_target);
    let bad_tmp_path = kern_string_literal(&bad_tmp);

    let _ = fs::remove_file(&target_file);
    let _ = fs::remove_file(&auto_target_file);
    let _ = fs::remove_file(&tmp_file);
    let _ = fs::remove_file(&bad_tmp);
    let _ = fs::remove_dir_all(&temp_root);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use std.proc;
use base.io.Write;
use base.coll.String;
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = gpa().on(page)..&;

    match ("{root_path}".path().create_dir_all(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 1,
    }}

    match ("{target_path}".path().write_all(gpa, "old")) {{
        .{{ Ok: count }} => {{
            if (count != 3) {{
                return 2;
            }}
        }},
        .{{ Err: _ }} => return 3,
    }}

    let written = match ("{target_path}".path().write_all_atomic_tmp(gpa, "{tmp_path}", "new-data")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 4,
    }};
    if (written != 8) {{
        return 5;
    }}

    let tmp_exists = match ("{tmp_path}".path().exists(gpa)) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 6,
    }};
    if (tmp_exists) {{
        return 7;
    }}

    let mut text = match ("{target_path}".path().read_to_string(gpa)) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 8,
    }};
    defer text..&.deinit(gpa);
    if (text.& != "new-data") {{
        return 9;
    }}

    let failed = "{bad_target_path}".path().write_all_atomic_tmp(gpa, "{bad_tmp_path}", "bad");
    if (!failed.is_err()) {{
        return 10;
    }}

    let bad_tmp_exists = match ("{bad_tmp_path}".path().exists(gpa)) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 11,
    }};
    if (bad_tmp_exists) {{
        return 12;
    }}

    let mut after_failure = match ("{target_path}".path().read_to_string(gpa)) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 13,
    }};
    defer after_failure..&.deinit(gpa);
    if (after_failure.& != "new-data") {{
        return 14;
    }}

    let mut auto_tmp0 = String.{{}};
    defer auto_tmp0..&.deinit(gpa);
    {{
        let mut sink = auto_tmp0..&.writer(gpa);
        let writer = &mut Write.{{ sink..& }};
        "{{}}.tmp.{{}}.{{}}".fmt(.{{ "{auto_target_path}", proc.process_id(), usize.{{0}}, }}).write_to(writer);
        if (sink..&.did_fail()) {{
            return 15;
        }}
    }}

    let mut auto_tmp1 = String.{{}};
    defer auto_tmp1..&.deinit(gpa);
    {{
        let mut sink = auto_tmp1..&.writer(gpa);
        let writer = &mut Write.{{ sink..& }};
        "{{}}.tmp.{{}}.{{}}".fmt(.{{ "{auto_target_path}", proc.process_id(), usize.{{1}}, }}).write_to(writer);
        if (sink..&.did_fail()) {{
            return 16;
        }}
    }}

    match (auto_tmp0.&.as_str().path().write_all(gpa, "blocked")) {{
        .{{ Ok: count }} => {{
            if (count != 7) {{
                return 17;
            }}
        }},
        .{{ Err: _ }} => return 18,
    }}

    let auto_written = match ("{auto_target_path}".path().write_all_atomic(gpa, "auto-data")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 19,
    }};
    if (auto_written != 9) {{
        return 20;
    }}

    let mut auto_text = match ("{auto_target_path}".path().read_to_string(gpa)) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 21,
    }};
    defer auto_text..&.deinit(gpa);
    if (auto_text.& != "auto-data") {{
        return 22;
    }}

    let mut collision_text = match (auto_tmp0.&.as_str().path().read_to_string(gpa)) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 23,
    }};
    defer collision_text..&.deinit(gpa);
    if (collision_text.& != "blocked") {{
        return 24;
    }}

    let tmp1_exists = match (auto_tmp1.&.as_str().path().exists(gpa)) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 25,
    }};
    if (tmp1_exists) {{
        return 26;
    }}

    return 0;
}}
"#,
        root_path = root_path,
        target_path = target_path,
        auto_target_path = auto_target_path,
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
    let _ = fs::remove_file(&auto_target_file);
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
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = gpa().on(page)..&;

    match ("{root_path}".path().create_dir_all(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 1,
    }}

    let written = match ("{from_path}".path().write_all(gpa, "kern")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 2,
    }};
    if (written != 4) {{
        return 3;
    }}

    let copied = match ("{from_path}".path().copy_to(gpa, "{to_path}")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 4,
    }};
    if (copied != 4) {{
        return 5;
    }}

    let appended = match ("{to_path}".path().append_all(gpa, "-lang")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 6,
    }};
    if (appended != 5) {{
        return 7;
    }}

    let mut text = match ("{to_path}".path().read_to_string(gpa)) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 8,
    }};
    defer text..&.deinit(gpa);
    if (text.& != "kern-lang") {{
        return 9;
    }}

    let mut src = match ("{from_path}".path().open_read(gpa)) {{
        .{{ Ok: file }} => file,
        .{{ Err: _ }} => return 10,
    }};
    defer src..&.deinit();

    let mut dst = match ("{to_path}".path().create(gpa)) {{
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

    let mut text2 = match ("{to_path}".path().read_to_string(gpa)) {{
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
