use super::*;

#[test]
fn runs_hosted_program_using_gpa_alignment_and_arena() {
    let output = build_and_run(
        "kernc_std_alloc",
        r#"
use base.mem.Layout;
use base.mem.alloc.{arena, gpa};
use sys.mem.page;

fn main() i32 {
    let page = page()..&;
    let gpa = gpa().on(page)..&;
    defer gpa.deinit();

    let aligned = Layout.{ size: 33, align: 256 };
    let ptr_a = match (gpa.alloc(aligned)) {
        .{ Some: ptr } => ptr,
        .None => return 1,
    };
    if (((ptr_a as usize) % 256) != 0) {
        return 2;
    }

    let compact = Layout.{ size: 17, align: 64 };
    let ptr_b = match (gpa.alloc(compact)) {
        .{ Some: ptr } => ptr,
        .None => return 3,
    };
    if (((ptr_b as usize) % 64) != 0) {
        return 4;
    }

    gpa.free(ptr_a, aligned);
    gpa.free(ptr_b, compact);

    let ptr_c = match (gpa.alloc(aligned)) {
        .{ Some: ptr } => ptr,
        .None => return 5,
    };
    if (((ptr_c as usize) % 256) != 0) {
        return 6;
    }
    gpa.free(ptr_c, aligned);

    let scratch = arena().on(page)..&;
    defer scratch.deinit();

    let arena_a = match (scratch.alloc(Layout.{ size: 24, align: 16 })) {
        .{ Some: ptr } => ptr,
        .None => return 7,
    };
    if (((arena_a as usize) % 16) != 0) {
        return 8;
    }

    let arena_b = match (scratch.alloc(Layout.{ size: 40, align: 32 })) {
        .{ Some: ptr } => ptr,
        .None => return 9,
    };
    if (((arena_b as usize) % 32) != 0) {
        return 10;
    }
    if ((arena_b as usize) <= (arena_a as usize)) {
        return 11;
    }

    scratch.reset();

    let arena_reused = match (scratch.alloc(Layout.{ size: 24, align: 16 })) {
        .{ Some: ptr } => ptr,
        .None => return 12,
    };
    if ((arena_reused as usize) != (arena_a as usize)) {
        return 13;
    }

    let grow = arena().on(page)..&;
    defer grow.deinit();

    let bump_a = match (grow.alloc(Layout.{ size: 12, align: 8 })) {
        .{ Some: ptr } => ptr,
        .None => return 14,
    };
    let bump_b = match (grow.alloc(Layout.{ size: 12, align: 8 })) {
        .{ Some: ptr } => ptr,
        .None => return 15,
    };
    if ((bump_b as usize) <= (bump_a as usize)) {
        return 16;
    }

    let large = match (grow.alloc(Layout.{ size: 9000, align: 64 })) {
        .{ Some: ptr } => ptr,
        .None => return 17,
    };
    if (((large as usize) % 64) != 0) {
        return 18;
    }

    grow.reset();

    let large_reused = match (grow.alloc(Layout.{ size: 9000, align: 64 })) {
        .{ Some: ptr } => ptr,
        .None => return 19,
    };
    if ((large_reused as usize) != (large as usize)) {
        return 20;
    }

    let bump_reused = match (grow.alloc(Layout.{ size: 12, align: 8 })) {
        .{ Some: ptr } => ptr,
        .None => return 21,
    };
    if ((bump_reused as usize) <= (large_reused as usize)) {
        return 22;
    }

    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    assert!(
        output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_gpa_invalid_free_usage() {
    let (source_path, executable_path) = build_temp_program(
        "kernc_std_alloc_invalid_free",
        r#"
use base.mem.Layout;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    defer gpa.deinit();

    let good = Layout.{ size: 16, align: 16 };
    let ptr = match (gpa.alloc(good)) {
        .{ Some: ptr } => ptr,
        .None => return 1,
    };

    gpa.free(ptr, Layout.{ size: 8, align: 16 });
    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    let run_output = Command::new(&executable_path).output().unwrap();
    assert!(
        !run_output.status.success(),
        "expected invalid GPA free to fail:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let _ = fs::remove_file(&source_path);
    let _ = fs::remove_file(&executable_path);
}

#[test]
fn runs_hosted_program_using_typed_allocation_helpers() {
    let output = build_and_run(
        "kernc_std_alloc_typed",
        r#"
use base.mem.alloc.Allocator;
use base.mem.alloc.GPA;
use sys.mem.Page;

struct Pair {
    left: i32,
    right: i32,
};

fn sum(items: &[i32]) i32 {
    return items.fold[i32, i32](0, [](accum: i32, value: i32) i32 {
        return accum + value;
    });
}

fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    let alloc = (&mut Allocator).{ gpa };
    defer gpa.deinit();

    let pair = match (alloc.alloc_one[Pair]()) {
        .{ Some: ptr } => ptr,
        .None => return 1,
    };
    pair.* = Pair.{ left: 11, right: 31 };
    if (pair.left + pair.right != 42) {
        return 2;
    }
    alloc.free_one[Pair](pair);

    let items = match (alloc.alloc_array[i32](5)) {
        .{ Some: slice } => slice,
        .None => return 3,
    };
    items.[0] = 5;
    items.[1] = 1;
    items.[2] = 4;
    items.[3] = 1;
    items.[4] = 3;
    items.sort();
    if (items != [5]i32.{ 1, 1, 3, 4, 5 }) {
        return 4;
    }
    if (sum(items) != 14) {
        return 5;
    }
    alloc.free_array[i32](items);

    let source = [4]i32.{ 7, 8, 9, 10 };
    let clone = match (alloc.clone_array[i32](source.&[0 .. 4])) {
        .{ Some: slice } => slice,
        .None => return 6,
    };
    clone.[2] = 90;
    if (source.[2] != 9 or clone.[2] != 90) {
        return 7;
    }
    clone.sort();
    if (clone.lower_bound(90) != 3) {
        return 8;
    }
    alloc.free_array[i32](clone);

    let empty = match (alloc.alloc_array[u64](0)) {
        .{ Some: slice } => slice,
        .None => return 9,
    };
    if (#empty != 0) {
        return 10;
    }
    alloc.free_array[u64](empty);

    return 0;
}
"#,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    );

    assert!(
        output.status.success(),
        "hosted std binary failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
