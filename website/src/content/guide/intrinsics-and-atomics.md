---
title: "Intrinsics And Atomics"
summary: "Use compiler-owned `@...` intrinsics for type queries, bit operations, and explicit atomic/fence primitives without pretending they are ordinary library functions."
order: 23
---

Intrinsics are compiler-owned operations.

They are written like calls, but they are not ordinary library functions.

That is why Kern spells them with `@...` and keeps them clearly separate from
user-defined APIs.

## A Validated Runtime Example

The following package was built and run successfully while writing this guide:

```kern
use std.io;

fn main() i32 {
    let bits = @popCount(u32.{0b101101});
    let lead = @clz(u32.{8});
    let trail = @ctz(u32.{40});

    io.println("size={} align={} bits={} lead={} trail={}", .{
        @sizeOf[u64](),
        @alignOf[u64](),
        bits,
        lead,
        trail,
    });

    return if (bits == 4 and lead == 28 and trail == 3) 0 else 1;
}
```

The validated run printed:

```text
size=8 align=8 bits=4 lead=28 trail=3
```

## What This Shows

### Type Queries Are Compile-Time Intrinsics

These two calls:

```kern
@sizeOf[u64]()
@alignOf[u64]()
```

are compile-time type queries.

They do not depend on `std`, and they are not optimizer hints. They are part of
the language's own compile-time model.

### Bit Intrinsics Work On Integer Categories

This example also used:

```kern
@popCount(...)
@clz(...)
@ctz(...)
```

These intrinsics operate on integer types as a type family.

That does not mean the marker trait itself is the source of all operator
semantics. It simply means the compiler checks that these intrinsics are being
used on valid integer-shaped inputs.

## Atomic Operations Are Also Compiler-Owned

Kern's atomic operations follow the same design:

- explicit intrinsic names
- explicit generic target type
- explicit memory ordering constant

While writing this guide, the current toolchain successfully emitted LLVM IR
for this atomic example:

```kern
const RELAXED = 0;
const ACQUIRE = 1;
const RELEASE = 2;
const SEQ_CST = 4;

fn main() i32 {
    let mut value = usize.{0};
    @atomicStore[usize](value..&, 7, RELEASE);
    let _ = @atomicLoad[usize](value.&, ACQUIRE);
    let _ = @atomicRmwAdd[usize](value..&, 5, RELAXED);
    let _ = @atomicXchg[usize](value..&, 99, SEQ_CST);
    return 0;
}
```

The emitted LLVM IR contained atomic operations such as:

```text
store atomic ...
load atomic ...
atomicrmw add ...
atomicrmw xchg ...
```

That confirms the current frontend and lowering path is carrying these
operations as real atomics rather than disguising them as plain calls.

The standard library also exposes named wrappers for these ordering constants,
but the important language rule is lower-level than the library surface: the
compiler consumes a stable compile-time ordering ABI.

## Ordering Constants Must Be Compile-Time Constants

Atomic orderings are not free-form runtime values.

The compiler requires ordering operands such as:

- `RELAXED`
- `ACQUIRE`
- `RELEASE`
- `ACQ_REL`
- `SEQ_CST`

to be compile-time constants in the expected intrinsic ABI.

If you pass a runtime variable where a constant ordering is required, Kern
rejects it during semantic checking.

## Practical Boundary

Use intrinsics when the operation is genuinely compiler-owned:

- type-size or alignment queries
- bit population / leading-zero / trailing-zero queries
- fences and atomic memory operations
- other target-sensitive primitives the language treats as builtin machinery

Do not think of these as "small std helpers".

They sit below that layer.

## Practical Takeaway

Keep these rules in mind:

- `@...` intrinsics are part of the language/compiler boundary
- type-query intrinsics are compile-time by nature
- bit intrinsics require valid integer-shaped inputs
- atomic intrinsics require explicit type arguments and explicit compile-time orderings

That explicitness is the point. Kern wants low-level operations to stay visible
instead of being smuggled through vague helper APIs.
