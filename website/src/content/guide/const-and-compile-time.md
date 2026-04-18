---
title: "Const And Compile-Time"
summary: "Use `const` and `const fn` for real compile-time evaluation without inventing a second function model."
order: 12
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

## Mental Model

Use `const` and `const fn` when a value must be known before lowering and code
generation.

That includes cases such as:

- global constants
- array lengths
- other constant expressions formed from `const fn`

Kern's rule is simple: if compile-time knowledge is required, the language uses
the ordinary semantic model plus explicit permission through `const fn`.
