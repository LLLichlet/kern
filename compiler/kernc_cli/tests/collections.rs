mod support;

use support::{build_and_run, compile_source_with_args};

fn compile_source_with_std(source: &str) -> std::process::Output {
    compile_source_with_args("kernc_std_coll_compile", source, &["--use-std"])
}

fn build_and_run_hosted(source: &str) -> std::process::Output {
    build_and_run(
        "kernc_std_coll",
        source,
        &["--use-std", "--link-profile", "hosted"],
    )
}

#[test]
fn runs_hosted_program_using_layout_based_allocator_api() {
    let output = build_and_run_hosted(
        r#"
use std.mem.{layout_of, array_layout_of};
use std.mem.alloc.Page;

extern fn main() i32 {
    let page = Page.{}..&;

    let item_layout = layout_of[u64]();
    if (item_layout.size != 8 or item_layout.align != 8) {
        return 1;
    }

    let array_layout = match (array_layout_of[u32](6)) {
        .{ Some: layout } => layout,
        .None => return 2,
    };
    if (array_layout.size != 24 or array_layout.align != 4) {
        return 3;
    }

    let ptr = match (page.alloc(array_layout)) {
        .{ Some: raw } => raw,
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
fn runs_hosted_program_using_std_coll_tree() {
    let output = build_and_run_hosted(
        r#"
use std.coll.Tree;
use std.mem.alloc.{Page, GPA};

extern fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    let map = Tree[i32, i32].{}..&;
    defer map.deinit(gpa);
    let mut lazy_calls = i32.{0};

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

    let value_ptr = match (map.get_ptr(7)) {
        .{ Some: ptr } => ptr,
        .None => return 5,
    };
    value_ptr.* = 123;
    if (!map.get(7).is_some_and(.[](value: i32) bool { return value == 123; })) {
        return 6;
    }

    let existing = match (map.get_or_insert_with(gpa, 7, .[calls = lazy_calls..&]() i32 {
        calls.* += 1;
        return 700;
    })) {
        .{ Some: ptr } => ptr,
        .None => return 7,
    };
    if (existing.* != 123) {
        return 8;
    }
    if (lazy_calls != 0) {
        return 9;
    }

    let inserted = match (map.get_or_insert(gpa, 100, 500)) {
        .{ Some: ptr } => ptr,
        .None => return 10,
    };
    if (inserted.* != 500) {
        return 11;
    }

    let lazy_inserted = match (map.get_or_insert_with(gpa, 200, .[calls = lazy_calls..&]() i32 {
        calls.* += 1;
        return 900;
    })) {
        .{ Some: ptr } => ptr,
        .None => return 12,
    };
    if (lazy_inserted.* != 900) {
        return 13;
    }
    if (lazy_calls != 1) {
        return 14;
    }

    if (!map.contains(15) or map.contains(1000)) {
        return 15;
    }
    if (map.len != 34) {
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
fn runs_hosted_program_using_custom_ord_tree_key() {
    let output = build_and_run_hosted(
        r#"
use std.coll.Tree;
use std.cmp.{Eq, Ordering, Comparable, Ord, LESS, EQUAL, GREATER};
use std.mem.alloc.{Page, GPA};

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
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    let map = Tree[Key, i32].{}..&;
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
fn rejects_tree_key_without_ord() {
    let output = compile_source_with_std(
        r#"
use std.coll.Tree;

type Key = struct {
    raw: i32,
};

extern fn main(args: [][]u8) i32 {
    let map = Tree[Key, i32].{}..&;
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
        stderr.contains("Ord[Key]") || stderr.contains("Tree[Key, i32]"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn runs_hosted_program_using_std_coll_map() {
    let output = build_and_run_hosted(
        r#"
use std.coll.Map;
use std.mem.alloc.{Page, GPA};

extern fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
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
        .{ Some: ptr } => ptr,
        .None => return 4,
    };
    value_ptr.* = 123;

    if (!map.get(7).is_some_and(.[](value: i32) bool { return value == 123; })) {
        return 5;
    }

    let removed = match (map.remove(7)) {
        .{ Some: value } => value,
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
use std.mem.alloc.{Page, GPA};

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
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
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
        .{ Some: value } => value,
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
fn runs_hosted_program_using_string_traits_for_ordering_and_hashing() {
    let output = build_and_run_hosted(
        r#"
use std.coll.String;
use std.cmp.{LESS, EQUAL, GREATER};
use std.hash.hash_of;
use std.mem.alloc.{Page, GPA};

extern fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;

    let alpha = String.{}..&;
    defer alpha.deinit(gpa);
    if (!alpha.clone_from(gpa, "alpha")) {
        return 1;
    }

    let alpha_copy = String.{}..&;
    defer alpha_copy.deinit(gpa);
    if (!alpha_copy.clone_from(gpa, "alpha")) {
        return 2;
    }

    let beta = String.{}..&;
    defer beta.deinit(gpa);
    if (!beta.clone_from(gpa, "beta")) {
        return 3;
    }

    if (alpha.cmp(alpha_copy.*) != EQUAL) {
        return 4;
    }
    if (alpha.cmp(beta.*) == EQUAL) {
        return 5;
    }
    if (alpha.cmp(beta.*) != LESS) {
        return 6;
    }
    if (beta.cmp(alpha.*) != GREATER) {
        return 7;
    }

    let alpha_hash = hash_of(alpha.*);
    let alpha_copy_hash = hash_of(alpha_copy.*);
    let beta_hash = hash_of(beta.*);

    if (alpha_hash != alpha_copy_hash) {
        return 8;
    }
    if (alpha_hash == beta_hash) {
        return 9;
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
fn runs_hosted_program_using_map_get_or_insert_apis() {
    let output = build_and_run_hosted(
        r#"
use std.coll.Map;
use std.mem.alloc.{Page, GPA};

extern fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    let map = Map[i32, i32].{}..&;
    defer map.deinit(gpa);
    let mut lazy_calls = i32.{0};

    let mut i = 0;
    for (; i < 6; i += 1) {
        let key = i as i32;
        if (!map.insert(gpa, key, key + 10)) {
            return 1;
        }
    }

    if (map.capacity != 8 or map.len != 6) {
        return 2;
    }

    if (!map.insert(gpa, 3, 99)) {
        return 3;
    }
    if (map.capacity != 8 or map.len != 6) {
        return 4;
    }

    let existing = match (map.get_or_insert(gpa, 3, 111)) {
        .{ Some: ptr } => ptr,
        .None => return 5,
    };
    if (existing.* != 99) {
        return 6;
    }
    existing.* = 123;
    if (!map.get(3).is_some_and(.[](value: i32) bool { return value == 123; })) {
        return 7;
    }
    if (map.capacity != 8 or map.len != 6) {
        return 8;
    }

    let inserted = match (map.get_or_insert(gpa, 100, 500)) {
        .{ Some: ptr } => ptr,
        .None => return 9,
    };
    if (inserted.* != 500) {
        return 10;
    }
    if (map.len != 7 or map.capacity != 16) {
        return 11;
    }

    let lazy_existing = match (map.get_or_insert_with(gpa, 100, .[calls = lazy_calls..&]() i32 {
        calls.* += 1;
        return 700;
    })) {
        .{ Some: ptr } => ptr,
        .None => return 12,
    };
    if (lazy_existing.* != 500) {
        return 13;
    }
    if (lazy_calls != 0) {
        return 14;
    }

    let lazy_inserted = match (map.get_or_insert_with(gpa, 200, .[calls = lazy_calls..&]() i32 {
        calls.* += 1;
        return 900;
    })) {
        .{ Some: ptr } => ptr,
        .None => return 15,
    };
    if (lazy_inserted.* != 900) {
        return 16;
    }
    if (lazy_calls != 1) {
        return 17;
    }
    if (map.len != 8) {
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
fn runs_hosted_program_using_map_traversal_and_filter_helpers() {
    let output = build_and_run_hosted(
        r#"
use std.coll.Map;
use std.mem.alloc.{Page, GPA};

extern fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    let map = Map[i32, i32].{}..&;
    defer map.deinit(gpa);

    if (!map.insert(gpa, 3, 30)) return 1;
    if (!map.insert(gpa, 1, 10)) return 2;
    if (!map.insert(gpa, 4, 40)) return 3;
    if (!map.insert(gpa, 2, 20)) return 4;

    let mut key_sum = i32.{0};
    let mut value_sum = i32.{0};
    map.for_each(.[key_sum = key_sum..&, value_sum = value_sum..&](key: i32, value: i32) void {
        key_sum.* += key;
        value_sum.* += value;
    });
    if (key_sum != 10 or value_sum != 100) {
        return 5;
    }

    let folded = map.fold(i32.{0}, .[](accum: i32, key: i32, value: i32) i32 {
        return accum + key + value;
    });
    if (folded != 110) {
        return 6;
    }

    map.for_each_mut(.[](key: i32, value: *mut i32) void {
        value.* += key;
    });
    if (!map.get(1).is_some_and(.[](value: i32) bool { return value == 11; })) return 7;
    if (!map.get(2).is_some_and(.[](value: i32) bool { return value == 22; })) return 8;
    if (!map.get(3).is_some_and(.[](value: i32) bool { return value == 33; })) return 9;
    if (!map.get(4).is_some_and(.[](value: i32) bool { return value == 44; })) return 10;

    map.retain(.[](key: i32, _: i32) bool {
        return key % 2 == 0;
    });
    if (map.len != 2) {
        return 11;
    }
    if (map.contains(1) or map.contains(3)) {
        return 12;
    }
    if (!map.contains(2) or !map.contains(4)) {
        return 13;
    }

    if (!map.compact(gpa)) {
        return 14;
    }
    let retained = map.fold(i32.{0}, .[](accum: i32, key: i32, value: i32) i32 {
        return accum + key * value;
    });
    if (retained != 220) {
        return 15;
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
fn runs_hosted_program_using_map_predicate_algorithms() {
    let output = build_and_run_hosted(
        r#"
use std.{Option};
use std.coll.Map;
use std.mem.alloc.{Page, GPA};

extern fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    let map = Map[i32, i32].{}..&;
    defer map.deinit(gpa);

    if (!map.insert(gpa, 1, 10)) return 1;
    if (!map.insert(gpa, 2, 20)) return 2;
    if (!map.insert(gpa, 3, 30)) return 3;
    if (!map.insert(gpa, 4, 40)) return 4;

    if (!map.any(.[](key: i32, value: i32) bool {
        return key == 3 and value == 30;
    })) {
        return 5;
    }

    if (map.any(.[](key: i32, _: i32) bool {
        return key == 9;
    })) {
        return 6;
    }

    if (!map.all(.[](_: i32, value: i32) bool {
        return value % 10 == 0;
    })) {
        return 7;
    }

    let even_count = map.count(.[](key: i32, _: i32) bool {
        return key % 2 == 0;
    });
    if (even_count != 2) {
        return 8;
    }

    let found = match (map.find_map(.[](key: i32, value: i32) Option[i32] {
        if (key == 4) {
            return .{ Some: value + 4 };
        }
        return .{ None };
    })) {
        .{ Some: value } => value,
        .None => return 9,
    };
    if (found != 44) {
        return 10;
    }

    map.retain_mut(.[](key: i32, value: *mut i32) bool {
        value.* += key;
        return key >= 2;
    });
    if (map.len != 3) {
        return 11;
    }
    if (map.contains(1)) {
        return 12;
    }
    if (!map.get(2).is_some_and(.[](value: i32) bool { return value == 22; })) return 13;
    if (!map.get(3).is_some_and(.[](value: i32) bool { return value == 33; })) return 14;
    if (!map.get(4).is_some_and(.[](value: i32) bool { return value == 44; })) return 15;

    let removed = match (map.remove_where(.[](key: i32, value: i32) bool {
        return key == 3 and value == 33;
    })) {
        .{ Some: value } => value,
        .None => return 16,
    };
    if (removed != 33) {
        return 17;
    }
    if (map.contains(3) or map.len != 2) {
        return 18;
    }

    if (map.remove_where(.[](key: i32, _: i32) bool {
        return key == 99;
    }).is_some()) {
        return 19;
    }

    let remaining = map.fold(i32.{0}, .[](accum: i32, key: i32, value: i32) i32 {
        return accum + key + value;
    });
    if (remaining != 72) {
        return 20;
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
fn runs_hosted_program_using_map_list_bridge_helpers() {
    let output = build_and_run_hosted(
        r#"
use std.coll.{Map, List};
use std.mem.alloc.{Page, GPA};

extern fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    let map = Map[i32, i32].{}..&;
    defer map.deinit(gpa);
    let keys = List[i32].{}..&;
    defer keys.deinit(gpa);
    let values = List[i32].{}..&;
    defer values.deinit(gpa);

    if (!keys.push(gpa, 1000)) return 1;
    if (!values.push(gpa, 2000)) return 2;

    if (!map.insert(gpa, 1, 10)) return 3;
    if (!map.insert(gpa, 2, 20)) return 4;
    if (!map.insert(gpa, 3, 30)) return 5;
    if (!map.insert(gpa, 4, 40)) return 6;

    let removed = match (map.remove(2)) {
        .{ Some: value } => value,
        .None => return 7,
    };
    if (removed != 20) {
        return 8;
    }

    if (!map.insert(gpa, 4, 400)) {
        return 9;
    }

    if (!map.append_keys(gpa, keys)) {
        return 10;
    }
    if (!map.append_values(gpa, values)) {
        return 11;
    }

    if (keys.len != 4 or values.len != 4) {
        return 12;
    }
    if (!keys.first().is_some_and(.[](key: i32) bool { return key == 1000; })) {
        return 13;
    }
    if (!values.first().is_some_and(.[](value: i32) bool { return value == 2000; })) {
        return 14;
    }

    let key_sum = keys.fold(i32.{0}, .[](accum: i32, key: i32) i32 {
        return accum + key;
    });
    if (key_sum != 1008) {
        return 15;
    }

    let value_sum = values.fold(i32.{0}, .[](accum: i32, value: i32) i32 {
        return accum + value;
    });
    if (value_sum != 2440) {
        return 16;
    }

    let appended_keys = keys.count(.[](key: i32) bool {
        return key >= 1 and key <= 4;
    });
    if (appended_keys != 3) {
        return 17;
    }

    let appended_values = values.count(.[](value: i32) bool {
        return value == 10 or value == 30 or value == 400;
    });
    if (appended_values != 3) {
        return 18;
    }

    let mut owned_keys = match (map.keys(gpa)) {
        .{ Some: out } => out,
        .None => return 19,
    };
    defer owned_keys..&.deinit(gpa);
    let mut owned_values = match (map.values(gpa)) {
        .{ Some: out } => out,
        .None => return 20,
    };
    defer owned_values..&.deinit(gpa);

    if (owned_keys.len != 3 or owned_values.len != 3) {
        return 21;
    }

    let owned_key_sum = owned_keys.&.fold(i32.{0}, .[](accum: i32, key: i32) i32 {
        return accum + key;
    });
    if (owned_key_sum != 8) {
        return 22;
    }

    let owned_value_sum = owned_values.&.fold(i32.{0}, .[](accum: i32, value: i32) i32 {
        return accum + value;
    });
    if (owned_value_sum != 440) {
        return 23;
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
fn runs_hosted_program_using_tree_ordered_traversal_helpers() {
    let output = build_and_run_hosted(
        r#"
use std.coll.{Tree, String};
use std.mem.alloc.{Page, GPA};

extern fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    let map = Tree[i32, i32].{}..&;
    defer map.deinit(gpa);

    if (!map.insert(gpa, 3, 30)) return 1;
    if (!map.insert(gpa, 1, 10)) return 2;
    if (!map.insert(gpa, 4, 40)) return 3;
    if (!map.insert(gpa, 2, 20)) return 4;

    let order = String.{}..&;
    defer order.deinit(gpa);
    map.for_each(.[order, gpa](key: i32, _: i32) void {
        let _ = order.push_char(gpa, (key as u8) + b'0');
    });
    if (!order.eq("1234")) {
        return 5;
    }

    let weighted = map.fold(i32.{0}, .[](accum: i32, key: i32, value: i32) i32 {
        return accum + key * value;
    });
    if (weighted != 300) {
        return 6;
    }

    map.for_each_mut(.[](key: i32, value: *mut i32) void {
        value.* += key;
    });
    if (!map.get(1).is_some_and(.[](value: i32) bool { return value == 11; })) return 7;
    if (!map.get(2).is_some_and(.[](value: i32) bool { return value == 22; })) return 8;
    if (!map.get(3).is_some_and(.[](value: i32) bool { return value == 33; })) return 9;
    if (!map.get(4).is_some_and(.[](value: i32) bool { return value == 44; })) return 10;

    let updated = map.fold(i32.{0}, .[](accum: i32, key: i32, value: i32) i32 {
        return accum + key + value;
    });
    if (updated != 120) {
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
fn runs_hosted_program_using_tree_boundary_queries() {
    let output = build_and_run_hosted(
        r#"
use std.coll.Tree;
use std.mem.alloc.{Page, GPA};

extern fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    let map = Tree[i32, i32].{}..&;
    defer map.deinit(gpa);

    if (!map.insert(gpa, 10, 100)) return 1;
    if (!map.insert(gpa, 30, 300)) return 2;
    if (!map.insert(gpa, 20, 200)) return 3;
    if (!map.insert(gpa, 40, 400)) return 4;

    if (!map.first_key().is_some_and(.[](key: i32) bool { return key == 10; })) return 5;
    if (!map.first().is_some_and(.[](value: i32) bool { return value == 100; })) return 6;
    if (!map.last_key().is_some_and(.[](key: i32) bool { return key == 40; })) return 7;
    if (!map.last().is_some_and(.[](value: i32) bool { return value == 400; })) return 8;

    if (!map.ceil_key(5).is_some_and(.[](key: i32) bool { return key == 10; })) return 9;
    if (!map.ceil(10).is_some_and(.[](value: i32) bool { return value == 100; })) return 10;
    if (!map.ceil_key(21).is_some_and(.[](key: i32) bool { return key == 30; })) return 11;
    if (!map.ceil(39).is_some_and(.[](value: i32) bool { return value == 400; })) return 12;
    if (map.ceil_key(41).is_some()) return 13;

    if (map.floor_key(9).is_some()) return 14;
    if (!map.floor_key(10).is_some_and(.[](key: i32) bool { return key == 10; })) return 15;
    if (!map.floor(29).is_some_and(.[](value: i32) bool { return value == 200; })) return 16;
    if (!map.floor_key(40).is_some_and(.[](key: i32) bool { return key == 40; })) return 17;

    let first = match (map.first_ptr()) {
        .{ Some: ptr } => ptr,
        .None => return 18,
    };
    first.* += 1;
    let last = match (map.last_ptr()) {
        .{ Some: ptr } => ptr,
        .None => return 19,
    };
    last.* += 2;
    let ceil_mid = match (map.ceil_ptr(21)) {
        .{ Some: ptr } => ptr,
        .None => return 20,
    };
    ceil_mid.* += 3;
    let floor_mid = match (map.floor_ptr(29)) {
        .{ Some: ptr } => ptr,
        .None => return 21,
    };
    floor_mid.* += 4;

    if (!map.get(10).is_some_and(.[](value: i32) bool { return value == 101; })) return 22;
    if (!map.get(20).is_some_and(.[](value: i32) bool { return value == 204; })) return 23;
    if (!map.get(30).is_some_and(.[](value: i32) bool { return value == 303; })) return 24;
    if (!map.get(40).is_some_and(.[](value: i32) bool { return value == 402; })) return 25;

    let empty = Tree[i32, i32].{}..&;
    defer empty.deinit(gpa);
    if (empty.first().is_some()) return 26;
    if (empty.last().is_some()) return 27;
    if (empty.ceil(1).is_some()) return 28;
    if (empty.floor(1).is_some()) return 29;

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
fn runs_hosted_program_using_tree_remove() {
    let output = build_and_run_hosted(
        r#"
use std.coll.{Tree, String};
use std.mem.alloc.{Page, GPA};

extern fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    let map = Tree[i32, i32].{}..&;
    defer map.deinit(gpa);

    let mut i = i32.{1};
    for (; i <= 40; i += 1) {
        if (!map.insert(gpa, i, i * 10)) {
            return 1;
        }
    }

    let removed_mid = match (map.remove(gpa, 20)) {
        .{ Some: value } => value,
        .None => return 2,
    };
    if (removed_mid != 200 or map.contains(20)) {
        return 3;
    }

    let removed_first = match (map.remove(gpa, 1)) {
        .{ Some: value } => value,
        .None => return 4,
    };
    if (removed_first != 10 or map.contains(1)) {
        return 5;
    }

    let removed_last = match (map.remove(gpa, 40)) {
        .{ Some: value } => value,
        .None => return 6,
    };
    if (removed_last != 400 or map.contains(40)) {
        return 7;
    }

    if (map.remove(gpa, 99).is_some()) {
        return 8;
    }
    if (map.len != 37) {
        return 9;
    }

    if (!map.first_key().is_some_and(.[](key: i32) bool { return key == 2; })) return 10;
    if (!map.last_key().is_some_and(.[](key: i32) bool { return key == 39; })) return 11;
    if (!map.ceil_key(20).is_some_and(.[](key: i32) bool { return key == 21; })) return 12;
    if (!map.floor_key(20).is_some_and(.[](key: i32) bool { return key == 19; })) return 13;

    let mut count = i32.{0};
    let mut ordered = String.{}..&;
    defer ordered.deinit(gpa);
    map.for_each(.[count = count..&, ordered, gpa](key: i32, _: i32) void {
        count.* += 1;
        if (key >= 2 and key <= 9) {
            let _ = ordered.push_char(gpa, (key as u8) + b'0');
        }
    });
    if (count != 37 or !ordered.eq("23456789")) {
        return 14;
    }

    let small = Tree[i32, i32].{}..&;
    defer small.deinit(gpa);
    if (!small.insert(gpa, 2, 20)) return 15;
    if (!small.insert(gpa, 1, 10)) return 16;
    if (!small.insert(gpa, 3, 30)) return 17;
    if (!small.remove(gpa, 2).is_some_and(.[](value: i32) bool { return value == 20; })) return 18;
    if (!small.remove(gpa, 1).is_some_and(.[](value: i32) bool { return value == 10; })) return 19;
    if (!small.remove(gpa, 3).is_some_and(.[](value: i32) bool { return value == 30; })) return 20;
    if (!small.is_empty()) return 21;
    if (small.first().is_some()) return 22;
    if (small.last().is_some()) return 23;

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
fn runs_hosted_program_using_list_slice_and_string_algorithms() {
    let output = build_and_run_hosted(
        r#"
use std.{Option};
use std.coll.{List, String, find_byte, rfind_byte, trim_ascii_start, trim_ascii_end, trim_ascii};
use std.cmp.{LESS, GREATER, EQUAL};
use std.mem.alloc.{Page, GPA};

extern fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;

    let list = List[i32].{}..&;
    defer list.deinit(gpa);

    if (!list.reserve(gpa, 6)) {
        return 1;
    }
    if (list.capacity() < 6) {
        return 2;
    }
    if (!list.push(gpa, 1) or !list.push(gpa, 2) or !list.push(gpa, 3)) {
        return 3;
    }
    if (!list.insert(gpa, 1, 9)) {
        return 4;
    }

    let removed = match (list.remove(2)) {
        .{ Some: value } => value,
        .None => return 5,
    };
    if (removed != 2) {
        return 6;
    }

    let prefix = list.as_slice();
    if (!list.append_slice(gpa, prefix)) {
        return 7;
    }

    let data = list.as_slice();
    if (!data.eq([6]i32.{1, 9, 3, 1, 9, 3})) {
        return 8;
    }
    if (!data.starts_with([3]i32.{1, 9, 3})) {
        return 9;
    }
    if (!data.ends_with([3]i32.{1, 9, 3})) {
        return 10;
    }
    if (!data.contains([2]i32.{9, 3})) {
        return 11;
    }

    let found = match (data.find([2]i32.{9, 3})) {
        .{ Some: index } => index,
        .None => return 12,
    };
    if (found != 1) {
        return 13;
    }
    if (!data.first().is_some_and(.[](value: i32) bool { return value == 1; })) {
        return 14;
    }
    if (!data.last().is_some_and(.[](value: i32) bool { return value == 3; })) {
        return 15;
    }

    let view = list.as_mut_slice();
    view.[1] = 8;
    list.truncate(4);
    if (!list.as_slice().eq([4]i32.{1, 8, 3, 1})) {
        return 16;
    }
    if (!list.first().is_some_and(.[](value: i32) bool { return value == 1; })) {
        return 17;
    }
    if (!list.last().is_some_and(.[](value: i32) bool { return value == 1; })) {
        return 18;
    }
    if (!list.contains([2]i32.{8, 3})) {
        return 19;
    }
    if (list.lex_cmp([4]i32.{1, 8, 3, 2}) != LESS) {
        return 20;
    }
    let first_big = match (list.position(.[](value: i32) bool { return value > 2; })) {
        .{ Some: index } => index,
        .None => return 21,
    };
    if (first_big != 1) {
        return 22;
    }
    let last_big = match (list.rposition(.[](value: i32) bool { return value > 2; })) {
        .{ Some: index } => index,
        .None => return 23,
    };
    if (last_big != 2) {
        return 24;
    }
    if (!list.any(.[](value: i32) bool { return value == 8; })) {
        return 25;
    }
    if (list.all(.[](value: i32) bool { return value < 8; })) {
        return 26;
    }
    let stripped_prefix = match (list.strip_prefix([2]i32.{1, 8})) {
        .{ Some: tail } => tail,
        .None => return 27,
    };
    if (!stripped_prefix.eq([2]i32.{3, 1})) {
        return 28;
    }
    let stripped_suffix = match (list.strip_suffix([2]i32.{3, 1})) {
        .{ Some: head } => head,
        .None => return 29,
    };
    if (!stripped_suffix.eq([2]i32.{1, 8})) {
        return 30;
    }

    list.reverse();
    if (!list.as_slice().eq([4]i32.{1, 3, 8, 1})) {
        return 31;
    }

    let mut kept = i32.{0};
    list.retain(.[counter = kept..&](value: i32) bool {
        counter.* += 1;
        return value >= 3;
    });
    if (kept != 4) {
        return 32;
    }
    if (!list.as_slice().eq([2]i32.{3, 8})) {
        return 33;
    }
    if (!list.shrink_to_fit(gpa)) {
        return 34;
    }
    if (list.capacity() != list.len) {
        return 35;
    }
    if (list.count(.[](value: i32) bool { return value >= 3; }) != 2) {
        return 36;
    }
    let mapped_big = match (list.find_map(.[](value: i32) Option[i32] {
        if (value > 3) {
            return .{ Some: value * 10 };
        }
        return .{ None };
    })) {
        .{ Some: value } => value,
        .None => return 37,
    };
    if (mapped_big != 80) {
        return 38;
    }

    let sorted = [6]i32.{1, 3, 3, 5, 8, 9};
    let sorted_view = sorted.[0 .. 6];
    let split = sorted_view.partition_point(.[](value: i32) bool {
        return value < 5;
    });
    if (split != 3) {
        return 39;
    }
    let found_eight = match (sorted_view.binary_search(8)) {
        .{ Some: index } => index,
        .None => return 40,
    };
    if (found_eight != 4) {
        return 41;
    }
    if (sorted_view.binary_search(7).is_some()) {
        return 42;
    }

    let text = String.{}..&;
    defer text.deinit(gpa);
    if (!text.reserve(gpa, 16)) {
        return 43;
    }
    if (text.capacity() < 16) {
        return 44;
    }
    if (!text.push_str(gpa, "kern") or !text.push_char(gpa, b'-') or !text.push_str(gpa, "lang")) {
        return 45;
    }
    if (!text.starts_with("kern") or !text.ends_with("lang")) {
        return 46;
    }
    if (!text.contains("-la")) {
        return 47;
    }
    let lang_index = match (text.find("lang")) {
        .{ Some: index } => index,
        .None => return 48,
    };
    if (lang_index != 5) {
        return 49;
    }
    if (!text.contains_byte(b'-')) {
        return 50;
    }
    let dash_index = match (text.find_byte(b'-')) {
        .{ Some: index } => index,
        .None => return 51,
    };
    if (dash_index != 4) {
        return 52;
    }
    if (text.lex_cmp("kern-lang") != EQUAL) {
        return 53;
    }
    if (text.lex_cmp("kern-lano") != LESS) {
        return 54;
    }
    if (text.lex_cmp("kern-lanf") != GREATER) {
        return 55;
    }
    let stripped_text_prefix = match (text.strip_prefix("kern-")) {
        .{ Some: tail } => tail,
        .None => return 56,
    };
    if (!stripped_text_prefix.eq("lang")) {
        return 57;
    }
    let stripped_text_suffix = match (text.strip_suffix("-lang")) {
        .{ Some: head } => head,
        .None => return 58,
    };
    if (!stripped_text_suffix.eq("kern")) {
        return 59;
    }

    let shaped = String.{}..&;
    defer shaped.deinit(gpa);
    if (!shaped.clone_from_string(gpa, text)) {
        return 60;
    }
    if (!shaped.insert_char(gpa, 4, b'_')) {
        return 61;
    }
    if (!shaped.insert_str(gpa, 5, "std")) {
        return 62;
    }
    if (!shaped.eq("kern_std-lang")) {
        return 63;
    }
    shaped.truncate(9);
    if (!shaped.eq("kern_std-")) {
        return 64;
    }
    shaped.retain_bytes(.[](byte: u8) bool {
        return byte != b'_';
    });
    if (!shaped.eq("kernstd-")) {
        return 65;
    }

    let scratch = String.{}..&;
    defer scratch.deinit(gpa);
    if (!scratch.push_str(gpa, "abcde")) {
        return 66;
    }
    let scratch_bytes = scratch.as_mut_bytes();
    if (!scratch_bytes.swap(1, 3)) {
        return 67;
    }
    scratch_bytes.reverse();
    if (!scratch.eq("ebcda")) {
        return 68;
    }

    let snapshot = text.as_str();
    if (!text.push_str(gpa, snapshot)) {
        return 69;
    }
    if (!text.eq("kern-langkern-lang")) {
        return 70;
    }
    let last_dash = match (text.rfind_byte(b'-')) {
        .{ Some: index } => index,
        .None => return 71,
    };
    if (last_dash != 13) {
        return 72;
    }
    let free_last_dash = match (rfind_byte(text.as_str(), b'-')) {
        .{ Some: index } => index,
        .None => return 73,
    };
    if (free_last_dash != 13) {
        return 74;
    }
    let free_first_dash = match (find_byte(text.as_str(), b'-')) {
        .{ Some: index } => index,
        .None => return 75,
    };
    if (free_first_dash != 4) {
        return 76;
    }

    let extra = String.{}..&;
    defer extra.deinit(gpa);
    if (!extra.push_str(gpa, "!")) {
        return 77;
    }
    if (!text.push_string(gpa, extra)) {
        return 78;
    }
    if (!text.eq("kern-langkern-lang!")) {
        return 79;
    }
    if (!text.as_bytes().ends_with("!")) {
        return 80;
    }

    let popped = match (text.pop_char()) {
        .{ Some: byte } => byte,
        .None => return 81,
    };
    if (popped != b'!') {
        return 82;
    }
    if (!text.eq("kern-langkern-lang")) {
        return 83;
    }

    text.reverse_bytes();
    if (!text.eq("gnal-nrekgnal-nrek")) {
        return 84;
    }
    text.reverse_bytes();
    if (!text.eq("kern-langkern-lang")) {
        return 85;
    }

    let padded = " \t kern \r\n";
    if (!trim_ascii_start(padded).eq("kern \r\n")) {
        return 86;
    }
    if (!trim_ascii_end(padded).eq(" \t kern")) {
        return 87;
    }
    if (!trim_ascii(padded).eq("kern")) {
        return 88;
    }
    if (!padded.trim_ascii_start().eq("kern \r\n")) {
        return 89;
    }
    if (!padded.trim_ascii_end().eq(" \t kern")) {
        return 90;
    }
    if (!padded.trim_ascii().eq("kern")) {
        return 91;
    }
    let space_index = match (padded.find_byte(b'k')) {
        .{ Some: index } => index,
        .None => return 92,
    };
    if (space_index != 3) {
        return 93;
    }

    let spaced = String.{}..&;
    defer spaced.deinit(gpa);
    if (!spaced.push_str(gpa, "  hi\t")) {
        return 94;
    }
    if (!spaced.trim_ascii().eq("hi")) {
        return 95;
    }

    let spaced_bytes = spaced.as_mut_bytes();
    spaced_bytes.[2] = b'!';
    if (!spaced.eq("  !i\t")) {
        return 96;
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
fn runs_hosted_program_using_coll_iteration_and_copy_helpers() {
    let output = build_and_run_hosted(
        r#"
use std.coll.{List, String};
use std.mem.alloc.{Page, GPA};

extern fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;

    let base = [4]i32.{1, 2, 3, 4};
    let base_view = base.[0 .. 4];

    let folded = base_view.fold(i32.{0}, .[](accum: i32, value: i32) i32 {
        return accum + value;
    });
    if (folded != 10) {
        return 1;
    }

    let mut visited = i32.{0};
    base_view.for_each(.[visited = visited..&](value: i32) void {
        visited.* += value;
    });
    if (visited != 10) {
        return 2;
    }

    let bytes = [4]mut i32.{0, 0, 0, 0};
    let writable = bytes..[0 .. 4];
    writable.fill(3);
    if (!bytes.[0 .. 4].eq([4]i32.{3, 3, 3, 3})) {
        return 3;
    }

    if (!writable.copy_from(base_view)) {
        return 4;
    }
    if (!bytes.[0 .. 4].eq(base_view)) {
        return 5;
    }

    let overlap = bytes..[1 .. 4];
    let source = bytes.[0 .. 3];
    if (!overlap.copy_from(source)) {
        return 6;
    }
    if (!bytes.[0 .. 4].eq([4]i32.{1, 1, 2, 3})) {
        return 7;
    }

    writable.for_each_mut(.[](value: *mut i32) void {
        value.* += 1;
    });
    if (!bytes.[0 .. 4].eq([4]i32.{2, 2, 3, 4})) {
        return 8;
    }

    let lhs = bytes..[0 .. 2];
    let rhs = bytes..[2 .. 4];
    if (!lhs.swap_with_slice(rhs)) {
        return 9;
    }
    if (!bytes.[0 .. 4].eq([4]i32.{3, 4, 2, 2})) {
        return 10;
    }

    let list = List[i32].{}..&;
    defer list.deinit(gpa);
    if (!list.extend(gpa, base_view)) {
        return 11;
    }

    let mut list_seen = i32.{0};
    list.for_each(.[list_seen = list_seen..&](value: i32) void {
        list_seen.* += value;
    });
    if (list_seen != 10) {
        return 12;
    }

    let doubled = list.fold(i32.{0}, .[](accum: i32, value: i32) i32 {
        return accum + value * 2;
    });
    if (doubled != 20) {
        return 13;
    }

    list.for_each_mut(.[](value: *mut i32) void {
        value.* *= 2;
    });
    if (!list.as_slice().eq([4]i32.{2, 4, 6, 8})) {
        return 14;
    }

    list.fill(7);
    if (!list.as_slice().eq([4]i32.{7, 7, 7, 7})) {
        return 15;
    }

    let extra = List[i32].{}..&;
    defer extra.deinit(gpa);
    if (!extra.extend(gpa, [2]i32.{9, 10})) {
        return 16;
    }
    if (!list.extend_from_list(gpa, extra)) {
        return 17;
    }
    if (!list.as_slice().eq([6]i32.{7, 7, 7, 7, 9, 10})) {
        return 18;
    }
    if (!list.resize(gpa, 8, 5)) {
        return 19;
    }
    if (!list.as_slice().eq([8]i32.{7, 7, 7, 7, 9, 10, 5, 5})) {
        return 20;
    }
    if (!list.resize(gpa, 3, 0)) {
        return 21;
    }
    if (!list.as_slice().eq([3]i32.{7, 7, 7})) {
        return 22;
    }
    if (!list.clone_from(gpa, [4]i32.{4, 3, 2, 1})) {
        return 23;
    }
    if (!list.as_slice().eq([4]i32.{4, 3, 2, 1})) {
        return 24;
    }
    if (!list.append_repeat(gpa, 6, 2)) {
        return 25;
    }
    if (!list.as_slice().eq([6]i32.{4, 3, 2, 1, 6, 6})) {
        return 26;
    }

    let middle = list.as_slice().[1 .. 3];
    if (!list.insert_slice(gpa, 2, middle)) {
        return 27;
    }
    if (!list.as_slice().eq([8]i32.{4, 3, 3, 2, 2, 1, 6, 6})) {
        return 28;
    }

    list.retain_mut(.[](value: *mut i32) bool {
        value.* *= 10;
        return value.* >= 30;
    });
    if (!list.as_slice().eq([5]i32.{40, 30, 30, 60, 60})) {
        return 29;
    }

    let swapped = match (list.swap_remove(1)) {
        .{ Some: value } => value,
        .None => return 30,
    };
    if (swapped != 30) {
        return 31;
    }
    if (!list.as_slice().eq([4]i32.{40, 60, 30, 60})) {
        return 32;
    }

    let text = String.{}..&;
    defer text.deinit(gpa);
    if (!text.clone_from(gpa, "kern")) {
        return 33;
    }
    if (!text.push_repeat(gpa, b'!', 3)) {
        return 34;
    }
    if (!text.eq("kern!!!")) {
        return 35;
    }

    let mut bangs = i32.{0};
    text.for_each_byte(.[bangs = bangs..&](byte: u8) void {
        if (byte == b'!') {
            bangs.* += 1;
        }
    });
    if (bangs != 3) {
        return 36;
    }

    let ascii_sum = text.fold_bytes(i32.{0}, .[](accum: i32, byte: u8) i32 {
        return accum + byte as i32;
    });
    if (ascii_sum != 531) {
        return 37;
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
fn runs_hosted_program_using_option_and_result_closure_methods() {
    let output = build_and_run_hosted(
        r#"
use std.{Option, Result};

extern fn main() i32 {
    let mut seen = i32.{0};

    let option = Option[i32].{ Some: 7 };
    let mapped = match (option.map(.[seen = seen..&](value: i32) i32 {
        seen.* += value;
        return value * 3;
    })) {
        .{ Some: value } => value,
        .None => return 1,
    };
    if (mapped != 21 or seen != 7) {
        return 2;
    }

    let filtered = option.filter(.[](value: i32) bool {
        return value == 7;
    });
    if (!filtered.is_some()) {
        return 3;
    }

    let none = Option[i32].{ None };
    let fallback_default = .[seen = seen..&]() i32 {
        seen.* += 10;
        return 99;
    };
    let fallback_map = .[](value: i32) i32 {
        return value;
    };
    let fallback = none.map_or_else(fallback_default, fallback_map);
    if (fallback != 99 or seen != 17) {
        return 4;
    }

    let option_fallback = none.or_else(.[seen = seen..&]() Option[i32] {
        seen.* += 100;
        return .{ Some: 123 };
    });
    let option_fallback_value = match (option_fallback) {
        .{ Some: value } => value,
        .None => return 5,
    };
    if (option_fallback_value != 123 or seen != 117) {
        return 6;
    }

    let result = Result[i32, i32].{ Ok: 5 };
    let mapped_result = result.map(.[seen = seen..&](value: i32) i32 {
        seen.* += value;
        return value + 1;
    });
    let chained = match (mapped_result.and_then(.[](value: i32) Result[i32, i32] {
        return .{ Ok: value * 2 };
    })) {
        .{ Ok: value } => value,
        .{ Err: _ } => return 7,
    };
    if (chained != 12 or seen != 122) {
        return 8;
    }

    let mut err_seen = i32.{0};
    let _ = Result[i32, i32].{ Err: 4 }.inspect_err(.[err_seen = err_seen..&](err: i32) void {
        err_seen.* = err;
    });
    if (err_seen != 4) {
        return 9;
    }

    let recovered = match (Result[i32, i32].{ Err: 8 }.or_else(.[](err: i32) Result[i32, i32] {
        return .{ Ok: err + 2 };
    })) {
        .{ Ok: value } => value,
        .{ Err: _ } => return 10,
    };
    if (recovered != 10) {
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
        stderr.contains("Eq[Key]")
            || stderr.contains("Hash[Key]")
            || stderr.contains("Map[Key, i32]"),
        "unexpected stderr:\n{}",
        stderr
    );
}
