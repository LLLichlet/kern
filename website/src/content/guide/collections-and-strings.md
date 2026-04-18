---
title: "Collections And Strings"
summary: "Use `List[T]`, `String`, and byte-slice helpers as explicit owned containers plus cheap borrowed algorithms."
order: 12
---

Once allocation is clear, the next practical question is what you actually do
with owned containers.

Kern's current `base.coll` layer keeps this straightforward:

- `List[T]` is the owned growable contiguous sequence
- `String` is the owned growable byte string
- borrowed slice and byte-string algorithms stay available on top

That means the container story is not split into "special containers" and
"special string magic". It stays close to slices and explicit ownership.

## A Validated Example

The following package was built and run successfully in a temporary directory
while writing this guide:

```kern
use std.io;
use base.coll.{List, String, trim_ascii};
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main() i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    defer gpa.deinit();

    let numbers = List[i32].{}..&;
    defer numbers.deinit(gpa);

    if (!numbers.push(gpa, 1)
        or !numbers.push(gpa, 2)
        or !numbers.push(gpa, 3))
    {
        return 1;
    }
    if (!numbers.insert(gpa, 1, 9)) {
        return 2;
    }

    let removed = match (numbers.remove(2)) {
        .{ Some: value } => value,
        .None => return 3,
    };
    let found = match (numbers.find([2]i32.{ 9, 3 })) {
        .{ Some: index } => index,
        .None => return 4,
    };

    let text = String.{}..&;
    defer text.deinit(gpa);
    if (!text.push_str(gpa, "kern")
        or !text.insert_str(gpa, 4, "-lang"))
    {
        return 5;
    }

    let dash = match (text.find_byte(b'-')) {
        .{ Some: index } => index,
        .None => return 6,
    };

    let trimmed = trim_ascii("  kern-lang \n");

    io.println("removed={} found={} text={} trimmed={}", .{
        removed,
        found,
        text.as_str(),
        trimmed,
    });

    if (!numbers.starts_with([2]i32.{ 1, 9 }) or !numbers.ends_with([2]i32.{ 9, 3 })) {
        return 7;
    }
    if (!text.ends_with("lang") or dash != 4) {
        return 8;
    }
    if (!trimmed.eq("kern-lang")) {
        return 9;
    }

    return 0;
}
```

The validated run printed:

```text
removed=2 found=1 text=kern-lang trimmed=kern-lang
```

## What This Shows

### `List[T]` Is The Normal Owned Sequence

This block:

```kern
let numbers = List[i32].{}..&;
numbers.push(gpa, 1);
numbers.insert(gpa, 1, 9);
```

shows the intended model:

- `List[T]` owns contiguous storage
- mutation and growth use an allocator
- borrowed querying is layered on top

After insertion, the list is `[1, 9, 2, 3]`.
After `remove(2)`, it becomes `[1, 9, 3]`.

### Query Helpers Follow Slice Semantics

These calls:

```kern
numbers.find([2]i32.{ 9, 3 })
numbers.starts_with([2]i32.{ 1, 9 })
numbers.ends_with([2]i32.{ 9, 3 })
```

work because list queries are deliberately shaped like slice queries.

That is an important design choice:

- borrowed and owned sequence APIs stay close
- learning slice algorithms pays off for owned containers too

### `String` Is An Owned Byte String, Not A Separate Text Universe

This block:

```kern
let text = String.{}..&;
text.push_str(gpa, "kern");
text.insert_str(gpa, 4, "-lang");
```

builds an owned byte string.

The result is:

```text
kern-lang
```

Then the example queries it with:

```kern
text.find_byte(b'-')
text.ends_with("lang")
text.as_str()
```

Again, the design stays close to slice logic rather than inventing a separate
container-specific search language.

### Borrowed Byte Helpers Also Exist As Free Functions

This line:

```kern
let trimmed = trim_ascii("  kern-lang \n");
```

uses a free function over a borrowed byte slice.

That same trimming family also exists in method form on byte slices and
strings.

So the current rule is:

- if you already have a `String`, use its methods when convenient
- if you only have borrowed bytes, the free functions and slice methods are enough

### Owned And Borrowed Forms Interoperate Cleanly

This call:

```kern
text.as_str()
```

hands a borrowed `[]u8` view to formatting and to other APIs.

That is the core ergonomic pattern:

- own data when you need mutation or retention
- borrow views when you only need read-only algorithms or output

Kern keeps those transitions explicit and cheap.

## A Good First Subset To Learn

For day-to-day work, a strong first subset is:

- `List.push`
- `List.insert`
- `List.remove`
- `List.find`
- `List.starts_with`
- `List.ends_with`
- `String.push_str`
- `String.insert_str`
- `String.find_byte`
- `trim_ascii`

That is enough to write a lot of practical parser, CLI, and build-tool code.

## Practical Takeaway

Treat Kern's collection model like this:

- `List[T]` and `String` are owned storage
- slice and byte algorithms remain the conceptual base
- borrowed views such as `as_slice()` and `as_str()` are the normal bridge
- string handling is still byte-slice oriented, not a detached runtime subsystem

That keeps collection code explicit, efficient, and easy to reason about.
