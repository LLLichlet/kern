---
title: "`craft.rn` And Normalization"
summary: "Use `craft.rn` as the pre-resolution normalization step that adjusts package planning state before the graph is finalized."
order: 17
---

`craft.rn` is not a generic build hook.

It is the package-planning phase that runs before canonical graph resolution is
finalized.

## A Validated Example

The following package was checked, built, and run successfully while writing
this guide:

```toml
[package]
name = "craftrn-guide"
version = "0.1.0"
kern = "0.7.0"
publish = false

[runtime]
entry = "rt"
libc = false
bundle = "std"
```

```kern
// craft.rn
use craft.plan;

pub fn craft(p: *mut plan.Plan) void {
    p.add_bin("demo", "src/main.rn");
}
```

```kern
// src/main.rn
use std.io;

fn main() i32 {
    io.println("crafted target", .{});
    return 0;
}
```

The validated run printed:

```text
crafted target
```

## Why This Example Matters

The manifest itself declared no `[[bin]]` targets.

But `craft check --verbose` reported:

```text
targets    lib no, bin 0, test 0, example 0, normalized 1
scripts    workspace craft no, package craft 1, build.rn 0
```

That is the clearest possible proof of what `craft.rn` is for:

- static manifest targets are one input
- `craft.rn` can normalize that planning state before resolution
- the normalized target set is what the rest of `craft` uses

## The Function Shape

Current `craft.rn` must use the planning API:

```kern
use craft.plan;

pub fn craft(p: *mut plan.Plan) void { ... }
```

This is not just style. It is the contract the tool expects.

## What To Use It For

The current implementation and docs support `craft.rn` for things such as:

- adding or adjusting targets
- changing source roots deterministically
- setting cfg / define values on the plan
- applying workspace policy to package planning

The important constraint is that this phase is part of canonical resolution.

So `craft.rn` should depend only on lock-stable, checked-in inputs. It is the
wrong place for host-specific or command-specific behavior.

## Practical Mental Model

Think of `craft.rn` as:

- before `Craft.lock`
- before final graph resolution
- about package normalization

If the change would alter what the package graph or normalized targets look
like, `craft.rn` is the candidate phase.
