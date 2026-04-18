---
title: "Const And Compile-Time"
summary: "Use `const` and `const fn` for real compile-time evaluation without inventing a second function model."
order: 21
---

In Kern, compile-time evaluation is part of the language itself.

It does not depend on `std`, and it does not introduce a separate "const-only"
ABI or function flavor.

## A Validated Example

The following example was built and run successfully while writing this guide:

```kern
use std.io;

type Pair = struct {
    left: i32,
    right: i32,
};

const fn inc(value: usize) usize {
    return value + usize.{1};
}

impl Pair {
    pub const fn sum() i32 {
        return self.left + self.right;
    }
}

const TABLE = [inc(usize.{3})]u8.{ 1, 2, 3, 4 };
const TOTAL = Pair.{ left: 5, right: 7 }.sum() + (TABLE.[3] as i32);

fn main() i32 {
    io.println("len={} total={} next={}", .{
        #TABLE,
        TOTAL,
        inc(#TABLE),
    });
    return 0;
}
```

For the validated run, this printed:

```text
len=4 total=16 next=5
```

## What This Example Shows

### `const fn` Can Feed Constant Contexts

This line uses `inc(...)` in an array length:

```kern
const TABLE = [inc(usize.{3})]u8.{ 1, 2, 3, 4 };
```

That proves the function is valid in a compile-time-required position.

Current constant contexts include things like:

- global `const` initializers
- array lengths
- constant subexpressions used by other compile-time-required constructs
- intrinsic operands that the language specifies as compile-time constants

### `pub const fn` Methods Work Inside `impl`

This method:

```kern
impl Pair {
    pub const fn sum() i32 {
        return self.left + self.right;
    }
}
```

was then used to form another constant:

```kern
const TOTAL = Pair.{ left: 5, right: 7 }.sum() + (TABLE.[3] as i32);
```

So Kern's const model is not limited to free functions.

### The Same Function Still Works At Runtime

The call:

```kern
inc(#TABLE)
```

happens during normal execution in `main`.

That is the key design point: `const fn` is still a normal function item. It is
eligible for compile-time interpretation, but it is not trapped inside a second
execution universe.

## Compile-Time Control Flow Is Real Control Flow

Kern's constant evaluator is not limited to trivial arithmetic.

While writing this guide, the current toolchain successfully ran a package that
used `let ... else` inside `const fn` and then consumed those results in a
constant table:

```kern
type Option[T] = enum {
    None,
    Some: T,
};

const fn unwrap_or(value: Option[i32], fallback: i32) i32 {
    let .{ Some: inner } = value else return fallback;
    return inner;
}

const PICKED = unwrap_or(Option[i32].{ Some: 9 }, 5);
const FALLBACK = unwrap_or(Option[i32].None, 5);
const TABLE = [usize.{3}]i32.{ PICKED, FALLBACK, unwrap_or(Option[i32].{ Some: 2 }, 0) };
```

The validated run printed:

```text
picked=9 fallback=5 last=2
```

So the practical model is:

- `const fn` may use ordinary local bindings
- `const fn` may branch and return early
- enum pattern matching and destructuring can participate in constant evaluation

Kern is not inventing a second miniature expression language here.

## Constant Evaluation Is Strict, Not Best-Effort

The current compiler is intentionally strict about invalid constant behavior.

It does not try to "sort of evaluate" bad code and hope for the best.

For example, while writing this guide the current compiler correctly rejected an
array length that does not fit into `usize`:

```kern
fn main() i32 {
    let _ = [18446744073709551616]u8.{ undef };
    return 0;
}
```

with a structured error explaining that the constant expression is too large
for that `usize`-like context.

This is an important design point:

- compile-time integer binding is checked against the destination type
- array lengths and similar positions do not silently wrap
- invalid constant arithmetic is reported as a user error, not left to a panic or backend failure

## Layout Checks Also Fail Early

Some errors are discovered during compile-time semantic and layout validation
even if they are not written inside a `const` item directly.

For example, this is rejected:

```kern
type A = struct {
    b: B,
};

type B = struct {
    a: A,
};
```

The current compiler reports that the type recursively contains itself by value
and prints a chain such as:

```text
recursive layout chain: A -> B -> A
```

That is the behavior users want from a systems language: the compiler explains
why the layout is impossible instead of overflowing or producing nonsense.

## Mental Model

Use `const` and `const fn` when a value must be known before lowering and code
generation.

That includes cases such as:

- global constants
- array lengths
- other constant expressions formed from `const fn`

Kern's rule is simple: if compile-time knowledge is required, the language uses
the ordinary semantic model plus explicit permission through `const fn`.

## Practical Takeaway

Keep these rules in mind:

- `const fn` is still an ordinary function, but it is also allowed in constant contexts
- constant evaluation can use real control flow such as `if`, `match`, and `let ... else`
- compile-time errors are diagnosed structurally instead of being treated as optimizer accidents
- impossible layouts and out-of-range literals are rejected early and clearly

The overall model is simple: compile-time Kern is still Kern, just with an
explicit execution boundary and stricter admission rules.
