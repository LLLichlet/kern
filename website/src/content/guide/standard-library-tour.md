---
title: "Standard Library Tour"
summary: "Use `std.io`, `std.fs`, and `std.proc` as ordinary Kern libraries, and manage owned results with explicit allocators."
order: 10
---

After the layer split is clear, the next practical question is simpler:
what do real Kern programs import first?

Today, that usually means:

- `std.io` for printing and formatting
- `std.fs` for path and file helpers
- `std.proc` for hosted process-facing helpers
- an explicit allocator when an API needs owned strings or paths

None of that is hidden prelude magic.
They are ordinary library modules.

## A Validated Example

The following package was built and run successfully in a temporary directory
while writing this guide:

```kern
use std.io;
use std.fs;
use std.proc;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main(argc: i32, argv: **u8) i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    let args = proc.args(argc, argv);

    match (fs.create_dir_all(gpa, "guide-output")) {
        .{ Ok: _ } => {},
        .{ Err: _ } => return 1,
    }

    let mut joined = match (fs.join(gpa, "guide-output", "message.txt")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 2,
    };
    defer joined..&.deinit(gpa);

    let mut normalized = match (fs.normalize(gpa, "./guide-output/../guide-output/message.txt")) {
        .{ Ok: path } => path,
        .{ Err: _ } => return 3,
    };
    defer normalized..&.deinit(gpa);

    let wrote = match (fs.write_all(gpa, normalized.&.as_str(), "hello from std")) {
        .{ Ok: count } => count,
        .{ Err: _ } => return 4,
    };
    if (wrote != 14) {
        return 5;
    }

    let mut text = match (fs.read_to_string(gpa, joined.&.as_str())) {
        .{ Ok: value } => value,
        .{ Err: _ } => return 6,
    };
    defer text..&.deinit(gpa);

    if (!text.&.eq("hello from std")) {
        return 7;
    }

    io.println("argc={} path={} text={}", .{
        args.argc(),
        normalized.&.as_str(),
        text.&.as_str(),
    });

    match (fs.remove_file_if_exists(gpa, joined.&.as_str())) {
        .{ Ok: _ } => {},
        .{ Err: _ } => return 8,
    }
    match (fs.remove_dir_if_exists(gpa, "guide-output")) {
        .{ Ok: _ } => {},
        .{ Err: _ } => return 9,
    }

    if (args.argc() < 1) {
        return 10;
    }
    if (!normalized.&.eq("guide-output/message.txt")) {
        return 11;
    }

    return 0;
}
```

The validated run printed:

```text
argc=1 path=guide-output/message.txt text=hello from std
```

## What This Shows

### `std` Modules Are Ordinary Imports

The example imported:

```kern
use std.io;
use std.fs;
use std.proc;
```

That is the real model:

- no hidden prelude
- no magical global filesystem object
- no special exception for process arguments

You import what you use.

### Allocation-Bearing APIs Ask For An Allocator

Path-building and file-reading helpers often return owned `String` values.
That means the caller must provide an allocator and later release the owned
result.

The current common hosted pattern is:

```kern
let page = Page.{}..&;
let gpa = GPA.{ backing: page }..&;
```

Then owned results such as `joined`, `normalized`, and `text` are cleaned up
with `deinit(gpa)`.

Those allocations remain explicit.

### `std.fs` Separates Lexical Path Work From Real I/O

This line:

```kern
fs.join(gpa, "guide-output", "message.txt")
```

is just path construction.

This line:

```kern
fs.normalize(gpa, "./guide-output/../guide-output/message.txt")
```

is still lexical path normalization.

Neither one opens the filesystem.

The actual filesystem effects happen later through calls like:

- `fs.create_dir_all`
- `fs.write_all`
- `fs.read_to_string`
- `fs.remove_file_if_exists`

That split keeps path logic separate from I/O.

### `std.proc.args` Wraps The Entry ABI Instead Of Hiding It

Kern's `main` ABI can stay explicit:

```kern
fn main(argc: i32, argv: **u8) i32
```

When you want a nicer borrowed view, use:

```kern
let args = proc.args(argc, argv);
```

That is still ordinary library code layered on top of the raw entry contract.
It is not a hidden runtime global.

### Errors Stay In Result Values

The example used `match` at every fallible step.

That is the current standard-library style:

- filesystem operations return result values
- allocation failures remain visible
- cleanup stays explicit

There is no exception channel hiding underneath.

## A Good First `std` Toolkit

For many hosted programs, the first small set worth learning is:

- `std.io.println`
- `std.fs.read_to_string`
- `std.fs.write_all`
- `std.fs.join`
- `std.fs.normalize`
- `std.proc.args`

If a call needs owned output, expect to pass an allocator.

## Practical Takeaway

Use `std` as ordinary Kern library code:

- import modules explicitly
- pass allocators where ownership is real
- treat path manipulation and filesystem effects as separate operations
- use `std.proc` to wrap raw process ABI when you want convenience without hidden state

That keeps even high-level Kern code honest about ownership, I/O, and process
boundaries.
