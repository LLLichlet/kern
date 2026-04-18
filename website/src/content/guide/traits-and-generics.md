---
title: "Traits And Generics"
summary: "Define trait contracts, implement them for concrete types, and constrain generic code with explicit `where` clauses."
order: 11
---

Kern's trait system is deliberately direct.

Traits describe callable contracts, `impl` blocks attach those contracts to
concrete types, and generic code states its requirements explicitly with
`where`.

## A Validated Example

The following example was built and run successfully while writing this guide:

```kern
use std.io;

type Add[Rhs] = trait {
    type Out;
    add: fn(Rhs) Out,
};

type Measure = trait {
    score: fn() i32,
};

type Vec2 = struct {
    x: i32,
    y: i32,
};

impl Vec2 : Add[i32] {
    type Out = Vec2;

    fn add(rhs: i32) Out {
        return Vec2.{ x: self.x + rhs, y: self.y + rhs };
    }
}

impl *Vec2 : Measure {
    pub fn score() i32 {
        return self.x + self.y;
    }
}

fn total[T](value: T) i32
    where *T: Measure,
{
    return value.&.score();
}

fn main() i32 {
    let start = Vec2.{ x: 3, y: 4 };
    let shifted = start.add(5);
    let dyn_value = *Measure.{ shifted.& };

    io.println("x={} score={} dyn={}", .{
        shifted.x,
        total(shifted),
        dyn_value.score(),
    });
    return 0;
}
```

For the validated run, this printed:

```text
x=8 score=17 dyn=17
```

## What This Shows

### Traits Can Declare Associated Types

`Add[Rhs]` declares an associated result type:

```kern
type Add[Rhs] = trait {
    type Out;
    add: fn(Rhs) Out,
};
```

The implementation then defines that associated type explicitly:

```kern
impl Vec2 : Add[i32] {
    type Out = Vec2;
    ...
}
```

Kern does not guess associated types for you.

### Generic Bounds Use `where`

The generic helper says exactly what it needs:

```kern
fn total[T](value: T) i32
    where *T: Measure,
```

That is an important Kern rule: generic parameter introduction and trait bounds
stay separate.

### Value And Pointer Trait Targets Are Different

The `Add[i32]` implementation is attached to `Vec2`.

The `Measure` implementation is attached to `*Vec2`.

Those are different types with different semantics, and Kern keeps that
distinction explicit.

### Trait Objects Use Explicit Constructor Syntax

This line constructs a trait object:

```kern
let dyn_value = *Measure.{ shifted.& };
```

That explicit construction is part of Kern's general style. Trait objects are
fat pointers with visible construction, not a hidden coercion that happens
without syntax.
