---
title: "Getting Started"
summary: "Create, check, build, and run a real minimal Kern package with the current toolchain."
order: 2
---

This chapter shows the smallest package shape that works with the current Kern
toolchain.

The commands and files below were validated against the current repository
toolchain before being written into the guide.

## Project Layout

Create a new directory with this shape:

```text
hello/
  Craft.toml
  src/
    main.rn
```

## `Craft.toml`

Use this package manifest:

```toml
[package]
name = "hello"
version = "0.1.0"
kern = "0.7.1"

[runtime]
entry = "rt"
libc = false
bundle = "std"

[[bin]]
name = "hello"
root = "src/main.rn"
```

What this says:

- the package is named `hello`
- the package opts into the current Kern toolchain version surface
- the runtime entry is the toolchain-owned `rt` startup path
- libc stays off
- the standard library bundle is enabled
- the runnable binary target lives at `src/main.rn`

## `src/main.rn`

Use this minimal program:

```kern
use std.io;

fn main() i32 {
    io.println("hello from guide", .{});
    0
}
```

This is intentionally small, but it already shows a few important Kern traits:

- `main` returns `i32`
- `std.io` is an ordinary imported module, not an implicit prelude
- formatting arguments use explicit aggregate syntax such as `.{}` or `.{value,}`

## First Commands

From the package root, run:

```bash
craft check
craft build
craft run
```

What each command does:

- `craft check` validates the package graph and runs semantic analysis without
  doing final code generation and linking
- `craft build` executes the derived compile and link actions
- `craft run` builds the selected runnable target and then executes it

For the validated example above, `craft run` produced:

```text
hello from guide
```

## Why Start With `craft`

You could invoke `kernc` directly, but most users should start with `craft`
because it owns:

- package discovery
- target selection
- runtime/library defaults from `Craft.toml`
- derived build actions

`kernc` is still important, but it is the compiler/linker driver beneath the
package layer rather than the package layer itself.

The next chapter explains that boundary explicitly.
