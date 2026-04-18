---
title: "Hosted Environment, Process, And Time"
summary: "Use `std.env`, `std.proc`, and `std.time` for hosted process arguments, shell capture, environment variables, and monotonic timing."
order: 15
---

Once you are writing hosted tools, three `std` modules become especially
useful:

- `std.env`
- `std.proc`
- `std.time`

These modules are still ordinary Kern libraries layered on top of Kern's owned
runtime and system boundaries.

## A Validated Example

The following program was compiled with `kernc` and run successfully in a
temporary directory while writing this guide:

```kern
use std.io;
use std.env;
use std.proc;
use std.time;
use base.mem.alloc.GPA;
use sys.mem.Page;

fn main(argc: i32, argv: **u8) i32 {
    let page = Page.{}..&;
    let gpa = GPA.{ backing: page }..&;
    let args = proc.args(argc, argv);

    if (args.len() != 2) {
        return 1;
    }

    let second = match (args.get(1)) {
        .{ Some: value } => value,
        .None => return 2,
    };
    if (!second.eq("delta")) {
        return 3;
    }

    let mut value = match (env.get_or_clone(gpa, "KERN_GUIDE_ENV", "fallback")) {
        .{ Some: text } => text,
        .None => return 4,
    };
    defer value..&.deinit(gpa);

    let mut saw = false;
    let visited = env.visit(.[saw = saw..&](entry: env.Var) bool {
        if (entry.name.eq("KERN_GUIDE_ENV")) {
            saw.* = entry.value.eq("alpha");
        }
        return true;
    });
    if (visited == 0 or !saw) {
        return 5;
    }

    let mut capture = match (proc.shell_capture(gpa, "printf shell-ok")) {
        .{ Ok: out } => out,
        .{ Err: _ } => return 6,
    };
    defer capture.output..&.deinit(gpa);

    let start = time.now();
    time.sleep_millis(5);
    let elapsed = start.elapsed();

    io.println("arg={} env={} shell={} ms={}", .{
        second,
        value.&.as_str(),
        capture.output.&.as_str(),
        elapsed.as_millis(),
    });

    if (!value.&.eq("alpha")) {
        return 7;
    }
    if (capture.status != 0 or !capture.output.&.eq("shell-ok")) {
        return 8;
    }
    if (elapsed.as_nanos() == 0) {
        return 9;
    }

    return 0;
}
```

The validated run printed:

```text
arg=delta env=alpha shell=shell-ok ms=5
```

## What This Shows

### `std.proc.args` Wraps The Raw Entry ABI

This line:

```kern
let args = proc.args(argc, argv);
```

takes the raw entry pair:

```kern
(argc: i32, argv: **u8)
```

and exposes a borrowed helper object with methods such as:

- `len()`
- `get(index)`
- `argc()`
- `argv()`

That keeps the low-level ABI explicit while still giving hosted tools a nicer
surface.

### `std.env` Separates Presence Checks, Owned Reads, And Enumeration

This chapter uses three different environment operations:

- `env.get_or_clone(...)`
- `env.visit(...)`
- the borrowed `env.Var` entry shape

That separation is useful:

- `has` checks presence without cloning a value
- `get` or `get_or_clone` produce owned text when you need to keep it
- `visit` enumerates the hosted environment without forcing a full copied map

### Environment Reads Return Owned Strings

This line:

```kern
let mut value = match (env.get_or_clone(...)) { ... };
```

returns a `String`.

That means the caller owns the result and must later release it:

```kern
defer value..&.deinit(gpa);
```

Kern does not hide that ownership cost.

### `proc.shell_capture` Is An Explicit Hosted Boundary

This call:

```kern
proc.shell_capture(gpa, "printf shell-ok")
```

launches one shell command and returns:

- captured combined output
- normalized exit status

Again, the captured output is owned text and must be deinitialized by the
caller.

This is a practical tool for build helpers, command wrappers, and small hosted
automation utilities.

### `std.time` Uses Monotonic Measurement

This block:

```kern
let start = time.now();
time.sleep_millis(5);
let elapsed = start.elapsed();
```

uses monotonic timing, not wall-clock or timezone policy.

That makes `std.time` a good fit for:

- elapsed-time measurement
- benchmarking loops
- retry backoff and sleeps

without dragging calendar semantics into the core library.

### Duration Values Stay Structured

`elapsed` is not a naked integer.
It is a `Duration` value with methods such as:

- `as_nanos()`
- `as_millis()`
- `as_secs()`
- `subsec_nanos()`

So timing code can stay readable without losing unit precision.

## Hosted Boundary Reminder

These modules are for hosted program shapes.

They belong to Kern's own hosted layer, not to an implicit libc base.
That distinction still matters:

- process arguments come through Kern's entry/runtime model
- environment access and shell capture come through hosted library layers
- time measurement comes through Kern-owned monotonic APIs

## Practical Takeaway

For hosted tools, start with:

- `std.proc.args` for command-line arguments
- `std.env.get`, `get_or_clone`, and `visit` for environment access
- `std.proc.shell_capture` for small command execution helpers
- `std.time.now` / `elapsed` / `sleep_millis` for timing

That gives you a practical hosted toolbox without breaking Kern's explicit
ownership and runtime model.
