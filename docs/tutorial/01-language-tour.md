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

fn main() i32 {
    io.println("hello, {}!", .{"world",});
    0
}
```

Compile it with:

```bash
kernc --library-bundle std --runtime-entry crt --runtime-libc yes examples/hello_world.rn -o hello
```

Important observations:

- `main` is a root entry definition with a fixed signature such as `fn main() i32`
  or `fn main(argc: i32, argv: **u8) i32` when `runtime_entry != none`
- string literals are `[]u8`
- the final expression can be the return value
- `--library-bundle std`, `--runtime-entry crt`, and `--runtime-libc yes` are
  separate choices: library injection, startup ownership, and libc linkage

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

Array and SIMD literals use the same `Type.{ ... }` family as other explicit
constructors, but single-element containers still need an array comma:

```kern
let one = [1]mut u8.{ 7, };
let zeros = [16]mut u8.{ 0; 16 };
let scratch = [256]mut u8.{undef};
```

Without the comma, `Type.{ value }` is parsed as scalar initialization, not a
single-element array literal.

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

## SIMD

SIMD values are builtin types, not library wrappers and not array aliases.

```kern
let a = f32x4.{ 1.0, 2.0, 3.0, 4.0 };
let b = f32x4.{ 5.0, 6.0, 7.0, 8.0 };
let mask = a < b; // boolx4

let lane = a.[2];

let mut out = a;
out.[2] = 9.0;

if (@simdAny(mask)) {
    out = @simdSelect(mask, b, out);
}
let mask_bits = @simdBitmask(mask);

let mixed = @simdShuffle(a, b, [4]u32.{ 0, 5, 2, 7 });
let swizzled = @simdSwizzle(a, [4]u32.{ 3, 0, 2, 1 });
let rev = @simdReverse(a);
let rot = @simdRotateLeft(a, 1);
let inter = @simdInterleaveLo(a, b);
let zip = @simdZipLo(a, b);
let cat = @simdConcatLo(a, b);
let de = @simdDeinterleaveLo(inter, @simdInterleaveHi(a, b));
let unzip = @simdUnzipHi(inter, @simdInterleaveHi(a, b));
let lo_half = @simdLowHalf[f32x2](a);
let hi_half = @simdHighHalf[f32x2](b);
let stitched = @simdWithLowHalf[f32x4](b, lo_half);
let total = @simdReduceAdd(mixed);
let mags = @simdAbs(f32x4.{ -1.0, 2.0, -0.0, -4.0 });
let lo = @simdMin(i32x4.{ 9, 2, -4, 8 }, i32x4.{ 3, 7, -5, 8 });
let hi = @simdMax(a, b);
let clipped = @simdClamp(a, f32x4.{ 0.0, 1.5, 1.0, 0.0 }, f32x4.{ 3.0, 3.0, 3.0, 3.0 });
let roots = @simdSqrt(f32x4.{ 1.0, 4.0, 9.0, 16.0 });
let lowered = @simdFloor(f32x4.{ 1.9, -1.2, 2.0, -0.0 });
let raised = @simdCeil(f32x4.{ 1.1, -1.8, 2.0, -0.0 });
let chopped = @simdTrunc(f32x4.{ 1.9, -1.8, 2.0, -0.0 });
let rounded = @simdRound(f32x4.{ 1.4, 1.6, -1.4, -1.6 });
let ones = @simdSplat[i32x4](1);
let as_float = @simdCast[f32x4](ones);
let bits = @simdBitcast[u32x4](as_float);
let pop = @popCount(u32x4.{ 1, 3, 7, 15 });
let data = [4]mut f32.{ 10.0, 20.0, 30.0, 40.0 };
let mask2 = boolx4.{ true, false, true, false };
let partial = @simdMaskedLoad[f32x4](data.[0]..&, mask2, f32x4.{ 0.0, 0.0, 0.0, 0.0 }, 4);
@simdMaskedStore(data.[0]..&, mask2, partial, 4);
let idx = [4]usize.{ 3, 0, 2, 1 };
let permuted = @simdGather[f32x4](data.[0]..&, idx.[0].&);
@simdScatter(data.[0]..&, idx.[0].&, permuted);
let masked = @simdMaskedGather[f32x4](data.[0]..&, idx.[0].&, mask2, f32x4.{ -1.0, -1.0, -1.0, -1.0 });
@simdMaskedScatter(data.[0]..&, idx.[0].&, mask2, masked);
```

The important split is:

- `f32x4`, `i32x4`, `boolx4`, and similar names are language primitives.
- SIMD integer primitives cover the full builtin integer family, including `isizexN`, `usizexN`, `i128xN`, and `u128xN`.
- `.[]` on SIMD means lane access.
- SIMD comparisons return `boolxN`, not scalar `bool`.
- `@simdAny`, `@simdAll`, `@simdBitmask`, and `@simdSelect` are compiler intrinsics for mask reduction, mask extraction, and lane-wise selection.
- `@simdShuffle` is the general two-input lane permutation primitive, while `@simdSwizzle` is the single-input shorthand.
- `@simdReverse`, `@simdRotateLeft`, `@simdRotateRight`, `@simdInterleaveLo`, `@simdInterleaveHi`, `@simdZipLo`, `@simdZipHi`, `@simdConcatLo`, `@simdConcatHi`, `@simdDeinterleaveLo`, `@simdDeinterleaveHi`, `@simdUnzipLo`, and `@simdUnzipHi` are higher-level rearrangement helpers built on top of explicit shuffle semantics.
- `@simdLowHalf`, `@simdHighHalf`, `@simdWithLowHalf`, and `@simdWithHighHalf` let you split and stitch vectors without treating SIMD as arrays.
- `@simdAbs`, `@simdMin`, `@simdMax`, and `@simdClamp` cover common lane-wise numeric operations that do not have dedicated expression syntax.
- `@simdSqrt`, `@simdFloor`, `@simdCeil`, `@simdTrunc`, and `@simdRound` cover common lane-wise floating-point math that does not have dedicated expression syntax.
- Existing bit intrinsics such as `@popCount` and `@clz` also work lane-wise on SIMD integer vectors.
- `@simdSplat`, `@simdCast`, `@simdBitcast`, `@simdShuffle`, `@simdReduce...`, `@simdLoad`, `@simdStore`, `@simdMaskedLoad`, `@simdMaskedStore`, `@simdGather`, `@simdScatter`, `@simdMaskedGather`, and `@simdMaskedScatter` cover the operations that do not fit ordinary expression syntax cleanly.

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
