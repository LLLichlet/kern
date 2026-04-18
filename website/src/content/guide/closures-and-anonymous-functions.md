---
title: "Closures And Anonymous Functions"
summary: "Use `.[...]` closure syntax, distinguish closure state from `*Fn` fat pointers, and rely on explicit boundary conversions instead of hidden capture magic."
order: 22
---

Kern closures are explicit in exactly the places that matter.

They are not magical opaque objects with invisible heap allocation or hidden
capture rules.

The language keeps two things separate:

- the physical closure state value
- the callable closure interface `*Fn(...) Ret` or `*mut Fn(...) Ret`

## A Validated Example

The following package was built and run successfully while writing this guide:

```kern
use std.io;

fn use_closure(cb: *Fn() i32) i32 {
    return cb();
}

fn use_mut_closure(cb: *mut Fn() void) void {
    cb();
}

fn main() i32 {
    let mut calls = i32.{0};
    let first = use_closure(.[ptr = calls..&]() i32 {
        ptr.* += 1;
        return ptr.*;
    });

    let mut counter = i32.{10};
    let mut closure = .[ptr = counter..&]() void {
        ptr.* += 5;
    };
    use_mut_closure(closure);

    io.println("first={} counter={}", .{ first, counter, });
    return if (first == 1 and counter == 15) 0 else 1;
}
```

The validated run printed:

```text
first=1 counter=15
```

## What This Shows

### Capture Lists Are Explicit

Kern closure syntax is:

```kern
.[captures](args) ReturnType { ... }
```

Capture entries are explicit bindings.

This example used:

```kern
.[ptr = counter..&]() void { ... }
```

That means the closure captured exactly one thing:

- `ptr`
- initialized from `counter..&`

Nothing here is inferred from body usage.

### Closures Start As Anonymous State Values

When you write a closure expression, the immediate value is not already a
`*Fn`.

It is an anonymous closure-state value owned by the compiler's type system.

That state can then boundary-convert into:

- `*Fn(...) Ret` for immutable callable access
- `*mut Fn(...) Ret` for mutable callable access

when a context explicitly expects that interface.

### Mutable And Immutable Closure Interfaces Are Different

These two signatures are intentionally distinct:

```kern
fn use_closure(cb: *Fn() i32) i32
fn use_mut_closure(cb: *mut Fn() void) void
```

That matches the rest of Kern's pointer model.

- `*Fn` means the callable environment is observed immutably
- `*mut Fn` means the callable environment may be mutated through the callback

## Stateless Closures Can Act Like Plain Function Values

A strictly empty capture list:

```kern
.[](value: i32) bool { return value > 0; }
```

has no captured state.

That matters because the compiler can treat it more like an ordinary stateless
call target instead of a stateful closure environment.

Function items participate in the same broad callback world.

For example, current Kern code can pass a named function item where a closure
callback shape is expected, as long as the callable signature matches.

## The Important Mental Split

Do not blur these two ideas together:

- a closure expression creates a concrete anonymous state value
- a `*Fn` or `*mut Fn` is the callable fat-pointer interface around that state

Kern keeps that distinction visible because it avoids a lot of hidden runtime
policy:

- no secret heap allocation
- no invisible boxing
- no pretend "everything is just one closure object" model

## Advanced Note: Explicit Escape Exists

If a closure must outlive its stack home, the current language model expects
you to make that escape explicit.

That means:

- allocate storage explicitly
- move or copy the closure state there
- then build the closure fat pointer intentionally

This is not yet a full heap-allocation tutorial chapter, but the direction is
important: escaping closure state is explicit systems work, not a hidden
language side effect.

## Practical Takeaway

Keep four rules in mind:

- closure captures are explicit
- closure expressions start as anonymous state values
- `*Fn` and `*mut Fn` are distinct callable interfaces
- escaping closure state is explicit instead of automatic

If you keep those boundaries straight, Kern closures stay predictable and
low-level in the good sense.
