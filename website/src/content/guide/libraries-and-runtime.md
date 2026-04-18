---
title: "Libraries And Runtime"
summary: "Understand `base`, `sys`, `rt`, `std`, and why hosted does not mean libc."
order: 7
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
