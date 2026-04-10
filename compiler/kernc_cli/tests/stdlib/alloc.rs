use super::*;

#[test]
fn runs_hosted_program_using_gpa_alignment_and_arena() {
    let output = build_and_run(
        "kernc_std_alloc",
        r#"
use base.mem.Layout;
use base.mem.alloc.{GPA, Arena};
use sys.mem.Page;

fn main() i32 {
    let page = Page.{}..&;

    let gpa = GPA.{ backing: page }..&;
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

    let arena = Arena.{ backing: page }..&;
    defer arena.deinit();

    let arena_a = match (arena.alloc(Layout.{ size: 24, align: 16 })) {
        .{ Some: ptr } => ptr,
        .None => return 7,
    };
    if (((arena_a as usize) % 16) != 0) {
        return 8;
    }

    let arena_b = match (arena.alloc(Layout.{ size: 40, align: 32 })) {
        .{ Some: ptr } => ptr,
        .None => return 9,
    };
    if (((arena_b as usize) % 32) != 0) {
        return 10;
    }
    if ((arena_b as usize) <= (arena_a as usize)) {
        return 11;
    }

    arena.reset();

    let arena_reused = match (arena.alloc(Layout.{ size: 24, align: 16 })) {
        .{ Some: ptr } => ptr,
        .None => return 12,
    };
    if ((arena_reused as usize) != (arena_a as usize)) {
        return 13;
    }

    let grow = Arena.{ backing: page }..&;
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
