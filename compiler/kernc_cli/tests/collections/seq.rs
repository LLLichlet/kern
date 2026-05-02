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
        "hosted std binary failed with status {:?}:\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn arrays_satisfy_trait_based_equality() {
    let output = build_and_run_hosted(
        r#"
use base.test;

fn main() i32 {
    let mut ctx = test.silent();
    let values = [4]i32.{ 1, 2, 3, 4 };
    if (values.len() != 4) {
        return 1;
    }
    ctx..&.eq(@loc(), [4]i32.{ 1, 2, 3, 4 }, [4]i32.{ 1, 2, 3, 4 }, "expected arrays to be equal", .{});
    ctx..&.not_eq(@loc(), [4]i32.{ 1, 2, 3, 4 }, [4]i32.{ 1, 2, 3, 5 }, "expected arrays to differ", .{});
    return 0;
}
"#,
    );

    assert!(
        output.status.success(),
        "hosted std binary failed with status {:?}:\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
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
fn runs_hosted_program_using_foundation_numeric_and_slice_algorithms() {
    let output = build_and_run_hosted(
        r##"
use base.num;
use base.cmp.{Ordering, LESS, GREATER};

const SAT_ADD = num.saturating_add_usize(num.USIZE_MAX, 1);
const SAT_MUL = num.saturating_mul_usize(num.USIZE_MAX, 2);
const CHECKED_SUM = num.checked_add_usize(40, 2);
const CHECKED_OVERFLOW = num.checked_add_usize(num.USIZE_MAX, 1);
const CONST_ALIGNED = num.align_up_usize(17, 8);

fn expect_bounds(items: []i32, value: i32, lower: usize, upper: usize) bool {
    return items.lower_bound(value) == lower and items.upper_bound(value) == upper;
}

fn main() i32 {
    let aligned = match (CONST_ALIGNED) {
        .{ Some: value } => value,
        .None => return 1,
    };
    if (aligned != 24) {
        return 1;
    }
    if (SAT_ADD != num.USIZE_MAX or SAT_MUL != num.USIZE_MAX) {
        return 2;
    }
    if (!CHECKED_SUM.is_some_and(.[](value: usize) bool {
        return value == 42;
    }) or CHECKED_OVERFLOW.is_some()) {
        return 3;
    }
    if (!num.checked_sub_usize(10, 3).is_some_and(.[](value: usize) bool {
        return value == 7;
    })) {
        return 4;
    }
    if (num.align_up_usize(10, 3).is_some()) {
        return 5;
    }
    if (!num.is_power_of_two_usize(64) or num.is_power_of_two_usize(0)) {
        return 6;
    }
    if (num.min[i32](8, 3) != 3 or num.max[i32](8, 3) != 8) {
        return 7;
    }
    if (num.clamp[i32](19, -2, 9) != 9 or num.clamp[i32](-5, -2, 9) != -2) {
        return 8;
    }

    let mut values = [8]i32.{ 5, 1, 3, 3, 9, 0, 8, 3 };
    let view = values..[0 .. 8];
    view.sort();

    let sorted = values.[0 .. 8];
    if (!sorted.is_sorted()) {
        return 9;
    }
    if (sorted != [8]i32.{ 0, 1, 3, 3, 3, 5, 8, 9 }) {
        return 10;
    }
    if (!expect_bounds(sorted, 3, 2, 5)) {
        return 11;
    }
    if (!expect_bounds(sorted, 4, 5, 5)) {
        return 12;
    }
    if (!sorted.binary_search(8).is_some_and(.[](index: usize) bool {
        return index == 6;
    })) {
        return 13;
    }

    view.sort_by(.[](lhs: i32, rhs: i32) Ordering {
        return rhs.cmp(lhs);
    });
    if (values.[0 .. 8] != [8]i32.{ 9, 8, 5, 3, 3, 3, 1, 0 }) {
        return 14;
    }

    let mut words = [5][]u8.{ "gamma", "alpha", "beta", "beta", "delta" };
    let word_view = words..[0 .. 5];
    word_view.sort();
    let sorted_words = words.[0 .. 5];
    if (!sorted_words.is_sorted()) {
        return 15;
    }
    if (sorted_words.lower_bound("beta") != 1 or sorted_words.upper_bound("beta") != 3) {
        return 16;
    }
    if (sorted_words.[0] != "alpha" or sorted_words.[4] != "gamma") {
        return 17;
    }

    let odd_first = [6]i32.{ 2, 4, 6, 1, 3, 5 };
    let split = odd_first.[0 .. 6].partition_point(.[](value: i32) bool {
        return (value % 2) == 0;
    });
    if (split != 3) {
        return 18;
    }

    if (LESS != Ordering.{-1} or GREATER != Ordering.{1}) {
        return 19;
    }

    let mut ring = [6]i32.{ 0, 1, 2, 3, 4, 5 };
    let ring_view = ring..[0 .. 6];
    ring_view.rotate_left(2);
    if (ring.[0 .. 6] != [6]i32.{ 2, 3, 4, 5, 0, 1 }) {
        return 20;
    }
    ring_view.rotate_right(8);
    if (ring.[0 .. 6] != [6]i32.{ 0, 1, 2, 3, 4, 5 }) {
        return 21;
    }
    if (!ring_view.copy_within(0, 3, 2)) {
        return 22;
    }
    if (ring.[0 .. 6] != [6]i32.{ 0, 1, 0, 1, 2, 5 }) {
        return 23;
    }
    if (!ring_view.copy_within(2, 6, 0)) {
        return 24;
    }
    if (ring.[0 .. 6] != [6]i32.{ 0, 1, 2, 5, 2, 5 }) {
        return 25;
    }
    if (ring_view.copy_within(4, 2, 0) or ring_view.copy_within(0, 4, 3)) {
        return 26;
    }

    return 0;
}
"##,
    );

    assert!(
        output.status.success(),
        "hosted std binary failed with status {:?}:\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn runs_hosted_program_using_coll_iteration_and_copy_helpers() {
    let output = build_and_run_hosted(
        r#"
use base.coll.{List, String};
use base.cmp.{Ordering, GREATER};
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

    list.rotate_left(1);
    if (list.as_slice() != [4]i32.{60, 30, 60, 40}) {
        return 100;
    }
    list.rotate_right(2);
    if (list.as_slice() != [4]i32.{60, 40, 60, 30}) {
        return 101;
    }
    if (!list.copy_within(0, 2, 2)) {
        return 102;
    }
    if (list.as_slice() != [4]i32.{60, 40, 60, 40}) {
        return 103;
    }
    if (list.copy_within(0, 5, 0)) {
        return 104;
    }
    if (!list.clone_from(gpa, [4]i32.{40, 60, 30, 60})) {
        return 105;
    }

    list.sort();
    if (!list.is_sorted()) {
        return 33;
    }
    if (list.as_slice() != [4]i32.{30, 40, 60, 60}) {
        return 34;
    }
    if (list.lower_bound(60) != 2 or list.upper_bound(60) != 4) {
        return 35;
    }
    if (!list.binary_search(40).is_some_and(.[](index: usize) bool {
        return index == 1;
    })) {
        return 36;
    }

    if (!list.clone_from(gpa, [7]i32.{1, 1, 2, 2, 2, 3, 1})) {
        return 106;
    }
    list.dedup();
    if (list.as_slice() != [4]i32.{1, 2, 3, 1}) {
        return 107;
    }
    list.dedup_by(.[](lhs: i32, rhs: i32) bool {
        return (lhs % 2) == (rhs % 2);
    });
    if (list.as_slice() != [3]i32.{1, 2, 3}) {
        return 108;
    }
    if (!list.clone_from(gpa, [4]i32.{30, 40, 60, 60})) {
        return 109;
    }

    list.sort_by(.[](lhs: i32, rhs: i32) Ordering {
        if (lhs.cmp(rhs) == GREATER) {
            return Ordering.{-1};
        }
        return rhs.cmp(lhs);
    });
    if (list.as_slice() != [4]i32.{60, 60, 40, 30}) {
        return 37;
    }

    let text = String.{}..&;
    defer text.deinit(gpa);
    if (!text.clone_from(gpa, "kern")) {
        return 38;
    }
    if (!text.push_repeat(gpa, b'!', 3)) {
        return 39;
    }
    if (text != "kern!!!") {
        return 40;
    }

    let mut bangs = i32.{0};
    text.for_each_byte(.[bangs = bangs..&](byte: u8) void {
        if (byte == b'!') {
            bangs.* += 1;
        }
    });
    if (bangs != 3) {
        return 41;
    }

    let ascii_sum = text.fold_bytes(i32.{0}, .[](accum: i32, byte: u8) i32 {
        return accum + byte as i32;
    });
    if (ascii_sum != 531) {
        return 42;
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
fn runs_hosted_program_using_integer_ranges_as_iterators() {
    let output = build_and_run_hosted(
        r#"
use base.coll.{
    Iterator,
    iter_all,
    iter_any,
    iter_count,
    iter_find,
    iter_find_map,
    iter_fold,
    iter_for_each,
    iter_last,
    iter_nth,
    iter_position,
    range,
    range_down,
    range_down_inclusive,
    range_inclusive,
};

fn main() i32 {
    let mut sum = 0;
    for (i: range(0, 5)) {
        sum += i;
    }
    if (sum != 10) {
        return 1;
    }

    let mut usize_sum = usize.{0};
    for (i: range(usize.{2}, usize.{5})) {
        usize_sum += i;
    }
    if (usize_sum != usize.{9}) {
        return 2;
    }

    let mut inclusive_sum = 0;
    for (i: range_inclusive(-2, 2)) {
        inclusive_sum += i;
    }
    if (inclusive_sum != 0) {
        return 3;
    }

    let mut empty_count = 0;
    for (_: range(5, 5)) {
        empty_count += 1;
    }
    for (_: range(5, 3)) {
        empty_count += 1;
    }
    for (_: range(usize.{5}, usize.{3})) {
        empty_count += 1;
    }
    for (_: range(-1, -4)) {
        empty_count += 1;
    }
    for (_: range_inclusive(5, 3)) {
        empty_count += 1;
    }
    for (_: range_inclusive(usize.{5}, usize.{3})) {
        empty_count += 1;
    }
    if (empty_count != 0) {
        return 4;
    }

    let mut seen = 0;
    for (value: range_inclusive(u8.{254}, u8.{255})) {
        if (seen == 0 and value != u8.{254}) {
            return 5;
        }
        if (seen == 1 and value != u8.{255}) {
            return 6;
        }
        seen += 1;
    }
    if (seen != 2) {
        return 7;
    }

    let mut descending_sum = i32.{0};
    for (value: range_down(5, 2)) {
        descending_sum += value;
    }
    if (descending_sum != 12) {
        return 8;
    }

    let mut inclusive_descending_sum = i32.{0};
    for (value: range_down_inclusive(3, 1)) {
        inclusive_descending_sum += value;
    }
    if (inclusive_descending_sum != 6) {
        return 9;
    }

    let mut empty_descending = 0;
    for (_: range_down(2, 5)) {
        empty_descending += 1;
    }
    for (_: range_down_inclusive(2, 5)) {
        empty_descending += 1;
    }
    if (empty_descending != 0) {
        return 10;
    }

    let mut count_range = range(2, 7);
    if (iter_count[i32](count_range..&) != 5) {
        return 11;
    }

    let mut nth_range = range(10, 20);
    let third = match (iter_nth[i32](nth_range..&, usize.{3})) {
        .{ Some: value } => value,
        .None => return 12,
    };
    if (third != 13) {
        return 13;
    }
    let after_third = match (nth_range..&.next()) {
        .{ Some: value } => value,
        .None => return 14,
    };
    if (after_third != 14) {
        return 15;
    }

    let mut fold_range = range_down_inclusive(4, 1);
    if (iter_fold[i32, i32](fold_range..&, 0, .[](accum: i32, value: i32) i32 {
        return accum + value;
    }) != 10) {
        return 16;
    }

    let mut any_range = range(0, 6);
    if (!iter_any[i32](any_range..&, .[](value: i32) bool { return value == 4; })) {
        return 17;
    }

    let mut all_range = range(1, 4);
    if (!iter_all[i32](all_range..&, .[](value: i32) bool { return value > 0; })) {
        return 18;
    }

    let mut find_range = range(3, 9);
    let found = match (iter_find[i32](find_range..&, .[](value: i32) bool { return value % 2 == 0; })) {
        .{ Some: value } => value,
        .None => return 19,
    };
    if (found != 4) {
        return 20;
    }

    let mut pos_range = range_down(9, 3);
    let pos = match (iter_position[i32](pos_range..&, .[](value: i32) bool { return value == 6; })) {
        .{ Some: value } => value,
        .None => return 21,
    };
    if (pos != usize.{3}) {
        return 22;
    }

    let mut map_range = range(0, 6);
    let mapped = match (iter_find_map[i32, i32](map_range..&, .[](value: i32) ?i32 {
        if (value < 3) return .None;
        return .{ Some: value * 10 };
    })) {
        .{ Some: value } => value,
        .None => return 23,
    };
    if (mapped != 30) {
        return 24;
    }

    let mut each_range = range(1, 4);
    let mut each_sum = i32.{0};
    iter_for_each[i32](each_range..&, .[sum = each_sum..&](value: i32) void {
        sum.* += value;
    });
    if (each_sum != 6) {
        return 25;
    }

    let mut last_range = range_down_inclusive(3, 1);
    let last = match (iter_last[i32](last_range..&)) {
        .{ Some: value } => value,
        .None => return 26,
    };
    if (last != 1) {
        return 27;
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
