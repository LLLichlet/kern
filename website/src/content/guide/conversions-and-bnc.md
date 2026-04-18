---
title: "Conversions And Boundary Natural Conversion"
summary: "Use explicit `as` casts for numeric and pointer reinterpretation, and rely on Boundary Natural Conversion only where Kern intentionally provides zero-cost boundary packaging."
order: 19
---

Kern is strict about conversions on purpose.

Most type changes are explicit.

At the same time, Kern also provides a small set of zero-cost boundary
conversions where the compiler already has exact metadata and the conversion is
part of the language's intended ergonomics.

Kern calls that Boundary Natural Conversion, or BNC.

## A Validated Example

The following package was built and run successfully while writing this guide:

```kern
use std.io;

type Vec2 = struct {
    x: i32,
    y: i32,
};

fn sum(values: []i32) i32 {
    let mut i = usize.{0};
    let mut total = i32.{0};
    for (; i < #values; i += usize.{1}) {
        total += values.[i];
    }
    return total;
}

fn dot(value: struct { x: i32, y: i32 }) i32 {
    return value.x * value.y;
}

fn main() i32 {
    let arr = [4]i32.{ 1, 2, 3, 4 };
    let point = Vec2.{ x: 6, y: 7 };
    let addr = (point.& as usize);
    let roundtrip = (addr as *Vec2);

    io.println("sum={} dot={} roundtrip={}", .{
        sum(arr),
        dot(point),
        roundtrip.*.x + roundtrip.*.y,
    });

    return if (sum(arr) == 10 and dot(point) == 42 and (roundtrip.*.x + roundtrip.*.y) == 13) 0 else 1;
}
```

The validated run printed:

```text
sum=10 dot=42 roundtrip=13
```

## What This Shows

### Array-To-Slice Decay Is BNC

This call:

```kern
sum(arr)
```

worked even though:

- `arr` is `[4]i32`
- `sum` expects `[]i32`

That is a Boundary Natural Conversion.
The compiler already knows the array address and length, so it can package the
slice boundary value without asking for an explicit cast.

### Named-Struct To Anonymous-Struct Decay Is Also BNC

This call:

```kern
dot(point)
```

worked even though:

- `point` is `Vec2`
- `dot` expects `struct { x: i32, y: i32 }`

That is another intentional BNC pathway.

Kern allows this when the structural shape and ABI expectations match.

### Pointer / Integer Reinterpretation Uses `as`

This part of the example was deliberately explicit:

```kern
let addr = (point.& as usize);
let roundtrip = (addr as *Vec2);
```

This is not BNC.
This is a normal explicit cast through `as`.

That distinction matters:

- BNC is a small compiler-owned boundary-packaging mechanism
- `as` is the ordinary explicit conversion operator

## What `as` Is For

Use `as` when you mean an explicit conversion such as:

- numeric conversions
- integer / float conversions
- pointer reinterpretation
- pointer / integer round-trips

Current Kern intentionally does not use `as` for everything.

In particular, `as` is not the mechanism for constructing trait objects or
other fat-pointer interfaces that require explicit packaging syntax.

## What BNC Is For

BNC exists only on specific language-owned pathways where the compiler can
prove the metadata and package it without ambiguity.

The important current pathways are:

- array to slice decay
- stateless closure to plain function boundary
- named structural types to matching anonymous structural types
- trait-object upcasts across a supertrait boundary

This is not "implicit conversion everywhere".
It is a very bounded set of conversions that the language intentionally treats
as zero-cost boundary packaging.

## ABI Boundaries Still Matter

BNC is not allowed to cross incompatible ABI/layout contracts just because two
types look similar on paper.

For example, native and `extern` structural layouts are not interchangeable by
default.

That is an important safety rule:

- structural convenience never overrides layout truth
- ABI-sensitive boundaries stay explicit

## Practical Takeaway

Use this mental model:

- if you are changing representation explicitly, use `as`
- if Kern already owns a specific zero-cost boundary packaging rule, BNC may apply
- do not expect BNC to erase ABI or layout differences

The goal is explicit conversion rules without turning the type system into
"convert whatever fits".
