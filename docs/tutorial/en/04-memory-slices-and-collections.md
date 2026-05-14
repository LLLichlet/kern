# 04. Memory, Slices, And Collections

English | [简体中文](../zh/04-内存切片与集合.md)

This chapter moves into Kern's lower-level side: arrays, slices, pointers,
explicit allocation, and scope cleanup. These are not isolated features. They
come from the same rules: values have types, storage paths decide whether
mutation is allowed, and library types usually ask the caller to arrange
resource management explicitly.

## Arrays And Slices

Fixed-size arrays are written `[N]T`, where `N` is the element count and `T` is
the element type:

```kern
let mut values = [5]i32.{ 1, 2, 3, 4, 5 };
values.[0] = 9;
```

`[5]i32.{ ... }` is the same "type provider plus initializer body" syntax from
earlier chapters. The outer `[5]i32` already provides the element type, so the
elements usually do not need to be written as `1i32`, `2i32`, and so on.

Repeated initialization:

```kern
let zeroes = [4]u8.{ 0; 4 };
```

Nested aggregates can use contextual initializer bodies:

```kern
let matrix = [2][3]i32.{
    .{ 1, 2, 3 },
    .{ 4, 5, 6 },
};
```

The compiler can infer array length from the initializer:

```kern
let inferred = [_]i32.{ 1, 2, 3 };
```

`[_]i32` is still an array type; only the length is inferred. It is useful for
lookup tables, test data, and short fixed sequences.

If you want a slice directly, the slice type can be the type provider:

```kern
let direct = &[i32].{ 1, 2, 3 };
```

This creates backing array storage and then forms a read-only slice. The slice
still does not own the elements; it is a view.

Slices are fat pointers: data pointer plus length.

```kern
let middle = values.&[1...4];
let head = values..&[0...3];
```

`values.&[1...4]` produces a read-only `&[i32]` over `[1, 4)`.
`values..&[0...3]` produces a writable `&mut [i32]`.

Range endpoints can be omitted:

```kern
let prefix = values.&[...3];
let tail = values.&[2...];
let whole = values.&[...];
let inclusive = values.&[1..=3];
```

`...3` starts at the beginning, `2...` runs to the end, `...` covers the whole
view, and `1..=3` includes the right endpoint. Slice bounds are memory offsets,
so present endpoints have type `usize`; unsuffixed numbers usually infer that
type from the slice context.

These range expressions are builtin range values. In slice brackets they must
be compiler-owned `SliceBounds` shapes, such as `usize...usize`, `...usize`,
`usize...`, `usize..=usize`, or `...`. `SliceBounds` is only a marker for valid
slice bounds; user code cannot implement it to invent new slicing behavior.

Kern does not write mutability as `[5]mut i32`, because mutability belongs to
the storage path, not to the element type:

```kern
let fixed = [3]i32.{ 1, 2, 3 };
let fixed_view = fixed.&[...];

let mut editable = [3]i32.{ 1, 2, 3 };
let editable_view = editable..&[...];
editable_view.[0] = 9;
```

Use `.len()` in ordinary code:

```kern
fn sum(items: &[i32]) i32 {
    let mut total = 0;
    let mut i = 0;
    while (i < items.len()) {
        total += items.[i];
        i += 1;
    }
    return total;
}
```

The compiler-owned primitive `items.@len()` extracts slice length without
depending on the standard library. The ordinary `items.len()` form is a normal
library wrapper over that primitive.

## Iterators

`base.coll` provides iterator implementations for builtin ranges, slice
iterators, and common consuming methods. The direct form is `for`:

```kern
let mut total = 0;
for (i: 1...4) {
    total += i * i;
}
```

Use `.rev()` on an existing range value when you want reverse traversal:

```kern
let mut descending = 0;
for (i: (1...4).rev()) {
    descending = descending * 10 + i;
}
```

This visits `3`, then `2`, then `1`. The parentheses are intentional:
`a...b.rev()` means `a...(b.rev())`, not `(a...b).rev()`.

Slices provide `.iter()`. Arrays can naturally decay to slices at method-call
and argument boundaries, so iterating over a whole array is usually direct:

```kern
let values = [3]i32.{ 1, 2, 3 };
let mut total = 0;

for (item: values.iter()) {
    total += item;
}
```

When you want to emphasize the slice boundary or iterate over part of an array,
write the slice explicitly:

```kern
for (item: values.&[1...].iter()) {
    total += item;
}
```

Iterators are explicit state values. A `for` loop has roughly this shape:

```kern
let mut iter = 1...4;
while (true) {
    let .{ Some: i } = iter..&.next() else break;
    total += i * i;
}
```

`next()` returns `?Item`: `Some(item)` when there is another element, `None`
when iteration is finished.

Implementing an iterator means implementing the `Iterator` trait for a mutable
receiver:

```kern
use base.coll.Iterator;

struct CountTo {
    current: usize,
    limit: usize,
};

impl &mut CountTo : Iterator {
    type Item = usize;

    pub fn next() ?Item {
        if (self.current >= self.limit) {
            return .None;
        }

        let item = self.current;
        self.current += 1;
        return .{ Some: item };
    }
}
```

The impl is on `&mut CountTo` because advancing the iterator changes
`current`.

Mutable slice iteration yields mutable element pointers:

```kern
let mut values = [3]i32.{ 1, 2, 3 };
for (item: values..&[...].iter()) {
    item.* += 1;
}
```

`values..&[...]` is `&mut [i32]`, so the iterator produces `&mut i32` values.

## Pointers

Kern currently has two common pointer families:

- `&T` / `&mut T`: ordinary object pointers.
- `^T` / `^mut T`: address / volatile pointers for MMIO and fixed-address access.

Most ordinary code uses `&T` and `&mut T`. Address-of syntax separates
read-only and writable access:

```kern
let mut value = 10i32;

let read_ptr = value.&;
let write_ptr = value..&;

let current = read_ptr.*;
write_ptr.* = current + 1;
```

`value.&` produces `&i32`; `value..&` produces `&mut i32`. `.*` is explicit
dereference. Kern does not auto-dereference pointer targets.

Library authors usually put read-only methods on `impl &T` and mutating
methods on `impl &mut T`. A writable pointer can still call read-only methods
where the library exposes them, so APIs can express which operations mutate
through receiver type.

## `defer` And Explicit Release

Kern has no garbage collector and no hidden destructor policy. Use `defer` to
run cleanup code when the current block exits:

```kern
{
    acquire();
    defer release();

    work();
}
```

`defer` runs at block exit, last registered first. It runs when control reaches
the end of the block, or leaves through `return`, `break`, or `continue`.

When a block itself produces a value, Kern computes the result first, runs the
block's defers, then yields the result to the outer context. The order is
explicit; do not return pointers to resources that a `defer` in the same block
is about to release.

## Allocation Pattern In Standard Containers

Kern's standard-library containers usually require an explicit allocator. This
is a library design, not a syntax rule: types such as `List[T]` and `String` do
not store the allocator inside themselves. Operations that may allocate, grow,
or release storage receive the allocator explicitly.

Typical code obtains an allocator, creates container values in the current
scope, and immediately registers cleanup:

```kern
use base.coll.{list, string};
use base.mem.alloc.gpa;
use std.mem.page;

let page = page()..&;
let gpa = gpa().on(page)..&;

let numbers = list[i32]()..&;
defer numbers.deinit(gpa);

if (!numbers.push(gpa, 3)) return;
if (!numbers.push(gpa, 1)) return;
if (!numbers.push(gpa, 2)) return;

let text = string()..&;
defer text.deinit(gpa);

if (!text.push_str(gpa, "Hello")) return;
if (!text.push_str(gpa, ", Kern")) return;
```

The important pieces are:

- `list[i32]()` creates an empty `List[i32]` value.
- `..&` obtains a writable receiver for `push` and `deinit`.
- `push(gpa, value)` may allocate, so the allocator is explicit.
- `deinit(gpa)` releases backing storage; the container does not magically know which allocator to use.

This differs from C++ RAII: release is not automatically tied to object
lifetime. The common Kern style is to keep the resource value in the current
scope and register cleanup with `defer` next to acquisition.

See [`examples/collections.kn`](../../../examples/collections.kn) and
[`examples/string.kn`](../../../examples/string.kn) for fuller examples.

## File I/O

`std.fs` provides user-facing filesystem helpers. Paths are usually obtained
from byte strings with `.path()`. This example keeps allocator use, `match`
error handling, and `defer` cleanup explicit:

```kern
use base.mem.alloc.gpa;
use std.fs;
use std.io;
use std.mem.page;

const SUCCESS = 0i32;
const FAILURE = 1i32;

fn main() i32 {
    let page = page()..&;
    let gpa = gpa().on(page)..&;

    let path = ".craft/example.txt".path();
    _ = path.remove_file_if_exists(gpa);

    let written = match (path.write_all_atomic(gpa, "kern examples\n")) {
        .{ Ok: value } => value,
        .{ Err: _ } => return FAILURE,
    };

    let mut text = match (path.read_to_string(gpa)) {
        .{ Ok: value } => value,
        .{ Err: _ } => return FAILURE,
    };
    defer text..&.deinit(gpa);

    "wrote {} bytes: {}".fmt(.{written, text.&.as_str()}).println();
    _ = path.remove_file_if_exists(gpa);
    return SUCCESS;
}
```

`read_to_string` returns a string value that owns backing storage. The example
binds it as `let mut text`, registers `deinit`, then uses `text.&.as_str()` for
read-only formatting.

## `const` And `static`

`const` is a compile-time binding. It does not create a runtime object, address,
or linker symbol:

```kern
const SUCCESS = 0i32;
const PAGE_SIZE = 4096usize;
```

Use it for exit codes, flags, array lengths, and platform constants.

`static` creates real global storage. Use `static mut` for writable global
storage:

```kern
static VERSION = [4]u8.{ b'k', b'e', b'r', b'n' };
static mut BOOT_COUNT = 0usize;
```

`static` can also appear inside a block, in a position similar to `let`, while
still creating static storage:

```kern
fn default_name() &[u8] {
    static name = "kern";
    return name;
}
```

Prefer `const` when you only need a compile-time value. Use `static` /
`static mut` only when you need a fixed address, global object identity, ABI
interaction, or long-lived global state.
