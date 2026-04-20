---
title: "Freestanding And Linking"
summary: "Build kernels and other freestanding artifacts with `craft` or `kernc`, own `_start`, and attach custom linker scripts explicitly."
order: 17
---

This chapter covers the practical path for kernels, boot stages, and other
freestanding artifacts.

The key rule is that freestanding policy stays explicit instead of hiding
behind a special build mode.

## `craft` First

For package-shaped projects, start with `craft` and make the runtime contract
visible in `Craft.toml`:

```toml
[package]
name = "kernel"
version = "0.1.0"
kern = "0.7.0"

[runtime]
entry = "none"
libc = false
bundle = "base"

[[bin]]
name = "kernel"
root = "src/main.rn"
```

That means:

- startup is owned by the package, not by `rt` or `crt`
- libc is off
- the package gets the freestanding `base` bundle instead of the hosted `std` path

## Export `_start` Yourself

With `entry = "none"`, there is no special `main` requirement.

Export the symbol you actually want:

```kern
#[export_name("_start")]
fn kmain() void {
    for (;;) {}
    @unreachable();
}
```

This is the current valid direction for minimal kernel-style entry code:

- no program `main`
- explicit `_start`
- explicit inline assembly template as one string literal

## Attach A Linker Script In `build.rn`

If the linker needs a custom script, keep that in `build.rn`:

```kern
use craft.builder;

pub fn build(b: *mut builder.Builder) void {
    b.link_arg_path("-T", "kernel.ld");
}
```

`link_arg_path("-T", ...)` resolves relative paths from the package root,
verifies that the file exists, and pushes the correct `-T <absolute-path>` pair
into the link step.

The actual commands stay simple:

```bash
craft check
craft build
```

## When To Drop To `kernc`

Use `kernc` directly when you want a one-off compile or link command rather
than package orchestration.

A minimal freestanding direct build looks like:

```bash
kernc \
  --runtime-entry none \
  --runtime-libc no \
  --library-bundle base \
  --entry-symbol _start \
  src/main.rn \
  -o kernel.bin
```

To attach a linker script directly:

```bash
kernc \
  --runtime-entry none \
  --runtime-libc no \
  --library-bundle base \
  --entry-symbol _start \
  --link-arg -T \
  --link-arg kernel.ld \
  src/main.rn \
  -o kernel.bin
```

If the linker script is not in the current working directory, pass an absolute
path instead.

## Mental Model

Keep these axes separate:

- `entry = "none"` or `--runtime-entry none` is about startup ownership
- `libc = false` or `--runtime-libc no` is about libc linkage
- `bundle = "base"` or `--library-bundle base` is about library surface
- `--entry-symbol _start` is about the final linker entry symbol
- `build.rn` or `--link-arg` is where linker-script policy belongs

Once you keep those decisions orthogonal, freestanding builds stop feeling
special-cased and become ordinary explicit toolchain configuration.
