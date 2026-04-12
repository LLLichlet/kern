use super::*;

#[test]
fn runs_hosted_program_using_std_coll_map() {
    let output = build_and_run_hosted(
        r#"
use base.coll.Map;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
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
use base.coll.Map;
use base.hash.Hash;
use base.mem.alloc.GPA;
use sys.mem.Page;

type Key = struct {
    group: i32,
    id: i32,
};

impl Key : Eq[Key] {
    pub fn eq(other: Key) bool {
        return self.group == other.group and self.id == other.id;
    }
}

impl Key : Hash[Key] {
    pub fn hash() u64 {
        return self.group as u64;
    }
}

fn main() i32 {
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
fn runs_hosted_program_using_byte_slice_keys() {
    let output = build_and_run_hosted(
        r#"
use base.coll.Map;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    let map = Map[[]u8, i32].{}..&;
    defer map.deinit(gpa);

    let alpha = [5]u8.{ b'a', b'l', b'p', b'h', b'a' };
    let alpha_probe = [5]u8.{ b'a', b'l', b'p', b'h', b'a' };
    let beta = [4]u8.{ b'b', b'e', b't', b'a' };

    if (!map.insert(gpa, alpha.[0 .. 5], 7)) {
        return 1;
    }
    if (!map.insert(gpa, beta.[0 .. 4], 9)) {
        return 2;
    }

    if (!map.contains(alpha_probe.[0 .. 5])) {
        return 3;
    }

    let alpha_value = match (map.get(alpha_probe.[0 .. 5])) {
        .{ Some: value } => value,
        .None => return 4,
    };
    if (alpha_value != 7) {
        return 5;
    }

    let removed = match (map.remove(alpha_probe.[0 .. 5])) {
        .{ Some: value } => value,
        .None => return 6,
    };
    if (removed != 7) {
        return 7;
    }
    if (map.contains(alpha.[0 .. 5])) {
        return 8;
    }
    if (!map.get(beta.[0 .. 4]).is_some_and(.[](value: i32) bool { return value == 9; })) {
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
fn runs_hosted_program_using_string_traits_for_ordering_and_hashing() {
    let output = build_and_run_hosted(
        r#"
use base.coll.String;
use base.cmp.{LESS, EQUAL, GREATER};
use base.hash.hash_of;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
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

    if (alpha.*.cmp(alpha_copy.*) != EQUAL) {
        return 4;
    }
    if (alpha.*.cmp(beta.*) == EQUAL) {
        return 5;
    }
    if (alpha.*.cmp(beta.*) != LESS) {
        return 6;
    }
    if (beta.*.cmp(alpha.*) != GREATER) {
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
use base.coll.Map;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
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
use base.coll.Map;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
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
use base.{Option};
use base.coll.Map;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
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
use base.coll.{Map, List};
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
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
fn rejects_map_key_without_eq_and_hash() {
    let output = compile_source_with_std(
        r#"
use base.coll.Map;

type Key = struct {
    raw: i32,
};

fn main() i32 {
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
