use super::*;

#[test]
fn compiles_const_fn_loops_with_assignment_break_and_continue() {
    let output = compile_source(
        r#"
const fn sum_skip(limit: i32) i32 {
    let mut acc = i32.{0};

    let mut i = i32.{0};
    while (i < limit) {
        if (i == i32.{2}) {
            i += i32.{1};
            continue;
        }
        if (i == i32.{5}) {
            break;
        }
        acc += i;
        i += i32.{1};
    }

    return acc;
}

const TOTAL = sum_skip(i32.{7});

fn main() i32 {
    return TOTAL;
}
"#,
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_const_fn_mutating_local_struct_fields_and_array_elements() {
    let output = compile_source(
        r#"
type Pair = struct {
    left: i32,
    right: i32,
};

const fn build_total() i32 {
    let mut pair = Pair.{ left: 1, right: 2 };
    pair.left += 4;
    pair.right = pair.left + pair.right;

    let mut items = [3]i32.{ 5, 6, 7 };
    items.[1] = pair.right;
    items.[2] += items.[0];

    return pair.right + items.[1] + items.[2];
}

const TOTAL = build_total();

fn main() i32 {
    return TOTAL;
}
"#,
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_const_fn_mutating_local_through_pointer() {
    let output = compile_source(
        r#"
const fn bump(ptr: *mut i32) void {
    ptr.* += 1;
}

const fn run() i32 {
    let mut value = i32.{1};
    bump(value..&);
    return value;
}

const RESULT = run();

fn main() i32 {
    return RESULT;
}
"#,
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_mut_pointer_to_array_whole_value_assignment() {
    let output = compile_source(
        r#"
fn replace(buf: *mut [4]u8) void {
    buf.* = [4]u8.{ 1, 2, 3, 4 };
}

fn main() i32 {
    return 0;
}
"#,
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_pointer_to_mut_array_element_assignment() {
    let output = compile_source(
        r#"
fn write(buf: *mut [4]u8, index: usize, value: u8) void {
    buf.*.[index] = value;
}

fn main() i32 {
    return 0;
}
"#,
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_element_assignment_through_shared_pointer_to_array() {
    let output = compile_source(
        r#"
fn write(buf: *[4]u8, index: usize, value: u8) void {
    buf.*.[index] = value;
}

fn main() i32 {
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
        stderr.contains("cannot assign to an immutable variable or location"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn compiles_const_fn_mutating_struct_field_through_pointer_auto_deref() {
    let output = compile_source(
        r#"
type Counter = struct {
    value: i32,
};

const fn bump(counter: *mut Counter) void {
    counter.value += 3;
}

const fn run() i32 {
    let mut counter = Counter.{ value: 4 };
    bump(counter..&);
    return counter.value;
}

const RESULT = run();

fn main() i32 {
    return RESULT;
}
"#,
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn compiles_mut_address_of_materialized_stack_temporaries() {
    let output = compile_source(
        r#"
fn bump(ptr: *mut i32) i32 {
    ptr.* += 1;
    return ptr.*;
}

fn make(flag: bool) i32 {
    if (flag) {
        return 7;
    }
    return 9;
}

fn main() i32 {
    let a = bump((if (true) 1 else 2)..&);
    let b = bump((match (1) {
        1 => 3,
        _ => 4,
    })..&);
    let c = bump(({ let value = i32.{5}; value })..&);
    let d = bump(make(true)..&);
    return a + b + c + d;
}
"#,
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn allows_mut_address_of_string_literal_temporary() {
    let output = build_and_run_source(
        r#"
fn main() i32 {
    let ptr = "hi"..&;
    ptr.*.[0] = b'b';
    return (ptr.*.[0] != b'b') as i32;
}
"#,
    );

    assert!(
        output.status.success(),
        "program failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn allows_assignment_through_mut_array_binding() {
    let output = compile_source(
        r#"
fn main() i32 {
    let mut arr = [4]i32.{ 0; 4 };
    arr.[0] = 3;
    return arr.[0];
}
"#,
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn allows_mutable_slice_from_mut_array_binding() {
    let output = compile_source(
        r#"
fn main() i32 {
    let mut arr = [3]u8.{ b'a', b'b', b'c' };
    let view = arr..[0 .. 3];
    view.[0] = b'Z';
    return 0;
}
"#,
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn allows_assignment_through_mut_array_field_path() {
    let output = compile_source(
        r#"
type Holder = struct {
    items: [3]u8,
};

fn main() i32 {
    let mut holder = Holder.{ items: [3]u8.{ b'a', b'b', b'c' } };
    holder.items.[0] = b'Z';
    return 0;
}
"#,
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn allows_mutable_slice_from_mut_array_field_path() {
    let output = compile_source(
        r#"
type Holder = struct {
    items: [3]u8,
};

fn main() i32 {
    let mut holder = Holder.{ items: [3]u8.{ b'a', b'b', b'c' } };
    let view = holder.items..[0 .. 3];
    view.[0] = b'Z';
    return 0;
}
"#,
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_rebinding_immutable_array_binding() {
    let output = compile_source(
        r#"
fn main() i32 {
    let arr = [3]u8.{ b'a', b'b', b'c' };
    arr = [3]u8.{ b'x', b'y', b'z' };
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
        stderr.contains("cannot assign to an immutable variable or location"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_mutable_closure_borrow_from_immutable_closure_binding() {
    let output = compile_source(
        r#"
fn takes_mut(cb: *mut Fn() i32) i32 {
    let _ = cb;
    return 0;
}

fn main() i32 {
    let closure = .[base = i32.{7}]() i32 {
        return base;
    };
    return takes_mut(closure);
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
        stderr.contains(
            "cannot implicitly borrow an immutable closure as a mutable closure `*mut Fn`"
        ),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_mutable_closure_object_from_immutable_pointer() {
    let output = compile_source(
        r#"
fn main() i32 {
    let closure = .[]() i32 {
        return 7;
    };
    let ptr = closure.&;
    let _ = *mut Fn() i32.{ ptr };
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
        stderr.contains("cannot create a mutable closure object from an immutable pointer"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_mutable_trait_object_from_immutable_pointer() {
    let output = compile_source(
        r#"
type Ops = trait {
    run: fn() i32,
};

impl *i32 : Ops {
    fn run() i32 {
        return self.*;
    }
}

fn main() i32 {
    let value = i32.{7};
    let ptr = value.&;
    let _ = *mut Ops.{ ptr };
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
        stderr.contains("cannot create a mutable trait object from an immutable pointer"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_immutable_trait_object_from_mut_only_pointer_impl() {
    let output = compile_source(
        r#"
type Base = trait {
    set: fn(i32) void,
    get: fn() i32,
};

type Cell = struct {
    value: i32,
};

impl *mut Cell : Base {
    pub fn set(value: i32) void {
        self.value = value;
    }

    pub fn get() i32 {
        return self.value;
    }
}

fn main() i32 {
    let mut cell = Cell.{ value: 1 };
    let obj = *Base.{ cell..& };
    obj.set(42);
    return cell.value;
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
        stderr.contains("the provided pointer type does not implement the target trait"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn accepts_immutable_trait_object_from_mut_pointer_when_shared_impl_exists() {
    let output = build_and_run_source(
        r#"
type Base = trait {
    get: fn() i32,
};

type Cell = struct {
    value: i32,
};

impl *Cell : Base {
    pub fn get() i32 {
        return self.value;
    }
}

fn main() i32 {
    let mut cell = Cell.{ value: 7 };
    let obj = *Base.{ cell..& };
    return obj.get() - 7;
}
"#,
    );

    assert!(
        output.status.success(),
        "program failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn accepts_mutable_trait_object_from_mut_pointer_impl() {
    let output = build_and_run_source(
        r#"
type Base = trait {
    set: fn(i32) void,
    get: fn() i32,
};

type Cell = struct {
    value: i32,
};

impl *mut Cell : Base {
    pub fn set(value: i32) void {
        self.value = value;
    }

    pub fn get() i32 {
        return self.value;
    }
}

fn main() i32 {
    let mut cell = Cell.{ value: 1 };
    let obj = *mut Base.{ cell..& };
    obj.set(42);
    return cell.value - obj.get();
}
"#,
    );

    assert!(
        output.status.success(),
        "program failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_implicit_immutable_trait_object_from_mut_only_pointer_impl() {
    let output = compile_source(
        r#"
type Base = trait {
    set: fn(i32) void,
    get: fn() i32,
};

type Cell = struct {
    value: i32,
};

impl *mut Cell : Base {
    pub fn set(value: i32) void {
        self.value = value;
    }

    pub fn get() i32 {
        return self.value;
    }
}

fn use_base(obj: *Base) void {
    obj.set(42);
}

fn main() i32 {
    let mut cell = Cell.{ value: 1 };
    use_base(cell..&);
    return cell.value;
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
        stderr.contains("mismatched types")
            || stderr.contains("type does not satisfy trait bounds"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn accepts_implicit_immutable_trait_object_from_mut_pointer_when_shared_impl_exists() {
    let output = build_and_run_source(
        r#"
type Base = trait {
    get: fn() i32,
};

type Cell = struct {
    value: i32,
};

impl *Cell : Base {
    pub fn get() i32 {
        return self.value;
    }
}

fn use_base(obj: *Base) i32 {
    return obj.get();
}

fn main() i32 {
    let mut cell = Cell.{ value: 9 };
    return use_base(cell..&) - 9;
}
"#,
    );

    assert!(
        output.status.success(),
        "program failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_coercing_let_mut_trait_object_handle_to_mutable_trait_object() {
    let output = compile_source(
        r#"
type Ops = trait {
    run: fn() i32,
};

impl *i32 : Ops {
    fn run() i32 {
        return self.*;
    }
}

fn takes_mut(value: *mut Ops) void {
    let _ = value;
}

fn main() i32 {
    let number = i32.{7};
    let mut ops = *Ops.{ number.& };
    takes_mut(ops);
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
        stderr.contains("expected `*mut Ops`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_coercing_let_mut_closure_handle_to_mutable_closure() {
    let output = compile_source(
        r#"
fn takes_shared(cb: *Fn() i32) *Fn() i32 {
    return cb;
}

fn takes_mut(cb: *mut Fn() i32) i32 {
    let _ = cb;
    return 0;
}

fn id() i32 {
    return 7;
}

fn main() i32 {
    let mut cb = takes_shared(id);
    return takes_mut(cb);
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
        stderr.contains("expected `*mut Fn() i32`"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_non_const_fn_in_const_context() {
    let output = compile_source(
        r#"
fn runtime_only(v: i32) i32 {
    return v + 1;
}

const BAD = runtime_only(1);

fn main() i32 {
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
        stderr.contains("only `const fn` can be called in constant expressions"),
        "unexpected stderr:\n{}",
        stderr
    );
}

#[test]
fn rejects_arrays_larger_than_llvm_indexable_limit() {
    let output = compile_source(
        r#"
fn main() i32 {
    let _ = [4294967296]u8.{ undef };
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
}

#[test]
fn allows_private_named_struct_fields_within_defining_module() {
    let output = compile_source_tree(
        "main.rn",
        &[
            (
                "main.rn",
                r#"
mod data;

fn main() i32 {
    return data.read_secret();
}
"#,
            ),
            (
                "data.rn",
                r#"
pub type Bag = struct {
    secret: i32,
    pub open: i32,
};

pub fn read_secret() i32 {
    let bag = Bag.{ secret: 5, open: 8 };
    return bag.secret + bag.open;
}
"#,
            ),
        ],
    );

    assert!(
        output.status.success(),
        "kernc failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rejects_private_named_struct_fields_across_modules() {
    let output = compile_source_tree(
        "main.rn",
        &[
            (
                "main.rn",
                r#"
mod data;

fn main() i32 {
    let bag = data.make();
    return bag.secret + bag.open;
}
"#,
            ),
            (
                "data.rn",
                r#"
pub type Bag = struct {
    secret: i32,
    pub open: i32,
};

pub fn make() Bag {
    return Bag.{ secret: 5, open: 8 };
}
"#,
            ),
        ],
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("field `secret` of type `Bag` is private"),
        "unexpected stderr:\n{}",
        stderr
    );
}
