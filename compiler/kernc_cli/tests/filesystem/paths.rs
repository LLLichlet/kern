use super::*;

#[test]
fn runs_hosted_program_using_std_fs_path_views() {
    let output = build_and_run_hosted(
        r#"
use std.fs;

fn main() i32 {
    let path = "/tmp/kern/archive.tar";

    if (!fs.file_name(path).is_some_and([](name: &[u8]) bool {
        return name == "archive.tar";
    })) {
        return 1;
    }
    if (!fs.parent(path).is_some_and([](dir: &[u8]) bool {
        return dir == "/tmp/kern";
    })) {
        return 2;
    }
    if (!fs.extension(path).is_some_and([](ext: &[u8]) bool {
        return ext == "tar";
    })) {
        return 3;
    }
    if (!fs.file_stem(path).is_some_and([](stem: &[u8]) bool {
        return stem == "archive";
    })) {
        return 4;
    }

    if (!fs.parent("/tmp/kern/").is_some_and([](dir: &[u8]) bool {
        return dir == "/tmp";
    })) {
        return 5;
    }
    if (!fs.parent("/tmp").is_some_and([](dir: &[u8]) bool {
        return dir == "/";
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

    if (!fs.file_stem(".gitignore").is_some_and([](stem: &[u8]) bool {
        return stem == ".gitignore";
    })) {
        return 10;
    }
    if (fs.extension(".gitignore").is_some()) {
        return 11;
    }
    if (!fs.file_stem("config.").is_some_and([](stem: &[u8]) bool {
        return stem == "config";
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
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;

    let mut joined = match (fs.join(gpa, "/tmp/kern", "src/main.rn")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 1,
    };
    defer joined..&.deinit(gpa);
    if (joined.& != "/tmp/kern/src/main.rn") {
        return 2;
    }

    let mut bare = match (fs.join(gpa, "", "note.txt")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 3,
    };
    defer bare..&.deinit(gpa);
    if (bare.& != "note.txt") {
        return 4;
    }

    let mut rooted = match (fs.join(gpa, "/tmp/kern", "/etc/passwd")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 5,
    };
    defer rooted..&.deinit(gpa);
    if (rooted.& != "/etc/passwd") {
        return 6;
    }

    let mut normalized = match (fs.normalize(gpa, "/tmp/./kern//src/../out/file.txt")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 7,
    };
    defer normalized..&.deinit(gpa);
    if (normalized.& != "/tmp/kern/out/file.txt") {
        return 8;
    }

    let mut relative = match (fs.normalize(gpa, "alpha/./beta/../gamma")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 9,
    };
    defer relative..&.deinit(gpa);
    if (relative.& != "alpha/gamma") {
        return 10;
    }

    let mut escaped = match (fs.normalize(gpa, "../../alpha/../beta")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 11,
    };
    defer escaped..&.deinit(gpa);
    if (escaped.& != "../../beta") {
        return 12;
    }

    let mut root = match (fs.normalize(gpa, "/alpha/../..")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 13,
    };
    defer root..&.deinit(gpa);
    if (root.& != "/") {
        return 14;
    }

    let mut empty = match (fs.normalize(gpa, "")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 15,
    };
    defer empty..&.deinit(gpa);
    if (empty.& != ".") {
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
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;

    let mut renamed = match (fs.with_file_name(gpa, "/tmp/kern/main.rn", "lib.rn")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 1,
    };
    defer renamed..&.deinit(gpa);
    if (renamed.& != "/tmp/kern/lib.rn") {
        return 2;
    }

    let mut reext = match (fs.with_extension(gpa, "/tmp/kern/main.rn", "ll")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 3,
    };
    defer reext..&.deinit(gpa);
    if (reext.& != "/tmp/kern/main.ll") {
        return 4;
    }

    let mut stripped = match (fs.with_extension(gpa, "archive.tar", "")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 5,
    };
    defer stripped..&.deinit(gpa);
    if (stripped.& != "archive") {
        return 6;
    }

    let mut hidden = match (fs.with_extension(gpa, ".gitignore", "txt")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 7,
    };
    defer hidden..&.deinit(gpa);
    if (hidden.& != ".gitignore.txt") {
        return 8;
    }

    let mut rooted = match (fs.with_file_name(gpa, "/", "boot")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 9,
    };
    defer rooted..&.deinit(gpa);
    if (rooted.& != "/boot") {
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
fn runs_hosted_program_using_std_fs_path_combinators() {
    let output = build_and_run_hosted(
        r#"
use std.fs;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    let path = "/tmp/kern/archive.tar".path();

    if (!path.file_name().is_some_and([](name: &[u8]) bool {
        return name == "archive.tar";
    })) {
        return 1;
    }

    if (!path.parent().is_some_and([](dir: &[u8]) bool {
        return dir == "/tmp/kern";
    })) {
        return 2;
    }

    if (!path.extension().is_some_and([](ext: &[u8]) bool {
        return ext == "tar";
    })) {
        return 3;
    }

    if (!path.file_stem().is_some_and([](stem: &[u8]) bool {
        return stem == "archive";
    })) {
        return 4;
    }

    let mut joined = match ("/tmp/kern".path().join(gpa, "src/main.rn")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 5,
    };
    defer joined..&.deinit(gpa);
    if (joined.& != "/tmp/kern/src/main.rn") {
        return 6;
    }

    let mut normalized = match ("/tmp/./kern//src/../out/file.txt".path().normalize(gpa)) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 7,
    };
    defer normalized..&.deinit(gpa);
    if (normalized.& != "/tmp/kern/out/file.txt") {
        return 8;
    }

    let mut renamed = match (path.with_file_name(gpa, "lib.rn")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 9,
    };
    defer renamed..&.deinit(gpa);
    if (renamed.& != "/tmp/kern/lib.rn") {
        return 10;
    }

    let mut reext = match (path.with_extension(gpa, "zip")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 11,
    };
    defer reext..&.deinit(gpa);
    if (reext.& != "/tmp/kern/archive.zip") {
        return 12;
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
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;

    if (!fs.parent("C:\\kern\\src\\main.rn").is_some_and([](dir: &[u8]) bool {
        return dir == "C:\\kern\\src";
    })) {
        return 1;
    }
    if (fs.parent("C:\\").is_some()) {
        return 2;
    }
    if (!fs.file_name("C:\\kern\\main.rn").is_some_and([](name: &[u8]) bool {
        return name == "main.rn";
    })) {
        return 3;
    }

    let mut joined = match (fs.join(gpa, "C:\\kern", "src\\main.rn")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 4,
    };
    defer joined..&.deinit(gpa);
    if (joined.& != "C:\\kern\\src\\main.rn") {
        return 5;
    }

    let mut forward = match (fs.join(gpa, "C:/kern", "src/main.rn")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 6,
    };
    defer forward..&.deinit(gpa);
    if (forward.& != "C:/kern/src/main.rn") {
        return 7;
    }

    let mut rooted = match (fs.join(gpa, "C:\\kern", "D:\\other\\out.rn")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 8,
    };
    defer rooted..&.deinit(gpa);
    if (rooted.& != "D:\\other\\out.rn") {
        return 9;
    }

    let mut normalized = match (fs.normalize(gpa, "C:\\kern\\.\\src\\\\..\\out\\file.txt")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 10,
    };
    defer normalized..&.deinit(gpa);
    if (normalized.& != "C:\\kern\\out\\file.txt") {
        return 11;
    }

    let mut forward_normalized = match (fs.normalize(gpa, "C:/kern/./src//../out/file.txt")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 12,
    };
    defer forward_normalized..&.deinit(gpa);
    if (forward_normalized.& != "C:/kern/out/file.txt") {
        return 13;
    }

    let mut unc_joined = match (fs.join(gpa, "\\\\server\\share", "dir\\main.rn")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 14,
    };
    defer unc_joined..&.deinit(gpa);
    if (unc_joined.& != "\\\\server\\share\\dir\\main.rn") {
        return 15;
    }

    let mut unc = match (fs.normalize(gpa, "\\\\server\\share\\src\\\\..\\out\\file.txt")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 16,
    };
    defer unc..&.deinit(gpa);
    if (unc.& != "\\\\server\\share\\out\\file.txt") {
        return 17;
    }

    if (!fs.parent("\\\\server\\share\\out\\file.txt").is_some_and([](dir: &[u8]) bool {
        return dir == "\\\\server\\share\\out";
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
    let expected_name = "\u{4F60}\u{597D}-emoji-\u{1F642}.txt";

    let _ = fs::remove_file(&temp_file);
    let _ = fs::remove_dir_all(&temp_root);

    let output = build_and_run_hosted(&format!(
        r#"
use std.fs;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {{
    let page = Page.{{}}..&;
    let gpa = GPA.{{ backing: page }}..&;

    match ("{root_path}".path().create_dir_all(gpa)) {{
        .{{ Ok: _ }} => {{}},
        .{{ Err: _ }} => return 1,
    }}

    match ("{file_path}".path().write_all(gpa, "unicode-ok")) {{
        .{{ Ok: count }} => {{
            if (count != 10) {{
                return 2;
            }}
        }},
        .{{ Err: _ }} => return 3,
    }}

    let mut text = match ("{file_path}".path().read_to_string(gpa)) {{
        .{{ Ok: text }} => text,
        .{{ Err: _ }} => return 4,
    }};
    defer text..&.deinit(gpa);
    if (text.& != "unicode-ok") {{
        return 5;
    }}

    let mut hits = usize.{{0}};
    let visited = match ("{root_path}".path().read_dir(gpa, [
        hits = hits..&
    ](entry: fs.DirEntry) bool {{
        if (entry.name == "{expected_name}") {{
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
        file_path = file_path,
        expected_name = expected_name
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
