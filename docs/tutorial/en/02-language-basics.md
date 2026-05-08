# 02. Language Basics

English | [简体中文](../zh/02-语言基础.md)

## A Small Program

```kern
use std.io;

fn add(a: i32, b: i32) i32 {
    return a + b;
}

fn main() i32 {
    let answer = add(20, 22);
    "answer = {}".fmt(.{answer}).println();
    return 0;
}
```

A Kern function declaration looks like:

```kern
fn name(arg: Type) ReturnType {
    ...
}
```

Use `void` when a function has no meaningful return value:

```kern
fn say(text: &[u8]) void {
    text.println();
}
```

## Bindings And Mutability

Local bindings use `let`:

```kern
let x = 10;
let mut y = 20;
y += 1;
```

One important Kern pattern is "type provider plus initializer body":

```kern
let mut value = i32.{10};
value = 11;
```

`i32.{10}` means: construct the value `10` with `i32` as the explicit type
source. It is not a suffix, and it is not an implicit conversion from an
untyped value. It shares the same initialization syntax as other Kern values:

```kern
let byte = u8.{0xff};
let size = usize.{1024};

struct Point {
    x: i32,
    y: i32,
};

let p = Point.{ x: 1, y: 2 };
let bytes = [4]u8.{ 1, 2, 3, 4 };
```

When context is already clear, the type provider can be omitted:

```kern
let mut i = 0;
while (i < 10) {
    i += 1;
}
```

When width, ABI, bit pattern, or pointer-adjacent logic matters, keep the type
provider:

```kern
let mask = u8.{0xff};
let null = usize.{0};
```

`mut` applies to the storage location, not to the type itself. `let mut value`
means this local storage can be reassigned or modified in place. It does not
upgrade every pointer or slice derived from that storage into writable access.

```kern
let mut value = i32.{10};

let ptr = value..&;
ptr.* = 12;
```

`value..&` produces `&mut i32`, and `ptr.*` writes through the pointer. Kern
does not perform automatic dereferencing, so pointer reads and writes use
explicit `.*`.

## Builtin Types

Common builtin types include:

- Integers: `i8`, `i16`, `i32`, `i64`, `i128`, and `u8`, `u16`, `u32`, `u64`, `u128`.
- Pointer-sized integers: `usize`, `isize`.
- Floating point: `f32`, `f64`.
- Boolean: `bool`.
- Never: `!`.
- Void: `void`.

Kern has contextual type inference. You can often write `let x = 0;` and let
the use site determine the type. When the type is part of the meaning, keep the
type provider:

```kern
let byte = u8.{0xff};
let max = usize.{1024};
```

## Strings And Bytes

Kern string literals are not implicit NUL-terminated C strings. `"Hello"` is a
`[5]u8`: a fixed-size byte array with five elements.

Array types are written `[N]T`:

```kern
let bytes = [4]u8.{ b'k', b'e', b'r', b'n' };
```

Slice types are written `&[T]` or `&mut [T]`. A slice is not an owning array; it
is a view over contiguous elements plus a length.

```kern
let view = bytes.&[1 .. 4];
```

`view` has type `&[u8]`, a read-only byte slice. Writable slices use `..&`:

```kern
let mut data = [4]u8.{ 1, 2, 3, 4 };
let head = data..&[0 .. 2];
```

When a function expects `&[u8]`, an array can naturally convert to a slice at
the boundary:

```kern
fn len(text: &[u8]) usize {
    return #text;
}

fn main() i32 {
    let n = len("kern");
    "len = {}".fmt(.{n}).println();
    return 0;
}
```

`#` extracts metadata carried by values such as arrays and slices. For arrays
and slices, that metadata is the length.

## Structs And Methods

```kern
struct Point {
    x: i32,
    y: i32,
};

impl &mut Point {
    fn move_by(dx: i32, dy: i32) void {
        self.x += dx;
        self.y += dy;
    }
}

fn main() i32 {
    let mut p = Point.{ x: 1, y: 2 };
    p..&.move_by(3, 4);
    "({}, {})".fmt(.{p.x, p.y}).println();
    return 0;
}
```

`impl` is written on a concrete value type. Kern models by values: `Point` is a
value, pointers such as `&Point` and `&mut Point` are values, and slices such as
`&[u8]` are values too. They are different types and can have different method
sets. Read-only methods often live on `impl &T`; methods that mutate the
receiver usually live on `impl &mut T`.

You can read `impl &mut Point` as: attach functions to the type `&mut Point`,
and provide a hidden `self: &mut Point` inside those functions. This call:

```kern
p..&.move_by(3, 4);
```

first obtains an `&mut Point`, then calls a method on that receiver type. Kern
does not automatically dereference receivers; choosing the receiver type is
part of API design.

Pointer types, slice types, and function-object types are all ordinary concrete
value types, so they can have their own methods. The standard library provides
methods for types such as `&[T]`, `&mut [T]`, and `&[u8]`; your own APIs can
choose receivers the same way:

```kern
impl &mut Point {
    fn clear() void {
        self.x = 0;
        self.y = 0;
    }
}
```

## Basic Control Flow

Kern is expression-oriented. `if` has this form:

```kern
let max = if (a > b) a else b;
```

Branches are expressions too. Use blocks when a branch needs multiple steps:

```kern
let max = if (a > b) {
    a
} else {
    b
};
```

Likewise, `if` does not have a C-style "single statement" special case. It
takes an expression as its branch body:

```kern
if (len == 0) return 0;
```

`while` is also `while (cond) expr`. Loop bodies are usually blocks:

```kern
let mut i = 0;
let mut total = 0;
while (i < 5) {
    total += i;
    i += 1;
}
```

When the body is one expression, it can be written directly:

```kern
while (ready()) tick();
```

Loop expressions are usually used as `void`; loops that cannot terminate
normally can behave as diverging computations.

Iterators can be used with `for`:

```kern
use base.coll.range;

let mut sum = 0;
for (i: range(1, 4)) {
    sum += i;
}
```

`for` stores the iterator as internal mutable state and repeatedly calls
`next()`. This is still a composition of ordinary language mechanisms, not a
hidden runtime. The next chapter introduces `enum`, `match`, `?T`, and `T!E`
for state modeling and error handling.
