---
title: "Libraries And Runtime"
summary: "Understand `base`, `sys`, `rt`, `std`, and why hosted does not mean libc."
order: 9
---

Kern's library and runtime model is one of the most important parts of the
language story.

If you miss this split, it becomes easy to misread Kern as "just another
language with a standard library and some startup magic". That is not the
current model.

## The Four Public Layers

The current toolchain exposes four public layers:

- `base`: runtime-independent foundation facilities
- `sys`: OS and provider boundaries
- `rt`: startup and minimal runtime glue
- `std`: higher-level user-facing facilities built on `base` and `sys`

These layers are documented as public toolchain structure, not as an internal
implementation detail.

## Current Default Package Shapes

The current toolchain defaults are intentionally libc-free:

- `lib` targets default to `entry = "none"`, `libc = false`, `bundle = "std"`
- `bin`, `example`, and `test` targets default to `entry = "rt"`, `libc = false`, `bundle = "std"`

That means the normal starting point for Kern is not "turn libc on and build
everything on top of it".

The normal starting point is:

```toml
[runtime]
entry = "rt"
libc = false
bundle = "std"
```

## What `std` Is Not

`std` is not:

- a hidden prelude
- a mirror of every lower-level namespace
- a synonym for libc

Instead, `std` is ordinary Kern library code layered on top of the lower
boundaries.

## Hosted Does Not Mean Libc

Kern keeps two questions separate:

1. is there a hosted OS/process environment?
2. is libc linked?

Those are not the same question.

Kern's current model is:

- hosted process access belongs to `sys`
- startup glue belongs to `rt`
- high-level facilities belong to `std`
- libc is an optional external ABI compatibility choice, not the semantic foundation

That is why a simple runnable package can use:

```toml
[runtime]
entry = "rt"
libc = false
bundle = "std"
```

and still be a normal usable program shape in today's toolchain.

## Use `sys` By Default, Not libc

For hosted programs, the default Kern direction is:

- use `sys` for OS and process boundaries
- use `std` for higher-level facilities built on that Kern-owned boundary
- leave libc disabled unless you intentionally want a foreign C ABI surface

That distinction matters because libc is not what makes hosted Kern code
"real". Hosted support already exists through Kern's own layering.

In practice, a project usually reaches for libc only when it wants one of these
things on purpose:

- compatibility with an existing C library ABI
- linkage against a foreign library that expects libc to be present
- an explicitly chosen CRT startup path

So the practical question is not "should hosted code go through sys or libc?".

The practical rule is:

- Kern's own hosted path already goes through `sys`
- libc remains a separate opt-in compatibility interface

## Two Intentional Runtime Profiles

For most packages, the default profile is enough:

```toml
[runtime]
entry = "rt"
libc = false
bundle = "std"
```

If you intentionally want the C runtime path, choose it explicitly:

```toml
[runtime]
entry = "crt"
libc = true
bundle = "std"
```

The second form is not "more normal". It is simply a different runtime policy.

You do not enable libc in order to make `std` work.
You enable libc because you want that foreign interface.

## Why This Matters In Practice

When you import:

```kern
use std.io;
```

you are using the public `std` layer explicitly.

When you choose:

```toml
entry = "rt"
```

you are selecting startup ownership explicitly.

Those are separate axes on purpose. Kern wants runtime policy to stay visible
instead of being collapsed into one vague "normal build" mode.

The important directional rule is:

- `sys` is Kern's own system boundary
- `std` is built on that Kern boundary
- libc is something a project may opt into when it intentionally wants that foreign interface

That keeps the architecture flexible without making libc the base that the rest
of Kern is forced to stand on.

## Practical Takeaway

Keep these ideas separate:

- `std` is the normal high-level Kern library layer
- `rt` and `crt` are startup-policy choices
- libc is optional and compatibility-oriented
- hosted does not imply libc

If you remember only one rule from this chapter, make it this one:

start with Kern's own layers, then opt into libc only when you actually want
that foreign boundary.
