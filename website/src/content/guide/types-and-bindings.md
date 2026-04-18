---
title: "Types And Bindings"
summary: "Understand explicit types, `let mut`, and why literal defaults matter in real Kern code."
order: 5
---

Kern has strong contextual typing, but it still expects you to keep low-level
meaning visible when it matters.

That becomes obvious as soon as you write loops and counters.

## A Validated Example

The following example was built and run successfully while writing this guide:

```kern
use std.io;

type Kind = enum {
    Zero,
    Positive: i32,
    Negative: i32,
};

fn classify(value: i32) Kind {
    if (value == 0) return .Zero;
    if (value > 0) return .{ Positive: value };
    return .{ Negative: value };
}

fn sum_to(limit: i32) i32 {
    let mut i = i32.{0};
    let mut total = i32.{0};

    for (; i < limit; i += i32.{1}) {
        total += i;
    }

    return total;
}

fn main() i32 {
    let total = sum_to(5);

    match (classify(total)) {
        .Zero => io.println("zero", .{}),
        .{ Positive: value } => io.println("positive {}", .{value,}),
        .{ Negative: value } => io.println("negative {}", .{value,}),
    };

    0
}
```

For the validated run, this printed:

```text
positive 10
```

## `let` Versus `let mut`

Bindings are immutable unless marked mutable:

```kern
let total = sum_to(5);
let mut i = i32.{0};
```

This follows a consistent rule in Kern:

- mutability belongs to storage and access paths
- it is not silently baked into every type spelling

## Why The `i32.{0}` Spelling Matters

While validating this example, the first draft used plain `0` and `1` in the
counter loop. That failed because the literals were inferred as `usize` in that
context, which then conflicted with the function's `i32` arithmetic.

That is a good example of Kern's design pressure:

- use contextual typing when it keeps the code shorter and still obvious
- make width/signedness explicit when they are part of the logic

So this:

```kern
let mut i = i32.{0};
let mut total = i32.{0};
for (; i < limit; i += i32.{1}) { ... }
```

is not noisy ceremony. It is a visible statement that this loop is operating in
`i32`, not in a default machine-sized unsigned counter.

## User-Defined Types

The example also shows two of Kern's most common user-defined types:

- `enum` for explicit tagged state
- `struct`-style storage inside payloads or aggregates

Even before getting into traits or pointers, Kern expects you to model state
with explicit types instead of "just use an integer and remember what it means".
