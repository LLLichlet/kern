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

fn compile_source_with_std(source: &str) -> std::process::Output {
    let source_path = unique_temp_path("kernc_std_coll_compile", "kr");
    let object_path = unique_temp_path("kernc_std_coll_compile", "o");

    fs::write(&source_path, source).unwrap();

    let source_arg = source_path.to_string_lossy().into_owned();
    let object_arg = object_path.to_string_lossy().into_owned();
    let args = vec!["-c", "--use-std", source_arg.as_str(), "-o", object_arg.as_str()];
    let output = run_kernc(&args);

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&object_path);
    output
}

fn build_and_run_hosted(source: &str) -> std::process::Output {
    let source_path = unique_temp_path("kernc_std_coll", "kr");
    let exe_ext = if cfg!(windows) { "exe" } else { "out" };
    let executable_path = unique_temp_path("kernc_std_coll", exe_ext);

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

#[test]
fn runs_hosted_program_using_layout_based_allocator_api() {
    let output = build_and_run_hosted(
        r#"
use std.mem.{layout_of, array_layout_of};
use std.mem.alloc.PageAllocator;

extern fn main() i32 {
    let page = PageAllocator.{}..&;

    let item_layout = layout_of[u64]();
    if (item_layout.size != 8 or item_layout.align != 8) {
        return 1;
    }

    let array_layout = match (array_layout_of[u32](6)) {
        .Some: layout => layout,
        .None => return 2,
    };
    if (array_layout.size != 24 or array_layout.align != 4) {
        return 3;
    }

    let ptr = match (page.alloc(array_layout)) {
        .Some: raw => raw,
        .None => return 4,
    };
    page.free(ptr, array_layout);
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
fn runs_hosted_program_using_std_coll_tree_map() {
    let output = build_and_run_hosted(
        r#"
use std.coll.TreeMap;
use std.mem.alloc.{PageAllocator, GPAllocator};

extern fn main() i32 {
    let page = PageAllocator.{}..&;
    let gpa = GPAllocator.{ backing: page }..&;
    let map = TreeMap[i32, i32].{}..&;
    defer map.deinit(gpa);

    let mut i = 0;
    for (; i < 32; i += 1) {
        let key = i as i32;
        if (!map.insert(gpa, key, key * 2)) {
            return 1;
        }
    }

    if (!map.insert(gpa, 7, 99)) {
        return 2;
    }

    if (!map.get(7).is_some_and(.[](value: i32) bool { return value == 99; })) {
        return 3;
    }
    if (!map.get(31).is_some_and(.[](value: i32) bool { return value == 62; })) {
        return 4;
    }

    if (!map.contains(15) or map.contains(1000)) {
        return 5;
    }
    if (map.len != 32) {
        return 6;
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
fn runs_hosted_program_using_custom_ord_tree_map_key() {
    let output = build_and_run_hosted(
        r#"
use std.coll.TreeMap;
use std.cmp.{Eq, Ordering, Comparable, Ord, LESS, EQUAL, GREATER};
use std.mem.alloc.{PageAllocator, GPAllocator};

type Key = struct {
    major: i32,
    minor: i32,
};

impl *Key : Eq[Key] {
    pub fn eq(other: Key) bool {
        return self.major == other.major and self.minor == other.minor;
    }
}

impl *Key : Comparable[Key] {
    pub fn cmp(other: Key) Ordering {
        if (self.major < other.major) return LESS;
        if (self.major > other.major) return GREATER;
        if (self.minor < other.minor) return LESS;
        if (self.minor > other.minor) return GREATER;
        return EQUAL;
    }
}

impl *Key : Ord[Key] {}

extern fn main() i32 {
    let page = PageAllocator.{}..&;
    let gpa = GPAllocator.{ backing: page }..&;
    let map = TreeMap[Key, i32].{}..&;
    defer map.deinit(gpa);

    if (!map.insert(gpa, Key.{ major: 1, minor: 0 }, 10)) {
        return 1;
    }
    if (!map.insert(gpa, Key.{ major: 0, minor: 8 }, 8)) {
        return 2;
    }
    if (!map.insert(gpa, Key.{ major: 1, minor: 2 }, 12)) {
        return 3;
    }
    if (!map.insert(gpa, Key.{ major: 1, minor: 0 }, 99)) {
        return 4;
    }

    if (!map.get(Key.{ major: 1, minor: 0 }).is_some_and(.[](value: i32) bool { return value == 99; })) {
        return 5;
    }
    if (!map.get(Key.{ major: 0, minor: 8 }).is_some_and(.[](value: i32) bool { return value == 8; })) {
        return 6;
    }
    if (map.contains(Key.{ major: 2, minor: 0 })) {
        return 7;
    }
    if (map.len != 3) {
        return 8;
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
fn rejects_tree_map_key_without_ord() {
    let output = compile_source_with_std(
        r#"
use std.coll.TreeMap;

type Key = struct {
    raw: i32,
};

extern fn main(args: [][]u8) i32 {
    let map = TreeMap[Key, i32].{}..&;
    let _ = map;
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
        stderr.contains("Ord[Key]") || stderr.contains("TreeMap[Key, i32]"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn runs_hosted_program_using_std_coll_map() {
    let output = build_and_run_hosted(
        r#"
use std.coll.Map;
use std.mem.alloc.{PageAllocator, GPAllocator};

extern fn main() i32 {
    let page = PageAllocator.{}..&;
    let gpa = GPAllocator.{ backing: page }..&;
    let map = Map[i32, i32].{}..&;
    defer map.deinit(gpa);

    let mut i = 0;
    for (; i < 128; i += 1) {
        let key = i as i32;
        if (!map.insert(gpa, key, key * 3)) {
            return 1;
        }
    }

    if (!map.insert(gpa, 7, 99)) {
        return 2;
    }
    if (map.len != 128) {
        return 3;
    }

    let value_ptr = match (map.get_ptr(7)) {
        .Some: ptr => ptr,
        .None => return 4,
    };
    value_ptr.* = 123;

    if (!map.get(7).is_some_and(.[](value: i32) bool { return value == 123; })) {
        return 5;
    }

    let removed = match (map.remove(7)) {
        .Some: value => value,
        .None => return 6,
    };
    if (removed != 123) {
        return 7;
    }

    if (map.contains(7)) {
        return 8;
    }

    if (!map.insert(gpa, 7, 777)) {
        return 9;
    }
    if (!map.get(7).is_some_and(.[](value: i32) bool { return value == 777; })) {
        return 10;
    }
    if (!map.get(100).is_some_and(.[](value: i32) bool { return value == 300; })) {
        return 11;
    }

    let before_compact = map.capacity;
    if (!map.compact(gpa)) {
        return 12;
    }
    if (map.capacity != before_compact) {
        return 13;
    }

    let missing = map.remove(999);
    if (missing.is_some()) {
        return 14;
    }

    map.clear();
    if (!map.is_empty() or map.len != 0) {
        return 15;
    }

    if (!map.insert(gpa, 42, 4242)) {
        return 16;
    }
    if (!map.get(42).is_some_and(.[](value: i32) bool { return value == 4242; })) {
        return 17;
    }

    if (!map.shrink_to_fit(gpa)) {
        return 18;
    }
    if (map.capacity != 8) {
        return 19;
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
fn runs_hosted_program_using_custom_hash_map_key_with_collisions() {
    let output = build_and_run_hosted(
        r#"
use std.coll.Map;
use std.cmp.Eq;
use std.hash.Hash;
use std.mem.alloc.{PageAllocator, GPAllocator};

type Key = struct {
    group: i32,
    id: i32,
};

impl *Key : Eq[Key] {
    pub fn eq(other: Key) bool {
        return self.group == other.group and self.id == other.id;
    }
}

impl *Key : Hash[Key] {
    pub fn hash() u64 {
        return self.group as u64;
    }
}

extern fn main() i32 {
    let page = PageAllocator.{}..&;
    let gpa = GPAllocator.{ backing: page }..&;
    let map = Map[Key, i32].{}..&;
    defer map.deinit(gpa);

    if (!map.insert(gpa, Key.{ group: 1, id: 10 }, 10)) {
        return 1;
    }
    if (!map.insert(gpa, Key.{ group: 1, id: 11 }, 11)) {
        return 2;
    }
    if (!map.insert(gpa, Key.{ group: 1, id: 12 }, 12)) {
        return 3;
    }
    if (!map.insert(gpa, Key.{ group: 2, id: 99 }, 99)) {
        return 4;
    }
    if (map.len != 4) {
        return 5;
    }

    if (!map.get(Key.{ group: 1, id: 11 }).is_some_and(.[](value: i32) bool { return value == 11; })) {
        return 6;
    }

    let removed = match (map.remove(Key.{ group: 1, id: 10 })) {
        .Some: value => value,
        .None => return 7,
    };
    if (removed != 10) {
        return 8;
    }

    if (!map.insert(gpa, Key.{ group: 1, id: 13 }, 13)) {
        return 9;
    }
    if (!map.get(Key.{ group: 1, id: 12 }).is_some_and(.[](value: i32) bool { return value == 12; })) {
        return 10;
    }
    if (!map.get(Key.{ group: 1, id: 13 }).is_some_and(.[](value: i32) bool { return value == 13; })) {
        return 11;
    }
    if (map.contains(Key.{ group: 1, id: 10 })) {
        return 12;
    }

    let cap_before = map.capacity;
    if (!map.reserve(gpa, 64)) {
        return 13;
    }
    if (map.capacity < cap_before) {
        return 14;
    }
    if (!map.get(Key.{ group: 1, id: 11 }).is_some_and(.[](value: i32) bool { return value == 11; })) {
        return 15;
    }

    let _ = map.remove(Key.{ group: 1, id: 11 });
    let _ = map.remove(Key.{ group: 1, id: 12 });
    if (!map.compact(gpa)) {
        return 16;
    }
    if (!map.insert(gpa, Key.{ group: 1, id: 14 }, 14)) {
        return 17;
    }
    if (!map.get(Key.{ group: 1, id: 14 }).is_some_and(.[](value: i32) bool { return value == 14; })) {
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
fn rejects_map_key_without_eq_and_hash() {
    let output = compile_source_with_std(
        r#"
use std.coll.Map;

type Key = struct {
    raw: i32,
};

extern fn main(args: [][]u8) i32 {
    let map = Map[Key, i32].{}..&;
    let _ = map;
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
        stderr.contains("Eq[Key]") || stderr.contains("Hash[Key]") || stderr.contains("Map[Key, i32]"),
        "unexpected stderr:\n{}",
        stderr
    );
}
