# Chapter 1: Language Tour

This chapter is not a full spec. It is the shortest path to a working Kern
mental model.

## Kern In One Sentence

Kern is a systems language that wants strong abstractions without hidden
runtime policy.

That design goal explains most of the syntax:

- no implicit allocation
- explicit module wiring
- explicit conversions
- explicit mutability
- exhaustive control flow

## Your First Program

```kern
use std.io;

extern fn main(args: [][]u8) i32 {
    io.println("hello, {}!", .{"world",});
    0
}
```

Compile it with:

```bash
kernc --use-std --link-profile hosted examples/hello_world.rn -o hello
```

Important observations:

- `main` is declared with `extern fn`, which marks a C ABI entry boundary.
- string literals are `[]u8`
- the final expression can be the return value
- `--use-std` and `--link-profile hosted` are two separate choices:
  `std` injection and hosted C runtime linkage

## Bindings And Mutability

Kern separates storage mutability from type identity.

```kern
let x = i32.{10};
let mut y = i32.{20};
```

`let mut y` means the binding may be reassigned. It does not magically make all
reachable memory writable.

The same split shows up in pointers and slices:

- `*T` vs `*mut T`
- `[]T` vs `[]mut T`
- `obj.&` vs `obj..&`
- `arr.[a .. b]` vs `arr..[a .. b]`

If you remember only one thing, remember this: Kern makes write permission an
explicit part of the access path.

## Structs, Unions, And Enums

Named aggregates are straightforward:

```kern
type Point = struct {
    x: i32,
    y: i32,
};

let p = Point.{ x: 3, y: 4 };
```

Kern deliberately forbids field-name elision such as `Point.{x, y}`. The goal
is to keep initialization explicit and unambiguous.

Enums cover both plain tagged constants and payload-carrying algebraic cases:

```kern
type Switch = enum {
    Off = 0,
    On = 1,
    Error: i32,
};
```

Pattern matching is exhaustive:

```kern
fn is_on(v: Switch) bool {
    match (v) {
        .Off => false,
        .On => true,
        .Error: _ => false,
    }
}
```

This is one of Kern's main state-management tools. If a value encodes multiple
states, prefer an `enum` and a `match` over sentinel integers and hidden
control flow.

## Traits And Methods

Traits define callable interfaces. `impl` attaches methods to concrete receiver
types.

```kern
type Base = trait {
    foo: fn() i32,
};

impl *i32 : Base {
    pub fn foo() i32 {
        return self.*;
    }
}
```

The implicit receiver is `self`, but the receiver type is still explicit in the
`impl` header. That keeps method dispatch rules easy to locate in code.

For generic code, Kern also has language-owned builtin traits for operator
capabilities such as `Eq[T]` and `Add[T, T]`, plus marker traits such as
`Integer`, `SignedInteger`, `UnsignedInteger`, and `Float`. Marker traits are
classification only; they do not imply operators by themselves.

Trait objects are explicit values, not background magic. A common pattern is:

```kern
let value = i32.{41};
let object = *Base.{ value.& };
```

Kern also supports trait-object upcasts when the supertrait relation is known.
Short-circuit `and` / `or`, assignment forms, address-of, dereference, and `#`
remain language-owned semantics rather than overloadable trait syntax.

## Closures

Kern closures are intentionally explicit about capture state.

```kern
let sum = .[](a: i32, b: i32) i32 {
    return a + b;
};

let base = i32.{100};
let add_base = .[base](x: i32) i32 {
    return base + x;
};
```

The important split is:

- `.[]` is stateless and can boundary-convert to a plain function pointer
- `.[...]` carries explicit captured state

There are no hidden captures. The closure syntax itself tells you exactly what
runtime state exists.

## Modules

Kern uses an explicit module tree.

Typical layout:

```text
src/
  main.rn
  net/
    init.rn
    packet.rn
```

Inside `main.rn`:

```kern
mod net;
use net.packet;
```

Inside `net/init.rn`:

```kern
pub mod packet;
```

This is closer to "explicit namespace tree" than to filesystem magic. That is a
good default for compiler and tooling predictability.

## Five Habits That Make Kern Easier

1. Write the type where ambiguity would otherwise exist.
2. Use `enum` plus `match` to model state transitions.
3. Treat pointer mutability and binding mutability as different questions.
4. Keep closure capture lists small and visible.
5. Reach for `docs/design.md` only after you already know which concept you are
   clarifying.

After this chapter, move directly to the `kernc` workflow. Learning Kern is much
easier once you can compile small experiments quickly.
