use super::*;

#[test]
fn runs_hosted_program_using_layout_based_allocator_api() {
    let output = build_and_run_hosted(
        r#"
use base.mem.{layout_of, array_layout_of};
use sys.mem.Page;

fn main() i32 {
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
fn arrays_satisfy_trait_based_equality() {
    let output = build_and_run_hosted(
        r#"
use std.test;

fn main() i32 {
    let values = [4]i32.{ 1, 2, 3, 4 };
    if (values.len() != 4) {
        return 1;
    }
    test.eq([4]i32.{ 1, 2, 3, 4 }, [4]i32.{ 1, 2, 3, 4 });
    test.not_eq([4]i32.{ 1, 2, 3, 4 }, [4]i32.{ 1, 2, 3, 5 });
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
use base.coll.{List, String, find_byte, rfind_byte, trim_ascii_start, trim_ascii_end, trim_ascii};
use base.cmp.{LESS, GREATER, EQUAL};
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
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
    if (data != [6]i32.{1, 9, 3, 1, 9, 3}) {
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
    if (list.as_slice() != [4]i32.{1, 8, 3, 1}) {
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
    if (stripped_prefix != [2]i32.{3, 1}) {
        return 28;
    }
    let stripped_suffix = match (list.strip_suffix([2]i32.{3, 1})) {
        .{ Some: head } => head,
        .None => return 29,
    };
    if (stripped_suffix != [2]i32.{1, 8}) {
        return 30;
    }

    list.reverse();
    if (list.as_slice() != [4]i32.{1, 3, 8, 1}) {
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
    if (list.as_slice() != [2]i32.{3, 8}) {
        return 33;
    }
    if (!list.shrink_to_fit(gpa)) {
        return 34;
    }
    if (list.capacity() != list.len()) {
        return 35;
    }
    if (list.count(.[](value: i32) bool { return value >= 3; }) != 2) {
        return 36;
    }
    let mapped_big = match (list.find_map(.[](value: i32) ?i32 {
        if (value > 3) {
            return .{ Some: value * 10 };
        }
        return .None;
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
    if (stripped_text_prefix != "lang") {
        return 57;
    }
    let stripped_text_suffix = match (text.strip_suffix("-lang")) {
        .{ Some: head } => head,
        .None => return 58,
    };
    if (stripped_text_suffix != "kern") {
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
    if (shaped != "kern_std-lang") {
        return 63;
    }
    shaped.truncate(9);
    if (shaped != "kern_std-") {
        return 64;
    }
    shaped.retain_bytes(.[](byte: u8) bool {
        return byte != b'_';
    });
    if (shaped != "kernstd-") {
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
    if (scratch != "ebcda") {
        return 68;
    }

    let snapshot = text.as_str();
    if (!text.push_str(gpa, snapshot)) {
        return 69;
    }
    if (text != "kern-langkern-lang") {
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
    if (text != "kern-langkern-lang!") {
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
    if (text != "kern-langkern-lang") {
        return 83;
    }

    text.reverse_bytes();
    if (text != "gnal-nrekgnal-nrek") {
        return 84;
    }
    text.reverse_bytes();
    if (text != "kern-langkern-lang") {
        return 85;
    }

    let padded = " \t kern \r\n";
    if (trim_ascii_start(padded) != "kern \r\n") {
        return 86;
    }
    if (trim_ascii_end(padded) != " \t kern") {
        return 87;
    }
    if (trim_ascii(padded) != "kern") {
        return 88;
    }
    if (padded.trim_ascii_start() != "kern \r\n") {
        return 89;
    }
    if (padded.trim_ascii_end() != " \t kern") {
        return 90;
    }
    if (padded.trim_ascii() != "kern") {
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
    if (spaced.trim_ascii() != "hi") {
        return 95;
    }

    let spaced_bytes = spaced.as_mut_bytes();
    spaced_bytes.[2] = b'!';
    if (spaced != "  !i\t") {
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
use base.coll.{List, String};
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
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

    let mut bytes = [4]i32.{0, 0, 0, 0};
    let writable = bytes..[0 .. 4];
    writable.fill(3);
    if (bytes.[0 .. 4] != [4]i32.{3, 3, 3, 3}) {
        return 3;
    }

    if (!writable.copy_from(base_view)) {
        return 4;
    }
    if (bytes.[0 .. 4] != base_view) {
        return 5;
    }

    let overlap = bytes..[1 .. 4];
    let source = bytes.[0 .. 3];
    if (!overlap.copy_from(source)) {
        return 6;
    }
    if (bytes.[0 .. 4] != [4]i32.{1, 1, 2, 3}) {
        return 7;
    }

    writable.for_each_mut(.[](value: *mut i32) void {
        value.* += 1;
    });
    if (bytes.[0 .. 4] != [4]i32.{2, 2, 3, 4}) {
        return 8;
    }

    let lhs = bytes..[0 .. 2];
    let rhs = bytes..[2 .. 4];
    if (!lhs.swap_with_slice(rhs)) {
        return 9;
    }
    if (bytes.[0 .. 4] != [4]i32.{3, 4, 2, 2}) {
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
    if (list.as_slice() != [4]i32.{2, 4, 6, 8}) {
        return 14;
    }

    list.fill(7);
    if (list.as_slice() != [4]i32.{7, 7, 7, 7}) {
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
    if (list.as_slice() != [6]i32.{7, 7, 7, 7, 9, 10}) {
        return 18;
    }
    if (!list.resize(gpa, 8, 5)) {
        return 19;
    }
    if (list.as_slice() != [8]i32.{7, 7, 7, 7, 9, 10, 5, 5}) {
        return 20;
    }
    if (!list.resize(gpa, 3, 0)) {
        return 21;
    }
    if (list.as_slice() != [3]i32.{7, 7, 7}) {
        return 22;
    }
    if (!list.clone_from(gpa, [4]i32.{4, 3, 2, 1})) {
        return 23;
    }
    if (list.as_slice() != [4]i32.{4, 3, 2, 1}) {
        return 24;
    }
    if (!list.append_repeat(gpa, 6, 2)) {
        return 25;
    }
    if (list.as_slice() != [6]i32.{4, 3, 2, 1, 6, 6}) {
        return 26;
    }

    let middle = list.as_slice().[1 .. 3];
    if (!list.insert_slice(gpa, 2, middle)) {
        return 27;
    }
    if (list.as_slice() != [8]i32.{4, 3, 3, 2, 2, 1, 6, 6}) {
        return 28;
    }

    list.retain_mut(.[](value: *mut i32) bool {
        value.* *= 10;
        return value.* >= 30;
    });
    if (list.as_slice() != [5]i32.{40, 30, 30, 60, 60}) {
        return 29;
    }

    let swapped = match (list.swap_remove(1)) {
        .{ Some: value } => value,
        .None => return 30,
    };
    if (swapped != 30) {
        return 31;
    }
    if (list.as_slice() != [4]i32.{40, 60, 30, 60}) {
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
    if (text != "kern!!!") {
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

fn main() i32 {
    let mut seen = i32.{0};

    let option = ?i32.{ Some: 7 };
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

    let none = ?i32.None;
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

    let option_fallback = none.or_else(.[seen = seen..&]() ?i32 {
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

    let result = i32!i32.{ Ok: 5 };
    let mapped_result = result.map(.[seen = seen..&](value: i32) i32 {
        seen.* += value;
        return value + 1;
    });
    let chained = match (mapped_result.and_then(.[](value: i32) i32!i32 {
        return .{ Ok: value * 2 };
    })) {
        .{ Ok: value } => value,
        .{ Err: _ } => return 7,
    };
    if (chained != 12 or seen != 122) {
        return 8;
    }

    let mut err_seen = i32.{0};
    let _ = i32!i32.{ Err: 4 }.inspect_err(.[err_seen = err_seen..&](err: i32) void {
        err_seen.* = err;
    });
    if (err_seen != 4) {
        return 9;
    }

    let recovered = match (i32!i32.{ Err: 8 }.or_else(.[](err: i32) i32!i32 {
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
fn runs_hosted_program_using_option_result_bridge_helpers() {
    let output = build_and_run_hosted(
        r#"
type DecodeError = enum {
    Missing,
    UnexpectedKind: i32,
};

fn require_value(value: ?i32) i32!DecodeError {
    return value.ok_or(.{ UnexpectedKind: 7 });
}

fn main() i32 {
    let some = ?i32.{ Some: 7 };
    let ok = match (some.ok_or(11)) {
        .{ Ok: value } => value,
        .{ Err: _ } => return 1,
    };
    if (ok != 7) {
        return 2;
    }

    let mut seen = 0;
    let none = ?i32.None;
    let err = match (none.ok_or_else(.[seen = seen..&]() i32 {
        seen.* += 1;
        return 23;
    })) {
        .{ Ok: _ } => return 3,
        .{ Err: value } => value,
    };
    if (err != 23 or seen != 1) {
        return 4;
    }

    let eager = match (none.ok_or(31)) {
        .{ Ok: _ } => return 5,
        .{ Err: value } => value,
    };
    if (eager != 31) {
        return 6;
    }

    let inferred = match (require_value(.{ Some: 19 })) {
        .{ Ok: value } => value,
        .{ Err: _ } => return 7,
    };
    if (inferred != 19) {
        return 8;
    }

    let inferred_err = match (require_value(.None)) {
        .{ Ok: _ } => return 9,
        .{ Err: .{ UnexpectedKind: kind } } => kind,
        .{ Err: .Missing } => return 10,
    };
    if (inferred_err != 7) {
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
