---
title: "SIMD"
summary: "Use builtin vector types and the `@simd...` family for explicit lane-wise math, masks, shuffles, and memory operations."
order: 24
---

Kern treats SIMD as a real language feature.

You do not start from vendor headers or platform-specific builtin names.
You start from builtin vector types such as:

- `i32x4`
- `u8x16`
- `f32x8`
- `boolx16`

Then you combine normal lane-wise operators with the explicit `@simd...`
intrinsic family.

## A Validated Example

The following package was built and run successfully in a temporary directory
while writing this guide:

```kern
use std.io;

fn main() i32 {
    let data = [8]i32.{ 1, 2, 3, 4, 10, 20, 30, 40 };
    let left = @simdLoad[i32x4](data.[0].&, 4);
    let right = @simdLoad[i32x4](data.[4].&, 4);
    let mixed = @simdShuffle(left, right, [4]u32.{ 0, 5, 2, 7 });
    let clipped = @simdClamp(
        mixed,
        @simdSplat[i32x4](0),
        @simdSplat[i32x4](25),
    );

    @simdStore(data.[0]..&, clipped, 4);

    let total = @simdReduceAdd(clipped);
    let mask = @simdBitmask(mixed > @simdSplat[i32x4](15));

    io.println("total={} mask={} first={} last={}", .{
        total,
        mask,
        data.[0],
        data.[3],
    });

    if (total != 49) {
        return 1;
    }
    if (mask != usize.{10}) {
        return 2;
    }
    if (data.[1] != 20 or data.[3] != 25) {
        return 3;
    }

    return 0;
}
```

The validated run printed:

```text
total=49 mask=10 first=1 last=25
```

## What This Shows

### Vector Values Use Builtin Lane Types

This declaration:

```kern
let data = [8]i32.{ 1, 2, 3, 4, 10, 20, 30, 40 };
```

is ordinary scalar storage.

These calls:

```kern
let left = @simdLoad[i32x4](data.[0].&, 4);
let right = @simdLoad[i32x4](data.[4].&, 4);
```

turn parts of that scalar storage into vector values with four `i32` lanes.

### Memory Boundaries Stay Explicit

SIMD memory operations are not hidden behind array auto-vectorization syntax.

You say exactly when vector memory happens:

- `@simdLoad`
- `@simdStore`
- `@simdMaskedLoad`
- `@simdMaskedStore`
- `@simdGather`
- `@simdScatter`

The alignment argument is part of the call and is an explicit promise from the
source program. It must be a non-zero power of two.

### Rearrangement Uses Named Helpers

This line:

```kern
let mixed = @simdShuffle(left, right, [4]u32.{ 0, 5, 2, 7 });
```

builds a new vector by selecting lanes from `left ++ right`.

That is Kern's preferred shape for lane rearrangement:

- explicit input vectors
- explicit indices
- explicit result type from the operands

Other current helpers include `@simdSwizzle`, `@simdReverse`,
`@simdRotateLeft`, `@simdInterleaveLo`, and half-extraction / half-insertion
operations.

### Scalar Broadcast Uses `@simdSplat`

This chapter used:

```kern
@simdSplat[i32x4](15)
```

to replicate one scalar into every lane.

That makes comparisons and clamps readable without inventing special vector
literal shorthand for repeated values.

### Masks Stay Vector-Typed Until You Collapse Them

This expression:

```kern
mixed > @simdSplat[i32x4](15)
```

produces a `boolx4`.

That matters because SIMD control state in Kern is still typed data, not hidden
branch metadata.

Only when the program intentionally wants a scalar summary does it collapse the
mask:

```kern
let mask = @simdBitmask(...)
```

`@simdBitmask` returns a `usize` whose bits correspond to the mask lanes.

### Reductions Are Named Operations

This line:

```kern
let total = @simdReduceAdd(clipped);
```

is a horizontal reduction.

Kern keeps that boundary explicit instead of treating lane-wise arithmetic and
scalar reduction as the same kind of operation.

## Diagnostic Rules Worth Remembering

The current compiler already enforces several important SIMD rules:

- `@simdCast` requires matching lane counts
- `@simdBitcast` requires the same total size
- rotation amounts must be compile-time constants
- some rearrangement helpers require even lane counts
- load/store alignment arguments must be valid power-of-two constants

That keeps many mistakes in semantic analysis instead of leaving them to
backend crashes or mysterious runtime behavior.

## Relationship To Other Intrinsics

The general intrinsic chapter covered compiler-owned operations such as
`@sizeOf`, `@alignOf`, `@popCount`, `@clz`, and atomics.

Those bit intrinsics also extend lane-wise to integer SIMD vectors.
SIMD builds on the same explicit design direction, but with vector types and
vector-specific operations added on top.

## Practical Takeaway

Think about Kern SIMD this way:

- vector types are first-class builtin types
- lane-wise arithmetic uses normal operators
- loads, stores, shuffles, and reductions stay explicit
- masks are values, not hidden control-flow state
- scalarization happens only when you ask for it

This keeps SIMD operations explicit without folding platform-specific
intrinsic conventions into ordinary source code.
