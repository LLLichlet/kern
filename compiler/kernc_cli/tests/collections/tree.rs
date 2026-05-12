use super::*;

#[test]
fn runs_hosted_program_using_std_coll_tree() {
    let output = build_and_run_hosted(
        r#"
use base.coll.{Tree, tree};
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = gpa().on(page)..&;
    let map = tree[i32, i32]()..&;
    defer map.deinit(gpa);
    let mut lazy_calls = i32.{0};

    let mut i = 0;
    while (i < 32) {
        let key = i as i32;
        if (map.try_insert(gpa, key, key * 2).is_err()) {
            return 1;
        }
        i += 1;
    }

    if (map.try_insert(gpa, 7, 99).is_err()) {
        return 2;
    }

    if (!map.get(7).is_some_and([](value: i32) bool { return value == 99; })) {
        return 3;
    }
    if (!map.get(31).is_some_and([](value: i32) bool { return value == 62; })) {
        return 4;
    }

    let value_ptr = match (map.get_ptr(7)) {
        .{ Some: ptr } => ptr,
        .None => return 5,
    };
    value_ptr.* = 123;
    if (!map.get(7).is_some_and([](value: i32) bool { return value == 123; })) {
        return 6;
    }

    let existing = match (map.try_get_or_insert_with(gpa, 7, [calls = lazy_calls..&]() i32 {
        calls.* += 1;
        return 700;
    })) {
        .{ Ok: ptr } => ptr,
        .{ Err: _ } => return 7,
    };
    if (existing.* != 123) {
        return 8;
    }
    if (lazy_calls != 0) {
        return 9;
    }

    let inserted = match (map.try_get_or_insert(gpa, 100, 500)) {
        .{ Ok: ptr } => ptr,
        .{ Err: _ } => return 10,
    };
    if (inserted.* != 500) {
        return 11;
    }

    let lazy_inserted = match (map.try_get_or_insert_with(gpa, 200, [calls = lazy_calls..&]() i32 {
        calls.* += 1;
        return 900;
    })) {
        .{ Ok: ptr } => ptr,
        .{ Err: _ } => return 12,
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
    if (map.len() != 34) {
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
use base.coll.{Tree, tree};
use base.cmp.{Ordering, Comparable, Ord, LESS, EQUAL, GREATER};
use base.mem.alloc.gpa;
use std.mem.Page;

struct Key {
    major: i32,
    minor: i32,
};

impl Key : Eq[Key] {
    pub fn eq(other: Key) bool {
        return self.major == other.major and self.minor == other.minor;
    }
}

impl Key : Comparable[Key] {
    pub fn cmp(other: Key) Ordering {
        if (self.major < other.major) return LESS;
        if (self.major > other.major) return GREATER;
        if (self.minor < other.minor) return LESS;
        if (self.minor > other.minor) return GREATER;
        return EQUAL;
    }
}

impl Key : Ord[Key] {}

fn main() i32 {
    let page = Page.{}..&;
    let gpa = gpa().on(page)..&;
    let map = tree[Key, i32]()..&;
    defer map.deinit(gpa);

    if (map.try_insert(gpa, Key.{ major: 1, minor: 0 }, 10).is_err()) {
        return 1;
    }
    if (map.try_insert(gpa, Key.{ major: 0, minor: 8 }, 8).is_err()) {
        return 2;
    }
    if (map.try_insert(gpa, Key.{ major: 1, minor: 2 }, 12).is_err()) {
        return 3;
    }
    if (map.try_insert(gpa, Key.{ major: 1, minor: 0 }, 99).is_err()) {
        return 4;
    }

    if (!map.get(Key.{ major: 1, minor: 0 }).is_some_and([](value: i32) bool { return value == 99; })) {
        return 5;
    }
    if (!map.get(Key.{ major: 0, minor: 8 }).is_some_and([](value: i32) bool { return value == 8; })) {
        return 6;
    }
    if (map.contains(Key.{ major: 2, minor: 0 })) {
        return 7;
    }
    if (map.len() != 3) {
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
use base.coll.{Tree, tree};

struct Key {
    raw: i32,
};

fn main() i32 {
    let map = tree[Key, i32]()..&;
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
fn runs_hosted_program_using_tree_ordered_traversal_helpers() {
    let output = build_and_run_hosted(
        r#"
use base.coll.{Tree, tree, String, string};
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = gpa().on(page)..&;
    let map = tree[i32, i32]()..&;
    defer map.deinit(gpa);

    if (map.try_insert(gpa, 3, 30).is_err()) return 1;
    if (map.try_insert(gpa, 1, 10).is_err()) return 2;
    if (map.try_insert(gpa, 4, 40).is_err()) return 3;
    if (map.try_insert(gpa, 2, 20).is_err()) return 4;

    let order = string()..&;
    defer order.deinit(gpa);
    map.for_each([order, gpa](key: i32, _: i32) void {
        let _ = order.try_push_char(gpa, (key as u8) + b'0');
    });
    if (order != "1234") {
        return 5;
    }

    let weighted = map.fold(i32.{0}, [](accum: i32, key: i32, value: i32) i32 {
        return accum + key * value;
    });
    if (weighted != 300) {
        return 6;
    }

    map.for_each_mut([](key: i32, value: &mut i32) void {
        value.* += key;
    });
    if (!map.get(1).is_some_and([](value: i32) bool { return value == 11; })) return 7;
    if (!map.get(2).is_some_and([](value: i32) bool { return value == 22; })) return 8;
    if (!map.get(3).is_some_and([](value: i32) bool { return value == 33; })) return 9;
    if (!map.get(4).is_some_and([](value: i32) bool { return value == 44; })) return 10;

    let updated = map.fold(i32.{0}, [](accum: i32, key: i32, value: i32) i32 {
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
use base.coll.{Tree, tree};
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = gpa().on(page)..&;
    let map = tree[i32, i32]()..&;
    defer map.deinit(gpa);

    if (map.try_insert(gpa, 10, 100).is_err()) return 1;
    if (map.try_insert(gpa, 30, 300).is_err()) return 2;
    if (map.try_insert(gpa, 20, 200).is_err()) return 3;
    if (map.try_insert(gpa, 40, 400).is_err()) return 4;

    if (!map.first_key().is_some_and([](key: i32) bool { return key == 10; })) return 5;
    if (!map.first().is_some_and([](value: i32) bool { return value == 100; })) return 6;
    if (!map.last_key().is_some_and([](key: i32) bool { return key == 40; })) return 7;
    if (!map.last().is_some_and([](value: i32) bool { return value == 400; })) return 8;

    if (!map.ceil_key(5).is_some_and([](key: i32) bool { return key == 10; })) return 9;
    if (!map.ceil(10).is_some_and([](value: i32) bool { return value == 100; })) return 10;
    if (!map.ceil_key(21).is_some_and([](key: i32) bool { return key == 30; })) return 11;
    if (!map.ceil(39).is_some_and([](value: i32) bool { return value == 400; })) return 12;
    if (map.ceil_key(41).is_some()) return 13;

    if (map.floor_key(9).is_some()) return 14;
    if (!map.floor_key(10).is_some_and([](key: i32) bool { return key == 10; })) return 15;
    if (!map.floor(29).is_some_and([](value: i32) bool { return value == 200; })) return 16;
    if (!map.floor_key(40).is_some_and([](key: i32) bool { return key == 40; })) return 17;

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

    if (!map.get(10).is_some_and([](value: i32) bool { return value == 101; })) return 22;
    if (!map.get(20).is_some_and([](value: i32) bool { return value == 204; })) return 23;
    if (!map.get(30).is_some_and([](value: i32) bool { return value == 303; })) return 24;
    if (!map.get(40).is_some_and([](value: i32) bool { return value == 402; })) return 25;

    let empty = tree[i32, i32]()..&;
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
use base.coll.{Tree, tree, String, string};
use base.mem.alloc.gpa;
use std.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = gpa().on(page)..&;
    let map = tree[i32, i32]()..&;
    defer map.deinit(gpa);

    let mut i = i32.{1};
    while (i <= 40) {
        if (map.try_insert(gpa, i, i * 10).is_err()) {
            return 1;
        }
        i += 1;
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
    if (map.len() != 37) {
        return 9;
    }

    if (!map.first_key().is_some_and([](key: i32) bool { return key == 2; })) return 10;
    if (!map.last_key().is_some_and([](key: i32) bool { return key == 39; })) return 11;
    if (!map.ceil_key(20).is_some_and([](key: i32) bool { return key == 21; })) return 12;
    if (!map.floor_key(20).is_some_and([](key: i32) bool { return key == 19; })) return 13;

    let mut count = i32.{0};
    let mut ordered = string()..&;
    defer ordered.deinit(gpa);
    map.for_each([count = count..&, ordered, gpa](key: i32, _: i32) void {
        count.* += 1;
        if (key >= 2 and key <= 9) {
            let _ = ordered.try_push_char(gpa, (key as u8) + b'0');
        }
    });
    if (count != 37 or ordered != "23456789") {
        return 14;
    }

    let small = tree[i32, i32]()..&;
    defer small.deinit(gpa);
    if (small.try_insert(gpa, 2, 20).is_err()) return 15;
    if (small.try_insert(gpa, 1, 10).is_err()) return 16;
    if (small.try_insert(gpa, 3, 30).is_err()) return 17;
    if (!small.remove(gpa, 2).is_some_and([](value: i32) bool { return value == 20; })) return 18;
    if (!small.remove(gpa, 1).is_some_and([](value: i32) bool { return value == 10; })) return 19;
    if (!small.remove(gpa, 3).is_some_and([](value: i32) bool { return value == 30; })) return 20;
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
