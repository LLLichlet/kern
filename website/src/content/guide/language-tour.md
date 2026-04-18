---
title: "Language Tour"
summary: "A first pass through Kern source using examples that compile on the current toolchain."
order: 4
---

This chapter gives a first look at Kern source forms that compile today.

The goal is not to explain every rule in the language reference. The goal is to
get a new reader comfortable with the shape of real Kern code.

## A Small Validated Example

The following example was built and run successfully while writing the guide:

```kern
use std.io;

type Point = struct {
    x: i32,
    y: i32,
};

type Mode = enum {
    Cold,
    Warm: i32,
};

fn classify(point: Point) Mode {
    if (point.x == point.y) return .Cold;
    return .{ Warm: point.x - point.y };
}

fn main() i32 {
    let point = Point.{ x: 7, y: 3 };
    let label = match (classify(point)) {
        .Cold => "cold",
        .{ Warm: _ } => "warm",
    };

    io.println("mode: {}", .{label,});
    0
}
```

For the validated run, this printed:

```text
mode: warm
```

## What This Example Shows

### `use`

Imports are explicit:

```kern
use std.io;
```

Kern does not rely on a hidden standard-library prelude for ordinary APIs.

### Struct Initialization

Struct literals use typed initialization syntax:

```kern
let point = Point.{ x: 7, y: 3 };
```

This keeps the destination type visible at the construction site.

### Enums And Pattern Matching

Enums are the language's general tagged-union mechanism:

```kern
type Mode = enum {
    Cold,
    Warm: i32,
};
```

Branching over enum state uses `match`:

```kern
let label = match (classify(point)) {
    .Cold => "cold",
    .{ Warm: _ } => "warm",
};
```

The syntax keeps the variant spelling visible without inventing a second
"special enum pattern language" on the side.

### Explicit Return Type

Program entry still uses an explicit low-level signature:

```kern
fn main() i32 { ... }
```

That is part of Kern's general style: startup and ABI-facing behavior are
spelled out rather than smuggled in as a hidden convention.

## What To Read Next

After this chapter, the most useful next references are:

- `docs/design.md` for the precise language semantics
- `docs/runtime-architecture.md` for the `base` / `sys` / `rt` / `std` split
- `docs/style.md` for repository-level source style
