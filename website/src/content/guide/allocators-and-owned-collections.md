---
title: "Allocators And Owned Collections"
summary: "Use `Page`, `GPA`, `Arena`, `List`, and `String` explicitly so ownership and lifetime policy stay visible in ordinary Kern code."
order: 11
---

By the time you start using `std.fs` or other higher-level APIs, you are
already touching one of Kern's biggest design decisions:

allocation is explicit.

Kern does not hide allocation policy behind a global heap object.
Owned containers and allocation-bearing helpers ask you which allocator should
back them.

## A Validated Example

The following package was built and run successfully in a temporary directory
while writing this guide:

```kern
use std.io;
use base.coll.{List, String};
use base.mem.alloc.{GPA, Arena};
use sys.mem.Page;

fn main() i32 {
    let page = Page.{}..&;

    let gpa = GPA.{ backing: page }..&;
    defer gpa.deinit();

    let arena = Arena.{ backing: page }..&;
    defer arena.deinit();

    let numbers = List[i32].{}..&;
    defer numbers.deinit(gpa);

    if (!numbers.push(gpa, 1)
        or !numbers.push(gpa, 2)
        or !numbers.push(gpa, 3)
        or !numbers.push(gpa, 4))
    {
        return 1;
    }

    let view = numbers.as_slice();
    let mut i = usize.{0};
    let mut total = i32.{0};
    for (; i < #view; i += usize.{1}) {
        total += view.[i];
    }

    let text = String.{}..&;
    defer text.deinit(gpa);
    if (!text.push_str(gpa, "kern")
        or !text.push_char(gpa, b'-')
        or !text.push_str(gpa, "guide"))
    {
        return 2;
    }

    let scratch = String.{}..&;
    if (!scratch.clone_from_exact_in_arena(arena, text.as_str())) {
        return 3;
    }

    io.println("len={} total={} text={} scratch={}", .{
        numbers.len,
        total,
        text.as_str(),
        scratch.as_str(),
    });

    if (!text.eq("kern-guide") or !scratch.eq("kern-guide")) {
        return 4;
    }

    arena.reset();

    return if (numbers.len == 4 and total == 10) 0 else 5;
}
```

The validated run printed:

```text
len=4 total=10 text=kern-guide scratch=kern-guide
```

## What This Shows

### `Page` Is A Raw Backing Allocator

The example starts with:

```kern
let page = Page.{}..&;
```

`sys.mem.Page` is a provider-owned page allocator.
It maps and unmaps memory directly through the OS.

Most user code does not build owned data structures directly on top of `Page`,
but it is the common backing source for higher-level allocators.

### `GPA` Is The General-Purpose Reusable Allocator

This line:

```kern
let gpa = GPA.{ backing: page }..&;
```

creates a general-purpose allocator backed by `Page`.

That is a good default for ordinary owned values such as:

- `String`
- `List[T]`
- std helpers that return owned paths or text

When those values are truly owned and individually releasable, `GPA` is the
right mental default.

### `Arena` Is For Scratch And Phase-Local Storage

This line:

```kern
let arena = Arena.{ backing: page }..&;
```

creates a bump allocator.

An arena is intentionally different from a general-purpose allocator:

- allocation is very cheap
- individual frees are ignored
- memory is reclaimed in bulk through `reset()` or `deinit()`

That makes it a good fit for scratch buffers, parsing passes, temporary build
state, and other short-lived phases.

### Owned Containers Ask For An Allocator On Mutation

Both of these types are plain library values:

```kern
let numbers = List[i32].{}..&;
let text = String.{}..&;
```

But mutating them requires an allocator:

```kern
numbers.push(gpa, 1)
text.push_str(gpa, "kern")
```

That is the rule worth internalizing:

- owned containers are ordinary values
- growth operations ask where memory should come from

Kern does not hide allocator policy in the container type itself.

### Borrowed Views Stay Cheap

This line:

```kern
let view = numbers.as_slice();
```

creates a borrowed slice view over the list storage.

Likewise:

```kern
text.as_str()
```

creates a borrowed byte-slice view over the string contents.

These borrowed views do not allocate.
They expose the current owned storage through the normal slice model.

### Arena-Owned Scratch Data Should Follow Arena Lifetime

This line:

```kern
scratch.clone_from_exact_in_arena(arena, text.as_str())
```

creates scratch string storage inside the arena.

That is why the example later uses:

```kern
arena.reset();
```

instead of trying to free the scratch string one object at a time.

The lifetime rule is simple:

- data backed by `GPA` is usually released with `deinit(gpa)`
- data backed by `Arena` lives until the arena is reset or destroyed

## The Main Allocator Profiles

For current Kern code, the main mental buckets are:

- `Page`: raw OS-backed page allocation
- `GPA`: ordinary reusable heap allocation
- `Arena`: fast bulk-reclaimed scratch allocation

You do not need to memorize allocator internals before writing code, but you do
need to choose the ownership profile on purpose.

## Practical Takeaway

Use this rule set:

- start with `GPA` for normal owned containers and returned strings
- use `Arena` for temporary phase-local data
- use `as_slice()` and `as_str()` when you want borrowed views
- clean up with the same ownership model you allocated with

That keeps ownership explicit without making routine code unnecessarily
verbose.
