use kernc_cli::test_support::build_and_run;

fn build_and_run_source_with_std(source: &str) -> std::process::Output {
    build_and_run(
        "kernc_fat_pointer_return",
        source,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    )
}

#[test]
fn returns_struct_containing_allocator_fat_pointer_inside_result() {
    let output = build_and_run_source_with_std(
        r#"
use base.mem.alloc.{Allocator, Arena};
use sys.mem.Page;

type Ref = struct {
    alloc: *mut Allocator,
    value: *mut i32,
};

fn make_ref(alloc: *mut Allocator, value: *mut i32) Ref!i32 {
    return .{ Ok: Ref.{ alloc: alloc, value: value } };
}

fn main() i32 {

    let mut page = Page.{}..&;
    let mut arena = Arena.{ backing: *mut Allocator.{ page } };
    let mut value = i32.{42};

    let r = match (make_ref(*mut Allocator.{ arena..& }, value..&)) {
        .{ Ok: r } => r,
        .{ Err: _ } => return 1,
    };

    let _ = r.alloc;
    return if (r.value.* == 42) 0 else 2;
}
"#,
    );

    assert!(
        output.status.success(),
        "program crashed or failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn returns_struct_containing_allocator_fat_pointer_inside_option() {
    let output = build_and_run_source_with_std(
        r#"
use base.mem.alloc.{Allocator, Arena};
use sys.mem.Page;

type Ref = struct {
    alloc: *mut Allocator,
    value: *mut i32,
};

fn make_ref(alloc: *mut Allocator, value: *mut i32) ?Ref {
    return .{ Some: Ref.{ alloc: alloc, value: value } };
}

fn main() i32 {

    let mut page = Page.{}..&;
    let mut arena = Arena.{ backing: *mut Allocator.{ page } };
    let mut value = i32.{42};

    let r = match (make_ref(*mut Allocator.{ arena..& }, value..&)) {
        .{ Some: r } => r,
        .None => return 1,
    };

    let _ = r.alloc;
    return if (r.value.* == 42) 0 else 2;
}
"#,
    );

    assert!(
        output.status.success(),
        "program crashed or failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn returns_struct_containing_allocator_fat_pointer_from_mut_method_result() {
    let output = build_and_run_source_with_std(
        r#"
use base.mem.alloc.{Allocator, Arena};
use sys.mem.Page;

type Ref = struct {
    alloc: *mut Allocator,
    value: *mut i32,
};

type Holder = struct {
    slot: *mut i32 = usize.{1} as *mut i32,
};

impl *mut Holder {
    fn ensure_ref(alloc: *mut Allocator, value: *mut i32) Ref!i32 {
        self.slot = value;
        return .{ Ok: Ref.{ alloc: alloc, value: self.slot } };
    }
}

fn main() i32 {

    let mut page = Page.{}..&;
    let mut arena = Arena.{ backing: *mut Allocator.{ page } };
    let mut value = i32.{42};
    let mut holder = Holder.{};

    let r = match (holder..&.ensure_ref(*mut Allocator.{ arena..& }, value..&)) {
        .{ Ok: r } => r,
        .{ Err: _ } => return 1,
    };

    let _ = r.alloc;
    return if (r.value.* == 42) 0 else 2;
}
"#,
    );

    assert!(
        output.status.success(),
        "program crashed or failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
