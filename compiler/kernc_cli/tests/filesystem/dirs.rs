use super::*;

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
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    let dir_exists_before = match ("{dir_path}".path().exists(gpa)) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 1,
    }};
    if (dir_exists_before) {{
        return 2;
    }}

    match ("{dir_path}".path().create_dir(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 3,
    }}

    let dir_exists = match ("{dir_path}".path().exists(gpa)) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 4,
    }};
    if (!dir_exists) {{
        return 5;
    }}

    let dir_meta = match ("{dir_path}".path().metadata(gpa)) {{
        .{{ Ok: meta }} => meta,
        .{{ Err: _ }} => return 6,
    }};
    if (!dir_meta.is_dir() or dir_meta.is_file()) {{
        return 7;
    }}

    let dir_is_dir = match ("{dir_path}".path().is_dir(gpa)) {{
        .{{ Ok: yes }} => yes,
        .{{ Err: _ }} => return 8,
    }};
    if (!dir_is_dir) {{
        return 9;
    }}

    let file_exists_before = match ("{file_path}".path().exists(gpa)) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 10,
    }};
    if (file_exists_before) {{
        return 11;
    }}

    let written = match ("{file_path}".path().write_all(gpa, "hello")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 12,
    }};
    if (written != 5) {{
        return 13;
    }}

    let file_meta = match ("{file_path}".path().metadata(gpa)) {{
        .{{ Ok: meta }} => meta,
        .{{ Err: _ }} => return 14,
    }};
    if (!file_meta.is_file() or file_meta.is_dir()) {{
        return 15;
    }}
    if (file_meta.size != 5) {{
        return 16;
    }}

    let file_is_file = match ("{file_path}".path().is_file(gpa)) {{
        .{{ Ok: yes }} => yes,
        .{{ Err: _ }} => return 17,
    }};
    if (!file_is_file) {{
        return 18;
    }}

    let file_is_dir = match ("{file_path}".path().is_dir(gpa)) {{
        .{{ Ok: yes }} => yes,
        .{{ Err: _ }} => return 19,
    }};
    if (file_is_dir) {{
        return 20;
    }}

    match ("{file_path}".path().remove_file(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 21,
    }}

    let file_exists_after = match ("{file_path}".path().exists(gpa)) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 22,
    }};
    if (file_exists_after) {{
        return 23;
    }}

    match ("{dir_path}".path().remove_dir(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 24,
    }}

    let dir_exists_after = match ("{dir_path}".path().exists(gpa)) {{
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
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    match ("{dir_path}".path().create_dir_all(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 1,
    }}

    let root_is_dir = match ("{root_path}".path().is_dir(gpa)) {{
        .{{ Ok: yes }} => yes,
        .{{ Err: _ }} => return 2,
    }};
    if (!root_is_dir) {{
        return 3;
    }}

    let nested_is_dir = match ("{dir_path}".path().is_dir(gpa)) {{
        .{{ Ok: yes }} => yes,
        .{{ Err: _ }} => return 4,
    }};
    if (!nested_is_dir) {{
        return 5;
    }}

    match ("{dir_path}".path().create_dir_all(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 6,
    }}

    let written = match ("{file_path}".path().write_all(gpa, "nested")) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 7,
    }};
    if (written != 6) {{
        return 8;
    }}

    let mut text = match ("{file_path}".path().read_to_string(gpa)) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 9,
    }};
    defer text..&.deinit(gpa);

    if (text.& != "nested") {{
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
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    match ("{alpha_path}".path().create_dir_all(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 1,
    }}
    match ("{file_a_path}".path().write_all(gpa, "A")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 2,
    }}
    match ("{file_b_path}".path().write_all(gpa, "B")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 3,
    }}

    let mut total = usize.{{0}};
    let mut saw_alpha_dir = bool.{{false}};
    let mut saw_a_file = bool.{{false}};
    let mut saw_b_file = bool.{{false}};

    let visited = match ("{root_path}".path().read_dir(gpa, [
        total = total..&,
        saw_alpha_dir = saw_alpha_dir..&,
        saw_a_file = saw_a_file..&,
        saw_b_file = saw_b_file..&
    ](entry: fs.DirEntry) bool {{
        total.* += 1;
        if (entry.name == "alpha") {{
            if (!entry.is_dir()) {{
                return false;
            }}
            saw_alpha_dir.* = true;
        }}
        if (entry.name == "a.txt") {{
            if (!entry.is_file()) {{
                return false;
            }}
            saw_a_file.* = true;
        }}
        if (entry.name == "b.txt") {{
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
    let stopped = match ("{root_path}".path().read_dir(gpa, [
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
fn runs_hosted_program_using_std_fs_owned_dir_entries_and_errors() {
    let temp_root = unique_temp_path("kernc_std_fs_owned_dir_entries", "dir");
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
use base.cmp.Ordering;
use std.{{fs, io}};
use base.mem.alloc.GPA;
use sys.mem.Page;

fn entry_cmp(left: fs.OwnedDirEntry, right: fs.OwnedDirEntry) Ordering {{
    return left.name.lex_cmp(right.name);
}}

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    match ("{alpha_path}".path().create_dir_all(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 1,
    }}
    match ("{file_b_path}".path().write_all(gpa, "B")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 2,
    }}
    match ("{file_a_path}".path().write_all(gpa, "A")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 3,
    }}

    let mut entries = match ("{root_path}".path().read_dir_entries(gpa)) {{
        .{{ Ok: entries }} => entries,
        .{{ Err: _ }} => return 4,
    }};
    defer entries..&.deinit(gpa);

    if (entries.&.len() != 3) {{
        return 5;
    }}

    entries..&.as_mut_slice().sort_by(entry_cmp);
    let items = entries.&.as_slice();
    if (items.[0].name != "a.txt" or !items.[0].is_file()) {{
        return 6;
    }}
    if (items.[1].name != "alpha" or !items.[1].is_dir()) {{
        return 7;
    }}
    if (items.[2].name != "b.txt" or !items.[2].is_file()) {{
        return 8;
    }}

    let count = match ("{alpha_path}".path().read_dir_entries_into(gpa, entries..&)) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 9,
    }};
    if (count != 0 or !entries.&.is_empty()) {{
        return 10;
    }}

    let err = match ("{root_path}/missing.txt".path().metadata(gpa)) {{
        .{{ Ok: _ }} => return 11,
        .{{ Err: err }} => err,
    }};
    if (err.kind() != fs.ErrorKind.NotFound) {{
        return 12;
    }}
    if (err.os_code().is_none()) {{
        return 13;
    }}
    if (!err.is_not_found() or err.is_already_exists()) {{
        return 14;
    }}
    "fs error: {{}}".fmt(.{{err}}).println();

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
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    match ("{nested_dir_path}".path().create_dir_all(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 1,
    }}
    match ("{nested_file_path}".path().write_all(gpa, "deep")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 2,
    }}
    match ("{sibling_file_path}".path().write_all(gpa, "root")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 3,
    }}

    match ("{root_path}".path().remove_dir_all(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 4,
    }}

    let root_exists = match ("{root_path}".path().exists(gpa)) {{
        .{{ Ok: exists }} => exists,
        .{{ Err: _ }} => return 5,
    }};
    if (root_exists) {{
        return 6;
    }}

    let nested_exists = match ("{nested_file_path}".path().exists(gpa)) {{
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
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    match ("{beta_path}".path().create_dir_all(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 1,
    }}
    match ("{root_file_path}".path().write_all(gpa, "root")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 2,
    }}
    match ("{beta_file_path}".path().write_all(gpa, "deep")) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 3,
    }}

    let mut saw_alpha = bool.{{false}};
    let mut saw_beta = bool.{{false}};
    let mut saw_root_file = bool.{{false}};
    let mut saw_beta_file = bool.{{false}};

    let walked = match ("{root_path}".path().walk_dir(gpa, [
        saw_alpha = saw_alpha..&,
        saw_beta = saw_beta..&,
        saw_root_file = saw_root_file..&,
        saw_beta_file = saw_beta_file..&
    ](path: &[u8], entry: fs.DirEntry, depth: usize) bool {{
        if (path == "{alpha_path}") {{
            if (!entry.is_dir() or depth != 1) {{
                return false;
            }}
            saw_alpha.* = true;
        }}
        if (path == "{root_file_path}") {{
            if (!entry.is_file() or depth != 1) {{
                return false;
            }}
            saw_root_file.* = true;
        }}
        if (path == "{beta_path}") {{
            if (!entry.is_dir() or depth != 2) {{
                return false;
            }}
            saw_beta.* = true;
        }}
        if (path == "{beta_file_path}") {{
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
    let walked_files = match ("{root_path}".path().walk_files(gpa, [
        file_hits = file_hits..&
    ](_: &[u8], entry: fs.DirEntry, _: usize) bool {{
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
    let walked_dirs = match ("{root_path}".path().walk_dirs(gpa, [
        dir_hits = dir_hits..&
    ](_: &[u8], entry: fs.DirEntry, _: usize) bool {{
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
    let stopped = match ("{root_path}".path().walk_dir(gpa, [
        early = early..&
    ](_: &[u8], _: fs.DirEntry, _: usize) bool {{
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
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    let missing_dir = match ("{root_path}".path().remove_dir_if_exists(gpa)) {{
        .{{ Ok: removed }} => removed,
        .{{ Err: _ }} => return 1,
    }};
    if (missing_dir) {{
        return 2;
    }}

    let created = match ("{root_path}".path().create_dir_if_missing(gpa)) {{
        .{{ Ok: created }} => created,
        .{{ Err: _ }} => return 3,
    }};
    if (!created) {{
        return 4;
    }}

    let created_again = match ("{root_path}".path().create_dir_if_missing(gpa)) {{
        .{{ Ok: created }} => created,
        .{{ Err: _ }} => return 5,
    }};
    if (created_again) {{
        return 6;
    }}

    match ("{file_path}".path().write_all(gpa, "payload")) {{
        .{{ Ok: count }} => {{
            if (count != 7) {{
                return 7;
            }}
        }},
        .{{ Err: _ }} => return 8,
    }}

    let removed_file = match ("{file_path}".path().remove_file_if_exists(gpa)) {{
        .{{ Ok: removed }} => removed,
        .{{ Err: _ }} => return 9,
    }};
    if (!removed_file) {{
        return 10;
    }}

    let removed_file_again = match ("{file_path}".path().remove_file_if_exists(gpa)) {{
        .{{ Ok: removed }} => removed,
        .{{ Err: _ }} => return 11,
    }};
    if (removed_file_again) {{
        return 12;
    }}

    let removed_dir = match ("{root_path}".path().remove_dir_if_exists(gpa)) {{
        .{{ Ok: removed }} => removed,
        .{{ Err: _ }} => return 13,
    }};
    if (!removed_dir) {{
        return 14;
    }}

    let removed_dir_again = match ("{root_path}".path().remove_dir_if_exists(gpa)) {{
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

#[test]
fn runs_hosted_program_using_std_fs_path_directory_combinators() {
    let temp_root = unique_temp_path("kernc_std_fs_path_dir_combinators", "dir");
    let file_path = temp_root.join("payload.txt");
    let root_path = kern_string_literal(&temp_root);
    let file_path_str = kern_string_literal(&file_path);

    let _ = fs::remove_file(&file_path);
    let _ = fs::remove_dir_all(&temp_root);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;
    let root = "{root_path}".path();
    let file = "{file_path}".path();

    let missing = match (root.remove_dir_if_exists(gpa)) {{
        .{{ Ok: removed }} => removed,
        .{{ Err: _ }} => return 1,
    }};
    if (missing) {{
        return 2;
    }}

    let created = match (root.create_dir_all(gpa)) {{
        .{{ Ok: created }} => created,
        .{{ Err: _ }} => return 3,
    }};
    if (!created) {{
        return 4;
    }}

    match (file.write_all(gpa, "payload")) {{
        .{{ Ok: count }} => {{
            if (count != 7) {{
                return 5;
            }}
        }},
        .{{ Err: _ }} => return 6,
    }}

    let mut hits = usize.{{0}};
    let visited = match (root.read_dir(gpa, [
        hits = hits..&
    ](entry: fs.DirEntry) bool {{
        if (entry.name == "payload.txt" and entry.is_file()) {{
            hits.* += 1;
        }}
        return true;
    }})) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 7,
    }};
    if (visited != 1 or hits != 1) {{
        return 8;
    }}

    let walked = match (root.walk_files(gpa, [
        hits = hits..&
    ](_: &[u8], entry: fs.DirEntry, _: usize) bool {{
        if (entry.name == "payload.txt") {{
            hits.* += 1;
        }}
        return true;
    }})) {{
        .{{ Ok: count }} => count,
        .{{ Err: _ }} => return 9,
    }};
    if (walked != 1 or hits != 2) {{
        return 10;
    }}

    match (root.remove_dir_all(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 11,
    }}

    let exists = match (root.exists(gpa)) {{
        .{{ Ok: value }} => value,
        .{{ Err: _ }} => return 12,
    }};
    if (exists) {{
        return 13;
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
