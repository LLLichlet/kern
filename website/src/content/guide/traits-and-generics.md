---
title: "Traits And Generics"
summary: "Define trait contracts, implement them for concrete types, and constrain generic code with explicit `where` clauses."
order: 20
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

If a trait requires an associated type, the corresponding `impl` must define
it explicitly. Leaving it out is a semantic error rather than something the
compiler tries to infer from method bodies.

### Generic Bounds Use `where`

The generic helper says exactly what it needs:

```kern
fn total[T](value: T) i32
    where *T: Measure,
```

That is an important Kern rule: generic parameter introduction and trait bounds
stay separate.

The right-hand side of a `where` bound must name a trait:

```kern
fn ok[T](value: T) void
    where T: Printable,
{
    let _ = value;
}
```

This is intentionally rejected:

```kern
fn bad[A](a: A) A
    where A: A,
{
    return a;
}
```

The current compiler reports that `where`-clause bounds must name a trait, and
it points out the non-trait right-hand side directly.

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

## Supertraits And Upcasts Stay Explicit

Trait hierarchies work, but Kern keeps both the hierarchy and the runtime
packaging visible.

While writing this guide, the current toolchain successfully ran this pattern:

```kern
type Base = trait { get: fn() i32, };
type Derived: Base = trait { bump: fn(i32) i32, };

impl *i32 : Base {
    pub fn get() i32 { return self.*; }
}

impl *i32 : Derived {
    pub fn bump(v: i32) i32 { return self.* + v; }
}

fn main() i32 {
    let value = i32.{7};
    let derived = *Derived.{ value.& };
    let base = *Base.{ derived };
    return base.get() + derived.bump(5);
}
```

That shows the current model clearly:

- supertraits are declared in the trait header
- a trait object is still constructed explicitly
- upcasting a trait object to a parent trait stays explicit at construction time

## Generic Function Items Need Explicit Instantiation

Kern allows generic function items to appear as values only after you make them
concrete.

This validated pattern worked while writing the guide:

```kern
fn id[T](x: T) T {
    return x;
}

fn main() i32 {
    let f = id[i32];
    return f(11);
}
```

The important rule is that Kern does not treat a still-polymorphic function as
an ordinary value.

So this is rejected:

```kern
let f = id;
```

and the current compiler explains why:

- the generic function cannot be used as a value without explicit instantiation
- you should write `id[...]` with concrete generic arguments

That fits Kern's general direction: generic intent stays explicit instead of
being recovered later from the left-hand side.

## Impl Prerequisites Are Real Prerequisites

An `impl` with a `where` clause does not mean "assume this impl exists and sort
it out later".

It means the impl is available only when that prerequisite is already
satisfied.

For example:

```kern
type Marker = trait {};
type Need = trait {};

impl[T] T : Marker
    where T: Need,
{}
```

does not make every `T` a `Marker`.
It makes `T: Marker` available only when `T: Need` is already true.

## Self-Referential Impl Bounds Are Rejected

Kern also rejects impls that try to prove themselves by assuming themselves.

This is intentionally invalid:

```kern
type Forge[T] = trait {
    make: fn() T,
};

type Carrier[T] = struct {};

impl[T] Carrier[T] : Forge[T]
    where Carrier[T]: Forge[T],
{
    fn make() T {
        return self.make();
    }
}
```

The current compiler diagnoses this directly as an impl that requires itself in
its own `where` clause.

That matters because the alternative is not "more expressive generics". The
alternative is letting unsound self-justifying impls leak through the type
system.

## Practical Takeaway

Keep four rules in mind:

- associated types must be defined explicitly by each matching `impl`
- `where` clauses must bind concrete type derivations to actual traits
- generic function values need explicit instantiation such as `id[i32]`
- impl prerequisites are enforced, and self-justifying impls are rejected

If you stay explicit about which type, which trait, and which instantiation you
mean, Kern's trait system stays predictable.
